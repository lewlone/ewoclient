//! M4 asset bake: client.jar → texture-array layers + per-state render data.
//!
//! Unlike M2 (full cubes only), M4 resolves the **general block model**:
//! blockstate variants (with x/y rotation) or multipart (with `when`
//! conditions), model parent chains, arbitrary `elements` (from/to boxes),
//! per-face uv / texture / cullface / tintindex, element rotation
//! (origin/axis/angle/rescale), and `shade`/`ambientocclusion` flags. Output
//! is either a fast-path `Cube` (a full opaque 16³ box, kept for cheap
//! face-culling + AO) or a `Model` — a baked quad list.
//!
//! Deferred (documented, not done): uvlock (minor texture-rotation cosmetic),
//! true per-biome tint (a fixed plains color is baked from the colormap
//! centers — see `grass_tint`/`foliage_tint`), animated-texture frame
//! ticking, and greedy meshing (conflicts with per-vertex AO).

use std::collections::HashMap;
use std::io::Read;
use std::path::Path;

use crate::read_json_file;

pub const TEX_SIZE: u32 = 16;

/// `BakedAssets::particle_layer` sentinel — this state has no particle sprite,
/// so a block-break burst draws nothing rather than an arbitrary texture.
pub const NO_PARTICLE_LAYER: u16 = u16::MAX;

/// `ModelBakery.DESTROY_STAGE_COUNT` — `block/destroy_stage_0` … `_9` (M81).
pub const DESTROY_STAGE_COUNT: usize = 10;

/// Face order used across the bake + mesher:
/// 0 up(+Y) 1 down(-Y) 2 north(-Z) 3 south(+Z) 4 west(-X) 5 east(+X).
pub const FACE_NAMES: [&str; 6] = ["up", "down", "north", "south", "west", "east"];
const FACE_NORMALS: [[f32; 3]; 6] = [
    [0.0, 1.0, 0.0],
    [0.0, -1.0, 0.0],
    [0.0, 0.0, -1.0],
    [0.0, 0.0, 1.0],
    [-1.0, 0.0, 0.0],
    [1.0, 0.0, 0.0],
];

/// One textured quad in block-local space (0..1), baked from a model face.
#[derive(Clone, Debug)]
pub struct Quad {
    pub verts: [[f32; 3]; 4],
    pub uv: [[f32; 2]; 4],
    /// Pre-tinted texture layer (legacy path: synthetic / no-biome worlds).
    pub layer: u16,
    /// Raw (untinted) texture layer for the M14 biome path — equals `layer`
    /// for untinted faces. Used with a dynamic per-vertex tint color.
    pub raw_layer: u16,
    /// Face index (0..5) this quad is culled against when that neighbor is a
    /// full opaque cube, or -1 to never cull.
    pub cull: i8,
    /// Face index for directional shading.
    pub dir: u8,
    /// Which biome color this quad's `tintindex` layer draws (biome path).
    pub tint: TintSource,
    /// Apply directional face shading (false for plants/torches).
    pub shade: bool,
}

/// Per-state render classification, indexed by global state id.
#[derive(Clone, Debug)]
pub enum RenderKind {
    Invisible,
    /// Full opaque 16³ cube — [up,down,north,south,west,east] pre-tinted layers,
    /// their raw (untinted) counterparts for the biome path, and a per-face tint
    /// source. Fast path (face-cull + AO in the mesher).
    Cube {
        faces: [u16; 6],
        raw_faces: [u16; 6],
        tint: [TintSource; 6],
    },
    /// Water/lava — geometry comes from the mesher's fluid path (corner
    /// heights, not a model; vanilla hardcodes fluid rendering the same
    /// way). `level`: 0 = source, 1..7 = flowing, ≥8 = falling full block.
    /// Water goes to the translucent mesh (texture alpha 180), lava to the
    /// opaque mesh at full bright.
    Fluid {
        layer: u16,
        /// Raw (untinted) fluid layer for the biome water tint. Equals `layer`
        /// for lava (which is never tinted).
        raw_layer: u16,
        level: u8,
        lava: bool,
    },
    /// General model — index into `BakedAssets::models`.
    Model(u32),
}

/// Which biome color a face's `tintindex` layer draws — the metadata the M14
/// dynamic-tint mesh path reads (a faithful transcription of the decompiled
/// `BlockColors.createDefault` registrations + `BlockTintSources`, keyed by
/// block + model `tintindex`, NOT by filename). `None` means the face is never
/// biome-tinted. Present only for the biome path; the legacy pre-tinted layers
/// keep the synthetic/no-biome (demo) render byte-identical.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TintSource {
    None,
    /// `getAverageGrassColor(pos)` — grass_block, fern, short_grass, bush,
    /// potted_fern, tall grass/large fern (lower half), sugar cane, pink
    /// petals/wildflowers layer 1.
    Grass,
    /// `doubleTallGrass` UPPER half: `getAverageGrassColor(pos.below())`.
    GrassBelow,
    /// `getAverageFoliageColor(pos)` — oak/jungle/acacia/dark_oak leaves, vine,
    /// mangrove leaves.
    Foliage,
    /// `getAverageDryFoliageColor(pos)` — leaf_litter.
    DryFoliage,
    /// `getAverageWaterColor(pos)` — the water fluid.
    Water,
    /// A constant color, NOT a biome colormap — spruce/birch leaves
    /// (`BlockTintSources.constant`). Stored opaque `[r,g,b]`.
    Constant([u8; 3]),
}

const fn rgb_of(argb: i32) -> [u8; 3] {
    [
        ((argb >> 16) & 0xFF) as u8,
        ((argb >> 8) & 0xFF) as u8,
        (argb & 0xFF) as u8,
    ]
}

/// `FoliageColor.FOLIAGE_EVERGREEN` (-10380959) — the spruce-leaves constant.
const SPRUCE_LEAF: TintSource = TintSource::Constant(rgb_of(-10380959));
/// `FoliageColor.FOLIAGE_BIRCH` (-8345771) — the birch-leaves constant.
const BIRCH_LEAF: TintSource = TintSource::Constant(rgb_of(-8345771));

/// The per-`tintindex` tint sources for a block, transcribed from
/// `BlockColors.createDefault`. The model face's `tintindex` selects the layer;
/// a tintindex past the list (or an untinted block) is `None`. `block` is the
/// short id (no `minecraft:`).
fn block_color_layers(block: &str) -> &'static [TintSource] {
    use TintSource::*;
    match block {
        // grass()/doubleTallGrass()/grassBlock()/sugarCane() → grass resolver.
        // (large_fern/tall_grass upper-half → GrassBelow is applied per-state.)
        "large_fern" | "tall_grass" => &[Grass],
        "fern" | "short_grass" | "potted_fern" | "bush" => &[Grass],
        "grass_block" => &[Grass],
        "sugar_cane" => &[Grass],
        // pink_petals/wildflowers: [BLANK, grass()] — layer 1 is the tinted one.
        "pink_petals" | "wildflowers" => &[None, Grass],
        // Constant leaves — NOT the foliage colormap.
        "spruce_leaves" => &[SPRUCE_LEAF],
        "birch_leaves" => &[BIRCH_LEAF],
        // foliage() resolver.
        "oak_leaves" | "jungle_leaves" | "acacia_leaves" | "dark_oak_leaves" | "vine"
        | "mangrove_leaves" => &[Foliage],
        // dryFoliage() resolver.
        "leaf_litter" => &[DryFoliage],
        // water()/waterParticles() → water resolver (the fluid path handles
        // WATER itself; WATER_CAULDRON is a model face).
        "water_cauldron" => &[Water],
        _ => &[],
    }
}

/// Resolve a face's tint source from its `tintindex` value + the block's
/// layers, applying the tall-grass UPPER → `GrassBelow` rule.
fn resolve_tint_source(layers: &[TintSource], tintindex: Option<i64>, upper_half: bool) -> TintSource {
    let Some(ti) = tintindex else {
        return TintSource::None;
    };
    let src = layers.get(ti as usize).copied().unwrap_or(TintSource::None);
    if upper_half && src == TintSource::Grass {
        TintSource::GrassBelow
    } else {
        src
    }
}

/// A model face's `tintindex` value (the layer selector), if present.
fn tintindex_of(face: &serde_json::Value) -> Option<i64> {
    face.get("tintindex").and_then(|t| t.as_i64())
}

/// Per-state tint metadata threaded through the baker.
#[derive(Clone, Copy)]
struct TintInfo {
    /// Legacy filename-based pre-tint input (kept so the demo is byte-identical).
    foliage: bool,
    /// This block's `BlockColors` tint layers, selected by a face's `tintindex`.
    layers: &'static [TintSource],
    /// Tall-grass / large-fern UPPER half → grass samples `pos.below()`.
    upper_half: bool,
}

pub struct BakedAssets {
    /// The language map (M50) — `en_us.json` **after** the deprecation pass,
    /// which is what `ClientLanguage.loadFrom` produces and what every key
    /// lookup in the client resolves against.
    pub lang: crate::lang::Language,
    /// Every item's English display name, for tooltips (M40). Keyed by full
    /// registry name; an item whose language key is missing has no entry.
    pub item_names: HashMap<String, String>,
    /// Enchantment display strings and the two tags that colour and order
    /// their tooltip lines (M42). The *registry* half is runtime data and
    /// lives on the session.
    pub enchantment_text: crate::enchantments::EnchantmentText,
    /// `misc/enchanted_glint_item.png` (M43). `None` means no glint is drawn
    /// rather than an invented shimmer.
    pub glint: Option<DecodedImage>,
    /// `misc/enchanted_glint_armor.png` (M50) — a **different sheet** from the
    /// item one above, which is the first of the three things that separate a
    /// worn piece's foil from a held stack's. `None` means no armour glint.
    pub armor_glint: Option<DecodedImage>,
    /// `misc/forcefield.png` — the world-border wall (M80). `None` means the
    /// wall is not drawn at all, rather than an invented sheet.
    pub forcefield: Option<DecodedImage>,
    /// Armour layer definitions and their 64x32 sheets (M46).
    pub equipment: crate::equipment::EquipmentAssets,
    /// Trim pattern sources + palettes; the sprites themselves are permuted on
    /// demand (M48).
    pub trims: crate::equipment::TrimAssets,
    pub render: Vec<RenderKind>,
    pub models: Vec<Vec<Quad>>,
    /// Per-state full-cube collision flag — true for a `Cube` OR a `Model`
    /// with a full 16³ element (grass_block renders as a Model because of its
    /// overlay element, but collides as a solid cube). Drives the client's
    /// M3 full-cube collision, independent of the render fast-path.
    pub solid: Vec<bool>,
    /// Per block state: whether its **fluid state is water** — `isWaterAt`
    /// (M30).
    ///
    /// True for water itself and for any waterlogged block, which is the
    /// distinction that matters: a conduit's activation scan requires water in
    /// all 27 cells around it, and a waterlogged slab or stair counts. Reading
    /// only `RenderKind::Fluid` would refuse to activate a conduit inside a
    /// perfectly legal frame.
    pub water: Vec<bool>,
    /// Per-state collision boxes in block-local `0..1`
    /// (`[minx,miny,minz,maxx,maxy,maxz]`); empty = the block has no
    /// collision. A `solid` state is the unit cube. Non-full-cube shapes come
    /// from the model geometry for a curated set of families (see
    /// `model_collision`) — slabs, stairs, fences, … — so a player can stand
    /// on a slab and can't walk through a fence.
    pub collide: Vec<Vec<[f32; 6]>>,
    /// Per-state light emission 0..15 (`torch` = 14, `glowstone` = 15).
    /// Extracted from the decompile by `tools/gen_block_light.py`; see
    /// [`crate::block_light`].
    pub emission: Vec<u8>,
    /// Per-state light dampening 0..15 — vanilla `BlockBehaviour
    /// .getLightDampening`: `isSolidRender ? 15 : propagatesSkylightDown ? 0 : 1`.
    /// Substituting what the bake already knows, that is
    /// `canOcclude && full_cube ? 15 : (!full_cube && !fluid ? 0 : 1)`, which is
    /// why glass/leaves/water come out at 1 rather than 0 or 15. The light
    /// engine's per-step cost is `max(1, dampening)`.
    pub dampening: Vec<u8>,
    /// Per-state bitmask of the six faces the block's occlusion shape fully
    /// covers, in [`FACE_DIRS`] order. Vanilla blocks light across a face when
    /// the two adjacent occlusion shapes together cover it
    /// (`LightEngine.getLightDampeningInto` → 16, i.e. "no light passes"), which
    /// is what makes a stair or slab cast a proper shadow even though its
    /// `dampening` is 0. Non-full-cube shapes only — a full cube is already
    /// handled by dampening 15.
    pub face_occludes: Vec<u8>,
    /// RGBA8 16×16 texels per layer (sRGB).
    pub layers: Vec<Vec<u8>>,
    pub layer_names: Vec<String>,
    /// Per block state: the texture-array layer vanilla's
    /// `getParticleMaterial(state).sprite()` samples for a block-break shard
    /// (M37) — the model's `#particle` texture slot, resolved through the same
    /// parent chain the faces use. `NO_PARTICLE_LAYER` when the block has no
    /// model, no `particle` slot, or its texture is missing from the jar, in
    /// which case no shard is drawn rather than an invented one.
    pub particle_layer: Vec<u16>,
    /// Baked plains-biome tint colors (colormap centers). Kept for the legacy
    /// pre-tinted path (synthetic / no-biome worlds → demo byte-identical).
    pub grass_tint: [u8; 3],
    pub foliage_tint: [u8; 3],
    /// Full biome colormaps (`65536` ARGB ints indexed `y<<8|x`) for the M14
    /// per-biome tint. Empty if the jar lacks the PNG — the biome engine then
    /// falls back to the vanilla default map color for that channel.
    pub grass_colormap: Vec<i32>,
    pub foliage_colormap: Vec<i32>,
    pub dry_foliage_colormap: Vec<i32>,
    /// The vanilla bitmap font (ascii.png) for nametags/HUD text. `None`
    /// only if the jar somehow lacks it — callers degrade to no text.
    pub font: Option<BakedFont>,
    /// Animated texture layers (water/lava/…): all frames pre-tinted, with
    /// the `.mcmeta` frame order + timing. The layer itself holds frame 0;
    /// the renderer re-uploads frames on the 20 Hz tick.
    pub animations: Vec<AnimatedLayer>,
    /// Every mob skin the entity pass's model registry wants, keyed by the
    /// registry's texture names (`rewo_gpu::mobs::MobDef::textures`).
    /// Missing jar entries are simply absent — those mobs render as
    /// capsules. "player" is the default wide Steve skin (offline servers
    /// carry no skin data, so every player wears it until online-mode
    /// profile fetching lands).
    pub mob_textures: Vec<MobTexture>,
    /// Vanilla's metadata-driven alternates (M64) — the cat's ten other coats,
    /// the wolf's eight other variants and their tame sheets, and so on. Same
    /// shape as [`MobTexture`] plus the variant id that addresses it, because
    /// they ride M57b's ETF variant machinery into the same atlas.
    pub mob_variant_textures: Vec<MobVariantTexture>,
    /// Held-item models (M22): every resolvable item's quads + textures.
    pub held_items: crate::held_items::HeldItems,
    /// In-game HUD sprites (hotbar / hearts / hunger / crosshair) from the
    /// jar's `gui/sprites/hud/`. `None` degrades to no HUD.
    pub hud: Option<HudSprites>,
    /// The locator bar's sprites + style table (M83). `None` degrades to no
    /// locator bar and leaves the rest of the HUD alone — see
    /// [`LocatorSprites`].
    pub locator: Option<LocatorSprites>,
    /// The container screen's textures (M35). `None` degrades to no screen,
    /// exactly as a missing HUD sprite degrades to no HUD.
    pub container: Option<ContainerSprites>,
    /// Sun + 8 moon-phase textures from `environment/celestial/`. `None` if the
    /// jar lacks them — the sky then renders without sun/moon (M12).
    pub celestial: Option<CelestialTextures>,
    /// `environment/end_sky.png`, the texture `SkyRenderer.renderEndSky` tiles
    /// over the End skybox cube (M16). `None` if the jar lacks it — the End
    /// then draws no sky at all rather than an invented flat colour.
    pub end_sky: Option<DecodedImage>,
    /// `ModelBakery.BREAKING_LOCATIONS` — the ten block-break crack overlays,
    /// stage 0 first (M81). `None` when the jar has none, which draws no
    /// crumbling at all rather than a partial set.
    pub destroy_stages: Option<[DecodedImage; DESTROY_STAGE_COUNT]>,
    /// `entity/end_portal/end_portal.png` — Sampler1 of the end-portal shader
    /// (M32). `None` if the jar lacks it, in which case no portal draws.
    pub end_portal: Option<DecodedImage>,
    /// `environment/rain.png` and `snow.png` (M33). `None` means that kind of
    /// precipitation draws nothing rather than an invented streak.
    pub rain: Option<DecodedImage>,
    pub snow: Option<DecodedImage>,
    /// The particle sprites (M37). `None` means particles draw nothing rather
    /// than an invented sprite.
    pub particles: Option<ParticleSprites>,
    /// `environment/clouds.png` (M33) — a *map*, one texel per 12x12x4 cell,
    /// never sampled as a surface. `None` means no clouds at all.
    pub clouds: Option<DecodedImage>,
    pub stats: BakeStats,
}

/// The mob-texture key a vanilla texture path maps to — e.g.
/// `"entity/cow/cow_temperate.png"` → `"cow"`. The join [`crate::etf`] uses
/// to attach a pack's random-entity rules to the textures the entity pass
/// actually bakes.
pub fn mob_texture_key(rel_path: &str) -> Option<&'static str> {
    MOB_TEXTURE_SPECS
        .iter()
        .find(|(_, path, _, _)| *path == rel_path)
        .map(|(key, _, _, _)| *key)
}

/// Every baked mob texture as `(key, jar-relative path, w, h)` — what
/// [`crate::etf`] walks to find a pack's `_e` emissive siblings.
pub fn mob_texture_specs() -> impl Iterator<Item = (&'static str, &'static str, u32, u32)> {
    MOB_TEXTURE_SPECS.iter().copied()
}

/// The vanilla pixel dimensions of a mob texture, by key.
pub fn mob_texture_size(key: &str) -> Option<(u32, u32)> {
    MOB_TEXTURE_SPECS
        .iter()
        .find(|(k, _, _, _)| *k == key)
        .map(|(_, _, w, h)| (*w, *h))
}

/// Decode an entity-sized PNG (any colour type) to `(RGBA8, w, h)` —
/// [`crate::etf`] needs it for a pack's alternate textures.
pub fn decode_entity_png(bytes: &[u8]) -> Option<(Vec<u8>, u32, u32)> {
    decode_png_any(bytes)
}

/// One decoded mob skin: RGBA8 + dimensions, keyed for the model registry.
pub struct MobTexture {
    pub key: &'static str,
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
}

/// One of vanilla's own metadata-driven alternates (M64).
pub struct MobVariantTexture {
    /// The mob-texture key it varies (`"cat"`, `"wolf"`, …).
    pub key: &'static str,
    /// `rewo_data::mob_variants` variant id — always in the reserved high
    /// band, so it cannot collide with a pack's ETF rule index.
    pub index: u16,
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
}

/// Every mob texture the entity model registry can use: (key, jar path,
/// width, height). Keep the keys in sync with `rewo_gpu::mobs::MOBS` —
/// a key absent here (or missing from the jar) degrades that mob to the
/// capsule fallback, never an error.
const MOB_TEXTURE_SPECS: &[(&str, &str, u32, u32)] = &[
    ("player", "entity/player/wide/steve.png", 64, 64),
    ("zombie", "entity/zombie/zombie.png", 64, 64),
    ("zombie_villager", "entity/zombie_villager/zombie_villager.png", 64, 64),
    ("husk", "entity/zombie/husk.png", 64, 64),
    ("drowned", "entity/zombie/drowned.png", 64, 64),
    ("drowned_outer", "entity/zombie/drowned_outer_layer.png", 64, 64),
    ("skeleton", "entity/skeleton/skeleton.png", 64, 32),
    ("stray", "entity/skeleton/stray.png", 64, 32),
    ("stray_overlay", "entity/skeleton/stray_overlay.png", 64, 32),
    ("bogged", "entity/skeleton/bogged.png", 64, 32),
    ("bogged_overlay", "entity/skeleton/bogged_overlay.png", 64, 32),
    ("parched", "entity/skeleton/parched.png", 64, 64),
    ("wither_skeleton", "entity/skeleton/wither_skeleton.png", 64, 32),
    ("creeper", "entity/creeper/creeper.png", 64, 32),
    ("spider", "entity/spider/spider.png", 64, 32),
    ("cave_spider", "entity/spider/cave_spider.png", 64, 32),
    ("enderman", "entity/enderman/enderman.png", 64, 32),
    ("slime", "entity/slime/slime.png", 64, 32),
    ("magma_cube", "entity/slime/magmacube.png", 64, 64),
    ("cow", "entity/cow/cow_temperate.png", 64, 64),
    ("mooshroom", "entity/cow/mooshroom_red.png", 64, 64),
    ("pig", "entity/pig/pig_temperate.png", 64, 64),
    ("sheep", "entity/sheep/sheep.png", 64, 32),
    ("sheep_wool", "entity/sheep/sheep_wool.png", 64, 32),
    ("chicken", "entity/chicken/chicken_temperate.png", 64, 32),
    ("wolf", "entity/wolf/wolf.png", 64, 32),
    ("squid", "entity/squid/squid.png", 64, 32),
    ("glow_squid", "entity/squid/glow_squid.png", 64, 32),
    ("rabbit", "entity/rabbit/rabbit_brown.png", 64, 64),
    ("villager", "entity/villager/villager.png", 64, 64),
    ("wandering_trader", "entity/wandering_trader/wandering_trader.png", 64, 64),
    ("witch", "entity/witch/witch.png", 64, 128),
    ("pillager", "entity/illager/pillager.png", 64, 64),
    ("vindicator", "entity/illager/vindicator.png", 64, 64),
    ("evoker", "entity/illager/evoker.png", 64, 64),
    ("illusioner", "entity/illager/illusioner.png", 64, 64),
    ("vex", "entity/illager/vex.png", 32, 32),
    ("phantom", "entity/phantom/phantom.png", 64, 64),
    ("guardian", "entity/guardian/guardian.png", 64, 64),
    ("elder_guardian", "entity/guardian/guardian_elder.png", 64, 64),
    ("shulker", "entity/shulker/shulker.png", 64, 64),
    ("silverfish", "entity/silverfish/silverfish.png", 64, 32),
    ("endermite", "entity/endermite/endermite.png", 64, 32),
    ("blaze", "entity/blaze/blaze.png", 64, 32),
    ("ghast", "entity/ghast/ghast.png", 128, 64),
    ("piglin", "entity/piglin/piglin.png", 64, 64),
    ("piglin_brute", "entity/piglin/piglin_brute.png", 64, 64),
    ("zombified_piglin", "entity/piglin/zombified_piglin.png", 64, 64),
    ("hoglin", "entity/hoglin/hoglin.png", 128, 64),
    ("zoglin", "entity/hoglin/zoglin.png", 128, 64),
    ("strider", "entity/strider/strider.png", 64, 128),
    ("bat", "entity/bat/bat.png", 32, 32),
    ("cat", "entity/cat/cat_tabby.png", 64, 32),
    ("ocelot", "entity/cat/ocelot.png", 64, 32),
    ("fox", "entity/fox/fox.png", 48, 32),
    ("goat", "entity/goat/goat.png", 64, 64),
    ("bee", "entity/bee/bee.png", 64, 64),
    ("frog", "entity/frog/frog_temperate.png", 48, 48),
    ("tadpole", "entity/tadpole/tadpole.png", 16, 16),
    ("armadillo", "entity/armadillo/armadillo.png", 64, 64),
    ("axolotl", "entity/axolotl/axolotl_lucy.png", 64, 64),
    ("dolphin", "entity/dolphin/dolphin.png", 64, 64),
    ("turtle", "entity/turtle/turtle.png", 128, 64),
    ("cod", "entity/fish/cod.png", 32, 32),
    ("salmon", "entity/fish/salmon.png", 32, 32),
    ("pufferfish", "entity/fish/pufferfish.png", 32, 32),
    ("tropical_fish", "entity/fish/tropical_a.png", 32, 32),
    ("panda", "entity/panda/panda.png", 64, 64),
    ("polar_bear", "entity/bear/polarbear.png", 128, 64),
    ("camel", "entity/camel/camel.png", 128, 128),
    ("llama", "entity/llama/llama_creamy.png", 128, 64),
    ("parrot", "entity/parrot/parrot_red_blue.png", 32, 32),
    ("horse", "entity/horse/horse_brown.png", 64, 64),
    ("donkey", "entity/horse/donkey.png", 64, 64),
    ("mule", "entity/horse/mule.png", 64, 64),
    ("skeleton_horse", "entity/horse/horse_skeleton.png", 64, 64),
    ("zombie_horse", "entity/horse/horse_zombie.png", 64, 64),
    ("snow_golem", "entity/snow_golem/snow_golem.png", 64, 64),
    ("iron_golem", "entity/iron_golem/iron_golem.png", 128, 128),
    ("allay", "entity/allay/allay.png", 32, 32),
    ("warden", "entity/warden/warden.png", 128, 128),
    ("sniffer", "entity/sniffer/sniffer.png", 192, 192),
    ("breeze", "entity/breeze/breeze.png", 32, 32),
    ("breeze_wind", "entity/breeze/breeze_wind.png", 128, 128),
    ("creaking", "entity/creaking/creaking.png", 64, 64),
    ("ravager", "entity/illager/ravager.png", 128, 128),
    ("wither", "entity/wither/wither.png", 64, 64),
    ("ender_dragon", "entity/enderdragon/dragon.png", 256, 256),
    ("happy_ghast", "entity/ghast/happy_ghast.png", 128, 128),
    ("copper_golem", "entity/copper_golem/copper_golem.png", 64, 64),
    ("nautilus", "entity/nautilus/nautilus.png", 128, 128),
    ("zombie_nautilus", "entity/nautilus/zombie_nautilus.png", 128, 128),
    // Emissive overlay layers (M52) — the textures vanilla's `EyesLayer` /
    // `LivingEntityEmissiveLayer` re-render the model with. Each is the
    // same size as its mob's base texture, which is what lets the emissive
    // pass reuse the base quads' UVs (`rewo_gpu::mobs::emissive_layers`).
    ("spider_eyes", "entity/spider/spider_eyes.png", 64, 32),
    ("enderman_eyes", "entity/enderman/enderman_eyes.png", 64, 32),
    ("phantom_eyes", "entity/phantom/phantom_eyes.png", 64, 64),
    ("breeze_eyes", "entity/breeze/breeze_eyes.png", 32, 32),
    ("creaking_eyes", "entity/creaking/creaking_eyes.png", 64, 64),
    ("copper_golem_eyes", "entity/copper_golem/copper_golem_eyes.png", 64, 64),
    ("warden_bioluminescent", "entity/warden/warden_bioluminescent_layer.png", 128, 128),
    ("warden_pulsating_1", "entity/warden/warden_pulsating_spots_1.png", 128, 128),
    ("warden_pulsating_2", "entity/warden/warden_pulsating_spots_2.png", 128, 128),
    ("warden_heart", "entity/warden/warden_heart.png", 128, 128),
    // M68 — the two render layers whose sheet is a *second* texture on a mesh
    // Rewo already builds.
    //
    // `SheepWoolUndercoatLayer`: 26.x's second fleece. Its model layer is
    // `SHEEP_WOOL_UNDERCOAT` -> **`sheepBodyLayer`**, i.e.
    // `SheepModel.createBodyLayer()` — the *body* mesh at deformation NONE,
    // not `SheepFurModel.createFurLayer()`'s inflated one, even though the
    // class wrapping it is `SheepFurModel`. The sheet's own layout settles it:
    // its head block is 12 px wide over 8 rows (w=6, d=8), which is the body
    // head's box unwrap, where the fur sheet's is 12 px over 6 (d=6).
    ("sheep_wool_undercoat", "entity/sheep/sheep_wool_undercoat.png", 64, 32),
    // `TropicalFishRenderer` swaps `this.model` between two meshes and the
    // texture with it, so `tropical_b.png` is not an alternate skin for one
    // fish — it is the sheet belonging to the *large* body plan.
    ("tropical_fish_large", "entity/fish/tropical_b.png", 32, 32),
    // `TropicalFishPatternLayer`'s twelve sheets, six per body plan. Entry 1
    // of each six is the baked base (KOB / FLOPPER); the other five ride the
    // variant band (`crate::mob_variants`), which is what lets one pattern
    // slot address all six.
    ("tropical_fish_pattern_a", "entity/fish/tropical_a_pattern_1.png", 32, 32),
    ("tropical_fish_pattern_b", "entity/fish/tropical_b_pattern_1.png", 32, 32),
];

/// A decoded GUI sprite: RGBA8 + pixel dimensions.
pub struct HudSprite {
    pub rgba: Vec<u8>,
    pub w: u32,
    pub h: u32,
}

/// A decoded RGBA8 image + its pixel dimensions (celestial textures).
pub struct DecodedImage {
    pub rgba: Vec<u8>,
    pub w: u32,
    pub h: u32,
}

/// The clear-weather Overworld celestial textures, loaded from the user's own
/// client jar (never redistributed). 26.2 moved these under `celestial/`
/// (`SkyRenderer` builds the `CELESTIALS` atlas from `sun` + `moon/<phase>`).
/// The `moons` array is indexed by `MoonPhase.index()` (0 = full moon), which
/// is the order `buildMoonPhases` iterates `MoonPhase.values()`.
pub struct CelestialTextures {
    pub sun: DecodedImage,
    pub moons: [DecodedImage; 8],
}

/// Moon-phase texture basenames in `MoonPhase` declaration/index order
/// (`MoonPhase.values()`), so `moons[i]` is phase `i`.
const MOON_PHASE_FILES: [&str; 8] = [
    "full_moon",       // 0
    "waning_gibbous",  // 1
    "third_quarter",   // 2
    "waning_crescent", // 3
    "new_moon",        // 4
    "waxing_crescent", // 5
    "first_quarter",   // 6
    "waxing_gibbous",  // 7
];

/// The HUD sprite set (1.20.2+ individual sprite files).
pub struct HudSprites {
    pub hotbar: HudSprite,
    pub selection: HudSprite,
    pub crosshair: HudSprite,
    pub heart_full: HudSprite,
    pub heart_half: HudSprite,
    pub heart_container: HudSprite,
    pub food_full: HudSprite,
    pub food_half: HudSprite,
    pub food_empty: HudSprite,
    /// `ExperienceBar.EXPERIENCE_BAR_BACKGROUND_SPRITE` (M79) — 182×5.
    pub experience_bar_background: HudSprite,
    /// `ExperienceBar.EXPERIENCE_BAR_PROGRESS_SPRITE` (M79) — also 182×5, and
    /// blitted as a **sub-rectangle** whose width is the filled part. Vanilla
    /// computes that width as `(int)(progress * 183.0F)` against a 182-wide
    /// background, so a full bar overhangs its own frame by one pixel. The
    /// discrepancy is vanilla's and is transcribed rather than tidied.
    pub experience_bar_progress: HudSprite,
}

/// One `assets/minecraft/waypoint_style/*.json` (M83).
///
/// `near_distance`/`far_distance` default to 128/332 and the sprite list is
/// required and non-empty; the names are `Identifier`s that
/// `WaypointStyle`'s canonical constructor prefixes with
/// `hud/locator_bar_dot/`.
pub struct WaypointStyleAsset {
    /// The registry key, e.g. `minecraft:default`.
    pub key: String,
    pub near_distance: i32,
    pub far_distance: i32,
    /// Indices into [`LocatorSprites::dots`], in the file's order.
    pub sprites: Vec<u16>,
}

/// The locator bar's sprite set + style table (M83).
///
/// Deliberately a **separate** struct from [`HudSprites`] rather than more
/// fields on it: `bake_hud` is all-or-nothing (`?` on every sprite), and a
/// resource pack or a version that dropped one dot would take the hotbar and
/// the hearts down with the locator bar.
pub struct LocatorSprites {
    /// 12×5 — a nine-slice, **not** the 182×5 the bar blits.
    pub background: HudSprite,
    /// 7×10 — two 7×5 animation frames stacked, per its `.mcmeta`.
    pub arrow_up: HudSprite,
    pub arrow_down: HudSprite,
    /// Every 9×9 dot named by any style, deduplicated, in first-seen order.
    pub dots: Vec<HudSprite>,
    pub styles: Vec<WaypointStyleAsset>,
}

/// One animated texture-array layer, from an N-frame vertical strip +
/// its `.mcmeta` ({"animation": {"frametime": T, "frames": [...]?}}).
pub struct AnimatedLayer {
    pub layer: u16,
    /// 16×16 RGBA frames, in strip order (tint already applied).
    pub frames: Vec<Vec<u8>>,
    /// Playback order (indices into `frames`); sequential when the mcmeta
    /// has no explicit list (water); lava ping-pongs.
    pub order: Vec<u32>,
    /// Game ticks per animation frame.
    pub frametime: u32,
}

/// The legacy bitmap font: a 16×16 grid of glyph cells covering code points
/// 0..256 (ASCII names only need rows 2..8). Advances derived from the
/// bitmap the way vanilla's legacy provider does (rightmost lit column + 1
/// spacing px; space = 4).
pub struct BakedFont {
    /// RGBA8 atlas, `atlas_size`² texels (sRGB; white glyphs on alpha).
    pub atlas: Vec<u8>,
    pub atlas_size: u32,
    /// Glyph cell edge in px (vanilla: 8).
    pub cell: u32,
    /// Horizontal advance per code point, in font px.
    pub advance: [u8; 256],
    /// A guaranteed-opaque-white texel (patched into the blank space cell),
    /// for drawing solid quads through the same pipeline. (u, v) in texels.
    pub white_texel: (u32, u32),
}

#[derive(Default, Debug)]
pub struct BakeStats {
    /// States given a non-full-cube collision shape from their model.
    pub shaped_collision_states: usize,
    pub cube_states: usize,
    pub model_states: usize,
    pub fluid_states: usize,
    pub invisible_states: usize,
    pub textures: usize,
}

/// Light `(emission, dampening)` for a fluid state.
///
/// Fluids are classified by name and `continue` before the shared per-state
/// light block, so this reproduces the two facts that block would otherwise
/// assign, read from the same generated tables it uses:
///   * emission from [`block_light::EMISSION`] (lava is pinned to 15; water is
///     absent → 0), and
///   * dampening from vanilla `BlockBehaviour.getLightDampening`
///     (`isSolidRender ? 15 : propagatesSkylightDown ? 0 : 1`). A fluid is
///     never a solid render cube and never in `DAMPENING_OVERRIDE`, so the
///     result is purely `propagatesSkylightDown ? 0 : 1`; both fluids carry
///     `SKY_PROPAGATE = 0` (`LiquidBlock.propagatesSkylightDown` = false) → 1.
fn fluid_light(lava: bool) -> (u8, u8) {
    let name = if lava { "minecraft:lava" } else { "minecraft:water" };
    let emission = crate::block_light::EMISSION
        .iter()
        .find(|&&(b, _)| b == name)
        .map_or(0, |&(_, e)| e);
    // 1 = always propagates; anything else (0 = never, 2 = only-if-not-
    // waterlogged, which fluids never carry) means dampening 1.
    let propagates = matches!(
        crate::block_light::SKY_PROPAGATE.iter().find(|&&(b, _)| b == name),
        Some(&(_, 1))
    );
    let dampening = if propagates { 0 } else { 1 };
    (emission, dampening)
}

/// Every item id the jar ships, from `assets/minecraft/items/*.json` — the
/// same list the bake walks, exposed so an oracle can count it independently
/// of a bake it is grading.
pub fn jar_item_ids(client_jar: &Path) -> Result<Vec<String>, String> {
    let f = std::fs::File::open(client_jar).map_err(|e| format!("open jar: {e}"))?;
    let zip = zip::ZipArchive::new(std::io::BufReader::new(f)).map_err(|e| format!("zip: {e}"))?;
    let mut out: Vec<String> = zip
        .file_names()
        .filter_map(|p| {
            p.strip_prefix("assets/minecraft/items/")
                .and_then(|r| r.strip_suffix(".json"))
                .filter(|r| !r.contains('/'))
                .map(str::to_string)
        })
        .collect();
    out.sort();
    Ok(out)
}

/// Read one text entry out of a client jar. For oracles that need to look at
/// a raw asset without standing up the whole bake.
pub fn jar_text(client_jar: &Path, path: &str) -> Option<String> {
    let f = std::fs::File::open(client_jar).ok()?;
    let mut z = zip::ZipArchive::new(std::io::BufReader::new(f)).ok()?;
    let mut s = String::new();
    z.by_name(path).ok()?.read_to_string(&mut s).ok()?;
    Some(s)
}

/// Every block whose blockstate models all bake to **no geometry** (M25).
///
/// A block entity renders as an ordinary block model plus a
/// `BlockEntityRenderer`; where the model has no `elements`, the renderer *is*
/// the block, and a client without one draws nothing. This walks each
/// blockstate's referenced models through their `parent` chain and reports the
/// blocks that never reach an `elements` array — which is the measurement
/// `rewo_world::block_entities`' classification rests on.
///
/// Air, water, lava, barriers and the markers come out of this too; they are
/// *correctly* invisible, and the caller filters them.
pub fn blocks_without_geometry(client_jar: &Path) -> Result<Vec<String>, String> {
    let file = std::fs::File::open(client_jar)
        .map_err(|e| format!("open {}: {e}", client_jar.display()))?;
    let mut jar = zip::ZipArchive::new(std::io::BufReader::new(file))
        .map_err(|e| format!("zip {}: {e}", client_jar.display()))?;

    const STATES: &str = "assets/minecraft/blockstates/";
    let names: Vec<String> = (0..jar.len())
        .filter_map(|i| jar.by_index(i).ok().map(|f| f.name().to_string()))
        .filter(|n| n.starts_with(STATES) && n.ends_with(".json"))
        .collect();

    fn json_at(
        jar: &mut zip::ZipArchive<std::io::BufReader<std::fs::File>>,
        path: &str,
    ) -> Option<serde_json::Value> {
        let mut s = String::new();
        jar.by_name(path).ok()?.read_to_string(&mut s).ok()?;
        serde_json::from_str(&s).ok()
    }

    /// Collect every `"model"` string anywhere in a blockstate — variants and
    /// multipart alike, so a block that only draws through one multipart case
    /// still counts as having geometry.
    fn collect(v: &serde_json::Value, out: &mut Vec<String>) {
        match v {
            serde_json::Value::Object(m) => {
                if let Some(serde_json::Value::String(s)) = m.get("model") {
                    out.push(s.rsplit(':').next().unwrap_or(s).to_string());
                }
                for x in m.values() {
                    collect(x, out);
                }
            }
            serde_json::Value::Array(a) => {
                for x in a {
                    collect(x, out);
                }
            }
            _ => {}
        }
    }

    let mut empty = Vec::new();
    for path in names {
        let Some(v) = json_at(&mut jar, &path) else {
            continue;
        };
        let mut models = Vec::new();
        collect(&v, &mut models);
        models.sort();
        models.dedup();
        if models.is_empty() {
            continue;
        }
        let mut has_geometry = false;
        'model: for m in &models {
            let mut cur = m.clone();
            // Bounded like the item-model chain: a cycle must terminate.
            for _ in 0..MAX_PARENT_DEPTH {
                let Some(d) = json_at(&mut jar, &format!("assets/minecraft/models/{cur}.json"))
                else {
                    break;
                };
                if d.get("elements")
                    .and_then(|e| e.as_array())
                    .is_some_and(|a| !a.is_empty())
                {
                    has_geometry = true;
                    break 'model;
                }
                match d.get("parent").and_then(|p| p.as_str()) {
                    Some(p) => cur = p.rsplit(':').next().unwrap_or(p).to_string(),
                    None => break,
                }
            }
        }
        if !has_geometry {
            empty.push(
                path.trim_start_matches(STATES)
                    .trim_end_matches(".json")
                    .to_string(),
            );
        }
    }
    empty.sort();
    Ok(empty)
}

/// How far a block model's `parent` chain is followed before giving up.
const MAX_PARENT_DEPTH: usize = 8;

pub fn bake(client_jar: &Path, blocks_json: &Path) -> Result<BakedAssets, String> {
    let file = std::fs::File::open(client_jar)
        .map_err(|e| format!("open {}: {e}", client_jar.display()))?;
    let mut jar = zip::ZipArchive::new(std::io::BufReader::new(file))
        .map_err(|e| format!("zip {}: {e}", client_jar.display()))?;

    let blocks = read_json_file(blocks_json)?;
    let blocks = blocks.as_object().ok_or("blocks.json: not an object")?;
    let mut max_id = 0usize;
    for def in blocks.values() {
        if let Some(states) = def.get("states").and_then(|s| s.as_array()) {
            for s in states {
                if let Some(id) = s.get("id").and_then(|i| i.as_u64()) {
                    max_id = max_id.max(id as usize);
                }
            }
        }
    }

    let grass_tint = colormap_center(&mut jar, "grass").unwrap_or([124, 189, 107]);
    let foliage_tint = colormap_center(&mut jar, "foliage").unwrap_or([89, 174, 48]);
    // Full colormaps for the M14 per-biome tint (empty → default map color).
    let grass_colormap = colormap_pixels(&mut jar, "grass").unwrap_or_default();
    let foliage_colormap = colormap_pixels(&mut jar, "foliage").unwrap_or_default();
    let dry_foliage_colormap = colormap_pixels(&mut jar, "dry_foliage").unwrap_or_default();
    let font = bake_font(&mut jar);
    if font.is_none() {
        log::warn!("rewo-data: font/ascii.png missing — nametags disabled");
    }
    let mut mob_textures = Vec::with_capacity(MOB_TEXTURE_SPECS.len());
    for &(key, path, w, h) in MOB_TEXTURE_SPECS {
        match bake_entity_tex(&mut jar, path, w, h) {
            Some(rgba) => mob_textures.push(MobTexture { key, w, h, rgba }),
            None => log::warn!("rewo-data: {path} missing — {key} renders as a capsule"),
        }
    }
    // M64: vanilla's metadata-driven alternates. Each must be its base's size,
    // because it reuses the base's UVs — the same constraint M57b puts on a
    // pack's ETF alternates, and every vanilla one satisfies it by
    // construction. One that does not is dropped rather than rendered
    // scrambled.
    let mut mob_variant_textures = Vec::new();
    for (key, index, path) in crate::mob_variants::specs() {
        let Some((w, h)) = mob_texture_size(key) else {
            log::warn!("rewo-data: variant {path} names unknown mob key {key}");
            continue;
        };
        match bake_entity_tex(&mut jar, path, w, h) {
            Some(rgba) => mob_variant_textures.push(MobVariantTexture {
                key,
                index,
                w,
                h,
                rgba,
            }),
            None => log::warn!("rewo-data: {path} missing — {key} keeps its base texture there"),
        }
    }
    let hud = bake_hud(&mut jar);
    let locator = bake_locator(&mut jar);
    let container = bake_container(&mut jar);
    let lang = crate::lang::Language::load(client_jar);
    let item_names = bake_item_names(&mut jar, &lang);
    let enchantment_text = crate::enchantments::EnchantmentText::load(client_jar, &lang);
    let glint = bake_misc_texture(&mut jar, "enchanted_glint_item.png");
    // M50: `ARMOR_ENTITY_GLINT` binds its own sheet. Both live in `misc/` and
    // they are different images — the worn foil is not the item foil at a
    // different scale, it is a different texture at a different scale.
    let armor_glint = bake_misc_texture(&mut jar, "enchanted_glint_armor.png");
    let forcefield = bake_misc_texture(&mut jar, "forcefield.png");
    let equipment = crate::equipment::EquipmentAssets::load(client_jar);
    let trims = crate::equipment::TrimAssets::load(client_jar);
    if hud.is_none() {
        log::warn!("rewo-data: HUD sprites missing — no in-game HUD");
    }
    let end_sky = bake_env_texture(&mut jar, "end_sky.png");
    let rain = bake_env_texture(&mut jar, "rain.png");
    let snow = bake_env_texture(&mut jar, "snow.png");
    let particles = bake_particle_sprites(&mut jar);
    let clouds = bake_env_texture(&mut jar, "clouds.png");
    for (name, present) in [
        ("rain.png", rain.is_some()),
        ("snow.png", snow.is_some()),
        ("clouds.png", clouds.is_some()),
    ] {
        if !present {
            log::warn!("rewo-data: environment/{name} missing — that weather draws nothing");
        }
    }
    let end_portal = {
        let path = "assets/minecraft/textures/entity/end_portal/end_portal.png";
        let mut bytes = Vec::new();
        jar.by_name(path)
            .ok()
            .and_then(|mut e| e.read_to_end(&mut bytes).ok())
            .and_then(|_| decode_png_any(&bytes))
            .map(|(rgba, w, h)| DecodedImage { rgba, w, h })
    };
    if end_sky.is_none() {
        log::warn!("rewo-data: environment/end_sky.png missing — no End skybox");
    }
    let celestial = bake_celestial(&mut jar);
    if celestial.is_none() {
        log::warn!("rewo-data: celestial textures missing — no sun/moon");
    }
    let destroy_stages = bake_destroy_stages(&mut jar);
    if destroy_stages.is_none() {
        log::warn!("rewo-data: block/destroy_stage_* missing — no block-break overlay");
    }

    let mut baker = Baker {
        jar: &mut jar,
        model_cache: HashMap::new(),
        layer_index: HashMap::new(),
        layers: Vec::new(),
        layer_names: Vec::new(),
        animations: Vec::new(),
        grass_tint,
        foliage_tint,
    };

    let mut render = vec![RenderKind::Invisible; max_id + 1];
    let mut solid = vec![false; max_id + 1];
    let mut water = vec![false; max_id + 1];
    let mut collide: Vec<Vec<[f32; 6]>> = vec![Vec::new(); max_id + 1];
    let mut emission = vec![0u8; max_id + 1];
    let mut dampening = vec![0u8; max_id + 1];
    let mut face_occludes = vec![0u8; max_id + 1];
    let mut particle_layer = vec![NO_PARTICLE_LAYER; max_id + 1];
    let mut models: Vec<Vec<Quad>> = Vec::new();
    let mut stats = BakeStats::default();

    let no_occlude: std::collections::HashSet<&str> =
        crate::block_light::NO_OCCLUDE.iter().copied().collect();
    let const_emission: HashMap<&str, u8> =
        crate::block_light::EMISSION.iter().copied().collect();
    let lit_emission: HashMap<&str, u8> =
        crate::block_light::LIT_EMISSION.iter().copied().collect();
    let sky_propagate: HashMap<&str, u8> =
        crate::block_light::SKY_PROPAGATE.iter().copied().collect();
    let damp_override: HashMap<&str, u8> =
        crate::block_light::DAMPENING_OVERRIDE.iter().copied().collect();
    let state_emission: HashMap<&str, (&str, &str, &str, &[(&str, u8)])> =
        crate::block_light::STATE_EMISSION
            .iter()
            .map(|&(b, gate, gv, vp, map)| (b, (gate, gv, vp, map)))
            .collect();

    for (block_name, def) in blocks {
        let states = def
            .get("states")
            .and_then(|s| s.as_array())
            .ok_or_else(|| format!("blocks.json: {block_name} has no states"))?;
        let short = block_name.strip_prefix("minecraft:").unwrap_or(block_name);
        // Fluids have no usable blockstate models (vanilla hardcodes their
        // renderer) — classify by name, keyed on the `level` property.
        if short == "water" || short == "lava" {
            let lava = short == "lava";
            let (layer, raw_layer) = if lava {
                // Lava is never biome-tinted: raw == pre-tinted.
                let l = baker.layer_for("block/lava_still", TintKind::None);
                (l, l)
            } else {
                // Water: pre-tinted (legacy #3F76E4) + a raw copy for the biome
                // water tint path.
                (
                    baker.layer_for("block/water_still", TintKind::Water),
                    baker.layer_for("block/water_still", TintKind::None),
                )
            };
            let (Some(layer), Some(raw_layer)) = (layer, raw_layer) else {
                log::warn!("rewo-data: {short}_still texture missing — fluid invisible");
                continue;
            };
            for state in states {
                let Some(id) = state.get("id").and_then(|i| i.as_u64()) else {
                    continue;
                };
                let level = state
                    .get("properties")
                    .and_then(|p| p.get("level"))
                    .and_then(|l| l.as_str())
                    .and_then(|l| l.parse::<u8>().ok())
                    .unwrap_or(0);
                render[id as usize] = RenderKind::Fluid {
                    layer,
                    raw_layer,
                    level,
                    lava,
                };
                water[id as usize] = !lava;
                // Fluids skip the shared per-state light block below (they
                // `continue`), so assign their light here from the same tables.
                let (e, d) = fluid_light(lava);
                emission[id as usize] = e;
                dampening[id as usize] = d;
                stats.fluid_states += 1;
            }
            continue;
        }
        let bs = baker.load_blockstate(short);
        let foliage = is_foliage(short);
        for state in states {
            let Some(id) = state.get("id").and_then(|i| i.as_u64()) else {
                continue;
            };
            let props = state.get("properties").and_then(|p| p.as_object());
            let resolved = bs
                .as_ref()
                .and_then(|bs| baker.resolve_state(bs, props, foliage, short, &mut models));
            match resolved {
                Some((k @ RenderKind::Cube { .. }, is_solid)) => {
                    render[id as usize] = k;
                    solid[id as usize] = is_solid;
                    particle_layer[id as usize] = baker.particle_layer_for(bs.as_ref(), props);
                    stats.cube_states += 1;
                }
                Some((k @ RenderKind::Model(_), is_solid)) => {
                    render[id as usize] = k;
                    solid[id as usize] = is_solid;
                    particle_layer[id as usize] = baker.particle_layer_for(bs.as_ref(), props);
                    stats.model_states += 1;
                }
                _ => stats.invisible_states += 1,
            }
            // Collision shape: a solid state is the unit cube; otherwise a
            // curated family may collide with its model geometry. Everything
            // else stays empty (today's behaviour).
            collide[id as usize] = if solid[id as usize] {
                vec![[0.0, 0.0, 0.0, 1.0, 1.0, 1.0]]
            } else if let (Some(tall), Some(bs)) = (model_collision(short), bs.as_ref()) {
                let refs = baker.state_refs(bs, props);
                let boxes = baker.collision_boxes(&refs, tall);
                if !boxes.is_empty() {
                    stats.shaped_collision_states += 1;
                }
                boxes
            } else {
                Vec::new()
            };

            // Light. Vanilla's rule (BlockBehaviour.getLightDampening) is
            //     isSolidRender ? 15 : propagatesSkylightDown ? 0 : 1
            // with propagatesSkylightDown = !fullCubeShape && no fluid. The
            // bake already knows the shape (`solid`) and the fluid-ness, so
            // the only imported fact is `canOcclude`.
            let full_cube = solid[id as usize];
            let fluid = matches!(render[id as usize], RenderKind::Fluid { .. });
            // `propagatesSkylightDown` defaults to "not a full cube and no
            // fluid", but 16 block classes override it — glass returns true so
            // sky passes it at full strength, fences return "not waterlogged".
            let waterlogged =
                props.and_then(|p| p.get("waterlogged")).and_then(|v| v.as_str()) == Some("true");
            // A waterlogged block's fluid state IS water, which is what
            // `isWaterAt` asks (M30).
            if waterlogged {
                water[id as usize] = true;
            }
            let propagates = match sky_propagate.get(block_name.as_str()) {
                Some(0) => false,
                Some(1) => true,
                Some(_) => !waterlogged,
                None => !full_cube && !fluid,
            };
            dampening[id as usize] = if let Some(&d) = damp_override.get(block_name.as_str()) {
                d
            } else if full_cube && !no_occlude.contains(block_name.as_str()) {
                15
            } else if propagates {
                0
            } else {
                1
            };
            // Directional occlusion for non-full-cube shapes (slab, stair,
            // …). `useShapeForLightOcclusion` is derived rather than scraped:
            // its only effect is through face coverage, and a block vanilla
            // leaves out of that list essentially never covers a whole face
            // (a fence post is 6/16 wide), so a false positive is inert.
            if !full_cube && !no_occlude.contains(block_name.as_str()) {
                face_occludes[id as usize] = face_coverage(&collide[id as usize]);
            }
            // Emission. A property-driven rule (a candle's `3 × candles`, a
            // glow berry's `berries`, a light block's `level`) wins over the
            // constant tables; see `block_light::STATE_EMISSION`.
            let prop = |name: &str| props.and_then(|p| p.get(name)).and_then(|v| v.as_str());
            emission[id as usize] = if let Some(&(gate, gate_val, value_prop, map)) =
                state_emission.get(block_name.as_str())
            {
                if !gate.is_empty() && prop(gate) != Some(gate_val) {
                    0
                } else {
                    prop(value_prop)
                        .and_then(|v| map.iter().find(|(k, _)| *k == v))
                        .map_or(0, |&(_, e)| e)
                }
            } else if let Some(&e) = const_emission.get(block_name.as_str()) {
                e
            } else if prop("lit") == Some("true") {
                lit_emission.get(block_name.as_str()).copied().unwrap_or(0)
            } else {
                0
            };
        }
    }

    // M22: held items, after every block layer exists (block items copy them).
    let held_items = baker.bake_held_items(&trims);

    stats.textures = baker.layers.len();
    log::info!(
        "rewo-data: baked {} cubes + {} models ({} invisible), {} textures, {} shaped-collision states",
        stats.cube_states,
        stats.model_states,
        stats.invisible_states,
        stats.textures,
        stats.shaped_collision_states
    );

    if !baker.animations.is_empty() {
        log::info!("rewo-data: {} animated texture layers", baker.animations.len());
    }
    Ok(BakedAssets {
        lang,
        item_names,
        enchantment_text,
        glint,
        armor_glint,
        forcefield,
        equipment,
        trims,
        held_items,
        render,
        solid,
        water,
        collide,
        emission,
        dampening,
        face_occludes,
        models,
        layers: baker.layers,
        layer_names: baker.layer_names,
        particle_layer,
        animations: baker.animations,
        grass_tint,
        foliage_tint,
        grass_colormap,
        foliage_colormap,
        dry_foliage_colormap,
        font,
        mob_textures,
        mob_variant_textures,
        hud,
        locator,
        container,
        celestial,
        end_sky,
        destroy_stages,
        end_portal,
        rain,
        snow,
        particles,
        clouds,
        stats,
    })
}

/// Decode one `assets/minecraft/textures/environment/<rel>` image, or `None`.
/// The particle sprite sets M37 simulates, taken from the jar's own
/// `assets/minecraft/particles/<name>.json` rather than guessed from the
/// texture directory.
///
/// Order within each set is the order the JSON lists — which for smoke and
/// poof is `generic_7` down to `generic_0`, i.e. the strip runs *backwards*
/// from the filenames. `SpriteSet.get(age, lifetime)` indexes this list, so
/// reversing it would run every puff of smoke inside out.
pub const PARTICLE_SPRITE_SETS: &[(&str, &[&str])] = &[
    ("flame", &["flame"]),
    ("crit", &["critical_hit"]),
    ("splash", &["splash_0", "splash_1", "splash_2", "splash_3"]),
    (
        "smoke",
        &[
            "generic_7", "generic_6", "generic_5", "generic_4", "generic_3", "generic_2",
            "generic_1", "generic_0",
        ],
    ),
    (
        "poof",
        &[
            "generic_7", "generic_6", "generic_5", "generic_4", "generic_3", "generic_2",
            "generic_1", "generic_0",
        ],
    ),
];

/// The particle sprites, decoded into `TEX_SIZE`-square layers so they can
/// live in the same texture array as the block textures a terrain shard needs
/// (M37). Vanilla's are 8x8; they are point-upscaled 2x, which is exact for
/// pixel art.
pub struct ParticleSprites {
    /// `TEX_SIZE * TEX_SIZE * 4` bytes each, in `PARTICLE_SPRITE_SETS` order,
    /// flattened — set 0's frames, then set 1's, and so on.
    pub layers: Vec<Vec<u8>>,
    /// Index into `layers` of each set's first frame, parallel to
    /// `PARTICLE_SPRITE_SETS`.
    pub set_offsets: Vec<u32>,
}

impl ParticleSprites {
    /// First layer of a named set, and how many frames it holds.
    pub fn set(&self, name: &str) -> Option<(u32, u32)> {
        let i = PARTICLE_SPRITE_SETS.iter().position(|(n, _)| *n == name)?;
        Some((self.set_offsets[i], PARTICLE_SPRITE_SETS[i].1.len() as u32))
    }
}

/// Decode every particle sprite M37 needs. A missing file drops the whole
/// bake rather than shipping a set with a hole in it — an out-of-range frame
/// index would otherwise sample a neighbouring sprite.
fn bake_particle_sprites(jar: Jar) -> Option<ParticleSprites> {
    let mut layers = Vec::new();
    let mut set_offsets = Vec::new();
    for (_, frames) in PARTICLE_SPRITE_SETS {
        set_offsets.push(layers.len() as u32);
        for f in *frames {
            let mut bytes = Vec::new();
            jar.by_name(&format!("assets/minecraft/textures/particle/{f}.png"))
                .ok()?
                .read_to_end(&mut bytes)
                .ok()?;
            let (rgba, w, h) = decode_png_any(&bytes)?;
            layers.push(upscale_to_tex_size(&rgba, w, h)?);
        }
    }
    log::info!("rewo-data: {} particle sprite layers", layers.len());
    Some(ParticleSprites { layers, set_offsets })
}

/// Point-upscale an RGBA image to `TEX_SIZE` square. Only exact integer
/// ratios are accepted — a non-divisor would need filtering, and silently
/// blurring a particle sprite is worse than not drawing it.
fn upscale_to_tex_size(rgba: &[u8], w: u32, h: u32) -> Option<Vec<u8>> {
    if w == 0 || h == 0 || TEX_SIZE % w != 0 || TEX_SIZE % h != 0 {
        return None;
    }
    if rgba.len() < (w * h * 4) as usize {
        return None;
    }
    let (sx, sy) = (TEX_SIZE / w, TEX_SIZE / h);
    let mut out = vec![0u8; (TEX_SIZE * TEX_SIZE * 4) as usize];
    for y in 0..TEX_SIZE {
        for x in 0..TEX_SIZE {
            let src = (((y / sy) * w + (x / sx)) * 4) as usize;
            let dst = ((y * TEX_SIZE + x) * 4) as usize;
            out[dst..dst + 4].copy_from_slice(&rgba[src..src + 4]);
        }
    }
    Some(out)
}

/// One `textures/misc/` PNG — the enchantment glint (M43).
fn bake_misc_texture(jar: Jar, rel: &str) -> Option<DecodedImage> {
    let mut bytes = Vec::new();
    jar.by_name(&format!("assets/minecraft/textures/misc/{rel}"))
        .ok()?
        .read_to_end(&mut bytes)
        .ok()?;
    let (rgba, w, h) = decode_png_any(&bytes)?;
    Some(DecodedImage { rgba, w, h })
}

fn bake_env_texture(jar: Jar, rel: &str) -> Option<DecodedImage> {
    let mut bytes = Vec::new();
    jar.by_name(&format!("assets/minecraft/textures/environment/{rel}"))
        .ok()?
        .read_to_end(&mut bytes)
        .ok()?;
    let (rgba, w, h) = decode_png_any(&bytes)?;
    Some(DecodedImage { rgba, w, h })
}

/// `ModelBakery.BREAKING_LOCATIONS` — the ten `block/destroy_stage_N.png`
/// crack overlays, in stage order (M81).
///
/// **Not interned into the world texture array.** `RenderTypes.crumbling`
/// binds each stage as its own `Sampler0`, so these are standalone textures
/// that never appear in a block model — putting them in the block array would
/// grow it by ten layers no mesh can reference. The crumbling pass owns its
/// own small array instead.
///
/// All ten or none: a partial set would make some stages of the same break
/// invisible, which reads as a rendering bug rather than a missing asset.
fn bake_destroy_stages(jar: Jar) -> Option<[DecodedImage; DESTROY_STAGE_COUNT]> {
    let mut out: Vec<DecodedImage> = Vec::with_capacity(DESTROY_STAGE_COUNT);
    for i in 0..DESTROY_STAGE_COUNT {
        let mut bytes = Vec::new();
        jar.by_name(&format!(
            "assets/minecraft/textures/block/destroy_stage_{i}.png"
        ))
        .ok()?
        .read_to_end(&mut bytes)
        .ok()?;
        let (rgba, w, h) = decode_png_any(&bytes)?;
        out.push(DecodedImage { rgba, w, h });
    }
    out.try_into().ok()
}

/// Load the sun + 8 moon-phase textures from the jar's `environment/celestial/`
/// (26.2 layout). Any missing file → no celestials (degrade, not error).
fn bake_celestial(jar: Jar) -> Option<CelestialTextures> {
    let load = |jar: Jar, rel: &str| -> Option<DecodedImage> {
        let mut bytes = Vec::new();
        jar.by_name(&format!("assets/minecraft/textures/environment/{rel}"))
            .ok()?
            .read_to_end(&mut bytes)
            .ok()?;
        let (rgba, w, h) = decode_png_any(&bytes)?;
        Some(DecodedImage { rgba, w, h })
    };
    let sun = load(jar, "celestial/sun.png")?;
    // `array::try_from` over a Vec would need Debug on the element; build by
    // index instead, bailing (→ no celestials) if any phase is absent.
    let mut moons: Vec<DecodedImage> = Vec::with_capacity(8);
    for name in MOON_PHASE_FILES {
        moons.push(load(jar, &format!("celestial/moon/{name}.png"))?);
    }
    let moons: [DecodedImage; 8] = moons.try_into().ok()?;
    Some(CelestialTextures { sun, moons })
}

/// Extract an entity texture of a known size to flat RGBA (mob models).
fn bake_entity_tex(jar: Jar, rel: &str, w: u32, h: u32) -> Option<Vec<u8>> {
    let mut bytes = Vec::new();
    jar.by_name(&format!("assets/minecraft/textures/{rel}"))
        .ok()?
        .read_to_end(&mut bytes)
        .ok()?;
    let (rgba, gw, gh) = decode_png_any(&bytes)?;
    if gw == w && gh == h {
        Some(rgba)
    } else {
        log::warn!("rewo-data: {rel} is {gw}×{gh}, expected {w}×{h}");
        None
    }
}

/// The textures the container screen and its tooltips draw (M35, M40, M58).
pub struct ContainerSprites {
    /// `AbstractContainerScreen.INVENTORY_LOCATION` — a 256×256 sheet whose
    /// top-left 176×166 is the panel. Kept whole: vanilla blits a sub-rect out
    /// of it, so cropping here would move the numbers into two places.
    pub background: HudSprite,
    /// `container/slot_highlight_back`, drawn under the hovered slot's item.
    pub highlight_back: HudSprite,
    /// `container/slot_highlight_front`, drawn over it. Both are 24×24 and are
    /// blitted at their own size, so the `.mcmeta` nine-slice never engages.
    pub highlight_front: HudSprite,
    /// `tooltip/background` and `tooltip/frame` (M40) — the two sprites
    /// `TooltipRenderUtil.extractTooltipBackground` blits, one over the other,
    /// at the same rect.
    ///
    /// Both are 100×100 nine-slice sprites, and **they are never blitted at
    /// that size**: a tooltip is whatever the text needs, so the corners come
    /// out at their natural size and the edges are stretched or tiled between
    /// them. The two disagree about which — background `border: 9` tiles its
    /// middles, frame `border: 10` sets `stretch_inner`, so it stretches them.
    pub tooltip_background: HudSprite,
    pub tooltip_frame: HudSprite,
    /// `ClientBundleTooltip`'s six sprites (M58) — the cell chrome of the grid
    /// M52 computed the geometry of.
    ///
    /// `container/bundle/slot_background`, and the bundle's **own** pair of
    /// highlights: `container/bundle/slot_highlight_back` and
    /// `..._front` are *different files* from the two above, which live at
    /// `container/slot_highlight_*` with no `bundle/` in the path. All three
    /// are 24×24 and blitted at exactly 24×24, which is
    /// `blitNineSlicedSprite`'s first branch — `width == nineSlice.width() &&
    /// height == nineSlice.height()` blits the whole sprite once and never
    /// slices at all.
    pub bundle_slot: HudSprite,
    pub bundle_highlight_back: HudSprite,
    pub bundle_highlight_front: HudSprite,
    /// The progress bar's three, which *are* sliced: a 12×12 border blitted at
    /// 96×13, and two 6×6 fills blitted at `getProgressBarFill(weight)`×13.
    /// `bundle_progressbar_fill` is the partial state and `_full` the complete
    /// one — and they are not shades of one colour, they are the chat palette's
    /// blue `5555FF` and red `FF5555`.
    pub bundle_bar_border: HudSprite,
    pub bundle_bar_fill: HudSprite,
    pub bundle_bar_full: HudSprite,
}

/// Every item's English display name, keyed by full registry name (M40).
///
/// `Item.getDescriptionId()` is `Util.makeDescriptionId("item", id)` — so
/// `item.minecraft.diamond_sword` — except that `BlockItem` overrides it to
/// return its **block's** id, `block.minecraft.dirt`. Rather than model that
/// override, this reads whichever of the two keys the language file actually
/// has, preferring the block spelling because that is the override's answer.
///
/// The ambiguity is real but empirically inert: of 26.2's 1537 items, exactly
/// seven carry both keys (`brewing_stand`, `cauldron`, `flower_pot`,
/// `nether_wart`, `pitcher_plant`, `resin_clump`, and one more), and in every
/// case the two strings are **identical** — so the preference cannot be
/// observed. It is written down anyway, because a future version where they
/// diverge would otherwise pick silently.
///
/// A missing key yields no entry at all rather than a prettified id: a
/// tooltip that says nothing is better than one that says `Diamond_sword`.
///
/// The lookup goes through the loaded [`crate::lang::Language`], not the raw
/// `en_us.json` (M50). That is not bookkeeping: 27 items read differently
/// through the two, because `deprecated.json` renames
/// `item.minecraft.<x>.new` onto `item.minecraft.<x>` for the eighteen
/// smithing templates and nine banner patterns — so the raw file says every
/// one of them is a "Smithing Template" or a "Banner Pattern" and the
/// language map says "Bolt Armor Trim" and "Creeper Charge Banner Pattern".
fn bake_item_names(jar: Jar, lang: &crate::lang::Language) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let names: Vec<String> = jar
        .file_names()
        .filter_map(|p| {
            p.strip_prefix("assets/minecraft/items/")
                .and_then(|r| r.strip_suffix(".json"))
                .filter(|r| !r.contains('/'))
                .map(str::to_string)
        })
        .collect();
    for name in names {
        let key = lang
            .get(&format!("block.minecraft.{name}"))
            .or_else(|| lang.get(&format!("item.minecraft.{name}")));
        if let Some(text) = key {
            out.insert(format!("minecraft:{name}"), text.to_string());
        }
    }
    log::info!("rewo-data: {} item display name(s)", out.len());
    out
}

/// Extract the container-screen textures. Any missing one → no screen.
fn bake_container(jar: Jar) -> Option<ContainerSprites> {
    let get = |jar: Jar, rel: &str| -> Option<HudSprite> {
        let mut bytes = Vec::new();
        jar.by_name(&format!("assets/minecraft/textures/{rel}"))
            .ok()?
            .read_to_end(&mut bytes)
            .ok()?;
        let (rgba, w, h) = decode_png_any(&bytes)?;
        Some(HudSprite { rgba, w, h })
    };
    Some(ContainerSprites {
        background: get(jar, "gui/container/inventory.png")?,
        highlight_back: get(jar, "gui/sprites/container/slot_highlight_back.png")?,
        highlight_front: get(jar, "gui/sprites/container/slot_highlight_front.png")?,
        tooltip_background: get(jar, "gui/sprites/tooltip/background.png")?,
        tooltip_frame: get(jar, "gui/sprites/tooltip/frame.png")?,
        // The `bundle/` prefix is load-bearing: the two names above exist
        // *twice* in the jar, once here and once a directory up, and the two
        // copies are different art.
        bundle_slot: get(jar, "gui/sprites/container/bundle/slot_background.png")?,
        bundle_highlight_back: get(
            jar,
            "gui/sprites/container/bundle/slot_highlight_back.png",
        )?,
        bundle_highlight_front: get(
            jar,
            "gui/sprites/container/bundle/slot_highlight_front.png",
        )?,
        bundle_bar_border: get(
            jar,
            "gui/sprites/container/bundle/bundle_progressbar_border.png",
        )?,
        bundle_bar_fill: get(
            jar,
            "gui/sprites/container/bundle/bundle_progressbar_fill.png",
        )?,
        bundle_bar_full: get(
            jar,
            "gui/sprites/container/bundle/bundle_progressbar_full.png",
        )?,
    })
}

/// Extract the in-game HUD sprite set. Any missing sprite → no HUD.
fn bake_hud(jar: Jar) -> Option<HudSprites> {
    let get = |jar: Jar, rel: &str| -> Option<HudSprite> {
        let mut bytes = Vec::new();
        jar.by_name(&format!("assets/minecraft/textures/{rel}"))
            .ok()?
            .read_to_end(&mut bytes)
            .ok()?;
        let (rgba, w, h) = decode_png_any(&bytes)?;
        Some(HudSprite { rgba, w, h })
    };
    Some(HudSprites {
        hotbar: get(jar, "gui/sprites/hud/hotbar.png")?,
        selection: get(jar, "gui/sprites/hud/hotbar_selection.png")?,
        crosshair: get(jar, "gui/sprites/hud/crosshair.png")?,
        heart_full: get(jar, "gui/sprites/hud/heart/full.png")?,
        heart_half: get(jar, "gui/sprites/hud/heart/half.png")?,
        heart_container: get(jar, "gui/sprites/hud/heart/container.png")?,
        food_full: get(jar, "gui/sprites/hud/food_full.png")?,
        food_half: get(jar, "gui/sprites/hud/food_half.png")?,
        food_empty: get(jar, "gui/sprites/hud/food_empty.png")?,
        experience_bar_background: get(jar, "gui/sprites/hud/experience_bar_background.png")?,
        experience_bar_progress: get(jar, "gui/sprites/hud/experience_bar_progress.png")?,
    })
}

/// Extract the locator bar's sprites and parse `waypoint_style/*.json` (M83).
///
/// The style files are read rather than hard-coded because the sprite *list*
/// is what selects a dot, and it is data: `bowtie.json` overrides
/// `near_distance` to 64 and puts its own sprite in front of the four defaults.
/// Any missing piece → no locator bar, and the rest of the HUD is unaffected.
fn bake_locator(jar: Jar) -> Option<LocatorSprites> {
    let get = |jar: Jar, rel: &str| -> Option<HudSprite> {
        let mut bytes = Vec::new();
        jar.by_name(&format!("assets/minecraft/textures/{rel}"))
            .ok()?
            .read_to_end(&mut bytes)
            .ok()?;
        let (rgba, w, h) = decode_png_any(&bytes)?;
        Some(HudSprite { rgba, w, h })
    };
    let background = get(jar, "gui/sprites/hud/locator_bar_background.png")?;
    let arrow_up = get(jar, "gui/sprites/hud/locator_bar_arrow_up.png")?;
    let arrow_down = get(jar, "gui/sprites/hud/locator_bar_arrow_down.png")?;

    let names: Vec<String> = jar
        .file_names()
        .filter_map(|p| {
            p.strip_prefix("assets/minecraft/waypoint_style/")
                .and_then(|r| r.strip_suffix(".json"))
                .filter(|r| !r.contains('/'))
                .map(str::to_string)
        })
        .collect();

    let mut dots: Vec<HudSprite> = Vec::new();
    let mut dot_index: HashMap<String, u16> = HashMap::new();
    let mut styles = Vec::new();
    for name in names {
        let mut text = String::new();
        if jar
            .by_name(&format!("assets/minecraft/waypoint_style/{name}.json"))
            .ok()?
            .read_to_string(&mut text)
            .is_err()
        {
            continue;
        }
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        // `DISTANCE_CODEC.optionalFieldOf("near_distance", 128)`.
        let near = v
            .get("near_distance")
            .and_then(|d| d.as_i64())
            .unwrap_or(128) as i32;
        let far = v
            .get("far_distance")
            .and_then(|d| d.as_i64())
            .unwrap_or(332) as i32;
        let mut sprites = Vec::new();
        for s in v
            .get("sprites")
            .and_then(|s| s.as_array())
            .map(Vec::as_slice)
            .unwrap_or(&[])
        {
            let Some(id) = s.as_str() else { continue };
            // `sprite.withPrefix("hud/locator_bar_dot/")` — the prefix goes on
            // the *path*, so `minecraft:default_0` becomes
            // `minecraft:hud/locator_bar_dot/default_0`.
            let path = id.strip_prefix("minecraft:").unwrap_or(id);
            if let Some(&i) = dot_index.get(path) {
                sprites.push(i);
                continue;
            }
            let Some(img) = get(jar, &format!("gui/sprites/hud/locator_bar_dot/{path}.png"))
            else {
                continue;
            };
            let i = dots.len() as u16;
            dots.push(img);
            dot_index.insert(path.to_string(), i);
            sprites.push(i);
        }
        // `ExtraCodecs.nonEmptyList(...)` — an empty list fails the codec, so
        // the style would not exist at all rather than resolve to nothing.
        if sprites.is_empty() {
            continue;
        }
        styles.push(WaypointStyleAsset {
            key: format!("minecraft:{name}"),
            near_distance: near,
            far_distance: far,
            sprites,
        });
    }
    if styles.is_empty() {
        return None;
    }
    log::info!(
        "rewo-data: locator bar — {} style(s), {} dot sprite(s)",
        styles.len(),
        dots.len()
    );
    Some(LocatorSprites {
        background,
        arrow_up,
        arrow_down,
        dots,
        styles,
    })
}

/// Decode any small PNG (any color type) to (RGBA8, w, h). Unlike
/// `decode_png_rgba` this doesn't assume the 16px block-texture size.
pub(crate) fn decode_png_any(bytes: &[u8]) -> Option<(Vec<u8>, u32, u32)> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0u8; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    let n = (info.width * info.height) as usize;
    let mut rgba = vec![0u8; n * 4];
    match info.color_type {
        png::ColorType::Rgba => rgba.copy_from_slice(&buf[..n * 4]),
        png::ColorType::Rgb => {
            for i in 0..n {
                rgba[i * 4..i * 4 + 3].copy_from_slice(&buf[i * 3..i * 3 + 3]);
                rgba[i * 4 + 3] = 255;
            }
        }
        png::ColorType::GrayscaleAlpha => {
            for i in 0..n {
                let g = buf[i * 2];
                rgba[i * 4..i * 4 + 3].copy_from_slice(&[g, g, g]);
                rgba[i * 4 + 3] = buf[i * 2 + 1];
            }
        }
        png::ColorType::Grayscale => {
            for i in 0..n {
                let g = buf[i];
                rgba[i * 4..i * 4 + 3].copy_from_slice(&[g, g, g]);
                rgba[i * 4 + 3] = 255;
            }
        }
        png::ColorType::Indexed => return None,
    }
    Some((rgba, info.width, info.height))
}

/// Extract + measure the legacy bitmap font from the jar.
fn bake_font(jar: Jar) -> Option<BakedFont> {
    let mut bytes = Vec::new();
    jar.by_name("assets/minecraft/textures/font/ascii.png")
        .ok()?
        .read_to_end(&mut bytes)
        .ok()?;
    let mut decoder = png::Decoder::new(std::io::Cursor::new(&bytes));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0u8; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    let (w, h) = (info.width, info.height);
    if w != h || w % 16 != 0 {
        return None;
    }
    let px = w as usize;
    let mut atlas = vec![0u8; px * px * 4];
    match info.color_type {
        png::ColorType::Rgba => atlas.copy_from_slice(&buf[..px * px * 4]),
        png::ColorType::GrayscaleAlpha => {
            for i in 0..px * px {
                let g = buf[i * 2];
                atlas[i * 4..i * 4 + 3].copy_from_slice(&[g, g, g]);
                atlas[i * 4 + 3] = buf[i * 2 + 1];
            }
        }
        _ => return None,
    }
    let cell = w / 16;
    let advance = font_advances(&atlas, w, cell);

    // Patch one opaque-white texel into the space glyph's cell (guaranteed
    // blank — and text layout never emits quads for spaces, so it can't
    // show). Solid quads (nametag backgrounds, capsules) sample it.
    let (wx, wy) = ((32 % 16) * cell, (32 / 16) * cell);
    let wi = ((wy as usize) * px + wx as usize) * 4;
    atlas[wi..wi + 4].copy_from_slice(&[255, 255, 255, 255]);

    Some(BakedFont {
        atlas,
        atlas_size: w,
        cell,
        advance,
        white_texel: (wx, wy),
    })
}

/// Per-glyph advances: rightmost column with any alpha + 2 (1 px glyph edge
/// + 1 px spacing), matching vanilla's legacy provider; blank cells (space)
/// advance 4.
fn font_advances(atlas: &[u8], size: u32, cell: u32) -> [u8; 256] {
    let mut advance = [0u8; 256];
    for cp in 0..256u32 {
        let (cx, cy) = ((cp % 16) * cell, (cp / 16) * cell);
        let mut rightmost: Option<u32> = None;
        for col in 0..cell {
            for row in 0..cell {
                let i = (((cy + row) * size + cx + col) * 4 + 3) as usize;
                if atlas[i] > 0 {
                    rightmost = Some(col);
                    break;
                }
            }
        }
        advance[cp as usize] = match rightmost {
            Some(r) => (r + 2) as u8,
            None => 4,
        };
    }
    advance
}

type Jar<'a> = &'a mut zip::ZipArchive<std::io::BufReader<std::fs::File>>;

/// The trim materials an item definition names its own icon variants for (M49).
///
/// Read from the definition rather than from the material registry: this is a
/// load-time bake of the *jar*, and the registry is the server's. An item
/// trimmed with a material its definition does not name simply has no variant,
/// which lands on vanilla's own fallback — the untrimmed icon.
fn trim_material_cases(def: &serde_json::Value) -> Vec<String> {
    let model = def.get("model");
    if model.and_then(|m| m.get("property")).and_then(|p| p.as_str())
        != Some("minecraft:trim_material")
    {
        return Vec::new();
    }
    model
        .and_then(|m| m.get("cases"))
        .and_then(|c| c.as_array())
        .map(Vec::as_slice)
        .unwrap_or(&[])
        .iter()
        .filter_map(|c| c.get("when"))
        .flat_map(|w| match w {
            // `when` is one name or a list of them — the same shape every
            // other `select` property uses.
            serde_json::Value::String(s) => vec![s.clone()],
            serde_json::Value::Array(a) => a
                .iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect(),
            _ => Vec::new(),
        })
        .collect()
}

struct Baker<'a> {
    jar: Jar<'a>,
    model_cache: HashMap<String, Option<ResolvedModel>>,
    layer_index: HashMap<String, u16>,
    layers: Vec<Vec<u8>>,
    layer_names: Vec<String>,
    animations: Vec<AnimatedLayer>,
    grass_tint: [u8; 3],
    foliage_tint: [u8; 3],
}

/// A model with its parent chain flattened: merged textures + all elements.
#[derive(Clone)]
struct ResolvedModel {
    textures: HashMap<String, String>,
    elements: Vec<serde_json::Value>,
    ambient_occlusion: bool,
}

/// A blockstate reference to a model with optional whole-model rotation.
struct ModelRef {
    model: String,
    x: i32,
    y: i32,
}

/// A parsed blockstate: variants or multipart.
enum BlockState {
    Variants(serde_json::Map<String, serde_json::Value>),
    Multipart(Vec<serde_json::Value>),
}

impl<'a> Baker<'a> {
    fn read_json(&mut self, path: &str) -> Option<serde_json::Value> {
        let mut entry = self.jar.by_name(path).ok()?;
        let mut text = String::new();
        entry.read_to_string(&mut text).ok()?;
        serde_json::from_str(&text).ok()
    }

    fn load_blockstate(&mut self, block: &str) -> Option<BlockState> {
        let json = self.read_json(&format!("assets/minecraft/blockstates/{block}.json"))?;
        if let Some(v) = json.get("variants").and_then(|v| v.as_object()) {
            Some(BlockState::Variants(v.clone()))
        } else {
            json.get("multipart")
                .and_then(|m| m.as_array())
                .map(|m| BlockState::Multipart(m.clone()))
        }
    }

    /// Resolve one state's render data. Cube fast-path only for a single
    /// variant whose model is a full opaque cube; everything else bakes to a
    /// quad list.
    fn resolve_state(
        &mut self,
        bs: &BlockState,
        props: Option<&serde_json::Map<String, serde_json::Value>>,
        foliage: bool,
        block: &str,
        models: &mut Vec<Vec<Quad>>,
    ) -> Option<(RenderKind, bool)> {
        let refs = self.state_refs(bs, props);
        if refs.is_empty() {
            return None;
        }

        // M14 tint metadata for this state: the block's BlockColors layers +
        // whether this is the tall-grass UPPER half (which samples pos.below()).
        let upper_half = props
            .and_then(|p| p.get("half"))
            .and_then(|h| h.as_str())
            == Some("upper");
        let tint = TintInfo {
            foliage,
            layers: block_color_layers(block),
            upper_half,
        };

        // Collision solidity: any referenced model with a full 16³ element
        // makes this a solid cube (grass_block renders as a Model due to its
        // overlay element but must still collide as a full cube).
        let is_solid = refs.iter().any(|r| self.model_has_full_cube(&r.model));

        // Fast path: a single, unrotated, full-cube model.
        if refs.len() == 1 && refs[0].x == 0 && refs[0].y == 0 {
            if let Some(cube) = self.try_cube(&refs[0].model, tint) {
                return Some((cube, true));
            }
        }

        let mut quads = Vec::new();
        for r in &refs {
            self.append_model_quads(r, tint, &mut quads);
        }
        if quads.is_empty() {
            return None;
        }
        let idx = models.len() as u32;
        models.push(quads);
        Some((RenderKind::Model(idx), is_solid))
    }

    /// The model refs a blockstate resolves to for these properties.
    fn state_refs(
        &self,
        bs: &BlockState,
        props: Option<&serde_json::Map<String, serde_json::Value>>,
    ) -> Vec<ModelRef> {
        match bs {
            BlockState::Variants(v) => pick_variant(v, props).into_iter().collect(),
            BlockState::Multipart(parts) => parts
                .iter()
                .filter(|p| multipart_applies(p, props))
                .filter_map(|p| model_ref(p.get("apply")?))
                .collect(),
        }
    }

    /// Collision boxes for one state, in block-local `0..1`, taken from the
    /// referenced models' elements and rotated by each ref's blockstate
    /// rotation (stairs/fences pick a rotated model per facing, so the shape
    /// has to rotate with it). `tall` raises the boxes to 1.5 — vanilla's
    /// fence/wall collision is taller than the model so players can't jump it.
    fn collision_boxes(&mut self, refs: &[ModelRef], tall: bool) -> Vec<[f32; 6]> {
        let mut out = Vec::new();
        for r in refs {
            let Some(resolved) = self.resolve_model(&r.model) else { continue };
            for el in resolved.elements.clone() {
                let (Some(from), Some(to)) = (box_coords(&el, "from"), box_coords(&el, "to"))
                else {
                    continue;
                };
                let mut b = [
                    from[0] / 16.0,
                    from[1] / 16.0,
                    from[2] / 16.0,
                    to[0] / 16.0,
                    to[1] / 16.0,
                    to[2] / 16.0,
                ];
                b = rotate_box(b, r.x, r.y);
                if tall {
                    b[4] = b[4].max(1.5);
                }
                // Skip zero-thickness planes (decorative overlays).
                if (b[3] - b[0]) > 1e-4 && (b[4] - b[1]) > 1e-4 && (b[5] - b[2]) > 1e-4 {
                    out.push(b);
                }
            }
        }
        out
    }

    /// True if the model (or a parent) has any element spanning the full
    /// 16³ box — the collision-cube test.
    fn model_has_full_cube(&mut self, model: &str) -> bool {
        let Some(resolved) = self.resolve_model(model) else {
            return false;
        };
        resolved.elements.iter().any(|el| {
            box_coords(el, "from") == Some([0.0, 0.0, 0.0])
                && box_coords(el, "to") == Some([16.0, 16.0, 16.0])
        })
    }

    /// If `model` is a full 16³ cube with all six faces, return a Cube.
    fn try_cube(&mut self, model: &str, tint: TintInfo) -> Option<RenderKind> {
        let resolved = self.resolve_model(model)?;
        if resolved.elements.len() != 1 {
            return None;
        }
        let el = &resolved.elements[0];
        if box_coords(el, "from")? != [0.0, 0.0, 0.0]
            || box_coords(el, "to")? != [16.0, 16.0, 16.0]
        {
            return None;
        }
        if el.get("rotation").is_some() {
            return None;
        }
        let faces = el.get("faces")?.as_object()?;
        let mut layers = [0u16; 6];
        let mut raw_layers = [0u16; 6];
        let mut tints = [TintSource::None; 6];
        for (i, name) in FACE_NAMES.iter().enumerate() {
            let face = faces.get(*name)?;
            let tex = face.get("texture")?.as_str()?;
            let tex_name = resolve_texture_var(tex, &resolved.textures)?;
            layers[i] = self.layer_for(&tex_name, foliage_of(&tex_name, tint.foliage))?;
            raw_layers[i] = self.layer_for(&tex_name, TintKind::None)?;
            tints[i] = resolve_tint_source(tint.layers, tintindex_of(face), tint.upper_half);
        }
        Some(RenderKind::Cube {
            faces: layers,
            raw_faces: raw_layers,
            tint: tints,
        })
    }

    fn append_model_quads(&mut self, r: &ModelRef, tint: TintInfo, out: &mut Vec<Quad>) {
        let Some(resolved) = self.resolve_model(&r.model) else {
            return;
        };
        let shade_default = resolved.ambient_occlusion; // proxy; real shade is per-element
        for el in &resolved.elements {
            self.element_quads(el, &resolved.textures, r.x, r.y, tint, shade_default, out);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn element_quads(
        &mut self,
        el: &serde_json::Value,
        textures: &HashMap<String, String>,
        rot_x: i32,
        rot_y: i32,
        tint: TintInfo,
        _shade_default: bool,
        out: &mut Vec<Quad>,
    ) {
        let (Some(from), Some(to)) = (box_coords(el, "from"), box_coords(el, "to")) else {
            return;
        };
        let shade = el.get("shade").and_then(|s| s.as_bool()).unwrap_or(true);
        let el_rot = el.get("rotation");
        let Some(faces) = el.get("faces").and_then(|f| f.as_object()) else {
            return;
        };
        for (fname, face) in faces {
            let Some(face_idx) = FACE_NAMES.iter().position(|n| n == fname) else {
                continue;
            };
            let Some(tex) = face.get("texture").and_then(|t| t.as_str()) else {
                continue;
            };
            let Some(tex_name) = resolve_texture_var(tex, textures) else {
                continue;
            };
            let Some(layer) = self.layer_for(&tex_name, foliage_of(&tex_name, tint.foliage)) else {
                continue;
            };
            let Some(raw_layer) = self.layer_for(&tex_name, TintKind::None) else {
                continue;
            };
            let tint_src = resolve_tint_source(tint.layers, tintindex_of(face), tint.upper_half);
            let has_cull = face.get("cullface").is_some();

            // Corners in 0..16, then transforms, then /16.
            let (mut verts, uv) = face_geometry(from, to, face_idx, face.get("uv"));
            // Element rotation.
            if let Some(rot) = el_rot {
                apply_element_rotation(&mut verts, rot);
            }
            // Whole-model variant rotation (x then y) around (8,8,8).
            for v in verts.iter_mut() {
                rotate_xy(v, rot_x, rot_y);
            }
            let mut fverts = [[0.0f32; 3]; 4];
            for (i, v) in verts.iter().enumerate() {
                fverts[i] = [(v[0] / 16.0), (v[1] / 16.0), (v[2] / 16.0)];
            }

            // Rotated normal → shade dir + (if declared) cullface dir.
            let mut normal = FACE_NORMALS[face_idx];
            if let Some(rot) = el_rot {
                rotate_normal_element(&mut normal, rot);
            }
            rotate_normal_xy(&mut normal, rot_x, rot_y);
            let dir = snap_face(normal);
            let cull = if has_cull && is_axis_flush(&fverts, dir) {
                dir as i8
            } else {
                -1
            };

            out.push(Quad {
                verts: fverts,
                uv,
                layer,
                raw_layer,
                cull,
                dir,
                tint: tint_src,
                shade,
            });
        }
    }


    /// Bake every resolvable item's held model (M22).
    ///
    /// Two phases, because resolving a definition and baking its geometry both
    /// need `&mut self`: resolve every definition first, then bake. The item
    /// list comes from the jar itself (`assets/minecraft/items/*.json`) rather
    /// than the registry, so this needs no extra input and cannot drift from
    /// what the jar ships.
    fn bake_held_items(&mut self, trims: &crate::equipment::TrimAssets) -> crate::held_items::HeldItems {
        use crate::held_items::{HeldItemModel, HeldItems, TexturePool};
        use crate::item_models::{resolve_definition, ItemGeometry, ItemModel, SelectionContext};

        let mut names: Vec<String> = self
            .jar
            .file_names()
            .filter_map(|p| {
                p.strip_prefix("assets/minecraft/items/")
                    .and_then(|r| r.strip_suffix(".json"))
                    .filter(|r| !r.contains('/'))
                    .map(str::to_string)
            })
            .collect();
        names.sort();

        // Phase 1: resolve definitions (no geometry yet).
        let mut resolved: Vec<(String, ItemModel, Option<ItemGeometry>)> =
            Vec::with_capacity(names.len());
        for name in &names {
            let Some(def) = self.read_json(&format!("assets/minecraft/items/{name}.json")) else {
                resolved.push((
                    name.clone(),
                    ItemModel::Unsupported("(missing definition)".into()),
                    None,
                ));
                continue;
            };
            let mut read_model =
                |p: &str| self.read_json(&format!("assets/minecraft/models/{p}.json"));
            let hand = resolve_definition(&def, &mut read_model, SelectionContext::hand());
            // Resolve a second time for the slot context. Almost always the
            // identical result — only a `select` on `minecraft:display_context`
            // can differ — so the geometry is compared rather than assumed,
            // and a second bake happens only when it really is different.
            let gui = resolve_definition(&def, &mut read_model, SelectionContext::gui());
            let gui_geometry = match (&hand, &gui) {
                (
                    ItemModel::Resolved { geometry: h, .. },
                    ItemModel::Resolved { geometry: g, .. },
                ) if h != g => Some(g.clone()),
                _ => None,
            };
            resolved.push((name.clone(), hand, gui_geometry));

            // M49: a trimmed icon is a *different model*, so it needs its own
            // bake. Keyed `"<item>#<material id>"` rather than by restructuring
            // the map: every existing lookup is by plain name and stays exact,
            // and a caller that knows a trim asks for the composed name first.
            //
            // Driven by the definition's own `cases` rather than by the
            // material registry, because the registry is the *server's* and
            // this is a load-time bake of the *jar's* assets — an item trimmed
            // with a material its own definition does not name has no icon
            // variant, which is vanilla's fallback.
            for material in trim_material_cases(&def) {
                let ctx = SelectionContext::hand().with_trim(Some(&material));
                let m = resolve_definition(&def, &mut read_model, ctx);
                let g = resolve_definition(
                    &def,
                    &mut read_model,
                    SelectionContext::gui().with_trim(Some(&material)),
                );
                let gg = match (&m, &g) {
                    (
                        ItemModel::Resolved { geometry: h, .. },
                        ItemModel::Resolved { geometry: q, .. },
                    ) if h != q => Some(q.clone()),
                    _ => None,
                };
                resolved.push((format!("{name}#{material}"), m, gg));
            }
        }

        // Phase 2: bake geometry.
        let mut pool = TexturePool::default();
        let mut models = HashMap::new();
        let mut unsupported: std::collections::BTreeMap<String, usize> = Default::default();
        for (name, model, gui_geometry) in resolved {
            let full = format!("minecraft:{name}");
            let (geometry, right, left, ground, gui, first_right, first_left) = match model {
                ItemModel::Resolved {
                    geometry,
                    third_person_right,
                    third_person_left,
                    ground,
                    gui,
                    first_person_right,
                    first_person_left,
                } => (
                    geometry,
                    third_person_right,
                    third_person_left,
                    ground,
                    gui,
                    first_person_right,
                    first_person_left,
                ),
                ItemModel::Unsupported(kind) => {
                    let bucket = if kind.starts_with("model ") {
                        "(bespoke model)".to_string()
                    } else {
                        kind
                    };
                    *unsupported.entry(bucket).or_default() += 1;
                    continue;
                }
            };
            let baked = match &geometry {
                ItemGeometry::Block(block) => self.bake_block_item(block, &mut pool),
                ItemGeometry::Sprite(layers) => self.bake_sprite_item(layers, &mut pool, trims),
            };
            // The slot geometry, when the definition selects a different
            // model there. A variant that fails to bake leaves `None`, so the
            // slot falls back to the hand's quads rather than drawing nothing.
            let gui_quads = gui_geometry.and_then(|g| match &g {
                ItemGeometry::Block(block) => self.bake_block_item(block, &mut pool),
                ItemGeometry::Sprite(layers) => self.bake_sprite_item(layers, &mut pool, trims),
            })
            .filter(|q| !q.is_empty());
            match baked {
                Some(quads) if !quads.is_empty() => {
                    models.insert(
                        full,
                        HeldItemModel {
                            quads,
                            right,
                            left,
                            ground,
                            gui,
                            first_right,
                            first_left,
                            from_block: matches!(geometry, ItemGeometry::Block(_)),
                            gui_quads,
                        },
                    );
                }
                // Geometry that resolved but produced nothing (an empty model,
                // an undecodable sprite) is suppressed like an unsupported
                // definition. Counting it as resolved would overstate the
                // coverage the gate reports.
                _ => *unsupported.entry("(no geometry)".to_string()).or_default() += 1,
            }
        }

        // M25b: block-entity models ride the same pool and map. They are not
        // items and are namespaced `rewo:be/…`, so they cannot be reached by
        // an item lookup — only by the block-entity draw path.
        // One loader, reborrowed per bake. Each `bake_*` takes `&mut dyn FnMut`
        // and the closure holds the jar, so they cannot be live at once —
        // hence the sequence rather than one call.
        macro_rules! bake_be {
            ($f:path) => {{
                let jar = &mut self.jar;
                $f(&mut pool, &mut |tex_name: &str| {
                    let path = format!("assets/minecraft/textures/{tex_name}.png");
                    let mut bytes = Vec::new();
                    jar.by_name(&path)
                        .ok()
                        .and_then(|mut e| e.read_to_end(&mut bytes).ok())?;
                    let (rgba, w, h) = decode_png_any(&bytes)?;
                    Some(crate::held_items::HeldTexture { w, h, rgba })
                })
            }};
        }
        let mut be_models = bake_be!(crate::block_entity_models::bake_chests);
        be_models.append(&mut bake_be!(
            crate::block_entity_models::bake_shulker_boxes
        ));
        be_models.append(&mut bake_be!(crate::block_entity_models::bake_skulls));
        be_models.append(&mut bake_be!(crate::block_entity_models::bake_conduit));
        be_models.append(&mut bake_be!(crate::block_entity_models::bake_banners));
        be_models.append(&mut bake_be!(
            crate::block_entity_models::bake_copper_golem_statues
        ));
        {
            // The pot bake validates every derived sherd texture against the
            // jar, so it returns a Result the others do not.
            let jar = &mut self.jar;
            let mut pots = crate::block_entity_models::bake_decorated_pot(
                &mut pool,
                &mut |tex_name: &str| {
                    let path = format!("assets/minecraft/textures/{tex_name}.png");
                    let mut bytes = Vec::new();
                    jar.by_name(&path)
                        .ok()
                        .and_then(|mut e| e.read_to_end(&mut bytes).ok())?;
                    let (rgba, w, h) = decode_png_any(&bytes)?;
                    Some(crate::held_items::HeldTexture { w, h, rgba })
                },
            );
            be_models.append(&mut pots);
        }
        let be_count = be_models.len();
        let block_entities: HashMap<String, HeldItemModel> = be_models.into_iter().collect();
        log::info!("rewo-data: {be_count} block-entity model(s) baked");

        let items = HeldItems {
            models,
            block_entities,
            textures: pool.into_textures(),
            unsupported,
        };
        log::info!(
            "rewo-data: held items — {} resolved ({} block + {} sprite), {} textures, {} unsupported",
            items.models.len(),
            items.block_count(),
            items.sprite_count(),
            items.textures.len(),
            items.unsupported_total()
        );
        items
    }

    /// A `minecraft:block/…` item: reuse the block model's quads and copy each
    /// referenced texture-array layer into the held-item texture pool.
    fn bake_block_item(
        &mut self,
        block: &str,
        pool: &mut crate::held_items::TexturePool,
    ) -> Option<Vec<crate::held_items::HeldQuad>> {
        use crate::held_items::{HeldQuad, HeldTexture};

        let mut quads = Vec::new();
        let r = ModelRef {
            model: format!("block/{block}"),
            x: 0,
            y: 0,
        };
        // No biome tint: a held item draws the neutral texture, and the
        // pre-tinted legacy layer is the one the block bake already chose.
        let tint = TintInfo {
            foliage: false,
            layers: &[],
            upper_half: false,
        };
        self.append_model_quads(&r, tint, &mut quads);
        if quads.is_empty() {
            return None;
        }
        let mut out = Vec::with_capacity(quads.len());
        for q in quads {
            // The layer pixels, copied so the entity atlas can hold them — the
            // entity pass cannot sample the world texture array.
            let layer = q.layer as usize;
            let layers = &self.layers;
            let tex = pool.intern(&format!("layer:{layer}"), || {
                let rgba = layers.get(layer)?.clone();
                (rgba.len() == (TEX_SIZE * TEX_SIZE * 4) as usize).then_some(HeldTexture {
                    w: TEX_SIZE,
                    h: TEX_SIZE,
                    rgba,
                })
            })?;
            out.push(HeldQuad {
                // The block bake normalises to 0..1; held-item space is the
                // 0..16 model units the display transform is written in.
                verts: q.verts.map(|v| [v[0] * 16.0, v[1] * 16.0, v[2] * 16.0]),
                uv: q.uv,
                tex,
                // Items never articulate.
                part: 0,
                dir: q.dir,
            });
        }
        Some(out)
    }

    /// A `builtin/generated` item: extrude each sprite layer.
    fn bake_sprite_item(
        &mut self,
        layers: &[String],
        pool: &mut crate::held_items::TexturePool,
        trims: &crate::equipment::TrimAssets,
    ) -> Option<Vec<crate::held_items::HeldQuad>> {
        use crate::held_items::{HeldQuad, HeldTexture};
        use crate::item_geometry::{extrude, SpriteMask};

        let mut out = Vec::new();
        for (i, tex_name) in layers.iter().enumerate() {
            let path = format!("assets/minecraft/textures/{tex_name}.png");
            let mut bytes = Vec::new();
            let read = self
                .jar
                .by_name(&path)
                .ok()
                .and_then(|mut e| e.read_to_end(&mut bytes).ok());
            // M49: a trimmed icon's layer1 is `trims/items/<piece>_trim_<material>`,
            // which is **not a file** — `items.json` generates it by palette
            // permutation exactly as `armor_trims.json` does for the entity
            // sheets. So a miss under that prefix is a sprite to make, not a
            // layer to drop.
            let permuted = if read.is_none() {
                tex_name
                    .strip_prefix("trims/items/")
                    .and_then(|_| tex_name.rsplit_once('_'))
                    .and_then(|(stem, suffix)| trims.permute(stem, suffix))
            } else {
                None
            };
            if read.is_none() && permuted.is_none() {
                continue; // a layer whose texture is absent contributes nothing
            }
            let Some((rgba, w, h)) = permuted.or_else(|| decode_png_any(&bytes)) else {
                continue;
            };
            // An animation strip stacks square frames; the item uses frame 0.
            let frame_h = w.min(h);
            let frame: Vec<u8> = rgba[..(w * frame_h * 4) as usize].to_vec();
            let mask = SpriteMask {
                width: w,
                height: frame_h,
                // `SpriteContents.isTransparent` is alpha == 0.
                transparent: (0..(w * frame_h) as usize)
                    .map(|p| frame[p * 4 + 3] == 0)
                    .collect(),
            };
            let Some(tex) = pool.intern(&format!("sprite:{tex_name}"), || {
                Some(HeldTexture {
                    w,
                    h: frame_h,
                    rgba: frame.clone(),
                })
            }) else {
                continue;
            };
            for q in extrude(&mask, i as u8) {
                out.push(HeldQuad {
                    // Items never articulate.
                    part: 0,
                    verts: q.verts,
                    // The extruder works in 0..16 sprite-model units; the
                    // renderer wants 0..1 of the texture.
                    uv: q.uv.map(|u| [u[0] / 16.0, u[1] / 16.0]),
                    tex,
                    dir: q.dir,
                });
            }
        }
        (!out.is_empty()).then_some(out)
    }

    fn resolve_model(&mut self, model: &str) -> Option<ResolvedModel> {
        let key = model.strip_prefix("minecraft:").unwrap_or(model).to_string();
        if let Some(c) = self.model_cache.get(&key) {
            return c.clone();
        }
        let resolved = self.resolve_model_uncached(&key);
        self.model_cache.insert(key, resolved.clone());
        resolved
    }

    fn resolve_model_uncached(&mut self, key: &str) -> Option<ResolvedModel> {
        let json = self.read_json(&format!("assets/minecraft/models/{key}.json"))?;
        let mut out = match json.get("parent").and_then(|p| p.as_str()) {
            Some(parent) => {
                // "builtin/*" parents (entity models) have no geometry.
                if parent.contains("builtin/") {
                    ResolvedModel {
                        textures: HashMap::new(),
                        elements: Vec::new(),
                        ambient_occlusion: true,
                    }
                } else {
                    self.resolve_model(parent).unwrap_or(ResolvedModel {
                        textures: HashMap::new(),
                        elements: Vec::new(),
                        ambient_occlusion: true,
                    })
                }
            }
            None => ResolvedModel {
                textures: HashMap::new(),
                elements: Vec::new(),
                ambient_occlusion: true,
            },
        };
        if let Some(textures) = json.get("textures").and_then(|t| t.as_object()) {
            for (var, value) in textures {
                // 26.x allows a texture to be either a plain ref string OR
                // an object `{sprite, force_translucent, …}` carrying render
                // metadata — take the `sprite` in the object case (glass,
                // tinted glass, ice, …), else the string.
                let name = value.as_str().map(str::to_string).or_else(|| {
                    value
                        .get("sprite")
                        .and_then(|s| s.as_str())
                        .map(str::to_string)
                });
                if let Some(name) = name {
                    out.textures.insert(var.clone(), name);
                }
            }
        }
        // A model defining its own elements overrides the parent's.
        if let Some(elements) = json.get("elements").and_then(|e| e.as_array()) {
            out.elements = elements.clone();
        }
        if let Some(ao) = json.get("ambientocclusion").and_then(|a| a.as_bool()) {
            out.ambient_occlusion = ao;
        }
        Some(out)
    }

    /// The texture-array layer a block-break shard samples (M37).
    ///
    /// Vanilla asks the block-state model set for
    /// `getParticleMaterial(state).sprite()`, which is the model's `particle`
    /// texture slot — the reason a broken grass_block throws *dirt*-coloured
    /// shards rather than green ones, even though its top face is green. So
    /// this resolves `#particle` through the merged parent chain rather than
    /// reusing a face texture, which would get exactly that case wrong.
    ///
    /// Untinted: vanilla multiplies the shard by the block's tint source
    /// separately, and baking a colormap into the layer here would double it.
    fn particle_layer_for(
        &mut self,
        bs: Option<&BlockState>,
        props: Option<&serde_json::Map<String, serde_json::Value>>,
    ) -> u16 {
        let Some(bs) = bs else {
            return NO_PARTICLE_LAYER;
        };
        let refs = self.state_refs(bs, props);
        let Some(first) = refs.first() else {
            return NO_PARTICLE_LAYER;
        };
        let Some(model) = self.resolve_model(&first.model) else {
            return NO_PARTICLE_LAYER;
        };
        let Some(name) = resolve_texture_var("#particle", &model.textures) else {
            return NO_PARTICLE_LAYER;
        };
        self.layer_for(&name, TintKind::None).unwrap_or(NO_PARTICLE_LAYER)
    }

    /// Texture-array layer for a texture name; `foliage` picks the tint
    /// colormap for grayscale grass/foliage textures.
    fn layer_for(&mut self, tex_name: &str, apply_tint: TintKind) -> Option<u16> {
        let cache_key = format!("{tex_name}#{apply_tint:?}");
        if let Some(&layer) = self.layer_index.get(&cache_key) {
            return Some(layer);
        }
        let short = tex_name.strip_prefix("minecraft:").unwrap_or(tex_name);
        let path = format!("assets/minecraft/textures/{short}.png");
        let mut bytes = Vec::new();
        self.jar.by_name(&path).ok()?.read_to_end(&mut bytes).ok()?;
        let mut frames = decode_png_rgba_frames(&bytes)?;
        // The `grass_block_top` / `short_grass` textures ship grayscale and
        // expect a colormap multiply — bake the plains color in so their
        // layer reads green. Per-biome variation is deferred. Tint applies
        // to every animation frame.
        let tint = match apply_tint {
            TintKind::Grass => Some(self.grass_tint),
            TintKind::Foliage => Some(self.foliage_tint),
            // Water ships grayscale + alpha 180; biome water color is a
            // registry value, not a colormap — plains #3F76E4 baked (the
            // same fixed-plains approach as grass until per-biome tint).
            TintKind::Water => Some([0x3F, 0x76, 0xE4]),
            TintKind::None => None,
        };
        if let Some(color) = tint {
            for f in &mut frames {
                tint_rgb(f, color);
            }
        }
        let layer = self.layers.len() as u16;
        // Multi-frame strip + .mcmeta → register the animation; the layer
        // itself holds frame 0 (also the static fallback).
        if frames.len() > 1 {
            if let Some((order, frametime)) = read_mcmeta(self.jar, &path, frames.len() as u32) {
                self.animations.push(AnimatedLayer {
                    layer,
                    frames: frames.clone(),
                    order,
                    frametime,
                });
            }
        }
        self.layers.push(frames.into_iter().next()?);
        self.layer_names.push(cache_key.clone());
        self.layer_index.insert(cache_key, layer);
        Some(layer)
    }
}

/// Parse `<texture>.png.mcmeta` → (frame order, frametime in ticks).
/// Missing `frames` list = sequential; missing `frametime` = 1.
fn read_mcmeta(jar: Jar, png_path: &str, frame_count: u32) -> Option<(Vec<u32>, u32)> {
    let mut bytes = Vec::new();
    jar.by_name(&format!("{png_path}.mcmeta"))
        .ok()?
        .read_to_end(&mut bytes)
        .ok()?;
    let json: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let anim = json.get("animation")?;
    let frametime = anim
        .get("frametime")
        .and_then(|f| f.as_u64())
        .unwrap_or(1)
        .max(1) as u32;
    let order = match anim.get("frames").and_then(|f| f.as_array()) {
        Some(list) => list
            .iter()
            .filter_map(|v| v.as_u64())
            .map(|v| (v as u32).min(frame_count - 1))
            .collect(),
        None => (0..frame_count).collect(),
    };
    Some((order, frametime))
}

#[derive(Clone, Copy, Debug)]
enum TintKind {
    None,
    Water,
    Grass,
    Foliage,
}

/// Grayscale textures that must be colormap-tinted at bake time (their model
/// faces don't always carry a tintindex, e.g. grass_block_top).
fn foliage_of(tex_name: &str, block_is_foliage: bool) -> TintKind {
    let short = tex_name.rsplit('/').next().unwrap_or(tex_name);
    if short.contains("grass_block_top") || short == "short_grass" || short == "tall_grass_top" {
        TintKind::Grass
    } else if block_is_foliage {
        TintKind::Foliage
    } else {
        TintKind::None
    }
}

fn is_foliage(block: &str) -> bool {
    block.ends_with("_leaves") || block == "vine" || block.contains("_stem")
}

// -- geometry ----------------------------------------------------------------

/// Corner positions (0..16) + uv (0..1) for one face of a box, textures
/// upright. Winding is irrelevant (mesher renders cull-none); only the
/// corner↔uv pairing matters visually.
fn face_geometry(
    f: [f32; 3],
    t: [f32; 3],
    face: usize,
    uv_field: Option<&serde_json::Value>,
) -> ([[f32; 3]; 4], [[f32; 2]; 4]) {
    let (fx, fy, fz) = (f[0], f[1], f[2]);
    let (tx, ty, tz) = (t[0], t[1], t[2]);
    // [TL, TR, BR, BL] with textures upright (+Y is up for side faces).
    let verts = match face {
        0 => [
            [fx, ty, fz],
            [tx, ty, fz],
            [tx, ty, tz],
            [fx, ty, tz],
        ], // up
        1 => [
            [fx, fy, fz],
            [tx, fy, fz],
            [tx, fy, tz],
            [fx, fy, tz],
        ], // down
        2 => [
            [tx, ty, fz],
            [fx, ty, fz],
            [fx, fy, fz],
            [tx, fy, fz],
        ], // north (-Z)
        3 => [
            [fx, ty, tz],
            [tx, ty, tz],
            [tx, fy, tz],
            [fx, fy, tz],
        ], // south (+Z)
        4 => [
            [fx, ty, fz],
            [fx, ty, tz],
            [fx, fy, tz],
            [fx, fy, fz],
        ], // west (-X)
        _ => [
            [tx, ty, tz],
            [tx, ty, fz],
            [tx, fy, fz],
            [tx, fy, tz],
        ], // east (+X)
    };
    // uv rect [u1,v1,u2,v2] in 0..16 texture space; default = face extent.
    let rect = uv_field
        .and_then(|u| u.as_array())
        .filter(|a| a.len() == 4)
        .map(|a| {
            [
                a[0].as_f64().unwrap_or(0.0) as f32,
                a[1].as_f64().unwrap_or(0.0) as f32,
                a[2].as_f64().unwrap_or(16.0) as f32,
                a[3].as_f64().unwrap_or(16.0) as f32,
            ]
        })
        .unwrap_or_else(|| default_uv(f, t, face));
    let (u1, v1, u2, v2) = (rect[0] / 16.0, rect[1] / 16.0, rect[2] / 16.0, rect[3] / 16.0);
    let uv = [[u1, v1], [u2, v1], [u2, v2], [u1, v2]];
    (verts, uv)
}

/// Default uv from the element's extent projected onto the face.
fn default_uv(f: [f32; 3], t: [f32; 3], face: usize) -> [f32; 4] {
    match face {
        0 | 1 => [f[0], f[2], t[0], t[2]], // x,z
        2 | 3 => [f[0], 16.0 - t[1], t[0], 16.0 - f[1]], // x,y
        _ => [f[2], 16.0 - t[1], t[2], 16.0 - f[1]],     // z,y
    }
}

fn box_coords(el: &serde_json::Value, key: &str) -> Option<[f32; 3]> {
    let a = el.get(key)?.as_array()?;
    Some([
        a[0].as_f64()? as f32,
        a[1].as_f64()? as f32,
        a[2].as_f64()? as f32,
    ])
}

/// Rotate corners around an element rotation {origin, axis, angle, rescale}.
fn apply_element_rotation(verts: &mut [[f32; 3]; 4], rot: &serde_json::Value) {
    let origin = rot
        .get("origin")
        .and_then(|o| o.as_array())
        .map(|a| {
            [
                a[0].as_f64().unwrap_or(8.0) as f32,
                a[1].as_f64().unwrap_or(8.0) as f32,
                a[2].as_f64().unwrap_or(8.0) as f32,
            ]
        })
        .unwrap_or([8.0, 8.0, 8.0]);
    let axis = rot.get("axis").and_then(|a| a.as_str()).unwrap_or("y");
    let angle = rot.get("angle").and_then(|a| a.as_f64()).unwrap_or(0.0) as f32;
    let rescale = rot.get("rescale").and_then(|r| r.as_bool()).unwrap_or(false);
    let rad = angle.to_radians();
    let (s, c) = (rad.sin(), rad.cos());
    let scale = if rescale && c.abs() > 1e-4 {
        1.0 / c.abs()
    } else {
        1.0
    };
    let ai = match axis {
        "x" => 0,
        "z" => 2,
        _ => 1,
    };
    for v in verts.iter_mut() {
        let mut p = [v[0] - origin[0], v[1] - origin[1], v[2] - origin[2]];
        let (a, b) = match ai {
            0 => (1, 2),
            2 => (0, 1),
            _ => (2, 0),
        };
        let (pa, pb) = (p[a], p[b]);
        p[a] = pa * c - pb * s;
        p[b] = pa * s + pb * c;
        if rescale {
            p[a] *= scale;
            p[b] *= scale;
        }
        v[0] = p[0] + origin[0];
        v[1] = p[1] + origin[1];
        v[2] = p[2] + origin[2];
    }
}

/// Rotate a point (0..16) by whole-model x then y (0/90/180/270) around
/// block center (8,8,8).
fn rotate_xy(v: &mut [f32; 3], x: i32, y: i32) {
    let c = [8.0f32, 8.0, 8.0];
    let mut p = [v[0] - c[0], v[1] - c[1], v[2] - c[2]];
    for _ in 0..((x / 90).rem_euclid(4)) {
        // x rotation: y→z, z→-y  (90° about +X)
        let (ny, nz) = (-p[2], p[1]);
        p[1] = ny;
        p[2] = nz;
    }
    for _ in 0..((y / 90).rem_euclid(4)) {
        // y rotation: x→-z, z→x  (90° about +Y)
        let (nx, nz) = (p[2], -p[0]);
        p[0] = nx;
        p[2] = nz;
    }
    v[0] = p[0] + c[0];
    v[1] = p[1] + c[1];
    v[2] = p[2] + c[2];
}

fn rotate_normal_element(n: &mut [f32; 3], rot: &serde_json::Value) {
    let axis = rot.get("axis").and_then(|a| a.as_str()).unwrap_or("y");
    let angle = rot.get("angle").and_then(|a| a.as_f64()).unwrap_or(0.0) as f32;
    let rad = angle.to_radians();
    let (s, c) = (rad.sin(), rad.cos());
    let (a, b) = match axis {
        "x" => (1, 2),
        "z" => (0, 1),
        _ => (2, 0),
    };
    let (na, nb) = (n[a], n[b]);
    n[a] = na * c - nb * s;
    n[b] = na * s + nb * c;
}

fn rotate_normal_xy(n: &mut [f32; 3], x: i32, y: i32) {
    for _ in 0..((x / 90).rem_euclid(4)) {
        let (ny, nz) = (-n[2], n[1]);
        n[1] = ny;
        n[2] = nz;
    }
    for _ in 0..((y / 90).rem_euclid(4)) {
        let (nx, nz) = (n[2], -n[0]);
        n[0] = nx;
        n[2] = nz;
    }
}

/// Snap a normal to the nearest of the six face directions.
fn snap_face(n: [f32; 3]) -> u8 {
    let mut best = 0u8;
    let mut best_dot = f32::MIN;
    for (i, fn_) in FACE_NORMALS.iter().enumerate() {
        let d = n[0] * fn_[0] + n[1] * fn_[1] + n[2] * fn_[2];
        if d > best_dot {
            best_dot = d;
            best = i as u8;
        }
    }
    best
}

/// True if all 4 corners sit flush against the block boundary on `dir`'s axis
/// (so a cullface is meaningful after rotation).
fn is_axis_flush(verts: &[[f32; 3]; 4], dir: u8) -> bool {
    let (axis, val) = match dir {
        0 => (1, 1.0),
        1 => (1, 0.0),
        2 => (2, 0.0),
        3 => (2, 1.0),
        4 => (0, 0.0),
        _ => (0, 1.0),
    };
    verts.iter().all(|v| (v[axis] - val).abs() < 1e-3)
}

// -- variant / multipart selection -------------------------------------------

fn model_ref(apply: &serde_json::Value) -> Option<ModelRef> {
    // `apply` may be an object or an array (random weighted) — take the first.
    let obj = if let Some(arr) = apply.as_array() {
        arr.first()?
    } else {
        apply
    };
    Some(ModelRef {
        model: obj.get("model")?.as_str()?.to_string(),
        x: obj.get("x").and_then(|v| v.as_i64()).unwrap_or(0) as i32,
        y: obj.get("y").and_then(|v| v.as_i64()).unwrap_or(0) as i32,
    })
}

fn pick_variant(
    variants: &serde_json::Map<String, serde_json::Value>,
    props: Option<&serde_json::Map<String, serde_json::Value>>,
) -> Option<ModelRef> {
    for (key, value) in variants {
        if variant_matches(key, props) {
            return model_ref(value);
        }
    }
    None
}

fn variant_matches(
    key: &str,
    props: Option<&serde_json::Map<String, serde_json::Value>>,
) -> bool {
    if key.is_empty() {
        return true;
    }
    let Some(props) = props else { return false };
    key.split(',').all(|pair| {
        let Some((k, v)) = pair.split_once('=') else {
            return false;
        };
        props.get(k).and_then(|pv| pv.as_str()) == Some(v)
    })
}

/// Evaluate a multipart entry's `when` against the state properties.
fn multipart_applies(
    part: &serde_json::Value,
    props: Option<&serde_json::Map<String, serde_json::Value>>,
) -> bool {
    let Some(when) = part.get("when") else {
        return true; // unconditional
    };
    when_matches(when, props)
}

fn when_matches(
    when: &serde_json::Value,
    props: Option<&serde_json::Map<String, serde_json::Value>>,
) -> bool {
    // OR of sub-conditions.
    if let Some(or) = when.get("OR").and_then(|o| o.as_array()) {
        return or.iter().any(|c| when_matches(c, props));
    }
    if let Some(and) = when.get("AND").and_then(|a| a.as_array()) {
        return and.iter().all(|c| when_matches(c, props));
    }
    let Some(obj) = when.as_object() else {
        return true;
    };
    let Some(props) = props else { return false };
    obj.iter().all(|(k, v)| {
        let Some(pv) = props.get(k).and_then(|p| p.as_str()) else {
            return false;
        };
        // Value may be "a|b" (any-of).
        match v.as_str() {
            Some(s) => s.split('|').any(|opt| opt == pv),
            None => false,
        }
    })
}

// -- textures ----------------------------------------------------------------

fn resolve_texture_var<'a>(
    mut tex_ref: &'a str,
    textures: &'a HashMap<String, String>,
) -> Option<String> {
    for _ in 0..8 {
        if let Some(var) = tex_ref.strip_prefix('#') {
            tex_ref = textures.get(var)?;
        } else {
            return Some(tex_ref.to_string());
        }
    }
    None
}


fn decode_png_rgba(bytes: &[u8]) -> Option<Vec<u8>> {
    decode_png_rgba_frames(bytes)?.into_iter().next()
}

/// Decode a block texture into its 16×16 RGBA frames — one for a plain
/// texture, N for an animation strip (width 16, height N×16).
fn decode_png_rgba_frames(bytes: &[u8]) -> Option<Vec<Vec<u8>>> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0u8; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    if info.width != TEX_SIZE || info.height < TEX_SIZE || info.height % TEX_SIZE != 0 {
        return None;
    }
    let n = (info.width as usize) * (info.height as usize);
    let mut rgba = vec![0u8; n * 4];
    match info.color_type {
        png::ColorType::Rgba => rgba.copy_from_slice(&buf[..n * 4]),
        png::ColorType::Rgb => {
            for i in 0..n {
                rgba[i * 4..i * 4 + 3].copy_from_slice(&buf[i * 3..i * 3 + 3]);
                rgba[i * 4 + 3] = 255;
            }
        }
        png::ColorType::GrayscaleAlpha => {
            for i in 0..n {
                let g = buf[i * 2];
                rgba[i * 4] = g;
                rgba[i * 4 + 1] = g;
                rgba[i * 4 + 2] = g;
                rgba[i * 4 + 3] = buf[i * 2 + 1];
            }
        }
        png::ColorType::Grayscale => {
            for i in 0..n {
                let g = buf[i];
                rgba[i * 4] = g;
                rgba[i * 4 + 1] = g;
                rgba[i * 4 + 2] = g;
                rgba[i * 4 + 3] = 255;
            }
        }
        png::ColorType::Indexed => return None,
    }
    let frame_bytes = (TEX_SIZE * TEX_SIZE * 4) as usize;
    Some(rgba.chunks_exact(frame_bytes).map(|c| c.to_vec()).collect())
}

/// Center pixel of a colormap PNG (the "plains" reference point).
fn colormap_center(jar: Jar, name: &str) -> Option<[u8; 3]> {
    let path = format!("assets/minecraft/textures/colormap/{name}.png");
    let mut bytes = Vec::new();
    jar.by_name(&path).ok()?.read_to_end(&mut bytes).ok()?;
    let mut decoder = png::Decoder::new(std::io::Cursor::new(&bytes));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0u8; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    let (w, h) = (info.width as usize, info.height as usize);
    let ch = match info.color_type {
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        _ => return None,
    };
    // Plains ≈ (temp 0.8, downpour 0.4) → about 62% across, 45% down.
    let x = (w as f32 * 0.55) as usize;
    let y = (h as f32 * 0.55) as usize;
    let i = (y.min(h - 1) * w + x.min(w - 1)) * ch;
    Some([buf[i], buf[i + 1], buf[i + 2]])
}

/// Decode a full biome colormap PNG (`grass.png` / `foliage.png` /
/// `dry_foliage.png`, 256×256) into `65536` ARGB ints indexed `y<<8 | x` —
/// exactly the `pixels` array vanilla feeds `GrassColor.init` (via
/// `NativeImage.makePixelArray` → `ARGB.fromABGR(getPixelABGR)` = ARGB). A
/// non-256-wide image is placed by the natural `y<<8 | x` layout;
/// `ColorMapColorUtil.get` only ever indexes `y<<8|x`, so a 256-wide map lines
/// up 1:1.
pub fn colormap_pixels(jar: Jar, name: &str) -> Option<Vec<i32>> {
    let path = format!("assets/minecraft/textures/colormap/{name}.png");
    let mut bytes = Vec::new();
    jar.by_name(&path).ok()?.read_to_end(&mut bytes).ok()?;
    let mut decoder = png::Decoder::new(std::io::Cursor::new(&bytes));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0u8; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    let (w, h) = (info.width as usize, info.height as usize);
    let ch = match info.color_type {
        png::ColorType::Rgb => 3,
        png::ColorType::Rgba => 4,
        _ => return None,
    };
    // Build a 256×256 = 65536 ARGB array (the size the index space needs).
    let mut pixels = vec![0i32; 256 * 256];
    for y in 0..h.min(256) {
        for x in 0..w.min(256) {
            let i = (y * w + x) * ch;
            let r = buf[i] as i32;
            let g = buf[i + 1] as i32;
            let b = buf[i + 2] as i32;
            let a = if ch == 4 { buf[i + 3] as i32 } else { 255 };
            pixels[(y << 8) | x] =
                ((a & 0xFF) << 24) | ((r & 0xFF) << 16) | ((g & 0xFF) << 8) | (b & 0xFF);
        }
    }
    Some(pixels)
}

fn tint_rgb(rgba: &mut [u8], color: [u8; 3]) {
    for px in rgba.chunks_exact_mut(4) {
        for c in 0..3 {
            px[c] = ((px[c] as u16 * color[c] as u16) / 255) as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snap_face_picks_axis() {
        assert_eq!(snap_face([0.0, 1.0, 0.0]), 0); // up
        assert_eq!(snap_face([0.0, -0.9, 0.1]), 1); // down
        assert_eq!(snap_face([0.0, 0.0, -1.0]), 2); // north
        assert_eq!(snap_face([1.0, 0.0, 0.0]), 5); // east
    }

    // M14: tint family comes from BlockColors (block id + tintindex), NOT the
    // texture filename. These pin the decompiled `BlockColors.createDefault`.
    #[test]
    fn block_color_layers_follow_blockcolors_not_filename() {
        use TintSource::*;
        assert_eq!(block_color_layers("grass_block"), &[Grass]);
        assert_eq!(block_color_layers("short_grass"), &[Grass]);
        assert_eq!(block_color_layers("fern"), &[Grass]);
        assert_eq!(block_color_layers("sugar_cane"), &[Grass]);
        assert_eq!(
            block_color_layers("oak_leaves"),
            &[Foliage],
            "oak leaves use the foliage colormap"
        );
        assert_eq!(block_color_layers("leaf_litter"), &[DryFoliage]);
        // pink_petals/wildflowers: layer 0 blank, layer 1 grass.
        assert_eq!(block_color_layers("pink_petals"), &[None, Grass]);
        // stone / dirt / any untinted block → no tint.
        assert_eq!(block_color_layers("stone"), &[] as &[TintSource]);
        assert_eq!(block_color_layers("dirt"), &[] as &[TintSource]);
    }

    #[test]
    fn spruce_and_birch_leaves_are_constant_not_foliage() {
        // The load-bearing "preserve constant spruce/birch" requirement: these
        // must be a fixed color, not the foliage colormap.
        assert_eq!(block_color_layers("spruce_leaves"), &[SPRUCE_LEAF]);
        assert_eq!(block_color_layers("birch_leaves"), &[BIRCH_LEAF]);
        // FoliageColor.FOLIAGE_EVERGREEN = -10380959, FOLIAGE_BIRCH = -8345771.
        assert_eq!(SPRUCE_LEAF, TintSource::Constant(rgb_of(-10380959)));
        assert_eq!(BIRCH_LEAF, TintSource::Constant(rgb_of(-8345771)));
        assert_ne!(block_color_layers("spruce_leaves"), &[TintSource::Foliage]);
    }

    #[test]
    fn resolve_tint_source_by_tintindex_and_half() {
        use TintSource::*;
        let petals = block_color_layers("pink_petals"); // [None, Grass]
        // tintindex 0 → blank, tintindex 1 → grass.
        assert_eq!(resolve_tint_source(petals, Some(0), false), None);
        assert_eq!(resolve_tint_source(petals, Some(1), false), Grass);
        // No tintindex on the face → never tinted.
        assert_eq!(resolve_tint_source(&[Grass], Option::None, false), None);
        // Out-of-range tintindex → None.
        assert_eq!(resolve_tint_source(&[Grass], Some(5), false), None);
        // Tall-grass UPPER half turns Grass into GrassBelow (samples pos.below()).
        assert_eq!(resolve_tint_source(&[Grass], Some(0), true), GrassBelow);
        // But a non-grass source is unaffected by the half.
        assert_eq!(resolve_tint_source(&[Foliage], Some(0), true), Foliage);
    }

    #[test]
    fn y_rotation_cycles_north_to_east() {
        // north normal (0,0,-1) rotated y=90 → east/west axis.
        let mut n = [0.0, 0.0, -1.0];
        rotate_normal_xy(&mut n, 0, 90);
        assert_eq!(snap_face(n), 4); // -X (west) — a 90° turn off north
    }

    #[test]
    fn slab_default_uv_covers_face() {
        // Full 16-wide top face → uv spans 0..1.
        let uv = default_uv([0.0, 0.0, 0.0], [16.0, 8.0, 16.0], 0);
        assert_eq!(uv, [0.0, 0.0, 16.0, 16.0]);
    }

    #[test]
    fn fluid_light_matches_vanilla() {
        // A full bake needs the client jar + blocks.json, so test the exact
        // fluid light assignment in isolation. Water: no emission, and
        // LiquidBlock.propagatesSkylightDown = false → getLightDampening = 1.
        // Lava: block_light::EMISSION pins it to 15, same dampening.
        assert_eq!(fluid_light(false), (0, 1), "water = (emission 0, dampening 1)");
        assert_eq!(fluid_light(true), (15, 1), "lava = (emission 15, dampening 1)");
    }

    #[test]
    fn font_advance_measures_rightmost_lit_column() {
        // 16×16 grid of 2-px cells; light up column 1 of glyph 'A' (65).
        let (size, cell) = (32u32, 2u32);
        let mut atlas = vec![0u8; (size * size * 4) as usize];
        let (cx, cy) = ((65 % 16) * cell, (65 / 16) * cell);
        atlas[((cy * size + cx + 1) * 4 + 3) as usize] = 255;
        let adv = font_advances(&atlas, size, cell);
        assert_eq!(adv[65], 3, "rightmost col 1 → advance 1+2");
        assert_eq!(adv[32], 4, "blank space cell advances 4");
    }
}

/// Rotate a block-local `0..1` box by a blockstate rotation (multiples of 90°
/// about X then Y, vanilla's `x`/`y` model fields). Axis-aligned boxes stay
/// axis-aligned at right angles, so this is a corner swap, not a real rotation.
fn rotate_box(b: [f32; 6], x_deg: i32, y_deg: i32) -> [f32; 6] {
    let mut lo = [b[0], b[1], b[2]];
    let mut hi = [b[3], b[4], b[5]];
    let steps = |d: i32| ((d % 360) + 360) % 360 / 90;
    for _ in 0..steps(x_deg) {
        // x rotation: y -> z, z -> -y (about the block centre)
        let (l, h) = (lo, hi);
        lo[1] = 1.0 - h[2];
        hi[1] = 1.0 - l[2];
        lo[2] = l[1];
        hi[2] = h[1];
    }
    for _ in 0..steps(y_deg) {
        // y rotation: x -> z, z -> -x
        let (l, h) = (lo, hi);
        lo[0] = 1.0 - h[2];
        hi[0] = 1.0 - l[2];
        lo[2] = l[0];
        hi[2] = h[0];
    }
    [lo[0], lo[1], lo[2], hi[0], hi[1], hi[2]]
}

/// Whether a non-full-cube block should collide using its *model* geometry,
/// and whether that shape is fence-tall.
///
/// Vanilla stores collision shapes in Java code, not in any datagen report, so
/// there is no ground-truth table to generate from (unlike the entity
/// hierarchies in `rewo-gpu::vanilla_hier`). Deriving shapes for *every* block
/// would be wrong in the obvious direction — torches, plants, rails and
/// redstone all have models but no collision, so the player would bump into
/// flowers. This list is therefore deliberately conservative: it names only
/// families whose model matches vanilla's collision closely, and everything
/// outside it keeps the previous behaviour (a full cube when `solid`, else
/// nothing). It can only ever *add* collision where we're confident.
fn model_collision(short: &str) -> Option<bool> {
    // Fence-likes: vanilla collides them 1.5 blocks tall so they can't be
    // jumped. Checked before `_fence` since `_fence_gate` also ends in `_gate`.
    if short.ends_with("_fence") || short == "fence" || short.ends_with("_fence_gate")
        || short.ends_with("_wall") || short == "wall"
    {
        return Some(true);
    }
    // `_trapdoor` must precede `_door` — it also ends with "door".
    let model_shaped = short.ends_with("_slab")
        || short.ends_with("_stairs")
        || short.ends_with("_trapdoor")
        || short.ends_with("_door")
        || short.ends_with("_carpet")
        || short == "snow"
        || short.ends_with("_bed")
        || short.ends_with("chest")
        || short.ends_with("_shulker_box")
        || short == "shulker_box"
        || short.ends_with("cauldron")
        || short.ends_with("anvil")
        || short == "hopper"
        || short == "composter"
        || short == "stonecutter"
        || short == "enchanting_table"
        || short == "end_portal_frame"
        || short == "daylight_detector"
        || short == "grindstone"
        || short == "lectern"
        || short == "cake"
        // The rest of vanilla's `useShapeForLightOcclusion` set. These carry
        // real collision shapes too, but they earn their place here because
        // light occlusion is computed from these boxes: a block with no boxes
        // has no occluding faces, so farmland would let light fall straight
        // through it where vanilla stops it at the full bottom face.
        || short == "farmland"
        || short == "dirt_path"
        || short == "sculk_sensor"
        || short == "sculk_shrieker"
        || short == "shelf"
        || short == "piston"
        || short == "sticky_piston"
        || short == "piston_head";
    model_shaped.then_some(false)
}

/// Neighbour offsets matching the bit order of [`BakedAssets::face_occludes`]
/// and the light engine's neighbour loop: −X, +X, −Y, +Y, −Z, +Z.
pub const FACE_DIRS: [(i32, i32, i32); 6] = [
    (-1, 0, 0),
    (1, 0, 0),
    (0, -1, 0),
    (0, 1, 0),
    (0, 0, -1),
    (0, 0, 1),
];

/// Which of the six faces this box list fully covers.
///
/// Vanilla compares real `VoxelShape`s; every vanilla block shape lies on
/// 1/16 boundaries, so rasterising each face at 16×16 and asking whether every
/// cell is covered gives the same answer for the shapes that matter, without
/// carrying a shape algebra. Boxes are block-local `0..1`.
fn face_coverage(boxes: &[[f32; 6]]) -> u8 {
    if boxes.is_empty() {
        return 0;
    }
    const EPS: f32 = 1.0e-4;
    let mut mask = 0u8;
    for (f, (dx, dy, dz)) in FACE_DIRS.iter().enumerate() {
        // The two axes spanning this face, and the plane the face sits on.
        let (axis, positive) = match (dx, dy, dz) {
            (-1, 0, 0) => (0, false),
            (1, 0, 0) => (0, true),
            (0, -1, 0) => (1, false),
            (0, 1, 0) => (1, true),
            (0, 0, -1) => (2, false),
            _ => (2, true),
        };
        let (u_axis, v_axis) = match axis {
            0 => (1, 2),
            1 => (0, 2),
            _ => (0, 1),
        };
        let mut covered = [[false; 16]; 16];
        for b in boxes {
            // Only boxes flush with this face occlude across it.
            let flush = if positive {
                b[axis + 3] >= 1.0 - EPS
            } else {
                b[axis] <= EPS
            };
            if !flush {
                continue;
            }
            let (u0, u1) = (b[u_axis], b[u_axis + 3]);
            let (v0, v1) = (b[v_axis], b[v_axis + 3]);
            for (ui, row) in covered.iter_mut().enumerate() {
                let uc = (ui as f32 + 0.5) / 16.0;
                if uc < u0 || uc > u1 {
                    continue;
                }
                for (vi, cell) in row.iter_mut().enumerate() {
                    let vc = (vi as f32 + 0.5) / 16.0;
                    if vc >= v0 && vc <= v1 {
                        *cell = true;
                    }
                }
            }
        }
        if covered.iter().all(|row| row.iter().all(|c| *c)) {
            mask |= 1 << f;
        }
    }
    mask
}
