//! Entity pass — textured mob models + capsule fallback + floating
//! nametags.
//!
//! Mob geometry comes from [`crate::mobs`] — a faithful port of vanilla's
//! `ModelPart.Cube` (see that module for the coordinate contract). This
//! module owns everything GPU-side: the shared atlas (font + all mob skins,
//! shelf-packed 64×64 slots), per-frame animation (vanilla `setupAnim`
//! angles about each part's pivot), the vanilla entity transform
//! (`rotY(180−yaw) · scale(−1,−1,1) · translate(0,−1.501,0)`), and the
//! draw pipelines.
//!
//! Deliberately simple: geometry is a **CPU-built world-space triangle
//! soup** rebuilt every frame — model quads, capsule shells (~500 verts
//! each), and camera-billboarded glyph quads. Entity counts are dozens, not
//! thousands, so building on the CPU costs microseconds and avoids
//! instancing + billboard shaders entirely; revisit if entity counts ever
//! grow 100×.
//!
//! One vertex format (pos, uv, rgba) through one shader family: glyphs
//! sample the font atlas, solid geometry (capsules, tag backgrounds)
//! samples the atlas's patched opaque-white texel. Two pipelines split
//! depth behavior: solid writes depth (GREATER — reversed-Z), text blends
//! with depth-write off. Both mask alpha writes (render discipline #2) and
//! take pre-linearized vertex colors (discipline #1).
//!
//! Vertices live in a [`crate::buf_ring::BufRing`] written by `set_draws`.
//! `set_draws` runs before the frame's fence wait, so a slot must survive
//! `fif + 1` frames rather than `fif` — the ring's own rule (M86).
//!
//! Verification knob: `REWO_MOB_DEBUG_TEX=1` replaces every mob texture
//! with facelabel colors (each box-UV face rect painted its
//! [`mobs::Facing::debug_color`]) — `rewo mobshot --check` renders each mob
//! and asserts the rendered dominant color matches the geometric
//! prediction, proving texture-face correspondence end-to-end.

use ash::vk;
use gpu_allocator::vulkan::{Allocation, AllocationCreateDesc, AllocationScheme};
use gpu_allocator::MemoryLocation;

use crate::mobs::{self, Facing};
use crate::world::DEPTH_FORMAT;
use crate::Gpu;

pub use crate::mobs::EntityModelKind;

const VERTEX_STRIDE: u64 = 52; // 3 pos + 2 uv + 4 rgba + 3 light + 1 hurt f32s

// The pipeline's vertex stride and every byte count handed to the ring use
// this constant, so the struct written into the ring must be exactly it.
const _: () = assert!(std::mem::size_of::<Vertex>() as u64 == VERTEX_STRIDE);
/// ~500 capsules' worth (a flat-world slime herd alone reaches 129
/// entities ≈ 65k verts). 13.6 MB per ring slot — cheap; the CPU soup
/// build is the real ceiling long before this is.
const MAX_VERTS: usize = 262_144;
/// Capsule tessellation: segments around Y × profile bands.
const SEGMENTS: usize = 12;
/// Nametag world scale per font pixel at cell=8 (vanilla's 0.025).
const TAG_PX: f32 = 0.025;
/// Tag anchor height above the entity's head.
const TAG_LIFT: f32 = 0.4;

// --- health bars (M59) ------------------------------------------------
//
// `REWO_HEALTH_BAR_SPEC.md` is the source of truth for every number below.
// It is the one part of Rewo with **no vanilla oracle** — vanilla renders no
// health bar over any entity — so these carry a spec citation where every
// other constant in this file carries a decompile one. They are *chosen*,
// and the spec exists so `healthbarshot` grades them against a written
// decision rather than against a restatement of this code.
//
// Units are **font pixels**, the same unit `push_tag` lays a nametag out in,
// so the bar rides `TAG_PX` and scales with distance exactly as a tag does.

/// The fill's full width (spec `BAR_W`).
const BAR_W: f32 = 40.0;
/// The fill's height (spec `BAR_H`).
const BAR_H: f32 = 3.0;
/// The plate's margin around the fill on all four sides (spec `BAR_PAD`) —
/// the same 1 px the nametag plate uses.
const BAR_PAD: f32 = 1.0;
/// How far below the tag anchor the bar hangs when a nametag is present
/// (spec `BAR_GAP`).
const BAR_GAP: f32 = 2.0;
/// Below this fraction the fill takes its critical colour (spec
/// `CRITICAL_FRAC`). Strictly below: at exactly the threshold the bar is
/// still healthy.
const CRITICAL_FRAC: f32 = 0.25;
/// The backing plate — **identical to the nametag plate's** `[0, 0, 0, 0.25]`,
/// which is the point: the two surfaces can never drift apart.
const BAR_PLATE: [f32; 4] = [0.0, 0.0, 0.0, 0.25];
/// The fill at or above [`CRITICAL_FRAC`].
const BAR_FILL_HEALTHY: [f32; 4] = [0.85, 0.20, 0.20, 1.0];
/// The fill below [`CRITICAL_FRAC`].
const BAR_FILL_CRITICAL: [f32; 4] = [0.95, 0.55, 0.15, 1.0];

/// One entity's health, as the two numbers the bar divides (M59).
///
/// A pair rather than a pre-divided fraction so the **emitter** owns the
/// arithmetic — the clamps, the hide-at-full rule and the colour threshold
/// are all properties of the render, and a caller that handed over a
/// fraction would be the place they silently diverged.
///
/// `EntityDraw::health` is `None` for every entity that must not show a bar
/// at all; the resolver upstream owns that decision (spec rules 4 and 5),
/// because it is the only side that knows what an entity *is*.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HealthBar {
    /// `LivingEntity.getHealth()` — metadata index 9, FLOAT.
    pub current: f32,
    /// `Attributes.MAX_HEALTH`, resolved from a **synced**
    /// `update_attributes` snapshot. Never a fallback: see spec rule 4.
    pub max: f32,
}

/// Borrowed view of `rewo_data::assets::BakedFont` — keeps this crate free
/// of a rewo-data dependency (same pattern as the texture-layer slices).
pub struct FontData<'a> {
    pub atlas: &'a [u8],
    pub size: u32,
    pub cell: u32,
    pub advance: &'a [u8; 256],
    pub white_texel: (u32, u32),
}

/// Entity state the vanilla emissive layers read ([`mobs::EmissiveAlpha`]).
///
/// Every field defaults to vanilla's *synched default*, so an entity whose
/// metadata or entity_event we don't decode renders exactly like a
/// freshly-spawned one rather than approximately.
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct EmissiveState {
    /// `Warden.getTendrilAnimation(partial)` in `0..1` — a 10-tick countdown
    /// started by **entity_event 61** (the warden hearing a vibration), so the
    /// resting value really is 0 and the tendrils hang still.
    pub tendril: f32,
    /// `Creaking.isActive()` — metadata `IS_ACTIVE`, whose vanilla
    /// `defineSynchedData` default is `false`.
    pub eyes_glow: bool,
}

/// The most sub-layers one armour piece may draw.
///
/// Two, because the 26.2 jar's largest humanoid layer list is leather's two
/// (a dyeable base plus its overlay) and the other twenty are one. A piece
/// naming more is truncated with a warning rather than silently dropped — the
/// cap is a fact about the shipped data, and if the data changes the log says
/// so.
pub const MAX_ARMOR_SUBLAYERS: usize = 2;

/// One worn piece, resolved to what it actually draws (M47).
///
/// Each entry is an atlas key and the tint it is drawn with, already through
/// `EquipmentLayerRenderer.getColorForLayer` — so a layer whose colour came
/// back 0 is **absent from this list**, not present with a zero tint. That is
/// vanilla's `if (color != 0)` guard, resolved before the frame rather than
/// inside the emitter. The tint is vanilla's colour, bytes over 255 (gamma;
/// `gamma_vertex` multiplies it with the shade, M433).
#[derive(Clone, Copy, Debug, Default)]
pub struct ArmorPiece<'a> {
    pub layers: [Option<(&'a str, [f32; 3])>; MAX_ARMOR_SUBLAYERS],
    /// The trim sprite's atlas origin, already permuted and uploaded (M48).
    ///
    /// An origin rather than a key because a trim sprite has no fixed home: it
    /// is generated on first sighting into the demand-filled trim pool, so
    /// where it lives is decided at runtime.
    pub trim: Option<(u32, u32)>,
    /// `ItemStack.hasFoil()` for the stack this piece came from (M50).
    ///
    /// One flag for the whole piece, not one per layer: `renderLayers` clears
    /// `renderFoil` inside the loop, so the foil is submitted **once**, riding
    /// the first layer that draws — and never for the trim, which is submitted
    /// after the loop.
    pub foil: bool,
}

/// One entity to draw this frame — position already frame-interpolated.
pub type EntityLight = [f32; 3];

/// M277 — `OverlayTexture.pack(u(whiteOverlayProgress), v(hurt))` as the
/// entity shader reads it from `light_hurt.a`: 1 for the red row (it wins:
/// every texel of the red rows is red, whatever the column), `2 + u/15` for
/// a white column `u = (int)(progress · 15)` above 0, and 0 for none.
pub fn overlay_value(hurt: bool, white: f32) -> f32 {
    if hurt {
        return 1.0;
    }
    let u = (white * 15.0) as i32;
    if u > 0 {
        2.0 + u.min(15) as f32 / 15.0
    } else {
        0.0
    }
}

/// M281 — a burning entity's flames: their light (the entity's with block
/// light 15, `LightCoordsUtil.withBlock(lightCoords, 15)`) and the frame each
/// of `fire_0` and `fire_1` shows this tick (`rewo_data`'s `fire_frame`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FireDraw {
    pub light: [f32; 3],
    pub frames: [usize; 2],
}

/// M281 — the baked model of one flame frame; must name what
/// `rewo_data::block_entity_models::fire_frame_model` bakes (pinned by a
/// test in `rewo-app`).
pub fn fire_model_name(sprite: usize, frame: usize) -> String {
    format!("rewo:be/fire_{sprite}/{frame}")
}

/// `CustomHeadLayer.SKULL_SCALE`.
pub const WORN_SKULL_SCALE: f32 = 1.1875;
/// `CustomHeadLayer.ITEM_SCALE` (M274).
pub const WORN_ITEM_SCALE: f32 = 0.625;

/// M273 — what is worn on the head and the renderer's
/// `CustomHeadLayer.Transforms`: a skull's block-entity model (a player head
/// may be a per-skin copy, M272), or (M274, `item`) any other item's model,
/// drawn through its `head` display context.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WornHead<'a> {
    pub model: &'a str,
    /// M274 — `CustomHeadLayer`'s item branch rather than its skull one.
    pub item: bool,
    /// `Transforms.yOffset`, in blocks (the villager's −0.1171875); the item
    /// branch's.
    pub y_offset: f32,
    /// `Transforms.skullYOffset`, in blocks (the villager's −0.0742).
    pub skull_y_offset: f32,
    /// `Transforms.horizontalScale` (the piglin's 1.0019531).
    pub horizontal_scale: f32,
    /// M420 — the item branch's stack `hasFoil()`: `headItem.submit` draws
    /// its glint like any item's (the skull branch submits none).
    pub foil: bool,
}

#[derive(Clone, Copy)]
pub struct EntityDraw<'a> {
    /// Feet-center world position.
    pub pos: [f32; 3],
    pub width: f32,
    pub height: f32,
    /// Linear-space base color (capsules; the player model is textured).
    pub color: [f32; 3],
    /// Nametag text (players); `None` draws no tag.
    pub name: Option<&'a str>,
    /// M410 — `EntityRenderState.scoreText` (`belowNameDisplay()`: a
    /// below-name score, a mannequin's description), drawn at the tag's
    /// place with the name lifted a line above it.
    pub score_text: Option<&'a str>,
    /// Health and max health, when a floating health bar should be drawn
    /// (M59). `None` draws no bar — and `None` is load-bearing, not a
    /// placeholder: spec rules 4 and 5 say a bar is suppressed outright for a
    /// non-living entity, an invisible one, one beyond the name-tag distance,
    /// and — above all — one whose max health the server has never synced.
    pub health: Option<HealthBar>,
    /// Which model to draw (falls back to the capsule when the model's
    /// texture wasn't baked).
    pub kind: EntityModelKind,
    /// Body yaw (degrees, MC convention) — rotates the whole model.
    pub yaw: f32,
    /// `LivingEntityRenderer`'s `state.deathTime` — the partial-tick-interpolated
    /// death clock, `0` for a living entity (M24). Drives the topple in
    /// [`death_flip_degrees`], applied between the model transform and the body
    /// yaw exactly where `setupRotations` runs it.
    pub death_time: f32,
    /// Head yaw (degrees, absolute). The head part rotates about its own
    /// pivot by the net `head_yaw − yaw` (vanilla `netHeadYaw`); equal to
    /// `yaw` leaves the head aligned with the body.
    pub head_yaw: f32,
    /// Look pitch (degrees) — tilts the head about the neck.
    pub pitch: f32,
    /// Walk-cycle phase + amplitude (vanilla limbSwing / limbSwingAmount)
    /// — arms and legs swing about their pivots. 0 amount = neutral pose.
    pub limb_swing: f32,
    pub limb_amount: f32,
    /// Active pose/state gesture + seconds since it started (drives the
    /// one-shot keyframe rigs — warden roar, sniffer dig, …).
    pub gesture: Option<(mobs::Gesture, f32)>,
    /// Per wire-event elapsed seconds since receipt (`None` = not fired), one
    /// slot per [`mobs::ModelEvent`] — warden attack / sonic boom. The event
    /// rig plays from this age and holds its neutral last frame afterward.
    pub events: [Option<f32>; mobs::ModelEvent::COUNT],
    /// Armadillo shell swap (vanilla `isHidingInShell`).
    pub shell: bool,
    /// M296 — `AbstractChestedHorse.hasChest()`: a donkey's or mule's
    /// chests show.
    pub chested: bool,
    /// M238 — the armour stand's arms and base plate
    /// (`mobs::StandParts`); inert on every other kind.
    pub stand_parts: mobs::StandParts,
    /// M271 — `LivingEntityRenderer`'s `isBodyVisible` is false and the
    /// viewer cannot see the invisible (not a spectator): the model (or its
    /// capsule) is not drawn, while its layers (armour, held items) and its
    /// name still are. An invisible armour stand with a name is the classic
    /// server hologram.
    pub body_hidden: bool,
    /// M273 — a skull worn on the head (`CustomHeadLayer`'s skull branch),
    /// `None` for no skull or a model without the layer.
    pub worn_head: Option<WornHead<'a>>,
    /// M277 — `getWhiteOverlayProgress` (a swelling creeper's flash), 0 for
    /// none; it packs with [`Self::hurt`] through [`overlay_value`].
    pub white_overlay: f32,
    /// M277 — a renderer's own `scale(x, y, x)` in entity space after the
    /// model flip (the creeper's swell: wider by `s`, taller by `hs`),
    /// `[horizontal, vertical]`; `[1, 1]` for every other entity.
    pub stretch: [f32; 2],
    /// M278 — the glowing outline's colour (`Entity.getTeamColor()`, gamma
    /// rgb in `0..1`), `None` for an entity that does not glow.
    pub outline: Option<[f32; 3]>,
    /// M281 — `displayFireAnimation` (burning, not fire-immune, not a
    /// spectator); `None` for none.
    pub on_fire: Option<FireDraw>,
    /// M294 — the body slot's equipment layers (`HORSE_BODY`) over the
    /// horse family's armour shell; `None` for none or another kind.
    pub body_armor: Option<ArmorPiece<'a>>,
    /// M295 — the saddle slot's equipment layers over the horse family's
    /// saddle mesh; `None` for none or another kind.
    pub saddle: Option<ArmorPiece<'a>>,
    /// M295 — `EquineRenderState.isRidden` (`isVehicle()`): the saddle's
    /// reins show.
    pub ridden: bool,
    /// M285 — `CreeperRenderState.isPowered`: a charged creeper draws
    /// `CreeperPowerLayer`'s energy swirl. Ignored for other kinds.
    pub powered: bool,
    /// M310 — the `PlayerModelPart` bits an avatar does NOT show (the
    /// customisation mask's complement): its outer layers hide by them
    /// ([`mobs::Show::AvatarPart`]). 0 shows every layer.
    pub avatar_hidden: u8,
    /// M311 — `WolfArmorLayer.maybeRenderCracks`: the crack sheet's key
    /// (`wolf_armor_crackiness_{low,medium,high}`) drawn `armorTranslucent`
    /// over the body armour's shell; `None` when undamaged or not a wolf.
    pub body_armor_crack: Option<&'a str>,
    /// M312 — `HappyGhastRenderState.isLeashHolder`: something is leashed
    /// to this entity (the happy ghast's ropes show while it also wears a
    /// harness).
    pub leash_holder: bool,
    /// M240 — draw [`Self::ground_item`] as `ThrownItemRenderer` does (a
    /// snowball, a pearl, a potion, a fireball, a rocket, the bobber):
    /// scaled and turned to face the camera instead of bobbing and spinning.
    pub billboard: Option<Billboard>,
    /// Allay dance inputs (`Some` only for a dancing Allay); `None` for every
    /// other entity and a non-dancing Allay (ordinary head-look / upright pose).
    pub allay_dance: Option<mobs::AllayDance>,
    /// Combat-swing pose (`ClientboundAnimatePacket`) — the
    /// `ArmedEntityRenderState` fields. [`mobs::SwingPose::NONE`] for an entity
    /// that isn't mid-swing. Carried for every entity (it also feeds CEM's
    /// `swing_progress`); only models built from `HumanoidModel.createMesh`
    /// have parts that pose from it.
    pub attack: mobs::SwingPose,
    /// Both arms' `HumanoidModel.ArmPose` — the *hold* baseline
    /// `pose{Right,Left}Arm` writes before `setupAttackAnimation` adds the
    /// strike. [`mobs::ArmPoses::EMPTY`] for an unarmed entity and for every
    /// model that is not a humanoid.
    pub arm_poses: mobs::ArmPoses,
    /// Synced mob state the M20 arm rigs read (aggressive, held item, baby,
    /// derived illager pose). Default for every entity that is not a mob.
    pub mob: mobs::MobCombat,
    /// `LivingEntityRenderer`'s `hasRedOverlay` — `hurtTime > 0` (M21). Drives
    /// the red damage flash in the entity shader.
    pub hurt: bool,
    /// Held items **by arm** — `[right, left]` (M22), as registry names.
    /// `ArmedEntityRenderState` carries the stacks per arm, and the hand→arm
    /// mapping is `getMainArm()`, which the app resolves. `None` = empty hand
    /// or an item whose model this client suppresses.
    pub held: [Option<&'a str>; 2],
    /// A **dropped** stack's item name — `ItemEntity` (M24b). `Some` replaces
    /// the model/capsule entirely: `ItemEntityRenderer` draws the item and
    /// nothing else.
    pub ground_item: Option<&'a str>,
    /// `ItemStack.hasFoil()` for each held stack, and for a dropped one
    /// (M45). Resolved by the app, which is the only side that knows what a
    /// stack is; the pass draws quads.
    /// The armour atlas key for each slot, head first (M46) —
    /// `<asset>/<layer>` as `rewo_data::equipment` packs it. `None` is an
    /// empty slot, or one whose asset this jar does not describe.
    pub armor: [Option<ArmorPiece<'a>>; 4],
    pub held_glint: [bool; 2],
    pub ground_glint: bool,
    /// The dropped stack's raw count. The renderer applies vanilla's
    /// `getRenderedAmount` bucketing (1/2/3/4/5), so this is the count as
    /// sent, not the copy count.
    pub ground_count: i32,
    /// `ItemEntity.bobOffs` — `random.nextFloat() * 2 * PI`.
    ///
    /// **Client-side by construction**: vanilla rolls it in the entity's
    /// constructor and never sends it, so there is no server value to match.
    /// The app derives it from the entity id, which is *a* valid roll and is
    /// at least stable across frames.
    pub bob_offset: f32,
    /// `ItemClusterRenderState.getSeedForItemStack` —
    /// `Item.getId(item) + stack.getDamageValue()`, the seed the per-copy
    /// jitter LCG is reset to. Rewo decodes no damage for a dropped stack, so
    /// the app passes the item protocol id.
    pub ground_seed: i32,
    /// Player skin: a normalized UV offset that relocates the (default-Steve)
    /// player-model quads onto this player's uploaded skin slot. `None` →
    /// the default skin.
    ///
    /// **Ignored for non-player models, and `emit_model` is what ignores it.**
    /// This sentence used to be here on its own while the pass added the offset
    /// unconditionally, so the invariant lived in one caller's `if is_player`
    /// (`live_cmd::collect_entities`). `mobtexshot`'s `m3`/`m4` grade both
    /// directions — inert on a zombie, still live on the player model — so a
    /// future caller cannot re-open it by accident.
    pub skin_uv: Option<[f32; 2]>,
    /// Uniform model-scale multiplier on top of the baked scale — vanilla's
    /// per-entity render scale (slime/magma-cube `size`). 1.0 = as baked.
    pub scale_mul: f32,
    /// An affine applied to the entity's **feet-relative** position, in block
    /// units, before [`Self::pos`] places it (M31).
    ///
    /// `None` for every ordinary entity, which *stands* in the world. A
    /// spawner's caged mob does not: `submitEntityInSpawner` pushes a
    /// translate-rotate-tilt-scale chain and renders the entity at the
    /// resulting origin, so the mob is **mounted inside a block** rather than
    /// positioned in the world. Expressing that as one matrix here keeps the
    /// model, its rig and its animations on exactly the path every other mob
    /// already uses; the alternative was a second emitter that would have had
    /// to duplicate all of it.
    pub mount: Option<[[f32; 4]; 3]>,
    /// Per-entity stable id for CEM `random(id)` (so a herd doesn't animate
    /// in lockstep). Any stable-per-entity float; 0 is fine for stills.
    pub anim_id: f32,
    /// Per-channel world-light color in `0..1`, sampled by the caller at the
    /// entity's **eye** with the same lightmap formula used for blocks. RGB is
    /// retained so warm block light and cool sky light tint the model exactly;
    /// `[1.0, 1.0, 1.0]` is fullbright for serverless still renders.
    pub light: EntityLight,
    /// Inputs to this entity's vanilla emissive layers (warden tendrils,
    /// creaking eyes). Defaults are vanilla's synched defaults.
    pub emissive: EmissiveState,
    /// Resource-pack texture variant (ETF / M57): 0 is the vanilla texture,
    /// any other id is one this mob's pack rules chose — see
    /// `rewo_data::etf::EtfPack::pick`. An id the atlas has no slot for falls
    /// back to the vanilla texture.
    pub variant: u16,
    /// Dye colour `0..15` for the mob's tinted texture, if it has one
    /// (`mobs::tinted_texture`) — the sheep's wool.
    ///
    /// `None` means the mob's vanilla *default* dye, not "no tint":
    /// `SheepRenderState.woolColor` starts at `DyeColor.WHITE`, and
    /// `SheepWoolLayer` tints by it unconditionally — so even a plain white
    /// sheep has its wool multiplied by 0xE6E6E6 rather than left at full
    /// brightness.
    pub dye: Option<u8>,
    /// `Sheep.isSheared()` — bit 0x10 of the wool byte (M64).
    ///
    /// Drops the quads of `mobs::shearable_texture(kind)`, because
    /// `SheepWoolLayer.submit` returns before submitting the fur model at all.
    /// Removing the *geometry* is the point: the fleece is inflated 0.6/1.75/
    /// 0.5 over the body, so a shorn sheep is visibly thinner, not just
    /// differently coloured. Inert on a mob with no shearable layer.
    pub sheared: bool,
    /// Whether this mob's **coplanar** layer draws at all — vanilla
    /// `SheepWoolUndercoatLayer.submit`'s gate (M68).
    ///
    /// The gate is `(state.isJebSheep || state.woolColor != DyeColor.WHITE)
    /// && !state.isBaby`, and it is resolved upstream where the metadata
    /// lives, exactly as `EntityDraw::cape`'s four gates are. Note what it
    /// does *not* contain: any test of `isSheared`. Shearing takes the fleece
    /// and leaves this, which is the whole observable point of the layer —
    /// a shorn dyed sheep keeps its colour, a shorn white one does not.
    ///
    /// `false` on every mob without such a layer, where it is inert.
    pub undercoat: bool,
    /// M342 — a tame wolf's collar colour, a `DyeColor` id; `None` draws no
    /// collar (vanilla's `collarColor` is null unless the wolf is tame).
    pub collar: Option<u8>,
    /// A tropical fish's two dyes as `DyeColor` ids, `[body, pattern]` (M68) —
    /// `TropicalFishRenderState`'s `baseColor` and `patternColor`, which the
    /// renderer feeds to `getModelTint` and to `TropicalFishPatternLayer`
    /// respectively. `None` for every other mob **and** for a fish whose
    /// variant has not arrived, which vanilla renders as `DEFAULT_VARIANT`'s
    /// WHITE/WHITE — so `None` means that, not "untinted".
    pub fish_dye: Option<[u8; 2]>,
    /// The worn cape (M60), or `None` when any of `CapeLayer`'s four gates
    /// suppresses it. The gates are resolved upstream, where the equipment
    /// and metadata live; by the time a draw is built the answer is already
    /// yes-or-no.
    pub cape: Option<CapeDraw>,
    /// M403 — `WingsLayer`: the elytra on a chest slot whose item has a
    /// `wings` equipment layer, resolved upstream like the armour.
    pub wings: Option<WingsDraw<'a>>,
    /// `ageInTicks` for a dropped stack's bob and spin, overriding the pass's
    /// shared clock (M81).
    ///
    /// `None` for a live item entity. `Some` for a **pickup animation**, whose
    /// vanilla implementation carries an `EntityRenderState` snapshot taken at
    /// the moment of collection — so the stack it draws is frozen mid-spin for
    /// the three ticks of its flight, not still turning. The app reconstructs
    /// the capture time from the animation's own life counter rather than
    /// storing a second clock.
    pub ground_age: Option<f32>,
}

/// M403 — which sheet `WingsLayer` draws the elytra with.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum WingsTexture<'a> {
    /// The equipment layer's own sheet, by its atlas key (`wings/elytra`).
    Equipment(&'a str),
    /// `getPlayerElytraTexture`: a caped player's own cape sheet (the
    /// 64 × 32 cape slot at this atlas origin), for a `use_player_texture`
    /// layer.
    Player((u32, u32)),
}

/// M403 — one entity's elytra (`WingsLayer` + `ElytraModel`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WingsDraw<'a> {
    pub texture: WingsTexture<'a>,
    /// `ItemStack.hasFoil()` of the chest stack.
    pub foil: bool,
    /// `elytraRotX/Y/Z` (radians), the left wing's: the right mirrors `y`
    /// and `z`.
    pub rot: [f32; 3],
    /// `isCrouching`: both wings drop 3 px (`ElytraModel.setupAnim`).
    pub crouching: bool,
}

/// One player's cape, resolved for one frame (M60).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CapeDraw {
    /// Atlas origin, in texels, of this player's uploaded cape slot.
    pub origin: (u32, u32),
    /// `AvatarRenderState.capeFlap`, degrees.
    pub flap: f32,
    /// `AvatarRenderState.capeLean`, degrees.
    pub lean: f32,
    /// `AvatarRenderState.capeLean2`, degrees.
    pub lean2: f32,
    /// `hasLayer(chestEquipment, HUMANOID)` — whether the chest item has a
    /// humanoid armour layer, which pushes the cape clear of it. This is
    /// **not** "the chest slot is occupied": an elytra suppresses the cape
    /// entirely and a carved pumpkin shifts it not at all.
    pub chest_humanoid: bool,
    /// The simulated spine (M61), or `None` for the vanilla rigid slab —
    /// which is the default, and which every one of M60's witnesses grades.
    pub wavy: Option<CapeJoints>,
}

/// Joints the wavy cape's geometry hangs from, in **cape space**: world-axis
/// aligned, model units, origin on the entity (M61).
///
/// A fixed array rather than a borrow, so [`CapeDraw`] stays `Copy` and
/// lifetime-free and no caller has to find somewhere to keep a slice alive
/// for the length of a draw list.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CapeJoints {
    /// Joints in use, `2..=CAPE_MAX_JOINTS`.
    pub n: u8,
    pub p: [[f32; 3]; CAPE_MAX_JOINTS],
}

/// `rewo_world::wavy_cape::SEGMENTS + 1`. Not imported: rewo-gpu depends on
/// no other rewo crate and is kept that way. `capeshot` asserts the two
/// agree.
pub const CAPE_MAX_JOINTS: usize = 17;

impl CapeJoints {
    pub fn from_slice(p: &[[f32; 3]]) -> Option<Self> {
        if p.len() < 2 || p.len() > CAPE_MAX_JOINTS {
            return None;
        }
        let mut out = [[0.0f32; 3]; CAPE_MAX_JOINTS];
        out[..p.len()].copy_from_slice(p);
        Some(Self {
            n: p.len() as u8,
            p: out,
        })
    }

    pub fn joints(&self) -> &[[f32; 3]] {
        &self.p[..self.n as usize]
    }

    pub fn segments(&self) -> usize {
        self.n as usize - 1
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct Vertex {
    pos: [f32; 3],
    uv: [f32; 2],
    /// Base tint × directional face shade, linear (`entity.frag` re-encodes
    /// it; M433: a shade is folded in by [`gamma_vertex`], in vanilla's gamma
    /// space). **The world light is deliberately
    /// NOT folded in here** (M21): vanilla's `entity.fsh` applies the hurt
    /// overlay to `texture * vertexColor` and only *then* multiplies by the
    /// lightmap, so the two have to reach the shader separately — otherwise a
    /// hurt mob in a dark cave would flash at full brightness.
    color: [f32; 4],
    /// `rgb` = the entity's per-channel world light, `a` = the hurt flag
    /// (`hasRedOverlay` as 0.0 / 1.0). One extra attribute rather than two;
    /// entity vertex counts are tiny.
    light_hurt: [f32; 4],
}

/// Combined entity atlas: the font occupies (0,0)..(128,128); mob textures
/// (16² tadpole up to 192² sniffer-class skins) shelf-pack around it. One
/// texture, one pipeline family.
const ATLAS_W: u32 = 1024;
/// M22 grew this by the held-item band, M48 by the trim band, M60 by the cape
/// band — each time at the *bottom*, so the mob shelf region above them was
/// unchanged and only the V denominator moved.
///
/// **M64 is the first growth that is not that**, and it is worth saying why
/// the recipe stopped applying. What ran out this time was the shelf region
/// itself: 42 vanilla mob-variant sheets did not fit under `ITEM_POOL_Y`, and
/// that ceiling is defined by subtraction from `ATLAS_H` — so raising it by
/// 128 rows necessarily slides the item, skin, trim and cape pools 128 rows
/// down with it. Nothing on disk or in a golden image depends on those
/// origins (every consumer computes them from these constants, and the atlas
/// is rebuilt at startup), and the *mob* shelf packing is still byte-for-byte
/// what it was because the packer is sequential and the region only grew at
/// its far end — which is what keeps `mobshot --check` at 243/243.
///
/// M194 grew it by the framed-map band (128 rows at the bottom, M48's recipe
/// again), so every earlier address is unchanged.
///
/// M223 is M64's case again: the eight boat and eight chest-boat wood sheets
/// (128 x 64 and 128 x 128) did not fit the shelf region, so 384 rows went in (256
/// were not enough with the shelf packer's gaps)
/// and the pools below it slid down with `ATLAS_H`; the mob packing ahead of
/// the new sheets is unchanged.
///
/// M246 grew it by the large-item band (256 rows at the very bottom, under
/// the map band, which is now anchored to it): M48's recipe, so every
/// earlier address is unchanged.
///
/// M297 is M64's and M223's case once more: the eighteen 128 x 64 llama
/// decor sheets (with M294-M295's twelve 64 x 64 horse ones) filled the
/// shelf region, and three mob variants (two chest boats, the wither's
/// invulnerable sheet) stopped packing, so it grew by 256 rows and the pools
/// below slid down with `ATLAS_H`.
///
/// M299 grew it 256 rows more: every sheet now carries a one-texel extruded
/// gutter (`blit_tex_extruded`), and the padding filled the shelf region
/// until a resource pack's alternates (mobshot's ETF fixture) stopped
/// packing.
///
/// M302 grew it 256 rows more for the happy ghast's sixteen 64 x 64 harness
/// colours (66 x 66 with their gutters, two shelves), which pushed eight
/// chest-boat woods out of the shelf region.
///
/// M307 grew it 256 rows more for the sulfur cube's four sheets (two
/// 128 x 128, two 64 x 64), after which three of the ETF fixture's
/// alternates stopped packing.
///
/// M348 grew it 256 rows more: the cold and warm cows' sheets and calves
/// filled the shelf region until the ETF fixture's emissive pig overlay
/// stopped packing (`mobshot --etf-check` f6, f7).
const ATLAS_H: u32 = 3648;

/// Dynamic player-skin pool: 32 slots of 64×64 in the atlas's bottom two
/// rows, filled at runtime as players' skins arrive. The mob packer is capped
/// above the dynamic bands so it never collides.
///
/// The most textures any one `mobs::MobDef` lists — the sheep's base + fleece
/// and undercoat (M68). Only sizes the per-slot tint scratch in `emit_model`; a
/// mob with more slots than this simply leaves the extra ones untinted, which
/// is why the bound is asserted in a test rather than enforced at runtime.
const MAX_MOB_TEXTURES: usize = 4;

/// The variant id a pack's emissive overlay arrives under — mirrors
/// `rewo_data::etf::EMISSIVE_INDEX`. Kept as a plain constant so this crate
/// stays free of a rewo-data dependency (the same reason `FontData` and the
/// texture slices are borrowed views).
pub const EMISSIVE_VARIANT: u32 = 1 << 16;

/// A round-robin atlas-slot ring that says **whose slot it just took**.
///
/// The trim and item pools hand out `next % cap` and increment. (The skin,
/// cape and face pools, whose addresses the app stores, are [`LruSlots`].)
/// Recycling is fine for a pool nothing remembers. It is a real
/// aliasing bug for one with a **key → slot cache**, which `upload_trim` and
/// `prepare_held_items` both have: after `cap` distinct uploads, two keys
/// resolve to one slot, the older key's entry is still in the map, and every
/// draw that resolves through it samples the newer sprite's pixels. That is
/// "renders with a texture that is not its own", one atlas band over from the
/// mob sheets — reachable at 65 distinct armour trims or 1,025 distinct held
/// item textures in one session, with nothing anywhere logging it.
///
/// The ring is a plain struct with no GPU in it precisely so the wrap can be
/// tested: `upload_trim` needs a device, `claim` does not.
///
/// **That covers `claim`, not the fix.** The bug is a *caller* keeping a stale
/// key, so unit tests on this type say nothing about whether either call site
/// drops its evictee — both `remove(&old)` blocks below could be deleted with
/// the whole suite and every gate green, which is exactly how the pre-M165
/// code passed. The call sites are graded at the pool by `mobtexshot`'s `m10`
/// (trim) and `m11` (items), through [`EntityPass::trim_slot_pairs`] /
/// [`EntityPass::item_slot_pairs`] and, for trim, through the public
/// `upload_trim` itself: each over-fills its pool by one and requires that the
/// evicted key no longer resolves to the slot that took its place.
pub(crate) struct SlotRing<K> {
    next: u32,
    cap: u32,
    /// Which key each slot currently holds — the bookkeeping the bare cursor
    /// did not have, and the only thing that can name an evictee.
    owner: Vec<Option<K>>,
}

impl<K: PartialEq> SlotRing<K> {
    pub(crate) fn new(cap: u32) -> Self {
        let mut owner = Vec::new();
        owner.resize_with(cap as usize, || None);
        Self {
            next: 0,
            cap,
            owner,
        }
    }

    /// Claim the next slot for `key`. Returns `(slot, evicted)`, where
    /// `evicted` is the key that slot held until now — the caller **must**
    /// drop its cache entry, or that key keeps addressing this upload.
    pub(crate) fn claim(&mut self, key: K) -> (u32, Option<K>) {
        let slot = self.next % self.cap;
        self.next = self.next.wrapping_add(1);
        let evicted = self.owner[slot as usize].take();
        self.owner[slot as usize] = Some(key);
        (slot, evicted)
    }
}

/// A per-player atlas pool with **least-recently-used** eviction: the skin,
/// cape and tab-face pools.
///
/// These pools hand the slot's address *out* (a skin UV delta, a cape origin,
/// a face slot) and the app stores it per player, so a recycled slot is not a
/// stale cache entry here but another player's texture under a remembered
/// address. The old round-robin cursor overwrote slot 0 at the 33rd distinct
/// player even while player #1 was on screen. This pool instead:
///
/// * reuses the slot the same owner already holds (a changed skin overwrites
///   its own slot);
/// * otherwise takes a free slot, lowest first (so a fresh pool fills in the
///   order the round-robin did);
/// * otherwise evicts the slot drawn least recently, **never one drawn in the
///   current frame** ([`Self::touch`] since the last [`Self::begin_frame`]),
///   and names the evicted owner so the caller can drop its address.
///
/// When every slot was drawn this frame, `claim` refuses: the newcomer keeps
/// the default texture rather than taking a visible player's.
pub(crate) struct LruSlots<K> {
    owner: Vec<Option<K>>,
    last_used: Vec<u64>,
    /// Bumped once per drawn frame; a slot whose `last_used` equals it was
    /// drawn (or claimed) in the current frame.
    frame: u64,
}

impl<K: Copy + PartialEq> LruSlots<K> {
    pub(crate) fn new(cap: u32) -> Self {
        Self {
            owner: vec![None; cap as usize],
            last_used: vec![0; cap as usize],
            frame: 1,
        }
    }

    /// Start a new frame: slots drawn in the previous one become evictable
    /// unless touched again.
    pub(crate) fn begin_frame(&mut self) {
        self.frame += 1;
    }

    /// Record that `slot` is drawn this frame. Out-of-range slots (an address
    /// that did not come from this pool) are ignored.
    pub(crate) fn touch(&mut self, slot: u32) {
        if let Some(t) = self.last_used.get_mut(slot as usize) {
            *t = self.frame;
        }
    }

    /// A slot for `key`: `Some((slot, evicted))`, where `evicted` is the other
    /// owner that slot held until now (the caller must forget that owner's
    /// address), or `None` when every slot is in use this frame.
    pub(crate) fn claim(&mut self, key: K) -> Option<(u32, Option<K>)> {
        let slot = match self.owner.iter().position(|o| *o == Some(key)) {
            Some(i) => i,
            None => match self.owner.iter().position(Option::is_none) {
                Some(i) => i,
                None => (0..self.owner.len())
                    .filter(|&i| self.last_used[i] < self.frame)
                    .min_by_key(|&i| (self.last_used[i], i))?,
            },
        };
        let evicted = self.owner[slot].filter(|o| *o != key);
        self.owner[slot] = Some(key);
        self.last_used[slot] = self.frame;
        Some((slot as u32, evicted))
    }
}

const SKIN_SLOT: u32 = 64;
const SKIN_POOL_COLS: u32 = ATLAS_W / SKIN_SLOT; // 16
const SKIN_POOL_ROWS: u32 = 2;
const SKIN_SLOTS: u32 = SKIN_POOL_COLS * SKIN_POOL_ROWS; // 32
const SKIN_POOL_Y: u32 = TRIM_POOL_Y - SKIN_POOL_ROWS * SKIN_SLOT; // 2944

// -- the trim pool (M48) ------------------------------------------------------
//
// Demand-filled, like the skin and item pools before it, and for the same
// reason: 18 patterns x 17 palettes x 2 layer types is 612 sheets, which no
// band of this atlas can hold. Only the combinations actually worn are ever
// generated.
//
// Placed at the **top** of the atlas, with the skin and item pools defined
// downward from it, so growing `ATLAS_H` moved nothing that was already there
// — every mob, item and skin address is what it was before M48.
const TRIM_SLOT_W: u32 = 64;
const TRIM_SLOT_H: u32 = 32;
const TRIM_POOL_COLS: u32 = ATLAS_W / TRIM_SLOT_W; // 16
const TRIM_POOL_ROWS: u32 = 4;
const TRIM_SLOTS: u32 = TRIM_POOL_COLS * TRIM_POOL_ROWS; // 64
const TRIM_POOL_Y: u32 = CAPE_POOL_Y - TRIM_POOL_ROWS * TRIM_SLOT_H; // 3072

fn trim_slot_origin(slot: u32) -> (u32, u32) {
    (
        (slot % TRIM_POOL_COLS) * TRIM_SLOT_W,
        TRIM_POOL_Y + (slot / TRIM_POOL_COLS) * TRIM_SLOT_H,
    )
}

// -- the cape pool (M60) ------------------------------------------------------
//
// Demand-filled like the pools before it. A cape has no reference sheet in the
// jar at all — it is fetched per player from the profile's texture URL — so it
// cannot ride the static shelf packer the way an armour sheet does.
//
// The slot is **64x32**, the cape model's own UV space: `createCapeLayer`
// builds against a 64x64 `LayerDefinition` but the box carries
// `xTexScale 1.0, yTexScale 0.5`, and `CubeDefinition.bake` multiplies those
// in, so the cube's UVs normalize against 64x32. A 64x64 slot would halve
// every V.
//
// Placed at the **bottom** of the atlas with `TRIM_POOL_Y` re-anchored to it,
// which is M48's recipe: growing `ATLAS_H` by exactly the new band's height
// leaves every mob, item, skin and trim address numerically unchanged.
const CAPE_SLOT_W: u32 = 64;
const CAPE_SLOT_H: u32 = 32;
const CAPE_POOL_COLS: u32 = ATLAS_W / CAPE_SLOT_W; // 16
const CAPE_POOL_ROWS: u32 = 2;
const CAPE_SLOTS: u32 = CAPE_POOL_COLS * CAPE_POOL_ROWS; // 32
const CAPE_POOL_Y: u32 = MAP_POOL_Y - CAPE_POOL_ROWS * CAPE_SLOT_H; // 3200

// -- the framed-map band (M194) -----------------------------------------------
//
// A map in an item frame samples its 128 x 128 pixels from this atlas (the
// held map has the hand pass's own). Placed at the **bottom** with
// `CAPE_POOL_Y` re-anchored to it — M48's and M60's recipe — so every mob,
// item, skin, trim and cape address is numerically what it was.
//
// Seven map slots, LRU by map id (a frame on screen keeps its slot), and the
// eighth 128 x 128 cell holds the map decoration sprites, 8 x 8 each by raw
// type id, 16 to a row.
const MAP_SLOT: u32 = 128;
const MAP_POOL_COLS: u32 = ATLAS_W / MAP_SLOT; // 8
/// The last cell is the decoration sprites.
const MAP_SLOTS: u32 = MAP_POOL_COLS - 1; // 7
const MAP_POOL_Y: u32 = BIG_POOL_Y - MAP_SLOT; // 3264

/// M246 — the large held-item pool: 64×64 slots for the textures the 16×16
/// item pool cannot hold. Every block-entity model (a chest's 64×64 sheet, a
/// head's 64×32, a banner's, a shulker box's) and the item-only shield and
/// trident sample one of these. The 16×16-only pool skipped them, so a world
/// chest, a held chest and every other block entity drew no quads at all from
/// M25b until M246 — the witnesses graded draw lists, never pixels.
/// Round-robin like the item pool; a texture larger than a slot (the dragon
/// head's 256×256) still draws nothing.
const BIG_SLOT: u32 = 64;
const BIG_POOL_COLS: u32 = ATLAS_W / BIG_SLOT; // 16
const BIG_POOL_ROWS: u32 = 4;
const BIG_SLOTS: u32 = BIG_POOL_COLS * BIG_POOL_ROWS; // 64
const BIG_POOL_Y: u32 = ATLAS_H - BIG_POOL_ROWS * BIG_SLOT; // 3392

/// The large-item band's geometry, for the gate: `(atlas w, atlas h, slot,
/// slots, band y)`.
pub fn big_pool_geometry() -> (u32, u32, u32, u32, u32) {
    (ATLAS_W, ATLAS_H, BIG_SLOT, BIG_SLOTS, BIG_POOL_Y)
}

/// Atlas origin of large item slot `i` (0..BIG_SLOTS).
fn big_slot_origin(i: u32) -> (u32, u32) {
    (
        (i % BIG_POOL_COLS) * BIG_SLOT,
        BIG_POOL_Y + (i / BIG_POOL_COLS) * BIG_SLOT,
    )
}
const MAP_DECO_ORIGIN: (u32, u32) = (MAP_SLOTS * MAP_SLOT, MAP_POOL_Y);
const MAP_DECO_SPRITE: u32 = 8;
const MAP_DECO_COLS: u32 = MAP_SLOT / MAP_DECO_SPRITE; // 16, so 256 type ids

/// The framed-map band's geometry, for the gate: `(atlas w, atlas h, slot,
/// slots, band y, decoration origin)`.
pub fn map_pool_geometry() -> (u32, u32, u32, u32, u32, (u32, u32)) {
    (
        ATLAS_W,
        ATLAS_H,
        MAP_SLOT,
        MAP_SLOTS,
        MAP_POOL_Y,
        MAP_DECO_ORIGIN,
    )
}

/// Where decoration type `id`'s sprite sits in the band, as `(x, y)`, or
/// `None` past it.
fn map_deco_origin(id: i32) -> Option<(u32, u32)> {
    let id = u32::try_from(id).ok()?;
    (id < MAP_DECO_COLS * MAP_DECO_COLS).then(|| {
        (
            MAP_DECO_ORIGIN.0 + (id % MAP_DECO_COLS) * MAP_DECO_SPRITE,
            MAP_DECO_ORIGIN.1 + (id / MAP_DECO_COLS) * MAP_DECO_SPRITE,
        )
    })
}

/// Atlas origin of dynamic cape slot `i` (0..CAPE_SLOTS).
pub fn cape_slot_origin(i: u32) -> (u32, u32) {
    (
        (i % CAPE_POOL_COLS) * CAPE_SLOT_W,
        CAPE_POOL_Y + (i / CAPE_POOL_COLS) * CAPE_SLOT_H,
    )
}

/// The cape pool's geometry, for the oracle. Returned rather than made `pub`
/// so the constants stay private and there is one place to read them from.
pub fn cape_pool_geometry() -> (u32, u32, u32, u32, u32, u32) {
    (
        ATLAS_W,
        ATLAS_H,
        CAPE_SLOT_W,
        CAPE_SLOT_H,
        CAPE_SLOTS,
        CAPE_POOL_Y,
    )
}

/// Dynamic held-item texture pool (M22): 16×16 slots in the band between the
/// mob shelves and the skin pool. 26.2 ships 1233 distinct held-item textures —
/// far more than an atlas band — but only the handful actually on screen need
/// to be resident, so slots are filled on demand and recycled round-robin,
/// exactly as the skin pool does.
const ITEM_SLOT: u32 = 16;
const ITEM_POOL_COLS: u32 = ATLAS_W / ITEM_SLOT; // 64
const ITEM_POOL_ROWS: u32 = 16;
const ITEM_SLOTS: u32 = ITEM_POOL_COLS * ITEM_POOL_ROWS; // 1024
const ITEM_POOL_Y: u32 = SKIN_POOL_Y - ITEM_POOL_ROWS * ITEM_SLOT; // 2688

/// Atlas origin of dynamic skin slot `i` (0..SKIN_SLOTS).
fn skin_slot_origin(i: u32) -> (u32, u32) {
    (
        (i % SKIN_POOL_COLS) * SKIN_SLOT,
        SKIN_POOL_Y + (i / SKIN_POOL_COLS) * SKIN_SLOT,
    )
}

/// The skin slot whose origin is (x, y), the inverse of [`skin_slot_origin`];
/// `None` for any other texel.
fn skin_slot_at(x: u32, y: u32) -> Option<u32> {
    let row = y.checked_sub(SKIN_POOL_Y)?;
    if !x.is_multiple_of(SKIN_SLOT) || !row.is_multiple_of(SKIN_SLOT) {
        return None;
    }
    let (col, row) = (x / SKIN_SLOT, row / SKIN_SLOT);
    (col < SKIN_POOL_COLS && row < SKIN_POOL_ROWS).then_some(row * SKIN_POOL_COLS + col)
}

/// The `EntityDraw::skin_uv` delta for skin slot `slot`: its origin relative
/// to the baked player texture at `player_origin`, in UV units.
fn skin_uv_of_slot(player_origin: (u32, u32), slot: u32) -> [f32; 2] {
    let (sx, sy) = skin_slot_origin(slot);
    let (px, py) = player_origin;
    [
        (sx as f32 - px as f32) / ATLAS_W as f32,
        (sy as f32 - py as f32) / ATLAS_H as f32,
    ]
}

/// The skin slot a `skin_uv` delta addresses, the inverse of
/// [`skin_uv_of_slot`] (rounded to the texel: the delta went through `f32`
/// division by a non-power-of-two height); `None` for a delta this pool did
/// not hand out.
fn skin_slot_of_uv(player_origin: (u32, u32), uv: [f32; 2]) -> Option<u32> {
    let (px, py) = player_origin;
    let x = (px as f32 + uv[0] * ATLAS_W as f32).round();
    let y = (py as f32 + uv[1] * ATLAS_H as f32).round();
    if !(x >= 0.0 && y >= 0.0) {
        return None;
    }
    skin_slot_at(x as u32, y as u32)
}

/// The cape slot whose origin is (x, y), the inverse of [`cape_slot_origin`].
fn cape_slot_at(x: u32, y: u32) -> Option<u32> {
    let row = y.checked_sub(CAPE_POOL_Y)?;
    if !x.is_multiple_of(CAPE_SLOT_W) || !row.is_multiple_of(CAPE_SLOT_H) {
        return None;
    }
    let (col, row) = (x / CAPE_SLOT_W, row / CAPE_SLOT_H);
    (col < CAPE_POOL_COLS && row < CAPE_POOL_ROWS).then_some(row * CAPE_POOL_COLS + col)
}

/// Atlas origin of dynamic item slot `i` (0..ITEM_SLOTS).
/// Where an item's glint quads go while its own quads are being built (M45).
///
/// Threaded into the two item emitters rather than recomputed afterwards. The
/// glint pipeline depth-tests `EQUAL`, so its vertices must land on **exactly**
/// the positions the item pass wrote — and the only way to be sure of that is
/// to emit them from the same expression, at the same moment, rather than from
/// a second derivation that agrees today. M44 records the same rule for the
/// hand; here the pose involves a death topple, a bob, a spin and a per-copy
/// jitter, so a parallel derivation would be far easier to get subtly wrong.
/// One glint **sheet** and its descriptor. The pipeline is not here: vanilla
/// has a single `RenderPipelines.GLINT` that all four glint render types share
/// and vary only by texture and texture-transform, so Rewo builds one too
/// ([`EntityPass::glint_pipeline`]) and hangs both sheets off it. Two identical
/// pipelines would also be two things to destroy, and M48's leak was exactly
/// one pipeline that outlived its `destroy`.
struct EntityGlint {
    sampler: vk::Sampler,
    image: vk::Image,
    image_alloc: Option<gpu_allocator::vulkan::Allocation>,
    view: vk::ImageView,
    pool: vk::DescriptorPool,
    set: vk::DescriptorSet,
}

pub(crate) struct GlintSink<'a> {
    pub verts: &'a mut Vec<Vertex>,
    /// `ENTITY_GLINT_TEXTURING`'s scrolling offsets for this frame.
    pub offsets: (f32, f32),
    /// The `TextureTransform`'s scale. `GLINT_SCALE_ENTITY` (0.5) for an item
    /// this entity holds or has dropped; `GLINT_SCALE_ARMOR` (0.16) for a worn
    /// piece — the two glints share this sink and differ in the scale and the
    /// sheet, which is the whole of `ARMOR_ENTITY_GLINT` against `ENTITY_GLINT`.
    pub scale: f32,
}

impl GlintSink<'_> {
    /// Push one triangle's worth, taking the position the item just used and
    /// substituting the quad's own `0..1` UV through the glint matrix.
    fn push(&mut self, pos: [f32; 3], uv: [f32; 2]) {
        self.verts.push(Vertex {
            pos,
            uv: crate::gui_item::glint_uv(uv, self.offsets, self.scale),
            // `GlintAlpha` rides in the alpha slot; the glint shader has no
            // lightmap term, so the light channels are unused here.
            color: [1.0, 1.0, 1.0, crate::gui_item::GLINT_STRENGTH],
            light_hurt: [1.0, 1.0, 1.0, 0.0],
        });
    }
}

fn item_slot_origin(i: u32) -> (u32, u32) {
    (
        (i % ITEM_POOL_COLS) * ITEM_SLOT,
        ITEM_POOL_Y + (i / ITEM_POOL_COLS) * ITEM_SLOT,
    )
}

/// Resolve a mob's vanilla emissive layers against the packed atlas.
///
/// A vanilla emissive layer re-renders the *same model* with a different
/// texture, so each layer is the mob's own quads — filtered to the layer's
/// parts — with their raw model-px UVs normalized against the overlay
/// texture's atlas slot instead of the base texture's. That is why the overlay
/// must share the base texture's pixel dimensions (they all do: spider_eyes is
/// 64x32 like spider.png, the four warden layers are 128^2 like warden.png); a
/// layer whose texture is missing or differently sized is dropped with a
/// warning rather than rendering scrambled.
fn build_emissive(
    m: &mobs::Model,
    def: &mobs::MobDef,
    slots: &std::collections::HashMap<&str, (u32, u32, u32, u32)>,
    pack_emissive: &std::collections::HashMap<&'static str, (u32, u32)>,
) -> Vec<EmissiveDraw> {
    // Re-point one raw quad's model-px UVs at `(ox, oy)`, keeping geometry.
    let repoint = |q: &mobs::RawQuad, ox: u32, oy: u32, tw: u32, th: u32| GpuQuad {
        pos: q.pos,
        uv: q.uv.map(|[u, v]| {
            [
                (ox as f32 + u.clamp(0.0, tw as f32)) / ATLAS_W as f32,
                (oy as f32 + v.clamp(0.0, th as f32)) / ATLAS_H as f32,
            ]
        }),
        part: q.part as u16,
        tex: q.tex as u8,
        facing: q.facing,
        normal: q.normal,
    };
    let mut out = Vec::new();
    // A resource pack's `<texture>_e.png` (ETF / M57) is an always-on
    // fullbright overlay of the whole model — the same shape as a vanilla
    // `EyesLayer`, so it goes through the same path. It sits on the base
    // texture's own UVs at the overlay's atlas slot, and only covers quads that
    // sample the texture it belongs to.
    for (slot, key) in def.textures.iter().enumerate() {
        let Some(&(ox, oy)) = pack_emissive.get(key) else {
            continue;
        };
        let Some(&(_, _, tw, th)) = slots.get(key) else {
            continue;
        };
        let quads: Vec<GpuQuad> = m
            .quads
            .iter()
            .filter(|q| q.tex == slot)
            .map(|q| repoint(q, ox, oy, tw, th))
            .collect();
        if quads.is_empty() {
            continue;
        }
        log::info!("etf: {:?} gains an emissive overlay from {key}", def.kind);
        // Cutout, like vanilla's `entityTranslucentEmissive` — OptiFine renders
        // emissive textures through the same alpha-cutout path.
        out.push(EmissiveDraw {
            quads,
            alpha: mobs::EmissiveAlpha::Always,
            cutout: true,
        });
    }
    for layer in mobs::emissive_layers(def.kind) {
        let Some(&(ox, oy, tw, th)) = slots.get(layer.tex) else {
            log::warn!(
                "entities: {:?} emissive layer {} missing from the atlas — layer dropped",
                def.kind,
                layer.tex
            );
            continue;
        };
        // The base texture the layer's quads were authored against.
        let Some(&(_, _, bw, bh)) = slots.get(def.textures[0]) else {
            continue;
        };
        if (tw, th) != (bw, bh) {
            log::warn!(
                "entities: {:?} emissive layer {} is {tw}x{th} but the base texture is {bw}x{bh} — layer dropped",
                def.kind,
                layer.tex
            );
            continue;
        }
        // Part names: the built-ins carry vanilla's, and a CEM pack model
        // carries the `.jem` bone names — which OptiFine *requires* to be
        // vanilla's part names (that is how a `.jem` identifies what it is
        // replacing), so the same filter applies to both.
        let keep = |part: usize| match layer.parts {
            mobs::PartFilter::All => true,
            mobs::PartFilter::Exact(names) => match m.cem_names.get(part) {
                Some(n) => names.contains(&n.as_str()),
                None => names.contains(&m.parts[part].name),
            },
        };
        let quads: Vec<GpuQuad> = m
            .quads
            .iter()
            .filter(|q| keep(q.part))
            .map(|q| repoint(q, ox, oy, tw, th))
            .collect();
        if quads.is_empty() {
            log::warn!(
                "entities: {:?} emissive layer {} matched no parts — layer dropped",
                def.kind,
                layer.tex
            );
            continue;
        }
        out.push(EmissiveDraw {
            quads,
            alpha: layer.alpha,
            cutout: layer.cutout,
        });
    }
    out
}

/// Shelf-pack `sizes` into the atlas, avoiding the font block. Deterministic:
/// callers pass entries sorted however they like; shelves grow downward,
/// each shelf as tall as its tallest member. Returns per-entry origins
/// (`None` if the atlas overflowed — practically unreachable at 1024²).
fn pack_shelves(sizes: &[(u32, u32)]) -> Vec<Option<(u32, u32)>> {
    let (mut x, mut y, mut shelf_h) = (128u32, 0u32, 0u32);
    let mut out = Vec::with_capacity(sizes.len());
    for &(w, h) in sizes {
        if w > ATLAS_W || h > ATLAS_H {
            out.push(None);
            continue;
        }
        loop {
            // Inside the font's rows, usable x starts at 128.
            let x_min = if y < 128 { 128 } else { 0 };
            if x < x_min {
                x = x_min;
            }
            if x + w <= ATLAS_W && y + h <= ITEM_POOL_Y {
                break;
            }
            if x + w > ATLAS_W {
                // New shelf.
                y += shelf_h.max(1);
                x = 0;
                shelf_h = 0;
                continue;
            }
            // Out of vertical space (the skin pool caps the packer).
            break;
        }
        if x + w <= ATLAS_W && y + h <= ITEM_POOL_Y {
            out.push(Some((x, y)));
            x += w;
            shelf_h = shelf_h.max(h);
        } else {
            out.push(None);
        }
    }
    out
}

/// One baked mob texture, keyed by the registry names in
/// [`mobs::MobDef::textures`] (e.g. "cow", "sheep_wool").
pub struct MobTexEntry<'a> {
    pub key: &'a str,
    pub w: u32,
    pub h: u32,
    pub rgba: &'a [u8],
}

/// One resource-pack alternate texture (ETF / M57): the same pixels-worth of
/// a mob texture, packed elsewhere in the atlas, addressed by a variant id.
/// Must be the base texture's size — the variant reuses its UVs.
pub struct VariantTexEntry<'a> {
    /// The mob-texture key this varies.
    pub base_key: &'static str,
    /// Variant id (`rewo_data::etf::Variant::index`); 0 is the vanilla
    /// texture and never appears here.
    pub index: u32,
    pub w: u32,
    pub h: u32,
    pub rgba: &'a [u8],
}

/// Borrowed view of the baked mob-texture table for `EntityPass::new`. A
/// missing entry degrades that mob to the capsule fallback.
#[derive(Default)]
pub struct MobTextures<'a> {
    pub entries: Vec<MobTexEntry<'a>>,
    /// Resource-pack alternates, if a pack supplied any.
    pub variants: Vec<VariantTexEntry<'a>>,
    /// M175 — vanilla's `isBaby` whole-sheet swaps as
    /// `(kind index, adult key, baby key)`. Both keys must already be baked
    /// entries of the SAME dimensions; the caller (which sees both the
    /// generated table and the bake) filters to those. Empty is the norm for
    /// kinds whose swap is variant-mediated.
    pub baby_swaps: Vec<(u16, &'static str, &'static str)>,
}

pub struct EntityPass {
    /// M436 — the dimension's cardinal lighting is NETHER: entity faces take
    /// the Nether light pair (`Lighting.updateLevel`). Set every frame.
    pub(crate) nether_light: bool,
    layout: vk::PipelineLayout,
    set_layout: vk::DescriptorSetLayout,
    solid_pipeline: vk::Pipeline,
    text_pipeline: vk::Pipeline,
    /// Vanilla's `EYES` / `ENTITY_TRANSLUCENT_EMISSIVE` (M57): translucent,
    /// depth-write off, and `GREATER_OR_EQUAL` so a layer lands on the base
    /// model's own depth instead of being rejected by it.
    emissive_pipeline: vk::Pipeline,
    emissive_verts: u32,
    /// M278 — the glowing entities' outline copies, the seventh range.
    outline_verts: u32,
    /// M412 — the fog words the fragment stages read: camera and
    /// render-distance start, fog colour and end, environmental band.
    /// [`NO_FOG`] until the world renderer sets them, so a GUI preview
    /// pass draws unfogged.
    fog: std::cell::Cell<[[f32; 4]; 3]>,
    /// M285 — the energy swirl (`RenderPipelines.ENERGY_SWIRL`): its own
    /// sheet (`creeper_armor.png`, REPEAT and NEAREST: no `.mcmeta`), its own
    /// additive pipeline, the eighth range.
    swirl: Option<EntityGlint>,
    swirl_pipeline: Option<vk::Pipeline>,
    swirl_verts: u32,
    /// M287 — the powered wither's sheet (`wither_armor.png`) on the same
    /// pipeline, and how many of the swirl range's vertices are the
    /// creeper's (they come first; the wither's follow).
    wither_swirl: Option<EntityGlint>,
    swirl_split: u32,
    /// M286 — the breeze's wind (`RenderPipelines.BREEZE_WIND`): its own
    /// sheet (`breeze_wind.png`, REPEAT and NEAREST), its own translucent
    /// gamma-space pipeline, the ninth range.
    wind: Option<EntityGlint>,
    wind_pipeline: Option<vk::Pipeline>,
    wind_verts: u32,
    /// M306 — `RenderPipelines.ENTITY_TRANSLUCENT` over the atlas: the
    /// translucent blend in gamma space, depth tested and written, no
    /// culling, sorted on upload; the tenth range. `None` without a UNORM
    /// view of the target, where no gamma-space pass draws.
    translucent_pipeline: Option<vk::Pipeline>,
    translucent_verts: u32,
    /// M363 — `RenderTypes.waterMask()`: depth written, no colour, the
    /// boat's water patch.
    water_mask_pipeline: vk::Pipeline,
    water_mask_verts: u32,
    /// The glint pipeline, shared by both sheets (M50). Built by whichever of
    /// the two `init_*_glint` calls runs first.
    glint_pipeline: Option<vk::Pipeline>,
    /// The item glint's sheet and descriptor set (M45) —
    /// `misc/enchanted_glint_item.png`, over what an entity holds or has
    /// dropped. `None` when the jar carried no such texture, in which case
    /// none is drawn.
    glint: Option<EntityGlint>,
    glint_verts: u32,
    /// The worn-armour glint's sheet (M50) — `misc/enchanted_glint_armor.png`.
    armor_glint: Option<EntityGlint>,
    armor_glint_verts: u32,
    trim_verts: u32,
    trim_pipeline: Option<vk::Pipeline>,
    pool: vk::DescriptorPool,
    set: vk::DescriptorSet,
    sampler: vk::Sampler,
    image: vk::Image,
    image_alloc: Option<Allocation>,
    view: vk::ImageView,
    ring: crate::buf_ring::BufRing,
    solid_verts: u32,
    text_verts: u32,
    /// Unit capsule shell: (position [0..1 y, ±0.5 xz], normal).
    capsule: Vec<([f32; 3], [f32; 3])>,
    /// Built mob models, indexed by `EntityModelKind::index()`. `None` =
    /// texture missing (or the capsule kind) → capsule fallback.
    models: Vec<Option<MobModel>>,
    /// Facelabel mode: kinds excluded from the color check (see
    /// `debug_ambiguous_kinds`). Empty outside debug-texture mode.
    debug_ambiguous: Vec<EntityModelKind>,
    // Font metrics (identity values when no font was provided).
    cell: u32,
    advance: [u8; 256],
    white_uv: [f32; 2],
    has_font: bool,
    /// Atlas origin of the default "player" skin slot — the reference the
    /// per-player skin UV offset is measured from.
    player_origin: (u32, u32),
    /// Baked held-item models (M22), moved in at init. `None` until the app
    /// supplies them, which is what a serverless still render does.
    held_items: Option<crate::held::HeldItems>,
    /// Texture index -> atlas item slot, for the textures currently resident.
    item_slots: std::collections::HashMap<u16, u32>,
    /// M246 — texture index -> (large slot, width, height), for the resident
    /// textures bigger than an item slot.
    big_slots: std::collections::HashMap<u16, (u32, u32, u32)>,
    big_ring: SlotRing<u16>,
    /// M247 — the quads the last frame emitted for each block-entity draw,
    /// index-matched to the draws `set_draws` was given. A quad whose texture
    /// is not resident is skipped, so this is what a draw really drew.
    be_quads: Vec<u32>,
    /// `<asset>/<layer>` → the sheet's packed rect in the entity atlas (M46).
    armor_slots: std::collections::HashMap<String, (u32, u32, u32, u32)>,
    /// Sprite path → its origin in the trim pool (M48).
    trim_slots: std::collections::HashMap<String, (u32, u32)>,
    trim_ring: SlotRing<String>,
    /// Round-robin ring over the item pool, like `trim_ring`.
    item_ring: SlotRing<u16>,
    /// The dynamic skin pool (`SKIN_SLOTS`), keyed by player UUID, LRU over
    /// the frames that drew each slot. This pool hands the address *out* and
    /// the app stores it per UUID, so an eviction is returned to the caller,
    /// which must drop the evicted player's address (see [`LruSlots`]).
    skin_lru: LruSlots<u128>,
    /// The cape pool (M60), the same at `CAPE_SLOTS`. A cape origin is an
    /// absolute texel address rather than a delta, so an unreported recycle
    /// would sample a *fixed* wrong rectangle.
    cape_lru: LruSlots<u128>,
    /// Per-entity CEM variable state, persisted across frames (see [`CemVars`]).
    cem_state: std::collections::HashMap<u64, CemVars>,
    /// Frame generation, bumped once per `set_draws`; entities not drawn for
    /// `CEM_STATE_TTL` generations are dropped so despawns can't leak.
    generation: u64,
    /// Monotonic frame count and the real per-frame delta, handed to the
    /// interpreter as `frame_counter` / `frame_time`. FA integrates against
    /// `frame_time` and uses `frame_counter` for its same-frame guard, so both
    /// must be real values rather than constants.
    frame_counter: f32,
    frame_dt: f32,
    prev_time: f32,
    /// Camera eye in world space — the interpreter's `player_pos_*`. FA aims
    /// eyes and heads by comparing it against the mob's own `pos_*`.
    cam_pos: [f32; 3],
    /// The framed-map band's slots (M194), LRU by map id, and which map and
    /// version each resident one holds.
    map_lru: LruSlots<i32>,
    map_resident: std::collections::HashMap<i32, (u32, u64)>,
    /// Which decoration type ids have a sprite in the band.
    map_deco_present: Vec<bool>,
}

/// Generations an unseen entity's CEM state survives before being pruned —
/// long enough that a briefly-culled mob keeps its integrators.
const CEM_STATE_TTL: u64 = 600;

/// One mob model ready to draw: quads in part-local model px with
/// atlas-normalized UVs, the animated parts, and the px→block scale.
pub struct MobModel {
    quads: Vec<GpuQuad>,
    /// M306 — `quads[translucent_from..]` draw in the translucent range
    /// (`entityTranslucent`); `quads.len()` when none do.
    translucent_from: usize,
    /// M285 — `CreeperPowerLayer`'s armour shell, UVs in `0..1` of its own
    /// sheet (not the atlas): drawn only while `EntityDraw::powered`. Empty
    /// for every other kind.
    swirl: Vec<GpuQuad>,
    /// M287 — which swirl sheet [`Self::swirl`] samples.
    swirl_sheet: SwirlSheet,
    /// M294 — the body-armour shell (`HORSE_ARMOR` / `UNDEAD_HORSE_ARMOR`),
    /// UVs in texels of a 64x64 equipment sheet, placed at draw time by the
    /// worn layer's atlas slot. Empty for every other kind.
    body_shell: Vec<GpuQuad>,
    /// M302 — the body shell while ridden, where it differs (the happy
    /// ghast's harness lowers its goggles). Empty for every other kind.
    body_shell_ridden: Vec<GpuQuad>,
    /// M295 — the saddle mesh and its reins (texel UVs of a 64x64 sheet,
    /// placed by the worn layer's slot). Empty for every other kind.
    saddle_shell: Vec<GpuQuad>,
    saddle_reins: Vec<GpuQuad>,
    /// M363 — the boat's water patch (`BOAT_WATER_PATCH`), drawn depth
    /// only. Empty for every other kind.
    water_patch: Vec<GpuQuad>,
    /// M298 — the saddle shell is the body's own mesh (the strider's), so
    /// it lies exactly on the body: it rides the trim range (`EQUAL`, no
    /// write, after the solid one) as M68's coplanar layers do.
    saddle_coplanar: bool,
    /// M286 — the breeze's wind funnel (its texture slot 1), UVs in `0..1`
    /// of its own sheet: `BreezeWindLayer` draws it every frame. Empty for
    /// every other kind (and in the face-label mode, which keeps it in
    /// `quads`).
    wind: Vec<GpuQuad>,
    /// The texture slot vanilla renders through a dye tint (the sheep's
    /// wool), if any — `EntityDraw::dye` multiplies only that slot.
    tinted_slot: Option<u8>,
    /// The texture slot belonging to a render layer a shorn mob skips
    /// (`mobs::shearable_texture`) — `EntityDraw::sheared` drops it (M64).
    shearable_slot: Option<u8>,
    /// M312 — the happy ghast's ropes' texture slot: drawn only while it
    /// holds a leash and wears a harness, with no overlay (`RopesLayer`).
    ropes_slot: Option<u8>,
    /// The texture slot of a **coplanar** render layer
    /// (`mobs::coplanar_layer_texture`) — the sheep's undercoat (M68). Its
    /// quads leave the solid range for the `EQUAL`, no-write trim range,
    /// because they land at exactly the base model's depth.
    coplanar_slot: Option<u8>,
    /// M342 — the collar's slot ([`mobs::collar_texture`]).
    collar_slot: Option<u8>,
    /// A tropical fish's `(body, pattern)` texture slots — the two vanilla
    /// tints from its packed variant (M68).
    fish_slots: Option<(u8, u8)>,
    /// ETF alternates: variant id → per-texture-slot UV offset, added to the
    /// quad's UVs to move it onto the alternate's atlas slot. A variant with
    /// no entry for a slot leaves that slot on the vanilla texture, which is
    /// what a pack varying only one of a mob's textures wants.
    variants: std::collections::HashMap<u16, Vec<[f32; 2]>>,
    /// M175 — per-slot UV offset to the isBaby sheet (see the build site).
    /// `None` for kinds with no swap or whose swap was filtered out.
    baby_offsets: Option<Vec<[f32; 2]>>,
    /// Vanilla emissive layers (`EyesLayer` / `LivingEntityEmissiveLayer`), in
    /// the renderer's `addLayer` order. Empty for most mobs.
    emissive: Vec<EmissiveDraw>,
    parts: Vec<mobs::Part>,
    keyframes: Vec<mobs::KfAnim>,
    /// Wire-event one-shot rigs (`ClientboundEntityEventPacket`).
    event_rigs: Vec<mobs::EventRig>,
    /// Model px → world blocks (vanilla 1/16 × the mob's render scale).
    scale: f32,
    /// Resource-pack CEM animation program (M9c) — drives bone channels
    /// per frame via the expression interpreter. `None` for built-ins.
    cem: Option<crate::cem_anim::AnimProgram>,
    /// Per-bone translation rest baseline for the CEM replace semantics
    /// (see `mobs::Model::cem_translate`). Empty for built-ins.
    cem_translate: Vec<[f32; 3]>,
}

/// One entity's CEM variable slots, carried across frames.
///
/// FA's animation variables are **integrators** — `var.run = var.run ±
/// rate*frame_time`, `var.t_jump`, `var.air`, `var.tr` — and each is gated by
/// `varb.pfc = frame_counter == var.pre_frame_counter`, a "same frame" check.
/// They therefore only advance when both their own value *and*
/// `var.pre_frame_counter` survive to the next frame. Re-zeroing the slots each
/// frame made `pfc` compare `0 == 0`, so every integrator was pinned at its
/// initial value and all smoothing / transition behaviour was inert.
#[derive(Default)]
struct CemVars {
    slots: Vec<f32>,
    /// Frame generation this entity was last drawn on (for pruning).
    seen: u64,
}

/// Key for [`CemVars`]: model kind + the caller's stable per-entity id. The
/// kind is part of the key so a re-used id on a different model can't inherit
/// a slot layout that no longer matches.
fn cem_key(d: &EntityDraw<'_>) -> u64 {
    ((d.kind.index() as u64) << 32) | d.anim_id.to_bits() as u64
}

/// One emissive layer, resolved against the atlas: the mob's own quads,
/// filtered to the layer's parts and re-pointed at the overlay texture's slot
/// (same raw UVs — vanilla re-renders the same geometry).
struct EmissiveDraw {
    quads: Vec<GpuQuad>,
    alpha: mobs::EmissiveAlpha,
    cutout: bool,
}

struct GpuQuad {
    pos: [[f32; 3]; 4],
    uv: [[f32; 2]; 4],
    part: u16,
    /// Which of the mob's textures this samples (index into
    /// `MobDef::textures`). An ETF variant shifts each slot by its own offset,
    /// so the quad has to remember which one it came from — the base bake
    /// folds `tex` into the UVs and would otherwise lose it.
    tex: u8,
    /// Vanilla face label + static-folded model-space normal — kept for the
    /// mobshot facelabel verification (`neutral_quads`).
    facing: Facing,
    normal: [f32; 3],
}

impl EntityPass {
    pub fn new(
        gpu: &mut Gpu,
        color_format: vk::Format,
        font: Option<FontData<'_>>,
        tex: MobTextures<'_>,
    ) -> Result<Self, String> {
        Self::new_with_cem(
            gpu,
            color_format,
            font,
            tex,
            std::collections::HashMap::new(),
        )
    }

    /// `new` plus resource-pack CEM model overrides (M9): a kind present in
    /// `cem` uses that parsed `.jem` model instead of its built-in, going
    /// through the same UV normalization + atlas bake.
    pub fn new_with_cem(
        gpu: &mut Gpu,
        color_format: vk::Format,
        font: Option<FontData<'_>>,
        tex: MobTextures<'_>,
        mut cem: std::collections::HashMap<EntityModelKind, mobs::Model>,
    ) -> Result<Self, String> {
        let device = gpu.device.clone();

        // ---- combined atlas: font at (0,0), mob skins in 64×64 slots ----
        let mut atlas = vec![0u8; (ATLAS_W * ATLAS_H * 4) as usize];
        let (cell, advance, white_texel, has_font) = match &font {
            Some(f) => {
                let side = f.size.min(128) as usize;
                for row in 0..side {
                    let src = row * f.size as usize * 4;
                    let dst = row * ATLAS_W as usize * 4;
                    atlas[dst..dst + side * 4].copy_from_slice(&f.atlas[src..src + side * 4]);
                }
                (f.cell, *f.advance, f.white_texel, true)
            }
            None => (8, [4u8; 256], (0, 16), false),
        };
        // The white texel lives in the (blank) space glyph cell — patch it
        // whether or not a font was blitted (the cell is zeroed either way).
        let wi = ((white_texel.1 * ATLAS_W + white_texel.0) * 4) as usize;
        atlas[wi..wi + 4].copy_from_slice(&[255, 255, 255, 255]);

        // Shelf-pack every provided mob texture (tallest first for tight
        // shelves; the key map keeps lookups order-independent). Pack
        // alternates (ETF) ride along in the same pass so they share the
        // packer's guarantees.
        let mut order: Vec<usize> = (0..tex.entries.len()).collect();
        order.sort_by_key(|&i| {
            let e = &tex.entries[i];
            (std::cmp::Reverse(e.h), std::cmp::Reverse(e.w), e.key)
        });
        // M299 — every sheet gets a one-texel gutter of its own edge texels
        // (`blit_tex_extruded`): a clamped UV rect at a sheet's far edge
        // (the bake clamps the vanilla rects that stray outside their
        // texture) then samples the edge texel, as vanilla's clamp-to-edge
        // sampler does, and not whatever the packer put next to it. Without
        // it the vex sampled a neighbouring sheet once M299's two sheets moved
        // it. The packer places padded cells; the sheet sits one texel in.
        let mut sizes: Vec<(u32, u32)> = order
            .iter()
            .map(|&i| (tex.entries[i].w + 2, tex.entries[i].h + 2))
            .collect();
        sizes.extend(tex.variants.iter().map(|v| (v.w + 2, v.h + 2)));
        let origins: Vec<Option<(u32, u32)>> = pack_shelves(&sizes)
            .into_iter()
            .map(|o| o.map(|(x, y)| (x + 1, y + 1)))
            .collect();
        let mut slots: std::collections::HashMap<&str, (u32, u32, u32, u32)> =
            std::collections::HashMap::new();
        for (slot, &i) in order.iter().enumerate() {
            let e = &tex.entries[i];
            match origins[slot] {
                Some((x, y)) if blit_tex_extruded(&mut atlas, e.rgba, x, y, e.w, e.h) => {
                    slots.insert(e.key, (x, y, e.w, e.h));
                }
                _ => log::warn!(
                    "entities: mob texture {} ({}×{}) didn't pack",
                    e.key,
                    e.w,
                    e.h
                ),
            }
        }
        // (base key, variant id) -> atlas origin, for the offset table below.
        // The reserved emissive id is split out: it is a layer, not a variant a
        // mob can be drawn as.
        let mut variant_slots: std::collections::HashMap<(&'static str, u32), (u32, u32)> =
            std::collections::HashMap::new();
        let mut pack_emissive: std::collections::HashMap<&'static str, (u32, u32)> =
            std::collections::HashMap::new();
        for (n, v) in tex.variants.iter().enumerate() {
            match origins[order.len() + n] {
                Some((x, y)) if blit_tex_extruded(&mut atlas, v.rgba, x, y, v.w, v.h) => {
                    if v.index == EMISSIVE_VARIANT {
                        pack_emissive.insert(v.base_key, (x, y));
                    } else {
                        variant_slots.insert((v.base_key, v.index), (x, y));
                    }
                }
                _ => log::warn!(
                    "entities: variant {} of {} ({}×{}) didn't pack",
                    v.index,
                    v.base_key,
                    v.w,
                    v.h
                ),
            }
        }

        // Facelabel verification mode: replace every mob texture with per-
        // face solid colors so a render proves texture-face correspondence.
        let debug_tex = crate::knobs::var("REWO_MOB_DEBUG_TEX").is_ok_and(|v| v == "1");

        // Build each registry mob whose textures are all present.
        let mut models: Vec<Option<MobModel>> = (0..EntityModelKind::COUNT).map(|_| None).collect();
        // M175 — the isBaby swaps by kind index, for the per-model offset
        // build below.
        let baby_by_kind: std::collections::HashMap<u16, (&str, &str)> = tex
            .baby_swaps
            .iter()
            .map(|(k, a, b)| (*k, (*a, *b)))
            .collect();
        // Facelabel mode: textures where different face labels paint the
        // same texel (vanilla reuses some regions across faces — the breeze
        // wind funnel's concentric shells) can't be verified by color; the
        // checker skips the kinds that sample them.
        let mut painted: std::collections::HashMap<(u32, u32), [u8; 3]> =
            std::collections::HashMap::new();
        let mut ambiguous_tex: std::collections::HashSet<&'static str> =
            std::collections::HashSet::new();
        let mut debug_ambiguous: Vec<EntityModelKind> = Vec::new();
        for def in mobs::MOBS {
            let origins: Option<Vec<(u32, u32, u32, u32)>> =
                def.textures.iter().map(|k| slots.get(k).copied()).collect();
            let Some(origins) = origins else { continue };
            // Resource-pack CEM override (M9) takes this kind's model instead
            // of the built-in; both normalize UVs the same way below.
            let m = match cem.remove(&def.kind) {
                Some(mut m) => {
                    // A pack model replaces the geometry but must keep this
                    // kind's *render scale* — packs author vanilla model-px,
                    // and vanilla applies a per-mob multiplier on top (ghast
                    // 4.5×, elder guardian 2.35×, slime 2×, cave spider 0.7×).
                    // `model_from_jem` can't know it, so inherit it here or
                    // every scaled mob renders at the wrong size.
                    m.scale = (def.build)().scale;
                    log::info!(
                        "cem: {:?} using pack model ({} quads)",
                        def.kind,
                        m.quads.len()
                    );
                    m
                }
                None => (def.build)(),
            };
            if debug_tex {
                for q in &m.quads {
                    if !paint_debug_rect(&mut atlas, origins[q.tex], q, &mut painted)
                        && ambiguous_tex.insert(def.textures[q.tex])
                    {
                        log::info!(
                            "mob debug-tex: {} ({:?} {:?} quad) repaints a texel with a new label",
                            def.textures[q.tex],
                            def.kind,
                            q.facing
                        );
                    }
                }
            }
            // Vanilla emissive layers: the same geometry, filtered to the
            // layer's parts and sampled from an overlay texture. Skipped in
            // facelabel mode — the overlay would repaint texels the base model
            // already labelled and make every emissive mob ambiguous.
            let emissive = if debug_tex {
                Vec::new()
            } else {
                build_emissive(&m, def, &slots, &pack_emissive)
            };
            // M286 — the breeze's wind funnel leaves the body for its own
            // layer (`BreezeWindLayer`), keeping its texel UVs over its own
            // sheet.
            let wind_slot = (def.kind == EntityModelKind::Breeze && !debug_tex)
                .then_some(mobs::BREEZE_WIND_SLOT);
            let wind: Vec<GpuQuad> = m
                .quads
                .iter()
                .filter(|q| wind_slot == Some(q.tex))
                .map(|q| {
                    let [tw, th] = mobs::BREEZE_WIND_TEXTURE;
                    GpuQuad {
                        pos: q.pos,
                        uv: q.uv.map(|[u, v]| [u / tw, v / th]),
                        part: q.part as u16,
                        tex: q.tex as u8,
                        facing: q.facing,
                        normal: q.normal,
                    }
                })
                .collect();
            let quads: Vec<GpuQuad> = m
                .quads
                .iter()
                .filter(|q| wind_slot != Some(q.tex))
                .map(|q| {
                    let (ox, oy, tw, th) = origins[q.tex];
                    // Clamp into the texture rect — a couple of vanilla fin
                    // rects stray outside their texture (clamp-sampler
                    // behavior); unclamped they'd bleed into atlas neighbors.
                    let uv = q.uv.map(|[u, v]| {
                        [
                            (ox as f32 + u.clamp(0.0, tw as f32)) / ATLAS_W as f32,
                            (oy as f32 + v.clamp(0.0, th as f32)) / ATLAS_H as f32,
                        ]
                    });
                    GpuQuad {
                        pos: q.pos,
                        uv,
                        part: q.part as u16,
                        tex: q.tex as u8,
                        facing: q.facing,
                        normal: q.normal,
                    }
                })
                .collect();
            // Per-variant UV offsets: for each variant id the pack packed for
            // any of this mob's textures, the shift from that texture's slot to
            // the alternate's. Slots the variant doesn't cover stay at zero
            // (i.e. keep the vanilla texture).
            let mut variants: std::collections::HashMap<u16, Vec<[f32; 2]>> =
                std::collections::HashMap::new();
            for (&(base_key, index), &(vx, vy)) in &variant_slots {
                let Some(slot) = def.textures.iter().position(|k| *k == base_key) else {
                    continue;
                };
                let (bx, by, _, _) = origins[slot];
                let e = variants
                    .entry(index as u16)
                    .or_insert_with(|| vec![[0.0, 0.0]; def.textures.len()]);
                e[slot] = [
                    (vx as f32 - bx as f32) / ATLAS_W as f32,
                    (vy as f32 - by as f32) / ATLAS_H as f32,
                ];
            }
            // M175 — the isBaby whole-sheet swap, as a per-slot UV offset to
            // wherever the baby sheet packed. Exact only because the bake
            // admitted same-size pairs; the offset rides the ADULT slot (the
            // one `getTextureLocation` returns for adults) and every other
            // slot stays at zero, so a mob's secondary layers keep their own
            // sheets exactly as vanilla's non-swapping render layers do.
            let baby_offsets =
                baby_by_kind
                    .get(&(def.kind.index() as u16))
                    .and_then(|(adult_key, baby_key)| {
                        let slot = def.textures.iter().position(|k| *k == *adult_key)?;
                        let (_, ax, ay) = slots.get(*adult_key).map(|(x, y, _, _)| ((), *x, *y))?;
                        let (_, bx, by) = slots.get(*baby_key).map(|(x, y, _, _)| ((), *x, *y))?;
                        let mut v = vec![[0.0f32, 0.0]; def.textures.len()];
                        v[slot] = [
                            (bx as f32 - ax as f32) / ATLAS_W as f32,
                            (by as f32 - ay as f32) / ATLAS_H as f32,
                        ];
                        Some(v)
                    });
            // M285 — the charged creeper's shell, built only for the vanilla
            // creeper (a pack model replaces the base geometry, and its parts
            // need not line up with the shell's) and only when its parts are
            // the base's, which the shell's transforms are indexed by.
            // M287 — and the powered wither's (`WITHER_ARMOR`).
            let shell = match def.kind {
                EntityModelKind::Creeper => Some((
                    mobs::creeper_armor(),
                    mobs::CREEPER_ARMOR_TEXTURE,
                    SwirlSheet::Creeper,
                )),
                EntityModelKind::Wither => Some((
                    mobs::wither_armor(),
                    mobs::WITHER_ARMOR_TEXTURE,
                    SwirlSheet::Wither,
                )),
                _ => None,
            }
            .filter(|_| !debug_tex);
            let swirl_sheet = shell.as_ref().map_or(SwirlSheet::Creeper, |s| s.2);
            let swirl = if let Some((shell, [tw, th], _)) = shell {
                if shell.parts.len() == m.parts.len() {
                    shell
                        .quads
                        .iter()
                        .map(|q| GpuQuad {
                            pos: q.pos,
                            uv: q.uv.map(|[u, v]| [u / tw, v / th]),
                            part: q.part as u16,
                            tex: 0,
                            facing: q.facing,
                            normal: q.normal,
                        })
                        .collect()
                } else {
                    Vec::new()
                }
            } else {
                Vec::new()
            };
            // M294 — the horse family's body-armour shell.
            let body_shell: Vec<GpuQuad> = match def.kind {
                EntityModelKind::Horse => Some(mobs::horse_armor()),
                EntityModelKind::SkeletonHorse | EntityModelKind::ZombieHorse => {
                    Some(mobs::undead_horse_armor())
                }
                // M297 — the llamas' decor shell (`LLAMA_DECOR`).
                EntityModelKind::Llama | EntityModelKind::TraderLlama => Some(mobs::llama_decor()),
                // M299 — the wolf's armour shell (`WOLF_ARMOR`).
                EntityModelKind::Wolf => Some(mobs::wolf_armor()),
                // M301 — the nautilus's (`NAUTILUS_ARMOR`), the zombie's too.
                EntityModelKind::Nautilus | EntityModelKind::ZombieNautilus => {
                    Some(mobs::nautilus_armor())
                }
                // M350 — the same shell on the coral mesh's part list.
                EntityModelKind::ZombieNautilusCoral => Some(mobs::nautilus_coral_armor()),
                // M302 — the happy ghast's harness (goggles up).
                EntityModelKind::HappyGhast => Some(mobs::happy_ghast_harness(false)),
                _ => None,
            }
            .filter(|shell| !debug_tex && shell.parts.len() == m.parts.len())
            .map(|shell| {
                shell
                    .quads
                    .iter()
                    .map(|q| GpuQuad {
                        pos: q.pos,
                        uv: q.uv,
                        part: q.part as u16,
                        tex: 0,
                        facing: q.facing,
                        normal: q.normal,
                    })
                    .collect()
            })
            .unwrap_or_default();
            // M295 — the horse family's saddle and reins.
            let raw = |q: &mobs::RawQuad| GpuQuad {
                pos: q.pos,
                uv: q.uv,
                part: q.part as u16,
                tex: 0,
                facing: q.facing,
                normal: q.normal,
            };
            // M363 — the boat's (and the chest boat's) water patch.
            let water_patch: Vec<GpuQuad> = match def.kind {
                EntityModelKind::Boat | EntityModelKind::ChestBoat if !debug_tex => {
                    mobs::boat_water_patch().quads.iter().map(raw).collect()
                }
                _ => Vec::new(),
            };
            let (saddle_shell, saddle_reins): (Vec<GpuQuad>, Vec<GpuQuad>) = match def.kind {
                EntityModelKind::Horse => Some((false, mobs::HORSE_SCALE)),
                EntityModelKind::Donkey => Some((true, mobs::DONKEY_SCALE)),
                EntityModelKind::Mule => Some((true, mobs::MULE_SCALE)),
                EntityModelKind::SkeletonHorse | EntityModelKind::ZombieHorse => Some((false, 1.0)),
                _ => None,
            }
            .filter(|_| !debug_tex)
            .map(|(ears, scale)| mobs::equine_saddle(ears, scale))
            // M298 — the pig's (the pig at 0.5) and the strider's (its own
            // mesh), with no reins.
            .or_else(|| match def.kind {
                // M349 — and the cold pig's: `PIG_SADDLE` is one layer
                // whatever the variant's mesh.
                EntityModelKind::Pig | EntityModelKind::ColdPig if !debug_tex => {
                    Some((mobs::pig_saddle(), mobs::empty_model()))
                }
                EntityModelKind::Strider if !debug_tex => {
                    Some((mobs::strider_saddle(), mobs::empty_model()))
                }
                // M300 — the camel's saddle, bridle and reins.
                // M303 — the camel husk's (`createCamelSaddleLayer` again).
                EntityModelKind::Camel | EntityModelKind::CamelHusk if !debug_tex => {
                    Some(mobs::camel_saddle())
                }
                // M301 — the nautilus's saddle (the zombie's too).
                EntityModelKind::Nautilus | EntityModelKind::ZombieNautilus if !debug_tex => {
                    Some((mobs::nautilus_saddle(), mobs::empty_model()))
                }
                EntityModelKind::ZombieNautilusCoral if !debug_tex => {
                    Some((mobs::nautilus_coral_saddle(), mobs::empty_model()))
                }
                _ => None,
            })
            .filter(|(saddle, _)| saddle.parts.len() == m.parts.len())
            .map(|(saddle, reins)| {
                (
                    saddle.quads.iter().map(raw).collect(),
                    reins.quads.iter().map(raw).collect(),
                )
            })
            .unwrap_or_default();
            // M306 — the translucent tail. Kept opaque under the face-label
            // debug texture (a blend would mix the labels it decodes) and on a
            // pack model (its own geometry carries no marker).
            let translucent_from = match (debug_tex, m.translucent_from) {
                (false, Some(i)) if wind_slot.is_none() => i.min(quads.len()),
                _ => quads.len(),
            };
            // M302 — and its goggles-down twin while ridden.
            let body_shell_ridden: Vec<GpuQuad> =
                if def.kind == EntityModelKind::HappyGhast && !body_shell.is_empty() {
                    mobs::happy_ghast_harness(true)
                        .quads
                        .iter()
                        .map(raw)
                        .collect()
                } else {
                    Vec::new()
                };
            models[def.kind.index()] = Some(MobModel {
                quads,
                translucent_from,
                body_shell,
                body_shell_ridden,
                saddle_shell,
                saddle_reins,
                water_patch,
                saddle_coplanar: def.kind == EntityModelKind::Strider,
                swirl,
                swirl_sheet,
                wind,
                tinted_slot: mobs::tinted_texture(def.kind)
                    .and_then(|k| def.textures.iter().position(|t| *t == k))
                    .map(|s| s as u8),
                shearable_slot: mobs::shearable_texture(def.kind)
                    .and_then(|k| def.textures.iter().position(|t| *t == k))
                    .map(|s| s as u8),
                ropes_slot: def
                    .textures
                    .iter()
                    .position(|t| *t == "happy_ghast_ropes")
                    .map(|s| s as u8),
                collar_slot: mobs::collar_texture(def.kind)
                    .and_then(|k| def.textures.iter().position(|t| *t == k))
                    .map(|s| s as u8),
                coplanar_slot: mobs::coplanar_layer_texture(def.kind)
                    .and_then(|k| def.textures.iter().position(|t| *t == k))
                    .map(|s| s as u8),
                fish_slots: mobs::fish_tint_textures(def.kind).and_then(|(b, p)| {
                    let f = |k: &str| def.textures.iter().position(|t| *t == k).map(|s| s as u8);
                    Some((f(b)?, f(p)?))
                }),
                variants,
                baby_offsets,
                emissive,
                parts: m.parts,
                keyframes: m.keyframes,
                event_rigs: m.event_rigs,
                scale: m.scale / 16.0,
                cem: m.cem,
                cem_translate: m.cem_translate,
            });
        }
        if debug_tex {
            for def in mobs::MOBS {
                if def.textures.iter().any(|k| ambiguous_tex.contains(k)) {
                    debug_ambiguous.push(def.kind);
                }
            }
        }
        let (image, image_alloc, view) = create_texture(gpu, &atlas, ATLAS_W, ATLAS_H)?;
        let white_uv = [
            (white_texel.0 as f32 + 0.5) / ATLAS_W as f32,
            (white_texel.1 as f32 + 0.5) / ATLAS_H as f32,
        ];

        unsafe {
            let sampler = device
                .create_sampler(
                    &vk::SamplerCreateInfo::default()
                        .mag_filter(vk::Filter::NEAREST)
                        .min_filter(vk::Filter::NEAREST)
                        .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                        .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                        .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE),
                    None,
                )
                .map_err(|e| format!("entity sampler: {e}"))?;

            let bindings = [vk::DescriptorSetLayoutBinding::default()
                .binding(0)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::FRAGMENT)];
            let set_layout = device
                .create_descriptor_set_layout(
                    &vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings),
                    None,
                )
                .map_err(|e| format!("entity set layout: {e}"))?;
            let pool_sizes = [vk::DescriptorPoolSize::default()
                .ty(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .descriptor_count(1)];
            let pool = device
                .create_descriptor_pool(
                    &vk::DescriptorPoolCreateInfo::default()
                        .max_sets(1)
                        .pool_sizes(&pool_sizes),
                    None,
                )
                .map_err(|e| format!("entity pool: {e}"))?;
            let set_layouts = [set_layout];
            let set = device
                .allocate_descriptor_sets(
                    &vk::DescriptorSetAllocateInfo::default()
                        .descriptor_pool(pool)
                        .set_layouts(&set_layouts),
                )
                .map_err(|e| format!("entity set: {e}"))?[0];
            let image_info = [vk::DescriptorImageInfo::default()
                .sampler(sampler)
                .image_view(view)
                .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
            device.update_descriptor_sets(
                &[vk::WriteDescriptorSet::default()
                    .dst_set(set)
                    .dst_binding(0)
                    .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                    .image_info(&image_info)],
                &[],
            );

            // M412 — `view_proj` for the vertex stage, then the fog words
            // (three vec4s) for the fragment stage.
            let pc = [
                vk::PushConstantRange::default()
                    .stage_flags(vk::ShaderStageFlags::VERTEX)
                    .offset(0)
                    .size(64),
                vk::PushConstantRange::default()
                    .stage_flags(vk::ShaderStageFlags::FRAGMENT)
                    .offset(64)
                    .size(48),
            ];
            let layout = device
                .create_pipeline_layout(
                    &vk::PipelineLayoutCreateInfo::default()
                        .set_layouts(&set_layouts)
                        .push_constant_ranges(&pc),
                    None,
                )
                .map_err(|e| format!("entity layout: {e}"))?;
            let solid_pipeline =
                build_pipeline(&device, layout, color_format, true, vk::CompareOp::GREATER)?;
            let text_pipeline =
                build_pipeline(&device, layout, color_format, false, vk::CompareOp::GREATER)?;
            // M48: the armour trim. Blended, no depth write, and `EQUAL`
            // against the armour it decorates.
            let trim_pipeline =
                build_pipeline(&device, layout, color_format, false, vk::CompareOp::EQUAL)?;
            // M57: vanilla's `RenderPipelines.EYES` / `ENTITY_TRANSLUCENT_EMISSIVE`
            // — translucent blend, depth-write off, `CompareOp.GREATER_THAN_OR_EQUAL`.
            // The `OR_EQUAL` is load-bearing: an emissive layer redraws geometry
            // whose depth the solid pass just wrote, so under the plain `GREATER`
            // every fragment would be rejected. (The `GREATER` half of vanilla's
            // own constant incidentally confirms 26.x is reversed-Z, the
            // convention Rewo adopted in M4.)
            let emissive_pipeline = build_pipeline(
                &device,
                layout,
                color_format,
                false,
                vk::CompareOp::GREATER_OR_EQUAL,
            )?;
            // M306 — `DepthStencilState.DEFAULT` (LEQUAL with writes, so
            // reversed-Z `GREATER_OR_EQUAL`) and `BlendFunction.TRANSLUCENT`.
            let translucent_pipeline = match crate::world::unorm_of(color_format) {
                Some(f) => Some(build_pipeline_full(
                    &device,
                    layout,
                    f,
                    PipelineState {
                        blend: true,
                        depth_write: true,
                        color_write: true,
                    },
                    vk::CompareOp::GREATER_OR_EQUAL,
                    include_bytes!(concat!(env!("OUT_DIR"), "/entity_translucent.frag.spv")),
                    false,
                )?),
                None => None,
            };

            // M363 — `RenderTypes.waterMask()`: `DepthStencilState.DEFAULT`
            // (reversed-Z `GREATER_OR_EQUAL` with writes) and no colour
            // write; the frag shader only for its alpha test, which the
            // patch's white texel passes.
            let water_mask_pipeline = build_pipeline_full(
                &device,
                layout,
                color_format,
                PipelineState {
                    blend: false,
                    depth_write: true,
                    color_write: false,
                },
                vk::CompareOp::GREATER_OR_EQUAL,
                include_bytes!(concat!(env!("OUT_DIR"), "/entity.frag.spv")),
                false,
            )?;

            let ring = crate::buf_ring::BufRing::with_capacity(
                gpu,
                "entity-verts",
                vk::BufferUsageFlags::VERTEX_BUFFER,
                MAX_VERTS as u64 * VERTEX_STRIDE,
            )?;

            Ok(Self {
                nether_light: false,
                layout,
                set_layout,
                solid_pipeline,
                text_pipeline,
                emissive_pipeline,
                emissive_verts: 0,
                translucent_pipeline,
                translucent_verts: 0,
                water_mask_pipeline,
                water_mask_verts: 0,
                outline_verts: 0,
                fog: std::cell::Cell::new(NO_FOG),
                swirl: None,
                swirl_pipeline: None,
                swirl_verts: 0,
                wither_swirl: None,
                swirl_split: 0,
                wind: None,
                wind_pipeline: None,
                wind_verts: 0,
                glint_pipeline: None,
                glint: None,
                glint_verts: 0,
                armor_glint: None,
                armor_glint_verts: 0,
                trim_verts: 0,
                trim_pipeline: Some(trim_pipeline),
                pool,
                set,
                sampler,
                image,
                image_alloc: Some(image_alloc),
                view,
                ring,
                solid_verts: 0,
                text_verts: 0,
                capsule: unit_capsule(),
                models,
                debug_ambiguous,
                cell,
                advance,
                white_uv,
                has_font,
                trim_slots: std::collections::HashMap::new(),
                trim_ring: SlotRing::new(TRIM_SLOTS),
                player_origin: slots
                    .get("player")
                    .map(|&(x, y, _, _)| (x, y))
                    .unwrap_or((0, 0)),
                // Armour sheets are looked up by name every frame — a mob's
                // model is fixed at build time, but what it wears is not.
                armor_slots: slots
                    .iter()
                    // `<layer dir>/<texture>` — both humanoid layer types
                    // start with `humanoid`, and no mob texture does.
                    // M294 — and the horse's body armour.
                    // M295 — and the saddles' (`<kind>_saddle/`).
                    .filter(|(k, _)| {
                        k.starts_with("humanoid")
                            // M403 — the elytra's sheet.
                            || k.starts_with("wings/")
                            || k.starts_with("horse_body/")
                            || k.starts_with("llama_body/")
                            || k.starts_with("wolf_body/")
                            || k.starts_with("nautilus_body/")
                            || k.starts_with("happy_ghast_body/")
                            || k.starts_with("wolf_armor_crackiness_")
                            || k.split('/').next().is_some_and(|d| d.ends_with("_saddle"))
                    })
                    .map(|(k, v)| ((*k).to_string(), *v))
                    .collect(),
                skin_lru: LruSlots::new(SKIN_SLOTS),
                cape_lru: LruSlots::new(CAPE_SLOTS),
                held_items: None,
                item_slots: std::collections::HashMap::new(),
                item_ring: SlotRing::new(ITEM_SLOTS),
                big_slots: std::collections::HashMap::new(),
                big_ring: SlotRing::new(BIG_SLOTS),
                be_quads: Vec::new(),
                cem_state: std::collections::HashMap::new(),
                generation: 0,
                frame_counter: 0.0,
                frame_dt: 1.0 / 20.0,
                prev_time: f32::NAN,
                cam_pos: [0.0; 3],
                map_lru: LruSlots::new(MAP_SLOTS),
                map_resident: std::collections::HashMap::new(),
                map_deco_present: Vec::new(),
            })
        }
    }

    // Reserve the next dynamic skin slot, upload a 64×64 RGBA skin into it,
    // and return the normalized UV offset relocating the default player
    // quads onto it (feed to `EntityDraw::skin_uv`). `rgba` must be
    // `64*64*4` bytes. The upload is an ordered, asynchronous transfer
    // (see [`upload_region`]) — no idle.

    /// Install the baked held-item models (M22). Textures are *not* uploaded
    /// here: 26.2 ships 1233 of them and the atlas band holds 1024 slots, so
    /// they are paged in on demand by [`Self::prepare_held_items`].
    pub fn set_held_items(&mut self, items: crate::held::HeldItems) {
        self.held_items = Some(items);
    }

    /// M260 — append block-entity-path models built after the bake (a block
    /// display's state) and the textures they added. The new quads index
    /// `textures` from the current end, so every installed index and every
    /// paged atlas slot stays valid. No-op before [`Self::set_held_items`].
    pub fn extend_held_items(
        &mut self,
        models: Vec<(String, crate::held::HeldItemModel)>,
        textures: Vec<crate::held::HeldTexture>,
    ) {
        if let Some(h) = self.held_items.as_mut() {
            h.textures.extend(textures);
            h.block_entities.extend(models);
        }
    }

    /// Whether an item name has a baked held model.
    pub fn has_held_item(&self, name: &str) -> bool {
        self.held_items
            .as_ref()
            .is_some_and(|h| h.models.contains_key(name))
    }

    /// Page in every texture the named items need, before the frame is built.
    ///
    /// Separate from `set_entities` because uploading needs the device, and
    /// because only the handful of items actually held this frame should
    /// occupy the pool. Unknown names are ignored — a suppressed item simply
    /// has no model.
    pub fn prepare_held_items(&mut self, gpu: &mut Gpu, names: &[&str]) -> Result<(), String> {
        let Some(items) = self.held_items.take() else {
            return Ok(());
        };
        let mut result = Ok(());
        // M419 — and the ground and frame variants' textures with each.
        let models = names.iter().flat_map(|n| {
            [items.any(n), items.ground.get(*n), items.fixed.get(*n)]
                .into_iter()
                .flatten()
        });
        for model in models {
            for q in &model.quads {
                if self.item_slots.contains_key(&q.tex) || self.big_slots.contains_key(&q.tex) {
                    continue;
                }
                let Some(tex) = items.textures.get(q.tex as usize) else {
                    continue;
                };
                // M246 — anything else up to 64×64 (a block-entity sheet)
                // goes in the large pool, at its own size in a 64×64 slot.
                if (tex.w != ITEM_SLOT || tex.h != ITEM_SLOT)
                    && tex.w <= BIG_SLOT
                    && tex.h <= BIG_SLOT
                    && tex.rgba.len() == (tex.w * tex.h * 4) as usize
                {
                    let (slot, evicted) = self.big_ring.claim(q.tex);
                    if let Some(old) = evicted {
                        self.big_slots.remove(&old);
                    }
                    let (sx, sy) = big_slot_origin(slot);
                    if let Err(e) = upload_region(gpu, self.image, &tex.rgba, sx, sy, tex.w, tex.h)
                    {
                        result = Err(e);
                        break;
                    }
                    self.big_slots.insert(q.tex, (slot, tex.w, tex.h));
                    continue;
                }
                // Item slots are one texel-block; a larger sprite is skipped
                // rather than scaled, so it is visibly absent instead of
                // subtly wrong.
                if tex.w != ITEM_SLOT || tex.h != ITEM_SLOT {
                    continue;
                }
                // Past 1,024 distinct textures the ring recycles, and the key
                // that used to own the slot has to leave the map with it —
                // otherwise that texture id keeps resolving to this sprite's
                // pixels and the item renders as whatever was paged in over it.
                let (slot, evicted) = self.item_ring.claim(q.tex);
                if let Some(old) = evicted {
                    self.item_slots.remove(&old);
                }
                let (sx, sy) = item_slot_origin(slot);
                if let Err(e) =
                    upload_region(gpu, self.image, &tex.rgba, sx, sy, ITEM_SLOT, ITEM_SLOT)
                {
                    result = Err(e);
                    break;
                }
                self.item_slots.insert(q.tex, slot);
            }
        }
        self.held_items = Some(items);
        result
    }

    /// Install the map decoration sprites (M194; `textures/map/decorations`,
    /// 8 x 8 RGBA each, by raw `map_decoration_type` id) into the framed-map
    /// band. A `None` or wrongly sized sprite stays absent and its
    /// decorations draw nothing, as vanilla's null `atlasSprite` does.
    pub fn init_map_decorations(
        &mut self,
        gpu: &mut Gpu,
        sprites: &[Option<&[u8]>],
    ) -> Result<(), String> {
        let mut present = vec![false; sprites.len()];
        for (id, sprite) in sprites.iter().enumerate() {
            let Some(rgba) = sprite.filter(|s| s.len() == (MAP_DECO_SPRITE.pow(2) * 4) as usize)
            else {
                continue;
            };
            let Some((x, y)) = map_deco_origin(id as i32) else {
                continue;
            };
            upload_region(
                gpu,
                self.image,
                rgba,
                x,
                y,
                MAP_DECO_SPRITE,
                MAP_DECO_SPRITE,
            )?;
            present[id] = true;
        }
        self.map_deco_present = present;
        Ok(())
    }

    /// Page in this frame's framed maps (M194): each `(map id, version)`
    /// keeps its slot if resident at that version, else claims the least
    /// recently drawn one and uploads `rgba_of(id)` (128 x 128 RGBA,
    /// `ClientMap::rgba`). Past [`MAP_SLOTS`] maps on screen at once the rest
    /// draw no pixels (a recorded deviation: vanilla's `MapTextureManager`
    /// has no cap).
    pub fn prepare_frame_maps(
        &mut self,
        gpu: &mut Gpu,
        maps: &[(i32, u64)],
        rgba_of: &mut dyn FnMut(i32) -> Option<Vec<u8>>,
    ) -> Result<(), String> {
        self.map_lru.begin_frame();
        for &(id, version) in maps {
            if let Some(&(slot, v)) = self.map_resident.get(&id) {
                if v == version {
                    self.map_lru.touch(slot);
                    continue;
                }
            }
            let Some((slot, evicted)) = self.map_lru.claim(id) else {
                continue;
            };
            if let Some(old) = evicted {
                self.map_resident.remove(&old);
            }
            self.map_resident.remove(&id);
            let Some(rgba) = rgba_of(id).filter(|r| r.len() == (MAP_SLOT * MAP_SLOT * 4) as usize)
            else {
                continue;
            };
            upload_region(
                gpu,
                self.image,
                &rgba,
                slot * MAP_SLOT,
                MAP_POOL_Y,
                MAP_SLOT,
                MAP_SLOT,
            )?;
            self.map_resident.insert(id, (slot, version));
        }
        Ok(())
    }

    /// The atlas UV rect `(u0, v0, du, dv)` of a resident framed map.
    pub fn frame_map_uv(&self, map_id: i32) -> Option<[f32; 4]> {
        let (slot, _) = *self.map_resident.get(&map_id)?;
        Some([
            (slot * MAP_SLOT) as f32 / ATLAS_W as f32,
            MAP_POOL_Y as f32 / ATLAS_H as f32,
            MAP_SLOT as f32 / ATLAS_W as f32,
            MAP_SLOT as f32 / ATLAS_H as f32,
        ])
    }

    /// Every `(texture id, slot)` the **item** pool currently addresses, and
    /// every `(sprite path, atlas origin)` the **trim** pool does.
    ///
    /// Read-only, and they exist for one reason: the invariant that makes
    /// [`SlotRing`] worth having — *no two live keys resolve to one slot* — is
    /// otherwise unobservable from outside this file, so the caller-side
    /// eviction could be (and was) deleted with every gate staying green.
    /// `mobtexshot`'s `m10`/`m11` state the invariant on these; nothing in the
    /// renderer calls them.
    ///
    /// The map's size is half the claim and the distinctness of the slots is
    /// the other half: without the eviction the map simply grows past the
    /// pool, so `len()` exceeds the cap *and*, by pigeonhole, two keys share a
    /// slot. With it, `len() <= cap` and the slots are pairwise distinct.
    pub fn item_slot_pairs(&self) -> Vec<(u16, u32)> {
        self.item_slots.iter().map(|(k, v)| (*k, *v)).collect()
    }

    /// The trim pool's half of [`Self::item_slot_pairs`].
    pub fn trim_slot_pairs(&self) -> Vec<(String, (u32, u32))> {
        self.trim_slots
            .iter()
            .map(|(k, v)| (k.clone(), *v))
            .collect()
    }

    /// Atlas UV rect `(u0, v0, du, dv)` of a resident item texture.
    fn item_uv(&self, tex: u16) -> Option<[f32; 4]> {
        if let Some(&(slot, w, h)) = self.big_slots.get(&tex) {
            let (sx, sy) = big_slot_origin(slot);
            return Some([
                sx as f32 / ATLAS_W as f32,
                sy as f32 / ATLAS_H as f32,
                w as f32 / ATLAS_W as f32,
                h as f32 / ATLAS_H as f32,
            ]);
        }
        let slot = *self.item_slots.get(&tex)?;
        let (sx, sy) = item_slot_origin(slot);
        Some([
            sx as f32 / ATLAS_W as f32,
            sy as f32 / ATLAS_H as f32,
            ITEM_SLOT as f32 / ATLAS_W as f32,
            ITEM_SLOT as f32 / ATLAS_H as f32,
        ])
    }

    /// Put one generated trim sprite in the pool, or return where it already
    /// is (M48).
    ///
    /// Keyed by the sprite path, so the same pattern-and-material worn by two
    /// entities is permuted and uploaded once. The pool wraps round-robin like
    /// the skin pool: sixty-four live sprites is far past any real scene, and
    /// wrapping recycles rather than failing.
    pub fn upload_trim(
        &mut self,
        gpu: &mut Gpu,
        key: &str,
        rgba: &[u8],
        w: u32,
        h: u32,
    ) -> Option<(u32, u32)> {
        if let Some(&o) = self.trim_slots.get(key) {
            return Some(o);
        }
        if w != TRIM_SLOT_W || h != TRIM_SLOT_H || rgba.len() != (w * h * 4) as usize {
            log::warn!(
                "entities: trim sprite {key} is {w}x{h}, expected {TRIM_SLOT_W}x{TRIM_SLOT_H}"
            );
            return None;
        }
        // Past 64 distinct sprites the ring recycles; the evicted sprite path
        // must leave `trim_slots` with it, or that path still resolves here and
        // the trim it names renders as this one.
        let (slot, evicted) = self.trim_ring.claim(key.to_string());
        if let Some(old) = evicted {
            self.trim_slots.remove(&old);
        }
        let (sx, sy) = trim_slot_origin(slot);
        upload_region(gpu, self.image, rgba, sx, sy, TRIM_SLOT_W, TRIM_SLOT_H).ok()?;
        self.trim_slots.insert(key.to_string(), (sx, sy));
        Some((sx, sy))
    }

    /// Upload `owner`'s 64x64 skin into the skin pool. Returns the UV delta
    /// for `EntityDraw::skin_uv` and the owner whose slot was taken, if any:
    /// that owner's stored delta now addresses this skin and must be dropped.
    /// Errs when every slot was drawn this frame (see [`LruSlots`]).
    pub fn upload_skin(
        &mut self,
        gpu: &mut Gpu,
        owner: u128,
        rgba: &[u8],
    ) -> Result<([f32; 2], Option<u128>), String> {
        let (slot, evicted) = self
            .skin_lru
            .claim(owner)
            .ok_or("every skin slot is drawn this frame")?;
        let (sx, sy) = skin_slot_origin(slot);
        upload_region(gpu, self.image, rgba, sx, sy, SKIN_SLOT, SKIN_SLOT)?;
        Ok((skin_uv_of_slot(self.player_origin, slot), evicted))
    }

    /// Claim a cape slot and upload one player's cape sheet into it (M60).
    ///
    /// Returns the slot's **atlas origin in texels**, not a UV delta. A skin
    /// relocates the player model's baked quads, so it answers with an offset
    /// to add to them; the cape has no baked quads to relocate — its emitter
    /// builds UVs from this origin directly, the way `emit_armor` does with a
    /// trim's.
    ///
    /// LRU per owner, like the skin pool: the second value is the owner whose
    /// slot was taken, whose stored origin must be dropped.
    pub fn upload_cape(
        &mut self,
        gpu: &mut Gpu,
        owner: u128,
        rgba: &[u8],
    ) -> Result<((u32, u32), Option<u128>), String> {
        let want = (CAPE_SLOT_W * CAPE_SLOT_H * 4) as usize;
        if rgba.len() != want {
            return Err(format!(
                "cape must be {CAPE_SLOT_W}x{CAPE_SLOT_H} RGBA ({want} bytes), got {}",
                rgba.len()
            ));
        }
        let (slot, evicted) = self
            .cape_lru
            .claim(owner)
            .ok_or("every cape slot is drawn this frame")?;
        let (sx, sy) = cape_slot_origin(slot);
        upload_region(gpu, self.image, rgba, sx, sy, CAPE_SLOT_W, CAPE_SLOT_H)?;
        Ok(((sx, sy), evicted))
    }

    /// Facelabel mode only: kinds whose textures paint conflicting face
    /// labels onto shared texels (vanilla region reuse) — the color check
    /// cannot apply to them.
    pub fn debug_ambiguous_kinds(&self) -> &[EntityModelKind] {
        &self.debug_ambiguous
    }

    /// The per-texture-slot UV offsets a pack variant applies to a mob — what
    /// `rewo mobshot --etf-check` asserts against, since a variant that shifts
    /// the wrong slot is invisible in a silhouette.
    pub fn variant_offsets(&self, kind: EntityModelKind, variant: u16) -> Option<&[[f32; 2]]> {
        self.models[kind.index()]
            .as_ref()?
            .variants
            .get(&variant)
            .map(|v| v.as_slice())
    }

    /// Kinds with a built model (all textures were present).
    pub fn available_kinds(&self) -> Vec<EntityModelKind> {
        EntityModelKind::ALL
            .iter()
            .copied()
            .filter(|k| self.models[k.index()].is_some())
            .collect()
    }

    /// The rest-pose (yaw 0, origin, no walk/look, time 0) world-space
    /// quads of a mob, with each quad's vanilla face label and world normal
    /// — the geometric ground truth `rewo mobshot --check` compares renders
    /// against. Runs the SAME `part_transforms` as `emit_model` (with the
    /// same zeroed inputs the check renders with), so the prediction can
    /// never disagree with the renderer's math.
    pub fn neutral_quads(
        &self,
        kind: EntityModelKind,
    ) -> Option<Vec<([[f32; 3]; 4], Facing, [f32; 3])>> {
        let model = self.models[kind.index()].as_ref()?;
        let s = model.scale;
        let ctx = AnimCtx {
            pitch: 0.0,
            net: 0.0,
            f: 0.0,
            pos: 0.0,
            amt: 0.0,
            age: 0.0,
            gesture: Option::None,
            events: [None; mobs::ModelEvent::COUNT],
            shell: false,
            body_yaw: 0.0,
            stand_pose: mobs::STAND_DEFAULT_POSE,
            allay_dance: Option::None,
            attack: mobs::SwingPose::NONE,
            arm_poses: mobs::ArmPoses::EMPTY,
            mob: mobs::MobCombat::default(),
            tendril: 0.0,
        };
        let xf = part_transforms(model, &ctx, None, None);
        Some(
            model
                .quads
                .iter()
                // Rest pose: no shell, no gesture — shell-only and
                // gesture-only parts don't render, so exclude them from the
                // geometric prediction too.
                .filter(|q| {
                    matches!(
                        model.parts[q.part as usize].show,
                        mobs::Show::Always
                            | mobs::Show::NotShell
                            // The rest pose is `IllagerArmPose::CROSSED`
                            // (`AbstractIllager.getArmPose`'s base case), so
                            // the folded arms render and the loose pair does
                            // not — matching what `set_entities` draws.
                            | mobs::Show::IllagerCrossedOnly
                            // The default armour stand: plate, no arms.
                            | mobs::Show::StandBasePlate
                    )
                })
                .map(|q| {
                    let (m, o) = &xf[q.part as usize];
                    let mut pos = [[0f32; 3]; 4];
                    for (i, c) in q.pos.iter().enumerate() {
                        let r = mat_apply(m, *c);
                        let v = [r[0] + o[0], r[1] + o[1], r[2] + o[2]];
                        // model → entity local, then the yaw-0 rotY(180°).
                        let e = [-v[0], mobs::MODEL_EYE_Y - v[1], v[2]];
                        pos[i] = [-e[0] * s, e[1] * s, -e[2] * s];
                    }
                    // Normals ride the same rotations (no translate).
                    let n = mat_apply(m, q.normal);
                    (pos, q.facing, [n[0], -n[1], -n[2]])
                })
                .collect(),
        )
    }

    /// Rebuild this frame's vertex soup. `cam_right`/`cam_up` orient the
    /// nametag billboards (world-space unit vectors from the camera);
    /// `time` (seconds) drives the ambient animations (wing flutter, rod
    /// orbits, tentacle sway — vanilla's `ageInTicks` = `time · 20`).
    pub fn set_draws(
        &mut self,
        draws: &[EntityDraw<'_>],
        block_entities: &[BlockEntityDraw<'_>],
        frame_maps: &[FrameMapDraw<'_>],
        world_text: &[WorldTextDraw<'_>],
        cam_right: [f32; 3],
        cam_up: [f32; 3],
        time: f32,
        cam_pos: [f32; 3],
    ) {
        self.cam_pos = cam_pos;
        let mut verts: Vec<Vertex> = Vec::with_capacity(1024);
        // The glint's own range, built alongside the geometry it sits on and
        // appended after the nametags — see [`GlintSink`].
        let mut glint_verts: Vec<Vertex> = Vec::new();
        // M48: the trim's own range. It depth-tests EQUAL against the armour
        // it decorates, so it cannot share the solid range's GREATER test.
        let mut trim_verts: Vec<Vertex> = Vec::new();
        // M50: the worn-armour foil. A separate range from the item glint
        // above because it samples a different sheet, which is a descriptor
        // change and therefore a separate draw.
        let mut armor_glint_verts: Vec<Vertex> = Vec::new();
        // M57: the vanilla emissive layers. Their pipeline is
        // `GREATER_OR_EQUAL` with no depth write, so like the trim they cannot
        // share the solid range's strict `GREATER`.
        let mut emissive_verts: Vec<Vertex> = Vec::new();
        // M285: the charged creeper's swirl, additive in gamma space.
        let mut swirl_verts: [Vec<Vertex>; 2] = [Vec::new(), Vec::new()];
        // M286: the breeze's wind, translucent in gamma space.
        let mut wind_verts: Vec<Vertex> = Vec::new();
        // M306: `entityTranslucent` model layers (the slime's shell).
        let mut translucent_verts: Vec<Vertex> = Vec::new();
        // M363 — the boats' water patches, depth only.
        let mut water_mask_verts: Vec<Vertex> = Vec::new();
        let glint_offsets =
            crate::gui_item::glint_offsets((time as f64) * 1000.0, crate::gui_item::GLINT_SPEED);

        // Advance the animation clock. `frame_time` drives FA's integrators and
        // `frame_counter` its same-frame guard, so both must be real. `time` is
        // wall-clock seconds; a still render repeats the same value, hence the
        // fallback tick.
        let dt = if self.prev_time.is_finite() {
            time - self.prev_time
        } else {
            0.0
        };
        self.prev_time = time;
        self.frame_dt = if dt > 0.0 { dt.min(0.25) } else { 1.0 / 20.0 };
        self.frame_counter += 1.0;
        self.generation = self.generation.wrapping_add(1);
        // Mark the skin and cape slots this frame draws, so an upload before
        // the next frame cannot evict a player who is on screen.
        self.skin_lru.begin_frame();
        self.cape_lru.begin_frame();
        for d in draws {
            if let Some(slot) = d
                .skin_uv
                .and_then(|uv| skin_slot_of_uv(self.player_origin, uv))
            {
                self.skin_lru.touch(slot);
            }
            if let Some(slot) = d
                .cape
                .as_ref()
                .and_then(|c| cape_slot_at(c.origin.0, c.origin.1))
            {
                self.cape_lru.touch(slot);
            }
        }
        // Taken out of `self` so `emit_model` can borrow it mutably alongside
        // the immutable model borrow; put back below.
        let mut cem_state = std::mem::take(&mut self.cem_state);

        // Fixed sun for capsule shading (matches the terrain's lit look).
        let sun = norm3([0.45, 0.8, 0.35]);
        // M278 — every vertex a glowing entity emits (model, armour, held
        // items: `OutlineBufferSource` wraps all of its submits), copied in its
        // outline colour. A draw's vertices are contiguous in `verts`, so the
        // range is closed when the next draw starts.
        let mut outline_verts: Vec<Vertex> = Vec::new();
        let mut outline_open: Option<(usize, [f32; 3])> = None;
        let close_outline =
            |verts: &[Vertex], open: &mut Option<(usize, [f32; 3])>, out: &mut Vec<Vertex>| {
                if let Some((start, c)) = open.take() {
                    out.extend(verts[start..].iter().map(|v| Vertex {
                        color: [c[0], c[1], c[2], 1.0],
                        ..*v
                    }));
                }
            };
        for d in draws {
            close_outline(&verts, &mut outline_open, &mut outline_verts);
            outline_open = d.outline.map(|c| (verts.len(), c));
            // `OverlayTexture.v(hasRedOverlay)` as a 0/1 flag (M21).
            let hurt = overlay_value(d.hurt, d.white_overlay);
            // A dropped stack is drawn by `ItemEntityRenderer` and has no
            // model or capsule of its own — the item IS the entity (M24b).
            if let Some(name) = d.ground_item {
                let mut sink = GlintSink {
                    verts: &mut glint_verts,
                    offsets: glint_offsets,
                    scale: crate::gui_item::GLINT_SCALE_ENTITY,
                };
                self.emit_ground_item(
                    &mut verts,
                    d,
                    name,
                    time,
                    hurt,
                    d.ground_glint.then_some(&mut sink),
                );
                continue;
            }
            if let Some(model) = &self.models[d.kind.index()] {
                let mut sink = GlintSink {
                    verts: &mut glint_verts,
                    offsets: glint_offsets,
                    scale: crate::gui_item::GLINT_SCALE_ENTITY,
                };
                // The armour foil's own sink: same scrolling clock, different
                // sheet and a sixteenth-ish of the scale (0.16 against 0.5).
                let mut armor_sink = GlintSink {
                    verts: &mut armor_glint_verts,
                    offsets: glint_offsets,
                    scale: crate::gui_item::GLINT_SCALE_ARMOR,
                };
                let t0 = translucent_verts.len();
                self.emit_model(
                    &mut verts,
                    &mut trim_verts,
                    &mut emissive_verts,
                    &mut swirl_verts,
                    &mut wind_verts,
                    &mut translucent_verts,
                    &mut water_mask_verts,
                    d,
                    model,
                    time,
                    &mut cem_state,
                    Some(&mut sink),
                    &mut armor_sink,
                );
                // M306 — a translucent layer affects the outline too
                // (`entityTranslucent(texture)` is `AFFECTS_OUTLINE`).
                if let Some(c) = d.outline {
                    outline_verts.extend(translucent_verts[t0..].iter().map(|v| Vertex {
                        color: [c[0], c[1], c[2], 1.0],
                        ..*v
                    }));
                }
                // M279 — an invisible body that glows draws as its outline
                // alone (`getRenderType`'s `appearsGlowing` arm,
                // `RenderTypes.outline`): the model again with the body shown
                // and no layers (they drew above), into scratch buffers, kept
                // only as outline copies.
                if let (true, Some(c)) = (d.body_hidden, d.outline) {
                    let ghost = EntityDraw {
                        body_hidden: false,
                        armor: [None; 4],
                        held: [None, None],
                        worn_head: None,
                        outline: None,
                        ..*d
                    };
                    let (mut gv, mut gt, mut ge, mut gg, mut ga, mut gtr) = (
                        Vec::new(),
                        Vec::new(),
                        Vec::new(),
                        Vec::new(),
                        Vec::new(),
                        Vec::new(),
                    );
                    let mut gsink = GlintSink {
                        verts: &mut gg,
                        offsets: glint_offsets,
                        scale: crate::gui_item::GLINT_SCALE_ENTITY,
                    };
                    let mut gasink = GlintSink {
                        verts: &mut ga,
                        offsets: glint_offsets,
                        scale: crate::gui_item::GLINT_SCALE_ARMOR,
                    };
                    self.emit_model(
                        &mut gv,
                        &mut gt,
                        &mut ge,
                        &mut [Vec::new(), Vec::new()],
                        &mut Vec::new(),
                        &mut gtr,
                        &mut Vec::new(),
                        &ghost,
                        model,
                        time,
                        &mut cem_state,
                        Some(&mut gsink),
                        &mut gasink,
                    );
                    outline_verts.extend(gv.iter().chain(&gtr).map(|v| Vertex {
                        color: [c[0], c[1], c[2], 1.0],
                        ..*v
                    }));
                }
                continue;
            }
            let base = d.color;
            let [light_r, light_g, light_b] = d.light;
            // The death topple is a `LivingEntityRenderer` rotation, not a
            // model one, so it applies to the capsule fallback exactly as it
            // does to a real model. The capsule is built in entity-local
            // metres with y in 0..1 and xz in ±0.5, so the roll is taken about
            // the feet — which is where `setupRotations` rotates, the whole
            // model hanging off the entity origin (M24).
            let (sr, cr) = death_roll(d).sin_cos();
            // M271 — an invisible body draws no capsule either.
            let capsule: &[([f32; 3], [f32; 3])] = if d.body_hidden { &[] } else { &self.capsule };
            for (p, n) in capsule {
                if verts.len() >= MAX_VERTS {
                    break;
                }
                let shade = 0.55 + 0.45 * (n[0] * sun[0] + n[1] * sun[1] + n[2] * sun[2]).max(0.0);
                // Scale to metres first, so the rotation is rigid rather than
                // shearing a non-cubic capsule.
                let (lx, ly, lz) = (p[0] * d.width, p[1] * d.height, p[2] * d.width);
                let (lx, ly) = (lx * cr - ly * sr, lx * sr + ly * cr);
                verts.push(Vertex {
                    pos: [d.pos[0] + lx, d.pos[1] + ly, d.pos[2] + lz],
                    uv: self.white_uv,
                    color: gamma_vertex(shade, linear_to_srgb3(base)),
                    light_hurt: [light_r, light_g, light_b, hurt],
                });
            }
        }
        close_outline(&verts, &mut outline_open, &mut outline_verts);
        // M281 — the flames, after every entity and outside the outline copy:
        // `submitFlame` is its own feature, not one `OutlineBufferSource`
        // wraps.
        for d in draws {
            self.emit_flame(&mut verts, d, cam_right);
        }
        // M25b: block entities. Solid geometry, so they belong with the mob
        // and item quads rather than after the transparent nametags.
        {
            // M198: a framed foil item's glint rides the item glint's range.
            let mut sink = GlintSink {
                verts: &mut glint_verts,
                offsets: glint_offsets,
                scale: crate::gui_item::GLINT_SCALE_ENTITY,
            };
            self.be_quads = self.emit_block_entities(&mut verts, block_entities, &mut sink);
        }
        // M194: maps in item frames, beside the frames' own models.
        self.emit_frame_maps(&mut verts, frame_maps);
        // M25e: world-space text (sign faces). Solid glyph quads, so they go
        // with the rest of the opaque geometry rather than after the tags.
        self.emit_world_text(&mut verts, world_text);
        // Drop state for entities that have been gone a while (despawns).
        let gen = self.generation;
        cem_state.retain(|_, v| gen.wrapping_sub(v.seen) < CEM_STATE_TTL);
        self.cem_state = cem_state;
        let solid = verts.len();

        if self.has_font {
            for d in draws {
                // M59: the health bar first, so a tag and a bar submitted for
                // the same entity land adjacently in the buffer.
                self.push_health_bar(&mut verts, d, cam_right, cam_up);
                // M410 — `submitNameDisplay`: the score line at the
                // attachment, then the name `9·1.15·0.025` blocks up.
                let lift = match d.score_text {
                    Some(score) => {
                        self.push_tag(&mut verts, d, score, cam_right, cam_up, 0.0);
                        SCORE_LINE_LIFT
                    }
                    Option::None => 0.0,
                };
                let Some(name) = d.name else { continue };
                self.push_tag(&mut verts, d, name, cam_right, cam_up, lift);
            }
            // M262: world-text plates (text display backgrounds), translucent.
            self.emit_world_text_backdrops(&mut verts, world_text);
            // M204: framed maps' banner names — translucent backgrounds, so
            // they ride the text range too.
            for m in frame_maps {
                for label in m.labels {
                    self.push_map_label(&mut verts, m, label);
                }
            }
        }
        let total = verts.len();
        if total >= MAX_VERTS {
            log::warn!("entities: vertex budget hit — some entities/tags dropped");
        }

        // Six ranges in one buffer:
        //     solid | text | glint | trim | armor_glint | emissive
        // The order here is storage, not draw order — `draw_armor_glint` runs
        // before `draw_trim` because vanilla submits the foil under the trim,
        // and `draw_emissive` runs right after `draw_solid`, which is where
        // vanilla's `order(1)` layer submits land. M57 appends its range at the
        // *end* deliberately: every offset above it is then unchanged, so the
        // five existing ranges keep their exact first-vertex arithmetic.
        let text_end = verts.len();
        verts.append(&mut glint_verts);
        let glint_end = verts.len();
        verts.append(&mut trim_verts);
        let trim_end = verts.len();
        verts.append(&mut armor_glint_verts);
        let armor_glint_end = verts.len();
        verts.append(&mut emissive_verts);
        let emissive_end = verts.len();
        verts.append(&mut outline_verts);
        let outline_end = verts.len();
        // M285 — `energySwirl`'s `sortOnUpload`: far quads first, so the
        // shell's back faces add before its front ones pass the depth test.
        // M287 — one render type per sheet, each sorted on its own.
        let [mut creeper_swirl, mut wither_swirl] = swirl_verts;
        sort_quads_back_to_front(&mut creeper_swirl, cam_pos);
        sort_quads_back_to_front(&mut wither_swirl, cam_pos);
        let split = creeper_swirl.len() as u32;
        verts.append(&mut creeper_swirl);
        verts.append(&mut wither_swirl);
        let swirl_end = verts.len();
        // M286 — `breezeWind`'s `sortOnUpload`, likewise.
        sort_quads_back_to_front(&mut wind_verts, cam_pos);
        verts.append(&mut wind_verts);
        let wind_end = verts.len();
        // M306 — `entityTranslucent`'s `sortOnUpload`, likewise.
        sort_quads_back_to_front(&mut translucent_verts, cam_pos);
        verts.append(&mut translucent_verts);
        let translucent_end = verts.len();
        // M363 — the water patches, last.
        verts.append(&mut water_mask_verts);
        let counts = [
            solid,
            text_end - solid,
            glint_end - text_end,
            trim_end - glint_end,
            armor_glint_end - trim_end,
            emissive_end - armor_glint_end,
            outline_end - emissive_end,
            swirl_end - outline_end,
            wind_end - swirl_end,
            translucent_end - wind_end,
            verts.len() - translucent_end,
        ]
        .map(|c| c as u32);
        // The per-emitter `MAX_VERTS` guards bound each range on its own, but
        // five ranges are appended after the solid one, so their sum can
        // exceed the ring slot. The slot keeps a prefix; every range count is
        // clamped to that same prefix so no draw reads past what was uploaded.
        let bytes: &[u8] = unsafe {
            std::slice::from_raw_parts(
                verts.as_ptr() as *const u8,
                verts.len() * VERTEX_STRIDE as usize,
            )
        };
        let kept = self.ring.write_fixed(bytes, VERTEX_STRIDE as usize) / VERTEX_STRIDE as usize;
        let [s0, s1, s2, s3, s4, s5, s6, s7, s8, s9, s10] = clamp_ranges(counts, kept as u32);
        self.solid_verts = s0;
        self.text_verts = s1;
        self.glint_verts = s2;
        self.trim_verts = s3;
        self.armor_glint_verts = s4;
        self.emissive_verts = s5;
        self.outline_verts = s6;
        self.swirl_verts = s7;
        self.swirl_split = split.min(s7);
        self.wind_verts = s8;
        self.translucent_verts = s9;
        self.water_mask_verts = s10;
    }

    // Draw one mob model: per-part vanilla `setupAnim` rotation about the
    // part's pivot in model space, then vanilla's entity transform —
    // `rotY(180° − yaw) · scale(−1,−1,1) · translate(0,−1.501,0)` — scaled
    // px→blocks and placed at the entity's feet. Texels ride the
    // alpha-test (`discard`) path.

    /// `ItemInHandLayer.submitArmWithItem` for one arm (M22).
    ///
    /// The transform chain, as vanilla writes it:
    ///
    /// ```text
    /// translateToHand(arm)            // root then arm — the arm part matrix
    /// mulPose(XP.rotationDegrees(-90))
    /// mulPose(YP.rotationDegrees(180))
    /// translate(+/-1/16, 2/16, -10/16)
    /// <ItemTransform.apply>           // centre, scale, rotate, translate
    /// ```
    ///
    /// A `PoseStack` transforms the coordinate system, so a *point* runs the
    /// chain in reverse call order: the display transform first, the arm matrix
    /// last. The item model is in 0..1 block units at that point, so it is
    /// scaled back to model px before entering the arm matrix, which
    /// `part_transforms` expresses in px.
    #[allow(clippy::too_many_arguments)]
    fn emit_held_item(
        &self,
        verts: &mut Vec<Vertex>,
        d: &EntityDraw<'_>,
        model: &MobModel,
        xf: &[([[f32; 3]; 3], [f32; 3])],
        left: bool,
        name: &str,
        frame: &EntityFrame,
        hurt: f32,
        glint: Option<&mut GlintSink<'_>>,
    ) {
        let Some(items) = self.held_items.as_ref() else {
            return;
        };
        let Some(item) = items.any(name) else {
            return;
        };
        let mut glint = glint;
        // The arm this hangs off. A model with no such part is not a humanoid,
        // so it simply holds nothing — which is what vanilla's `ArmedModel`
        // bound expresses at the type level.
        let arm_name = if left { "left_arm" } else { "right_arm" };
        let Some(arm) = model.parts.iter().position(|p| p.name == arm_name) else {
            return;
        };
        let (m, o) = &xf[arm];
        let transform = if left { &item.left } else { &item.right };
        let offset = crate::held::hand_offset(left, d.mob.is_baby);
        let [light_r, light_g, light_b] = d.light;

        for q in &item.quads {
            if verts.len() + 6 > MAX_VERTS {
                return;
            }
            let Some([u0, v0, du, dv]) = self.item_uv(q.tex) else {
                continue; // texture not resident — draw nothing, never garbage
            };
            let mut p4 = [[0f32; 3]; 4];
            // Model-space corners kept alongside, so the face normal can be
            // taken AFTER the display + hand rotations rather than from the
            // quad's pre-rotation `dir` — an item is turned on its side in the
            // hand, so the baked direction is not the direction it faces.
            let mut m4 = [[0f32; 3]; 4];
            for (i, corner) in q.verts.iter().enumerate() {
                // model units 0..16 -> block units 0..1
                let p = [corner[0] / 16.0, corner[1] / 16.0, corner[2] / 16.0];
                let p = crate::held::apply_display(transform, left, p);
                let p = [p[0] + offset[0], p[1] + offset[1], p[2] + offset[2]];
                // YP(180) then XP(-90), in that order.
                let p = crate::mobs::rotate_zyx(p, [0.0, std::f32::consts::PI, 0.0]);
                let p = crate::mobs::rotate_zyx(p, [-std::f32::consts::FRAC_PI_2, 0.0, 0.0]);
                // block units -> model px, then through the arm matrix.
                let p = [p[0] * 16.0, p[1] * 16.0, p[2] * 16.0];
                let r = mat_apply(m, p);
                let v = [r[0] + o[0], r[1] + o[1], r[2] + o[2]];
                m4[i] = v;
                // The same model -> entity-local -> world chain the mob quads
                // take, so an item cannot drift from the arm holding it.
                // M402 — the body's own frame (`EntityFrame`).
                p4[i] = frame.place(v);
            }
            let n = face_normal(&m4);
            let shade = mobs::mix_light_flipped(n, self.nether_light);
            let color = gamma_vertex(shade, [1.0; 3]);
            for &i in &[0usize, 1, 2, 0, 2, 3] {
                verts.push(Vertex {
                    pos: p4[i],
                    uv: [u0 + q.uv[i][0] * du, v0 + q.uv[i][1] * dv],
                    color,
                    light_hurt: [light_r, light_g, light_b, hurt],
                });
                // Same position, same moment — see [`GlintSink`].
                if let Some(g) = glint.as_deref_mut() {
                    g.push(p4[i], q.uv[i]);
                }
            }
        }
    }

    /// M273 — a worn skull, `CustomHeadLayer.submit`'s skull branch:
    ///
    /// ```text
    /// scale(h, 1, h); root.translateAndRotate; translateToHead (the head
    /// part); translate(0, skullYOffset, 0); scale(1.1875); submitSkull
    /// ```
    ///
    /// The skull model is a mob model, y-down, authored at the head part's
    /// origin (its head cube spans `-8..0` in y), so its px corners are
    /// head-local px as they stand. Vanilla draws the layer for an
    /// invisible body too (M271). Drawn at rest: the piglin's ears and the
    /// dragon's jaw take `wornHeadAnimationPos`, which Rewo leaves at 0.
    fn emit_worn_head(
        &self,
        verts: &mut Vec<Vertex>,
        d: &EntityDraw<'_>,
        model: &MobModel,
        xf: &[([[f32; 3]; 3], [f32; 3])],
        frame: &EntityFrame,
        hurt: f32,
        // M420 — the item glint, for a foil stack worn as an item.
        glint: Option<&mut GlintSink<'_>>,
    ) {
        let Some(w) = d.worn_head else {
            return;
        };
        let mut glint = glint.filter(|_| w.item && w.foil);
        let Some(skull) = self.held_items.as_ref().and_then(|h| h.any(w.model)) else {
            return;
        };
        let Some(head) = model.parts.iter().position(|p| p.name == "head") else {
            return;
        };
        let (m, o) = &xf[head];
        let [light_r, light_g, light_b] = d.light;
        // Head-local px of a model corner (px, `0..16` for an item).
        let local = |c: &[f32; 3]| -> [f32; 3] {
            if w.item {
                // M274 — `translateToHead`: translate(0, -0.25 + yOffset, 0),
                // YP(180), scale(0.625, -0.625, -0.625); inside it the item's
                // `head` display transform and its -0.5 centring.
                let p = crate::held::apply_display(
                    &skull.head,
                    false,
                    [c[0] / 16.0, c[1] / 16.0, c[2] / 16.0],
                );
                [
                    -WORN_ITEM_SCALE * p[0] * 16.0,
                    (-WORN_ITEM_SCALE * p[1] - 0.25 + w.y_offset) * 16.0,
                    WORN_ITEM_SCALE * p[2] * 16.0,
                ]
            } else {
                [
                    c[0] * WORN_SKULL_SCALE,
                    c[1] * WORN_SKULL_SCALE + w.skull_y_offset * 16.0,
                    c[2] * WORN_SKULL_SCALE,
                ]
            }
        };
        for q in &skull.quads {
            if verts.len() + 6 > MAX_VERTS {
                return;
            }
            let Some([u0, v0, du, dv]) = self.item_uv(q.tex) else {
                continue;
            };
            let mut p4 = [[0f32; 3]; 4];
            let mut m4 = [[0f32; 3]; 4];
            for (i, c) in q.verts.iter().enumerate() {
                let r = mat_apply(m, local(c));
                let v = [
                    (r[0] + o[0]) * w.horizontal_scale,
                    r[1] + o[1],
                    (r[2] + o[2]) * w.horizontal_scale,
                ];
                m4[i] = v;
                // M402 — the body's own frame (`EntityFrame`).
                p4[i] = frame.place(v);
            }
            let shade = mobs::mix_light_flipped(face_normal(&m4), self.nether_light);
            let color = gamma_vertex(shade, [1.0; 3]);
            for &i in &[0usize, 1, 2, 0, 2, 3] {
                verts.push(Vertex {
                    pos: p4[i],
                    uv: [u0 + q.uv[i][0] * du, v0 + q.uv[i][1] * dv],
                    color,
                    light_hurt: [light_r, light_g, light_b, hurt],
                });
                // Same position, same moment — see [`GlintSink`].
                if let Some(g) = glint.as_deref_mut() {
                    g.push(p4[i], q.uv[i]);
                }
            }
        }
    }

    /// M281 — `FlameFeatureRenderer.prepare`: layers of `fire_0` and
    /// `fire_1` (alternating, every other pair mirrored), each 0.9 times the
    /// last one's width, 0.45 apart and 0.03 further back, scaled by
    /// `width · 1.4`, turned about Y to the camera and pushed
    /// `0.3 - (int)h · 0.02` toward it, until the entity's height is covered.
    fn emit_flame(&self, verts: &mut Vec<Vertex>, d: &EntityDraw<'_>, cam_right: [f32; 3]) {
        let Some(fire) = d.on_fire else {
            return;
        };
        let light = fire.light;
        let Some(items) = self.held_items.as_ref() else {
            return;
        };
        let uv = |sprite: usize| {
            let name = fire_model_name(sprite, fire.frames[sprite]);
            self.item_uv(items.any(&name)?.quads.first()?.tex)
        };
        let (Some(fire1), Some(fire2)) = (uv(0), uv(1)) else {
            return;
        };
        // The camera's yaw alone (`rotationAroundAxis(Y, camera.orientation)`):
        // its horizontal right, and the horizontal direction back toward it.
        let rl = (cam_right[0] * cam_right[0] + cam_right[2] * cam_right[2]).sqrt();
        if rl < 1.0e-6 {
            return;
        }
        let r = [cam_right[0] / rl, 0.0, cam_right[2] / rl];
        let back = [-r[2], 0.0, r[0]];
        let s = d.width * 1.4;
        if s <= 0.0 {
            return;
        }
        let mut h = d.height / s;
        let z0 = 0.3 - (h as i32) as f32 * 0.02;
        let (mut half, mut yo, mut zo, mut ss) = (0.5f32, 0.0f32, 0.0f32, 0u32);
        let place = |x: f32, y: f32, z: f32| {
            [
                d.pos[0] + s * (x * r[0] + z * back[0]),
                d.pos[1] + s * y,
                d.pos[2] + s * (x * r[2] + z * back[2]),
            ]
        };
        while h > 0.0 {
            if verts.len() + 6 > MAX_VERTS {
                return;
            }
            let [u0, v0, du, dv] = if ss % 2 == 0 { fire1 } else { fire2 };
            let (mut ua, mut ub) = (u0 + du, u0);
            if (ss / 2) % 2 == 0 {
                std::mem::swap(&mut ua, &mut ub);
            }
            let (va, vb) = (v0 + dv, v0);
            let z = z0 + zo;
            let quad = [
                (place(-half, -yo, z), [ua, va]),
                (place(half, -yo, z), [ub, va]),
                (place(half, 1.4 - yo, z), [ub, vb]),
                (place(-half, 1.4 - yo, z), [ua, vb]),
            ];
            for &i in &[0usize, 1, 2, 0, 2, 3] {
                verts.push(Vertex {
                    pos: quad[i].0,
                    uv: quad[i].1,
                    color: [1.0, 1.0, 1.0, 1.0],
                    light_hurt: [light[0], light[1], light[2], 0.0],
                });
            }
            h -= 0.45;
            yo -= 0.45;
            half *= 0.9;
            zo -= 0.03;
            ss += 1;
        }
    }

    /// The worn cape, hanging off the body (M60).
    ///
    /// `CapeLayer` is a render layer over the same `body` the torso used, so
    /// like [`Self::emit_armor`] this takes the `xf` that was just built
    /// rather than deriving the pose again — and the body is *animated*
    /// (`HumanoidModel` sets `body.xRot = 0.5` crouching and `body.yRot`
    /// during an attack swing), so a second derivation would visibly drift.
    ///
    /// # The cape is not a `Part`
    ///
    /// It could not be. Rewo's [`Part`](mobs::Part) stores a Euler triple and
    /// composes `Rz·Ry·Rx`; the cape needs `Rx·Rz·Ry`
    /// ([`cape_rotation`]), which that composition cannot express — and
    /// teaching `part_transforms` a matrix override would put a new branch in
    /// the path every mob's geometry runs through, for one quad that vanilla
    /// itself keeps in a separate model (`PlayerCapeModel`, whose
    /// `createCapeLayer` calls `clearRecursively()` precisely so the humanoid
    /// mesh does not come along). Emitting it here instead means
    /// `part_transforms`, `neutral_quads` and `oracle_part_deltas` are all
    /// untouched, so `mobshot`'s geometric prediction still grades the same
    /// code it did before.
    ///
    /// The child transform is the one `part_transforms` would have applied:
    /// `m = m_body · R`, `o = m_body · pivot + o_body`.
    #[allow(clippy::too_many_arguments)]
    fn emit_cape(
        &self,
        verts: &mut Vec<Vertex>,
        d: &EntityDraw<'_>,
        model: &MobModel,
        xf: &[([[f32; 3]; 3], [f32; 3])],
        scale: f32,
        st: f32,
        ct: f32,
        sr: f32,
        cr: f32,
        frame: &EntityFrame,
    ) {
        let Some(cape) = d.cape else { return };
        if !mobs::wears_cape(d.kind) {
            return;
        }
        let Some(bi) = mobs::armor_part(&model.parts, "body") else {
            return;
        };
        let (m, o) = cape_transform(&xf[bi], &cape);
        let shift = cape_clearance_shift(cape.chest_humanoid);
        // M61. The reduction rule is *this branch*: at one segment the
        // simulation contributes nothing and the code below runs unchanged,
        // so the vertices are the vanilla cape's bit-for-bit rather than
        // within a tolerance of them. Stiffening a one-link chain could not
        // have produced that — a rigid two-joint chain is a pendulum and
        // hangs straight down, where the vanilla cape sits at
        // `Rx(6 + capeLean/2 + capeFlap)`.
        //
        // M413 — and only on an upright body: the chain simulates in
        // world-aligned cape space, which knows the death roll but not a
        // glide, swim, spin, sleep or pitch. A body in any of those wears
        // the vanilla cape, which places through the frame like every layer.
        if let Some(w) = cape.wavy.filter(|w| w.segments() >= 2 && frame.upright()) {
            self.emit_wavy_cape(verts, d, &cape, &w, m, o, shift, scale, st, ct, sr, cr);
            return;
        }
        let [light_r, light_g, light_b] = d.light;
        for (_facing, pos, uvs) in mobs::cape_faces() {
            if verts.len() + 6 > MAX_VERTS {
                return;
            }
            let mut p4 = [[0f32; 3]; 4];
            for (i, corner) in pos.iter().enumerate() {
                let rr = mat_apply(&m, *corner);
                let v = [
                    rr[0] + o[0] + shift[0],
                    rr[1] + o[1] + shift[1],
                    rr[2] + o[2] + shift[2],
                ];
                // M402 — the body's own frame (`EntityFrame`).
                p4[i] = frame.place(v);
            }
            let n = face_normal(&p4);
            let shade = mobs::mix_light_flipped(n, self.nether_light);
            let color = gamma_vertex(shade, [1.0; 3]);
            let uv4 = cape_face_uv(cape.origin, &uvs);
            for &i in &[0usize, 1, 2, 0, 2, 3] {
                verts.push(Vertex {
                    pos: p4[i],
                    uv: uv4[i],
                    color,
                    // `CapeLayer` submits with `OverlayTexture.NO_OVERLAY`, so
                    // the cape does **not** take the red damage flash even
                    // while the wearer does. Hard-zeroed rather than read off
                    // `d.hurt` for that reason.
                    light_hurt: [light_r, light_g, light_b, 0.0],
                });
            }
        }
    }

    /// The wavy cape's `N` slabs, hung off the simulated spine (M61).
    ///
    /// Everything vanilla about the cape has already happened by the time
    /// this runs: `m` and `o` are the rotation and offset
    /// [`cape_transform`] produced from the three already-gated angles, and
    /// `shift` is `CapeLayer`'s clearance translate. This routine only
    /// replaces the rigid slab's *shape*.
    ///
    /// # Re-pinning
    ///
    /// The simulation runs in cape space with an anchor it derives from
    /// entity state alone — it cannot see the animated body transform, the
    /// clearance shift or the death roll. So joint 0 is moved onto the true
    /// attachment point here and the rest of the chain is translated
    /// rigidly with it. A rigid translation cannot distort cloth, and it
    /// makes "joint 0 is the vanilla attachment point" true of the rendered
    /// geometry and not only of the simulation.
    ///
    /// # Frames are per joint, not per slab
    ///
    /// Each joint carries one width/thickness frame, built by rotating the
    /// cape's rest frame the shortest way onto the joint's own tangent.
    /// Consecutive slabs therefore share their boundary quad *exactly*,
    /// which is what makes the surface watertight without internal caps —
    /// see [`mobs::cape_slab_quads`]. Per-slab frames would open a slit at
    /// every joint the moment the chain bent.
    #[allow(clippy::too_many_arguments)]
    fn emit_wavy_cape(
        &self,
        verts: &mut Vec<Vertex>,
        d: &EntityDraw<'_>,
        cape: &CapeDraw,
        w: &CapeJoints,
        m: [[f32; 3]; 3],
        o: [f32; 3],
        shift: [f32; 3],
        scale: f32,
        st: f32,
        ct: f32,
        sr: f32,
        cr: f32,
    ) {
        let n = w.segments();
        // The cape's own axes, in cape space. `m`'s columns are the images
        // of the model basis, so column 1 is the cape's local "down the
        // spine" — the direction the rest chain lies along.
        let axis = |k: usize| model_dir_to_cape([m[0][k], m[1][k], m[2][k]], st, ct, sr, cr);
        let x0 = axis(0);
        let y0 = axis(1);
        let z0 = axis(2);

        // The true attachment point: the spine's top, in cape space.
        let spine_top = mat_apply(&m, [0.0, 0.0, -0.5]);
        let anchor = model_pos_to_cape(
            [
                spine_top[0] + o[0] + shift[0],
                spine_top[1] + o[1] + shift[1],
                spine_top[2] + o[2] + shift[2],
            ],
            st,
            ct,
            sr,
            cr,
        );

        let src = w.joints();
        let fix = [
            anchor[0] - src[0][0],
            anchor[1] - src[0][1],
            anchor[2] - src[0][2],
        ];
        let mut joint = [[0f32; 3]; CAPE_MAX_JOINTS];
        for (j, p) in src.iter().enumerate() {
            joint[j] = [p[0] + fix[0], p[1] + fix[1], p[2] + fix[2]];
        }

        // Per-joint tangents: the centred difference inside the chain, the
        // one adjacent link at each end.
        let mut right = [[0f32; 3]; CAPE_MAX_JOINTS];
        let mut norm = [[0f32; 3]; CAPE_MAX_JOINTS];
        for j in 0..=n {
            let a = joint[j.saturating_sub(1)];
            let b = joint[(j + 1).min(n)];
            let t = normalize_or([b[0] - a[0], b[1] - a[1], b[2] - a[2]], y0);
            let r = min_rotation(y0, t);
            right[j] = mat_apply(&r, x0);
            norm[j] = mat_apply(&r, z0);
        }

        let [light_r, light_g, light_b] = d.light;
        for q in mobs::cape_slab_quads(n) {
            if verts.len() + 6 > MAX_VERTS {
                return;
            }
            let mut p4 = [[0f32; 3]; 4];
            for i in 0..4 {
                let j = q.joint[i];
                let (dw, dt) = (q.off[i][0], q.off[i][1]);
                // Cape space -> world: the chain is already world-aligned,
                // so only the death roll (about the anchor) and the px->
                // block scale are left.
                let v = [
                    joint[j][0] + right[j][0] * dw + norm[j][0] * dt - anchor[0],
                    joint[j][1] + right[j][1] * dw + norm[j][1] * dt - anchor[1],
                    joint[j][2] + right[j][2] * dw + norm[j][2] * dt - anchor[2],
                ];
                let v = roll_in_cape_space(v, st, ct, sr, cr);
                let mut l = [
                    (anchor[0] + v[0]) * scale,
                    (anchor[1] + v[1]) * scale,
                    (anchor[2] + v[2]) * scale,
                ];
                if let Some(mm) = &d.mount {
                    l = [
                        mm[0][0] * l[0] + mm[0][1] * l[1] + mm[0][2] * l[2] + mm[0][3],
                        mm[1][0] * l[0] + mm[1][1] * l[1] + mm[1][2] * l[2] + mm[1][3],
                        mm[2][0] * l[0] + mm[2][1] * l[1] + mm[2][2] * l[2] + mm[2][3],
                    ];
                }
                p4[i] = [d.pos[0] + l[0], d.pos[1] + l[1], d.pos[2] + l[2]];
            }
            let fnorm = face_normal(&p4);
            let shade = mobs::mix_light_flipped(fnorm, self.nether_light);
            let color = gamma_vertex(shade, [1.0; 3]);
            let uv4 = cape_face_uv(cape.origin, &q.uv);
            for &i in &[0usize, 1, 2, 0, 2, 3] {
                verts.push(Vertex {
                    pos: p4[i],
                    uv: uv4[i],
                    color,
                    // As the rigid path: `CapeLayer` submits with
                    // `NO_OVERLAY`, so a hurt wearer's cape does not flash.
                    light_hurt: [light_r, light_g, light_b, 0.0],
                });
            }
        }
    }

    /// M403 — `WingsLayer.submit`: the elytra, `ElytraModel`'s two wings
    /// (`texOffs(22, 0)`, a 10 × 20 × 2 box grown 1.0, the right one
    /// mirrored) at `(±5, crouching ? 3 : 0, 0)`, turned by the entity's
    /// `elytraRot` (the right wing's `yRot` and `zRot` negated), half size on
    /// a baby, and pushed back `translate(0, 0, 0.125)`; through the body's
    /// own frame, so a glider's wings lie with it. `armorCutoutNoCull`: the
    /// entity pass discards transparent texels and culls nothing. A
    /// glinting elytra rides the armour foil, from the same positions.
    fn emit_wings(
        &self,
        verts: &mut Vec<Vertex>,
        armor_glint: &mut GlintSink<'_>,
        d: &EntityDraw<'_>,
        frame: &EntityFrame,
    ) {
        let Some(w) = d.wings else { return };
        // The renderers that add a `WingsLayer`: avatars, the humanoid
        // mobs (M416: their babies too) and the armour stand.
        if !mobs::wears_humanoid_armor(d.kind) && mobs::baby_armor_set(d.kind).is_none() {
            return;
        }
        let Some((ax, ay, sheet_w, sheet_h)) = (match w.texture {
            WingsTexture::Equipment(key) => self.armor_slots.get(key).copied(),
            WingsTexture::Player((x, y)) => Some((x, y, 64, 32)),
        }) else {
            return;
        };
        let [light_r, light_g, light_b] = d.light;
        let hurt = overlay_value(d.hurt, d.white_overlay);
        // `ELYTRA_BABY` is `ElytraModel.BABY_TRANSFORMER`,
        // `MeshTransformer.scaling(0.5)`: the root pose scaled by `k` and
        // moved `24.016·(1 − k)` px down (M416; M403 left out the move).
        let k = if d.mob.is_baby { 0.5 } else { 1.0 };
        let drop = 24.016 * (1.0 - k);
        let y = if w.crouching { 3.0 } else { 0.0 };
        let [rx, ry, rz] = w.rot;
        for (pivot, rot, min, mirror) in [
            ([5.0, y, 0.0], [rx, ry, rz], [-10.0, 0.0, 0.0], false),
            ([-5.0, y, 0.0], [rx, -ry, -rz], [0.0, 0.0, 0.0], true),
        ] {
            for (_facing, pos, uvs) in
                mobs::cube_faces((22.0, 0.0), min, [10.0, 20.0, 2.0], 1.0, mirror)
            {
                if verts.len() + 6 > MAX_VERTS {
                    return;
                }
                let p4: [[f32; 3]; 4] = std::array::from_fn(|i| {
                    let r = mobs::rotate_zyx(pos[i], rot);
                    frame.place([
                        (r[0] + pivot[0]) * k,
                        (r[1] + pivot[1]) * k + drop,
                        (r[2] + pivot[2]) * k + 2.0,
                    ])
                });
                let shade = mobs::mix_light_flipped(face_normal(&p4), self.nether_light);
                let color = gamma_vertex(shade, [1.0; 3]);
                for &i in &[0usize, 1, 2, 0, 2, 3] {
                    verts.push(Vertex {
                        pos: p4[i],
                        uv: [
                            (ax as f32 + uvs[i][0]) / ATLAS_W as f32,
                            (ay as f32 + uvs[i][1]) / ATLAS_H as f32,
                        ],
                        color,
                        light_hurt: [light_r, light_g, light_b, hurt],
                    });
                    if w.foil {
                        armor_glint.push(
                            p4[i],
                            [uvs[i][0] / sheet_w as f32, uvs[i][1] / sheet_h as f32],
                        );
                    }
                }
            }
        }
    }

    /// Worn armour, over the body it sits on (M46).
    ///
    /// `HumanoidArmorLayer` is a render *layer*: it re-poses the humanoid mesh
    /// with the model's own angles and draws it inflated, so the armour is
    /// built here from the same `xf` the body just used. Deriving the pose a
    /// second time would drift the moment an arm swung.
    ///
    /// Nothing is drawn at all unless vanilla gives this entity the layer —
    /// see [`mobs::wears_humanoid_armor`], which is transcribed from the
    /// renderers rather than sniffed from the mesh. An illager has the whole
    /// humanoid limb set and still wears no armour.
    #[allow(clippy::too_many_arguments)]
    fn emit_armor(
        &self,
        verts: &mut Vec<Vertex>,
        trim_verts: &mut Vec<Vertex>,
        armor_glint: &mut GlintSink<'_>,
        d: &EntityDraw<'_>,
        model: &MobModel,
        xf: &[([[f32; 3]; 3], [f32; 3])],
        frame: &EntityFrame,
    ) {
        // M416 — a baby humanoid wears its own armour mesh.
        let baby = mobs::baby_armor_set(d.kind);
        if baby.is_none() && !mobs::wears_humanoid_armor(d.kind) {
            return;
        }
        let [light_r, light_g, light_b] = d.light;
        let hurt = overlay_value(d.hurt, d.white_overlay);
        for (slot, piece) in [
            (mobs::ArmorSlot::Chest, d.armor[1]),
            (mobs::ArmorSlot::Legs, d.armor[2]),
            (mobs::ArmorSlot::Feet, d.armor[3]),
            (mobs::ArmorSlot::Head, d.armor[0]),
        ] {
            let Some(piece) = piece else { continue };
            // This slot's boxes: the body part each hangs from, the shift
            // from that part's rest pivot to the armour part's own (M416:
            // a baby's armour mesh has pivots of its own; the adult's
            // shares the body's), and the faces.
            let placed: Vec<(usize, [f32; 3], [mobs::CubeFace; 6])> = match baby {
                Some(set) => mobs::baby_armor_boxes(set, slot)
                    .into_iter()
                    .filter_map(|b| {
                        let pi = if b.part.is_empty() {
                            mobs::STATIC_PART
                        } else {
                            mobs::armor_part(&model.parts, b.part)?
                        };
                        let rest = model.parts[pi].pivot;
                        let shift = std::array::from_fn(|k| b.pivot[k] - rest[k]);
                        let faces = mobs::cube_faces_g(b.uv, b.min, b.dims, b.grow, b.mirror);
                        Some((pi, shift, faces))
                    })
                    .collect(),
                Option::None => mobs::armor_boxes(slot)
                    .into_iter()
                    .filter_map(|b| {
                        let pi = mobs::armor_part(&model.parts, b.part)?;
                        let faces =
                            mobs::cube_faces(b.uv, b.min, b.dims, slot.grow() + b.extend, b.mirror);
                        Some((pi, [0.0; 3], faces))
                    })
                    .collect(),
            };
            // In list order: `renderLayers` walks the layers in order, and an
            // overlay has to land on top of the base it covers. The trim is
            // appended as a further "layer" with its own origin and no tint —
            // `submitModel(..., -1, sprite, ...)` passes white, because a
            // trim's colour is baked into its palette-permuted sprite.
            let dyed = piece
                .layers
                .iter()
                .flatten()
                .map(|(k, t)| (Some(*k), None, *t));
            let trimmed = piece.trim.into_iter().map(|o| (None, Some(o), [1.0f32; 3]));
            // M50: `renderLayers` clears `renderFoil` inside the loop, so the
            // foil is drawn **once per piece**, riding the first layer that
            // draws — and the trim, submitted after the loop, never gets one.
            // A glinting trim would be an invention; a second glint on
            // leather's overlay layer would double an additive blend.
            let mut foil_pending = piece.foil;
            for (key, trim_origin, tint) in dyed.chain(trimmed) {
                let Some((ax, ay, sheet_w, sheet_h)) = (match (key, trim_origin) {
                    (Some(k), _) => self.armor_slots.get(k).copied(),
                    // A trim never carries the foil, so its sheet size is never
                    // read; the pool slot's own dimensions are the honest answer.
                    (_, Some((x, y))) => Some((x, y, TRIM_SLOT_W, TRIM_SLOT_H)),
                    _ => None,
                }) else {
                    continue;
                };
                // This layer is the one the foil rides, if the piece has one.
                let foil_here = foil_pending && trim_origin.is_none();
                foil_pending &= !foil_here;
                // The trim goes to its own range: its depth test is EQUAL, so it
                // paints only where the armour it decorates already won.
                let verts: &mut Vec<Vertex> = if trim_origin.is_some() {
                    trim_verts
                } else {
                    verts
                };
                for (pi, shift, faces) in &placed {
                    let (m, o) = &xf[*pi];
                    let o = &[o[0] + shift[0], o[1] + shift[1], o[2] + shift[2]];
                    // The sheet is 64x32 in the classic armour layout (64x64
                    // for a baby's), packed into the shared atlas at `(ax, ay)`.
                    for (_facing, pos, uvs) in faces {
                        if verts.len() + 6 > MAX_VERTS {
                            return;
                        }
                        let mut p4 = [[0f32; 3]; 4];
                        for (i, corner) in pos.iter().enumerate() {
                            let r = mat_apply(m, *corner);
                            let v = [r[0] + o[0], r[1] + o[1], r[2] + o[2]];
                            // M402 — the body's own frame (`EntityFrame`).
                            p4[i] = frame.place(v);
                        }
                        let n = face_normal(&p4);
                        let shade = mobs::mix_light_flipped(n, self.nether_light);
                        // M433 — the dye is vanilla's colour, bytes over 255: multiplied with
                        // the shade on gamma-encoded numbers.
                        let color = gamma_vertex(shade, tint);
                        let uv4: [[f32; 2]; 4] = std::array::from_fn(|i| {
                            [
                                (ax as f32 + uvs[i][0]) / ATLAS_W as f32,
                                (ay as f32 + uvs[i][1]) / ATLAS_H as f32,
                            ]
                        });
                        // The dye is a **vertex colour**, which is where vanilla
                        // puts it: `submitModel(..., color, ...)`, and `entity.fsh`
                        // multiplies `texture * vertexColor`. It rides in the same
                        // channel and the same space as the directional shade, so
                        // an untinted layer is exactly `tint = 1`.
                        for &i in &[0usize, 1, 2, 0, 2, 3] {
                            verts.push(Vertex {
                                pos: p4[i],
                                uv: uv4[i],
                                color,
                                light_hurt: [light_r, light_g, light_b, hurt],
                            });
                            // M50: the foil, from the same `p4` at the same moment.
                            // Its pipeline depth-tests EQUAL, so a position derived
                            // a second time — however faithfully — would be rejected
                            // fragment by fragment. M45 records the same rule.
                            //
                            // The UV is the quad's **own** `0..1` in its own sheet,
                            // not its place in Rewo's atlas: vanilla feeds the glint
                            // pass the model's `UV0`, which `ModelPart.Cube` already
                            // divided by the 64x32 sheet. An atlas coordinate would
                            // make the pattern depend on the packer.
                            //
                            // No tint. `RenderPipelines.GLINT` binds
                            // `DefaultVertexFormat.POSITION_TEX` — no Color element,
                            // so `BufferBuilder` drops the colour `submitModel` was
                            // handed — `glint.vsh` declares no colour attribute, and
                            // `RenderType.writeDynamicTransforms` writes
                            // `ColorModulator` as WHITE. A dyed leather piece's foil
                            // is the plain sheen.
                            if foil_here {
                                armor_glint.push(
                                    p4[i],
                                    [uvs[i][0] / sheet_w as f32, uvs[i][1] / sheet_h as f32],
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn emit_model(
        &self,
        verts: &mut Vec<Vertex>,
        trim_verts: &mut Vec<Vertex>,
        // Vanilla emissive-layer geometry (M57). Its own range because it
        // needs its own pipeline (depth GREATER_OR_EQUAL, no depth write).
        emis: &mut Vec<Vertex>,
        // M285 — the energy swirl's geometry (its own sheets and pipeline),
        // by [`SwirlSheet`].
        swirl: &mut [Vec<Vertex>; 2],
        // M286 — the breeze's wind (its own sheet and pipeline).
        wind_out: &mut Vec<Vertex>,
        // M306 — the `entityTranslucent` tail of the model.
        translucent_out: &mut Vec<Vertex>,
        // M363 — the depth-only water patch (`waterMask`).
        mask_out: &mut Vec<Vertex>,
        d: &EntityDraw<'_>,
        model: &MobModel,
        time: f32,
        cem_state: &mut std::collections::HashMap<u64, CemVars>,
        glint: Option<&mut GlintSink<'_>>,
        armor_glint: &mut GlintSink<'_>,
    ) {
        let mut glint = glint;
        let theta = (180.0 - d.yaw).to_radians();
        let (st, ct) = theta.sin_cos();
        // M24 death topple. `LivingEntityRenderer.setupRotations` pushes
        // `Axis.ZP.rotationDegrees(fall * getFlipDegrees())` *after* the body
        // yaw, and `submit` then pushes `scale(-1,-1,1)` and the -1.501
        // translate. A PoseStack transforms the coordinate system, so a point
        // runs that chain in reverse: model transform, then this roll, then the
        // yaw — which is precisely the seam between `e` and the rotY below.
        let (sr, cr) = death_roll(d).sin_cos();
        let ctx = AnimCtx {
            pitch: d.pitch.to_radians(),
            // Vanilla head yaw is net-of-body (`netHeadYaw`), rotated about
            // the head part's own pivot.
            net: wrap_degrees(d.head_yaw - d.yaw).to_radians(),
            f: d.limb_swing * 0.6662,
            pos: d.limb_swing,
            amt: d.limb_amount,
            age: time * 20.0,
            gesture: d.gesture,
            events: d.events,
            shell: d.shell,
            body_yaw: d.yaw.to_radians(),
            stand_pose: d.stand_parts.pose,
            allay_dance: d.allay_dance,
            attack: d.attack,
            arm_poses: d.arm_poses,
            mob: d.mob,
            tendril: d.emissive.tendril,
        };
        // Resource-pack CEM animation (M9c): evaluate the expression program
        // this frame → per-bone [rx,ry,rz,tx,ty,tz] deltas, applied in
        // `part_transforms` alongside the built-in animation.
        // `REWO_CEM_NOANIM=1` skips it, rendering the pack model's *rest pose*
        // — the diagnostic that separates a static-geometry bug from an
        // animation bug when a CEM mob looks malformed.
        let cem = model.cem.as_ref().filter(|_| !cem_noanim()).map(|prog| {
            // Reload this entity's variable slots so FA's integrators continue
            // where they left off (see `CemVars`), then hand them straight back.
            let entry = cem_state.entry(cem_key(d)).or_default();
            entry.seen = self.generation;
            let carried = std::mem::take(&mut entry.slots);
            let mut actx = cem_anim_context(
                d,
                CemFrameInputs {
                    frame_time: self.frame_dt,
                    frame_counter: self.frame_counter,
                    age_seconds: time,
                    cam_pos: self.cam_pos,
                },
                carried,
            );
            let frame =
                crate::cem::eval_program(prog, &mut actx, model.parts.len(), &model.cem_translate);
            // Hand the (now-advanced) slots back for the next frame.
            cem_state.entry(cem_key(d)).or_default().slots = std::mem::take(&mut actx.user);
            frame
        });
        // A hidden bone hides its subtree. Parts are in tree pre-order, so one
        // forward pass ANDs each child with its parent.
        let cem_visible = cem.as_ref().map(|f| {
            let mut vis = f.visible.clone();
            for i in 0..vis.len() {
                if let Some(p) = model.parts[i].parent {
                    vis[i] &= vis[p as usize];
                }
            }
            vis
        });
        let (cem_deltas, cem_scale) = match &cem {
            Some(f) => (Some(f.deltas.as_slice()), Some(f.scale.as_slice())),
            Option::None => (None, None),
        };
        // M391 — `HappyGhastModel.setupAnim`: a harnessed happy ghast's
        // body (and its tentacles) at 0.9375, on top of any CEM scale.
        let squeezed = body_squeeze_scale(model, d, cem_scale);
        let cem_scale = squeezed.as_deref().or(cem_scale);
        let xf = part_transforms(model, &ctx, cem_deltas, cem_scale);
        // Per-entity render scale (slime/magma size) on top of the baked px→
        // block scale — vanilla scales the whole model uniformly by `size`.
        let s = model.scale * if d.scale_mul > 0.0 { d.scale_mul } else { 1.0 };
        // Model-space quad corners -> world space, through this entity's pose.
        // Shared by the base pass and the emissive layers so a layer can never
        // drift off the model it is glowing on (M57); M402 — and the whole
        // `setupRotations` chain is the one every layer places through.
        let frame = EntityFrame::new(d, s, time);
        let place = |q: &GpuQuad| {
            let (m, o) = &xf[q.part as usize];
            let mut p4 = [[0f32; 3]; 4];
            for (i, corner) in q.pos.iter().enumerate() {
                let r = mat_apply(m, *corner);
                p4[i] = frame.place([r[0] + o[0], r[1] + o[1], r[2] + o[2]]);
            }
            p4
        };
        // M436 — vanilla's `minecraft_mix_light` over the face's WORLD
        // normal: the quad's model normal carried through the same part pose
        // and frame its corners take (the model's y-down flip and the yaw
        // included), so a mob's sides shade by which way it faces. The
        // stored normal, not the winding: a mirrored box reverses its winding.
        let nether = self.nether_light;
        let shade_of = |q: &GpuQuad| -> f32 {
            let (m, o) = &xf[q.part as usize];
            let p = q.pos[0];
            let at = |v: [f32; 3]| {
                let r = mat_apply(m, v);
                frame.place([r[0] + o[0], r[1] + o[1], r[2] + o[2]])
            };
            let (a, b) = (
                at(p),
                at([p[0] + q.normal[0], p[1] + q.normal[1], p[2] + q.normal[2]]),
            );
            mobs::mix_light([b[0] - a[0], b[1] - a[1], b[2] - a[2]], nether)
        };
        // Per-part visibility (CEM bone hides + the vanilla shell / gesture /
        // illager swaps). Vanilla's emissive layers run the *same* `setupAnim`
        // on a copy of the model, so they obey exactly the same rules.
        let visible = |part: usize| {
            // M271 — an invisible body draws no part; its layers still draw.
            if d.body_hidden {
                return false;
            }
            // CEM `bone.visible` (goat horns, bee stinger, sheared faces ...).
            if cem_visible.as_ref().is_some_and(|v| !v[part]) {
                return false;
            }
            match model.parts[part].show {
                mobs::Show::Always => true,
                mobs::Show::ShellOnly => d.shell,
                mobs::Show::NotShell => !d.shell,
                mobs::Show::During(g) => matches!(d.gesture, Some((active, _)) if active == g),
                // `IllagerModel.setupAnim`'s tail: `arms.visible = crossedArms`
                // and `left/rightArm.visible = !crossedArms`.
                mobs::Show::IllagerCrossedOnly => {
                    d.mob.illager_pose == mobs::IllagerArmPose::Crossed
                }
                mobs::Show::IllagerNotCrossed => {
                    d.mob.illager_pose != mobs::IllagerArmPose::Crossed
                }
                mobs::Show::StandArms => d.stand_parts.arms,
                mobs::Show::StandBasePlate => d.stand_parts.base_plate,
                mobs::Show::Chested => d.chested,
                mobs::Show::AvatarPart(bit) => d.avatar_hidden & bit == 0,
                // M350 — the coral nautilus's corals.
                mobs::Show::Unarmoured => d.body_armor.is_none(),
                // M354 — the bee's stinger.
                mobs::Show::HasStinger => !d.mob.bee_stung,
                // M365 — the turtle's egg belly.
                mobs::Show::TurtleEgg => d.mob.turtle_egg,
            }
        };
        // The pack variant this entity drew, if the atlas has a slot for it.
        let variant_uv = (d.variant != 0)
            .then(|| model.variants.get(&d.variant))
            .flatten()
            .map(|v| v.as_slice());
        // M175 — a baby mob whose kind has a whole-sheet swap re-points its
        // main slot's UVs at the baby sheet. Additive with the pack variant
        // (a pack alternate of an adult and a baby offset never both apply to
        // one slot in practice; if they ever do, the sum is still the right
        // composition of two atlas moves).
        let baby_uv = model
            .baby_offsets
            .as_ref()
            .filter(|_| d.mob.is_baby)
            .map(|v| v.as_slice());
        // `EntityDraw::skin_uv`'s doc has always said "Ignored for non-player
        // models" and, until the real-texture gate (`mobtexshot`) asked, this
        // pass ignored nothing: a skin offset on a zombie relocated every one
        // of its quads onto whatever the atlas holds at that delta. The
        // invariant was held only by `live_cmd`'s `if is_player`, i.e. by one
        // caller — a comment acting as a justification rather than a guard, and
        // the exact shape of the "a mob renders with another mob's texture"
        // report §0.0 has carried since M46. Enforced here so it is a property
        // of the pass. Measured before the fix: a zombie with
        // `skin_uv = Some([0.125, 0.0625])` differed from the same zombie with
        // `None` in 7,362 bytes.
        let skin_du = match d.kind {
            EntityModelKind::Player | EntityModelKind::PlayerSlim => {
                d.skin_uv.unwrap_or([0.0, 0.0])
            }
            _ => [0.0, 0.0],
        };
        // Directional face shade x the entity's per-channel world light.
        let [light_r, light_g, light_b] = d.light;
        let hurt = overlay_value(d.hurt, d.white_overlay);
        // M64: `SheepWoolLayer.submit` opens `if (!state.isSheared)`, so a
        // shorn mob's fleece is not submitted at all. Rewo bakes that layer as
        // a texture slot of the one model, so the layer's absence is the
        // absence of its quads — and *removing* them is the point: the fleece
        // is inflated over the body, so a shorn sheep is thinner, not
        // recoloured.
        let shorn_slot = d.sheared.then_some(model.shearable_slot).flatten();
        // M68: the coplanar layer's slot, if it is drawn at all this frame.
        // `None` here means "there is no such layer, or its gate said no", and
        // in the second case its quads are skipped outright rather than moved.
        let coplanar_slot = model.coplanar_slot;
        // M68: per-texture-slot layer colour. Vanilla tints a *layer*; Rewo
        // bakes a layer as a texture slot, so one entry per slot is the same
        // statement. `1.0` is untinted. M433 — kept as vanilla's bytes over
        // 255 (gamma), which `gamma_vertex` multiplies with the shade.
        let mut slot_tint = [[1.0f32; 3]; MAX_MOB_TEXTURES];
        let lin3 = |rgb: [u8; 3]| rgb.map(|b| b as f32 / 255.0);
        // `SheepWoolLayer` and `SheepWoolUndercoatLayer` both pass
        // `state.getWoolColor()`, so the two slots take the *same* colour —
        // that is why the undercoat needs no dye field of its own.
        let wool = mobs::SHEEP_WOOL_COLORS[(d.dye.unwrap_or(0) & 15) as usize];
        for s in [model.tinted_slot, model.coplanar_slot]
            .into_iter()
            .flatten()
            .filter(|s| model.collar_slot != Some(*s))
        {
            if let Some(t) = slot_tint.get_mut(s as usize) {
                *t = lin3(wool);
            }
        }
        // M342 — the collar takes its own dye, `getTextureDiffuseColor`.
        if let (Some(s), Some(c)) = (model.collar_slot, d.collar) {
            if let Some(t) = slot_tint.get_mut(s as usize) {
                *t = lin3(mobs::DYE_DIFFUSE_COLORS[(c & 15) as usize]);
            }
        }
        if let Some((body, pattern)) = model.fish_slots {
            let [b, p] = d.fish_dye.unwrap_or([0, 0]);
            for (s, dye) in [(body, b), (pattern, p)] {
                if let Some(t) = slot_tint.get_mut(s as usize) {
                    *t = lin3(mobs::DYE_DIFFUSE_COLORS[(dye & 15) as usize]);
                }
            }
        }
        for (qi, q) in model.quads.iter().enumerate() {
            if verts.len() + 6 > MAX_VERTS || translucent_out.len() + 6 > MAX_VERTS {
                return;
            }
            if !visible(q.part as usize) {
                continue;
            }
            if shorn_slot == Some(q.tex) {
                continue;
            }
            // M312 — `RopesLayer.submit`'s gate: `isLeashHolder` and a
            // harness in the body slot.
            let ropes = model.ropes_slot == Some(q.tex);
            if ropes && !(d.leash_holder && d.body_armor.is_some()) {
                continue;
            }
            // The coplanar layer leaves the solid range: at the base model's
            // exact depth the solid pass's strict `GREATER` rejects every one
            // of its fragments. `trim_verts` is `CompareOp::EQUAL` with no
            // depth write, drawn after the solid range — the reversed-Z
            // reading of vanilla's `entityCutout` for coplanar geometry.
            let coplanar = coplanar_slot == Some(q.tex);
            // M323 — the lamb's fleece is coplanar too, but it is
            // `SheepWoolLayer`'s: shearing gates it (`shorn_slot`, above) and
            // the undercoat's dye rule does not.
            // M342 — a collar is coplanar too, and its gate is the collar
            // itself (`WolfCollarLayer`: only a tame wolf has one).
            let collar = model.collar_slot == Some(q.tex);
            if collar && d.collar.is_none() {
                continue;
            }
            if coplanar && !collar && !d.undercoat && model.shearable_slot != Some(q.tex) {
                continue;
            }
            let p4 = place(q);
            // Shade only -- the light rides its own attribute so the hurt
            // overlay can be mixed in before it (vanilla's `entity.fsh` order)
            // -- and, on the textures vanilla dyes, the layer colour, which
            // every one of those layers multiplies into the vertex colour the
            // same way. Linearized above: the tables are sRGB and the
            // attachment encodes on store (render discipline #1).
            let t = slot_tint[q.tex as usize];
            let c = gamma_vertex(shade_of(q), t);
            // Three things can move a quad's UVs onto a different texture of
            // the same size: a player's uploaded skin, a pack's ETF variant,
            // and (M175) a baby mob's whole-sheet swap. They never apply to
            // the same mob (skins are the player model's), so the variant
            // wins where it exists and the baby offset adds on top.
            let du = {
                let base = variant_uv.map_or(skin_du, |v| v[q.tex as usize]);
                match baby_uv {
                    Some(b) => [
                        base[0] + b[q.tex as usize][0],
                        base[1] + b[q.tex as usize][1],
                    ],
                    None => base,
                }
            };
            let out: &mut Vec<Vertex> = if coplanar {
                trim_verts
            } else if qi >= model.translucent_from {
                translucent_out
            } else {
                verts
            };
            // M397 — a sulfur cube holding a block draws the block instead
            // of its core (`SulfurCubeInnerLayer`'s if / else).
            if d.mob.sulfur_block.is_some()
                && qi < model.translucent_from
                && matches!(
                    d.kind,
                    EntityModelKind::SulfurCube | EntityModelKind::SulfurCubeSmall
                )
            {
                continue;
            }
            // M396 — a lit sulfur cube's core (the opaque quads under its
            // translucent shell) flashes white.
            let overlay = if ropes {
                0.0
            } else if d.mob.sulfur_core_flash && qi < model.translucent_from {
                overlay_value(false, 1.0)
            } else {
                hurt
            };
            for &i in &[0usize, 1, 2, 0, 2, 3] {
                out.push(Vertex {
                    pos: p4[i],
                    uv: [q.uv[i][0] + du[0], q.uv[i][1] + du[1]],
                    color: [c[0], c[1], c[2], 1.0],
                    // M312 — the ropes are submitted with `NO_OVERLAY`.
                    light_hurt: [light_r, light_g, light_b, overlay],
                });
            }
        }
        // M294: `SimpleEquipmentLayer`'s `HORSE_BODY`: the shell in each
        // worn layer's sheet, dyed by the vertex colour, lit and overlaid as
        // the armour below is. M295: the saddle the same way, with its reins
        // while ridden.
        let reins: &[GpuQuad] = if d.ridden { &model.saddle_reins } else { &[] };
        for (piece, shell, coplanar) in [
            (
                d.body_armor,
                if d.ridden && !model.body_shell_ridden.is_empty() {
                    &model.body_shell_ridden
                } else {
                    &model.body_shell
                }
                .iter()
                .chain([].iter()),
                false,
            ),
            (
                d.saddle,
                model.saddle_shell.iter().chain(reins.iter()),
                model.saddle_coplanar,
            ),
        ] {
            let Some(piece) = piece else { continue };
            for (key, tint) in piece.layers.iter().flatten() {
                let Some(&(ax, ay, _, _)) = self.armor_slots.get(*key) else {
                    continue;
                };
                for q in shell.clone() {
                    // M297 — a shell keeps the base model's parts, so the
                    // decor's chests follow `hasChest` as the body's do.
                    if model.parts[q.part as usize].show == mobs::Show::Chested && !d.chested {
                        continue;
                    }
                    let out: &mut Vec<Vertex> = if coplanar { trim_verts } else { verts };
                    if out.len() + 6 > MAX_VERTS {
                        return;
                    }
                    let p4 = place(q);
                    let c = gamma_vertex(shade_of(q), *tint);
                    for &i in &[0usize, 1, 2, 0, 2, 3] {
                        out.push(Vertex {
                            pos: p4[i],
                            uv: [
                                (ax as f32 + q.uv[i][0]) / ATLAS_W as f32,
                                (ay as f32 + q.uv[i][1]) / ATLAS_H as f32,
                            ],
                            color: [c[0], c[1], c[2], 1.0],
                            light_hurt: [light_r, light_g, light_b, hurt],
                        });
                    }
                }
            }
        }
        // M363 — `BoatRenderer.submit`: the water patch with
        // `RenderTypes.waterMask()` unless the boat is under water. Depth
        // only, so the white texel stands in for the sheet (it passes the
        // alpha test the frag shader still runs).
        if !d.mob.under_water {
            for q in &model.water_patch {
                if mask_out.len() + 6 > MAX_VERTS {
                    break;
                }
                let p4 = place(q);
                for &i in &[0usize, 1, 2, 0, 2, 3] {
                    mask_out.push(Vertex {
                        pos: p4[i],
                        uv: self.white_uv,
                        color: [1.0; 4],
                        light_hurt: [1.0, 1.0, 1.0, 0.0],
                    });
                }
            }
        }
        // M311 — `maybeRenderCracks`: the crack sheet over the armour's own
        // shell, `armorTranslucent` (the translucent range, sorted), with
        // `OverlayTexture.NO_OVERLAY` and untinted.
        if let (Some(key), Some(_)) = (d.body_armor_crack, d.body_armor) {
            if let Some(&(ax, ay, _, _)) = self.armor_slots.get(key) {
                for q in &model.body_shell {
                    if translucent_out.len() + 6 > MAX_VERTS {
                        return;
                    }
                    let p4 = place(q);
                    for &i in &[0usize, 1, 2, 0, 2, 3] {
                        translucent_out.push(Vertex {
                            pos: p4[i],
                            uv: [
                                (ax as f32 + q.uv[i][0]) / ATLAS_W as f32,
                                (ay as f32 + q.uv[i][1]) / ATLAS_H as f32,
                            ],
                            color: gamma_vertex(shade_of(q), [1.0; 3]),
                            light_hurt: [light_r, light_g, light_b, 0.0],
                        });
                    }
                }
            }
        }
        // M46: worn armour, a render layer over the body, before the held
        // item so a chestplate does not paint over a sword.
        self.emit_armor(verts, trim_verts, armor_glint, d, model, &xf, &frame);
        // M403: the elytra (`WingsLayer`), off the same frame.
        self.emit_wings(verts, armor_glint, d, &frame);
        // M60: the cape, hanging off the same body. `AvatarRenderer` adds
        // `HumanoidArmorLayer` first and `CapeLayer` eleven layers later, so
        // this is vanilla's order — though nothing rests on it here, since
        // both are opaque `entitySolid` draws in one range and the depth test
        // settles them. What keeps the cape off a chestplate is the clearance
        // shift, not the sequence.
        self.emit_cape(verts, d, model, &xf, s, st, ct, sr, cr, &frame);
        // M273: a worn skull, a render layer off the head.
        self.emit_worn_head(
            verts,
            d,
            model,
            &xf,
            &frame,
            overlay_value(d.hurt, d.white_overlay),
            glint.as_deref_mut(),
        );
        // M22: whatever each arm holds, drawn after the body —
        // `ItemInHandLayer` is a render layer, so it sits on top of the
        // model it hangs off.
        for (i, left) in [(0usize, false), (1usize, true)] {
            if let Some(name) = d.held[i] {
                let hurt = overlay_value(d.hurt, d.white_overlay);
                self.emit_held_item(
                    verts,
                    d,
                    model,
                    &xf,
                    left,
                    name,
                    &frame,
                    hurt,
                    glint.as_deref_mut().filter(|_| d.held_glint[i]),
                );
            }
        }
        // ---- vanilla emissive layers (M57) ------------------------------
        // `LivingEntityEmissiveLayer.submit`: skip the layer outright when its
        // alpha function is ~0, else render the filtered model with
        // `ARGB.white(alpha)` through an EMISSIVE pipeline — which, per
        // `entity.vsh`, samples **no lightmap at all**:
        //
        //     #ifndef EMISSIVE
        //         lightMapColor = sample_lightmap(Sampler2, UV2);
        //     #endif
        //
        // So the fullbright is not a brightness the layer adds, it is a
        // multiply the layer omits. `light_hurt.rgb` is therefore the identity
        // `[1,1,1]` below rather than `d.light` — the same statement in Rewo's
        // ABI, since `entity.frag` ends on `c *= v_light_hurt.rgb`.
        for layer in &model.emissive {
            let alpha = emissive_alpha(layer.alpha, ctx.age, &d.emissive);
            if alpha <= 1.0e-5 {
                continue;
            }
            // Vanilla's `ALPHA_CUTOUT` 0.1 on `entityTranslucentEmissive`
            // discards fragments with `texel.a * alpha < 0.1`. Every vanilla
            // emissive texture is either fully transparent or at least 0.19
            // opaque (checked across all nine), so the cutout only ever fires
            // layer-wide, as an alpha floor — which is what this is.
            if layer.cutout && alpha < 0.1 {
                continue;
            }
            // `NO_CARDINAL_LIGHTING` (the `eyes` pipeline) passes the vertex
            // colour through unshaded; `PER_FACE_LIGHTING` (emissive) keeps the
            // per-face directional term, which for us is the baked `shade`.
            // Same split as vanilla's two pipelines.
            let shaded = layer.cutout;
            for q in &layer.quads {
                if emis.len() + 6 > MAX_VERTS {
                    return;
                }
                if !visible(q.part as usize) {
                    continue;
                }
                let p4 = place(q);
                let c = if shaded {
                    srgb_to_linear(shade_of(q))
                } else {
                    1.0
                };
                for &i in &[0usize, 1, 2, 0, 2, 3] {
                    emis.push(Vertex {
                        pos: p4[i],
                        uv: q.uv[i],
                        color: [c, c, c, alpha],
                        light_hurt: [1.0, 1.0, 1.0, 0.0],
                    });
                }
            }
        }
        // ---- M285: `CreeperPowerLayer` (`EnergySwirlLayer.submit`) -------
        // `if (isPowered)`: the armour shell (the model at deformation 2,
        // posed by the same `setupAnim`) through `energySwirl(texture,
        // xOffset(t) % 1, t * 0.01 % 1)` with `xOffset(t) = t * 0.01` and
        // `t = ageInTicks`, coloured `-8355712` (0xFF808080). The pipeline is
        // `EMISSIVE` and `NO_CARDINAL_LIGHTING` (no light, no face shade),
        // `NO_OVERLAY`, so the vertex is the grey alone; the offset is the
        // `OffsetTextureTransform`, added to the layer's own UVs (the sheet's
        // REPEAT sampler wraps them). It is the layer's own model copy, so
        // the body's visibility (invisibility) does not hide it.
        // M287 — the powered wither's `WitherArmorLayer` is the same layer
        // class with its own `xOffset`.
        if d.powered && !model.swirl.is_empty() {
            let [u_off, v_off] = swirl_offsets(model.swirl_sheet, ctx.age);
            let grey = SWIRL_GREY;
            let out = &mut swirl[model.swirl_sheet as usize];
            for q in &model.swirl {
                if out.len() + 6 > MAX_VERTS {
                    return;
                }
                let p4 = place(q);
                for &i in &[0usize, 1, 2, 0, 2, 3] {
                    out.push(Vertex {
                        pos: p4[i],
                        uv: [q.uv[i][0] + u_off, q.uv[i][1] + v_off],
                        color: [grey, grey, grey, 1.0],
                        light_hurt: [1.0, 1.0, 1.0, 0.0],
                    });
                }
            }
        }
        // M286 — the breeze's wind layer.
        self.emit_wind(wind_out, d, model, ctx.age, place);
    }

    /// M286 — `BreezeWindLayer.submit`, every frame: the wind model (the
    /// breeze mesh kept to `wind_body`, posed by the same `setupAnim`)
    /// through `breezeWind(texture, xOffset(t) % 1, 0)` with `xOffset(t) = t
    /// · 0.02`, `t = ageInTicks`, in the default white. The pipeline keeps the
    /// lightmap but drops the face shade (`NO_CARDINAL_LIGHTING`) and the
    /// overlay; it is the layer's own copy, so invisibility does not hide it.
    fn emit_wind(
        &self,
        out: &mut Vec<Vertex>,
        d: &EntityDraw<'_>,
        model: &MobModel,
        age: f32,
        place: impl Fn(&GpuQuad) -> [[f32; 3]; 4],
    ) {
        if model.wind.is_empty() {
            return;
        }
        let off = (age * 0.02) % 1.0;
        let [lr, lg, lb] = d.light;
        for q in &model.wind {
            if out.len() + 6 > MAX_VERTS {
                return;
            }
            let p4 = place(q);
            for &i in &[0usize, 1, 2, 0, 2, 3] {
                out.push(Vertex {
                    pos: p4[i],
                    uv: [q.uv[i][0] + off, q.uv[i][1]],
                    color: [1.0, 1.0, 1.0, 1.0],
                    light_hurt: [lr, lg, lb, 0.0],
                });
            }
        }
    }

    /// Glyph + background quads for one nametag, camera-billboarded.
    fn push_tag(
        &self,
        verts: &mut Vec<Vertex>,
        d: &EntityDraw<'_>,
        name: &str,
        right: [f32; 3],
        up: [f32; 3],
        // M410 — blocks above the attachment (a name over a score line).
        lift: f32,
    ) {
        let cell = self.cell as f32;
        let scale = TAG_PX * (8.0 / cell);
        let total_px: f32 = name.bytes().map(|b| self.advance[b as usize] as f32).sum();
        let anchor = [d.pos[0], d.pos[1] + d.height + TAG_LIFT + lift, d.pos[2]];
        let world = |px: f32, py: f32| -> [f32; 3] {
            [
                anchor[0] + (right[0] * px + up[0] * py) * scale,
                anchor[1] + (right[1] * px + up[1] * py) * scale,
                anchor[2] + (right[2] * px + up[2] * py) * scale,
            ]
        };
        let mut quad = |x0: f32, y0: f32, x1: f32, y1: f32, uv: [f32; 4], color: [f32; 4]| {
            if verts.len() + 6 > MAX_VERTS {
                return;
            }
            let (p00, p10, p11, p01) = (world(x0, y0), world(x1, y0), world(x1, y1), world(x0, y1));
            let [u0, v0, u1, v1] = uv;
            for (p, u, v) in [
                (p00, u0, v0),
                (p10, u1, v0),
                (p11, u1, v1),
                (p00, u0, v0),
                (p11, u1, v1),
                (p01, u0, v1),
            ] {
                verts.push(Vertex {
                    pos: p,
                    uv: [u, v],
                    color,
                    // Nametags are fullbright and never take the hurt overlay.
                    light_hurt: [1.0, 1.0, 1.0, 0.0],
                });
            }
        };

        // Background: vanilla's 25% black plate, 1px padding.
        let (bx0, bx1) = (-total_px / 2.0 - 1.0, total_px / 2.0 + 1.0);
        let wu = self.white_uv;
        quad(
            bx0,
            -1.0,
            bx1,
            cell + 1.0,
            [wu[0], wu[1], wu[0], wu[1]],
            [0.0, 0.0, 0.0, 0.25],
        );

        // Glyphs, left to right.
        let mut pen = -total_px / 2.0;
        for b in name.bytes() {
            let adv = self.advance[b as usize] as f32;
            if b != b' ' {
                let [u0, v0, u1, v1] = self.glyph_uv(b);
                quad(pen, 0.0, pen + cell, cell, [u0, v0, u1, v1], [1.0; 4]);
            }
            pen += adv;
        }
    }

    /// One framed map's banner label (M204), `MapRenderer.render`:
    ///
    /// ```text
    /// float width = font.width(decoration.name);
    /// float scale = Mth.clamp(25.0F / width, 0.0F, 6.0F / 9.0F);
    /// translate(x / 2 + 64 - width * scale / 2, y / 2 + 64 + 4, -0.025F);
    /// scale(scale, scale, -1.0F); translate(0, 0, 0.1F);
    /// submitText(..., NORMAL, light, color -1, background Integer.MIN_VALUE, ...)
    /// ```
    ///
    /// The background is `Font`'s effect rect, `(x - 1, y - 1)` to
    /// `(x + width, y + 9)` at `z -0.01`, in `0x80000000` (black at half
    /// alpha); the glyphs are white. Plain text: a styled name's colours are
    /// not carried (a recorded deviation).
    fn push_map_label(&self, verts: &mut Vec<Vertex>, m: &FrameMapDraw<'_>, label: &MapLabel) {
        let width: f32 = label
            .text
            .bytes()
            .map(|b| self.advance[b as usize] as f32)
            .sum();
        if width <= 0.0 {
            return;
        }
        let scale = (25.0 / width).clamp(0.0, 6.0 / 9.0);
        let base = glam::Mat4::from_cols_array_2d(&[
            [m.transform[0][0], m.transform[1][0], m.transform[2][0], 0.0],
            [m.transform[0][1], m.transform[1][1], m.transform[2][1], 0.0],
            [m.transform[0][2], m.transform[1][2], m.transform[2][2], 0.0],
            [m.transform[0][3], m.transform[1][3], m.transform[2][3], 1.0],
        ]);
        let pose =
            base * glam::Mat4::from_translation(glam::Vec3::new(
                f32::from(label.x) / 2.0 + 64.0 - width * scale / 2.0,
                f32::from(label.y) / 2.0 + 64.0 + 4.0,
                -0.025,
            )) * glam::Mat4::from_scale(glam::Vec3::new(scale, scale, -1.0))
                * glam::Mat4::from_translation(glam::Vec3::new(0.0, 0.0, 0.1));
        let place = |x: f32, y: f32, z: f32| -> [f32; 3] {
            let p = pose.transform_point3(glam::Vec3::new(x, y, z));
            [m.pos[0] + p.x, m.pos[1] + p.y, m.pos[2] + p.z]
        };
        let [lr, lg, lb] = m.light;
        let mut quad =
            |x0: f32, y0: f32, x1: f32, y1: f32, z: f32, uv: [f32; 4], color: [f32; 4]| {
                if verts.len() + 6 > MAX_VERTS {
                    return;
                }
                let [u0, v0, u1, v1] = uv;
                let (p00, p10, p11, p01) = (
                    place(x0, y0, z),
                    place(x1, y0, z),
                    place(x1, y1, z),
                    place(x0, y1, z),
                );
                for (p, u, v) in [
                    (p00, u0, v0),
                    (p10, u1, v0),
                    (p11, u1, v1),
                    (p00, u0, v0),
                    (p11, u1, v1),
                    (p01, u0, v1),
                ] {
                    verts.push(Vertex {
                        pos: p,
                        uv: [u, v],
                        color,
                        light_hurt: [lr, lg, lb, 0.0],
                    });
                }
            };
        let wu = self.white_uv;
        quad(
            -1.0,
            -1.0,
            width,
            9.0,
            -0.01,
            [wu[0], wu[1], wu[0], wu[1]],
            [0.0, 0.0, 0.0, 128.0 / 255.0],
        );
        // Glyphs: an 8-px cell per byte from the pen, the font atlas's cells
        // scaled as `emit_world_text` scales them. `glyph_uv` returns the
        // cell's bottom as `v0`, so the y-down quad takes its V reversed.
        let mut pen = 0.0;
        for b in label.text.bytes() {
            let adv = self.advance[b as usize] as f32;
            if b != b' ' {
                let [u0, v0, u1, v1] = self.glyph_uv(b);
                quad(pen, 0.0, pen + 8.0, 8.0, 0.0, [u0, v1, u1, v0], [1.0; 4]);
            }
            pen += adv;
        }
    }

    /// The floating health bar — a backing plate and a fill, both
    /// camera-billboarded (M59).
    ///
    /// A deliberate sibling of [`Self::push_tag`], and for a reason worth
    /// stating: a bar needs no new geometry type, texture, pipeline or blend
    /// state. It is the nametag's plate twice over — the same untextured quad
    /// sampling the same guaranteed-opaque white texel, on the same camera
    /// basis, in the same font-pixel units, emitted into the same alpha-blended
    /// text range. Everything here is layout and arithmetic.
    ///
    /// Every rule below cites `REWO_HEALTH_BAR_SPEC.md`, which is the source of
    /// truth; there is no vanilla behaviour to transcribe.
    fn push_health_bar(
        &self,
        verts: &mut Vec<Vertex>,
        d: &EntityDraw<'_>,
        right: [f32; 3],
        up: [f32; 3],
    ) {
        // Spec rules 4 and 5: the resolver has already decided this entity may
        // show a bar at all. There is no fallback here on purpose.
        let Some(hb) = d.health else { return };
        // Spec rule 1. Both clamps: absorption can push health past max, and a
        // max lowered after the fact can leave a stale ratio above 1.
        let fraction = (hb.current / hb.max).clamp(0.0, 1.0);
        // A zero or NaN max divides to NaN, which `clamp` propagates. Hidden
        // rather than drawn at some arbitrary width — the same instinct as rule
        // 4, one level down.
        if fraction.is_nan() {
            return;
        }
        // Spec rule 3 — **hidden at full health**, and this is a choice, not an
        // oversight. A peaceful scene stays uncluttered, and the damaged signal
        // becomes the bar's *presence* rather than its width. Emitting a full
        // bar here would be the natural-looking bug.
        if fraction >= 1.0 {
            return;
        }

        let cell = self.cell as f32;
        let scale = TAG_PX * (8.0 / cell);
        // The nametag's anchor, unchanged (spec: `pos.y + height + TAG_LIFT`).
        let anchor = [d.pos[0], d.pos[1] + d.height + TAG_LIFT, d.pos[2]];
        let world = |px: f32, py: f32| -> [f32; 3] {
            [
                anchor[0] + (right[0] * px + up[0] * py) * scale,
                anchor[1] + (right[1] * px + up[1] * py) * scale,
                anchor[2] + (right[2] * px + up[2] * py) * scale,
            ]
        };
        let wu = self.white_uv;
        let mut quad = |x0: f32, y0: f32, x1: f32, y1: f32, color: [f32; 4]| {
            if verts.len() + 6 > MAX_VERTS {
                return;
            }
            let (p00, p10, p11, p01) = (world(x0, y0), world(x1, y0), world(x1, y1), world(x0, y1));
            for p in [p00, p10, p11, p00, p11, p01] {
                verts.push(Vertex {
                    pos: p,
                    uv: wu,
                    color,
                    // Fullbright and never hurt-flashed, exactly as a tag is: the
                    // bar is a label, not a surface of the mob.
                    light_hurt: [1.0, 1.0, 1.0, 0.0],
                });
            }
        };

        // The tag renders *above* the anchor (its plate spans -1 .. cell+1), so
        // the bar hangs *below* it. Spec: at the anchor with no tag, `BAR_GAP`
        // below the anchor with one.
        let top = if d.name.is_some() { -BAR_GAP } else { 0.0 };
        let half = BAR_W / 2.0;

        // Plate: the fill's box grown by `BAR_PAD` on all four sides.
        quad(
            -half - BAR_PAD,
            top - BAR_H - 2.0 * BAR_PAD,
            half + BAR_PAD,
            top,
            BAR_PLATE,
        );

        // Fill: spec rule 2 — `fraction * BAR_W` **exactly**, no rounding to
        // whole pixels, growing rightward from the plate's inner left edge. It is
        // emitted even at width zero, so a visible bar is always twelve vertices
        // and the count alone says whether one is showing.
        let color = if fraction < CRITICAL_FRAC {
            BAR_FILL_CRITICAL
        } else {
            BAR_FILL_HEALTHY
        };
        quad(
            -half,
            top - BAR_PAD - BAR_H,
            -half + fraction * BAR_W,
            top - BAR_PAD,
            color,
        );
    }

    /// Exactly the vertices [`Self::set_draws`] emits for one entity's health
    /// bar, as `(world position, linear colour)` pairs — the oracle hook
    /// `healthbarshot` measures (M59).
    ///
    /// It calls [`Self::push_health_bar`] itself rather than describing it. M45
    /// and M41 both shipped gates that had quietly stopped testing their subject
    /// because they reimplemented a slice of the path they were grading; a hook
    /// that *is* the emitter cannot drift from it.
    pub fn oracle_health_bar(
        &self,
        d: &EntityDraw<'_>,
        right: [f32; 3],
        up: [f32; 3],
    ) -> Vec<([f32; 3], [f32; 4])> {
        let mut verts: Vec<Vertex> = Vec::new();
        self.push_health_bar(&mut verts, d, right, up);
        verts.iter().map(|v| (v.pos, v.color)).collect()
    }

    /// How many vertices the last [`Self::set_draws`] put in the blended text
    /// range — nametags and health bars. Lets a gate prove the emitter is on the
    /// real path and not merely callable.
    pub fn text_vert_count(&self) -> u32 {
        self.text_verts
    }

    /// Shared draw state (viewport, descriptor, push, vertex buffer).
    ///
    /// `false` when this frame uploaded nothing, in which case the caller must
    /// not draw.
    #[must_use]
    unsafe fn bind_common(
        &self,
        gpu: &Gpu,
        cb: vk::CommandBuffer,
        view_proj: [[f32; 4]; 4],
        extent: vk::Extent2D,
    ) -> bool {
        let Some(vbuf) = self.ring.bind() else {
            return false;
        };
        let device = &gpu.device;
        let viewport = vk::Viewport::default()
            .y(extent.height as f32)
            .width(extent.width as f32)
            .height(-(extent.height as f32))
            .max_depth(1.0);
        device.cmd_set_viewport(cb, 0, &[viewport]);
        device.cmd_set_scissor(cb, 0, &[vk::Rect2D::default().extent(extent)]);
        device.cmd_bind_descriptor_sets(
            cb,
            vk::PipelineBindPoint::GRAPHICS,
            self.layout,
            0,
            &[self.set],
            &[],
        );
        device.cmd_push_constants(
            cb,
            self.layout,
            vk::ShaderStageFlags::VERTEX,
            0,
            std::slice::from_raw_parts(view_proj.as_ptr() as *const u8, 64),
        );
        // M412 — this frame's fog (none, for a pass nobody set it on).
        let fog = self.fog.get();
        device.cmd_push_constants(
            cb,
            self.layout,
            vk::ShaderStageFlags::FRAGMENT,
            64,
            std::slice::from_raw_parts(fog.as_ptr() as *const u8, 48),
        );
        device.cmd_bind_vertex_buffers(cb, 0, &[vbuf], &[0]);
        true
    }

    /// M412 — the fog this pass draws with from now on (`world.frag`'s:
    /// `[camera, render start]`, `[colour, render end]`, `[env start, env
    /// end, _, _]`).
    pub fn set_fog(&self, fog: [[f32; 4]; 3]) {
        self.fog.set(fog);
    }

    /// M278 — the entity pass's pipeline layout, which the outline's
    /// geometry pipeline shares.
    pub fn layout(&self) -> vk::PipelineLayout {
        self.layout
    }

    /// M278 — whether any entity glows this frame.
    pub fn has_outline(&self) -> bool {
        self.outline_verts > 0
    }

    /// M278 — the glowing entities' outline copies, the last range, with
    /// `pipeline` (the outline pass's, rendering into the outline target).
    pub fn draw_outline_geometry(
        &self,
        gpu: &Gpu,
        cb: vk::CommandBuffer,
        view_proj: [[f32; 4]; 4],
        extent: vk::Extent2D,
        pipeline: vk::Pipeline,
    ) {
        if self.outline_verts == 0 {
            return;
        }
        unsafe {
            if !self.bind_common(gpu, cb, view_proj, extent) {
                return;
            }
            gpu.device
                .cmd_bind_pipeline(cb, vk::PipelineBindPoint::GRAPHICS, pipeline);
            gpu.device.cmd_draw(
                cb,
                self.outline_verts,
                1,
                self.solid_verts
                    + self.text_verts
                    + self.glint_verts
                    + self.trim_verts
                    + self.armor_glint_verts
                    + self.emissive_verts,
                0,
            );
        }
    }

    /// Opaque capsules — draw before any translucent content.
    pub fn draw_solid(
        &self,
        gpu: &Gpu,
        cb: vk::CommandBuffer,
        view_proj: [[f32; 4]; 4],
        extent: vk::Extent2D,
    ) {
        if self.solid_verts == 0 {
            return;
        }
        unsafe {
            if !self.bind_common(gpu, cb, view_proj, extent) {
                return;
            }
            let device = &gpu.device;
            device.cmd_bind_pipeline(cb, vk::PipelineBindPoint::GRAPHICS, self.solid_pipeline);
            device.cmd_draw(cb, self.solid_verts, 1, 0, 0);
        }
    }

    /// Build the glint pipeline and upload `misc/enchanted_glint_item.png`
    /// (M45). Optional, like the other two glints: no sheet, no shimmer.
    /// Vanilla emissive layers (mob eyes, the warden's glow) — drawn
    /// immediately after the solid models, which is where vanilla's `order(1)`
    /// layer submits land: on top of the base model, under the translucent
    /// world (M57).
    pub fn draw_emissive(
        &self,
        gpu: &Gpu,
        cb: vk::CommandBuffer,
        view_proj: [[f32; 4]; 4],
        extent: vk::Extent2D,
    ) {
        if self.emissive_verts == 0 {
            return;
        }
        unsafe {
            if !self.bind_common(gpu, cb, view_proj, extent) {
                return;
            }
            let device = &gpu.device;
            device.cmd_bind_pipeline(cb, vk::PipelineBindPoint::GRAPHICS, self.emissive_pipeline);
            // Last range in the buffer — see the storage comment in `set_draws`.
            device.cmd_draw(
                cb,
                self.emissive_verts,
                1,
                self.solid_verts
                    + self.text_verts
                    + self.glint_verts
                    + self.trim_verts
                    + self.armor_glint_verts,
                0,
            );
        }
    }

    /// M285 — the energy swirl's sheet (`creeper_armor.png`) and its
    /// pipeline (`RenderPipelines.ENERGY_SWIRL`). `color_format` is the UNORM
    /// view's: the swirl blends in gamma space, as vanilla's does.
    pub fn init_swirl(
        &mut self,
        gpu: &mut Gpu,
        which: SwirlSheet,
        rgba: &[u8],
        w: u32,
        h: u32,
        color_format: vk::Format,
    ) -> Result<(), String> {
        let slot = match which {
            SwirlSheet::Creeper => &self.swirl,
            SwirlSheet::Wither => &self.wither_swirl,
        };
        if slot.is_some() {
            return Ok(());
        }
        let sheet = self.load_sheet(gpu, rgba, w, h, color_format, vk::Filter::NEAREST)?;
        if self.swirl_pipeline.is_none() {
            self.swirl_pipeline = Some(build_sheet_pipeline(
                &gpu.device,
                self.layout,
                color_format,
                include_bytes!(concat!(env!("OUT_DIR"), "/entity_swirl.frag.spv")),
                SheetBlend::Swirl,
            )?);
        }
        match which {
            SwirlSheet::Creeper => self.swirl = Some(sheet),
            SwirlSheet::Wither => self.wither_swirl = Some(sheet),
        }
        Ok(())
    }

    /// M286 — the breeze wind's sheet (`breeze_wind.png`) and its pipeline
    /// (`RenderPipelines.BREEZE_WIND`), in the UNORM view's format.
    pub fn init_wind(
        &mut self,
        gpu: &mut Gpu,
        rgba: &[u8],
        w: u32,
        h: u32,
        color_format: vk::Format,
    ) -> Result<(), String> {
        if self.wind.is_some() {
            return Ok(());
        }
        let sheet = self.load_sheet(gpu, rgba, w, h, color_format, vk::Filter::NEAREST)?;
        if self.wind_pipeline.is_none() {
            self.wind_pipeline = Some(build_sheet_pipeline(
                &gpu.device,
                self.layout,
                color_format,
                include_bytes!(concat!(env!("OUT_DIR"), "/entity_wind.frag.spv")),
                SheetBlend::Wind,
            )?);
        }
        self.wind = Some(sheet);
        Ok(())
    }

    /// Vertices in the wind range for the last [`Self::set_entities`].
    pub fn wind_vertex_count(&self) -> u32 {
        self.wind_verts
    }

    /// M363 — vertices in the water-mask range for the last
    /// [`Self::set_entities`].
    pub fn water_mask_vertex_count(&self) -> u32 {
        self.water_mask_verts
    }

    /// M363 — the boats' water patches: depth written, no colour
    /// (`RenderTypes.waterMask()`), after the solid models and before the
    /// translucent terrain, so the water surface inside a floating hull
    /// fails its depth test.
    pub fn draw_water_mask(
        &self,
        gpu: &Gpu,
        cb: vk::CommandBuffer,
        view_proj: [[f32; 4]; 4],
        extent: vk::Extent2D,
    ) {
        if self.water_mask_verts == 0 {
            return;
        }
        unsafe {
            if !self.bind_common(gpu, cb, view_proj, extent) {
                return;
            }
            let device = &gpu.device;
            device.cmd_bind_pipeline(
                cb,
                vk::PipelineBindPoint::GRAPHICS,
                self.water_mask_pipeline,
            );
            // The last range in the buffer.
            device.cmd_draw(
                cb,
                self.water_mask_verts,
                1,
                self.solid_verts
                    + self.text_verts
                    + self.glint_verts
                    + self.trim_verts
                    + self.armor_glint_verts
                    + self.emissive_verts
                    + self.outline_verts
                    + self.swirl_verts
                    + self.wind_verts
                    + self.translucent_verts,
                0,
            );
        }
    }

    /// M306 — vertices in the translucent range for the last
    /// [`Self::set_entities`].
    pub fn translucent_vertex_count(&self) -> u32 {
        self.translucent_verts
    }

    /// M306 — the `entityTranslucent` layers, after the wind, in gamma
    /// space: `BlendFunction.TRANSLUCENT` over the atlas, depth
    /// `GREATER_OR_EQUAL` with writes, no culling.
    pub fn draw_translucent(
        &self,
        gpu: &Gpu,
        cb: vk::CommandBuffer,
        view_proj: [[f32; 4]; 4],
        extent: vk::Extent2D,
    ) {
        let Some(pipeline) = self.translucent_pipeline else {
            return;
        };
        if self.translucent_verts == 0 {
            return;
        }
        unsafe {
            if !self.bind_common(gpu, cb, view_proj, extent) {
                return;
            }
            let device = &gpu.device;
            device.cmd_bind_pipeline(cb, vk::PipelineBindPoint::GRAPHICS, pipeline);
            // Last range in the buffer.
            device.cmd_draw(
                cb,
                self.translucent_verts,
                1,
                self.solid_verts
                    + self.text_verts
                    + self.glint_verts
                    + self.trim_verts
                    + self.armor_glint_verts
                    + self.emissive_verts
                    + self.outline_verts
                    + self.swirl_verts
                    + self.wind_verts,
                0,
            );
        }
    }

    /// M286 — the breeze's wind, after the swirl (both `order(1)`), in gamma
    /// space: `BlendFunction.TRANSLUCENT`, depth `GREATER_OR_EQUAL` with
    /// writes, no culling.
    pub fn draw_wind(
        &self,
        gpu: &Gpu,
        cb: vk::CommandBuffer,
        view_proj: [[f32; 4]; 4],
        extent: vk::Extent2D,
    ) {
        let (Some(g), Some(pipeline)) = (self.wind.as_ref(), self.wind_pipeline) else {
            return;
        };
        if self.wind_verts == 0 {
            return;
        }
        unsafe {
            if !self.bind_common(gpu, cb, view_proj, extent) {
                return;
            }
            let device = &gpu.device;
            device.cmd_bind_pipeline(cb, vk::PipelineBindPoint::GRAPHICS, pipeline);
            device.cmd_bind_descriptor_sets(
                cb,
                vk::PipelineBindPoint::GRAPHICS,
                self.layout,
                0,
                &[g.set],
                &[],
            );
            // Last range in the buffer.
            device.cmd_draw(
                cb,
                self.wind_verts,
                1,
                self.solid_verts
                    + self.text_verts
                    + self.glint_verts
                    + self.trim_verts
                    + self.armor_glint_verts
                    + self.emissive_verts
                    + self.outline_verts
                    + self.swirl_verts,
                0,
            );
        }
    }

    pub fn swirl_ready(&self) -> bool {
        self.swirl.is_some() && self.swirl_pipeline.is_some()
    }

    /// M294 — whether an equipment sheet's key has a slot in the atlas.
    pub fn armor_sheet_resident(&self, key: &str) -> bool {
        self.armor_slots.contains_key(key)
    }

    /// Vertices in the swirl range for the last [`Self::set_entities`], on
    /// one sheet (M287).
    pub fn swirl_vertex_count(&self, which: SwirlSheet) -> u32 {
        match which {
            SwirlSheet::Creeper => self.swirl_split,
            SwirlSheet::Wither => self.swirl_verts - self.swirl_split,
        }
    }

    /// M285 — the energy swirl, right after the emissive layers (vanilla's
    /// `order(1)`), in gamma space. Depth `GREATER_OR_EQUAL` with writes
    /// (`DepthStencilState.DEFAULT`), no culling, `BlendFunction.ADDITIVE`.
    pub fn draw_swirl(
        &self,
        gpu: &Gpu,
        cb: vk::CommandBuffer,
        view_proj: [[f32; 4]; 4],
        extent: vk::Extent2D,
    ) {
        let Some(pipeline) = self.swirl_pipeline else {
            return;
        };
        let base = self.solid_verts
            + self.text_verts
            + self.glint_verts
            + self.trim_verts
            + self.armor_glint_verts
            + self.emissive_verts
            + self.outline_verts;
        // M287 — the creeper's sub-range, then the wither's, each on its
        // own sheet.
        let parts = [
            (self.swirl.as_ref(), self.swirl_split, base),
            (
                self.wither_swirl.as_ref(),
                self.swirl_verts - self.swirl_split,
                base + self.swirl_split,
            ),
        ];
        for (sheet, count, first) in parts {
            let Some(g) = sheet else { continue };
            if count == 0 {
                continue;
            }
            unsafe {
                if !self.bind_common(gpu, cb, view_proj, extent) {
                    return;
                }
                let device = &gpu.device;
                device.cmd_bind_pipeline(cb, vk::PipelineBindPoint::GRAPHICS, pipeline);
                device.cmd_bind_descriptor_sets(
                    cb,
                    vk::PipelineBindPoint::GRAPHICS,
                    self.layout,
                    0,
                    &[g.set],
                    &[],
                );
                device.cmd_draw(cb, count, 1, first, 0);
            }
        }
    }

    pub fn init_glint(
        &mut self,
        gpu: &mut Gpu,
        rgba: &[u8],
        w: u32,
        h: u32,
        color_format: vk::Format,
    ) -> Result<(), String> {
        if self.glint.is_some() {
            return Ok(());
        }
        self.glint = Some(self.load_glint_sheet(gpu, rgba, w, h, color_format)?);
        Ok(())
    }

    /// The same, for `misc/enchanted_glint_armor.png` (M50) — a second sheet on
    /// the shared pipeline. Independent of [`Self::init_glint`]: a jar with one
    /// texture and not the other draws the glint it has.
    pub fn init_armor_glint(
        &mut self,
        gpu: &mut Gpu,
        rgba: &[u8],
        w: u32,
        h: u32,
        color_format: vk::Format,
    ) -> Result<(), String> {
        if self.armor_glint.is_some() {
            return Ok(());
        }
        self.armor_glint = Some(self.load_glint_sheet(gpu, rgba, w, h, color_format)?);
        Ok(())
    }

    /// Upload one glint sheet and point a descriptor set at it, building the
    /// shared pipeline on the first call.
    fn load_glint_sheet(
        &mut self,
        gpu: &mut Gpu,
        rgba: &[u8],
        w: u32,
        h: u32,
        color_format: vk::Format,
    ) -> Result<EntityGlint, String> {
        let sheet = self.load_sheet(gpu, rgba, w, h, color_format, vk::Filter::LINEAR)?;
        if self.glint_pipeline.is_none() {
            self.glint_pipeline = Some(build_glint_pipeline(
                &gpu.device,
                self.layout,
                color_format,
            )?);
        }
        Ok(sheet)
    }

    /// Upload one UNORM sheet with a REPEAT sampler at `filter` and point a
    /// descriptor set at it (the glints' LINEAR from their `blur` mcmeta;
    /// M285's swirl NEAREST, having none).
    fn load_sheet(
        &mut self,
        gpu: &mut Gpu,
        rgba: &[u8],
        w: u32,
        h: u32,
        _color_format: vk::Format,
        filter: vk::Filter,
    ) -> Result<EntityGlint, String> {
        let device = gpu.device.clone();
        let (image, image_alloc, view) = create_glint_texture(gpu, rgba, w, h)?;
        let sampler = unsafe {
            device
                .create_sampler(
                    &vk::SamplerCreateInfo::default()
                        .mag_filter(filter)
                        .min_filter(filter)
                        .address_mode_u(vk::SamplerAddressMode::REPEAT)
                        .address_mode_v(vk::SamplerAddressMode::REPEAT)
                        .address_mode_w(vk::SamplerAddressMode::REPEAT),
                    None,
                )
                .map_err(|e| format!("entity glint sampler: {e}"))?
        };
        let sizes = [vk::DescriptorPoolSize::default()
            .ty(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .descriptor_count(1)];
        let pool = unsafe {
            device
                .create_descriptor_pool(
                    &vk::DescriptorPoolCreateInfo::default()
                        .max_sets(1)
                        .pool_sizes(&sizes),
                    None,
                )
                .map_err(|e| format!("entity glint pool: {e}"))?
        };
        let set_layouts = [self.set_layout];
        let set = unsafe {
            device
                .allocate_descriptor_sets(
                    &vk::DescriptorSetAllocateInfo::default()
                        .descriptor_pool(pool)
                        .set_layouts(&set_layouts),
                )
                .map_err(|e| format!("entity glint set: {e}"))?[0]
        };
        let info = [vk::DescriptorImageInfo::default()
            .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
            .image_view(view)
            .sampler(sampler)];
        unsafe {
            device.update_descriptor_sets(
                &[vk::WriteDescriptorSet::default()
                    .dst_set(set)
                    .dst_binding(0)
                    .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                    .image_info(&info)],
                &[],
            );
        }
        Ok(EntityGlint {
            sampler,
            image,
            image_alloc: Some(image_alloc),
            view,
            pool,
            set,
        })
    }

    pub fn glint_ready(&self) -> bool {
        self.glint.is_some()
    }

    pub fn armor_glint_ready(&self) -> bool {
        self.armor_glint.is_some()
    }

    /// Vertices in the worn-armour foil range for the last [`Self::set_entities`].
    ///
    /// Exposed for `itemshot`, where "how much foil geometry was emitted" is
    /// the exact property two of vanilla's rules are about — one foil per
    /// piece however many layers it has, and none at all for the trim — and a
    /// count states them without the pixel confounds of an additive blend
    /// under an opaque decal. `handshot` counts its glint vertices the same way.
    pub fn armor_glint_vertex_count(&self) -> u32 {
        self.armor_glint_verts
    }

    /// The glint over held and dropped items. Drawn after the solid pass —
    /// its depth test is `EQUAL`, so the geometry it sits on has to be there
    /// already — and before the nametags, which are blended.
    pub fn draw_glint(
        &self,
        gpu: &Gpu,
        cb: vk::CommandBuffer,
        view_proj: [[f32; 4]; 4],
        extent: vk::Extent2D,
    ) {
        self.draw_one_glint(
            gpu,
            cb,
            view_proj,
            extent,
            self.glint.as_ref(),
            self.glint_verts,
            self.solid_verts + self.text_verts,
        );
    }

    /// The glint over **worn armour** (M50). Same pipeline and the same
    /// depth-`EQUAL` rule as the item glint — `RenderPipelines.GLINT` is
    /// `DepthStencilState(CompareOp.EQUAL, false)` and both glints use it — over
    /// a different sheet, at a different scale, from a different vertex range.
    ///
    /// Drawn **before** the trim: `renderLayers` submits layer, then foil, then
    /// the trim, and `SubmitNodeStorage` keeps its phases in an
    /// `Int2ObjectAVLTreeMap` keyed by that increasing `order`, so a trim's
    /// opaque texels paint over the foil rather than under it.
    ///
    /// The `VIEW_OFFSET_Z_LAYERING` those render types carry is **not** what
    /// separates the foil from the armour. `ARMOR_CUTOUT_NO_CULL`,
    /// `ARMOR_DECAL_CUTOUT_NO_CULL` and `ARMOR_ENTITY_GLINT` all set it, all
    /// with bias `1.0` on a fresh `getModelViewMatrixCopy()`, so it cancels
    /// exactly within the stack — which is the only reason an `EQUAL` test can
    /// work here at all. What it separates is the armour from the *body*, and
    /// Rewo's armour is inflated geometry that already wins that comparison.
    pub fn draw_armor_glint(
        &self,
        gpu: &Gpu,
        cb: vk::CommandBuffer,
        view_proj: [[f32; 4]; 4],
        extent: vk::Extent2D,
    ) {
        self.draw_one_glint(
            gpu,
            cb,
            view_proj,
            extent,
            self.armor_glint.as_ref(),
            self.armor_glint_verts,
            self.solid_verts + self.text_verts + self.glint_verts + self.trim_verts,
        );
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_one_glint(
        &self,
        gpu: &Gpu,
        cb: vk::CommandBuffer,
        view_proj: [[f32; 4]; 4],
        extent: vk::Extent2D,
        sheet: Option<&EntityGlint>,
        verts: u32,
        first: u32,
    ) {
        let (Some(g), Some(pipeline)) = (sheet, self.glint_pipeline) else {
            return;
        };
        if verts == 0 {
            return;
        }
        unsafe {
            if !self.bind_common(gpu, cb, view_proj, extent) {
                return;
            }
            let device = &gpu.device;
            device.cmd_bind_pipeline(cb, vk::PipelineBindPoint::GRAPHICS, pipeline);
            // The glint sheet, not the entity atlas — `bind_common` bound the
            // latter, so this overrides it for these draws only.
            device.cmd_bind_descriptor_sets(
                cb,
                vk::PipelineBindPoint::GRAPHICS,
                self.layout,
                0,
                &[g.set],
                &[],
            );
            device.cmd_draw(cb, verts, 1, first, 0);
        }
    }

    /// The armour trim (M48) — drawn after the solid pass, before the glint.
    ///
    /// Its pipeline depth-tests `EQUAL` and writes no depth, so it paints only
    /// where the armour it decorates already won. It samples the **entity
    /// atlas** like the armour does, so `bind_common`'s descriptor is right and
    /// there is no set to override.
    pub fn draw_trim(
        &self,
        gpu: &Gpu,
        cb: vk::CommandBuffer,
        view_proj: [[f32; 4]; 4],
        extent: vk::Extent2D,
    ) {
        let Some(pipeline) = self.trim_pipeline else {
            return;
        };
        if self.trim_verts == 0 {
            return;
        }
        unsafe {
            if !self.bind_common(gpu, cb, view_proj, extent) {
                return;
            }
            gpu.device
                .cmd_bind_pipeline(cb, vk::PipelineBindPoint::GRAPHICS, pipeline);
            gpu.device.cmd_draw(
                cb,
                self.trim_verts,
                1,
                self.solid_verts + self.text_verts + self.glint_verts,
                0,
            );
        }
    }

    /// Blended nametag text — draw last (after water).
    pub fn draw_text(
        &self,
        gpu: &Gpu,
        cb: vk::CommandBuffer,
        view_proj: [[f32; 4]; 4],
        extent: vk::Extent2D,
    ) {
        if self.text_verts == 0 {
            return;
        }
        unsafe {
            if !self.bind_common(gpu, cb, view_proj, extent) {
                return;
            }
            let device = &gpu.device;
            device.cmd_bind_pipeline(cb, vk::PipelineBindPoint::GRAPHICS, self.text_pipeline);
            device.cmd_draw(cb, self.text_verts, 1, self.solid_verts, 0);
        }
    }

    pub fn destroy(&mut self, gpu: &mut Gpu) {
        unsafe {
            let device = &gpu.device;
            // Both sheets, then the one pipeline they share. M48's leak was a
            // pipeline that outlived its `destroy` and showed up only as a
            // `vkDestroyDevice` VUID with every witness still green.
            for sheet in [
                self.glint.take(),
                self.armor_glint.take(),
                self.swirl.take(),
                self.wither_swirl.take(),
                self.wind.take(),
            ] {
                let Some(mut g) = sheet else { continue };
                device.destroy_descriptor_pool(g.pool, None);
                device.destroy_sampler(g.sampler, None);
                device.destroy_image_view(g.view, None);
                device.destroy_image(g.image, None);
                if let Some(a) = g.image_alloc.take() {
                    let _ = gpu.allocator.free(a);
                }
            }
            if let Some(p) = self.glint_pipeline.take() {
                device.destroy_pipeline(p, None);
            }
            if let Some(p) = self.swirl_pipeline.take() {
                device.destroy_pipeline(p, None);
            }
            if let Some(p) = self.wind_pipeline.take() {
                device.destroy_pipeline(p, None);
            }
            if let Some(p) = self.translucent_pipeline.take() {
                device.destroy_pipeline(p, None);
            }
            device.destroy_pipeline(self.water_mask_pipeline, None);
            device.destroy_pipeline(self.solid_pipeline, None);
            device.destroy_pipeline(self.text_pipeline, None);
            device.destroy_pipeline(self.emissive_pipeline, None);
            if let Some(p) = self.trim_pipeline.take() {
                device.destroy_pipeline(p, None);
            }
            device.destroy_pipeline_layout(self.layout, None);
            device.destroy_descriptor_pool(self.pool, None);
            device.destroy_descriptor_set_layout(self.set_layout, None);
            device.destroy_sampler(self.sampler, None);
            device.destroy_image_view(self.view, None);
            device.destroy_image(self.image, None);
        }
        self.ring.destroy(gpu);
        if let Some(a) = self.image_alloc.take() {
            let _ = gpu.allocator.free(a);
        }
    }
}

/// M285 — `MeshData.sortQuads` with `VertexSorting.DISTANCE_TO_ORIGIN` in
/// camera-relative space: each quad (six vertices here, `0 1 2 0 2 3`) keyed
/// by its centroid's squared distance from the camera, farthest first, by a
/// stable sort (`IntArrays.mergeSort`).
fn sort_quads_back_to_front(verts: &mut Vec<Vertex>, cam: [f32; 3]) {
    let mut quads: Vec<[Vertex; 6]> = verts.as_chunks::<6>().0.to_vec();
    let key = |q: &[Vertex; 6]| {
        let mut c = [0f32; 3];
        for v in [q[0], q[1], q[2], q[5]] {
            for (a, p) in c.iter_mut().zip(v.pos) {
                *a += p * 0.25;
            }
        }
        (0..3).map(|i| (c[i] - cam[i]).powi(2)).sum::<f32>()
    };
    quads.sort_by(|a, b| key(b).total_cmp(&key(a)));
    verts.clear();
    verts.extend(quads.into_iter().flatten());
}

/// Clamp the six storage-ordered vertex ranges (solid | text | glint | trim |
/// armor_glint | emissive) to the `kept` vertices that fit the ring slot. The
/// cut is a prefix, so the range it falls in is shortened (to whole triangles)
/// and every later range is empty; offsets derived by summing the earlier
/// counts stay correct.
fn clamp_ranges<const N: usize>(counts: [u32; N], kept: u32) -> [u32; N] {
    let mut left = kept;
    let mut out = [0u32; N];
    for (o, &c) in out.iter_mut().zip(&counts) {
        if c <= left {
            *o = c;
            left -= c;
        } else {
            *o = left / 3 * 3;
            left = 0;
        }
    }
    out
}

/// Unit capsule triangle soup: y ∈ [0, 1], xz ∈ [−0.5, 0.5]. Profile =
/// quarter-sphere cap (3 bands) + cylinder + cap, swept around Y.
fn unit_capsule() -> Vec<([f32; 3], [f32; 3])> {
    // (radius, y, n_radial, n_y) per profile row.
    let mut profile: Vec<(f32, f32, f32, f32)> = Vec::new();
    for i in 0..=3 {
        let th = (-90.0 + 30.0 * i as f32).to_radians();
        profile.push((0.5 * th.cos(), 0.25 * (1.0 + th.sin()), th.cos(), th.sin()));
    }
    profile.push((0.5, 0.75, 1.0, 0.0));
    for i in 1..=3 {
        let th = (30.0 * i as f32).to_radians();
        profile.push((0.5 * th.cos(), 0.75 + 0.25 * th.sin(), th.cos(), th.sin()));
    }

    let mut out = Vec::with_capacity((profile.len() - 1) * SEGMENTS * 6);
    let ring = |row: &(f32, f32, f32, f32), s: usize| -> ([f32; 3], [f32; 3]) {
        let a = (s as f32 / SEGMENTS as f32) * std::f32::consts::TAU;
        let (r, y, nr, ny) = *row;
        (
            [r * a.cos(), y, r * a.sin()],
            norm3([nr * a.cos(), ny, nr * a.sin()]),
        )
    };
    for band in profile.windows(2) {
        for s in 0..SEGMENTS {
            let (a0, a1) = (s, (s + 1) % SEGMENTS);
            let (p00, p10) = (ring(&band[0], a0), ring(&band[0], a1));
            let (p01, p11) = (ring(&band[1], a0), ring(&band[1], a1));
            out.extend_from_slice(&[p00, p10, p11, p00, p11, p01]);
        }
    }
    out
}

fn norm3(v: [f32; 3]) -> [f32; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-6);
    [v[0] / l, v[1] / l, v[2] / l]
}

// ---- per-part animation (vanilla setupAnim formulas) ---------------------

/// M365 — `AdultTurtleModel.setupAnim`: `if (eggBelly.visible) root.y--`,
/// the whole turtle 1 px up while it carries an egg.
fn turtle_lift(mob: &mobs::MobCombat) -> f32 {
    if mob.turtle_egg {
        -1.0
    } else {
        0.0
    }
}

/// Per-frame animation inputs, in vanilla's units: look angles in radians,
/// `f` = walkAnimationPos·0.6662, `pos` = raw walkAnimationPos, `amt` =
/// walkAnimationSpeed, `age` = ageInTicks (wall-clock seconds × 20).
struct AnimCtx {
    pitch: f32,
    net: f32,
    f: f32,
    pos: f32,
    amt: f32,
    age: f32,
    /// Active gesture + its age in seconds (one-shot rigs).
    gesture: Option<(mobs::Gesture, f32)>,
    /// Wire-event elapsed seconds per [`mobs::ModelEvent`] (`None` = inactive).
    events: [Option<f32>; mobs::ModelEvent::COUNT],
    /// Armadillo shell swap.
    shell: bool,
    /// M238 — the entity's body yaw, radians, for
    /// [`mobs::Anim::CounterBodyYaw`].
    body_yaw: f32,
    /// M270 — an armour stand's six poses, degrees, for
    /// [`mobs::Anim::StandPose`].
    stand_pose: [[f32; 3]; 6],
    /// Allay dance inputs (`Some` only for a dancing Allay) — drives
    /// [`mobs::Anim::AllayRoot`] / [`mobs::Anim::AllayHead`].
    allay_dance: Option<mobs::AllayDance>,
    /// Combat-swing pose — drives [`mobs::Anim::HumanoidBody`] /
    /// [`mobs::Anim::HumanoidArmRight`] / [`mobs::Anim::HumanoidArmLeft`].
    attack: mobs::SwingPose,
    /// `state.rightArmPose` / `state.leftArmPose` + handedness — the hold pose
    /// applied between the walk swing and the attack.
    arm_poses: mobs::ArmPoses,
    /// Synced mob state driving the undead / skeleton / illager arm rigs.
    mob: mobs::MobCombat,
    /// `Warden.getTendrilAnimation` — drives the tendril sway as well as the
    /// tendril emissive layer's alpha (M57).
    tendril: f32,
}

const DEG: f32 = std::f32::consts::PI / 180.0;

/// Vanilla `Mth.triangleWave`.
fn triangle_wave(a: f32, b: f32) -> f32 {
    ((a.rem_euclid(b) - b * 0.5).abs() - b * 0.25) / (b * 0.25)
}

/// Warden heartbeat period in ticks at anger 0 — vanilla `getHeartBeatDelay()
/// = 40 - floor(clamp(anger / ANGRY.minimumAnger, 0, 1) * 30)`. The client
/// anger level rides metadata we don't decode, so a calm warden's 40 ticks is
/// the constant; an angry one beats up to 4x faster in vanilla.
const WARDEN_HEARTBEAT_TICKS: f32 = 40.0;
/// Ticks a warden's heart/tendril countdown runs (`= 10`, decremented once per
/// client tick), and the divisor `getHeartAnimation` normalizes by.
const WARDEN_PULSE_TICKS: f32 = 10.0;

/// One emissive layer's alpha this frame — vanilla's `AlphaFunction` bodies,
/// with `age` in ticks (M57).
fn emissive_alpha(a: mobs::EmissiveAlpha, age: f32, state: &EmissiveState) -> f32 {
    match a {
        mobs::EmissiveAlpha::Always => 1.0,
        // `(warden, ageInTicks) -> max(0, cos(ageInTicks * 0.045 + phi) * 0.25)`.
        mobs::EmissiveAlpha::PulsatingSpots { phase } => {
            ((age * 0.045 + phase).cos() * 0.25).max(0.0)
        }
        mobs::EmissiveAlpha::Tendril => state.tendril.clamp(0.0, 1.0),
        // `Warden.tick` sets `heartAnimation = 10` every heartbeat delay and
        // decrements it each tick; `getHeartAnimation` lerps between the
        // previous and current value and divides by 10 — which for a continuous
        // clock is exactly `max(0, 10 - (age mod delay)) / 10`. The phase here
        // is the *world* clock rather than the entity's own tickCount (we do
        // not track spawn ticks), so a herd of wardens would beat in unison;
        // one warden is indistinguishable.
        mobs::EmissiveAlpha::Heart => {
            ((WARDEN_PULSE_TICKS - age.rem_euclid(WARDEN_HEARTBEAT_TICKS)) / WARDEN_PULSE_TICKS)
                .max(0.0)
        }
        mobs::EmissiveAlpha::EyesGlowing => {
            if state.eyes_glow {
                1.0
            } else {
                0.0
            }
        }
    }
}

/// Vanilla `Mth`'s sine-table scale: `65536 / 2π`.
pub const MTH_SIN_SCALE: f64 = 10430.378350470453;

/// `Mth.SIN` — 65,536 entries of `(float)Math.sin(i / 10430.378350470453)`.
///
/// `libm::sin` (fdlibm) matches Java's `Math.sin` bit-for-bit where the
/// platform libm drifts, the same discipline M12's star geometry relies on.
fn mth_table() -> &'static [f32; 65536] {
    static TABLE: std::sync::OnceLock<Box<[f32; 65536]>> = std::sync::OnceLock::new();
    TABLE.get_or_init(|| {
        let mut t = Box::new([0.0f32; 65536]);
        for (i, v) in t.iter_mut().enumerate() {
            *v = libm::sin(i as f64 / MTH_SIN_SCALE) as f32;
        }
        t
    })
}

/// `Mth.sin(double)` — `SIN[(int)((long)(x · scale) & 65535L)]`.
///
/// This is a **quantized** sine, not a rounded one: the index truncates, so the
/// result can sit up to a full table step (≈9.59e-5 rad of argument) from the
/// true value. Reproducing the quantization rather than tolerating it is what
/// makes the attack pose bit-comparable with vanilla instead of merely close.
pub fn mth_sin(x: f64) -> f32 {
    mth_table()[(((x * MTH_SIN_SCALE) as i64) & 65535) as usize]
}

/// `Mth.cos(double)` — the same table read a quarter-turn along.
pub fn mth_cos(x: f64) -> f32 {
    mth_table()[(((x * MTH_SIN_SCALE + 16384.0) as i64) & 65535) as usize]
}

/// `Mth.sqrt(float)` — `(float)Math.sqrt(x)`, i.e. the *double* square root
/// narrowed, which is not always the correctly-rounded single-precision one.
pub fn mth_sqrt(x: f32) -> f32 {
    libm::sqrt(x as f64) as f32
}

// ---- HumanoidModel.setupAttackAnimation (M19) ----------------------------
//
// Vanilla *assigns* `body.yRot` and the two arms' `x` / `z` and then *adds* to
// their rotations. This pose pipeline is additive over a rest pose whose body
// rotation is zero and whose arm pivots are `(∓5, 2, 0)`, so an assignment
// becomes `value` and an assignment to a pivot becomes `value − base`. That is
// exactly why only models built from `HumanoidModel.createMesh` may carry the
// `Humanoid*` anims (see [`mobs::Anim::HumanoidArmRight`]).

/// `body.yRot = Mth.sin(Mth.sqrt(attackTime) · 2π) · 0.2`, negated when the
/// attack arm is the left one. Shared by the body and both arms.
///
/// The inner product is computed in `f32` (Java multiplies two floats) and only
/// then widened for the table lookup, exactly as `Mth.sin(float)` does.
fn attack_body_yrot(a: &mobs::SwingPose) -> f32 {
    let arg = mth_sqrt(a.attack_time) * std::f32::consts::TAU;
    let y = mth_sin(arg as f64) * 0.2;
    if a.left_arm {
        -y
    } else {
        y
    }
}

/// `SpearAnimations.progress` — `clamp(Mth.inverseLerp(t, start, end), 0, 1)`.
pub(crate) fn spear_progress(t: f32, start: f32, end: f32) -> f32 {
    ((t - start) / (end - start)).clamp(0.0, 1.0)
}

/// `Ease.outQuart(x) = 1 − square(square(1 − x))`.
fn ease_out_quart(x: f32) -> f32 {
    let s = (1.0 - x) * (1.0 - x);
    1.0 - s * s
}

/// `Ease.inOutSine(x) = −(Mth.cos(π·x) − 1) / 2` — `Mth.cos`, not libm's.
pub(crate) fn ease_in_out_sine(x: f32) -> f32 {
    -(mth_cos((std::f32::consts::PI * x) as f64) - 1.0) / 2.0
}

/// `Ease.outBack(x) = 1 + c3·(x−1)³ + c1·(x−1)²` with `c1 = 1.70158` and
/// `c3 = c1 + 1`. The overshoot past 1 is the point — it is what makes the
/// spear's mid-thrust read as a lunge rather than a slide.
pub(crate) fn ease_out_back(x: f32) -> f32 {
    const C1: f32 = 1.70158;
    const C3: f32 = 2.70158;
    let d = x - 1.0;
    1.0 + C3 * d * d * d + C1 * d * d
}

/// `Ease.inQuad(x) = x·x`.
fn ease_in_quad(x: f32) -> f32 {
    x * x
}

/// `Ease.inOutExpo` — the exact two-branch form, including its `x == 0` /
/// `x == 1` special cases and its `double`-precision `Math.pow`.
pub(crate) fn ease_in_out_expo(x: f32) -> f32 {
    if x < 0.5 {
        if x == 0.0 {
            0.0
        } else {
            (2.0f64.powf(20.0 * x as f64 - 10.0) / 2.0) as f32
        }
    } else if x == 1.0 {
        1.0
    } else {
        ((2.0 - 2.0f64.powf(-20.0 * x as f64 + 10.0)) / 2.0) as f32
    }
}

/// One part's `setupAnim` output: Euler deltas summed onto the base pose
/// (composed as a single `rotateZYX`, exactly vanilla) + a pivot offset for
/// the anims that move parts (blaze rods, crawler segments).
fn anim_delta(anim: mobs::Anim, amp: f32, c: &AnimCtx) -> ([f32; 3], [f32; 3]) {
    let (mut rot, mut off) = anim_delta_base(anim, amp, c);
    // M402 — `HumanoidModel.setupAnim`: `if (fallFlying) head.xRot = −π/4`,
    // assigned over the look before anything else reads it.
    if c.mob.fall_flying && anim == mobs::Anim::Head {
        rot[0] = -std::f32::consts::FRAC_PI_4;
    }
    // M399 — `HumanoidModel.setupAnim`'s crouch block (after the attack,
    // before the bob, whose additions commute with these): the body bends
    // 0.5 forward and drops 3.2 with the arms, which also swing 0.4
    // forward; the legs step back 4 and the head drops 4.2. Only an avatar
    // sets `crouching`, so the shared head and leg anims see it only there.
    if c.mob.crouching {
        use mobs::Anim::*;
        match anim {
            HumanoidBody => {
                rot[0] = 0.5;
                off[1] += 3.2;
            }
            HumanoidArmRight | HumanoidArmLeft => {
                rot[0] += 0.4;
                off[1] += 3.2;
            }
            LegRight | LegLeft => off[2] += 4.0,
            Head => off[1] += 4.2,
            _ => {}
        }
    }
    // M400 — `HumanoidModel.setupAnim`'s swim (after the bob): the head
    // pitches toward −π/4, the arms lerp into the stroke, the legs kick.
    if c.mob.swim_amount > 0.0 {
        humanoid_swim(anim, &mut rot, c);
    }
    (rot, off)
}

/// `Mth.rotLerpRad(a, from, to)`: `from + a·diff` with `diff = to − from`
/// wrapped into `[−π, π)` by its two loops (so a difference of exactly π
/// turns to −π, as vanilla's does).
fn rot_lerp_rad(a: f32, from: f32, to: f32) -> f32 {
    use std::f32::consts::{PI, TAU};
    let mut d = to - from;
    while d < -PI {
        d += TAU;
    }
    while d >= PI {
        d -= TAU;
    }
    from + a * d
}

/// M400 — `HumanoidModel.setupAnim`'s swim block on one part.
fn humanoid_swim(anim: mobs::Anim, rot: &mut [f32; 3], c: &AnimCtx) {
    use mobs::Anim::*;
    use std::f32::consts::PI;
    let swim = c.mob.swim_amount;
    let lerp = |a: f32, from: f32, to: f32| from + a * (to - from);
    match anim {
        // M402 — `else if (swimAmount > 0)`: a glider's head stays pinned.
        Head if c.mob.fall_flying => {}
        Head => rot[0] = rot_lerp_rad(swim, rot[0], -PI / 4.0),
        LegLeft => {
            rot[0] = lerp(
                swim,
                rot[0],
                0.3 * mth_cos((c.pos * 0.333_333_34 + PI) as f64),
            )
        }
        LegRight => rot[0] = lerp(swim, rot[0], 0.3 * mth_cos((c.pos * 0.333_333_34) as f64)),
        HumanoidArmRight | HumanoidArmLeft => {
            let left = anim == HumanoidArmLeft;
            if c.arm_poses.using_item {
                return;
            }
            let pose = if left {
                c.arm_poses.left
            } else {
                c.arm_poses.right
            };
            let attacking = c.attack.attack_time > 0.0 && c.attack.left_arm == left;
            let a = if pose != mobs::ArmPose::Spear && !attacking {
                swim
            } else {
                0.0
            };
            // The left arm lerps by `rotLerpRad`, the right by `lerp`.
            let l = |from: f32, to: f32| {
                if left {
                    rot_lerp_rad(a, from, to)
                } else {
                    lerp(a, from, to)
                }
            };
            let quad = |x: f32| -65.0 * x + x * x;
            let swim_pos = c.pos % 26.0;
            let (x, z) = if swim_pos < 14.0 {
                let k = 1.870_796_4 * quad(swim_pos) / quad(14.0);
                (0.0, if left { PI + k } else { PI - k })
            } else if swim_pos < 22.0 {
                let t = (swim_pos - 14.0) / 8.0;
                (
                    PI / 2.0 * t,
                    if left {
                        5.012_389 - 1.870_796_4 * t
                    } else {
                        1.270_796_3 + 1.870_796_4 * t
                    },
                )
            } else {
                let t = (swim_pos - 22.0) / 4.0;
                (PI / 2.0 - PI / 2.0 * t, PI)
            };
            rot[0] = l(rot[0], x);
            rot[1] = l(rot[1], PI);
            rot[2] = l(rot[2], z);
        }
        _ => {}
    }
}

/// M401 — `setupRotations`' sleeping chain on an entity-local point:
/// `YP(angle) · ZP(90) · YP(270)` (the point turns by `YP(270)` first).
pub fn sleeping_point(e: [f32; 3], angle_deg: f32) -> [f32; 3] {
    let yp = |p: [f32; 3], deg: f32| {
        let (s, c) = (deg * DEG).sin_cos();
        [p[0] * c + p[2] * s, p[1], -p[0] * s + p[2] * c]
    };
    let p = yp(e, 270.0);
    let p = [-p[1], p[0], p[2]];
    yp(p, angle_deg)
}

/// M401 — `LivingEntityRenderer.sleepDirectionToRotation`: south 90, west
/// 0, north 270, east 180 (anything else 0).
pub fn sleep_direction_rotation(facing: &str) -> f32 {
    match facing {
        "south" => 90.0,
        "north" => 270.0,
        "east" => 180.0,
        _ => 0.0,
    }
}

/// M402 — an entity's render frame: everything `LivingEntityRenderer.submit`
/// and `setupRotations` do between a posed model's px and the world, in one
/// place. The body, its armour and trims, the cape, a worn head and the held
/// items all hang off the same pose stack in vanilla, so they all place
/// their points through this; until M402 each layer carried its own copy of
/// the chain with only the death topple in it, and a swimming or sleeping
/// player's armour, cape and held items stood upright beside the body.
#[derive(Clone, Copy, Debug)]
pub struct EntityFrame {
    pos: [f32; 3],
    /// The model's px→block scale times the per-entity size.
    scale: f32,
    stretch: [f32; 2],
    /// `AvatarRenderer.setupRotations`' glide (`YP(flyingYRot)`, then
    /// `XP(tilt)`), outermost of the avatar's own rotations.
    glide: Option<YxRotation>,
    /// The swim's `XP(tilt)` (sine, cosine) after its `translate`.
    swim: Option<(f32, f32, [f32; 3])>,
    inner: InnerRotation,
    /// M409 — the dragon renderer's `XP(pitch)` (sine, cosine), before the
    /// yaw.
    pitch: Option<(f32, f32)>,
    /// `YP(180 − bodyRot)` (sine, cosine).
    yaw: (f32, f32),
    mount: Option<[[f32; 4]; 3]>,
}

/// `LivingEntityRenderer.setupRotations`' `if (deathTime > 0) … else if
/// (isAutoSpinAttack) … else if (SLEEPING) …` chain.
#[derive(Clone, Copy, Debug)]
enum InnerRotation {
    /// `ZP(fall · flipDegrees)` (sine, cosine): identity while alive.
    Topple(f32, f32),
    Spin(YxRotation),
    /// The bed's angle in degrees: replaces the body yaw.
    Sleep(f32),
}

impl EntityFrame {
    /// `time` is the draw's animation clock in seconds (`ageInTicks / 20`).
    pub fn new(d: &EntityDraw<'_>, scale: f32, time: f32) -> Self {
        let theta = (180.0 - d.yaw).to_radians();
        let inner = if d.death_time > 0.0 {
            let (sr, cr) = death_roll(d).sin_cos();
            InnerRotation::Topple(sr, cr)
        } else if d.mob.auto_spin {
            InnerRotation::Spin(spin_rotations(d.pitch, time * 20.0))
        } else if let Some(angle) = d.mob.sleep_angle {
            InnerRotation::Sleep(angle)
        } else {
            InnerRotation::Topple(0.0, 1.0)
        };
        let swim = (d.mob.swim_amount > 0.0 && !d.mob.fall_flying).then(|| {
            let (sw, cw) =
                (swim_tilt(d.mob.swim_amount, d.mob.swim_in_water, d.pitch) * DEG).sin_cos();
            let t = if d.mob.swim_visual {
                [0.0, -1.0 / scale, 0.3 / scale]
            } else {
                [0.0; 3]
            };
            (sw, cw, t)
        });
        Self {
            pos: d.pos,
            scale,
            stretch: d.stretch,
            glide: glide_rotations(&d.mob),
            swim,
            inner,
            pitch: (d.mob.body_pitch != 0.0).then(|| (d.mob.body_pitch * DEG).sin_cos()),
            yaw: theta.sin_cos(),
            mount: d.mount,
        }
    }

    /// M413 — whether the chain is the plain yaw (a death topple aside):
    /// no glide, swim, spin, sleep or pitch.
    pub fn upright(&self) -> bool {
        self.glide.is_none()
            && self.swim.is_none()
            && self.pitch.is_none()
            && matches!(self.inner, InnerRotation::Topple(..))
    }

    /// A posed model point (px, model space, y down) in the world.
    pub fn place(&self, v: [f32; 3]) -> [f32; 3] {
        // model → entity local (px): scale(−1,−1,1) after the −1.501-block
        // translate.
        let e = [-v[0], mobs::MODEL_EYE_Y - v[1], v[2]];
        let e = [
            e[0] * self.stretch[0],
            e[1] * self.stretch[1],
            e[2] * self.stretch[0],
        ];
        // M402 — the glide: `YP(flyingYRot)` then `XP(tilt)`.
        let e = match &self.glide {
            Some(g) => g.apply(e),
            None => e,
        };
        // M400 — the swim: `XP(tilt)` after `translate(0, −1, 0.3)` (blocks:
        // `/ s` in these px).
        let e = match self.swim {
            Some((sw, cw, t)) => {
                let (y, z) = (e[1] + t[1], e[2] + t[2]);
                [e[0] + t[0], y * cw - z * sw, y * sw + z * cw]
            }
            None => e,
        };
        let (st, ct) = self.yaw;
        let pitched = |e: [f32; 3]| match self.pitch {
            Some((s, c)) => [e[0], e[1] * c - e[2] * s, e[1] * s + e[2] * c],
            None => e,
        };
        let yawed = |e: [f32; 3]| {
            let e = pitched(e);
            [e[0] * ct + e[2] * st, e[1], -e[0] * st + e[2] * ct]
        };
        let w = match self.inner {
            InnerRotation::Topple(sr, cr) => {
                yawed([e[0] * cr - e[1] * sr, e[0] * sr + e[1] * cr, e[2]])
            }
            InnerRotation::Spin(r) => yawed(r.apply(e)),
            // M401 — `YP(angle) · ZP(90) · YP(270)` in place of the yaw.
            InnerRotation::Sleep(angle) => sleeping_point(e, angle),
        };
        // Feet-relative, in block units. A *mounted* entity — a spawner's
        // caged mob — runs that through one more affine before the block
        // position places it (M31).
        let mut l = [w[0] * self.scale, w[1] * self.scale, w[2] * self.scale];
        if let Some(m) = &self.mount {
            l = [
                m[0][0] * l[0] + m[0][1] * l[1] + m[0][2] * l[2] + m[0][3],
                m[1][0] * l[0] + m[1][1] * l[1] + m[1][2] * l[2] + m[1][3],
                m[2][0] * l[0] + m[2][1] * l[1] + m[2][2] * l[2] + m[2][3],
            ];
        }
        [self.pos[0] + l[0], self.pos[1] + l[1], self.pos[2] + l[2]]
    }
}

/// M412 — fog words that fog nothing (both bands start a billion blocks
/// out).
pub const NO_FOG: [[f32; 4]; 3] = [
    [0.0, 0.0, 0.0, 1.0e9],
    [0.0, 0.0, 0.0, 1.0e9 + 1.0],
    [1.0e9, 1.0e9 + 1.0, 0.0, 0.0],
];

/// M410 — `submitNameDisplay`'s `translate(0, 9·1.15·0.025, 0)`: how far
/// the name rises over a score line, in blocks.
pub const SCORE_LINE_LIFT: f32 = 9.0 * 1.15 * 0.025;

/// M402 — two rotations a point runs through in order, the first about Y
/// and the second about X (radians, as sine/cosine pairs): what a pose
/// stack's `XP(x) · YP(y)` does to a point.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct YxRotation {
    y: Option<(f32, f32)>,
    x: (f32, f32),
}

impl YxRotation {
    pub fn new(y_rad: Option<f32>, x_rad: f32) -> Self {
        Self {
            y: y_rad.map(f32::sin_cos),
            x: x_rad.sin_cos(),
        }
    }

    /// The point in the entity's local px frame, `YP` first, then `XP`.
    pub fn apply(&self, e: [f32; 3]) -> [f32; 3] {
        let e = match self.y {
            Some((s, c)) => [e[0] * c + e[2] * s, e[1], -e[0] * s + e[2] * c],
            Option::None => e,
        };
        let (s, c) = self.x;
        [e[0], e[1] * c - e[2] * s, e[1] * s + e[2] * c]
    }
}

/// M402 — `AvatarRenderer.setupRotations`' fall-flying branch:
/// `XP(fallFlyingScale · (−90 − xRot))` (skipped during a riptide spin) and
/// `YP(flyingYRot)`, or `None` when the entity is not gliding.
pub fn glide_rotations(mob: &mobs::MobCombat) -> Option<YxRotation> {
    let gliding = mob.fall_flying && (mob.glide_tilt != 0.0 || mob.flying_y_rot.is_some());
    gliding.then(|| YxRotation::new(mob.flying_y_rot, mob.glide_tilt.to_radians()))
}

/// M402 — `LivingEntityRenderer.setupRotations`' riptide spin:
/// `XP(−90 − xRot) · YP(ageInTicks · −75)`, the body laid along the look
/// and turning about its own length.
pub fn spin_rotations(pitch_deg: f32, age_ticks: f32) -> YxRotation {
    YxRotation::new(
        Some((age_ticks * -75.0).to_radians()),
        (-90.0 - pitch_deg).to_radians(),
    )
}

/// M400 — `AvatarRenderer.setupRotations`' swim tilt in degrees,
/// `lerp(swimAmount, 0, isInWater ? −90 − xRot : −90)`, and whether the
/// `translate(0, −1, 0.3)` applies (`isVisuallySwimming`).
pub fn swim_tilt(swim: f32, in_water: bool, pitch_deg: f32) -> f32 {
    let target = if in_water { -90.0 - pitch_deg } else { -90.0 };
    swim * target
}

fn anim_delta_base(anim: mobs::Anim, amp: f32, c: &AnimCtx) -> ([f32; 3], [f32; 3]) {
    use mobs::Anim::*;
    use std::f32::consts::PI;
    let mut rot = [0.0f32; 3];
    let mut off = [0.0f32; 3];
    match anim {
        None => {}
        CounterBodyYaw => rot[1] = -c.body_yaw,
        StandPose(i) => {
            let i = usize::from(i).min(5);
            for (k, r) in rot.iter_mut().enumerate() {
                *r = (c.stand_pose[i][k] - mobs::STAND_DEFAULT_POSE[i][k]).to_radians();
            }
        }
        Head => {
            rot[0] = c.pitch;
            rot[1] = c.net;
        }
        HeadYaw => rot[1] = c.net,
        ArmRight => rot[0] = (c.f + PI).cos() * 2.0 * c.amt * 0.5 * amp,
        ArmLeft => rot[0] = c.f.cos() * 2.0 * c.amt * 0.5 * amp,
        // M352 — `EndermanModel.setupAnim`.
        EndermanArmRight | EndermanArmLeft => {
            let left = anim == EndermanArmLeft;
            if c.mob.carried_block.is_some() {
                rot[0] = -0.5;
                rot[2] = if left { -0.05 } else { 0.05 };
            } else {
                // M379 — `HumanoidModel`'s `bobModelPart` (scale +1 right, −1
                // left) lands before `EndermanModel` halves and clamps the
                // `xRot`; its `zRot` stays whole.
                let phase = if left { 0.0 } else { PI };
                let k = if left { -1.0 } else { 1.0 };
                let bob_x = k * (c.age * 0.067).sin() * 0.05;
                rot[0] = (((c.f + phase).cos() * 2.0 * c.amt * 0.5 + bob_x) * 0.5).clamp(-0.4, 0.4);
                rot[2] = k * ((c.age * 0.09).cos() * 0.05 + 0.05);
            }
        }
        EndermanLegRight => rot[0] = (c.f.cos() * 1.4 * c.amt * 0.5).clamp(-0.4, 0.4),
        EndermanLegLeft => rot[0] = ((c.f + PI).cos() * 1.4 * c.amt * 0.5).clamp(-0.4, 0.4),
        // M381 — `HumanoidModel` / `IllagerModel` riding: the legs splay
        // forward (assigned over the walk).
        LegRight | LegLeft if c.mob.riding => {
            let s = if anim == LegRight { 1.0 } else { -1.0 };
            rot = [-1.4137167, s * PI / 10.0, s * 0.07853982];
        }
        // M402 — the humanoid legs' swing is `/ state.speedValue`; the
        // divisor is 1 for every kind that cannot be fall flying.
        LegRight | QuadHindRight | QuadFrontLeft => {
            rot[0] = c.f.cos() * 1.4 * c.amt * amp / c.mob.walk_divisor()
        }
        LegLeft | QuadHindLeft | QuadFrontRight => {
            rot[0] = (c.f + PI).cos() * 1.4 * c.amt * amp / c.mob.walk_divisor()
        }
        SpiderLeg { left, phase } => {
            let swing = -((c.f * 2.0 + phase).cos()) * 0.4 * c.amt;
            let step = (c.f + phase).sin().abs() * 0.4 * c.amt;
            let dir = if left { -1.0 } else { 1.0 };
            rot[1] = dir * swing;
            rot[2] = dir * step;
        }
        TailWagY => {
            // M355 — `WolfModel.setupAnim`: no wag while angry; the tail's
            // `xRot` is `state.tailAngle` (the part rests at π/5).
            if !c.mob.wolf_angry {
                rot[1] = c.f.cos() * 1.4 * c.amt;
            }
            if let Some(a) = c.mob.wolf_tail {
                rot[0] = a - PI / 5.0;
            }
            // M356 — and `setSittingPose`'s `tail.y += 9`, `z -= 2`.
            if c.mob.sitting {
                let s = c.mob.age_scale;
                off = [0.0, 9.0 * s, -2.0 * s];
            }
        }
        // M359 — `AbstractEquineModel.setupAnim` (not eating: Rewo draws
        // none). M417 — rearing (`standing`, the adults' constants; the
        // babies are static): the head group tips back to π/12 over the
        // look and moves `(0, −8, +8)·standing` (`animateHeadPartsPlacement`:
        // `y += lerp(s, 0, −8)`, `z = lerp(s, z, −4)` from its −12), the
        // body rears −π/4, the front legs rise 12 and come forward 4 and
        // paw at `−π/3 ± cos(age·0.6 + π)`, the hind legs brace at π/12;
        // each walk term fades by `1 − standing`.
        // M426 — and grazing: `xRot = standing·(π/12 + look) + eating·
        // (125° + sin(age)·0.05) + (1 − max(standing, eating))·(π/6 + look +
        // feeding·sin(age)·0.05)`, the yaw faded by the same factor outside
        // rearing, the group lowered `lerp(eating, −8·standing, 7)`.
        EquineHead | EquineHeadFoal => {
            let (s, e) = (c.mob.stand_anim, c.mob.eat_anim);
            let look = c.pitch
                + if c.amt > 0.2 {
                    (c.pos * 0.8).cos() * 0.15 * c.amt
                } else {
                    0.0
                };
            let wobble = c.age.sin() * 0.05;
            let free = 1.0 - s.max(e);
            let x = s * (PI / 12.0 + look)
                + e * (2.181_661_6 + wobble)
                + free * (PI / 6.0 + look + c.mob.feeding_anim * wobble);
            rot[0] = x - PI / 6.0;
            let yaw = c.net.clamp(-20f32.to_radians(), 20f32.to_radians());
            rot[1] = s * yaw + free * yaw;
            // `animateHeadPartsPlacement` (M428: the foal's own).
            off = if anim == EquineHeadFoal {
                [0.0, -2.0 * s + e * (2.0 + 2.0 * s), 2.0 * s]
            } else {
                [0.0, -8.0 * s + e * (7.0 + 8.0 * s), 8.0 * s]
            };
        }
        EquineBody => rot[0] = c.mob.stand_anim * -PI / 4.0,
        // M429 — `BabyDonkeyModel.setupAnim` over `AbstractEquineModel`'s:
        // `xRot = standing·(π/12 − π/6) + eating·(π/2 + wobble) + (1 −
        // max)·(π/6 − π/6 + feeding·wobble)` over a rest of 0, the yaw as
        // the adult's, the group at `lerp(eating, −3, −1.2)` and
        // `lerp(standing, −5, −3.6)`.
        EquineHeadDonkeyFoal => {
            let (s, e) = (c.mob.stand_anim, c.mob.eat_anim);
            let wobble = c.age.sin() * 0.05;
            let free = 1.0 - s.max(e);
            rot[0] = -s * PI / 12.0 + e * (PI / 2.0 + wobble) + free * c.mob.feeding_anim * wobble;
            let yaw = c.net.clamp(-20f32.to_radians(), 20f32.to_radians());
            rot[1] = s * yaw + free * yaw;
            off = [0.0, e * 1.8, s * 1.4];
        }
        EquineLegDonkeyFoal { hind, left } => {
            let s = c.mob.stand_anim;
            let walk = 1.0 - s;
            let leg = (c.f + PI).cos();
            let bob = (c.age * 0.6 + PI).cos();
            rot[0] = match (hind, left) {
                (false, true) => bob * s + leg * 0.8 * c.amt * walk,
                (false, false) => -bob * s - leg * 0.8 * c.amt * walk,
                (true, true) => PI / 3.0 * s - leg * 0.5 * c.amt * walk,
                (true, false) => PI / 3.0 * s + leg * 0.5 * c.amt * walk,
            };
            // Rest heights 3.5; the left front leg's z rest is −5.3, the
            // right's −5.4 (it takes the left's every frame).
            let left_hind = 3.5 + s * (-0.3 - 3.5);
            off = match (hind, left) {
                (false, true) => [0.0, -s, 0.5 * s],
                (false, false) => [0.0, -s, 0.1 + 0.5 * s],
                (true, true) => [0.0, left_hind - 3.5, 0.0],
                (true, false) => [0.0, left_hind + s * (-0.3 - left_hind) - 3.5, 0.0],
            };
        }
        EquineLeg { hind, left } | EquineLegFoal { hind, left } => {
            let s = c.mob.stand_anim;
            let walk = 1.0 - s;
            let leg = (c.f + PI).cos();
            let bob = (c.age * 0.6 + PI).cos();
            rot[0] = match (hind, left) {
                (false, true) => (-PI / 3.0 + bob) * s + leg * 0.8 * c.amt * walk,
                (false, false) => (-PI / 3.0 - bob) * s - leg * 0.8 * c.amt * walk,
                (true, true) => PI / 12.0 * s - leg * 0.5 * c.amt * walk,
                (true, false) => PI / 12.0 * s + leg * 0.5 * c.amt * walk,
            };
            // `getLegStandingY/ZOffset` (M428: the foal's 4 and 0).
            if !hind {
                off = if matches!(anim, EquineLegFoal { .. }) {
                    [0.0, -4.0 * s, 0.0]
                } else {
                    [0.0, -12.0 * s, 4.0 * s]
                };
            }
        }
        EquineTail | EquineTailFoal => {
            rot[0] = c.amt * 0.75;
            // × `ageScale` (M428: the foal's 0.5).
            let k = if anim == EquineTailFoal { 0.5 } else { 1.0 };
            off = [0.0, c.amt * k, 2.0 * c.amt * k];
        }
        // M365 — `TurtleModel.setupAnim` over `QuadrupedModel.setupAnim`.
        TurtleHead => {
            rot[0] = c.pitch;
            rot[1] = c.net;
            off[1] = turtle_lift(&c.mob);
        }
        TurtleBody => off[1] = turtle_lift(&c.mob),
        // M368 — `SnowGolemModel.setupAnim`.
        SnowGolemBody => rot[1] = c.net * 0.25,
        SnowGolemArm { left } => {
            let u = c.net * 0.25;
            rot[1] = u;
            let (sin, cos) = u.sin_cos();
            off = if left {
                [cos * 5.0 - 5.0, 0.0, -sin * 5.0]
            } else {
                [5.0 - cos * 5.0, 0.0, sin * 5.0]
            };
        }
        // M367 — `HoglinModel.setupAnim` (and `BabyHoglinModel`'s headbutt).
        HoglinHead { baby } => {
            rot[1] = c.net;
            let hb = c.mob.headbutt;
            rot[0] = hb * (-PI / 9.0 - 0.872_664_6);
            if baby {
                off[1] = hb * 2.5;
            }
        }
        HoglinEar { left } => {
            let flap = c.amt * c.pos.sin();
            rot[2] = if left { flap } else { -flap };
        }
        HoglinLeg { front, left } => {
            let phase = if front == left { PI } else { 0.0 };
            rot[0] = (c.pos + phase).cos() * 1.2 * c.amt;
        }
        // M366 — `StriderModel.setupAnim` and `customAnimations`.
        StriderBody { adult } => {
            let s = c.amt.min(0.25);
            if !c.mob.ridden {
                rot[0] = c.pitch;
                rot[1] = c.net;
            }
            rot[2] = 0.1 * (c.pos * 1.5).sin() * 4.0 * s;
            let k = if adult { 2.0 } else { 1.0 };
            off[1] = -k * (c.pos * 1.5).cos() * 2.0 * s;
        }
        StriderLeg { left } => {
            let s = c.amt.min(0.25);
            let h = c.pos * 1.5 * 0.5;
            let (swing, bob) = if left { (h, h + PI) } else { (h + PI, h) };
            rot[0] = swing.sin() * 2.0 * s;
            rot[2] = PI / 18.0 * swing.cos() * s;
            off[1] = 2.0 * bob.sin() * 2.0 * s;
        }
        StriderBristle { order, baby } => {
            let s = c.amt.min(0.25);
            let flow = (c.pos * 1.5 + PI).cos() * s;
            let (mult, sway) = match order {
                0 => (0.6, 0.1 * (c.age * 0.4).sin()),
                1 => (1.2, 0.1 * (c.age * 0.2).sin()),
                _ => (1.3, 0.05 * (c.age * -0.4).sin()),
            };
            let add = flow * mult + sway;
            if baby {
                rot[0] = add;
            } else {
                rot[2] = add;
            }
        }
        TurtleLeg { hind, left } => {
            // The quadruped's swing: right hind and left front on cos(f),
            // the other two half a cycle on.
            let phase = if hind == left { PI } else { 0.0 };
            rot[0] = (c.f + phase).cos() * 1.4 * c.amt;
            let sign = if left { 1.0 } else { -1.0 };
            if c.mob.turtle_on_land {
                let (lay, lay_amp) = if c.mob.turtle_laying {
                    (4.0, 2.0)
                } else {
                    (1.0, 1.0)
                };
                let swing_pos = c.pos * 5.0;
                rot[1] = if hind {
                    sign * swing_pos.cos() * 3.0 * c.amt
                } else {
                    sign * (lay * swing_pos).cos() * 8.0 * c.amt * lay_amp
                };
            } else {
                let swing = (c.pos * 0.6662 * 0.6).cos() * 0.5 * c.amt;
                if hind {
                    rot[0] = -sign * swing;
                } else {
                    rot[2] = sign * swing;
                }
            }
            off[1] = turtle_lift(&c.mob);
        }
        // M358 — `SheepModel.setupAnim`: grazing, the eat scales; else the
        // look.
        SheepHead => {
            rot[1] = c.net;
            match c.mob.sheep_eat {
                Some((position, angle)) => {
                    rot[0] = angle;
                    off[1] = position * 9.0 * c.mob.age_scale;
                }
                Option::None => rot[0] = c.pitch,
            }
        }
        // M357 — `AdultFelineModel` / `BabyFelineModel.setupAnim`. A kitten
        // is the age scale's half; its sitting pose is its own.
        FelineHead => {
            rot[0] = c.pitch;
            rot[1] = c.net;
            if c.mob.sitting {
                let s = c.mob.age_scale;
                off = if s < 1.0 {
                    [0.0, 0.0, 0.75]
                } else {
                    [0.0, -3.3 * s, 1.0 * s]
                };
            }
        }
        FelineBody if c.mob.sitting => {
            let s = c.mob.age_scale;
            if s < 1.0 {
                rot[0] = -0.436_332_32;
                off = [0.0, 1.25, 0.0];
            } else {
                rot[0] = PI / 4.0 - PI / 2.0;
                off = [0.0, -4.0 * s, 5.0 * s];
            }
        }
        FelineTail1 if c.mob.sitting => {
            let s = c.mob.age_scale;
            if s < 1.0 {
                rot[0] = 0.545_415_4;
                off = [0.0, 4.0, -0.9];
            } else {
                rot[0] = 1.727_876_1 - 0.9;
                off = [0.0, 8.0 * s, -2.0 * s];
            }
        }
        FelineBody | FelineTail1 => {}
        FelineTail2 => {
            let s = c.mob.age_scale;
            if c.mob.sitting && s >= 1.0 {
                rot[0] = 2.670_354 - 1.727_876_1;
                off = [0.0, 2.0 * s, -0.8 * s];
            } else if !c.mob.sitting {
                rot[0] = PI / 4.0 * c.pos.cos() * c.amt;
            }
        }
        FelineLeg { hind, left } => {
            if c.mob.sitting {
                let s = c.mob.age_scale;
                if s < 1.0 {
                    if hind {
                        off = [0.0, 0.0, -0.9];
                    }
                } else if hind {
                    rot[0] = -PI / 2.0;
                    off = [0.0, 3.0 * s, -4.0 * s];
                } else {
                    rot[0] = -PI / 20.0;
                    off = [0.0, 2.0 * s, -2.0 * s];
                }
            } else {
                let phase = if hind == left { 0.0 } else { PI };
                rot[0] = (c.f + phase).cos() * c.amt * amp;
            }
        }
        // M356 — `WolfModel.setSittingPose` (and `AdultWolfModel`'s mane).
        WolfBody if c.mob.sitting => {
            let s = c.mob.age_scale;
            rot[0] = -PI / 4.0;
            off = [0.0, 4.0 * s, -2.0 * s];
        }
        WolfMane if c.mob.sitting => {
            rot[0] = 2.0 * PI / 5.0 - PI / 2.0;
            off = [0.0, 2.0, 0.0];
        }
        WolfBody | WolfMane => {}
        WolfHind { left } => {
            if c.mob.sitting {
                let s = c.mob.age_scale;
                rot[0] = 3.0 * PI / 2.0;
                off = [0.0, 6.7 * s, -5.0 * s];
            } else {
                let phase = if left { PI } else { 0.0 };
                rot[0] = (c.f + phase).cos() * 1.4 * c.amt * amp;
            }
        }
        WolfFront { left } => {
            if c.mob.sitting {
                let s = c.mob.age_scale;
                rot[0] = 5.811_947;
                off = [if left { -0.01 } else { 0.01 } * s, 1.0 * s, 0.0];
            } else {
                let phase = if left { 0.0 } else { PI };
                rot[0] = (c.f + phase).cos() * 1.4 * c.amt * amp;
            }
        }
        GolemArmRight => rot[0] = (-0.2 + 1.5 * triangle_wave(c.pos, 13.0)) * c.amt,
        GolemArmLeft => rot[0] = (-0.2 - 1.5 * triangle_wave(c.pos, 13.0)) * c.amt,
        GolemLegRight => rot[0] = -1.5 * triangle_wave(c.pos, 13.0) * c.amt,
        GolemLegLeft => rot[0] = 1.5 * triangle_wave(c.pos, 13.0) * c.amt,
        BlazeRod { ring, idx } => {
            // Vanilla repositions each rod: ring parameters (start angle,
            // spin speed, radius, base y) + a per-rod bob. `i` is the rod's
            // global 0..12 index in the y-bob term.
            let i = ring as f32 * 4.0 + idx as f32;
            let (a0, speed, r, y0) = match ring {
                0 => (0.0, -0.1, 9.0, -2.0),
                1 => (PI / 4.0, 0.03, 7.0, 2.0),
                _ => (0.47123894, -0.05, 5.0, 11.0),
            };
            let a = a0 + c.age * PI * speed + idx as f32 * (PI / 2.0);
            let y = if ring < 2 {
                y0 + ((i * 2.0 + c.age) * 0.25).cos()
            } else {
                y0 + ((i * 1.5 + c.age) * 0.5).cos()
            };
            off = [a.cos() * r, y, a.sin() * r];
        }
        GhastTentacle { i } => rot[0] = 0.2 * (c.age * 0.3 + i as f32).sin() + 0.4,
        SquidTentacle => rot[0] = 0.3 * (0.5 + 0.5 * (c.age * 0.2).sin()),
        PhantomWing { left } => {
            let flap = c.age * 7.448451 * DEG;
            let z = flap.cos() * 16.0 * DEG;
            rot[2] = if left { z } else { -z };
        }
        PhantomTail => {
            let flap = c.age * 7.448451 * DEG;
            rot[0] = -(5.0 + (flap * 2.0).cos() * 5.0) * DEG;
        }
        AllayWing { left } => {
            let flap = (c.age * 20.0 * DEG + c.pos).cos() * PI * 0.15 + c.amt;
            let fly = (c.amt / 0.3).min(1.0);
            rot[0] = 0.43633232 * (1.0 - fly);
            rot[1] = if left {
                PI / 4.0 - flap
            } else {
                -PI / 4.0 + flap
            };
        }
        AllayRoot => {
            // M369 — `root.y += cos(age·9°)·0.25·idleBobFactor`, outside the
            // dance branch: the hovering allay bobs a quarter pixel.
            let fly = (c.amt / 0.3).min(1.0);
            off[1] = (c.age * 9.0 * DEG).cos() * 0.25 * (1.0 - fly);
            // AllayModel `state.isDancing` branch, `this.root`. Idle → 0 (the
            // model stays upright; the else-branch never touches root).
            if let Some(dance) = c.allay_dance {
                // danceSpeed = ageInTicks·8° + animationSpeed (walkAnimationSpeed).
                let dance_speed = c.age * 8.0 * DEG + c.amt;
                let spin = dance.spinning_progress;
                // root.yRot = isSpinning ? 4π·spin : 0 (unchanged reset value).
                rot[1] = if dance.is_spinning {
                    PI * 4.0 * spin
                } else {
                    0.0
                };
                // root.zRot = danceFrequency·(1 − spin), danceFreq = cos(dS)·16°.
                rot[2] = dance_speed.cos() * 16.0 * DEG * (1.0 - spin);
            }
        }
        // M370 — `GuardianModel.setupAnim`: the spikes' wobble about their
        // rest (which bakes in `cos(i)·0.01`) and the tail's sway.
        GuardianSpike { i } => {
            let i = usize::from(i);
            let d = ((c.age * 1.5 + i as f32).cos() - (i as f32).cos()) * 0.01;
            off = [
                mobs::GUARDIAN_SPIKE_X[i] * d,
                mobs::GUARDIAN_SPIKE_Y[i] * d,
                mobs::GUARDIAN_SPIKE_Z[i] * d,
            ];
        }
        GuardianTail { seg } => {
            let k = [0.05, 0.1, 0.15][usize::from(seg.min(2))];
            rot[1] = (c.age * 0.125).sin() * PI * k;
        }
        // M369 — `AllayModel.setupAnim`'s body pitch and arms.
        AllayBody => rot[0] = (c.amt / 0.3).min(1.0) * PI / 4.0,
        AllayArm { left } => {
            let fly = (c.amt / 0.3).min(1.0);
            let hold = c.mob.allay_holding;
            rot[0] = hold * (-PI / 3.0 + (-1.134464 + PI / 3.0) * fly);
            let bob = (1.0 - fly) * (1.0 - hold);
            let z = 0.43633232 - (c.age * 9.0 * DEG + PI * 1.5).cos() * PI * 0.075 * bob;
            rot[2] = if left { -z } else { z };
            rot[1] = if left { -0.27925268 } else { 0.27925268 } * hold;
        }
        AllayHead => match c.allay_dance {
            // Dancing: head tilts with the beat (xRot stays 0), scaled down as
            // the spin ramps in. head.yRot = cos(dS)·30°·(1−spin),
            // head.zRot = cos(dS)·14°·(1−spin).
            Some(dance) => {
                let dance_speed = c.age * 8.0 * DEG + c.amt;
                let spin = dance.spinning_progress;
                let cos_ds = dance_speed.cos();
                rot[1] = cos_ds * 30.0 * DEG * (1.0 - spin);
                rot[2] = cos_ds * 14.0 * DEG * (1.0 - spin);
            }
            // Not dancing: the ordinary look (identical to `Anim::Head`).
            // `Option::None` — the `use mobs::Anim::*` glob shadows a bare `None`.
            Option::None => {
                rot[0] = c.pitch;
                rot[1] = c.net;
            }
        },
        VexArm { left } => {
            let bob = (c.age * 5.5 * DEG).cos() * 0.1;
            rot[2] = if left {
                -(PI / 5.0 + bob)
            } else {
                PI / 5.0 + bob
            };
        }
        VexWing { left } => {
            let y = 1.0995574 + (c.age * 45.836624 * DEG).cos() * 16.2 * DEG;
            rot[0] = 0.47123888;
            rot[1] = if left { y } else { -y };
            rot[2] = if left { -0.47123888 } else { 0.47123888 };
        }
        BeeWing { left } => {
            let z = (c.age * 120.32113 * DEG).cos() * PI * 0.15;
            rot[2] = if left { -z } else { z };
        }
        // M354 — `BeeModel.bobUpAndDown`, deltas on the flying pose.
        BeeBone | BeeFrontLegs | BeeBackLegs if !c.mob.bee_angry => {
            let s = (c.age * 0.18).cos();
            match anim {
                BeeBone => {
                    rot[0] = 0.1 + s * PI * 0.025;
                    off[1] = -s * 0.9;
                }
                BeeFrontLegs => rot[0] = -s * PI * 0.1 + PI / 8.0 - PI / 4.0,
                _ => rot[0] = -s * PI * 0.05,
            }
        }
        BeeBone | BeeFrontLegs | BeeBackLegs => {}
        // M391 — scaled, not moved: `body_squeeze_scale`.
        HappyGhastBody => {}
        // M395 — `MagmaCubeModel.setupAnim`: the segments spread upward by
        // the squish (a landing's negative squish leaves them).
        MagmaSegment { i } => {
            off[1] = -(4.0 - f32::from(i)) * c.mob.cube_squish.max(0.0) * 1.7;
        }
        // M394 — the bubble column's tilt, in model space.
        BoatBubble => {
            if c.mob.boat_bubble != 0.0 {
                rot = bubble_tilt(c.mob.boat_bubble).0;
            }
        }
        // M393 — `rotX(θ)` between the renderer's yaw and its
        // `scale(−1, −1, 1)` is `rotX(−θ)` past the flip, in model space.
        BoatHurt => rot[0] = -c.mob.boat_wobble * DEG,
        // M393 — `AbstractBoatModel.animatePaddle(time, side, paddle)`, the
        // assigned angles less the paddle's rest.
        BoatPaddle { right } => {
            let t = c.mob.boat_rowing[usize::from(right)];
            let a = paddle_angles(t, right);
            let rest = mobs::paddle_rest(right);
            rot[0] = a[0] - rest[0];
            rot[1] = a[1] - rest[1];
        }
        FishTail { amp: a, speed } => rot[1] = -a * (speed * c.age).sin(),
        PufferFin { left } => {
            let s = (c.age * 0.2).sin();
            rot[2] = if left { 0.2 - 0.4 * s } else { -0.2 + 0.4 * s };
        }
        // M388 — `EnderDragonModel.setupAnim` (straight, level flight).
        Dragon { part } => {
            // M409 — with the flight history when the dragon has one.
            let (r, o) = match &c.mob.dragon {
                Some(d) => mobs::dragon_flight_pose(d),
                Option::None => mobs::dragon_pose(c.age),
            };
            let i = usize::from(part).min(mobs::DRAGON_SLOTS - 1);
            rot = r[i];
            off = o[i];
        }
        // M387 — `AdultAxolotlModel.setupAnim`'s blended poses.
        Axolotl { part } => {
            let (r, body_y) = mobs::axolotl_pose(
                c.age,
                c.pitch,
                c.net,
                c.mob.axolotl_in_water,
                c.mob.axolotl_moving,
                c.mob.axolotl_playing_dead,
            );
            rot = r[usize::from(part.min(9))];
            if part == 0 {
                off[1] = body_y;
            }
        }
        // M380 — a piglin arm: `HumanoidModel.setupAnim`'s arm stages (M392:
        // all of them), then `PiglinModel.setupAnim`'s poses.
        PiglinArm { left } => {
            if piglin_swings_weapon_down(c) {
                // M390 — `PiglinModel.setupAttackAnimation`: swinging a
                // melee weapon, `swingWeaponDown` in place of the humanoid
                // swing (between the hold pose and the bob, as
                // `HumanoidModel.setupAnim` orders them).
                let phase = if left { c.f } else { c.f + PI };
                rot[0] = phase.cos() * 2.0 * c.amt * 0.5 * amp;
                if c.mob.riding {
                    rot[0] += -PI / 5.0;
                }
                apply_pose_stage(&mut rot, left, c);
                swing_weapon_down(
                    &mut rot,
                    left,
                    c.mob.main_arm_left,
                    c.attack.attack_time,
                    c.age,
                );
                bob_model_part(&mut rot, left, c.age);
            } else {
                // M392 — the walk, the riding drop (M381), the hold pose
                // (`HumanoidMobRenderer.getArmPose`: a spear's) and
                // `HumanoidModel.setupAttackAnimation`'s swing, which places
                // the arm pivots at ∓cos/±sin(body yaw)·5·ageScale. The
                // baby's arms rest at ±4, not the ±5 `humanoid_arm` measures
                // from, so they take the extra px vanilla's assignment moves.
                humanoid_arm(&mut rot, &mut off, left, c, amp);
                if c.attack.attack_time > 0.0 && c.mob.is_baby {
                    off[0] += if left { 1.0 } else { -1.0 };
                }
            }
            piglin_arm_pose(&mut rot, &mut off, left, c);
        }
        // M380 — `AbstractPiglinModel.setupAnim`'s ears.
        PiglinEar { left } => {
            let f = c.age * 0.1 + c.pos * 0.5;
            let a = 0.08 + c.amt * 0.4;
            rot[2] = if left {
                -(f * 1.2).cos() * a
            } else {
                f.cos() * a
            };
            // M390 — dancing, `PiglinModel.setupAnim` assigns the ears
            // about π/6 (the baby's too, whose rest is 5°).
            if c.mob.piglin_pose == mobs::PiglinArmPose::Dancing {
                let dp = c.age / 60.0;
                let rest = if c.mob.is_baby { 5.0 * DEG } else { PI / 6.0 };
                rot[2] = if left {
                    -PI / 6.0 - DEG * mth_cos((dp * 30.0) as f64) * 10.0 + rest
                } else {
                    PI / 6.0 + DEG * mth_sin((dp * 30.0) as f64) * 10.0 - rest
                };
            }
        }
        // M390 — `PiglinModel.setupAnim`'s head: the look, then the dance's
        // bob or the admiring stare.
        PiglinHead => {
            rot[0] = c.pitch;
            rot[1] = c.net;
            match c.mob.piglin_pose {
                mobs::PiglinArmPose::Dancing => {
                    let dp = c.age / 60.0;
                    off[0] += mth_sin((dp * 10.0) as f64);
                    off[1] += mth_sin((dp * 40.0) as f64) + 0.4;
                }
                mobs::PiglinArmPose::AdmiringItem => {
                    rot[0] = 0.5;
                    rot[1] = 0.0;
                }
                _ => {}
            }
        }
        // M390 — the dance's body bob. M392 — and the humanoid swing's body
        // yaw (`swingWeaponDown` does not turn it).
        PiglinBody => {
            if c.attack.attack_time > 0.0 && !piglin_swings_weapon_down(c) {
                rot[1] = attack_body_yrot(&c.attack);
            }
            if c.mob.piglin_pose == mobs::PiglinArmPose::Dancing {
                off[1] += mth_sin((c.age / 60.0 * 40.0) as f64) * 0.35;
            }
        }
        // M379 — `WitchModel.setupAnim`'s nose.
        WitchNose => {
            let t = c.age * c.mob.witch_nose_rate;
            rot[0] = t.sin() * 4.5 * DEG;
            rot[2] = t.cos() * 2.5 * DEG;
            if c.mob.witch_holding {
                rot[0] = -0.9;
                off = [0.0, 3.0, -1.5];
            }
        }
        // M378 — `ParrotModel.setupAnim` / `prepare`, standing or sitting.
        ParrotSit => {
            if c.mob.sitting {
                off[1] = 1.9;
            }
        }
        ParrotHead => {
            rot[0] = c.pitch;
            rot[1] = c.net;
            if c.mob.sitting {
                off[1] = 1.9;
            }
        }
        ParrotTail => {
            if c.mob.sitting {
                rot[0] = PI / 6.0;
                off[1] = 1.9;
            } else {
                rot[0] = c.f.cos() * 0.3 * c.amt;
            }
        }
        ParrotLeg { left } => {
            if c.mob.sitting {
                rot[0] = PI / 2.0;
                off[1] = 1.9;
            } else {
                rot[0] = (c.f + if left { 0.0 } else { PI }).cos() * 1.4 * c.amt;
            }
        }
        // M377 — the camel's and armadillo's clamped looks, the bee's
        // antennae.
        CamelHead => {
            let mut pitch = c.pitch.clamp(-25.0 * DEG, 45.0 * DEG);
            if c.mob.camel_jump_cooldown > 0.0 {
                let lift = 45.0 * DEG * c.mob.camel_jump_cooldown / 55.0;
                pitch = (pitch + lift).clamp(-25.0 * DEG, 70.0 * DEG);
            }
            rot[0] = pitch;
            rot[1] = c.net.clamp(-30.0 * DEG, 30.0 * DEG);
        }
        ArmadilloHead => {
            rot[0] = c.pitch.clamp(-22.5 * DEG, 25.0 * DEG);
            rot[1] = c.net.clamp(-32.5 * DEG, 32.5 * DEG);
        }
        BeeAntenna if !c.mob.bee_angry => rot[0] = (c.age * 0.18).cos() * PI * 0.03,
        BeeAntenna => {}
        // M376 — `WitherBossModel.setupAnim`'s breathing.
        WitherRibcage => rot[0] = 0.05 * (c.age * 0.1).cos() * PI,
        WitherTail => {
            let breath = (c.age * 0.1).cos();
            rot[0] = 0.1 * breath * PI;
            let (rib, mean) = ((0.065 + 0.05 * breath) * PI, mobs::WITHER_RIBCAGE_MEAN);
            off = [
                0.0,
                (rib.cos() - mean.cos()) * 10.0,
                (rib.sin() - mean.sin()) * 10.0,
            ];
        }
        // M374 — `WardenModel.animateWalk` and `animateIdlePose`.
        WardenBody | WardenHead | WardenArm { .. } | WardenLeg { .. } => {
            let sm = (3.0 * c.amt).min(0.5);
            let smm = sm.min(0.35);
            let ap = c.pos * 0.8662;
            let (s, co) = ap.sin_cos();
            let (ws, wc) = (c.age * 0.1).sin_cos();
            match anim {
                WardenBody => {
                    rot[2] = 0.1 * s * sm + 0.025 * ws;
                    rot[0] = co * smm + 0.025 * wc;
                }
                WardenHead => {
                    rot[0] = c.pitch + 1.2 * (ap + PI / 2.0).cos() * smm + 0.06 * ws;
                    rot[1] = c.net;
                    rot[2] = 0.3 * s * sm + 0.06 * wc;
                }
                WardenArm { left } => {
                    rot[0] = -0.8 * if left { co } else { s } * sm;
                }
                WardenLeg { left } => {
                    rot[0] = if left { co } else { (ap + PI).cos() } * sm;
                }
                _ => {}
            }
        }
        // M372 — `NautilusModel.applyBodyRotation`.
        NautilusBody => {
            let lim = 10.0 * DEG;
            rot[0] = c.pitch.clamp(-lim, lim);
            rot[1] = c.net.clamp(-lim, lim);
        }
        // M371 — `DolphinModel.setupAnim`: while moving the tail's `xRot` is
        // assigned (the rest replaced, not added to); still, the rest holds.
        DolphinTail { k, rest } => {
            if c.mob.dolphin_moving {
                rot[0] = -k * (c.age * 0.3).cos() - rest;
            }
        }
        DolphinBody => {
            rot[0] = c.pitch;
            rot[1] = c.net;
            if c.mob.dolphin_moving {
                rot[0] += -0.05 - 0.05 * (c.age * 0.3).cos();
            }
        }
        Crawl { i, ry, tx, with_x } => {
            let ph = c.age * 0.9 + i as f32 * 0.15 * PI;
            let w = (i as i32 - 2).abs() as f32;
            rot[1] = ph.cos() * PI * ry * (1.0 + w);
            if with_x {
                off[0] = ph.sin() * PI * tx * w;
            }
        }
        HumanoidBody => {
            // `setupAttackAnimation` short-circuits on `attackTime <= 0`, so an
            // idle humanoid's body carries no rotation at all.
            if c.attack.attack_time > 0.0 {
                rot[1] = attack_body_yrot(&c.attack);
            }
        }
        HumanoidArmRight | HumanoidArmLeft => {
            humanoid_arm(&mut rot, &mut off, anim == HumanoidArmLeft, c, amp);
        }
        // `AbstractZombieModel.setupAnim` = `super.setupAnim(state)` then
        // `animateZombieArms(...)`, which *overwrites* the arms.
        UndeadArmRight | UndeadArmLeft => {
            let left = anim == UndeadArmLeft;
            humanoid_arm(&mut rot, &mut off, left, c, amp);
            animate_zombie_arms(&mut rot, left, c, c.mob.aggressive);
        }
        // M382 — `DrownedModel.setupAnim`: the zombie's arm, then the
        // trident throw's post-step on the arm posed `THROW_TRIDENT`.
        DrownedArmRight | DrownedArmLeft => {
            let left = anim == DrownedArmLeft;
            humanoid_arm(&mut rot, &mut off, left, c, amp);
            animate_zombie_arms(&mut rot, left, c, c.mob.aggressive);
            let pose = if left {
                c.arm_poses.left
            } else {
                c.arm_poses.right
            };
            if pose == mobs::ArmPose::ThrowTrident {
                rot[0] = rot[0] * 0.5 - PI;
                rot[1] = 0.0;
            }
        }
        // `SkeletonModel.setupAnim` = `super.setupAnim(state)` then its own
        // attack override, gated on aggressive && !holdingBow.
        SkeletonArmRight | SkeletonArmLeft => {
            let left = anim == SkeletonArmLeft;
            humanoid_arm(&mut rot, &mut off, left, c, amp);
            skeleton_attack_arm(&mut rot, left, c);
        }
        // `IllagerModel.setupAnim` assigns its own walk over everything
        // `super.setupAnim` left, then runs the arm-pose switch.
        // M381 — `IllagerModel.setupAnim` riding: both arms −π/5, outright.
        IllagerArmRight | IllagerArmLeft if c.mob.riding => rot = [-PI / 5.0, 0.0, 0.0],
        IllagerArmRight | IllagerArmLeft => {
            illager_arm(&mut rot, &mut off, anim == IllagerArmLeft, c);
        }
        // `WardenModel.animateTendrils`: one shared angle, the right tendril
        // taking its negation (M57).
        WardenTendril { left } => {
            let x = c.tendril * ((c.age as f64 * 2.25).cos() as f32 * PI * 0.1);
            rot[0] = if left { x } else { -x };
        }
    }
    (rot, off)
}

/// `HumanoidModel.setupAnim`'s arm stage: the walk assignment, the
/// [`mobs::ArmPose`] hold dispatch, `setupAttackAnimation`, and the idle bob.
/// Factored out because three other models run it as their `super.setupAnim`
/// before layering their own override on top.
fn humanoid_arm(rot: &mut [f32; 3], off: &mut [f32; 3], left: bool, c: &AnimCtx, amp: f32) {
    use std::f32::consts::PI;
    {
        // The ordinary walk swing, first — `setupAnim` assigns it before
        // `setupAttackAnimation` adds on top.
        let phase = if left { c.f } else { c.f + PI };
        // M402 — `/ state.speedValue` (1 unless fall flying).
        rot[0] = phase.cos() * 2.0 * c.amt * 0.5 * amp / c.mob.walk_divisor();
        // M381 — `if (state.isPassenger) arm.xRot += −π/5`, right after the
        // walk swing.
        if c.mob.riding {
            rot[0] += -PI / 5.0;
        }
        // Then the *hold* pose, from the item in this arm. `setupAnim`
        // runs its `pose{Right,Left}Arm` dispatch here — after the walk
        // assignment, before `setupAttackAnimation` — so the strike is
        // added on top of this baseline, not instead of it. Omitting the
        // stage renders every armed entity from an unarmed baseline: an
        // `ITEM` arm sits π/10 (18°) higher with its walk swing unhalved.
        apply_pose_stage(rot, left, c);
        let a = c.attack;
        if a.attack_time <= 0.0 {
            // `bobModelPart` still runs — it is outside the attack guard.
            bob_model_part(rot, left, c.age);
            return;
        }
        let by = attack_body_yrot(&a);
        let (sin_by, cos_by) = (mth_sin(by as f64), mth_cos(by as f64));
        // rightArm.x = −cos(by)·5·ageScale, rightArm.z =  sin(by)·5·ageScale
        // leftArm.x  =  cos(by)·5·ageScale, leftArm.z  = −sin(by)·5·ageScale
        // …expressed as deltas from the ±5 rest pivot.
        let sign = if left { 1.0 } else { -1.0 };
        off[0] = sign * (cos_by * 5.0 * a.age_scale - mobs::HUMANOID_ARM_PIVOT_X);
        off[2] = -sign * sin_by * 5.0 * a.age_scale;
        // Both arms take the body's yaw; only the left also takes it on x.
        rot[1] += by;
        if left {
            rot[0] += by;
        }
        let is_attack_arm = left == a.left_arm;
        match a.kind {
            // `case WHACK:` falls through to `case NONE: default: break;`,
            // so it runs its block and stops — it is not also a STAB.
            mobs::SwingKind::Whack if is_attack_arm => {
                let swing = ease_out_quart(a.attack_time);
                let aa = mth_sin((swing * PI) as f64);
                let bb = mth_sin((a.attack_time * PI) as f64) * -(c.pitch - 0.7) * 0.75;
                rot[0] -= aa * 1.2 + bb;
                rot[1] += by * 2.0;
                rot[2] += mth_sin((a.attack_time * PI) as f64) * -0.4;
            }
            // `SpearAnimations.thirdPersonAttackHand` *undoes* the shared
            // yaw on both arms (and the left arm's x) before posing the
            // attack arm — so a STAB leaves the other arm at its walk pose
            // while still riding the moved pivot.
            mobs::SwingKind::Stab => {
                rot[1] -= by;
                if left {
                    rot[0] -= by;
                }
                if is_attack_arm {
                    let t = a.attack_time;
                    let prepare = ease_in_out_sine(spear_progress(t, 0.0, 0.05));
                    let attack = ease_in_quad(spear_progress(t, 0.05, 0.2));
                    let retract = ease_in_out_expo(spear_progress(t, 0.4, 1.0));
                    rot[0] += (90.0 * prepare - 120.0 * attack + 30.0 * retract) * DEG;
                }
            }
            // NONE, and WHACK on the non-attack arm: the prologue only.
            _ => {}
        }
        bob_model_part(rot, left, c.age);
    }
}

/// `AnimationUtils.animateAttackArms` — the shared undead strike. Vanilla
/// **assigns** all three rotations, wiping whatever the humanoid stage left.
fn animate_attack_arms(
    rot: &mut [f32; 3],
    left: bool,
    attack_time: f32,
    negate: bool,
    arm_drop: f32,
) {
    use std::f32::consts::PI;
    let a_y = if negate { 1.0f32 } else { -1.0 } * mth_sin((attack_time * PI) as f64);
    let inv = 1.0 - attack_time;
    let a_x = mth_sin(((1.0 - inv * inv) * PI) as f64);
    let x_rot = arm_drop + a_y * 1.2 - a_x * 0.4;
    let y_rot = 0.1 - a_y * 0.6;
    rot[0] = x_rot;
    // right.yRot = negate ? -y : y   |   left.yRot = negate ? y : -y
    rot[1] = if negate == left { y_rot } else { -y_rot };
    rot[2] = 0.0;
}

/// `AnimationUtils.animateZombieArms`.
///
/// Two vanilla quirks are load-bearing and reproduced rather than tidied: a
/// STAB item **skips the strike entirely** (the arms keep whatever
/// `super.setupAnim` left), and `bobArms` runs here *in addition to* the bob
/// `HumanoidModel.setupAnim` already applied — so an undead arm is bobbed
/// twice, and on the STAB path both bobs survive.
fn animate_zombie_arms(rot: &mut [f32; 3], left: bool, c: &AnimCtx, aggressive: bool) {
    use std::f32::consts::PI;
    if c.attack.kind != mobs::SwingKind::Stab {
        // `!state.isBaby || mainHand.isEmpty()` — only a BABY HOLDING SOMETHING
        // drops its arms; an adult always raises them.
        let raise = !c.mob.is_baby || c.mob.main_hand_empty;
        let arm_drop = if raise {
            -PI / if aggressive { 1.5 } else { 2.25 }
        } else {
            0.0
        };
        animate_attack_arms(rot, left, c.attack.attack_time, raise, arm_drop);
    }
    bob_model_part(rot, left, c.age);
}

/// `SkeletonModel.setupAnim`'s override — only when aggressive and not holding
/// a bow, so a bow-armed skeleton keeps its aiming pose.
///
/// Same two sine terms as the undead rig but a different assembly: xRot is
/// pinned to −π/2 and the strike **subtracted**, and the yaw is not negated by
/// an arm flag. Like the undead rig it re-bobs, and it too assigns.
fn skeleton_attack_arm(rot: &mut [f32; 3], left: bool, c: &AnimCtx) {
    use std::f32::consts::PI;
    if !c.mob.aggressive || c.mob.holding_bow {
        return;
    }
    let t = c.attack.attack_time;
    let attack2 = mth_sin((t * PI) as f64);
    let inv = 1.0 - t;
    let attack = mth_sin(((1.0 - inv * inv) * PI) as f64);
    rot[2] = 0.0;
    let y = 0.1 - attack2 * 0.6;
    rot[1] = if left { y } else { -y };
    rot[0] = -std::f32::consts::FRAC_PI_2 - (attack2 * 1.2 - attack * 0.4);
    bob_model_part(rot, left, c.age);
}

/// `AnimationUtils.swingWeaponDown` — the armed-illager strike. The two arms
/// get different terms, and which is which depends on the main arm.
fn swing_weapon_down(rot: &mut [f32; 3], left: bool, main_arm_left: bool, t: f32, age: f32) {
    use std::f32::consts::PI;
    let attack2 = mth_sin((t * PI) as f64);
    let attack = mth_sin(((1.0 - (1.0 - t) * (1.0 - t)) * PI) as f64);
    rot[2] = 0.0;
    rot[1] = if left { -PI / 20.0 } else { PI / 20.0 };
    // The main arm holds the weapon high; the off arm trails.
    let holds_weapon = left == main_arm_left;
    if holds_weapon {
        rot[0] = -1.8849558 + mth_cos((age * 0.09) as f64) * 0.15;
        rot[0] += attack2 * 2.2 - attack * 0.4;
    } else {
        rot[0] = -0.0 + mth_cos((age * 0.19) as f64) * 0.5;
        rot[0] += attack2 * 1.2 - attack * 0.4;
    }
}

/// M390 — `PiglinModel.setupAttackAnimation`'s branch: a piglin attacking
/// with a melee weapon swings it down instead of the humanoid swing.
fn piglin_swings_weapon_down(c: &AnimCtx) -> bool {
    c.mob.piglin_pose == mobs::PiglinArmPose::AttackingWithMeleeWeapon && c.attack.attack_time > 0.0
}

/// M390 — `PiglinModel.setupAnim`'s arm half, after the humanoid stage
/// (the walk, the riding drop, the weapon swing and the bob): each pose
/// assigns what it names over them.
fn piglin_arm_pose(rot: &mut [f32; 3], off: &mut [f32; 3], left: bool, c: &AnimCtx) {
    use mobs::PiglinArmPose as P;
    use std::f32::consts::PI;
    let right_handed = !c.mob.main_arm_left;
    match c.mob.piglin_pose {
        P::Dancing => {
            let dp = c.age / 60.0;
            let z = DEG * (70.0 + mth_cos((dp * 40.0) as f64) * 10.0);
            let s = mth_sin((dp * 40.0) as f64);
            if left {
                rot[2] = -z;
                off[1] += s * 0.5 + 0.5;
            } else {
                rot[2] = z;
                off[1] += s * 0.5 - 0.5;
            }
        }
        // `holdWeaponHigh`, only while not swinging.
        P::AttackingWithMeleeWeapon => {
            if c.attack.attack_time == 0.0 && left == c.mob.main_arm_left {
                rot[0] = -1.8;
            }
        }
        // `AnimationUtils.animateCrossbowHold(rightArm, leftArm, head,
        // mainArm == RIGHT)`: the holding arm is the main one.
        P::CrossbowHold => {
            if left != right_handed {
                rot[1] = (if right_handed { -0.3 } else { 0.3 }) + c.net;
                rot[0] = -PI / 2.0 + c.pitch + 0.1;
            } else {
                rot[1] = (if right_handed { 0.6 } else { -0.6 }) + c.net;
                rot[0] = -1.5 + c.pitch;
            }
        }
        // `AnimationUtils.animateCrossbowCharge(rightArm, leftArm,
        // maxCrossbowChargeDuration, ticksUsingItem, mainArm == RIGHT)`;
        // the duration is the unenchanted 25 ticks.
        P::CrossbowCharge => {
            let max = mobs::ArmPoses::CROSSBOW_CHARGE_DURATION;
            if left != right_handed {
                rot[1] = if right_handed { -0.8 } else { 0.8 };
                rot[0] = -0.970_796_35;
            } else {
                let alpha = c.arm_poses.ticks_using_item.clamp(0.0, max) / max;
                let sign = if right_handed { 1.0 } else { -1.0 };
                rot[1] = lerp(alpha, 0.4, 0.85) * sign;
                rot[0] = lerp(alpha, -0.970_796_35, -PI / 2.0);
            }
        }
        // The off arm raises the item to the eyes.
        P::AdmiringItem => {
            if left == right_handed {
                rot[1] = if left { 0.5 } else { -0.5 };
                rot[0] = -0.9;
            }
        }
        P::Default => {}
    }
}

/// `IllagerModel.setupAnim`'s arm stage.
///
/// Vanilla calls `super.setupAnim` first, then **assigns** its own walk over
/// both arms — which wipes the humanoid hold pose, the attack and the idle bob
/// outright — and only then runs the `IllagerArmPose` switch. So the humanoid
/// stage is not run here at all: everything it would contribute is overwritten
/// before it could be seen.
fn illager_arm(rot: &mut [f32; 3], off: &mut [f32; 3], left: bool, c: &AnimCtx) {
    use mobs::IllagerArmPose as P;
    use std::f32::consts::PI;
    // The illager's own walk: the humanoid coefficients without `/ speedValue`,
    // with yRot and zRot explicitly zeroed.
    let phase = if left { c.f } else { c.f + PI };
    rot[0] = phase.cos() * 2.0 * c.amt * 0.5;
    rot[1] = 0.0;
    rot[2] = 0.0;
    match c.mob.illager_pose {
        P::Attacking => {
            if c.mob.main_hand_empty {
                // `animateZombieArms(left, right, true, state)` — literally
                // `true`, not the mob's aggressive flag.
                animate_zombie_arms(rot, left, c, true);
            } else {
                swing_weapon_down(rot, left, c.mob.main_arm_left, c.attack.attack_time, c.age);
            }
        }
        P::Spellcasting => {
            // The pivot is *assigned* to (∓5, ·, 0): z back to 0 and x to the
            // rest column, so the delta is z −rest_z and x 0.
            off[2] = 0.0;
            rot[0] = mth_cos((c.age * 0.6662) as f64) * 0.25;
            rot[2] = if left {
                -PI * 3.0 / 4.0
            } else {
                PI * 3.0 / 4.0
            };
            rot[1] = 0.0;
        }
        P::BowAndArrow => {
            // Both arms are written by the RIGHT-arm branch in vanilla; the
            // head angles are the net head yaw/pitch this rig already has.
            if left {
                rot[0] = -0.9424779 + c.pitch;
                rot[1] = c.net - 0.4;
                rot[2] = std::f32::consts::FRAC_PI_2;
            } else {
                rot[1] = -0.1 + c.net;
                rot[0] = -std::f32::consts::FRAC_PI_2 + c.pitch;
            }
        }
        P::Celebrating => {
            off[2] = 0.0;
            rot[0] = mth_cos((c.age * 0.6662) as f64) * 0.05;
            rot[2] = if left { -PI * 3.0 / 4.0 } else { 2.670354 };
            rot[1] = 0.0;
        }
        // CROSSED hides both arms (the `arms` part shows instead) and NEUTRAL
        // leaves the walk alone. CROSSBOW_HOLD / CROSSBOW_CHARGE need
        // `ticksUsingItem`, which is not synchronised for a remote entity —
        // excluded rather than approximated, so they render as the plain walk.
        P::Crossed | P::Neutral | P::CrossbowHold | P::CrossbowCharge => {}
    }
}

/// `180.0F / (float)Math.PI`. Vanilla writes the radians→degrees conversion as
/// its own f32 division rather than reusing `1/DEG`, and `thirdPersonHandUse`
/// clamps through it, so the round trip is reproduced with the same constant.
const RAD_TO_DEG: f32 = 180.0 / std::f32::consts::PI;

/// `HumanoidModel.setupAnim`'s pose stage, for one arm — the whole dispatch,
/// not one case of it.
///
/// Vanilla calls `poseRightArm` / `poseLeftArm` in a definite order, and three
/// of the eleven cases write to **both** arms. So computing one arm's rotation
/// means replaying the dispatch and applying every running pose's contribution
/// *to this arm*, in vanilla's order — a later both-arm pose legitimately
/// overwrites what an earlier one-arm pose assigned.
fn apply_pose_stage(rot: &mut [f32; 3], left: bool, c: &AnimCtx) {
    let p = c.arm_poses;
    if !p.known {
        return;
    }
    let o = p.order();
    apply_pose_effect(rot, p.for_arm(o.first_left), o.first_left, left, c);
    if o.second_runs {
        let second_left = !o.first_left;
        apply_pose_effect(rot, p.for_arm(second_left), second_left, left, c);
    }
}

/// One `pose{Right,Left}Arm` case's effect **on the arm `target_left`**.
///
/// `pose_left` is the arm the case was invoked for; `target_left` is the arm
/// whose rotation is being computed. When they differ, only a
/// [`mobs::ArmPose::writes_both_arms`] pose contributes anything.
///
/// Vanilla *assigns* these rotations, and the humanoid arm's rest rotation is
/// zero, so an assignment to the delta is the assignment — the same identity
/// `setupAttackAnimation`'s body yaw relies on.
fn apply_pose_effect(
    rot: &mut [f32; 3],
    pose: mobs::ArmPose,
    pose_left: bool,
    target_left: bool,
    c: &AnimCtx,
) {
    use std::f32::consts::PI;
    let own = pose_left == target_left;
    if !own && !pose.writes_both_arms() {
        return;
    }
    // `holdingInRightArm` — the boolean every mirrored case keys off.
    let right_handed_pose = !pose_left;
    match pose {
        // `case EMPTY: arm.yRot = 0.0F;`
        mobs::ArmPose::Empty => rot[1] = 0.0,
        // `case ITEM: arm.xRot = arm.xRot * 0.5F - (float)(Math.PI / 10);
        //            arm.yRot = 0.0F;`
        mobs::ArmPose::Item => {
            rot[0] = rot[0] * 0.5 - PI / 10.0;
            rot[1] = 0.0;
        }
        // `case BLOCK: this.poseBlockingArm(arm, right);`
        //
        // ```text
        // arm.xRot = arm.xRot * 0.5F - 0.9424779F
        //          + Mth.clamp(head.xRot, -PI*4/9, 0.43633232F);
        // arm.yRot = (right ? -30 : 30) * (PI/180)
        //          + Mth.clamp(head.yRot, -PI/6, PI/6);
        // ```
        mobs::ArmPose::Block => {
            rot[0] = rot[0] * 0.5 - 0.942_477_9 + c.pitch.clamp(-(PI * 4.0 / 9.0), 0.436_332_32);
            let yaw = if right_handed_pose { -30.0 } else { 30.0 };
            rot[1] = yaw * DEG + c.net.clamp(-PI / 6.0, PI / 6.0);
        }
        // `case BOW_AND_ARROW:` — writes both arms. The 0.4 nudge lands on the
        // arm that is *not* holding the bow, with the sign following which arm
        // that is; both arms take the same xRot.
        //
        // ```text
        // right: rightArm.yRot = -0.1F + head.yRot;        leftArm.yRot = 0.1F + head.yRot + 0.4F;
        // left:  rightArm.yRot = -0.1F + head.yRot - 0.4F; leftArm.yRot = 0.1F + head.yRot;
        //        both.xRot = -PI/2 + head.xRot;
        // ```
        mobs::ArmPose::BowAndArrow => {
            let base = if target_left { 0.1 } else { -0.1 };
            let nudge = if own {
                0.0
            } else if target_left {
                0.4
            } else {
                -0.4
            };
            rot[1] = base + c.net + nudge;
            rot[0] = -PI / 2.0 + c.pitch;
        }
        // `case THROW_TRIDENT: arm.xRot = arm.xRot * 0.5F - (float)Math.PI;
        //                     arm.yRot = 0.0F;`
        mobs::ArmPose::ThrowTrident => {
            rot[0] = rot[0] * 0.5 - PI;
            rot[1] = 0.0;
        }
        // `AnimationUtils.animateCrossbowCharge(rightArm, leftArm,
        //      maxCrossbowChargeDuration, ticksUsingItem, holdingInRightArm)`:
        //
        // ```text
        // holdingArm.yRot = holdingInRightArm ? -0.8F : 0.8F;
        // holdingArm.xRot = -0.97079635F;
        // pullingArm.xRot = holdingArm.xRot;
        // useTicks   = Mth.clamp(ticksUsingItem, 0, max);
        // lerpAlpha  = useTicks / max;
        // pullingArm.yRot = Mth.lerp(a, 0.4F, 0.85F) * (holdingInRightArm ? 1 : -1);
        // pullingArm.xRot = Mth.lerp(a, pullingArm.xRot, -PI/2);
        // ```
        //
        // The `max <= 0` guard is a divide-by-zero backstop, not a modelled
        // state: whenever this pose is selected the charge duration is the
        // literal 25, because the pose is only reachable through the use
        // branch and an enchanted crossbow is suppressed upstream. Vanilla
        // would evaluate `0.0F / 0.0F` and drive both arms to NaN, so the
        // guard is written rather than left to chance.
        mobs::ArmPose::CrossbowCharge => {
            let max = c.arm_poses.max_crossbow_charge;
            if !(max > 0.0) {
                return;
            }
            // `own` here means "this is the holding arm": the case is invoked
            // for the arm that holds the crossbow.
            if own {
                rot[1] = if right_handed_pose { -0.8 } else { 0.8 };
                rot[0] = -0.970_796_35;
            } else {
                let alpha = c.arm_poses.ticks_using_item.clamp(0.0, max) / max;
                let sign = if right_handed_pose { 1.0 } else { -1.0 };
                rot[1] = lerp(alpha, 0.4, 0.85) * sign;
                rot[0] = lerp(alpha, -0.970_796_35, -PI / 2.0);
            }
        }
        // `AnimationUtils.animateCrossbowHold(rightArm, leftArm, head,
        //                                     holdingInRightArm)`:
        //
        // ```text
        // holdingArm.yRot  = (holdingInRightArm ? -0.3F : 0.3F) + head.yRot;
        // shootingArm.yRot = (holdingInRightArm ?  0.6F : -0.6F) + head.yRot;
        // holdingArm.xRot  = -PI/2 + head.xRot + 0.1F;
        // shootingArm.xRot = -1.5F + head.xRot;
        // ```
        mobs::ArmPose::CrossbowHold => {
            if own {
                rot[1] = (if right_handed_pose { -0.3 } else { 0.3 }) + c.net;
                rot[0] = -PI / 2.0 + c.pitch + 0.1;
            } else {
                rot[1] = (if right_handed_pose { 0.6 } else { -0.6 }) + c.net;
                rot[0] = -1.5 + c.pitch;
            }
        }
        // ```text
        // arm.xRot = Mth.clamp(head.xRot - 1.9198622F
        //                      - (isCrouching ? PI/12 : 0), -2.4F, 3.3F);
        // arm.yRot = head.yRot -/+ PI/12;      // right subtracts, left adds
        // ```
        //
        // M399 — the crouch term is live: an avatar's `crouching`.
        mobs::ArmPose::Spyglass => {
            let crouch = if c.mob.crouching { PI / 12.0 } else { 0.0 };
            rot[0] = (c.pitch - 1.919_862_2 - crouch).clamp(-2.4, 3.3);
            rot[1] = c.net
                + if right_handed_pose {
                    -PI / 12.0
                } else {
                    PI / 12.0
                };
        }
        // ```text
        // arm.xRot = Mth.clamp(head.xRot, -1.2F, 1.2F) - 1.4835298F;
        // arm.yRot = head.yRot -/+ PI/6;
        // ```
        mobs::ArmPose::TootHorn => {
            rot[0] = c.pitch.clamp(-1.2, 1.2) - 1.483_529_8;
            rot[1] = c.net
                + if right_handed_pose {
                    -PI / 6.0
                } else {
                    PI / 6.0
                };
        }
        // `case BRUSH: arm.xRot = arm.xRot * 0.5F - (float)(Math.PI / 5);
        //             arm.yRot = 0.0F;`
        mobs::ArmPose::Brush => {
            rot[0] = rot[0] * 0.5 - PI / 5.0;
            rot[1] = 0.0;
        }
        // `case SPEAR: SpearAnimations.thirdPersonHandUse(arm, head,
        //              holdingInRightArm, useItemStack, state);`
        mobs::ArmPose::Spear => {
            let invert = if pose_left { -1.0 } else { 1.0 };
            rot[1] = -0.1 * invert + c.net;
            rot[0] = -std::f32::consts::FRAC_PI_2 + c.pitch + 0.8;
            // `if (state.isFallFlying || state.swimAmount > 0.0F)
            //      arm.xRot -= 0.9599311F;`
            // Rewo models neither flag, so both are false and the duck is
            // unreachable — declared alongside crouch and swim.
            rot[1] = DEG * (rot[1] * RAD_TO_DEG).clamp(-60.0, 60.0);
            rot[0] = DEG * (rot[0] * RAD_TO_DEG).clamp(-120.0, 30.0);
            // The remainder of `thirdPersonHandUse` — the KINETIC_WEAPON sway
            // and raise terms — is gated on `!(state.ticksUsingItem <= 0.0F)`,
            // where `ticksUsingItem` is `HumanoidRenderState.ticksUsingItem(arm)`
            // — the *per-arm* accessor, which additionally requires the used
            // hand to be this arm's. M23 supplies that input for humanoid mobs;
            // the sway itself is not transcribed (see the milestone's stated
            // exclusions), so this stays the unconditional half.
        }
    }
}

/// `Mth.lerp(delta, start, end)` — `start + delta * (end - start)`.
fn lerp(delta: f32, start: f32, end: f32) -> f32 {
    start + delta * (end - start)
}

/// `ItemClusterRenderState.getRenderedAmount` — how many copies a stack draws.
///
/// A step function, not a scale: 64 gravel and 49 gravel both render 5.
pub fn rendered_amount(count: i32) -> i32 {
    match count {
        i32::MIN..=1 => 1,
        2..=16 => 2,
        17..=32 => 3,
        33..=48 => 4,
        _ => 5,
    }
}

/// `LegacyRandomSource` / `java.util.Random`, for the per-copy jitter.
///
/// Mirrors `rewo_world::lightmap::LegacyRandom48` rather than importing it,
/// the same crate-boundary rule [`crate::held`] and [`crate::mobs`] follow —
/// `rewo-gpu` depends on neither `rewo-world` nor `rewo-data`.
struct LegacyRandom48 {
    seed: i64,
}

impl LegacyRandom48 {
    const MULTIPLIER: i64 = 0x5DEECE66D;
    const MASK: i64 = (1 << 48) - 1;

    fn with_seed(seed: i64) -> Self {
        Self {
            seed: (seed ^ Self::MULTIPLIER) & Self::MASK,
        }
    }

    fn next(&mut self, bits: u32) -> i32 {
        self.seed = self.seed.wrapping_mul(Self::MULTIPLIER).wrapping_add(11) & Self::MASK;
        (self.seed >> (48 - bits)) as i32
    }

    /// `BitRandomSource.nextFloat` — `next(24) * 5.9604645e-8`.
    fn next_float(&mut self) -> f32 {
        self.next(24) as f32 * 5.960_464_5e-8
    }
}

/// One line of **world-space** text — text that lives on a surface rather
/// than facing the camera (M25e).
///
/// A nametag billboards; sign text does not. Both are glyph quads from the same
/// font atlas, so this shares the entity pass's buffer and shader, and differs
/// only in where the basis comes from: a nametag takes the camera's right/up,
/// this takes the surface's.
#[derive(Clone, Copy, Debug)]
pub struct WorldTextDraw<'a> {
    /// The affine that maps **font pixels** to world space, row-major 3x4.
    ///
    /// Vanilla builds exactly this: a sign's is a translate-rotate chain
    /// ending in `scale(s, -s, s)`, the negative y being the flip from the
    /// font's y-down layout to the world's y-up.
    pub transform: [[f32; 4]; 3],
    /// The text, already split to one rendered line.
    pub text: &'a str,
    /// Baseline origin in font pixels, before the transform. Vanilla centres a
    /// sign line with `x = -font.width(line) / 2`, which the caller does.
    pub x: f32,
    pub y: f32,
    /// Depth along the transform's own third axis, in font pixels (M27).
    ///
    /// Zero for ordinary text. Glowing sign text draws an eight-copy outline
    /// behind the glyphs, and vanilla keeps those copies coplanar and orders
    /// them by draw call under `POLYGON_OFFSET`. Rewo's world text rides the
    /// entity pass's depth-tested buffer with no such offset, so the outline is
    /// pushed a hair *behind* instead — same result, by depth rather than by
    /// order, and a documented deviation rather than a coincidence.
    pub z: f32,
    /// Linear-space colour.
    pub color: [f32; 3],
    /// Per-channel world light.
    pub light: [f32; 3],
    /// M262 — a plate drawn behind this line's glyphs (a text display's
    /// background): `[x0, y0, x1, y1]` in font px, linear RGBA.
    pub backdrop: Option<([f32; 4], [f32; 4])>,
}

/// How many animated part groups one block-entity model may carry.
///
/// Group 0 is "as baked" and never indexes the arrays, so this is the number
/// of *animated* groups plus that one. Four covers the deepest model here — a
/// piglin head, whose two ears animate to different formulas at once.
pub const MAX_PARTS: usize = 7;

/// One block entity to draw (M25b).
///
/// Deliberately not an [`EntityDraw`]: a block entity has no yaw of its own, no
/// animation state and no nametag, and — the load-bearing difference —
/// `BlockEntityRenderer` runs outside `LivingEntityRenderer`'s
/// `scale(-1,-1,1)` / `-1.501` chain, so its model is y-up in block-local
/// space. Folding it into `EntityDraw` would mean a flag that inverts half the
/// transform, which is exactly the kind of shared path that renders one of the
/// two cases subtly wrong.
#[derive(Clone, Copy, Debug)]
pub struct BlockEntityDraw<'a> {
    /// The block's minimum corner in world space — the model spans `+0..1`.
    pub pos: [f32; 3],
    /// A `rewo:be/…` model name.
    pub model: &'a str,
    /// The block-entity renderer's own `Transformation`, as a row-major 3x4
    /// affine matrix applied to the model in **block units** — vanilla's
    /// `poseStack.mulPose(modelTransformation(...))`.
    ///
    /// A matrix rather than a facing angle because the renderers do not agree
    /// on a shape: a chest's is `rotationAround(YP(-yRot), 0.5, 0, 0.5)`, and a
    /// shulker box's is a translate-scale-rotate-flip chain that ends up
    /// y-down. Expressing both as what vanilla already calls a
    /// `Transformation` keeps the emitter from growing a per-type branch.
    pub transform: [[f32; 4]; 3],
    /// Per-channel world light at the block, as the terrain lightmap resolves
    /// it.
    pub light: [f32; 3],
    /// The animated part groups' transforms, in **model px** and applied to
    /// coordinates relative to the matching [`Self::part_pivots`] entry —
    /// vanilla's `ModelPart.render`, which translates by the pose offset and
    /// then rotates before drawing box-local coordinates.
    ///
    /// Indexed by a quad's `part`, where **0 always means "draw where it was
    /// baked"** and is never read from here. Matrices rather than an angle for
    /// the same reason [`Self::transform`] is one a level up: a chest lid
    /// rotates about a fixed hinge, a shulker lid slides half a block while
    /// spinning three-quarters of a turn, and the scalar "openness" they
    /// replaced could only express the first (M26).
    ///
    /// An *array* because one group is not enough either: a piglin head's two
    /// ears animate to **different** formulas at the same time (M29), so the
    /// slot count is what lets one model carry both. The caller builds them —
    /// `rewo_data::be_transform::*_part` — so this crate has no per-type branch.
    pub part_transforms: [[[f32; 4]; 3]; MAX_PARTS],
    /// Each group's pivot, in model px, index-matched to
    /// [`Self::part_transforms`].
    pub part_pivots: [[f32; 3]; MAX_PARTS],
    /// A linear-space colour multiplied into the vertex colour (M28c).
    ///
    /// `[1, 1, 1]` for every block entity whose texture already carries its
    /// colour, which is all of them bar the banner: a banner pattern sprite is
    /// a greyscale **mask**, and the dye that colours it is a per-layer
    /// argument to `submitPatternLayer` rather than anything in the texture.
    /// Sixteen dyes times forty-four patterns is why it is a tint and not
    /// seven hundred baked variants.
    pub tint: [f32; 3],
    /// Run the model through its `display.fixed` transform before
    /// [`Self::transform`] — an item in an item frame (M194), which
    /// `ItemFrameRenderer` submits with `ItemDisplayContext.FIXED`. `false`
    /// for every block entity and for the frame's own block model, which are
    /// placed directly.
    pub fixed_display: bool,
    /// Draw the enchantment glint over it too (M198): a framed item whose
    /// stack `hasFoil()` — `ItemFrameRenderer` submits the stack, and the
    /// item renderer adds `RenderTypes.entityGlint()` for a foil stack in any
    /// world context. `false` for every block entity.
    pub glint: bool,
    /// M277 — the white overlay's progress (`OverlayTexture.u`), lit TNT's
    /// flash; 0 for none.
    pub white: f32,
}

/// A map in an item frame (M194): `MapRenderer.render(state, pose, ...,
/// showOnlyFrame = true, light)` — the pixel quad and the decorations whose
/// type is `showOnItemFrame`.
pub struct FrameMapDraw<'a> {
    /// The frame entity's position.
    pub pos: [f32; 3],
    /// Map units (`0..128`) to entity-relative block units —
    /// `item_frame::map_pose` as an affine.
    pub transform: [[f32; 4]; 3],
    /// Which framed-map slot to sample ([`EntityPass::prepare_frame_maps`]).
    pub map_id: i32,
    /// The map's light (`getLightCoords(isGlowFrame, 15728850, light)`).
    pub light: [f32; 3],
    /// The decorations to draw, already filtered to `renderOnFrame`, in the
    /// map's order (the z step counts them).
    pub decorations: &'a [FrameMapDecoration],
    /// The names among them (M204) — a named banner's label.
    pub labels: &'a [MapLabel],
}

/// A map decoration's name label (M204), `MapRenderer.render`'s
/// `decoration.name != null` branch: plain text (the component flattened),
/// at the decoration's half-pixel position.
#[derive(Clone, Debug, PartialEq)]
pub struct MapLabel {
    pub x: i8,
    pub y: i8,
    pub text: String,
}

/// One decoration on a framed map.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrameMapDecoration {
    pub type_id: i32,
    pub x: i8,
    pub y: i8,
    pub rot: u8,
}

impl EntityPass {
    /// The atlas rect of one font byte, as `[u0, v0, u1, v1]`.
    ///
    /// The font sheet is a 16x16 grid of `cell`-px cells. `v0` is the cell's
    /// **bottom** and `v1` its top, because the image's y runs down while the
    /// glyph's does not — flipping those two is how text renders upside down.
    fn glyph_uv(&self, b: u8) -> [f32; 4] {
        let (aw, ah) = (ATLAS_W as f32, ATLAS_H as f32);
        let cell = self.cell as f32;
        let cx = (b as u32 % 16 * self.cell) as f32;
        let cy = (b as u32 / 16 * self.cell) as f32;
        [cx / aw, (cy + cell) / ah, (cx + cell) / aw, cy / ah]
    }

    /// World-space text (M25e) — the same glyph quads a nametag uses, but
    /// placed by an explicit affine instead of the camera basis.
    ///
    /// No drop shadow: vanilla's sign text has none (the screen-space overlay
    /// is what draws shadows), and adding one would be a visible invention.
    pub(crate) fn emit_world_text(&self, verts: &mut Vec<Vertex>, draws: &[WorldTextDraw<'_>]) {
        if !self.has_font {
            return;
        }
        let cell = self.cell as f32;
        for d in draws {
            let m = &d.transform;
            let pz = d.z;
            let place = |px: f32, py: f32| -> [f32; 3] {
                [
                    m[0][0] * px + m[0][1] * py + m[0][2] * pz + m[0][3],
                    m[1][0] * px + m[1][1] * py + m[1][2] * pz + m[1][3],
                    m[2][0] * px + m[2][1] * py + m[2][2] * pz + m[2][3],
                ]
            };
            let mut pen = d.x;
            for b in d.text.bytes() {
                let adv = self.advance[b as usize] as f32;
                if b != b' ' {
                    if verts.len() + 6 > MAX_VERTS {
                        return;
                    }
                    // `glyph_uv` gives the cell's bottom as `v0`, for the
                    // nametag's y-UP frame. A world-text frame is y-DOWN (a
                    // sign's and a text display's transform end in a
                    // negative y scale, vanilla's font layout), so the
                    // glyph's top, `y0`, takes the cell's top, `v1`. Until
                    // M263 this took `v0`, and every sign and hologram in
                    // the windowed client read upside down.
                    let [u0, v1, u1, v0] = self.glyph_uv(b);
                    // The glyph cell is `cell` px square in the atlas but 8 px
                    // in font space, so it scales by 8/cell.
                    let s = 8.0 / cell;
                    let (x0, y0) = (pen, d.y);
                    let (x1, y1) = (pen + cell * s, d.y + cell * s);
                    let corners = [
                        (place(x0, y0), u0, v0),
                        (place(x1, y0), u1, v0),
                        (place(x1, y1), u1, v1),
                        (place(x0, y0), u0, v0),
                        (place(x1, y1), u1, v1),
                        (place(x0, y1), u0, v1),
                    ];
                    for (p, u, v) in corners {
                        verts.push(Vertex {
                            pos: p,
                            uv: [u, v],
                            color: [d.color[0], d.color[1], d.color[2], 1.0],
                            light_hurt: [d.light[0], d.light[1], d.light[2], 0.0],
                        });
                    }
                }
                pen += adv;
            }
        }
    }

    /// M262 — the plates behind world-text lines (a text display's
    /// background): untextured quads from the atlas's white texel, as the
    /// nametag plate and the map label use, a hundredth of a font px behind
    /// the glyphs (vanilla's `-0.01F`). Translucent, so they go in the
    /// blended text range; the glyphs are solid and depth-test in front.
    fn emit_world_text_backdrops(&self, verts: &mut Vec<Vertex>, draws: &[WorldTextDraw<'_>]) {
        for d in draws {
            let Some(([x0, y0, x1, y1], rgba)) = d.backdrop else {
                continue;
            };
            if verts.len() + 6 > MAX_VERTS {
                return;
            }
            let m = &d.transform;
            let bz = d.z - 0.01;
            let at = |px: f32, py: f32| -> [f32; 3] {
                [
                    m[0][0] * px + m[0][1] * py + m[0][2] * bz + m[0][3],
                    m[1][0] * px + m[1][1] * py + m[1][2] * bz + m[1][3],
                    m[2][0] * px + m[2][1] * py + m[2][2] * bz + m[2][3],
                ]
            };
            for (px, py) in [(x0, y0), (x1, y0), (x1, y1), (x0, y0), (x1, y1), (x0, y1)] {
                verts.push(Vertex {
                    pos: at(px, py),
                    uv: self.white_uv,
                    color: rgba,
                    light_hurt: [d.light[0], d.light[1], d.light[2], 0.0],
                });
            }
        }
    }

    /// The width of a string in font pixels — `Font.width`, which is the sum
    /// of the per-glyph advances. Sign text centres on it.
    pub fn text_width(&self, text: &str) -> f32 {
        text.bytes().map(|b| self.advance[b as usize] as f32).sum()
    }

    /// The per-glyph advance table, for layout that happens before a draw.
    pub fn font_advance(&self) -> &[u8; 256] {
        &self.advance
    }

    /// Framed maps (M194). `RenderTypes.text`: no directional shade, the
    /// vertex colour white, light from the lightmap — so unlike the item
    /// paths these quads are not shaded by their normal.
    pub(crate) fn emit_frame_maps(&self, verts: &mut Vec<Vertex>, draws: &[FrameMapDraw<'_>]) {
        let to_entity = |m: &[[f32; 4]; 3], pos: [f32; 3], p: [f32; 3]| {
            let r = |i: usize| m[i][0] * p[0] + m[i][1] * p[1] + m[i][2] * p[2] + m[i][3];
            [pos[0] + r(0), pos[1] + r(1), pos[2] + r(2)]
        };
        let quad = |verts: &mut Vec<Vertex>,
                    d: &FrameMapDraw<'_>,
                    m: &[[f32; 4]; 3],
                    corners: &[[f32; 3]; 4],
                    uv: &[[f32; 2]; 4]| {
            if verts.len() + 6 > MAX_VERTS {
                return;
            }
            let [lr, lg, lb] = d.light;
            for &i in &[0usize, 1, 2, 0, 2, 3] {
                verts.push(Vertex {
                    pos: to_entity(m, d.pos, corners[i]),
                    uv: uv[i],
                    color: [1.0, 1.0, 1.0, 1.0],
                    light_hurt: [lr, lg, lb, 0.0],
                });
            }
        };
        for d in draws {
            let Some([u0, v0, du, dv]) = self.frame_map_uv(d.map_id) else {
                continue;
            };
            let (corners, uv01) = crate::hand::MAP_PIXELS_QUAD;
            let uv: [[f32; 2]; 4] =
                std::array::from_fn(|i| [u0 + uv01[i][0] * du, v0 + uv01[i][1] * dv]);
            quad(verts, d, &d.transform, &corners, &uv);
            let base = glam::Mat4::from_cols_array_2d(&[
                [d.transform[0][0], d.transform[1][0], d.transform[2][0], 0.0],
                [d.transform[0][1], d.transform[1][1], d.transform[2][1], 0.0],
                [d.transform[0][2], d.transform[1][2], d.transform[2][2], 0.0],
                [d.transform[0][3], d.transform[1][3], d.transform[2][3], 1.0],
            ]);
            // `count` steps for every decoration drawn, sprite or not.
            for (count, deco) in d.decorations.iter().enumerate() {
                let present = usize::try_from(deco.type_id)
                    .ok()
                    .and_then(|i| self.map_deco_present.get(i))
                    .copied()
                    .unwrap_or(false);
                let Some((x, y)) = map_deco_origin(deco.type_id).filter(|_| present) else {
                    continue;
                };
                let (aw, ah) = (ATLAS_W as f32, ATLAS_H as f32);
                let (su0, sv0) = (x as f32 / aw, y as f32 / ah);
                let (su1, sv1) = (
                    (x + MAP_DECO_SPRITE) as f32 / aw,
                    (y + MAP_DECO_SPRITE) as f32 / ah,
                );
                let z = count as f32 * -0.001;
                let m = crate::item_frame::affine(
                    base * crate::hand::map_decoration_pose(deco.x, deco.y, deco.rot),
                );
                quad(
                    verts,
                    d,
                    &m,
                    &[
                        [-1.0, 1.0, z],
                        [1.0, 1.0, z],
                        [1.0, -1.0, z],
                        [-1.0, -1.0, z],
                    ],
                    &[[su0, sv0], [su1, sv0], [su1, sv1], [su0, sv1]],
                );
            }
        }
    }

    /// `ChestRenderer.submit` — the block-entity models (M25b).
    ///
    /// ```text
    /// poseStack.mulPose(modelTransformation(state.facing));
    ///   // = rotationAround(YP.rotationDegrees(-facing.toYRot()), 0.5, 0, 0.5)
    /// ```
    ///
    /// The rotation is **about the block centre in x/z**, not the origin, which
    /// is what keeps a rotated chest inside its own block. `ModelPart.compile`
    /// divides model px by 16, so the quads land in 0..1 block units and the
    /// block position is a plain translate on top.
    ///
    /// A quad in group 0 is drawn where it was baked. A quad in any other
    /// group goes through [`BlockEntityDraw::part_transform`] in the group's
    /// own space — a chest's lid rotating about its hinge, a shulker box's lid
    /// sliding and spinning off its base. Both animations run on the
    /// *interpolated* clock, so they advance per frame rather than per tick.
    /// Returns the quads emitted per draw (M247).
    pub(crate) fn emit_block_entities(
        &self,
        verts: &mut Vec<Vertex>,
        draws: &[BlockEntityDraw<'_>],
        glint: &mut GlintSink<'_>,
    ) -> Vec<u32> {
        let mut emitted = vec![0u32; draws.len()];
        let Some(items) = self.held_items.as_ref() else {
            return emitted;
        };
        for (n, d) in draws.iter().enumerate() {
            // M419 — a framed item's `FIXED` model.
            let model = if d.fixed_display {
                items.fixed_model(d.model)
            } else {
                items.any(d.model)
            };
            let Some(model) = model else {
                continue;
            };
            let m = &d.transform;
            let [light_r, light_g, light_b] = d.light;
            for q in &model.quads {
                if verts.len() + 6 > MAX_VERTS {
                    return emitted;
                }
                let Some([u0, v0, du, dv]) = self.item_uv(q.tex) else {
                    continue;
                };
                emitted[n] += 1;
                let mut p4 = [[0f32; 3]; 4];
                let mut m4 = [[0f32; 3]; 4];
                for (i, corner) in q.verts.iter().enumerate() {
                    // The animated group moves in its own space first, in model
                    // px — `ModelPart.render` translates by the pose offset and
                    // then rotates, so a box's own coordinates are already
                    // relative to it. The bake stores them *with* the offset
                    // applied, so it comes back off here and the group's
                    // transform puts it wherever the animation says.
                    // Group 0 is drawn where it was baked. Any other group goes
                    // through its OWN matrix in its own space, which is what
                    // lets a piglin's two ears move differently inside one
                    // model (M29).
                    let g = q.part as usize;
                    let corner = if g == 0 || g >= MAX_PARTS {
                        *corner
                    } else {
                        let a = &d.part_transforms[g];
                        let piv = &d.part_pivots[g];
                        let v = [corner[0] - piv[0], corner[1] - piv[1], corner[2] - piv[2]];
                        [
                            a[0][0] * v[0] + a[0][1] * v[1] + a[0][2] * v[2] + a[0][3],
                            a[1][0] * v[0] + a[1][1] * v[1] + a[1][2] * v[2] + a[1][3],
                            a[2][0] * v[0] + a[2][1] * v[1] + a[2][2] * v[2] + a[2][3],
                        ]
                    };
                    // model px -> block units. The renderer's transform is
                    // written in block units, and `ModelPart.compile` is what
                    // divides by 16, so the scale comes first.
                    let p = [corner[0] / 16.0, corner[1] / 16.0, corner[2] / 16.0];
                    // A framed item: `ItemTransform.apply` for the FIXED
                    // context (centre, scale, rotate, translate), the same
                    // derivation every other item path uses.
                    let p = if d.fixed_display {
                        crate::held::apply_display(&model.fixed, false, p)
                    } else {
                        p
                    };
                    let p = [
                        m[0][0] * p[0] + m[0][1] * p[1] + m[0][2] * p[2] + m[0][3],
                        m[1][0] * p[0] + m[1][1] * p[1] + m[1][2] * p[2] + m[1][3],
                        m[2][0] * p[0] + m[2][1] * p[1] + m[2][2] * p[2] + m[2][3],
                    ];
                    m4[i] = p;
                    p4[i] = [d.pos[0] + p[0], d.pos[1] + p[1], d.pos[2] + p[2]];
                }
                // Shade from the rotated normal, for the same reason the item
                // paths do: the baked face directions are pre-rotation.
                let n = face_normal(&m4);
                let shade = mobs::mix_light_flipped(n, self.nether_light);
                // M433 — the banner's dye arrives linear; vanilla multiplies its bytes.
                let color = gamma_vertex(shade, linear_to_srgb3(d.tint));
                for &i in &[0usize, 1, 2, 0, 2, 3] {
                    verts.push(Vertex {
                        pos: p4[i],
                        uv: [u0 + q.uv[i][0] * du, v0 + q.uv[i][1] * dv],
                        color,
                        // A block entity is never hurt — the overlay flag is
                        // a `LivingEntity` property — but lit TNT flashes
                        // white (M277).
                        light_hurt: [light_r, light_g, light_b, overlay_value(false, d.white)],
                    });
                    // Same position, same moment — see [`GlintSink`].
                    if d.glint {
                        glint.push(p4[i], q.uv[i]);
                    }
                }
            }
        }
        emitted
    }

    /// M247 — the quads the last frame emitted per block-entity draw.
    pub fn block_entity_quads(&self) -> &[u32] {
        &self.be_quads
    }

    /// `ItemEntityRenderer.submit` — a dropped stack (M24b).
    ///
    /// ```text
    /// AABB bb = item.getModelBoundingBox();
    /// float minOffsetY = -(float)bb.minY + 0.0625F;
    /// float bob  = Mth.sin(ageInTicks / 10.0F + bobOffset) * 0.1F + 0.1F;
    /// float spin = ItemEntity.getSpin(ageInTicks, bobOffset);  // age/20 + off
    /// poseStack.translate(0, bob + minOffsetY, 0);
    /// poseStack.mulPose(Axis.YP.rotation(spin));
    /// submitMultipleFromCount(...);
    /// ```
    ///
    /// **No `scale(-1,-1,1)` and no `-1.501` translate** — those belong to
    /// `LivingEntityRenderer`, not `EntityRenderer`, so a ground item lives in
    /// entity-local metres with y already up. That is why this is a separate
    /// emitter rather than a flag on the held-item path.
    fn emit_ground_item(
        &self,
        verts: &mut Vec<Vertex>,
        d: &EntityDraw<'_>,
        name: &str,
        time: f32,
        hurt: f32,
        glint: Option<&mut GlintSink<'_>>,
    ) {
        let Some(items) = self.held_items.as_ref() else {
            return;
        };
        // M419 — its `GROUND` model.
        let Some(item) = items.ground_model(name) else {
            return;
        };
        let mut glint = glint;
        // M240 — a thrown item: the GROUND transform, then the billboard,
        // centred on the entity; one copy, no bob, no spin.
        if let Some(bb) = d.billboard {
            let [light_r, light_g, light_b] = d.light;
            for q in &item.quads {
                if verts.len() + 6 > MAX_VERTS {
                    return;
                }
                let Some([u0, v0, du, dv]) = self.item_uv(q.tex) else {
                    continue;
                };
                let mut p4 = [[0f32; 3]; 4];
                let mut m4 = [[0f32; 3]; 4];
                for (i, c) in q.verts.iter().enumerate() {
                    let p = [c[0] / 16.0, c[1] / 16.0, c[2] / 16.0];
                    let p = bb.place(crate::held::apply_display(&item.ground, false, p));
                    m4[i] = p;
                    p4[i] = [d.pos[0] + p[0], d.pos[1] + p[1], d.pos[2] + p[2]];
                }
                let shade = mobs::mix_light_flipped(face_normal(&m4), self.nether_light);
                // M433 — the orb's tint arrives linear and unshaded; the shade folds in
                // gamma space.
                let color = bb.tint.map_or_else(
                    || gamma_vertex(shade, [1.0; 3]),
                    |t| [t[0], t[1], t[2], 1.0],
                );
                for &i in &[0usize, 1, 2, 0, 2, 3] {
                    verts.push(Vertex {
                        pos: p4[i],
                        uv: [u0 + q.uv[i][0] * du, v0 + q.uv[i][1] * dv],
                        color,
                        light_hurt: [light_r, light_g, light_b, hurt],
                    });
                    if let Some(g) = glint.as_deref_mut() {
                        g.push(p4[i], q.uv[i]);
                    }
                }
            }
            return;
        }
        let amount = rendered_amount(d.ground_count);
        if amount == 0 {
            return;
        }

        // Every corner through the GROUND display transform, once — both the
        // bounding box and the drawn geometry read it, and vanilla's
        // `getModelBoundingBox` is exactly the extent of the transformed model.
        let placed: Vec<[[f32; 3]; 4]> = item
            .quads
            .iter()
            .map(|q| {
                let mut out = [[0f32; 3]; 4];
                for (i, c) in q.verts.iter().enumerate() {
                    let p = [c[0] / 16.0, c[1] / 16.0, c[2] / 16.0];
                    // `left = false`: the ground context is never mirrored.
                    out[i] = crate::held::apply_display(&item.ground, false, p);
                }
                out
            })
            .collect();
        let mut lo = [f32::MAX; 3];
        let mut hi = [f32::MIN; 3];
        for q in &placed {
            for c in q {
                for k in 0..3 {
                    lo[k] = lo[k].min(c[k]);
                    hi[k] = hi[k].max(c[k]);
                }
            }
        }
        if lo[0] > hi[0] {
            return; // no geometry
        }
        let min_offset_y = -lo[1] + 0.0625;
        let model_depth = hi[2] - lo[2];

        // `ageInTicks`. Rewo keeps no per-entity age, so this is the shared
        // clock — the *phase* differs from vanilla's per-entity tickCount, but
        // `bobOffs` is itself a client-side roll, so items still bob out of
        // step with each other, which is the only visible property.
        //
        // M81: a pickup animation overrides it with the age at *capture*,
        // because vanilla's `ItemPickupParticle` carries a render-state
        // snapshot rather than the live entity — the stack freezes mid-spin
        // for the flight.
        let age = d.ground_age.unwrap_or(time * 20.0);
        let bob = mth_sin((age / 10.0 + d.bob_offset) as f64) * 0.1 + 0.1;
        // `ItemEntity.getSpin(ageInTicks, bobOffset)` = `age / 20 + offset`.
        let spin = age / 20.0 + d.bob_offset;
        let (ss, cs) = spin.sin_cos();

        // `submitMultipleFromCount`: the copies, from a seeded LCG. The seed is
        // `Item.getId(item) + stack.getDamageValue()`; Rewo decodes no damage
        // for a dropped stack, so it is the item id.
        let mut rng = LegacyRandom48::with_seed(d.ground_seed as i64);
        let flat = model_depth <= 0.0625;
        let offset_z = model_depth * 1.5;
        let mut copies: Vec<[f32; 3]> = Vec::with_capacity(amount as usize);
        if !flat {
            copies.push([0.0; 3]);
            for _ in 1..amount {
                let xo = (rng.next_float() * 2.0 - 1.0) * 0.15;
                let yo = (rng.next_float() * 2.0 - 1.0) * 0.15;
                let zo = (rng.next_float() * 2.0 - 1.0) * 0.15;
                copies.push([xo, yo, zo]);
            }
        } else {
            // A flat item fans out along z instead of jittering in 3D, and the
            // whole fan is centred first.
            let mut z = -(offset_z * (amount - 1) as f32 / 2.0);
            copies.push([0.0, 0.0, z]);
            for _ in 1..amount {
                z += offset_z;
                let xo = (rng.next_float() * 2.0 - 1.0) * 0.15 * 0.5;
                let yo = (rng.next_float() * 2.0 - 1.0) * 0.15 * 0.5;
                copies.push([xo, yo, z]);
            }
        }

        let [light_r, light_g, light_b] = d.light;
        for copy in &copies {
            for (qi, q) in item.quads.iter().enumerate() {
                if verts.len() + 6 > MAX_VERTS {
                    return;
                }
                let Some([u0, v0, du, dv]) = self.item_uv(q.tex) else {
                    continue; // texture not resident — draw nothing, never garbage
                };
                let mut p4 = [[0f32; 3]; 4];
                let mut m4 = [[0f32; 3]; 4];
                for i in 0..4 {
                    let p = placed[qi][i];
                    let p = [p[0] + copy[0], p[1] + copy[1], p[2] + copy[2]];
                    // YP(spin).
                    let p = [p[0] * cs + p[2] * ss, p[1], -p[0] * ss + p[2] * cs];
                    m4[i] = p;
                    p4[i] = [
                        d.pos[0] + p[0],
                        d.pos[1] + p[1] + bob + min_offset_y,
                        d.pos[2] + p[2],
                    ];
                }
                // Shade from the spun normal, for the same reason the held
                // path does: a dropped item is rotating, so its baked face
                // directions are not the directions it faces.
                let n = face_normal(&m4);
                let shade = mobs::mix_light_flipped(n, self.nether_light);
                let color = gamma_vertex(shade, [1.0; 3]);
                for &i in &[0usize, 1, 2, 0, 2, 3] {
                    verts.push(Vertex {
                        pos: p4[i],
                        uv: [u0 + q.uv[i][0] * du, v0 + q.uv[i][1] * dv],
                        color,
                        light_hurt: [light_r, light_g, light_b, hurt],
                    });
                    // Same position, same moment — a dropped stack bobs, spins
                    // and jitters per copy, so a second derivation of any of
                    // that would miss the depth-equal test.
                    if let Some(g) = glint.as_deref_mut() {
                        g.push(p4[i], q.uv[i]);
                    }
                }
            }
        }
    }
}

/// M240 — `ThrownItemRenderer.submit`'s `scale(s) · mulPose(camera.orientation)`:
/// the camera's yaw and pitch (degrees) and the renderer's scale.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Billboard {
    pub cam_yaw: f32,
    pub cam_pitch: f32,
    pub scale: f32,
    /// M251 — a vertex colour drawn unshaded, for a renderer that submits
    /// its own coloured geometry (`ExperienceOrbRenderer`); `None` shades the
    /// item model by its faces as `ThrownItemRenderer` does.
    pub tint: Option<[f32; 3]>,
}

impl Billboard {
    /// `Camera.rotation.rotationYXZ(π - yRot, -xRot, 0)` applied to `p`:
    /// `Ry(π - yaw) · Rx(-pitch) · p`, after the scale.
    pub fn place(&self, p: [f32; 3]) -> [f32; 3] {
        let p = [p[0] * self.scale, p[1] * self.scale, p[2] * self.scale];
        let pitch = -self.cam_pitch.to_radians();
        let (sx, cx) = pitch.sin_cos();
        let p = [p[0], p[1] * cx - p[2] * sx, p[1] * sx + p[2] * cx];
        let yaw = std::f32::consts::PI - self.cam_yaw.to_radians();
        let (sy, cy) = yaw.sin_cos();
        [p[0] * cy + p[2] * sy, p[1], -p[0] * sy + p[2] * cy]
    }
}

/// `LivingEntityRenderer.getFlipDegrees()` — the angle a dead entity topples
/// through, in degrees.
///
/// 90 by default; exactly three renderers override it, and all three answer
/// 180 because their models are already lying flat and roll right over.
/// Written as an exhaustive-by-listing match rather than a default so that a
/// mob added later cannot silently inherit an unverified 90.
pub fn death_flip_degrees(kind: mobs::EntityModelKind) -> f32 {
    use mobs::EntityModelKind as K;
    match kind {
        // `SpiderRenderer`, `CaveSpider` (a `SpiderRenderer`),
        // `SilverfishRenderer`, `EndermiteRenderer`.
        K::Spider | K::CaveSpider | K::Silverfish | K::Endermite => 180.0,
        _ => 90.0,
    }
}

/// The death topple angle in radians:
///
/// ```text
/// float fall = (state.deathTime - 1.0F) / 20.0F * 1.6F;
/// fall = Mth.sqrt(fall);
/// if (fall > 1.0F) fall = 1.0F;
/// poseStack.mulPose(Axis.ZP.rotationDegrees(fall * this.getFlipDegrees()));
/// ```
///
/// Two details are worth not smoothing over. `deathTime` is guarded at `> 0`
/// by the extractor, so the first dying frame feeds `1 + partialTicks` and the
/// numerator starts at `partialTicks` — the topple begins from zero rather
/// than jumping. And `Mth.sqrt` of a *negative* argument is NaN, which cannot
/// happen here for the same reason: the guard keeps the numerator ≥ 0.
fn death_roll(d: &EntityDraw<'_>) -> f32 {
    death_roll_for(d.kind, d.death_time)
}

/// [`death_roll`] by its two real inputs, so the gate can drive the production
/// curve without building a whole [`EntityDraw`].
pub fn death_roll_for(kind: mobs::EntityModelKind, death_time: f32) -> f32 {
    if death_time <= 0.0 {
        return 0.0;
    }
    let fall = (((death_time - 1.0) / 20.0 * 1.6).max(0.0)).sqrt().min(1.0);
    (fall * death_flip_degrees(kind)).to_radians()
}

/// `AnimationUtils.bobModelPart(part, ageInTicks, scale)`:
///
/// ```text
/// part.zRot += scale * (Mth.cos(ageInTicks * 0.09F) * 0.05F + 0.05F);
/// part.xRot += scale * (Mth.sin(ageInTicks * 0.067F) * 0.05F);
/// ```
///
/// `HumanoidModel.setupAnim` calls it for the right arm with `scale = 1` and
/// the left with `-1`, **after** `setupAttackAnimation` and the crouch block —
/// applied last here for the same reason, though the order is immaterial since
/// both terms are `+=`.
///
/// Vanilla guards it with `armPose != SPYGLASS`. Rewo models no arm poses (every
/// pose is `EMPTY`), so the bob is unconditional — exact for the poses this
/// client can be in, with the spyglass case a documented gap alongside crouch,
/// swim and item-use, which Rewo also does not model.
fn bob_model_part(rot: &mut [f32; 3], left: bool, age_in_ticks: f32) {
    let scale = if left { -1.0 } else { 1.0 };
    rot[2] += scale * (mth_cos((age_in_ticks * 0.09) as f64) * 0.05 + 0.05);
    rot[0] += scale * (mth_sin((age_in_ticks * 0.067) as f64) * 0.05);
}

/// Sample a keyframe channel at `t` seconds — vanilla `Entry.apply`: prev =
/// last frame at-or-before, interpolation mode from the NEXT frame,
/// catmull-rom over the surrounding four.
fn kf_sample(frames: &[mobs::KfFrame], t: f32) -> [f32; 3] {
    let mut prev = 0usize;
    for (i, f) in frames.iter().enumerate() {
        if t <= f.t {
            break;
        }
        prev = i;
    }
    let next = (prev + 1).min(frames.len() - 1);
    let (fp, fn_) = (&frames[prev], &frames[next]);
    let alpha = if next != prev && fn_.t > fp.t {
        ((t - fp.t) / (fn_.t - fp.t)).clamp(0.0, 1.0)
    } else {
        0.0
    };
    if fn_.catmullrom {
        let p0 = &frames[prev.saturating_sub(1)].v;
        let p3 = &frames[(next + 1).min(frames.len() - 1)].v;
        let mut out = [0.0f32; 3];
        for i in 0..3 {
            out[i] = catmullrom(alpha, p0[i], fp.v[i], fn_.v[i], p3[i]);
        }
        out
    } else {
        let mut out = [0.0f32; 3];
        for i in 0..3 {
            out[i] = fp.v[i] + (fn_.v[i] - fp.v[i]) * alpha;
        }
        out
    }
}

/// Vanilla `Mth.catmullrom`.
fn catmullrom(a: f32, p0: f32, p1: f32, p2: f32, p3: f32) -> f32 {
    0.5 * (2.0 * p1
        + (p2 - p0) * a
        + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * a * a
        + (3.0 * p1 - p0 - 3.0 * p2 + p3) * a * a * a)
}

/// A keyframe driver's (time seconds, value scale) — vanilla's
/// `applyWalk`/`apply` call sites.
fn kf_drive(driver: mobs::KfDriver, c: &AnimCtx) -> (f32, f32) {
    use mobs::KfDriver::*;
    match driver {
        Walk { speed_f, scale_f } => (c.pos * 0.05 * speed_f, (c.amt * scale_f).min(1.0)),
        WalkPlusAge {
            age_div,
            amt_add,
            speed_f,
            scale_f,
        } => (
            (c.pos + c.age / age_div) * 0.05 * speed_f,
            ((c.amt + amt_add) * scale_f).min(1.0),
        ),
        Age => (c.age * 0.05, 1.0),
        AgeGatedByWalk { gate } => (c.age * 0.05, (c.amt * gate).min(1.0)),
        // Only reachable under KfGate::During — the gesture is active.
        GestureAge => match c.gesture {
            Some((_, age)) => (age, 1.0),
            Option::None => (0.0, 0.0),
        },
    }
}

/// The pass-wide half of the CEM interpreter's per-frame inputs — everything
/// [`cem_anim_context`] needs that is not on the `EntityDraw`.
#[derive(Clone, Copy, Debug)]
pub struct CemFrameInputs {
    /// Real seconds since the previous frame (FA integrates against it).
    pub frame_time: f32,
    /// Monotonic frame counter (FA's same-frame guard).
    pub frame_counter: f32,
    /// Wall-clock seconds; `ageInTicks = age_seconds · 20`.
    pub age_seconds: f32,
    /// Camera eye in world space — OptiFine's `player_pos_*`.
    pub cam_pos: [f32; 3],
}

/// Build the OptiFine CEM interpreter's per-frame variable bindings for one
/// entity. `carried` is that entity's `var.*` slot store from the previous
/// frame (FA's integrators continue through it).
///
/// Extracted from `emit_model` so the `swingshot` oracle can prove the input
/// mapping — notably that `swing_progress` really is
/// `getAttackAnim(partialTicks)` and not the default 0 — without standing up a
/// Vulkan device. The renderer calls this exact function.
pub fn cem_anim_context(
    d: &EntityDraw<'_>,
    frame: CemFrameInputs,
    carried: Vec<f32>,
) -> crate::cem_anim::AnimContext {
    crate::cem_anim::AnimContext {
        user: carried,
        frame_time: frame.frame_time,
        frame_counter: frame.frame_counter,
        head_yaw: wrap_degrees(d.head_yaw - d.yaw),
        head_pitch: d.pitch,
        limb_swing: d.limb_swing,
        limb_speed: d.limb_amount,
        age: frame.age_seconds * 20.0,
        time: frame.age_seconds * 20.0,
        is_on_ground: true,
        is_alive: true,
        is_child: false,
        health: 20.0,
        max_health: 20.0,
        // OptiFine `swing_progress` is `getAttackAnim(partialTicks)` — the same
        // value the built-in attack rig consumes.
        swing_progress: d.attack.attack_time,
        id: d.anim_id,
        // World position + body yaw, and the viewer's position: FA aims
        // eyes/heads by comparing `player_pos_*` against `pos_*`, and derives
        // its turn-detection vars from `rot_y`.
        pos_x: d.pos[0],
        pos_y: d.pos[1],
        pos_z: d.pos[2],
        player_pos_x: frame.cam_pos[0],
        player_pos_y: frame.cam_pos[1],
        player_pos_z: frame.cam_pos[2],
        rot_y: d.yaw.to_radians(),
        ..Default::default()
    }
}

/// Animation inputs for [`oracle_part_deltas`] — the pose/rig state the
/// renderer feeds `part_transforms`, in vanilla units. Lets the `eventshot`
/// oracle exercise the exact production rig math with no GPU device.
#[derive(Clone, Default)]
pub struct OracleInputs {
    /// Look pitch (radians) and net head yaw (radians) — the `Anim::Head` inputs.
    pub pitch: f32,
    pub net: f32,
    /// Raw `walkAnimationPos` / `walkAnimationSpeed`.
    pub limb_swing: f32,
    pub limb_amount: f32,
    /// Wall-clock seconds → `ageInTicks = age_seconds · 20` (ambient anims).
    pub age_seconds: f32,
    /// Active metadata gesture + its age in seconds.
    pub gesture: Option<(mobs::Gesture, f32)>,
    /// Wire-event elapsed seconds per [`mobs::ModelEvent`] (`None` = inactive).
    pub events: [Option<f32>; mobs::ModelEvent::COUNT],
    /// Armadillo shell swap.
    pub shell: bool,
    /// Allay dance inputs (`Some` only for a dancing Allay) — the `danceshot`
    /// oracle feeds these to prove the `AllayRoot`/`AllayHead` pose math.
    pub allay_dance: Option<mobs::AllayDance>,
    /// Combat-swing pose — the `swingshot` oracle feeds these to prove
    /// `HumanoidModel.setupAttackAnimation`.
    pub attack: mobs::SwingPose,
    /// Both arms' hold pose + handedness — the `swingshot` oracle feeds these
    /// to prove `pose{Right,Left}Arm` and the `setupAnim` pose dispatch.
    pub arm_poses: mobs::ArmPoses,
    /// Synced mob state for the undead / skeleton / illager rigs (M20).
    pub mob: mobs::MobCombat,
    /// `Warden.getTendrilAnimation` in `0..1` (M57) — 0 at rest, which is
    /// vanilla's synched default for a warden that has heard nothing.
    pub tendril: f32,
}

/// The per-part animation deltas (added rotation radians ZYX + pivot offset
/// model px) the renderer would apply for `kind` under `inputs`, paired with
/// each part's vanilla name. Builds the SAME `mobs` model and runs the SAME
/// [`anim_deltas`] as `part_transforms`/`emit_model`, so `rewo eventshot
/// --check` verifies entity-event rigs against the real production math without
/// standing up a Vulkan device. `None` if the kind has no built-in model.
pub fn oracle_part_deltas(
    kind: EntityModelKind,
    inputs: &OracleInputs,
) -> Option<Vec<(&'static str, [f32; 3], [f32; 3])>> {
    let def = mobs::MOBS.iter().find(|d| d.kind == kind)?;
    let model = (def.build)();
    let ctx = AnimCtx {
        pitch: inputs.pitch,
        net: inputs.net,
        f: inputs.limb_swing * 0.6662,
        pos: inputs.limb_swing,
        amt: inputs.limb_amount,
        age: inputs.age_seconds * 20.0,
        gesture: inputs.gesture,
        events: inputs.events,
        shell: inputs.shell,
        body_yaw: 0.0,
        stand_pose: mobs::STAND_DEFAULT_POSE,
        allay_dance: inputs.allay_dance,
        attack: inputs.attack,
        arm_poses: inputs.arm_poses,
        mob: inputs.mob,
        tendril: inputs.tendril,
    };
    let (drots, doffs) = anim_deltas(
        &model.parts,
        &model.keyframes,
        &model.event_rigs,
        &ctx,
        None,
    );
    Some(
        model
            .parts
            .iter()
            .enumerate()
            .map(|(i, p)| (p.name, drots[i], doffs[i]))
            .collect(),
    )
}

/// Per-part animation deltas: added rotation (radians, summed ZYX) + pivot
/// offset (model px), from the procedural `setupAnim` formulas, the CEM
/// program, the metadata keyframe rigs, and the wire-event one-shot rigs — all
/// ADDED, exactly like vanilla's `offsetRotation`/`offsetPos` stacking. Factored
/// out of [`part_transforms`] so the `eventshot` oracle can read the raw rig
/// contributions (the compose step below turns them into matrices) without a
/// GPU device.
fn anim_deltas(
    parts: &[mobs::Part],
    keyframes: &[mobs::KfAnim],
    event_rigs: &[mobs::EventRig],
    ctx: &AnimCtx,
    cem_deltas: Option<&[[f32; 6]]>,
) -> (Vec<[f32; 3]>, Vec<[f32; 3]>) {
    let n = parts.len();
    let mut drots = vec![[0.0f32; 3]; n];
    let mut doffs = vec![[0.0f32; 3]; n];
    for (i, p) in parts.iter().enumerate() {
        let (r, o) = anim_delta(p.anim, p.amp, ctx);
        drots[i] = r;
        doffs[i] = o;
    }
    // CEM program deltas: rx/ry/rz onto the rotation, tx/ty/tz onto the
    // pivot offset (model px). Applied about the bone's pivot by the
    // compose step below, exactly like the built-in deltas.
    if let Some(deltas) = cem_deltas {
        for (i, d) in deltas.iter().enumerate().take(n) {
            drots[i][0] += d[0];
            drots[i][1] += d[1];
            drots[i][2] += d[2];
            doffs[i][0] += d[3];
            doffs[i][1] += d[4];
            doffs[i][2] += d[5];
        }
    }
    for kf in keyframes {
        // The gate decides whether the rig plays this frame (vanilla's
        // `setupAnim` call patterns); the driver decides where in time.
        match kf.gate {
            mobs::KfGate::Always => {}
            mobs::KfGate::During(g) => {
                if !matches!(ctx.gesture, Some((active, _)) if active == g) {
                    continue;
                }
            }
            mobs::KfGate::Unless(g) => {
                if matches!(ctx.gesture, Some((active, _)) if active == g) {
                    continue;
                }
            }
            mobs::KfGate::NotShell => {
                if ctx.shell {
                    continue;
                }
            }
        }
        let (mut t, scale) = kf_drive(kf.driver, ctx);
        if scale <= 0.0 {
            continue;
        }
        if kf.def.looping {
            t = t.rem_euclid(kf.def.length);
        }
        for (ch, &pi) in kf.def.channels.iter().zip(&kf.parts) {
            let v = kf_sample(ch.frames, t);
            let dst = match ch.target {
                mobs::KfTarget::Rot => &mut drots[pi as usize],
                mobs::KfTarget::Pos => &mut doffs[pi as usize],
            };
            for i in 0..3 {
                dst[i] += v[i] * scale;
            }
        }
    }
    // Wire-event one-shot rigs — distinct from the metadata gesture rigs: the
    // gate is "an event age is present", not a pose match. Vanilla applies
    // these at full weight (`animation.apply(state, ageInTicks)`); a non-looping
    // rig sampled past its length holds its last (neutral) frame, so a fired
    // event that has run out contributes nothing — no explicit stop needed.
    for rig in event_rigs {
        let Some(age) = ctx.events[rig.event.index()] else {
            continue;
        };
        let mut t = age;
        if rig.def.looping {
            t = t.rem_euclid(rig.def.length);
        }
        for (ch, &pi) in rig.def.channels.iter().zip(&rig.parts) {
            let v = kf_sample(ch.frames, t);
            let dst = match ch.target {
                mobs::KfTarget::Rot => &mut drots[pi as usize],
                mobs::KfTarget::Pos => &mut doffs[pi as usize],
            };
            for i in 0..3 {
                dst[i] += v[i];
            }
        }
    }
    (drots, doffs)
}

/// M394 — `AbstractBoatRenderer`'s bubble tilt, `mulPose(new Quaternionf()
/// .setAngleAxis(angle·π/180, 1, 0, 1))`, as a part rotation (`rotateZYX`
/// Euler angles, in model space) and a uniform scale.
///
/// JOML's `setAngleAxis` does not normalise the axis: `q = (s, 0, s, c)`
/// with `s, c = sin, cos(θ/2)`, `|q|² = 1 + s²`, and `rotate(q)`'s matrix is
/// homogeneous in `q`, so it is `|q|²` times the rotation by
/// `α = 2·atan2(√2·s, c)` about `(1, 0, 1)/√2`. Past the renderer's
/// `scale(−1, −1, 1)` (`rotZ(π)`) the axis is `(−1, 0, 1)/√2`.
fn bubble_tilt(angle_deg: f32) -> ([f32; 3], f32) {
    let half = angle_deg * DEG * 0.5;
    let (s, c) = half.sin_cos();
    let alpha = 2.0 * (std::f32::consts::SQRT_2 * s).atan2(c);
    let u = [
        -std::f32::consts::FRAC_1_SQRT_2,
        0.0,
        std::f32::consts::FRAC_1_SQRT_2,
    ];
    // Rodrigues: R = I + sin α·K + (1 − cos α)·K².
    let (sa, ca) = alpha.sin_cos();
    let t = 1.0 - ca;
    let r = [
        [
            ca + u[0] * u[0] * t,
            u[0] * u[1] * t - u[2] * sa,
            u[0] * u[2] * t + u[1] * sa,
        ],
        [
            u[1] * u[0] * t + u[2] * sa,
            ca + u[1] * u[1] * t,
            u[1] * u[2] * t - u[0] * sa,
        ],
        [
            u[2] * u[0] * t - u[1] * sa,
            u[2] * u[1] * t + u[0] * sa,
            ca + u[2] * u[2] * t,
        ],
    ];
    // `Rz·Ry·Rx`: m20 = −sin y, m21 = cos y·sin x, m22 = cos y·cos x,
    // m10 = sin z·cos y, m00 = cos z·cos y.
    let y = (-r[2][0]).clamp(-1.0, 1.0).asin();
    let x = r[2][1].atan2(r[2][2]);
    let z = r[1][0].atan2(r[0][0]);
    ([x, y, z], 1.0 + s * s)
}

/// M393 — `AbstractBoatModel.animatePaddle`'s `(xRot, yRot)`:
/// `clampedLerp((sin(−t) + 1)/2, −π/3, −π/12)`, `clampedLerp((sin(−t + 1)
/// + 1)/2, −π/4, π/4)`, the right paddle's yaw `π − yRot`.
fn paddle_angles(t: f32, right: bool) -> [f32; 2] {
    use std::f32::consts::PI;
    let clamped = |d: f32, a: f32, b: f32| a + d.clamp(0.0, 1.0) * (b - a);
    let x = clamped((mth_sin(-t as f64) + 1.0) / 2.0, -PI / 3.0, -PI / 12.0);
    let y = clamped(
        (mth_sin((-t + 1.0) as f64) + 1.0) / 2.0,
        -PI / 4.0,
        PI / 4.0,
    );
    [x, if right { PI - y } else { y }]
}

/// M391 — the per-part scales with `HappyGhastModel`'s body squeeze folded
/// in: `Some` only for a happy ghast whose body slot holds something
/// (`!state.bodyItem.isEmpty()`, i.e. `d.body_armor`), the `HappyGhastBody`
/// part's scale times [`mobs::HAPPY_GHAST_BODY_SQUEEZE`] and every other
/// part's CEM scale (or 1) unchanged.
///
/// M394 — and a tilted boat's bubble part, at the tilt's `|q|²`
/// ([`bubble_tilt`]).
fn body_squeeze_scale(
    model: &MobModel,
    d: &EntityDraw,
    cem_scale: Option<&[[f32; 3]]>,
) -> Option<Vec<[f32; 3]>> {
    if d.mob.boat_bubble != 0.0 {
        let k = bubble_tilt(d.mob.boat_bubble).1;
        return Some(
            model
                .parts
                .iter()
                .enumerate()
                .map(|(i, p)| {
                    let base = cem_scale
                        .and_then(|s| s.get(i).copied())
                        .unwrap_or([1.0; 3]);
                    if p.anim == mobs::Anim::BoatBubble {
                        base.map(|v| v * k)
                    } else {
                        base
                    }
                })
                .collect(),
        );
    }
    if d.kind != mobs::EntityModelKind::HappyGhast || d.body_armor.is_none() {
        return None;
    }
    Some(
        model
            .parts
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let base = cem_scale
                    .and_then(|s| s.get(i).copied())
                    .unwrap_or([1.0; 3]);
                if p.anim == mobs::Anim::HappyGhastBody {
                    base.map(|v| v * mobs::HAPPY_GHAST_BODY_SQUEEZE)
                } else {
                    base
                }
            })
            .collect(),
    )
}

/// Compose every part's global transform `(M, o)` — child-in-parent
/// hierarchy with vanilla's `translate(pivot); rotateZYX(base + delta)`
/// per level, plus any keyframe-rig contributions (which ADD onto the
/// procedural deltas, exactly like vanilla's `offsetRotation`/`offsetPos`).
/// Shared by rendering and the mobshot geometric prediction so the two can
/// never disagree.
fn part_transforms(
    model: &MobModel,
    ctx: &AnimCtx,
    cem_deltas: Option<&[[f32; 6]]>,
    cem_scale: Option<&[[f32; 3]]>,
) -> Vec<([[f32; 3]; 3], [f32; 3])> {
    let n = model.parts.len();
    let (drots, doffs) = anim_deltas(
        &model.parts,
        &model.keyframes,
        &model.event_rigs,
        ctx,
        cem_deltas,
    );
    let mut out: Vec<([[f32; 3]; 3], [f32; 3])> = Vec::with_capacity(n);
    for (i, p) in model.parts.iter().enumerate() {
        let e = [
            p.rot[0] + drots[i][0],
            p.rot[1] + drots[i][1],
            p.rot[2] + drots[i][2],
        ];
        let mut r = mat_zyx(e);
        // CEM scale channels (sx/sy/sz) scale the part's geometry about its
        // pivot. Vanilla applies scale innermost (translate→rotate→scale), so
        // fold it as `R·S`; children inherit the parent's `R·S`, scaling the
        // whole subtree — exactly like vanilla's pose stack.
        if let Some(sc) = cem_scale {
            if let Some(&s) = sc.get(i) {
                if s != [1.0, 1.0, 1.0] {
                    r = mat_mul(r, scale_mat(s));
                }
            }
        }
        let pivot = [
            p.pivot[0] + doffs[i][0],
            p.pivot[1] + doffs[i][1],
            p.pivot[2] + doffs[i][2],
        ];
        let (m, o) = match p.parent {
            Some(par) => {
                let (pm, po) = &out[par as usize];
                let gp = mat_apply(pm, pivot);
                (
                    mat_mul(*pm, r),
                    [gp[0] + po[0], gp[1] + po[1], gp[2] + po[2]],
                )
            }
            Option::None => (r, pivot),
        };
        out.push((m, o));
    }
    out
}

// ---- small math helpers for the per-part animation ----------------------

const IDENTITY3: [[f32; 3]; 3] = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

/// `rotateZYX(z, y, x)` as a matrix: `Rz·Ry·Rx` (x applied first) —
/// matches `mobs::rotate_zyx`.
fn mat_zyx(e: [f32; 3]) -> [[f32; 3]; 3] {
    if e == [0.0; 3] {
        return IDENTITY3;
    }
    mat_mul(mat_mul(rot_z(e[2]), rot_y(e[1])), rot_x(e[0]))
}

/// `PlayerCapeModel.setupAnim`'s **net** cape rotation, from the three
/// `AvatarRenderState` angles in degrees (M60).
///
/// # Why this is not `mat_zyx`, and why that matters
///
/// Every other part in this renderer composes `Rz·Ry·Rx` from a Euler triple,
/// because that is what vanilla's `ModelPart` does. The cape is the one part
/// that does not go through `ModelPart`'s angles at all. `setupAnim` builds
///
/// ```text
/// Quaternionf().rotateY(-PI).rotateX(a).rotateZ(b).rotateY(c)
/// ```
///
/// — JOML's `rotate*` post-multiply, so that quaternion is
/// `Ry(-π)·Rx(a)·Rz(b)·Ry(c)` — and hands it to `ModelPart.rotateBy`, which
/// post-multiplies it onto the part's existing `rotationZYX`. The part's
/// existing rotation is its `PartPose`, `Ry(π)`. So the product is
///
/// ```text
/// Ry(π) · Ry(-π) · Rx(a) · Rz(b) · Ry(c)  =  Rx(a) · Rz(b) · Ry(c)
/// ```
///
/// The leading `rotateY(-PI)` exists **to cancel the pose**, which is why
/// [`mobs::CAPE_PIVOT`] carries the pose's offset and not its rotation.
///
/// `rotateBy` then decomposes the product back to ZYX Euler and stores it —
/// an exact round-trip on the matrix, so nothing is lost by staying in matrix
/// form and skipping Euler entirely. It is *not* the same as feeding
/// `(a, c, b)` to [`mat_zyx`]: that would build `Rz(b)·Ry(c)·Rx(a)`, a
/// different rotation whenever two of the three are non-zero — which is
/// exactly the case a cape sways in.
pub fn cape_rotation(flap_deg: f32, lean_deg: f32, lean2_deg: f32) -> [[f32; 3]; 3] {
    let a = (6.0 + lean_deg / 2.0 + flap_deg).to_radians();
    let b = (lean2_deg / 2.0).to_radians();
    let c = (180.0 - lean2_deg / 2.0).to_radians();
    mat_mul(mat_mul(rot_x(a), rot_z(b)), rot_y(c))
}

/// The cape's model-space transform: the `(matrix, offset)` pair
/// `part_transforms` would have produced for it as a child of `body` (M60).
///
/// Exactly the child branch of that function — `m = m_parent · r`,
/// `o = m_parent · pivot + o_parent` — with [`cape_rotation`] standing in for
/// the Euler composition it cannot express. Shared with `capeshot` so the
/// oracle grades this arithmetic rather than a second copy of it.
pub fn cape_transform(
    body: &([[f32; 3]; 3], [f32; 3]),
    cape: &CapeDraw,
) -> ([[f32; 3]; 3], [f32; 3]) {
    let (mb, ob) = body;
    let r = cape_rotation(cape.flap, cape.lean, cape.lean2);
    let pivot = mat_apply(mb, mobs::CAPE_PIVOT);
    (
        mat_mul(*mb, r),
        [pivot[0] + ob[0], pivot[1] + ob[1], pivot[2] + ob[2]],
    )
}

/// `CapeLayer`'s `poseStack.translate(0, -0.053125, 0.06875)`, in **model
/// pixels** (M60).
///
/// Vanilla's translate is in blocks and sits outside the model but inside the
/// render flip, so scaling it by 16 and adding it to the model-space position
/// puts it at exactly the same point in the chain. It moves the cape up and
/// back — away from a chestplate's inflated shell.
pub fn cape_clearance_shift(chest_humanoid: bool) -> [f32; 3] {
    if chest_humanoid {
        [0.0, -0.053125 * 16.0, 0.06875 * 16.0]
    } else {
        [0.0; 3]
    }
}

/// Model space → **cape space** for a point (M61): world-axis aligned, model
/// units, origin on the entity.
///
/// The three steps are the ones `emit_model` runs inline on every quad — the
/// model flip (`-x`, and `MODEL_EYE_Y - y` because model +y is world down),
/// the death roll, then the body yaw. What is left afterwards is only the
/// px→block scale and the entity's own position, which is exactly why the
/// wavy cape's simulation can live in this space and still land in the right
/// place.
pub fn model_pos_to_cape(v: [f32; 3], st: f32, ct: f32, sr: f32, cr: f32) -> [f32; 3] {
    let e = [-v[0], mobs::MODEL_EYE_Y - v[1], v[2]];
    let e = [e[0] * cr - e[1] * sr, e[0] * sr + e[1] * cr, e[2]];
    [e[0] * ct + e[2] * st, e[1], -e[0] * st + e[2] * ct]
}

/// [`model_pos_to_cape`] for a direction — the same map without its
/// translation, so the `MODEL_EYE_Y` offset drops out and the y flip is a
/// bare negation.
pub fn model_dir_to_cape(v: [f32; 3], st: f32, ct: f32, sr: f32, cr: f32) -> [f32; 3] {
    let e = [-v[0], -v[1], v[2]];
    let e = [e[0] * cr - e[1] * sr, e[0] * sr + e[1] * cr, e[2]];
    [e[0] * ct + e[2] * st, e[1], -e[0] * st + e[2] * ct]
}

/// Apply the death roll to a cape-space vector (M61).
///
/// The roll is a rotation in the *pre-yaw* frame, and the wavy cape's chain
/// is simulated in the post-yaw world frame, so it has to be un-yawed,
/// rolled, and yawed back. For every living entity `(sr, cr)` is `(0, 1)`
/// and this is the identity.
pub fn roll_in_cape_space(v: [f32; 3], st: f32, ct: f32, sr: f32, cr: f32) -> [f32; 3] {
    let e = [v[0] * ct - v[2] * st, v[1], v[0] * st + v[2] * ct];
    let e = [e[0] * cr - e[1] * sr, e[0] * sr + e[1] * cr, e[2]];
    [e[0] * ct + e[2] * st, e[1], -e[0] * st + e[2] * ct]
}

fn normalize_or(v: [f32; 3], fallback: [f32; 3]) -> [f32; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if !l.is_finite() || l <= 1e-9 {
        return fallback;
    }
    [v[0] / l, v[1] / l, v[2] / l]
}

/// The shortest rotation taking unit `a` onto unit `b` (Rodrigues).
///
/// Used to carry the cape's rest frame onto each joint's tangent, so the
/// width and thickness axes twist as little as possible along the chain.
/// Deriving each joint's frame from the rest frame rather than from its
/// predecessor keeps it deterministic and free of accumulated drift.
fn min_rotation(a: [f32; 3], b: [f32; 3]) -> [[f32; 3]; 3] {
    let v = [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ];
    let c = a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let s2 = v[0] * v[0] + v[1] * v[1] + v[2] * v[2];
    if s2 < 1e-12 {
        if c >= 0.0 {
            return IDENTITY3;
        }
        // Antiparallel: a half turn about any axis perpendicular to `a`.
        // Picking the world axis `a` leans on least keeps the cross product
        // well conditioned.
        let seed = if a[0].abs() < 0.9 {
            [1.0, 0.0, 0.0]
        } else {
            [0.0, 1.0, 0.0]
        };
        let k = normalize_or(
            [
                a[1] * seed[2] - a[2] * seed[1],
                a[2] * seed[0] - a[0] * seed[2],
                a[0] * seed[1] - a[1] * seed[0],
            ],
            [1.0, 0.0, 0.0],
        );
        return [
            [
                2.0 * k[0] * k[0] - 1.0,
                2.0 * k[0] * k[1],
                2.0 * k[0] * k[2],
            ],
            [
                2.0 * k[1] * k[0],
                2.0 * k[1] * k[1] - 1.0,
                2.0 * k[1] * k[2],
            ],
            [
                2.0 * k[2] * k[0],
                2.0 * k[2] * k[1],
                2.0 * k[2] * k[2] - 1.0,
            ],
        ];
    }
    let f = 1.0 / (1.0 + c);
    let vx = [[0.0, -v[2], v[1]], [v[2], 0.0, -v[0]], [-v[1], v[0], 0.0]];
    let vx2 = mat_mul(vx, vx);
    let mut out = IDENTITY3;
    for r in 0..3 {
        for k in 0..3 {
            out[r][k] += vx[r][k] + vx2[r][k] * f;
        }
    }
    out
}

/// Atlas UVs for one cape face, from the slot origin and the box UVs
/// [`mobs::cape_faces`] produced (M60).
///
/// The V divisor is `ATLAS_H`, and the slot is `CAPE_SLOT_H` = **32** tall,
/// so a box V of 17 lands 17/32 of the way down the cape's own sheet — the
/// `yTexScale 0.5` the cube was authored with. A 64-tall slot would halve it.
pub fn cape_face_uv(origin: (u32, u32), uvs: &[[f32; 2]; 4]) -> [[f32; 2]; 4] {
    let (ax, ay) = origin;
    std::array::from_fn(|i| {
        [
            (ax as f32 + uvs[i][0]) / ATLAS_W as f32,
            (ay as f32 + uvs[i][1]) / ATLAS_H as f32,
        ]
    })
}

/// Rotation about Z.
fn rot_z(a: f32) -> [[f32; 3]; 3] {
    let (s, c) = a.sin_cos();
    [[c, -s, 0.0], [s, c, 0.0], [0.0, 0.0, 1.0]]
}

/// Rotation about X (y-down model space, matches `mobs::rotate_zyx`).
fn rot_x(a: f32) -> [[f32; 3]; 3] {
    let (s, c) = a.sin_cos();
    [[1.0, 0.0, 0.0], [0.0, c, -s], [0.0, s, c]]
}

/// Rotation about Y.
fn rot_y(a: f32) -> [[f32; 3]; 3] {
    let (s, c) = a.sin_cos();
    [[c, 0.0, s], [0.0, 1.0, 0.0], [-s, 0.0, c]]
}

/// Diagonal scale matrix (CEM `sx/sy/sz`, applied about the part pivot).
fn scale_mat(s: [f32; 3]) -> [[f32; 3]; 3] {
    [[s[0], 0.0, 0.0], [0.0, s[1], 0.0], [0.0, 0.0, s[2]]]
}

/// `a · b` (row-major; `b` applies to the vector first).
fn mat_mul(a: [[f32; 3]; 3], b: [[f32; 3]; 3]) -> [[f32; 3]; 3] {
    let mut out = [[0.0f32; 3]; 3];
    for (i, row) in out.iter_mut().enumerate() {
        for (j, cell) in row.iter_mut().enumerate() {
            *cell = a[i][0] * b[0][j] + a[i][1] * b[1][j] + a[i][2] * b[2][j];
        }
    }
    out
}

/// Unit normal of a quad from three of its corners, right-hand wound.
/// Degenerate quads (a zero-area side face) fall back to +Y, which
/// `shade_for` reads as the world bottom — visible, not a NaN.
fn face_normal(p: &[[f32; 3]; 4]) -> [f32; 3] {
    let a = [p[1][0] - p[0][0], p[1][1] - p[0][1], p[1][2] - p[0][2]];
    let b = [p[2][0] - p[0][0], p[2][1] - p[0][1], p[2][2] - p[0][2]];
    let n = [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ];
    let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
    if len < 1e-6 {
        [0.0, 1.0, 0.0]
    } else {
        [n[0] / len, n[1] / len, n[2] / len]
    }
}

fn mat_apply(m: &[[f32; 3]; 3], v: [f32; 3]) -> [f32; 3] {
    [
        m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2],
        m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2],
        m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2],
    ]
}

/// `REWO_CEM_NOANIM=1` → render CEM pack models in their rest pose (skip the
/// animation program). Diagnostic knob: isolates static-geometry bugs from
/// animation bugs. Read once.
fn cem_noanim() -> bool {
    static NOANIM: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *NOANIM.get_or_init(|| crate::knobs::var("REWO_CEM_NOANIM").is_ok_and(|v| v == "1"))
}

/// Wrap to [−180, 180) — vanilla `Mth.wrapDegrees` for the net head yaw.
fn wrap_degrees(deg: f32) -> f32 {
    (deg + 180.0).rem_euclid(360.0) - 180.0
}

/// Fill a quad's UV bounding rect in the atlas with its face-label color
/// (facelabel verification mode). Sub-pixel rects round outward; rects are
/// clamped to the texture's slot. Returns `false` if a texel was already
/// painted a *different* label (rect reuse across faces — the check can't
/// apply to that texture).
fn paint_debug_rect(
    atlas: &mut [u8],
    slot: (u32, u32, u32, u32),
    q: &mobs::RawQuad,
    painted: &mut std::collections::HashMap<(u32, u32), [u8; 3]>,
) -> bool {
    let (ox, oy, tw, th) = slot;
    let (min_u, max_u) = q.uv.iter().fold((f32::MAX, f32::MIN), |(lo, hi), p| {
        (lo.min(p[0]), hi.max(p[0]))
    });
    let (min_v, max_v) = q.uv.iter().fold((f32::MAX, f32::MIN), |(lo, hi), p| {
        (lo.min(p[1]), hi.max(p[1]))
    });
    let color = q.facing.debug_color();
    let [r, g, b] = color;
    let (x0, x1) = (
        (min_u.floor().max(0.0)) as u32,
        (max_u.ceil().max(0.0) as u32).min(tw),
    );
    let (y0, y1) = (
        (min_v.floor().max(0.0)) as u32,
        (max_v.ceil().max(0.0) as u32).min(th),
    );
    let mut clean = true;
    for y in y0..y1 {
        for x in x0..x1 {
            match painted.insert((ox + x, oy + y), color) {
                Some(prev) if prev != color => clean = false,
                _ => {}
            }
            let i = (((oy + y) * ATLAS_W + ox + x) * 4) as usize;
            atlas[i..i + 4].copy_from_slice(&[r, g, b, 255]);
        }
    }
    clean
}

/// sRGB component → linear (CPU-side color prep; discipline #1).
pub fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Linear component → sRGB, [`srgb_to_linear`]'s inverse.
pub fn linear_to_srgb(c: f32) -> f32 {
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

fn linear_to_srgb3(c: [f32; 3]) -> [f32; 3] {
    c.map(linear_to_srgb)
}

/// M433 — an entity vertex colour as vanilla's `vertexColor`: the face shade
/// times the layer's tint (`tint` in vanilla's space, bytes over 255),
/// multiplied on gamma-encoded numbers as `entity.fsh` does, and handed to
/// the pass linear (`entity.frag` re-encodes it before multiplying by the
/// texel and the light). Until M433 the shade multiplied the linear texel, so
/// a face shade of 0.5 acted as 0.73.
pub(crate) fn gamma_vertex(shade: f32, tint: [f32; 3]) -> [f32; 4] {
    if tint == [1.0; 3] {
        let s = srgb_to_linear(shade);
        return [s, s, s, 1.0];
    }
    let [r, g, b] = tint.map(|t| srgb_to_linear(shade * t));
    [r, g, b, 1.0]
}

/// Blit a `w`×`h` RGBA entity skin into the atlas at (x, y). Returns whether
/// it was present + correctly sized.
/// M299 — [`blit_tex`] plus a one-texel border of the sheet's own edge
/// texels around it (corners included), the atlas's stand-in for a
/// clamp-to-edge sampler. `(x, y)` is the sheet's origin; the border lies at
/// `x - 1 ..= x + w` and `y - 1 ..= y + h`.
fn blit_tex_extruded(atlas: &mut [u8], px: &[u8], x: u32, y: u32, w: u32, h: u32) -> bool {
    if x == 0 || y == 0 || !blit_tex(atlas, Some(px), x, y, w, h) {
        return false;
    }
    let (x, y, w, h) = (x as usize, y as usize, w as usize, h as usize);
    let aw = ATLAS_W as usize;
    let at = |cx: usize, cy: usize| (cy * aw + cx) * 4;
    // Rows above and below, then whole columns left and right (which also
    // fills the four corners from the rows just written).
    for (dst_row, src_row) in [(y - 1, y), (y + h, y + h - 1)] {
        let (d, s) = (at(x, dst_row), at(x, src_row));
        atlas.copy_within(s..s + w * 4, d);
    }
    for row in y - 1..=y + h {
        let (d, s) = (at(x - 1, row), at(x, row));
        atlas.copy_within(s..s + 4, d);
        let (d, s) = (at(x + w, row), at(x + w - 1, row));
        atlas.copy_within(s..s + 4, d);
    }
    true
}

fn blit_tex(atlas: &mut [u8], tex: Option<&[u8]>, x: u32, y: u32, w: u32, h: u32) -> bool {
    let (w, h) = (w as usize, h as usize);
    match tex {
        Some(px) if px.len() == w * h * 4 => {
            for row in 0..h {
                let src = row * w * 4;
                let dst = ((y as usize + row) * ATLAS_W as usize + x as usize) * 4;
                atlas[dst..dst + w * 4].copy_from_slice(&px[src..src + w * 4]);
            }
            true
        }
        _ => false,
    }
}

/// Copy `rgba` (`width*height*4`) into an already-initialized, already-
/// sampled atlas `image` at offset (x, y): SHADER_READ_ONLY → TRANSFER_DST
/// → SHADER_READ_ONLY. Asynchronous: the leading barrier's first scope covers
/// every earlier submission, so frames already queued finish sampling before
/// the copy, and later frames see it; the staging buffer and command pool are
/// released through deferred destruction once the next frame retires. No
/// idle, no fence wait.
pub(crate) fn upload_region(
    gpu: &mut Gpu,
    image: vk::Image,
    rgba: &[u8],
    x: u32,
    y: u32,
    width: u32,
    height: u32,
) -> Result<(), String> {
    let expect = (width * height * 4) as usize;
    if rgba.len() != expect {
        return Err(format!(
            "upload_region: {} bytes, want {expect}",
            rgba.len()
        ));
    }
    submit_region_copy(gpu, image, rgba, x, y, width, height, false)
}

/// [`upload_region`] for a one-byte-per-texel (`R8_UNORM`) image: the Velvet
/// glyph atlas's per-glyph updates. Same ordering argument, same no-wait
/// submit.
pub(crate) fn upload_region_r8(
    gpu: &mut Gpu,
    image: vk::Image,
    data: &[u8],
    x: u32,
    y: u32,
    width: u32,
    height: u32,
) -> Result<(), String> {
    let expect = (width * height) as usize;
    if data.len() != expect {
        return Err(format!(
            "upload_region_r8: {} bytes, want {expect}",
            data.len()
        ));
    }
    submit_region_copy(gpu, image, data, x, y, width, height, false)
}

/// Create an `R8_UNORM` sampled image of `width`×`height` and upload `data`
/// into it **without waiting**: the copy is a queue submission ordered before
/// every later frame (submission order + the trailing barrier), and its
/// staging buffer is released through deferred destruction. The
/// Velvet glyph atlas (M52b) is the user, and it is on the gameplay path, so
/// unlike [`create_texture`] this never waits on a fence. UNORM, not SRGB:
/// the atlas is a coverage mask, and an sRGB decode would make every glyph
/// edge wrong.
pub(crate) fn create_texture_r8_async(
    gpu: &mut Gpu,
    data: &[u8],
    width: u32,
    height: u32,
) -> Result<(vk::Image, Allocation, vk::ImageView), String> {
    let expect = (width * height) as usize;
    if data.len() != expect {
        return Err(format!(
            "create_texture_r8_async: {} bytes, want {expect}",
            data.len()
        ));
    }
    let format = vk::Format::R8_UNORM;
    unsafe {
        let device = gpu.device.clone();
        let image = device
            .create_image(
                &vk::ImageCreateInfo::default()
                    .image_type(vk::ImageType::TYPE_2D)
                    .format(format)
                    .extent(vk::Extent3D {
                        width,
                        height,
                        depth: 1,
                    })
                    .mip_levels(1)
                    .array_layers(1)
                    .samples(vk::SampleCountFlags::TYPE_1)
                    .tiling(vk::ImageTiling::OPTIMAL)
                    .usage(vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_DST)
                    .initial_layout(vk::ImageLayout::UNDEFINED),
                None,
            )
            .map_err(|e| format!("r8 image: {e}"))?;
        let req = device.get_image_memory_requirements(image);
        let alloc = match gpu.allocator.allocate(&AllocationCreateDesc {
            name: "glyph-atlas",
            requirements: req,
            location: MemoryLocation::GpuOnly,
            linear: false,
            allocation_scheme: AllocationScheme::GpuAllocatorManaged,
        }) {
            Ok(a) => a,
            Err(e) => {
                device.destroy_image(image, None);
                return Err(format!("r8 alloc: {e}"));
            }
        };
        let fail = |gpu: &mut Gpu, alloc: Allocation, view: Option<vk::ImageView>| {
            if let Some(v) = view {
                gpu.device.destroy_image_view(v, None);
            }
            gpu.device.destroy_image(image, None);
            let _ = gpu.allocator.free(alloc);
        };
        if let Err(e) = device.bind_image_memory(image, alloc.memory(), alloc.offset()) {
            fail(gpu, alloc, None);
            return Err(format!("r8 bind: {e}"));
        }
        let view = match device.create_image_view(
            &vk::ImageViewCreateInfo::default()
                .image(image)
                .view_type(vk::ImageViewType::TYPE_2D)
                .format(format)
                .subresource_range(
                    vk::ImageSubresourceRange::default()
                        .aspect_mask(vk::ImageAspectFlags::COLOR)
                        .level_count(1)
                        .layer_count(1),
                ),
            None,
        ) {
            Ok(v) => v,
            Err(e) => {
                fail(gpu, alloc, None);
                return Err(format!("r8 view: {e}"));
            }
        };
        if let Err(e) = submit_region_copy(gpu, image, data, 0, 0, width, height, true) {
            // Nothing was submitted that names the image unless the submit
            // itself failed after recording, and then nothing executes.
            fail(gpu, alloc, Some(view));
            return Err(e);
        }
        Ok((image, alloc, view))
    }
}

/// Record and submit one staged buffer→image copy of a `width`×`height`
/// region at (x, y), tightly packed in `data`, with no fence wait.
///
/// `from_undefined` is a freshly created image: its leading barrier discards
/// (UNDEFINED, no first scope). Otherwise the image is already sampled, so
/// the leading barrier's first scope is every earlier fragment-shader read,
/// which covers frames already submitted.
#[allow(clippy::too_many_arguments)]
fn submit_region_copy(
    gpu: &mut Gpu,
    image: vk::Image,
    rgba: &[u8],
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    from_undefined: bool,
) -> Result<(), String> {
    gpu.collect_garbage();
    unsafe {
        let device = gpu.device.clone();
        let staging = device
            .create_buffer(
                &vk::BufferCreateInfo::default()
                    .size(rgba.len() as u64)
                    .usage(vk::BufferUsageFlags::TRANSFER_SRC)
                    .sharing_mode(vk::SharingMode::EXCLUSIVE),
                None,
            )
            .map_err(|e| format!("skin staging: {e}"))?;
        let sreq = device.get_buffer_memory_requirements(staging);
        let mut salloc = gpu
            .allocator
            .allocate(&AllocationCreateDesc {
                name: "skin-staging",
                requirements: sreq,
                location: MemoryLocation::CpuToGpu,
                linear: true,
                allocation_scheme: AllocationScheme::GpuAllocatorManaged,
            })
            .map_err(|e| format!("skin staging alloc: {e}"))?;
        device
            .bind_buffer_memory(staging, salloc.memory(), salloc.offset())
            .map_err(|e| format!("skin staging bind: {e}"))?;
        salloc.mapped_slice_mut().ok_or("skin staging not mapped")?[..rgba.len()]
            .copy_from_slice(rgba);

        let pool = device
            .create_command_pool(
                &vk::CommandPoolCreateInfo::default().queue_family_index(gpu.graphics_family),
                None,
            )
            .map_err(|e| format!("skin pool: {e}"))?;
        let cb = device
            .allocate_command_buffers(
                &vk::CommandBufferAllocateInfo::default()
                    .command_pool(pool)
                    .level(vk::CommandBufferLevel::PRIMARY)
                    .command_buffer_count(1),
            )
            .map_err(|e| format!("skin cb: {e}"))?[0];
        device
            .begin_command_buffer(
                cb,
                &vk::CommandBufferBeginInfo::default()
                    .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
            )
            .map_err(|e| format!("skin begin: {e}"))?;
        let range = vk::ImageSubresourceRange::default()
            .aspect_mask(vk::ImageAspectFlags::COLOR)
            .level_count(1)
            .layer_count(1);
        let to_dst = if from_undefined {
            vk::ImageMemoryBarrier2::default()
                .src_stage_mask(vk::PipelineStageFlags2::NONE)
                .old_layout(vk::ImageLayout::UNDEFINED)
        } else {
            vk::ImageMemoryBarrier2::default()
                .src_stage_mask(vk::PipelineStageFlags2::FRAGMENT_SHADER)
                .src_access_mask(vk::AccessFlags2::SHADER_SAMPLED_READ)
                .old_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
        }
        .dst_stage_mask(vk::PipelineStageFlags2::TRANSFER)
        .dst_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
        .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
        .image(image)
        .subresource_range(range);
        device.cmd_pipeline_barrier2(
            cb,
            &vk::DependencyInfo::default().image_memory_barriers(std::slice::from_ref(&to_dst)),
        );
        device.cmd_copy_buffer_to_image(
            cb,
            staging,
            image,
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            &[vk::BufferImageCopy::default()
                .image_subresource(
                    vk::ImageSubresourceLayers::default()
                        .aspect_mask(vk::ImageAspectFlags::COLOR)
                        .layer_count(1),
                )
                .image_offset(vk::Offset3D {
                    x: x as i32,
                    y: y as i32,
                    z: 0,
                })
                .image_extent(vk::Extent3D {
                    width,
                    height,
                    depth: 1,
                })],
        );
        let to_read = vk::ImageMemoryBarrier2::default()
            .src_stage_mask(vk::PipelineStageFlags2::TRANSFER)
            .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
            .dst_stage_mask(vk::PipelineStageFlags2::FRAGMENT_SHADER)
            .dst_access_mask(vk::AccessFlags2::SHADER_SAMPLED_READ)
            .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
            .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
            .image(image)
            .subresource_range(range);
        device.cmd_pipeline_barrier2(
            cb,
            &vk::DependencyInfo::default().image_memory_barriers(std::slice::from_ref(&to_read)),
        );
        device
            .end_command_buffer(cb)
            .map_err(|e| format!("skin end: {e}"))?;
        let cbs = [vk::CommandBufferSubmitInfo::default().command_buffer(cb)];
        let submitted = device.queue_submit2(
            gpu.graphics_queue,
            &[vk::SubmitInfo2::default().command_buffer_infos(&cbs)],
            vk::Fence::null(),
        );
        gpu.defer_destroy(move |g| {
            g.device.destroy_command_pool(pool, None);
            g.device.destroy_buffer(staging, None);
            let _ = g.allocator.free(salloc);
        });
        submitted.map_err(|e| format!("skin submit: {e}"))?;
    }
    Ok(())
}

pub(crate) fn create_texture(
    gpu: &mut Gpu,
    rgba: &[u8],
    width: u32,
    height: u32,
) -> Result<(vk::Image, Allocation, vk::ImageView), String> {
    create_texture_fmt(gpu, rgba, width, height, vk::Format::R8G8B8A8_SRGB)
}

/// A glint sheet, uploaded **UNORM** (M50).
///
/// Vanilla binds no sRGB texture views, so its `texture(Sampler0, ...)` hands
/// `core/glint.fsh` the raw byte/255 and every number downstream — including
/// the `(SRC_COLOR, ONE)` square — is gamma-encoded. Sampling the same sheet
/// through an sRGB view would hand the shader a linearised value instead, and
/// squaring that is a different quantity entirely.
pub(crate) fn create_glint_texture(
    gpu: &mut Gpu,
    rgba: &[u8],
    width: u32,
    height: u32,
) -> Result<(vk::Image, Allocation, vk::ImageView), String> {
    create_texture_fmt(gpu, rgba, width, height, vk::Format::R8G8B8A8_UNORM)
}

pub(crate) fn create_texture_fmt(
    gpu: &mut Gpu,
    rgba: &[u8],
    width: u32,
    height: u32,
    format: vk::Format,
) -> Result<(vk::Image, Allocation, vk::ImageView), String> {
    unsafe {
        let device = gpu.device.clone();
        let image = device
            .create_image(
                &vk::ImageCreateInfo::default()
                    .image_type(vk::ImageType::TYPE_2D)
                    .format(format)
                    .extent(vk::Extent3D {
                        width,
                        height,
                        depth: 1,
                    })
                    .mip_levels(1)
                    .array_layers(1)
                    .samples(vk::SampleCountFlags::TYPE_1)
                    .tiling(vk::ImageTiling::OPTIMAL)
                    .usage(vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_DST)
                    .initial_layout(vk::ImageLayout::UNDEFINED),
                None,
            )
            .map_err(|e| format!("font image: {e}"))?;
        let req = device.get_image_memory_requirements(image);
        let alloc = gpu
            .allocator
            .allocate(&AllocationCreateDesc {
                name: "font-atlas",
                requirements: req,
                location: MemoryLocation::GpuOnly,
                linear: false,
                allocation_scheme: AllocationScheme::GpuAllocatorManaged,
            })
            .map_err(|e| format!("font alloc: {e}"))?;
        device
            .bind_image_memory(image, alloc.memory(), alloc.offset())
            .map_err(|e| format!("font bind: {e}"))?;

        // One-shot staged upload (transient pool — init-time only).
        let staging = device
            .create_buffer(
                &vk::BufferCreateInfo::default()
                    .size(rgba.len() as u64)
                    .usage(vk::BufferUsageFlags::TRANSFER_SRC)
                    .sharing_mode(vk::SharingMode::EXCLUSIVE),
                None,
            )
            .map_err(|e| format!("font staging: {e}"))?;
        let sreq = device.get_buffer_memory_requirements(staging);
        let mut salloc = gpu
            .allocator
            .allocate(&AllocationCreateDesc {
                name: "font-staging",
                requirements: sreq,
                location: MemoryLocation::CpuToGpu,
                linear: true,
                allocation_scheme: AllocationScheme::GpuAllocatorManaged,
            })
            .map_err(|e| format!("font staging alloc: {e}"))?;
        device
            .bind_buffer_memory(staging, salloc.memory(), salloc.offset())
            .map_err(|e| format!("font staging bind: {e}"))?;
        salloc.mapped_slice_mut().ok_or("font staging not mapped")?[..rgba.len()]
            .copy_from_slice(rgba);

        let pool = device
            .create_command_pool(
                &vk::CommandPoolCreateInfo::default().queue_family_index(gpu.graphics_family),
                None,
            )
            .map_err(|e| format!("font pool: {e}"))?;
        let cb = device
            .allocate_command_buffers(
                &vk::CommandBufferAllocateInfo::default()
                    .command_pool(pool)
                    .level(vk::CommandBufferLevel::PRIMARY)
                    .command_buffer_count(1),
            )
            .map_err(|e| format!("font cb: {e}"))?[0];
        device
            .begin_command_buffer(
                cb,
                &vk::CommandBufferBeginInfo::default()
                    .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
            )
            .map_err(|e| format!("font begin: {e}"))?;
        let range = vk::ImageSubresourceRange::default()
            .aspect_mask(vk::ImageAspectFlags::COLOR)
            .level_count(1)
            .layer_count(1);
        let to_dst = vk::ImageMemoryBarrier2::default()
            .src_stage_mask(vk::PipelineStageFlags2::NONE)
            .dst_stage_mask(vk::PipelineStageFlags2::TRANSFER)
            .dst_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
            .old_layout(vk::ImageLayout::UNDEFINED)
            .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
            .image(image)
            .subresource_range(range);
        device.cmd_pipeline_barrier2(
            cb,
            &vk::DependencyInfo::default().image_memory_barriers(std::slice::from_ref(&to_dst)),
        );
        device.cmd_copy_buffer_to_image(
            cb,
            staging,
            image,
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            &[vk::BufferImageCopy::default()
                .image_subresource(
                    vk::ImageSubresourceLayers::default()
                        .aspect_mask(vk::ImageAspectFlags::COLOR)
                        .layer_count(1),
                )
                .image_extent(vk::Extent3D {
                    width,
                    height,
                    depth: 1,
                })],
        );
        let to_read = vk::ImageMemoryBarrier2::default()
            .src_stage_mask(vk::PipelineStageFlags2::TRANSFER)
            .src_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
            .dst_stage_mask(vk::PipelineStageFlags2::FRAGMENT_SHADER)
            .dst_access_mask(vk::AccessFlags2::SHADER_SAMPLED_READ)
            .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
            .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
            .image(image)
            .subresource_range(range);
        device.cmd_pipeline_barrier2(
            cb,
            &vk::DependencyInfo::default().image_memory_barriers(std::slice::from_ref(&to_read)),
        );
        device
            .end_command_buffer(cb)
            .map_err(|e| format!("font end: {e}"))?;
        let fence = device
            .create_fence(&vk::FenceCreateInfo::default(), None)
            .map_err(|e| format!("font fence: {e}"))?;
        let cbs = [vk::CommandBufferSubmitInfo::default().command_buffer(cb)];
        device
            .queue_submit2(
                gpu.graphics_queue,
                &[vk::SubmitInfo2::default().command_buffer_infos(&cbs)],
                fence,
            )
            .map_err(|e| format!("font submit: {e}"))?;
        device
            .wait_for_fences(&[fence], true, u64::MAX)
            .map_err(|e| format!("font wait: {e}"))?;
        device.destroy_fence(fence, None);
        device.destroy_command_pool(pool, None);
        device.destroy_buffer(staging, None);
        let _ = gpu.allocator.free(salloc);

        let view = device
            .create_image_view(
                &vk::ImageViewCreateInfo::default()
                    .image(image)
                    .view_type(vk::ImageViewType::TYPE_2D)
                    .format(format)
                    .subresource_range(range),
                None,
            )
            .map_err(|e| format!("font view: {e}"))?;
        Ok((image, alloc, view))
    }
}

/// The entity glint's pipeline (M45) — `build_pipeline`'s solid variant with
/// `RenderPipelines.GLINT`'s three differences, over the entity vertex layout.
fn build_glint_pipeline(
    device: &ash::Device,
    layout: vk::PipelineLayout,
    color_format: vk::Format,
) -> Result<vk::Pipeline, String> {
    build_sheet_pipeline(
        device,
        layout,
        color_format,
        include_bytes!(concat!(env!("OUT_DIR"), "/entity_glint.frag.spv")),
        SheetBlend::Glint,
    )
}

/// The two gamma-space sheet pipelines over entity geometry.
#[derive(Clone, Copy, PartialEq)]
enum SheetBlend {
    /// `RenderPipelines.GLINT`: `(SRC_COLOR, ONE)`, depth `EQUAL`, no write.
    Glint,
    /// M285 — `ENERGY_SWIRL`: `BlendFunction.ADDITIVE` `(ONE, ONE)`, depth
    /// `GREATER_OR_EQUAL` with writes (`DepthStencilState.DEFAULT`).
    Swirl,
    /// M286 — `BREEZE_WIND`: `BlendFunction.TRANSLUCENT` `(SRC_ALPHA,
    /// ONE_MINUS_SRC_ALPHA)`, depth `GREATER_OR_EQUAL` with writes.
    Wind,
}

/// M287 — the energy-swirl sheets, one render type each.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SwirlSheet {
    /// `CreeperPowerLayer`, `creeper_armor.png`.
    Creeper = 0,
    /// `WitherArmorLayer`, `wither_armor.png`.
    Wither = 1,
}

/// `energySwirl(texture, xOffset(t) % 1, t * 0.01 % 1)`'s two offsets:
/// `CreeperPowerLayer.xOffset(t) = t * 0.01`, `WitherArmorLayer.xOffset(t)
/// = Mth.cos(t * 0.02) * 3` (Java's float `%` keeps the sign; the sampler
/// repeats either way).
pub fn swirl_offsets(which: SwirlSheet, t: f32) -> [f32; 2] {
    let x = match which {
        SwirlSheet::Creeper => t * 0.01,
        SwirlSheet::Wither => mth_cos((t * 0.02) as f64) * 3.0,
    };
    [x % 1.0, (t * 0.01) % 1.0]
}

/// `0xFF808080`'s grey, the swirl's vertex colour (gamma; the pass is).
pub(crate) const SWIRL_GREY: f32 = 128.0 / 255.0;

fn build_sheet_pipeline(
    device: &ash::Device,
    layout: vk::PipelineLayout,
    color_format: vk::Format,
    frag_spv: &[u8],
    blend: SheetBlend,
) -> Result<vk::Pipeline, String> {
    unsafe {
        let vert = crate::overlay::create_shader(
            device,
            include_bytes!(concat!(env!("OUT_DIR"), "/entity.vert.spv")),
        )?;
        let frag = crate::overlay::create_shader(device, frag_spv)?;
        let entry = c"main";
        let stages = [
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::VERTEX)
                .module(vert)
                .name(entry),
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::FRAGMENT)
                .module(frag)
                .name(entry),
        ];
        let bindings = [vk::VertexInputBindingDescription::default()
            .binding(0)
            .stride(std::mem::size_of::<Vertex>() as u32)
            .input_rate(vk::VertexInputRate::VERTEX)];
        let attrs = [
            vk::VertexInputAttributeDescription::default()
                .location(0)
                .format(vk::Format::R32G32B32_SFLOAT)
                .offset(0),
            vk::VertexInputAttributeDescription::default()
                .location(1)
                .format(vk::Format::R32G32_SFLOAT)
                .offset(12),
            vk::VertexInputAttributeDescription::default()
                .location(2)
                .format(vk::Format::R32G32B32A32_SFLOAT)
                .offset(20),
            vk::VertexInputAttributeDescription::default()
                .location(3)
                .format(vk::Format::R32G32B32A32_SFLOAT)
                .offset(36),
        ];
        let vertex_input = vk::PipelineVertexInputStateCreateInfo::default()
            .vertex_binding_descriptions(&bindings)
            .vertex_attribute_descriptions(&attrs);
        let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
            .topology(vk::PrimitiveTopology::TRIANGLE_LIST);
        let viewport = vk::PipelineViewportStateCreateInfo::default()
            .viewport_count(1)
            .scissor_count(1);
        let raster = vk::PipelineRasterizationStateCreateInfo::default()
            .polygon_mode(vk::PolygonMode::FILL)
            .cull_mode(vk::CullModeFlags::NONE)
            .front_face(vk::FrontFace::COUNTER_CLOCKWISE)
            .line_width(1.0);
        let multisample = vk::PipelineMultisampleStateCreateInfo::default()
            .rasterization_samples(vk::SampleCountFlags::TYPE_1);
        // EQUAL, no write. The world pass is reversed-Z, so its ordinary test
        // is GREATER — but equality is equality either way, and it is what
        // lands the sheen on the item's own fragments.
        let depth = match blend {
            SheetBlend::Glint => vk::PipelineDepthStencilStateCreateInfo::default()
                .depth_test_enable(true)
                .depth_write_enable(false)
                .depth_compare_op(vk::CompareOp::EQUAL),
            SheetBlend::Swirl | SheetBlend::Wind => {
                vk::PipelineDepthStencilStateCreateInfo::default()
                    .depth_test_enable(true)
                    .depth_write_enable(true)
                    .depth_compare_op(vk::CompareOp::GREATER_OR_EQUAL)
            }
        };
        let src = match blend {
            SheetBlend::Glint => vk::BlendFactor::SRC_COLOR,
            SheetBlend::Swirl => vk::BlendFactor::ONE,
            SheetBlend::Wind => vk::BlendFactor::SRC_ALPHA,
        };
        let dst = match blend {
            SheetBlend::Glint | SheetBlend::Swirl => vk::BlendFactor::ONE,
            SheetBlend::Wind => vk::BlendFactor::ONE_MINUS_SRC_ALPHA,
        };
        let blend_attachments = [vk::PipelineColorBlendAttachmentState::default()
            .blend_enable(true)
            .src_color_blend_factor(src)
            .dst_color_blend_factor(dst)
            .color_blend_op(vk::BlendOp::ADD)
            .src_alpha_blend_factor(vk::BlendFactor::ZERO)
            .dst_alpha_blend_factor(vk::BlendFactor::ONE)
            .alpha_blend_op(vk::BlendOp::ADD)
            .color_write_mask(
                vk::ColorComponentFlags::R
                    | vk::ColorComponentFlags::G
                    | vk::ColorComponentFlags::B
                    | vk::ColorComponentFlags::A,
            )];
        let blend_state =
            vk::PipelineColorBlendStateCreateInfo::default().attachments(&blend_attachments);
        let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
        let dynamic = vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);
        let color_formats = [color_format];
        let mut rendering = vk::PipelineRenderingCreateInfo::default()
            .color_attachment_formats(&color_formats)
            .depth_attachment_format(crate::world::DEPTH_FORMAT);
        let ci = vk::GraphicsPipelineCreateInfo::default()
            .stages(&stages)
            .vertex_input_state(&vertex_input)
            .input_assembly_state(&input_assembly)
            .viewport_state(&viewport)
            .rasterization_state(&raster)
            .multisample_state(&multisample)
            .depth_stencil_state(&depth)
            .color_blend_state(&blend_state)
            .dynamic_state(&dynamic)
            .layout(layout)
            .push_next(&mut rendering);
        let pipeline = device
            .create_graphics_pipelines(vk::PipelineCache::null(), &[ci], None)
            .map_err(|(_, e)| format!("entity glint pipeline: {e}"))?[0];
        device.destroy_shader_module(vert, None);
        device.destroy_shader_module(frag, None);
        Ok(pipeline)
    }
}

/// The entity pipeline.
///
/// `compare` is the depth test. The world pass is reversed-Z, so the ordinary
/// one is `GREATER`; the trim pass (M48) uses `EQUAL` with no write, which is
/// `ARMOR_DECAL_CUTOUT_NO_CULL`'s `DepthStencilState(CompareOp.EQUAL, false)`
/// and lands the decoration on exactly the armour fragments that won.
/// M278 — the outline geometry's pipeline: the entity vertex layout and
/// vertex shader, `outline_geometry.frag` (flat outline colour where the
/// texture is not transparent), into the UNORM outline target with its own
/// reversed-Z depth, unblended.
pub(crate) fn build_outline_pipeline(
    device: &ash::Device,
    layout: vk::PipelineLayout,
) -> Result<vk::Pipeline, String> {
    build_pipeline_with(
        device,
        layout,
        crate::world::OUTLINE_FORMAT,
        true,
        vk::CompareOp::GREATER,
        include_bytes!(concat!(env!("OUT_DIR"), "/outline_geometry.frag.spv")),
        true,
    )
}

fn build_pipeline(
    device: &ash::Device,
    layout: vk::PipelineLayout,
    color_format: vk::Format,
    solid: bool,
    compare: vk::CompareOp,
) -> Result<vk::Pipeline, String> {
    build_pipeline_with(
        device,
        layout,
        color_format,
        solid,
        compare,
        include_bytes!(concat!(env!("OUT_DIR"), "/entity.frag.spv")),
        false,
    )
}

fn build_pipeline_with(
    device: &ash::Device,
    layout: vk::PipelineLayout,
    color_format: vk::Format,
    solid: bool,
    compare: vk::CompareOp,
    frag_spv: &[u8],
    // M278 — write alpha too (the outline target's alpha is the silhouette).
    write_alpha: bool,
) -> Result<vk::Pipeline, String> {
    build_pipeline_full(
        device,
        layout,
        color_format,
        PipelineState {
            blend: !solid,
            depth_write: solid,
            color_write: true,
        },
        compare,
        frag_spv,
        write_alpha,
    )
}

/// M306 — blend and depth write chosen apart: `ENTITY_TRANSLUCENT` is the
/// first entity pipeline that both blends and writes depth.
#[derive(Clone, Copy)]
struct PipelineState {
    blend: bool,
    depth_write: bool,
    /// M363 — `false` for a depth-only draw (`RenderTypes.waterMask()`).
    color_write: bool,
}

fn build_pipeline_full(
    device: &ash::Device,
    layout: vk::PipelineLayout,
    color_format: vk::Format,
    state: PipelineState,
    compare: vk::CompareOp,
    frag_spv: &[u8],
    write_alpha: bool,
) -> Result<vk::Pipeline, String> {
    unsafe {
        let vert = crate::overlay::create_shader(
            device,
            include_bytes!(concat!(env!("OUT_DIR"), "/entity.vert.spv")),
        )?;
        let frag = crate::overlay::create_shader(device, frag_spv)?;
        let entry = c"main";
        let stages = [
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::VERTEX)
                .module(vert)
                .name(entry),
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::FRAGMENT)
                .module(frag)
                .name(entry),
        ];
        let bindings = [vk::VertexInputBindingDescription::default()
            .binding(0)
            .stride(VERTEX_STRIDE as u32)
            .input_rate(vk::VertexInputRate::VERTEX)];
        let attrs = [
            vk::VertexInputAttributeDescription::default()
                .location(0)
                .format(vk::Format::R32G32B32_SFLOAT)
                .offset(0),
            vk::VertexInputAttributeDescription::default()
                .location(1)
                .format(vk::Format::R32G32_SFLOAT)
                .offset(12),
            vk::VertexInputAttributeDescription::default()
                .location(2)
                .format(vk::Format::R32G32B32A32_SFLOAT)
                .offset(20),
            // rgb = world light, a = the hurt flag (M21).
            vk::VertexInputAttributeDescription::default()
                .location(3)
                .format(vk::Format::R32G32B32A32_SFLOAT)
                .offset(36),
        ];
        let vertex_input = vk::PipelineVertexInputStateCreateInfo::default()
            .vertex_binding_descriptions(&bindings)
            .vertex_attribute_descriptions(&attrs);
        let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
            .topology(vk::PrimitiveTopology::TRIANGLE_LIST);
        let viewport = vk::PipelineViewportStateCreateInfo::default()
            .viewport_count(1)
            .scissor_count(1);
        let raster = vk::PipelineRasterizationStateCreateInfo::default()
            .polygon_mode(vk::PolygonMode::FILL)
            .cull_mode(vk::CullModeFlags::NONE)
            .front_face(vk::FrontFace::COUNTER_CLOCKWISE)
            .line_width(1.0);
        let multisample = vk::PipelineMultisampleStateCreateInfo::default()
            .rasterization_samples(vk::SampleCountFlags::TYPE_1);
        let depth = vk::PipelineDepthStencilStateCreateInfo::default()
            .depth_test_enable(true)
            .depth_write_enable(state.depth_write)
            .depth_compare_op(compare);
        let blend_attachments = [vk::PipelineColorBlendAttachmentState::default()
            .blend_enable(state.blend)
            .src_color_blend_factor(vk::BlendFactor::SRC_ALPHA)
            .dst_color_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
            .color_blend_op(vk::BlendOp::ADD)
            .src_alpha_blend_factor(vk::BlendFactor::ONE)
            .dst_alpha_blend_factor(vk::BlendFactor::ZERO)
            .alpha_blend_op(vk::BlendOp::ADD)
            .color_write_mask(if !state.color_write {
                vk::ColorComponentFlags::empty()
            } else if write_alpha {
                vk::ColorComponentFlags::RGBA
            } else {
                vk::ColorComponentFlags::R | vk::ColorComponentFlags::G | vk::ColorComponentFlags::B
            })];
        let blend =
            vk::PipelineColorBlendStateCreateInfo::default().attachments(&blend_attachments);
        let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
        let dynamic = vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);
        let color_formats = [color_format];
        let mut rendering = vk::PipelineRenderingCreateInfo::default()
            .color_attachment_formats(&color_formats)
            .depth_attachment_format(DEPTH_FORMAT);
        let ci = vk::GraphicsPipelineCreateInfo::default()
            .stages(&stages)
            .vertex_input_state(&vertex_input)
            .input_assembly_state(&input_assembly)
            .viewport_state(&viewport)
            .rasterization_state(&raster)
            .multisample_state(&multisample)
            .depth_stencil_state(&depth)
            .color_blend_state(&blend)
            .dynamic_state(&dynamic)
            .layout(layout)
            .push_next(&mut rendering);
        let pipeline = device
            .create_graphics_pipelines(vk::PipelineCache::null(), std::slice::from_ref(&ci), None)
            .map_err(|(_, e)| format!("entity pipeline: {e}"))?[0];
        device.destroy_shader_module(vert, None);
        device.destroy_shader_module(frag, None);
        Ok(pipeline)
    }
}

#[cfg(test)]
mod tests {
    /// M240 — the billboard faces the camera: looking south (yaw 0) the
    /// quad's +Z points north, back at the camera; looking east (yaw -90)
    /// it points west; looking straight down (pitch 90) it points up.
    #[test]
    fn a_billboard_turns_its_front_to_the_camera() {
        let n = |yaw: f32, pitch: f32| {
            super::Billboard {
                cam_yaw: yaw,
                cam_pitch: pitch,
                scale: 1.0,
                tint: None,
            }
            .place([0.0, 0.0, 1.0])
        };
        let close = |a: [f32; 3], b: [f32; 3]| (0..3).all(|k| (a[k] - b[k]).abs() < 1e-5);
        assert!(close(n(0.0, 0.0), [0.0, 0.0, -1.0]), "{:?}", n(0.0, 0.0));
        assert!(
            close(n(-90.0, 0.0), [-1.0, 0.0, 0.0]),
            "{:?}",
            n(-90.0, 0.0)
        );
        assert!(close(n(0.0, 90.0), [0.0, 1.0, 0.0]), "{:?}", n(0.0, 90.0));
        let half = super::Billboard {
            cam_yaw: 0.0,
            cam_pitch: 0.0,
            scale: 0.5,
            tint: None,
        };
        assert!(close(half.place([1.0, 0.0, 0.0]), [-0.5, 0.0, 0.0]));
    }

    use super::*;

    /// A fresh pool fills lowest-first (the order the round-robin it replaced
    /// used, so the gates' slot addresses are unchanged), and the same owner
    /// keeps its slot.
    #[test]
    fn lru_slots_fill_in_order_and_reuse_an_owners_slot() {
        let mut p: LruSlots<u128> = LruSlots::new(3);
        assert_eq!(p.claim(10), Some((0, None)));
        assert_eq!(p.claim(11), Some((1, None)));
        assert_eq!(p.claim(10), Some((0, None)), "same owner, same slot");
        assert_eq!(p.claim(12), Some((2, None)));
    }

    /// Full pool: the least recently drawn slot goes and its owner is named;
    /// a slot drawn this frame is never taken, and when all are, the claim is
    /// refused rather than overwriting a visible player.
    #[test]
    fn lru_slots_evict_least_recently_drawn_never_this_frame() {
        let mut p: LruSlots<u128> = LruSlots::new(3);
        for k in [10, 11, 12] {
            p.claim(k);
        }
        // Frame A draws 10 and 12; frame B draws only 12.
        p.begin_frame();
        p.touch(0);
        p.touch(2);
        p.begin_frame();
        p.touch(2);
        // 11 was last drawn longest ago (at its claim), then 10.
        assert_eq!(p.claim(20), Some((1, Some(11))));
        assert_eq!(p.claim(21), Some((0, Some(10))));
        // Slot 2 (drawn this frame) and the two just claimed this frame are
        // all protected: refuse.
        assert_eq!(p.claim(22), None);
        // Next frame nothing is drawn yet: all three were last used in the
        // previous frame, so the lowest index breaks the tie.
        p.begin_frame();
        assert_eq!(p.claim(22), Some((0, Some(21))));
        // Out-of-range touches (an address from elsewhere) are ignored.
        p.touch(99);
    }

    /// `skin_slot_at` / `cape_slot_at` invert the origin functions for every
    /// slot and reject any other texel, so `set_draws` touches exactly the
    /// slot a draw's address came from.
    #[test]
    fn pool_slot_lookup_inverts_the_origin() {
        for i in 0..SKIN_SLOTS {
            let (x, y) = skin_slot_origin(i);
            assert_eq!(skin_slot_at(x, y), Some(i));
            assert_eq!(skin_slot_at(x + 1, y), None);
            // Through the f32 UV delta `upload_skin` returns, from a few
            // plausible baked-player origins.
            for origin in [(0, 0), (64, 128), (960, 1536)] {
                let uv = skin_uv_of_slot(origin, i);
                assert_eq!(skin_slot_of_uv(origin, uv), Some(i), "{origin:?}");
            }
        }
        // The default skin (delta 0) is not a pool slot.
        assert_eq!(skin_slot_of_uv((64, 128), [0.0, 0.0]), None);
        assert_eq!(skin_slot_at(0, SKIN_POOL_Y - 1), None);
        assert_eq!(
            skin_slot_at(0, SKIN_POOL_Y + SKIN_POOL_ROWS * SKIN_SLOT),
            None
        );
        for i in 0..CAPE_SLOTS {
            let (x, y) = cape_slot_origin(i);
            assert_eq!(cape_slot_at(x, y), Some(i));
            assert_eq!(cape_slot_at(x, y + 1), None);
        }
        assert_eq!(
            cape_slot_at(0, CAPE_POOL_Y + CAPE_POOL_ROWS * CAPE_SLOT_H),
            None
        );
    }

    #[test]
    fn capsule_stays_inside_the_unit_bounds() {
        for (p, n) in unit_capsule() {
            assert!((0.0..=1.0).contains(&p[1]), "y {}", p[1]);
            assert!(p[0].abs() <= 0.5 + 1e-5 && p[2].abs() <= 0.5 + 1e-5);
            let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
            assert!((len - 1.0).abs() < 1e-3, "normal not unit: {len}");
        }
    }

    #[test]
    fn srgb_linearize_endpoints() {
        assert_eq!(srgb_to_linear(0.0), 0.0);
        assert!((srgb_to_linear(1.0) - 1.0).abs() < 1e-6);
        assert!(srgb_to_linear(0.5) < 0.5, "midtones darken in linear");
    }

    /// M433 — `entity.frag` re-encodes the vertex colour, so what reaches
    /// vanilla's gamma-space multiply is `shade · tint`, not its 2.2th root.
    #[test]
    fn a_vertex_colour_re_encodes_to_vanillas_shade_times_tint() {
        for (shade, tint) in [
            (0.5, [1.0; 3]),
            (0.8, [0.6, 0.3, 0.1]),
            (1.0, [0.25, 1.0, 0.5]),
        ] {
            let v = gamma_vertex(shade, tint);
            for c in 0..3 {
                let want = shade * tint[c];
                assert!(
                    (linear_to_srgb(v[c]) - want).abs() < 1e-5,
                    "{shade} {tint:?} ch{c}"
                );
            }
            assert_eq!(v[3], 1.0);
        }
        // The pre-M433 vertex colour (the shade multiplied in linear) does not.
        assert!((linear_to_srgb(0.5) - 0.5).abs() > 0.2);
    }

    #[test]
    fn wrap_degrees_matches_vanilla() {
        assert_eq!(wrap_degrees(0.0), 0.0);
        assert_eq!(wrap_degrees(190.0), -170.0);
        assert_eq!(wrap_degrees(-190.0), 170.0);
        assert_eq!(wrap_degrees(360.0), 0.0);
        // body 350, head 10 → net +20 (turned left of body), not −340.
        assert_eq!(wrap_degrees(10.0 - 350.0), 20.0);
    }

    #[test]
    fn head_anim_matrix_matches_vanilla_rotate_zyx() {
        // Ry(net)·Rx(pitch) must equal mobs::rotate_zyx(v, [pitch, net, 0]).
        let (pitch, net) = (0.6f32, -1.1f32);
        let m = mat_mul(rot_y(net), rot_x(pitch));
        for v in [[1.0, 0.0, 0.0], [0.3, -0.7, 0.9], [0.0, 1.0, -1.0]] {
            let a = mat_apply(&m, v);
            let b = mobs::rotate_zyx(v, [pitch, net, 0.0]);
            for i in 0..3 {
                assert!((a[i] - b[i]).abs() < 1e-5, "{a:?} vs {b:?}");
            }
        }
    }

    #[test]
    fn an_extruded_blit_repeats_the_edge_texels_around_the_sheet() {
        let mut atlas = vec![0u8; (ATLAS_W * 8 * 4) as usize];
        // A 2x2 sheet: red, green / blue, white.
        let px = [
            255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,
        ];
        assert!(blit_tex_extruded(&mut atlas, &px, 3, 3, 2, 2));
        let at = |x: u32, y: u32| {
            let i = ((y * ATLAS_W + x) * 4) as usize;
            [atlas[i], atlas[i + 1], atlas[i + 2]]
        };
        assert_eq!(at(3, 3), [255, 0, 0]);
        assert_eq!(at(2, 2), [255, 0, 0], "corner from the red texel");
        assert_eq!(at(5, 2), [0, 255, 0], "corner from the green texel");
        assert_eq!(at(2, 5), [0, 0, 255], "corner from the blue texel");
        assert_eq!(at(5, 5), [255, 255, 255]);
        assert_eq!(at(4, 2), [0, 255, 0], "top edge repeats the top row");
        assert_eq!(
            at(5, 4),
            [255, 255, 255],
            "right edge repeats the right column"
        );
        assert_eq!(at(6, 4), [0, 0, 0], "nothing beyond the border");
        assert!(
            !blit_tex_extruded(&mut atlas, &px, 0, 3, 2, 2),
            "no room for a border"
        );
    }

    #[test]
    fn the_pool_origins_are_what_their_comments_say() {
        // M297 — the bands below the shelf region, anchored to `ATLAS_H`
        // (the trailing comments on them had drifted from these before M297).
        assert_eq!(
            [
                ITEM_POOL_Y,
                SKIN_POOL_Y,
                TRIM_POOL_Y,
                CAPE_POOL_Y,
                MAP_POOL_Y,
                BIG_POOL_Y
            ],
            [2688, 2944, 3072, 3200, 3264, 3392]
        );
    }

    #[test]
    fn shelf_packing_avoids_font_and_overlaps() {
        // A realistic mix: several 64², 64×32, a couple of 128², one 192².
        let mut sizes: Vec<(u32, u32)> = Vec::new();
        sizes.push((192, 192));
        for _ in 0..8 {
            sizes.push((128, 128));
        }
        for _ in 0..30 {
            sizes.push((64, 64));
        }
        for _ in 0..25 {
            sizes.push((64, 32));
        }
        sizes.push((16, 16));
        // Sorted tallest-first like the constructor does.
        sizes.sort_by_key(|&(w, h)| (std::cmp::Reverse(h), std::cmp::Reverse(w)));
        let placed = pack_shelves(&sizes);
        let mut rects: Vec<(u32, u32, u32, u32)> = Vec::new();
        for (i, p) in placed.iter().enumerate() {
            let (w, h) = sizes[i];
            let (x, y) = p.expect("everything must pack at 1024²");
            assert!(x + w <= ATLAS_W && y + h <= ATLAS_H);
            // Never inside the font block.
            assert!(x >= 128 || y >= 128, "({x},{y}) overlaps font");
            for &(rx, ry, rw, rh) in &rects {
                let overlap = x < rx + rw && rx < x + w && y < ry + rh && ry < y + h;
                assert!(!overlap, "({x},{y},{w},{h}) overlaps ({rx},{ry},{rw},{rh})");
            }
            rects.push((x, y, w, h));
        }
    }

    // ---- emissive layers, the dye table, the tendrils (M57) -------------

    /// Fabricated atlas slots for the warden and its four overlay textures,
    /// deliberately at awkward origins so a dropped or transposed offset shows
    /// up.
    fn warden_slots() -> std::collections::HashMap<&'static str, (u32, u32, u32, u32)> {
        [
            ("warden", (0u32, 0u32, 128u32, 128u32)),
            ("warden_bioluminescent", (256, 384, 128, 128)),
            ("warden_pulsating_1", (384, 256, 128, 128)),
            ("warden_pulsating_2", (512, 128, 128, 128)),
            ("warden_heart", (128, 512, 128, 128)),
        ]
        .into_iter()
        .collect()
    }

    /// No resource pack loaded — the built-in layers only.
    fn no_pack_emissive() -> std::collections::HashMap<&'static str, (u32, u32)> {
        std::collections::HashMap::new()
    }

    fn warden_def() -> &'static mobs::MobDef {
        mobs::MOBS
            .iter()
            .find(|d| d.kind == EntityModelKind::Warden)
            .expect("warden is in the registry")
    }

    /// The invariant that makes an emissive layer *the same pixels, a different
    /// texture*: every layer quad must land on the overlay texture's atlas slot
    /// at exactly the model-px texel its base quad names. Checked against the
    /// raw `mobs::Model` UVs — the input to the bake, not its output.
    #[test]
    fn emissive_quads_sample_the_overlay_slot_at_the_base_texel() {
        let def = warden_def();
        let m = (def.build)();
        let slots = warden_slots();
        let layers = build_emissive(&m, def, &slots, &no_pack_emissive());
        assert_eq!(layers.len(), 5, "WardenRenderer adds five emissive layers");

        for (layer, spec) in layers.iter().zip(mobs::emissive_layers(def.kind)) {
            let (ox, oy, _, _) = slots[spec.tex];
            // The base quads this layer should have kept, filtered the same way
            // but by an independent expression of the rule.
            let want: Vec<&mobs::RawQuad> = m
                .quads
                .iter()
                .filter(|q| match spec.parts {
                    mobs::PartFilter::All => true,
                    mobs::PartFilter::Exact(names) => names.contains(&m.parts[q.part].name),
                })
                .collect();
            assert_eq!(layer.quads.len(), want.len(), "{} quad count", spec.tex);
            for (got, raw) in layer.quads.iter().zip(&want) {
                for k in 0..4 {
                    let u = got.uv[k][0] * ATLAS_W as f32 - ox as f32;
                    let v = got.uv[k][1] * ATLAS_H as f32 - oy as f32;
                    assert!(
                        (u - raw.uv[k][0]).abs() < 1e-3 && (v - raw.uv[k][1]).abs() < 1e-3,
                        "{}: quad corner {k} sampled ({u}, {v}), base texel is {:?}",
                        spec.tex,
                        raw.uv[k]
                    );
                }
                assert_eq!(got.pos, raw.pos, "{}: geometry must be identical", spec.tex);
            }
        }
    }

    /// `retainExactParts` really excludes parts. The heart layer is body-only
    /// and the bioluminescent layer is everything *but* the body, so between
    /// them a filter that silently passed everything is impossible to miss.
    #[test]
    fn emissive_part_filters_select_vanillas_parts() {
        let def = warden_def();
        let m = (def.build)();
        let layers = build_emissive(&m, def, &warden_slots(), &no_pack_emissive());
        let body_quads = m
            .quads
            .iter()
            .filter(|q| m.parts[q.part].name == "body")
            .count();
        assert!(body_quads > 0);
        // Layer 0 = bioluminescent (head + limbs, no body).
        assert!(
            layers[0]
                .quads
                .iter()
                .all(|q| m.parts[q.part as usize].name != "body"),
            "the bioluminescent layer must not include the body"
        );
        // Layer 3 = tendrils. Each tendril is a 16x16x0 plate, so only its two
        // zero-depth faces survive `cube_faces`' area filter.
        assert_eq!(
            layers[3].quads.len(),
            4,
            "two tendril plates, two faces each"
        );
        assert!(layers[3]
            .quads
            .iter()
            .all(|q| m.parts[q.part as usize].name.ends_with("_tendril")));
        // Layer 4 = heart, body only.
        assert_eq!(layers[4].quads.len(), body_quads);
    }

    /// A layer whose overlay texture is a different size than the base can't
    /// share its UVs, so it must be dropped rather than rendered scrambled.
    #[test]
    fn emissive_layer_with_a_mismatched_texture_is_dropped() {
        let def = warden_def();
        let m = (def.build)();
        let mut slots = warden_slots();
        slots.insert("warden_heart", (128, 512, 64, 64));
        let layers = build_emissive(&m, def, &slots, &no_pack_emissive());
        assert_eq!(
            layers.len(),
            4,
            "the mismatched layer is dropped, the rest stay"
        );
        // And a missing one, likewise.
        let mut slots = warden_slots();
        slots.remove("warden_pulsating_2");
        assert_eq!(
            build_emissive(&m, def, &slots, &no_pack_emissive()).len(),
            4
        );
    }

    /// A pack's `<texture>_e.png` (ETF) becomes an always-on fullbright layer
    /// over exactly the quads that sample that texture — so on a multi-texture
    /// mob it covers one and leaves the others alone.
    #[test]
    fn a_pack_emissive_overlay_covers_only_its_own_texture() {
        let def = mobs::MOBS
            .iter()
            .find(|d| d.kind == EntityModelKind::Sheep)
            .expect("sheep is in the registry");
        let m = (def.build)();
        // Three since M68 — base, fleece, undercoat. The third makes the
        // containment claim stronger, not weaker: the overlay must now skip
        // *two* other slots, and the undercoat's quads are geometric twins of
        // the base's, so a filter keying off position rather than slot would
        // pick them up.
        assert_eq!(
            def.textures,
            &["sheep", "sheep_wool", "sheep_wool_undercoat"]
        );
        let slots: std::collections::HashMap<&'static str, (u32, u32, u32, u32)> = [
            ("sheep", (0u32, 0u32, 64u32, 32u32)),
            ("sheep_wool", (64, 0, 64, 32)),
            ("sheep_wool_undercoat", (128, 0, 64, 32)),
        ]
        .into_iter()
        .collect();
        // An overlay on the wool only.
        let pack: std::collections::HashMap<&'static str, (u32, u32)> =
            [("sheep_wool", (256u32, 128u32))].into_iter().collect();
        let layers = build_emissive(&m, def, &slots, &pack);
        assert_eq!(layers.len(), 1, "the sheep has no built-in emissive layers");
        let wool_quads = m.quads.iter().filter(|q| q.tex == 1).count();
        assert!(wool_quads > 0);
        assert_eq!(
            layers[0].quads.len(),
            wool_quads,
            "only the wool's quads glow"
        );
        assert!(
            layers[0].cutout,
            "OptiFine emissive textures are alpha-cutout"
        );
        // And it samples the overlay's slot at the wool's own texels.
        for (got, raw) in layers[0]
            .quads
            .iter()
            .zip(m.quads.iter().filter(|q| q.tex == 1))
        {
            let u = got.uv[0][0] * ATLAS_W as f32 - 256.0;
            let v = got.uv[0][1] * ATLAS_H as f32 - 128.0;
            assert!((u - raw.uv[0][0]).abs() < 1e-3 && (v - raw.uv[0][1]).abs() < 1e-3);
        }
    }

    /// The wool colours against the two rules that produced them:
    /// `DyeColor.getTextureDiffuseColor()` scaled to 0.75, with white
    /// overridden by `ColorLerper.getModifiedColor`'s literal.
    #[test]
    fn sheep_wool_colors_match_the_dye_table_at_sheep_brightness() {
        // `DyeColor`'s third constructor field, in ordinal order.
        const DIFFUSE: [u32; 16] = [
            16383998, 16351261, 13061821, 3847130, 16701501, 8439583, 15961002, 4673362, 10329495,
            1481884, 8991416, 3949738, 8606770, 6192150, 11546150, 1908001,
        ];
        let t = mobs::SHEEP_WOOL_COLORS;
        // White is special-cased outright: `-1644826` = 0xFFE6E6E6.
        assert_eq!(t[0], [0xE6, 0xE6, 0xE6]);
        assert_ne!(
            t[0],
            [
                (DIFFUSE[0] >> 16 & 255) as u8,
                (DIFFUSE[0] >> 8 & 255) as u8,
                (DIFFUSE[0] & 255) as u8
            ],
            "white must be the override, not the dimmed diffuse colour"
        );
        for i in 1..16 {
            let src = DIFFUSE[i];
            for c in 0..3 {
                let ch = (src >> (16 - 8 * c) & 255) as f32;
                assert_eq!(t[i][c], (ch * 0.75).floor() as u8, "dye {i} channel {c}");
            }
        }
    }

    /// The `AlphaFunction` bodies, against values computed by hand from the
    /// decompiled expressions.
    #[test]
    fn emissive_alpha_matches_vanillas_alpha_functions() {
        let rest = EmissiveState::default();
        let a = |f, age, s: &EmissiveState| emissive_alpha(f, age, s);
        assert_eq!(a(mobs::EmissiveAlpha::Always, 123.0, &rest), 1.0);

        // max(0, cos(age*0.045 + phi)*0.25): the two spot layers are exactly out
        // of phase, so at any age one is lit and the other is clamped off — the
        // pulse vanilla shows.
        let spots =
            |phase: f32, age: f32| a(mobs::EmissiveAlpha::PulsatingSpots { phase }, age, &rest);
        assert!((spots(0.0, 0.0) - 0.25).abs() < 1e-6);
        assert_eq!(spots(std::f32::consts::PI, 0.0), 0.0);
        // Quarter period of cos(age*0.045) is age = (pi/2)/0.045 ~ 34.9.
        assert!(spots(0.0, std::f32::consts::FRAC_PI_2 / 0.045).abs() < 1e-6);
        for age in [0.0, 7.5, 34.9, 70.0, 130.0] {
            assert!(spots(0.0, age) + spots(std::f32::consts::PI, age) > 0.0 - 1e-9);
            assert!(spots(0.0, age) <= 0.25 && spots(std::f32::consts::PI, age) <= 0.25);
        }

        // heartAnimation: 10 at the beat, decrementing to 0 over 10 ticks, then
        // flat until the next beat 40 ticks after the last.
        let heart = |age: f32| a(mobs::EmissiveAlpha::Heart, age, &rest);
        assert_eq!(heart(0.0), 1.0);
        assert!((heart(5.0) - 0.5).abs() < 1e-6);
        assert_eq!(heart(10.0), 0.0);
        assert_eq!(heart(39.9), 0.0);
        assert!(
            (heart(40.0) - 1.0).abs() < 1e-6,
            "the beat repeats every 40 ticks"
        );
        assert!((heart(82.0) - 0.8).abs() < 1e-5);

        // The two entity-state functions pass their state through, and both sit
        // at 0 for vanilla's synched defaults.
        assert_eq!(a(mobs::EmissiveAlpha::Tendril, 0.0, &rest), 0.0);
        assert_eq!(a(mobs::EmissiveAlpha::EyesGlowing, 0.0, &rest), 0.0);
        let on = EmissiveState {
            tendril: 0.7,
            eyes_glow: true,
        };
        assert_eq!(a(mobs::EmissiveAlpha::Tendril, 0.0, &on), 0.7);
        assert_eq!(a(mobs::EmissiveAlpha::EyesGlowing, 0.0, &on), 1.0);
    }

    /// M270 — an armour stand part's pose is absolute: at vanilla's default
    /// pose every part's delta is zero (the model is baked there); a head
    /// pose of (45, 10, -5) adds exactly that to the head's baked (0, 0, 0),
    /// and a right arm at (-90, 0, 0) moves from its baked (-15, 0, 10) by
    /// (-75, 0, -10).
    #[test]
    fn stand_poses_replace_the_baked_default() {
        let ctx = |stand_pose| AnimCtx {
            pitch: 0.0,
            net: 0.0,
            f: 0.0,
            pos: 0.0,
            amt: 0.0,
            age: 0.0,
            gesture: None,
            events: [None; mobs::ModelEvent::COUNT],
            shell: false,
            body_yaw: 0.0,
            stand_pose,
            allay_dance: None,
            attack: mobs::SwingPose::NONE,
            arm_poses: mobs::ArmPoses::EMPTY,
            mob: mobs::MobCombat::default(),
            tendril: 0.0,
        };
        let close = |a: [f32; 3], b: [f32; 3]| (0..3).all(|k| (a[k] - b[k]).abs() < 1e-6);
        let rad = |v: [f32; 3]| v.map(f32::to_radians);
        for i in 0..6u8 {
            let (r, _) = anim_delta(
                mobs::Anim::StandPose(i),
                1.0,
                &ctx(mobs::STAND_DEFAULT_POSE),
            );
            assert!(close(r, [0.0; 3]), "part {i}: {r:?}");
        }
        let mut pose = mobs::STAND_DEFAULT_POSE;
        pose[0] = [45.0, 10.0, -5.0];
        pose[3] = [-90.0, 0.0, 0.0];
        let (head, _) = anim_delta(mobs::Anim::StandPose(0), 1.0, &ctx(pose));
        assert!(close(head, rad([45.0, 10.0, -5.0])), "{head:?}");
        let (arm, _) = anim_delta(mobs::Anim::StandPose(3), 1.0, &ctx(pose));
        assert!(close(arm, rad([-75.0, 0.0, -10.0])), "{arm:?}");
    }

    /// `WardenModel.animateTendrils`: one angle, mirrored between the two
    /// tendrils, scaled by the countdown and zero without it.
    #[test]
    fn warden_tendril_sway_matches_vanilla() {
        let ctx = |tendril, age| AnimCtx {
            pitch: 0.0,
            net: 0.0,
            f: 0.0,
            pos: 0.0,
            amt: 0.0,
            age,
            gesture: None,
            events: [None; mobs::ModelEvent::COUNT],
            shell: false,
            body_yaw: 0.0,
            stand_pose: mobs::STAND_DEFAULT_POSE,
            allay_dance: None,
            attack: mobs::SwingPose::NONE,
            arm_poses: mobs::ArmPoses::EMPTY,
            mob: mobs::MobCombat::default(),
            tendril,
        };
        // At rest the tendrils do not move, whatever the age.
        for age in [0.0, 3.0, 17.5] {
            let (r, _) = anim_delta(
                mobs::Anim::WardenTendril { left: true },
                1.0,
                &ctx(0.0, age),
            );
            assert_eq!(r, [0.0; 3]);
        }
        // Full countdown at age 0: cos(0) = 1 -> +/-pi*0.1, left positive.
        let peak = std::f32::consts::PI * 0.1;
        let (l, _) = anim_delta(
            mobs::Anim::WardenTendril { left: true },
            1.0,
            &ctx(1.0, 0.0),
        );
        let (r, _) = anim_delta(
            mobs::Anim::WardenTendril { left: false },
            1.0,
            &ctx(1.0, 0.0),
        );
        assert!((l[0] - peak).abs() < 1e-6 && (r[0] + peak).abs() < 1e-6);
        assert_eq!([l[1], l[2]], [0.0, 0.0], "tendrils rotate about X only");
        // Half a countdown scales the same angle.
        let (h, _) = anim_delta(
            mobs::Anim::WardenTendril { left: true },
            1.0,
            &ctx(0.5, 0.0),
        );
        assert!((h[0] - peak * 0.5).abs() < 1e-6);
        // cos(age*2.25) = 0 at age = (pi/2)/2.25 — the still point the emissive
        // gate renders at.
        let (z, _) = anim_delta(
            mobs::Anim::WardenTendril { left: true },
            1.0,
            &ctx(1.0, std::f32::consts::FRAC_PI_2 / 2.25),
        );
        assert!(
            z[0].abs() < 1e-6,
            "the sway must vanish at its zero crossing"
        );
    }

    /// The tendril split must not have moved the tendrils: their rest geometry
    /// has to match the plates they replaced, which were folded onto the head at
    /// the same offsets. (This is what keeps `mobshot --check` at 243/243.)
    #[test]
    fn warden_tendrils_keep_their_rest_position() {
        let m = (warden_def().build)();
        let quads: Vec<&mobs::RawQuad> = m
            .quads
            .iter()
            .filter(|q| m.parts[q.part].name.ends_with("_tendril"))
            .collect();
        assert_eq!(quads.len(), 4);
        // Vanilla: right_tendril box [-16,-13,0]+[16,16,0] at pose (-8,-12,0)
        // inside a head pivoted at (0,-13,0) under a body at (0,3,0) — but `pos`
        // here is still part-local, so the box corners are the raw `addBox` ones.
        let xs: Vec<f32> = quads
            .iter()
            .flat_map(|q| q.pos.iter().map(|p| p[0]))
            .collect();
        let (lo, hi) = (
            xs.iter().cloned().fold(f32::MAX, f32::min),
            xs.iter().cloned().fold(f32::MIN, f32::max),
        );
        assert_eq!((lo, hi), (-16.0, 16.0), "both plates span the head's sides");
        for q in &quads {
            for p in &q.pos {
                assert_eq!(p[2], 0.0, "tendril plates are flat at z=0");
                assert!((-13.0..=3.0).contains(&p[1]), "y {} out of the box", p[1]);
            }
        }
        // And the pivots are vanilla's PartPose offsets.
        let pivot = |name: &str| m.parts.iter().find(|p| p.name == name).unwrap().pivot;
        assert_eq!(pivot("right_tendril"), [-8.0, -12.0, 0.0]);
        assert_eq!(pivot("left_tendril"), [8.0, -12.0, 0.0]);
    }
    // ---- the dynamic-pool slot ring (real-texture mob gate) --------------

    /// The property the bare `next % cap` cursor could not state: a claim that
    /// recycles a slot **names the key it took it from**, so a key→slot cache
    /// can drop the stale entry instead of resolving to the new upload.
    #[test]
    fn slot_ring_names_the_key_it_evicts() {
        let mut r: SlotRing<u16> = SlotRing::new(3);
        // The first pass round fills empty slots and evicts nobody.
        assert_eq!(r.claim(10), (0, None));
        assert_eq!(r.claim(11), (1, None));
        assert_eq!(r.claim(12), (2, None));
        // The wrap is where the old pools went silent.
        assert_eq!(r.claim(13), (0, Some(10)));
        assert_eq!(r.claim(14), (1, Some(11)));
        // And it keeps naming the *current* owner, not the first one.
        assert_eq!(r.claim(15), (2, Some(12)));
        assert_eq!(r.claim(16), (0, Some(13)));
    }

    /// A one-slot ring is the degenerate case a cache is most likely to get
    /// wrong, because every claim after the first is an eviction.
    #[test]
    fn slot_ring_of_one_evicts_every_time() {
        let mut r: SlotRing<String> = SlotRing::new(1);
        assert_eq!(r.claim("a".into()), (0, None));
        assert_eq!(r.claim("b".into()), (0, Some("a".into())));
        assert_eq!(r.claim("c".into()), (0, Some("b".into())));
    }

    /// The cursor is `wrapping_add`, so a very long session rolls over `u32`
    /// rather than panicking in debug — and the slot it lands on must still be
    /// inside the pool.
    ///
    /// The roll is **seamless**, and the fact that makes it so is that `2^32`
    /// *is* a multiple of both real caps (64 and 1,024 are powers of two), so
    /// `u32::MAX % cap == cap - 1` and the next claim lands on slot 0 with
    /// nothing skipped. An earlier version of this comment said `u32::MAX` is
    /// *not* a multiple of the caps and drew the same conclusion from it, which
    /// is a non-sequitur — the test was right and the reasoning beside it was
    /// not. A cap that was not a power of two would genuinely skip here.
    #[test]
    fn the_swirl_offsets_follow_each_layer() {
        let [u, v] = swirl_offsets(SwirlSheet::Creeper, 150.0);
        assert!((u - 0.5).abs() < 1e-5 && (v - 0.5).abs() < 1e-5, "{u} {v}");
        // `Mth.cos(0) * 3 = 3`, `3 % 1 = 0`; at t = 50 `cos(1) * 3` ~ 1.6209.
        assert_eq!(swirl_offsets(SwirlSheet::Wither, 0.0), [0.0, 0.0]);
        let [u, _] = swirl_offsets(SwirlSheet::Wither, 50.0);
        assert!((u - (3.0 * 1f32.cos()) % 1.0).abs() < 1e-3, "{u}");
        // A negative cosine keeps its sign through `%`, as Java's does.
        let [u, _] = swirl_offsets(SwirlSheet::Wither, 100.0);
        assert!(u < 0.0, "{u}");
    }

    #[test]
    fn the_swirl_sorts_its_quads_farthest_first() {
        let quad = |z: f32| {
            let v = |x: f32, y: f32| Vertex {
                pos: [x, y, z],
                uv: [z, 0.0],
                color: [1.0; 4],
                light_hurt: [1.0, 1.0, 1.0, 0.0],
            };
            [
                v(0.0, 0.0),
                v(1.0, 0.0),
                v(1.0, 1.0),
                v(0.0, 0.0),
                v(1.0, 1.0),
                v(0.0, 1.0),
            ]
        };
        let mut verts: Vec<Vertex> = [1.0, 5.0, 3.0, 5.0].into_iter().flat_map(quad).collect();
        sort_quads_back_to_front(&mut verts, [0.5, 0.5, 0.0]);
        let order: Vec<f32> = verts.chunks(6).map(|q| q[0].uv[0]).collect();
        assert_eq!(order, vec![5.0, 5.0, 3.0, 1.0]);
        assert!(verts
            .chunks(6)
            .all(|q| q.iter().all(|v| v.pos[2] == q[0].pos[2])));
    }

    #[test]
    fn clamped_ranges_never_exceed_the_upload() {
        // Everything fits: untouched.
        assert_eq!(clamp_ranges([6, 3, 3, 0, 0, 3], 100), [6, 3, 3, 0, 0, 3]);
        // The cut lands in the glint range: it shrinks to whole triangles and
        // every later range is empty.
        let c = clamp_ranges([90, 6, 30, 12, 6, 6], 110);
        assert_eq!(c, [90, 6, 12, 0, 0, 0]);
        assert!(c.iter().sum::<u32>() <= 110);
        // Nothing uploaded, nothing drawn.
        assert_eq!(clamp_ranges([3, 3, 3, 3, 3, 3], 0), [0; 6]);
    }

    #[test]
    fn slot_ring_survives_a_cursor_rollover() {
        let mut r: SlotRing<u32> = SlotRing::new(64);
        r.next = u32::MAX - 1;
        let (a, _) = r.claim(1);
        let (b, _) = r.claim(2);
        let (c, _) = r.claim(3);
        assert_eq!(a, (u32::MAX - 1) % 64);
        assert_eq!(b, u32::MAX % 64);
        assert_eq!(c, 0);
        assert!([a, b, c].iter().all(|s| *s < 64));
    }
}

#[cfg(test)]
mod enderman_pose_tests {
    use super::*;

    /// M352 — `EndermanModel.setupAnim`: a carrying enderman holds both arms
    /// at `xRot` -0.5, `zRot` ±0.05 whatever its walk; otherwise its arms and
    /// legs swing at half the humanoid's and never past ±0.4.
    #[test]
    fn the_enderman_holds_its_block_and_clamps_its_swing() {
        use std::f32::consts::PI;
        let rig = |anim: mobs::Anim, carrying: bool, f: f32, amt: f32| {
            let c = AnimCtx {
                pitch: 0.0,
                net: 0.0,
                f,
                pos: 0.0,
                amt,
                age: 0.0,
                gesture: None,
                events: [None; mobs::ModelEvent::COUNT],
                shell: false,
                body_yaw: 0.0,
                stand_pose: mobs::STAND_DEFAULT_POSE,
                allay_dance: None,
                attack: mobs::SwingPose::NONE,
                arm_poses: mobs::ArmPoses::EMPTY,
                mob: mobs::MobCombat {
                    carried_block: carrying.then_some(1),
                    ..Default::default()
                },
                tendril: 0.0,
            };
            anim_delta(anim, 1.0, &c).0
        };
        for f in [0.0, 1.0, 2.5] {
            let r = rig(mobs::Anim::EndermanArmRight, true, f, 1.0);
            assert_eq!((r[0], r[2]), (-0.5, 0.05));
            let l = rig(mobs::Anim::EndermanArmLeft, true, f, 1.0);
            assert_eq!((l[0], l[2]), (-0.5, -0.05));
        }
        // A full-speed stride (amount 1.5): the raw half-swing reaches 0.75.
        let a = rig(mobs::Anim::EndermanArmRight, false, PI, 1.5);
        assert_eq!(a[0], 0.4);
        let leg = rig(mobs::Anim::EndermanLegLeft, false, PI, 1.5);
        assert_eq!(leg[0], 0.4);
        let small = rig(mobs::Anim::EndermanLegRight, false, 0.0, 0.2);
        assert!((small[0] - 1.4 * 0.2 * 0.5).abs() < 1e-6);
    }
}

#[cfg(test)]
mod bee_bob_tests {
    use super::*;

    /// M354 — `BeeModel.bobUpAndDown` at age 0 (`s` = 1) and a quarter
    /// period on (`s` = 0), and nothing while angry.
    #[test]
    fn a_calm_bee_bobs_and_an_angry_one_does_not() {
        use std::f32::consts::PI;
        let ctx = |age: f32, angry: bool| AnimCtx {
            pitch: 0.0,
            net: 0.0,
            f: 0.0,
            pos: 0.0,
            amt: 0.0,
            age,
            gesture: None,
            events: [None; mobs::ModelEvent::COUNT],
            shell: false,
            body_yaw: 0.0,
            stand_pose: mobs::STAND_DEFAULT_POSE,
            allay_dance: None,
            attack: mobs::SwingPose::NONE,
            arm_poses: mobs::ArmPoses::EMPTY,
            mob: mobs::MobCombat {
                bee_angry: angry,
                ..Default::default()
            },
            tendril: 0.0,
        };
        let near = |a: f32, b: f32| (a - b).abs() < 1e-5;
        let (bone, off) = anim_delta(mobs::Anim::BeeBone, 1.0, &ctx(0.0, false));
        assert!(near(bone[0], 0.1 + PI * 0.025) && near(off[1], -0.9));
        let (front, _) = anim_delta(mobs::Anim::BeeFrontLegs, 1.0, &ctx(0.0, false));
        assert!(near(front[0] + PI / 4.0, -PI * 0.1 + PI / 8.0));
        let (back, _) = anim_delta(mobs::Anim::BeeBackLegs, 1.0, &ctx(0.0, false));
        assert!(near(back[0] + PI / 4.0, -PI * 0.05 + PI / 4.0));
        // A quarter period on: `s` = cos(π/2) = 0.
        let quarter = PI / 2.0 / 0.18;
        let (bone, off) = anim_delta(mobs::Anim::BeeBone, 1.0, &ctx(quarter, false));
        assert!(near(bone[0], 0.1) && near(off[1], 0.0));
        for anim in [
            mobs::Anim::BeeBone,
            mobs::Anim::BeeFrontLegs,
            mobs::Anim::BeeBackLegs,
        ] {
            let (r, o) = anim_delta(anim, 1.0, &ctx(3.0, true));
            assert_eq!((r, o), ([0.0; 3], [0.0; 3]));
        }
    }
}

#[cfg(test)]
mod wolf_tail_tests {
    use super::*;

    /// M355 — the tail takes `tailAngle` over its π/5 rest, and an angry
    /// wolf's does not wag.
    #[test]
    fn the_wolfs_tail_takes_its_angle_and_an_angry_one_does_not_wag() {
        use std::f32::consts::PI;
        let ctx = |tail: Option<f32>, angry: bool| AnimCtx {
            pitch: 0.0,
            net: 0.0,
            f: 0.0,
            pos: 0.0,
            amt: 1.0,
            age: 0.0,
            gesture: None,
            events: [None; mobs::ModelEvent::COUNT],
            shell: false,
            body_yaw: 0.0,
            stand_pose: mobs::STAND_DEFAULT_POSE,
            allay_dance: None,
            attack: mobs::SwingPose::NONE,
            arm_poses: mobs::ArmPoses::EMPTY,
            mob: mobs::MobCombat {
                wolf_tail: tail,
                wolf_angry: angry,
                ..Default::default()
            },
            tendril: 0.0,
        };
        let (r, _) = anim_delta(mobs::Anim::TailWagY, 1.0, &ctx(None, false));
        assert_eq!(r, [0.0, 1.4, 0.0]);
        let (r, _) = anim_delta(mobs::Anim::TailWagY, 1.0, &ctx(Some(1.5393804), true));
        assert!((r[0] + PI / 5.0 - 1.5393804).abs() < 1e-6 && r[1] == 0.0);
    }
}

#[cfg(test)]
mod wolf_sit_tests {
    use super::*;

    /// M356 — `WolfModel.setSittingPose`: the body pitched to π/4 from its
    /// π/2 rest and moved (4, −2), the hind legs folded to 3π/2 and moved
    /// (6.7, −5), the front legs at 5.811947, the tail lowered 9; a puppy's
    /// offsets halved; standing, the legs walk as the quadruped's.
    #[test]
    fn a_sitting_wolf_folds_its_legs_and_drops_its_rump() {
        use std::f32::consts::PI;
        let ctx = |sitting: bool, age_scale: f32| AnimCtx {
            pitch: 0.0,
            net: 0.0,
            f: 0.0,
            pos: 0.0,
            amt: 1.0,
            age: 0.0,
            gesture: None,
            events: [None; mobs::ModelEvent::COUNT],
            shell: false,
            body_yaw: 0.0,
            stand_pose: mobs::STAND_DEFAULT_POSE,
            allay_dance: None,
            attack: mobs::SwingPose::NONE,
            arm_poses: mobs::ArmPoses::EMPTY,
            mob: mobs::MobCombat {
                sitting,
                age_scale,
                ..Default::default()
            },
            tendril: 0.0,
        };
        let near = |a: [f32; 3], b: [f32; 3]| a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-5);
        let (r, o) = anim_delta(mobs::Anim::WolfBody, 1.0, &ctx(true, 1.0));
        assert!(near(r, [-PI / 4.0, 0.0, 0.0]) && near(o, [0.0, 4.0, -2.0]));
        let (r, o) = anim_delta(mobs::Anim::WolfHind { left: true }, 1.0, &ctx(true, 0.5));
        assert!(near(r, [3.0 * PI / 2.0, 0.0, 0.0]) && near(o, [0.0, 3.35, -2.5]));
        let (r, o) = anim_delta(mobs::Anim::WolfFront { left: false }, 1.0, &ctx(true, 1.0));
        assert!(near(r, [5.811_947, 0.0, 0.0]) && near(o, [0.01, 1.0, 0.0]));
        let (_, o) = anim_delta(mobs::Anim::TailWagY, 1.0, &ctx(true, 1.0));
        assert!(near(o, [0.0, 9.0, -2.0]));
        let (r, o) = anim_delta(mobs::Anim::WolfMane, 1.0, &ctx(true, 1.0));
        assert!(near(r, [2.0 * PI / 5.0 - PI / 2.0, 0.0, 0.0]) && near(o, [0.0, 2.0, 0.0]));
        // Standing: the body and mane at rest, the legs on the walk.
        let (r, o) = anim_delta(mobs::Anim::WolfBody, 1.0, &ctx(false, 1.0));
        assert!(near(r, [0.0; 3]) && near(o, [0.0; 3]));
        let (r, _) = anim_delta(mobs::Anim::WolfHind { left: false }, 1.0, &ctx(false, 1.0));
        let (q, _) = anim_delta(mobs::Anim::QuadHindRight, 1.0, &ctx(false, 1.0));
        assert_eq!(r, q);
        let (r, _) = anim_delta(mobs::Anim::WolfFront { left: true }, 1.0, &ctx(false, 1.0));
        let (q, _) = anim_delta(mobs::Anim::QuadFrontLeft, 1.0, &ctx(false, 1.0));
        assert_eq!(r, q);
    }
}

#[cfg(test)]
mod feline_sit_tests {
    use super::*;
    use std::f32::consts::PI;

    fn ctx(sitting: bool, age_scale: f32, f: f32, pos: f32, amt: f32) -> AnimCtx {
        AnimCtx {
            pitch: 0.0,
            net: 0.0,
            f,
            pos,
            amt,
            age: 0.0,
            gesture: None,
            events: [None; mobs::ModelEvent::COUNT],
            shell: false,
            body_yaw: 0.0,
            stand_pose: mobs::STAND_DEFAULT_POSE,
            allay_dance: None,
            attack: mobs::SwingPose::NONE,
            arm_poses: mobs::ArmPoses::EMPTY,
            mob: mobs::MobCombat {
                sitting,
                age_scale,
                ..Default::default()
            },
            tendril: 0.0,
        }
    }

    fn near(a: [f32; 3], b: [f32; 3]) -> bool {
        a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-5)
    }

    /// M357 — the adult's sitting pose (every absolute angle less its rest)
    /// and the kitten's, from `AdultFelineModel` / `BabyFelineModel`.
    #[test]
    fn a_sitting_cat_and_kitten_take_vanillas_pose() {
        use mobs::Anim as A;
        let adult = ctx(true, 1.0, 0.0, 0.0, 0.0);
        let (r, o) = anim_delta(A::FelineBody, 1.0, &adult);
        assert!(near(r, [-PI / 4.0, 0.0, 0.0]) && near(o, [0.0, -4.0, 5.0]));
        let (r, o) = anim_delta(A::FelineTail1, 1.0, &adult);
        assert!(near(r, [1.727_876_1 - 0.9, 0.0, 0.0]) && near(o, [0.0, 8.0, -2.0]));
        let (r, o) = anim_delta(A::FelineTail2, 1.0, &adult);
        assert!(near(r, [2.670_354 - 1.727_876_1, 0.0, 0.0]) && near(o, [0.0, 2.0, -0.8]));
        let (r, o) = anim_delta(
            A::FelineLeg {
                hind: true,
                left: true,
            },
            1.0,
            &adult,
        );
        assert!(near(r, [-PI / 2.0, 0.0, 0.0]) && near(o, [0.0, 3.0, -4.0]));
        let (r, o) = anim_delta(
            A::FelineLeg {
                hind: false,
                left: false,
            },
            1.0,
            &adult,
        );
        assert!(near(r, [-PI / 20.0, 0.0, 0.0]) && near(o, [0.0, 2.0, -2.0]));
        let (_, o) = anim_delta(A::FelineHead, 1.0, &adult);
        assert!(near(o, [0.0, -3.3, 1.0]));
        let kitten = ctx(true, 0.5, 0.0, 0.0, 0.0);
        let (r, o) = anim_delta(A::FelineBody, 1.0, &kitten);
        assert!(near(r, [-0.436_332_32, 0.0, 0.0]) && near(o, [0.0, 1.25, 0.0]));
        let (r, o) = anim_delta(A::FelineTail1, 1.0, &kitten);
        assert!(near(r, [0.545_415_4, 0.0, 0.0]) && near(o, [0.0, 4.0, -0.9]));
        let (r, o) = anim_delta(
            A::FelineLeg {
                hind: true,
                left: false,
            },
            1.0,
            &kitten,
        );
        assert!(near(r, [0.0; 3]) && near(o, [0.0, 0.0, -0.9]));
        let (r, o) = anim_delta(
            A::FelineLeg {
                hind: false,
                left: true,
            },
            1.0,
            &kitten,
        );
        assert!(near(r, [0.0; 3]) && near(o, [0.0; 3]));
        let (_, o) = anim_delta(A::FelineHead, 1.0, &kitten);
        assert!(near(o, [0.0, 0.0, 0.75]));
    }

    /// M357 — standing, the legs walk at amplitude 1 (left hind and right
    /// front on cos(f), the others on cos(f + π)) and the second tail
    /// segment sways π/4 · cos(walkPos) · speed over its 1.7278761 rest.
    #[test]
    fn a_walking_cat_sways_its_tail_and_steps_at_amplitude_one() {
        use mobs::Anim as A;
        let walk = ctx(false, 1.0, 0.0, 0.0, 1.0);
        for (hind, left, want) in [
            (true, true, 1.0),
            (true, false, -1.0),
            (false, true, -1.0),
            (false, false, 1.0),
        ] {
            let (r, _) = anim_delta(A::FelineLeg { hind, left }, 1.0, &walk);
            assert!((r[0] - want).abs() < 1e-5, "{hind} {left}: {}", r[0]);
        }
        let (r, _) = anim_delta(A::FelineTail2, 1.0, &walk);
        assert!((r[0] - PI / 4.0).abs() < 1e-5);
        let (r, o) = anim_delta(A::FelineBody, 1.0, &walk);
        assert!(near(r, [0.0; 3]) && near(o, [0.0; 3]));
    }
}

#[cfg(test)]
mod sheep_head_tests {
    use super::*;

    /// M358 — a grazing sheep's head takes the eat angle and drops by the eat
    /// position · 9 · the age scale; a calm one looks.
    #[test]
    fn a_grazing_sheep_lowers_its_head() {
        let ctx = |eat: Option<(f32, f32)>, age_scale: f32| AnimCtx {
            pitch: 0.3,
            net: 0.2,
            f: 0.0,
            pos: 0.0,
            amt: 0.0,
            age: 0.0,
            gesture: None,
            events: [None; mobs::ModelEvent::COUNT],
            shell: false,
            body_yaw: 0.0,
            stand_pose: mobs::STAND_DEFAULT_POSE,
            allay_dance: None,
            attack: mobs::SwingPose::NONE,
            arm_poses: mobs::ArmPoses::EMPTY,
            mob: mobs::MobCombat {
                sheep_eat: eat,
                age_scale,
                ..Default::default()
            },
            tendril: 0.0,
        };
        let (r, o) = anim_delta(mobs::Anim::SheepHead, 1.0, &ctx(None, 1.0));
        assert_eq!((r, o), ([0.3, 0.2, 0.0], [0.0; 3]));
        let (r, o) = anim_delta(mobs::Anim::SheepHead, 1.0, &ctx(Some((1.0, 0.7)), 1.0));
        assert_eq!((r, o), ([0.7, 0.2, 0.0], [0.0, 9.0, 0.0]));
        let (_, o) = anim_delta(mobs::Anim::SheepHead, 1.0, &ctx(Some((0.5, 0.7)), 0.5));
        assert_eq!(o, [0.0, 2.25, 0.0]);
    }
}

#[cfg(test)]
mod equine_anim_tests {
    use super::*;
    use std::f32::consts::PI;

    fn ctx(pitch: f32, net: f32, f: f32, pos: f32, amt: f32) -> AnimCtx {
        AnimCtx {
            pitch,
            net,
            f,
            pos,
            amt,
            age: 0.0,
            gesture: None,
            events: [None; mobs::ModelEvent::COUNT],
            shell: false,
            body_yaw: 0.0,
            stand_pose: mobs::STAND_DEFAULT_POSE,
            allay_dance: None,
            attack: mobs::SwingPose::NONE,
            arm_poses: mobs::ArmPoses::EMPTY,
            mob: mobs::MobCombat::default(),
            tendril: 0.0,
        }
    }

    /// M359 — the head looks (the yaw clamped to ±20°) and nods while
    /// walking faster than 0.2; the legs swing at 0.8 (front) and 0.5
    /// (hind); the tail lifts with the speed.
    #[test]
    fn a_horse_looks_and_walks_as_vanillas() {
        use mobs::Anim as A;
        let (r, _) = anim_delta(A::EquineHead, 1.0, &ctx(0.3, 1.0, 0.0, 0.0, 0.0));
        assert!((r[0] - 0.3).abs() < 1e-6 && (r[1] - 20f32.to_radians()).abs() < 1e-6);
        let (r, _) = anim_delta(A::EquineHead, 1.0, &ctx(0.0, -0.1, 0.0, 0.0, 0.5));
        assert!((r[0] - 0.15 * 0.5).abs() < 1e-6 && (r[1] + 0.1).abs() < 1e-6);
        let (r, _) = anim_delta(A::EquineHead, 1.0, &ctx(0.0, 0.0, 0.0, 0.0, 0.2));
        assert_eq!(r[0], 0.0);
        // f = 0: legAnim1 = cos(π) = −1.
        let walk = ctx(0.0, 0.0, 0.0, 0.0, 1.0);
        for (hind, left, want) in [
            (false, true, -0.8),
            (false, false, 0.8),
            (true, true, 0.5),
            (true, false, -0.5),
        ] {
            let (r, _) = anim_delta(A::EquineLeg { hind, left }, 1.0, &walk);
            assert!((r[0] - want).abs() < 1e-6, "{hind} {left}: {}", r[0]);
        }
        let (r, o) = anim_delta(A::EquineTail, 1.0, &ctx(0.0, 0.0, 0.0, 0.0, 0.4));
        assert!((r[0] - 0.3).abs() < 1e-6 && o == [0.0, 0.4, 0.8]);
        let _ = PI;
    }
}

#[cfg(test)]
mod turtle_anim_tests {
    use super::*;
    use std::f32::consts::PI;

    fn ctx(pos: f32, amt: f32, mob: mobs::MobCombat) -> AnimCtx {
        AnimCtx {
            pitch: 0.0,
            net: 0.0,
            f: pos * 0.6662,
            pos,
            amt,
            age: 0.0,
            gesture: Option::None,
            events: [Option::None; mobs::ModelEvent::COUNT],
            shell: false,
            body_yaw: 0.0,
            stand_pose: mobs::STAND_DEFAULT_POSE,
            allay_dance: Option::None,
            attack: mobs::SwingPose::NONE,
            arm_poses: mobs::ArmPoses::EMPTY,
            mob,
            tendril: 0.0,
        }
    }

    fn leg(hind: bool, left: bool, c: &AnimCtx) -> [f32; 3] {
        anim_delta(mobs::Anim::TurtleLeg { hind, left }, 1.0, c).0
    }

    /// M365 — `TurtleModel.setupAnim`, term by term: on land the flippers
    /// sweep about y (front 8, hind 3, mirrored left to right; laying an egg
    /// quadruples the front's rate and doubles its reach) over the
    /// quadruped's x swing; in water the hind x swing and the front z swing
    /// replace the sweep; an egg lifts every root part 1 px.
    #[test]
    fn the_turtle_paddles_on_land_and_swims_in_water() {
        let land = mobs::MobCombat {
            turtle_on_land: true,
            ..Default::default()
        };
        let (pos, amt) = (0.3_f32, 0.8_f32);
        let c = ctx(pos, amt, land);
        let rf = leg(false, false, &c);
        assert!((rf[1] + (pos * 5.0).cos() * 8.0 * amt).abs() < 1e-5);
        assert!((leg(false, true, &c)[1] + rf[1]).abs() < 1e-6);
        let rh = leg(true, false, &c);
        assert!((rh[1] + (pos * 5.0).cos() * 3.0 * amt).abs() < 1e-5);
        assert!((rh[0] - (pos * 0.6662).cos() * 1.4 * amt).abs() < 1e-5);
        let lh = leg(true, true, &c);
        assert!((lh[0] - (pos * 0.6662 + PI).cos() * 1.4 * amt).abs() < 1e-5);
        let laying = ctx(
            pos,
            amt,
            mobs::MobCombat {
                turtle_on_land: true,
                turtle_laying: true,
                ..Default::default()
            },
        );
        let lf = leg(false, true, &laying);
        assert!((lf[1] - (4.0 * pos * 5.0).cos() * 8.0 * amt * 2.0).abs() < 1e-5);
        let water = ctx(pos, amt, mobs::MobCombat::default());
        let swing = (pos * 0.6662 * 0.6).cos() * 0.5 * amt;
        assert!((leg(true, false, &water)[0] - swing).abs() < 1e-6);
        assert!((leg(true, true, &water)[0] + swing).abs() < 1e-6);
        let wrf = leg(false, false, &water);
        assert!((wrf[2] + swing).abs() < 1e-6 && wrf[1] == 0.0);
        let egg = ctx(
            0.0,
            0.0,
            mobs::MobCombat {
                turtle_egg: true,
                ..Default::default()
            },
        );
        for a in [
            mobs::Anim::TurtleHead,
            mobs::Anim::TurtleBody,
            mobs::Anim::TurtleLeg {
                hind: true,
                left: false,
            },
        ] {
            assert_eq!(anim_delta(a, 1.0, &egg).1[1], -1.0);
            assert_eq!(anim_delta(a, 1.0, &water).1[1], 0.0);
        }
    }
}

#[cfg(test)]
mod strider_anim_tests {
    use super::*;
    use std::f32::consts::PI;

    fn ctx(pitch: f32, pos: f32, amt: f32, age: f32, ridden: bool) -> AnimCtx {
        AnimCtx {
            pitch,
            net: 0.4,
            f: pos * 0.6662,
            pos,
            amt,
            age,
            gesture: Option::None,
            events: [Option::None; mobs::ModelEvent::COUNT],
            shell: false,
            body_yaw: 0.0,
            stand_pose: mobs::STAND_DEFAULT_POSE,
            allay_dance: Option::None,
            attack: mobs::SwingPose::NONE,
            arm_poses: mobs::ArmPoses::EMPTY,
            mob: mobs::MobCombat {
                ridden,
                ..Default::default()
            },
            tendril: 0.0,
        }
    }

    /// M366 — `StriderModel`, term by term: the speed clamps at 0.25; the
    /// body looks unless ridden, rolls and bobs (twice as far on the adult);
    /// the legs swing, splay and bob in opposition; the bristles flow with
    /// the stride and sway on the age, on z (adult) or x (baby).
    #[test]
    fn the_strider_walks_as_vanilla_animates_it() {
        use mobs::Anim::*;
        let (pos, age) = (0.7_f32, 13.0_f32);
        let fast = ctx(0.2, pos, 1.0, age, false);
        let clamp = ctx(0.2, pos, 0.25, age, false);
        for a in [
            StriderBody { adult: true },
            StriderLeg { left: true },
            StriderBristle {
                order: 1,
                baby: false,
            },
        ] {
            assert_eq!(anim_delta(a, 1.0, &fast), anim_delta(a, 1.0, &clamp));
        }
        let (r, o) = anim_delta(StriderBody { adult: true }, 1.0, &clamp);
        assert_eq!((r[0], r[1]), (0.2, 0.4));
        assert!((r[2] - 0.1 * (pos * 1.5).sin() * 4.0 * 0.25).abs() < 1e-6);
        assert!((o[1] + 2.0 * (pos * 1.5).cos() * 2.0 * 0.25).abs() < 1e-6);
        let (_, ob) = anim_delta(StriderBody { adult: false }, 1.0, &clamp);
        assert!((ob[1] * 2.0 - o[1]).abs() < 1e-6);
        let (rr, _) = anim_delta(
            StriderBody { adult: true },
            1.0,
            &ctx(0.2, pos, 0.25, age, true),
        );
        assert_eq!((rr[0], rr[1]), (0.0, 0.0));
        let h = pos * 0.75;
        let (lr, lo) = anim_delta(StriderLeg { left: true }, 1.0, &clamp);
        assert!((lr[0] - h.sin() * 0.5).abs() < 1e-6);
        assert!((lr[2] - PI / 18.0 * h.cos() * 0.25).abs() < 1e-6);
        assert!((lo[1] - 4.0 * (h + PI).sin() * 0.25).abs() < 1e-5);
        let (rr, ro) = anim_delta(StriderLeg { left: false }, 1.0, &clamp);
        assert!((rr[0] + lr[0]).abs() < 1e-5 && (ro[1] + lo[1]).abs() < 1e-5);
        let still = ctx(0.0, pos, 0.0, age, false);
        let (b0, _) = anim_delta(
            StriderBristle {
                order: 0,
                baby: false,
            },
            1.0,
            &still,
        );
        assert!((b0[2] - 0.1 * (age * 0.4).sin()).abs() < 1e-6 && b0[0] == 0.0);
        let (b2, _) = anim_delta(
            StriderBristle {
                order: 2,
                baby: true,
            },
            1.0,
            &still,
        );
        assert!((b2[0] - 0.05 * (age * -0.4).sin()).abs() < 1e-6 && b2[2] == 0.0);
        let flow = (pos * 1.5 + PI).cos() * 0.25;
        let (b1, _) = anim_delta(
            StriderBristle {
                order: 1,
                baby: false,
            },
            1.0,
            &clamp,
        );
        assert!((b1[2] - (flow * 1.2 + 0.1 * (age * 0.2).sin())).abs() < 1e-6);
    }
}

#[cfg(test)]
mod hoglin_anim_tests {
    use super::*;
    use std::f32::consts::PI;

    fn ctx(pitch: f32, pos: f32, amt: f32, headbutt: f32) -> AnimCtx {
        AnimCtx {
            pitch,
            net: 0.3,
            f: pos * 0.6662,
            pos,
            amt,
            age: 0.0,
            gesture: Option::None,
            events: [Option::None; mobs::ModelEvent::COUNT],
            shell: false,
            body_yaw: 0.0,
            stand_pose: mobs::STAND_DEFAULT_POSE,
            allay_dance: Option::None,
            attack: mobs::SwingPose::NONE,
            arm_poses: mobs::ArmPoses::EMPTY,
            mob: mobs::MobCombat {
                headbutt,
                ..Default::default()
            },
            tendril: 0.0,
        }
    }

    /// M367 — `HoglinModel.setupAnim`, term by term: the head turns but
    /// never pitches with the look, and the headbutt swings it to −π/9 (and
    /// drops the baby's 2.5 px); the ears flap with the walk; the legs swing
    /// at 1.2 on the raw walk position, in diagonal pairs.
    #[test]
    fn the_hoglin_moves_as_vanilla_moves_it() {
        use mobs::Anim::*;
        let rest = ctx(0.6, 0.4, 0.0, 0.0);
        let (r, o) = anim_delta(HoglinHead { baby: true }, 1.0, &rest);
        assert_eq!((r[0], r[1], o[1]), (0.0, 0.3, 0.0));
        let butt = ctx(0.6, 0.4, 0.0, 1.0);
        let (r, o) = anim_delta(HoglinHead { baby: true }, 1.0, &butt);
        assert!((0.872_664_6 + r[0] + PI / 9.0).abs() < 1e-6);
        assert_eq!(o[1], 2.5);
        assert_eq!(anim_delta(HoglinHead { baby: false }, 1.0, &butt).1[1], 0.0);
        let (pos, amt) = (0.4_f32, 0.7_f32);
        let walk = ctx(0.0, pos, amt, 0.0);
        let left = anim_delta(HoglinEar { left: true }, 1.0, &walk).0[2];
        assert!((left - amt * pos.sin()).abs() < 1e-6);
        assert_eq!(
            anim_delta(HoglinEar { left: false }, 1.0, &walk).0[2],
            -left
        );
        let leg = |front, left| anim_delta(HoglinLeg { front, left }, 1.0, &walk).0[0];
        assert!((leg(true, false) - pos.cos() * 1.2 * amt).abs() < 1e-6);
        assert!((leg(true, true) - (pos + PI).cos() * 1.2 * amt).abs() < 1e-6);
        assert_eq!(leg(false, false), leg(true, true));
        assert_eq!(leg(false, true), leg(true, false));
    }
}

#[cfg(test)]
mod snow_golem_anim_tests {
    use super::*;

    fn ctx(net: f32) -> AnimCtx {
        AnimCtx {
            pitch: 0.0,
            net,
            f: 0.0,
            pos: 0.0,
            amt: 0.0,
            age: 0.0,
            gesture: Option::None,
            events: [Option::None; mobs::ModelEvent::COUNT],
            shell: false,
            body_yaw: 0.0,
            stand_pose: mobs::STAND_DEFAULT_POSE,
            allay_dance: Option::None,
            attack: mobs::SwingPose::NONE,
            arm_poses: mobs::ArmPoses::EMPTY,
            mob: mobs::MobCombat::default(),
            tendril: 0.0,
        }
    }

    /// M368 — `SnowGolemModel.setupAnim`: the upper body turns a quarter of
    /// the look; each arm turns with it and its pivot rides the 5 px circle
    /// (left at `(cos·5, −sin·5)`, right opposite), resting where it sits.
    #[test]
    fn the_snow_golems_arms_ride_its_turning_body() {
        use mobs::Anim::*;
        let c = ctx(0.8);
        let u = 0.2_f32;
        assert!((anim_delta(SnowGolemBody, 1.0, &c).0[1] - u).abs() < 1e-6);
        let (lr, lo) = anim_delta(SnowGolemArm { left: true }, 1.0, &c);
        assert!((lr[1] - u).abs() < 1e-6);
        assert!((5.0 + lo[0] - u.cos() * 5.0).abs() < 1e-5 && (lo[2] + u.sin() * 5.0).abs() < 1e-5);
        let (_, ro) = anim_delta(SnowGolemArm { left: false }, 1.0, &c);
        assert!(
            (-5.0 + ro[0] + u.cos() * 5.0).abs() < 1e-5 && (ro[2] - u.sin() * 5.0).abs() < 1e-5
        );
        let (r0, o0) = anim_delta(SnowGolemArm { left: true }, 1.0, &ctx(0.0));
        assert_eq!((r0[1], o0), (0.0, [0.0; 3]));
    }
}

#[cfg(test)]
mod allay_pose_anim_tests {
    use super::*;
    use std::f32::consts::PI;

    fn ctx(amt: f32, age: f32, hold: f32) -> AnimCtx {
        AnimCtx {
            pitch: 0.0,
            net: 0.0,
            f: 0.0,
            pos: 0.0,
            amt,
            age,
            gesture: Option::None,
            events: [Option::None; mobs::ModelEvent::COUNT],
            shell: false,
            body_yaw: 0.0,
            stand_pose: mobs::STAND_DEFAULT_POSE,
            allay_dance: Option::None,
            attack: mobs::SwingPose::NONE,
            arm_poses: mobs::ArmPoses::EMPTY,
            mob: mobs::MobCombat {
                allay_holding: hold,
                ..Default::default()
            },
            tendril: 0.0,
        }
    }

    /// M369 — `AllayModel.setupAnim`, term by term: hovering, the root bobs
    /// and the arms splay ±0.436 with the idle bob; flying (speed ≥ 0.3) the
    /// body pitches π/4, the bob stops; holding an item the arms reach
    /// forward (−π/3 hovering, −1.134 flying) and turn in, without the bob.
    #[test]
    fn the_allay_hovers_flies_and_holds() {
        use mobs::Anim::*;
        const DEG: f32 = PI / 180.0;
        let age = 7.0_f32;
        let hover = ctx(0.0, age, 0.0);
        assert!(
            (anim_delta(AllayRoot, 1.0, &hover).1[1] - (age * 9.0 * DEG).cos() * 0.25).abs() < 1e-6
        );
        let z = 0.43633232 - (age * 9.0 * DEG + PI * 1.5).cos() * PI * 0.075;
        let (l, _) = anim_delta(AllayArm { left: true }, 1.0, &hover);
        let (r, _) = anim_delta(AllayArm { left: false }, 1.0, &hover);
        assert!((l[2] + z).abs() < 1e-6 && (r[2] - z).abs() < 1e-6 && l[0] == 0.0);
        let fly = ctx(0.6, age, 0.0);
        assert!((anim_delta(AllayBody, 1.0, &fly).0[0] - PI / 4.0).abs() < 1e-6);
        assert_eq!(anim_delta(AllayRoot, 1.0, &fly).1[1], 0.0);
        assert!((anim_delta(AllayArm { left: false }, 1.0, &fly).0[2] - 0.43633232).abs() < 1e-6);
        let (h, _) = anim_delta(AllayArm { left: true }, 1.0, &ctx(0.0, age, 1.0));
        assert!((h[0] + PI / 3.0).abs() < 1e-6 && (h[1] + 0.27925268).abs() < 1e-6);
        assert!((h[2] + 0.43633232).abs() < 1e-6);
        let (hf, _) = anim_delta(AllayArm { left: false }, 1.0, &ctx(0.6, age, 1.0));
        assert!((hf[0] + 1.134464).abs() < 1e-5);
    }
}

#[cfg(test)]
mod guardian_anim_tests {
    use super::*;
    use std::f32::consts::PI;

    fn ctx(age: f32) -> AnimCtx {
        AnimCtx {
            pitch: 0.0,
            net: 0.0,
            f: 0.0,
            pos: 0.0,
            amt: 0.0,
            age,
            gesture: Option::None,
            events: [Option::None; mobs::ModelEvent::COUNT],
            shell: false,
            body_yaw: 0.0,
            stand_pose: mobs::STAND_DEFAULT_POSE,
            allay_dance: Option::None,
            attack: mobs::SwingPose::NONE,
            arm_poses: mobs::ArmPoses::EMPTY,
            mob: mobs::MobCombat::default(),
            tendril: 0.0,
        }
    }

    /// M370 — `GuardianModel.setupAnim`: each spike rides out along its
    /// direction by `cos(age·1.5 + i)·0.01` (none at age 0, where the rest
    /// pose sits); the tail segments sway 0.05π / 0.1π / 0.15π on
    /// sin(tail phase), the phase 0.125 a tick.
    #[test]
    fn the_guardian_spikes_wobble_and_its_tail_sways() {
        use mobs::Anim::*;
        assert_eq!(
            anim_delta(GuardianSpike { i: 3 }, 1.0, &ctx(0.0)).1,
            [0.0; 3]
        );
        let age = 9.0_f32;
        let (_, o) = anim_delta(GuardianSpike { i: 5 }, 1.0, &ctx(age));
        let d = ((age * 1.5 + 5.0).cos() - 5.0_f32.cos()) * 0.01;
        assert!((o[0] - 8.0 * d).abs() < 1e-6 && o[1] == 0.0 && (o[2] + 8.0 * d).abs() < 1e-6);
        for (seg, k) in [(0u8, 0.05_f32), (1, 0.1), (2, 0.15)] {
            let (r, _) = anim_delta(GuardianTail { seg }, 1.0, &ctx(age));
            assert!((r[1] - (age * 0.125).sin() * PI * k).abs() < 1e-6);
        }
    }
}

#[cfg(test)]
mod dolphin_anim_tests {
    use super::*;

    fn ctx(moving: bool, age: f32) -> AnimCtx {
        AnimCtx {
            pitch: 0.2,
            net: 0.3,
            f: 0.0,
            pos: 0.0,
            amt: 0.0,
            age,
            gesture: Option::None,
            events: [Option::None; mobs::ModelEvent::COUNT],
            shell: false,
            body_yaw: 0.0,
            stand_pose: mobs::STAND_DEFAULT_POSE,
            allay_dance: Option::None,
            attack: mobs::SwingPose::NONE,
            arm_poses: mobs::ArmPoses::EMPTY,
            mob: mobs::MobCombat {
                dolphin_moving: moving,
                ..Default::default()
            },
            tendril: 0.0,
        }
    }

    /// M371 — `DolphinModel.setupAnim`: the body looks, and swimming bobs;
    /// the tail's stroke replaces its rest while moving (so the adult tail
    /// at the stroke's zero is level, not 6° low) and leaves it alone still.
    #[test]
    fn the_dolphin_looks_bobs_and_strokes() {
        use mobs::Anim::*;
        let age = 4.0_f32;
        assert_eq!(
            anim_delta(DolphinBody, 1.0, &ctx(false, age)).0,
            [0.2, 0.3, 0.0]
        );
        let (b, _) = anim_delta(DolphinBody, 1.0, &ctx(true, age));
        assert!((b[0] - (0.2 - 0.05 - 0.05 * (age * 0.3).cos())).abs() < 1e-6);
        let rest = -0.10471976_f32;
        let tail = DolphinTail { k: 0.1, rest };
        assert_eq!(anim_delta(tail, 1.0, &ctx(false, age)).0, [0.0; 3]);
        let (t, _) = anim_delta(tail, 1.0, &ctx(true, age));
        assert!((rest + t[0] - (-0.1 * (age * 0.3).cos())).abs() < 1e-6);
    }
}

#[cfg(test)]
mod nautilus_body_anim_tests {
    use super::*;

    /// M372 — the soft body follows a small look exactly and stops at ±10°.
    #[test]
    fn the_nautilus_body_follows_the_look_within_ten_degrees() {
        let ctx = |pitch: f32, net: f32| AnimCtx {
            pitch,
            net,
            f: 0.0,
            pos: 0.0,
            amt: 0.0,
            age: 0.0,
            gesture: Option::None,
            events: [Option::None; mobs::ModelEvent::COUNT],
            shell: false,
            body_yaw: 0.0,
            stand_pose: mobs::STAND_DEFAULT_POSE,
            allay_dance: Option::None,
            attack: mobs::SwingPose::NONE,
            arm_poses: mobs::ArmPoses::EMPTY,
            mob: mobs::MobCombat::default(),
            tendril: 0.0,
        };
        let small = anim_delta(mobs::Anim::NautilusBody, 1.0, &ctx(0.05, -0.1)).0;
        assert_eq!((small[0], small[1]), (0.05, -0.1));
        let big = anim_delta(mobs::Anim::NautilusBody, 1.0, &ctx(1.0, -2.0)).0;
        assert!((big[0] - 10.0 * DEG).abs() < 1e-6 && (big[1] + 10.0 * DEG).abs() < 1e-6);
    }
}

#[cfg(test)]
mod warden_anim_tests {
    use super::*;
    use std::f32::consts::PI;

    fn ctx(pos: f32, amt: f32, age: f32) -> AnimCtx {
        AnimCtx {
            pitch: 0.1,
            net: 0.2,
            f: pos * 0.6662,
            pos,
            amt,
            age,
            gesture: Option::None,
            events: [Option::None; mobs::ModelEvent::COUNT],
            shell: false,
            body_yaw: 0.0,
            stand_pose: mobs::STAND_DEFAULT_POSE,
            allay_dance: Option::None,
            attack: mobs::SwingPose::NONE,
            arm_poses: mobs::ArmPoses::EMPTY,
            mob: mobs::MobCombat::default(),
            tendril: 0.0,
        }
    }

    /// M374 — `WardenModel`, term by term: the speed term caps at 0.5
    /// (any speed above 1/6 draws alike); standing, the head and body sway
    /// on `age·0.1`; walking, the right arm swings on sin, the left on cos.
    #[test]
    fn the_warden_sways_and_walks_as_vanilla_animates_it() {
        use mobs::Anim::*;
        let (pos, age) = (1.3_f32, 17.0_f32);
        for a in [
            WardenBody,
            WardenHead,
            WardenArm { left: false },
            WardenLeg { left: true },
        ] {
            assert_eq!(
                anim_delta(a, 1.0, &ctx(pos, 0.2, age)),
                anim_delta(a, 1.0, &ctx(pos, 1.0, age))
            );
        }
        let still = ctx(pos, 0.0, age);
        let (ws, wc) = (age * 0.1).sin_cos();
        let (h, _) = anim_delta(WardenHead, 1.0, &still);
        assert!(
            (h[0] - (0.1 + 0.06 * ws)).abs() < 1e-6
                && h[1] == 0.2
                && (h[2] - 0.06 * wc).abs() < 1e-6
        );
        let (b, _) = anim_delta(WardenBody, 1.0, &still);
        assert!((b[2] - 0.025 * ws).abs() < 1e-6 && (b[0] - 0.025 * wc).abs() < 1e-6);
        let walk = ctx(pos, 1.0, 0.0);
        let ap = pos * 0.8662;
        let r = anim_delta(WardenArm { left: false }, 1.0, &walk).0[0];
        let l = anim_delta(WardenArm { left: true }, 1.0, &walk).0[0];
        assert!((r + 0.8 * ap.sin() * 0.5).abs() < 1e-6 && (l + 0.8 * ap.cos() * 0.5).abs() < 1e-6);
        let rl = anim_delta(WardenLeg { left: false }, 1.0, &walk).0[0];
        assert!((rl - (ap + PI).cos() * 0.5).abs() < 1e-6);
    }
}

#[cfg(test)]
mod wither_breath_anim_tests {
    use super::*;
    use std::f32::consts::PI;

    /// M376 — the ribcage swings ±0.05π about its rest, the tail ±0.1π, and
    /// the tail's pivot rides the ribcage's 10 px arc (none at the mean).
    #[test]
    fn the_wither_ribcage_and_tail_breathe() {
        let ctx = |age: f32| AnimCtx {
            pitch: 0.0,
            net: 0.0,
            f: 0.0,
            pos: 0.0,
            amt: 0.0,
            age,
            gesture: Option::None,
            events: [Option::None; mobs::ModelEvent::COUNT],
            shell: false,
            body_yaw: 0.0,
            stand_pose: mobs::STAND_DEFAULT_POSE,
            allay_dance: Option::None,
            attack: mobs::SwingPose::NONE,
            arm_poses: mobs::ArmPoses::EMPTY,
            mob: mobs::MobCombat::default(),
            tendril: 0.0,
        };
        let full = ctx(0.0);
        assert!((anim_delta(mobs::Anim::WitherRibcage, 1.0, &full).0[0] - 0.05 * PI).abs() < 1e-6);
        let (t, o) = anim_delta(mobs::Anim::WitherTail, 1.0, &full);
        assert!((t[0] - 0.1 * PI).abs() < 1e-6);
        let rib = 0.115 * PI;
        let mean = mobs::WITHER_RIBCAGE_MEAN;
        assert!((o[1] - (rib.cos() - mean.cos()) * 10.0).abs() < 1e-4);
        assert!((o[2] - (rib.sin() - mean.sin()) * 10.0).abs() < 1e-4);
        // A quarter cycle on (age·0.1 = π/2) the breath crosses its mean.
        let (t2, o2) = anim_delta(mobs::Anim::WitherTail, 1.0, &ctx(5.0 * PI));
        assert!(t2[0].abs() < 1e-5 && o2[1].abs() < 1e-4 && o2[2].abs() < 1e-4);
    }
}

#[cfg(test)]
mod small_pose_anim_tests {
    use super::*;
    use std::f32::consts::PI;

    fn ctx(pitch: f32, net: f32, mob: mobs::MobCombat) -> AnimCtx {
        AnimCtx {
            pitch,
            net,
            f: 0.0,
            pos: 0.0,
            amt: 0.0,
            age: 9.0,
            gesture: Option::None,
            events: [Option::None; mobs::ModelEvent::COUNT],
            shell: false,
            body_yaw: 0.0,
            stand_pose: mobs::STAND_DEFAULT_POSE,
            allay_dance: Option::None,
            attack: mobs::SwingPose::NONE,
            arm_poses: mobs::ArmPoses::EMPTY,
            mob,
            tendril: 0.0,
        }
    }

    /// M377 — the camel's look clamps at ±30° / −25..45° and lifts after a
    /// dash (to 70° at most); the armadillo's at ±32.5° / −22.5..25°; a calm
    /// bee's antennae bob, an angry one's do not.
    #[test]
    fn the_clamped_heads_and_antennae_follow_vanilla() {
        use mobs::Anim::*;
        let d = mobs::MobCombat::default();
        let (r, _) = anim_delta(CamelHead, 1.0, &ctx(1.5, -1.5, d));
        assert!((r[0] - 45.0 * DEG).abs() < 1e-6 && (r[1] + 30.0 * DEG).abs() < 1e-6);
        let dash = mobs::MobCombat {
            camel_jump_cooldown: 55.0,
            ..Default::default()
        };
        let (r, _) = anim_delta(CamelHead, 1.0, &ctx(1.5, 0.0, dash));
        assert!((r[0] - 70.0 * DEG).abs() < 1e-6);
        let (r, _) = anim_delta(CamelHead, 1.0, &ctx(0.0, 0.0, dash));
        assert!((r[0] - 45.0 * DEG).abs() < 1e-6);
        let (r, _) = anim_delta(ArmadilloHead, 1.0, &ctx(-1.0, 1.0, d));
        assert!((r[0] + 22.5 * DEG).abs() < 1e-6 && (r[1] - 32.5 * DEG).abs() < 1e-6);
        let (a, _) = anim_delta(BeeAntenna, 1.0, &ctx(0.0, 0.0, d));
        assert!((a[0] - (9.0_f32 * 0.18).cos() * PI * 0.03).abs() < 1e-6);
        let angry = mobs::MobCombat {
            bee_angry: true,
            ..Default::default()
        };
        assert_eq!(
            anim_delta(BeeAntenna, 1.0, &ctx(0.0, 0.0, angry)).0,
            [0.0; 3]
        );
    }
}

#[cfg(test)]
mod parrot_anim_tests {
    use super::*;
    use std::f32::consts::PI;

    fn ctx(sitting: bool) -> AnimCtx {
        AnimCtx {
            pitch: 0.1,
            net: 0.2,
            f: 0.7,
            pos: 0.7 / 0.6662,
            amt: 0.5,
            age: 0.0,
            gesture: Option::None,
            events: [Option::None; mobs::ModelEvent::COUNT],
            shell: false,
            body_yaw: 0.0,
            stand_pose: mobs::STAND_DEFAULT_POSE,
            allay_dance: Option::None,
            attack: mobs::SwingPose::NONE,
            arm_poses: mobs::ArmPoses::EMPTY,
            mob: mobs::MobCombat {
                sitting,
                ..Default::default()
            },
            tendril: 0.0,
        }
    }

    /// M378 — standing, the legs swing 1.4 in opposition and the tail 0.3;
    /// sitting, everything drops 1.9 px, the tail tips π/6 and the legs π/2.
    #[test]
    fn the_parrot_walks_and_sits() {
        use mobs::Anim::*;
        let walk = ctx(false);
        let l = anim_delta(ParrotLeg { left: true }, 1.0, &walk).0[0];
        let r = anim_delta(ParrotLeg { left: false }, 1.0, &walk).0[0];
        assert!((l - 0.7_f32.cos() * 0.7).abs() < 1e-6 && (l + r).abs() < 1e-6);
        assert!((anim_delta(ParrotTail, 1.0, &walk).0[0] - 0.7_f32.cos() * 0.15).abs() < 1e-6);
        assert_eq!(anim_delta(ParrotSit, 1.0, &walk).1, [0.0; 3]);
        let sit = ctx(true);
        for a in [ParrotSit, ParrotHead, ParrotTail, ParrotLeg { left: true }] {
            assert_eq!(anim_delta(a, 1.0, &sit).1[1], 1.9);
        }
        assert_eq!(anim_delta(ParrotTail, 1.0, &sit).0[0], PI / 6.0);
        assert_eq!(
            anim_delta(ParrotLeg { left: false }, 1.0, &sit).0[0],
            PI / 2.0
        );
    }
}

#[cfg(test)]
mod witch_enderman_anim_tests {
    use super::*;

    fn ctx(age: f32, mob: mobs::MobCombat) -> AnimCtx {
        AnimCtx {
            pitch: 0.0,
            net: 0.0,
            f: 0.0,
            pos: 0.0,
            amt: 0.0,
            age,
            gesture: Option::None,
            events: [Option::None; mobs::ModelEvent::COUNT],
            shell: false,
            body_yaw: 0.0,
            stand_pose: mobs::STAND_DEFAULT_POSE,
            allay_dance: Option::None,
            attack: mobs::SwingPose::NONE,
            arm_poses: mobs::ArmPoses::EMPTY,
            mob,
            tendril: 0.0,
        }
    }

    /// M379 — the witch's nose wobbles at its id's rate (still at rate 0)
    /// and lifts while drinking; the enderman's arms bob with the humanoid
    /// idle (z whole, x halved), not while carrying a block.
    #[test]
    fn the_witch_nose_and_enderman_arms_move() {
        use mobs::Anim::*;
        let age = 40.0_f32;
        let rated = mobs::MobCombat {
            witch_nose_rate: 0.07,
            ..Default::default()
        };
        let (n, _) = anim_delta(WitchNose, 1.0, &ctx(age, rated));
        assert!((n[0] - (age * 0.07).sin() * 4.5 * DEG).abs() < 1e-6);
        assert!((n[2] - (age * 0.07).cos() * 2.5 * DEG).abs() < 1e-6);
        let still = anim_delta(WitchNose, 1.0, &ctx(age, mobs::MobCombat::default())).0;
        assert_eq!((still[0], still[2]), (0.0, 2.5 * DEG));
        let drinking = mobs::MobCombat {
            witch_holding: true,
            ..rated
        };
        let (d, o) = anim_delta(WitchNose, 1.0, &ctx(age, drinking));
        assert_eq!((d[0], o), (-0.9, [0.0, 3.0, -1.5]));
        let (r, _) = anim_delta(EndermanArmRight, 1.0, &ctx(age, mobs::MobCombat::default()));
        let (l, _) = anim_delta(EndermanArmLeft, 1.0, &ctx(age, mobs::MobCombat::default()));
        let z = (age * 0.09).cos() * 0.05 + 0.05;
        assert!((r[2] - z).abs() < 1e-6 && (l[2] + z).abs() < 1e-6);
        assert!((r[0] - (age * 0.067).sin() * 0.025).abs() < 1e-6 && (l[0] + r[0]).abs() < 1e-6);
        let carrying = mobs::MobCombat {
            carried_block: Some(1),
            ..Default::default()
        };
        assert_eq!(
            anim_delta(EndermanArmRight, 1.0, &ctx(age, carrying)).0,
            [-0.5, 0.0, 0.05]
        );
    }
}

#[cfg(test)]
mod piglin_ear_anim_tests {
    use super::*;

    /// M380 — the ears flap about their rest: the left on cos(f·1.2), the
    /// right on cos(f), wider with the walk speed.
    #[test]
    fn the_piglin_ears_flap() {
        let ctx = AnimCtx {
            pitch: 0.0,
            net: 0.0,
            f: 0.0,
            pos: 2.0,
            amt: 0.5,
            age: 13.0,
            gesture: Option::None,
            events: [Option::None; mobs::ModelEvent::COUNT],
            shell: false,
            body_yaw: 0.0,
            stand_pose: mobs::STAND_DEFAULT_POSE,
            allay_dance: Option::None,
            attack: mobs::SwingPose::NONE,
            arm_poses: mobs::ArmPoses::EMPTY,
            mob: mobs::MobCombat::default(),
            tendril: 0.0,
        };
        let f = 13.0_f32 * 0.1 + 2.0 * 0.5;
        let a = 0.08 + 0.5 * 0.4;
        let l = anim_delta(mobs::Anim::PiglinEar { left: true }, 1.0, &ctx).0[2];
        let r = anim_delta(mobs::Anim::PiglinEar { left: false }, 1.0, &ctx).0[2];
        assert!((l + (f * 1.2).cos() * a).abs() < 1e-6 && (r - f.cos() * a).abs() < 1e-6);
    }
}

#[cfg(test)]
mod boat_anim_tests {
    use super::*;

    fn ctx(rowing: [f32; 2], wobble: f32) -> AnimCtx {
        AnimCtx {
            pitch: 0.0,
            net: 0.0,
            f: 0.0,
            pos: 0.0,
            amt: 0.0,
            age: 0.0,
            gesture: Option::None,
            events: [Option::None; mobs::ModelEvent::COUNT],
            shell: false,
            body_yaw: 0.0,
            stand_pose: mobs::STAND_DEFAULT_POSE,
            allay_dance: Option::None,
            attack: mobs::SwingPose::NONE,
            arm_poses: mobs::ArmPoses::EMPTY,
            mob: mobs::MobCombat {
                boat_rowing: rowing,
                boat_wobble: wobble,
                ..Default::default()
            },
            tendril: 0.0,
        }
    }

    /// M395 — the magma cube's segments spread upward
    /// by `(4 − i)·squish·1.7` (none for a landing's negative squish).
    #[test]
    fn the_magma_cube_spreads_its_segments_by_the_squish() {
        let mut c = ctx([0.0; 2], 0.0);
        c.mob.cube_squish = 0.5;
        let lift = |i: u8, c: &AnimCtx| anim_delta(mobs::Anim::MagmaSegment { i }, 1.0, c).1[1];
        assert!((lift(0, &c) + 4.0 * 0.5 * 1.7).abs() < 1e-6);
        assert_eq!(lift(4, &c), 0.0);
        assert!((lift(7, &c) - 3.0 * 0.5 * 1.7).abs() < 1e-6);
        c.mob.cube_squish = -0.3;
        assert_eq!(lift(0, &c), 0.0);
    }

    /// M394 — the bubble tilt's rotation and scale are JOML's matrix for
    /// the un-normalised quaternion `(s, 0, s, c)`, conjugated by the flip:
    /// `S·(k·R) = M_joml·S`.
    #[test]
    fn a_boat_tilts_as_jomls_unnormalised_quaternion_turns_it() {
        let deg = 9.0_f32;
        let (e, k) = bubble_tilt(deg);
        let model = mat_mul(mat_zyx(e), scale_mat([k; 3]));
        // JOML `Matrix3f.rotation(q)` for q = (x, y, z, w), not normalised.
        let half = deg * DEG * 0.5;
        let (x, y, z, w) = (half.sin(), 0.0, half.sin(), half.cos());
        let joml = [
            [
                w * w + x * x - y * y - z * z,
                2.0 * (x * y - z * w),
                2.0 * (x * z + y * w),
            ],
            [
                2.0 * (x * y + z * w),
                w * w - x * x + y * y - z * z,
                2.0 * (y * z - x * w),
            ],
            [
                2.0 * (x * z - y * w),
                2.0 * (y * z + x * w),
                w * w - x * x - y * y + z * z,
            ],
        ];
        let s = [[-1.0, 0.0, 0.0], [0.0, -1.0, 0.0], [0.0, 0.0, 1.0]];
        let a = mat_mul(s, model);
        let b = mat_mul(joml, s);
        for i in 0..3 {
            for j in 0..3 {
                assert!((a[i][j] - b[i][j]).abs() < 1e-5, "{a:?} vs {b:?}");
            }
        }
        assert!(k > 1.0, "the un-normalised axis also scales");
        assert_eq!(
            anim_delta(mobs::Anim::BoatBubble, 1.0, &ctx([0.0; 2], 0.0)).0,
            [0.0; 3]
        );
    }

    /// M401 — the sleeping chain lays the model down face up along the
    /// bed: a standing head top (0, 32 px) points the way the bed faces
    /// (east, rotation 180: +x), and the face (−z before the body turn)
    /// points up.
    #[test]
    fn a_sleeper_lies_face_up_along_its_bed() {
        let east = sleep_direction_rotation("east");
        assert_eq!(east, 180.0);
        let head = sleeping_point([0.0, 32.0, 0.0], east);
        assert!(
            (head[0] - 32.0).abs() < 1e-3 && head[1].abs() < 1e-3 && head[2].abs() < 1e-3,
            "{head:?}"
        );
        let face = sleeping_point([0.0, 0.0, -1.0], east);
        assert!((face[1] - 1.0).abs() < 1e-5, "{face:?}");
        let south = sleeping_point([0.0, 32.0, 0.0], sleep_direction_rotation("south"));
        assert!((south[2] - 32.0).abs() < 1e-3, "south: {south:?}");
    }

    /// M402 — a glider's model: the head pins at −π/4 whatever the look
    /// (and the swim's head lerp yields to it), and the walk swing of arms
    /// and legs divides by `speedValue`; not gliding, nothing changes.
    #[test]
    fn a_gliders_head_pins_and_its_walk_slows() {
        use mobs::Anim as A;
        use std::f32::consts::FRAC_PI_4;
        let mut c = ctx([0.0; 2], 0.0);
        c.pitch = 0.6;
        c.f = 1.0;
        c.amt = 1.0;
        let walk_leg = anim_delta(A::LegRight, 1.0, &c).0[0];
        let walk_arm = anim_delta(A::HumanoidArmRight, 1.0, &c).0[0];
        assert_eq!(anim_delta(A::Head, 1.0, &c).0[0], 0.6);
        c.mob.fall_flying = true;
        c.mob.fall_speed = 4.0;
        c.mob.swim_amount = 0.5;
        assert_eq!(anim_delta(A::Head, 1.0, &c).0[0], -FRAC_PI_4);
        let leg = anim_delta(A::LegRight, 1.0, &c).0[0];
        assert!(
            (leg - (walk_leg / 4.0 * 0.5 + 0.3 * mth_cos(0.0) * 0.5)).abs() < 1e-3,
            "{leg} {walk_leg}"
        );
        c.mob.swim_amount = 0.0;
        let leg = anim_delta(A::LegRight, 1.0, &c).0[0];
        assert!((leg - walk_leg / 4.0).abs() < 1e-6, "{leg} {walk_leg}");
        assert!(walk_arm.abs() > 1e-3);
        // The arm's walk divides; its idle bob (age 0: zRot only) does not
        // touch xRot.
        let arm = anim_delta(A::HumanoidArmRight, 1.0, &c).0[0];
        assert!((arm - walk_arm / 4.0).abs() < 1e-6, "{arm} {walk_arm}");
        // A divisor below 1 is vanilla's floor.
        c.mob.fall_speed = 0.2;
        assert_eq!(anim_delta(A::LegRight, 1.0, &c).0[0], walk_leg);
    }

    /// M402 — the render frame: a glider (`XP(−90)` at `xRot` 0) and a
    /// riptide spinner both lie with the head toward where they look (yaw
    /// 0 looks south, +z), the flying turn swings that toward the movement,
    /// and the spin turns about the body's length (the head stays put).
    #[test]
    fn a_glider_and_a_spinner_lie_along_the_look() {
        let frame = |glide: Option<YxRotation>, inner: InnerRotation| EntityFrame {
            pos: [0.0; 3],
            scale: 1.0 / 16.0,
            stretch: [1.0, 1.0],
            glide,
            swim: None,
            inner,
            pitch: None,
            yaw: (180f32.to_radians()).sin_cos(),
            mount: None,
        };
        // The crown of a standing model's head: 32 px above the feet.
        let crown = [0.0, mobs::MODEL_EYE_Y - 32.0, 0.0];
        let upright = frame(None, InnerRotation::Topple(0.0, 1.0)).place(crown);
        assert!(
            (upright[1] - 2.0).abs() < 1e-4 && upright[2].abs() < 1e-4,
            "{upright:?}"
        );
        let mob = mobs::MobCombat {
            fall_flying: true,
            glide_tilt: -90.0,
            ..Default::default()
        };
        let g = frame(glide_rotations(&mob), InnerRotation::Topple(0.0, 1.0)).place(crown);
        assert!(g[1].abs() < 1e-4 && (g[2] - 2.0).abs() < 1e-4, "{g:?}");
        // Moving east while looking south, the body turns a quarter toward
        // the movement: the head comes round to the side.
        let turned = mobs::MobCombat {
            flying_y_rot: Some(std::f32::consts::FRAC_PI_2),
            ..mob
        };
        let t = frame(glide_rotations(&turned), InnerRotation::Topple(0.0, 1.0)).place(crown);
        assert!(
            t[1].abs() < 1e-4 && (t[2] - 2.0).abs() < 1e-4,
            "the turn is about the body's length: {t:?}"
        );
        let shoulder = [6.0, mobs::MODEL_EYE_Y - 22.0, 0.0];
        let s0 = frame(glide_rotations(&mob), InnerRotation::Topple(0.0, 1.0)).place(shoulder);
        let s1 = frame(glide_rotations(&turned), InnerRotation::Topple(0.0, 1.0)).place(shoulder);
        assert!((s0[0] - s1[0]).abs() > 0.3, "{s0:?} {s1:?}");
        // The spin: the same lie at `xRot` 0, at every spin angle.
        for age in [0.0, 1.0, 2.5] {
            let sp = frame(None, InnerRotation::Spin(spin_rotations(0.0, age))).place(crown);
            assert!(
                sp[1].abs() < 1e-4 && (sp[2] - 2.0).abs() < 1e-4,
                "{age}: {sp:?}"
            );
        }
        // No tilt and no turn is no glide at all.
        assert!(glide_rotations(&mobs::MobCombat {
            fall_flying: true,
            ..Default::default()
        })
        .is_none());
    }

    /// M400 — `HumanoidModel`'s swim at full swim amount: the head pitches
    /// to −π/4; at stroke position 0 both arms reach overhead (`xRot` 0,
    /// `yRot` π, `zRot` π); at 18 (halfway through the pull) `xRot` π/4,
    /// the right arm's `zRot` 1.2708 + 1.8708/2; the legs kick
    /// `0.3·cos(pos/3)`; at half the amount, half way from the walk pose.
    /// The tilt: −90° out of water, `−90 − xRot` in it.
    #[test]
    fn an_avatar_swims_as_humanoid_model_swims() {
        use mobs::Anim as A;
        use std::f32::consts::PI;
        let at = |pos: f32, amount: f32| {
            let mut c = ctx([0.0; 2], 0.0);
            c.pos = pos;
            c.mob.swim_amount = amount;
            c
        };
        let c0 = at(0.0, 1.0);
        assert!((anim_delta(A::Head, 1.0, &c0).0[0] + PI / 4.0).abs() < 1e-5);
        for arm in [A::HumanoidArmRight, A::HumanoidArmLeft] {
            // The left arm's `rotLerpRad` turns a difference of exactly π
            // to −π: the same angle.
            let r = anim_delta(arm, 1.0, &c0).0;
            let same = |a: f32, b: f32| {
                ((a - b).rem_euclid(2.0 * PI)).min((b - a).rem_euclid(2.0 * PI)) < 1e-3
            };
            assert!(
                r[0].abs() < 1e-4 && same(r[1], PI) && same(r[2], PI),
                "{r:?}"
            );
        }
        let c18 = at(18.0, 1.0);
        let r = anim_delta(A::HumanoidArmRight, 1.0, &c18).0;
        assert!((r[0] - PI / 4.0).abs() < 1e-4);
        assert!((r[2] - (1.270_796_3 + 1.870_796_4 * 0.5)).abs() < 1e-4);
        let l = anim_delta(A::LegRight, 1.0, &c18).0[0];
        assert!((l - 0.3 * (18.0f32 / 3.0).cos()).abs() < 1e-3);
        // Half the amount: half way from the (still) walk pose.
        let h = anim_delta(A::LegRight, 1.0, &at(18.0, 0.5)).0[0];
        assert!((h - l * 0.5).abs() < 1e-3, "{h} {l}");
        assert_eq!(swim_tilt(1.0, false, 30.0), -90.0);
        assert_eq!(swim_tilt(0.5, true, 30.0), -60.0);
    }

    /// M399 — a crouching avatar: the body bends 0.5 and drops 3.2 with the
    /// arms (which swing 0.4 further), the legs step back 4, the head drops
    /// 4.2; the spyglass arm lowers π/12 more; nothing without the flag.
    #[test]
    fn a_crouching_avatar_bends_as_humanoid_model_bends_it() {
        use mobs::Anim as A;
        use std::f32::consts::PI;
        let stand = ctx([0.0; 2], 0.0);
        let mut crouch = ctx([0.0; 2], 0.0);
        crouch.mob.crouching = true;
        let d = |a: A, c: &AnimCtx| anim_delta(a, 1.0, c);
        assert_eq!(d(A::HumanoidBody, &crouch).0[0], 0.5);
        assert_eq!(d(A::HumanoidBody, &crouch).1[1], 3.2);
        for arm in [A::HumanoidArmRight, A::HumanoidArmLeft] {
            let (s, c) = (d(arm, &stand), d(arm, &crouch));
            assert!((c.0[0] - s.0[0] - 0.4).abs() < 1e-6);
            assert!((c.1[1] - s.1[1] - 3.2).abs() < 1e-6);
        }
        assert_eq!(d(A::LegRight, &crouch).1[2], 4.0);
        assert_eq!(d(A::Head, &crouch).1[1], 4.2);
        for a in [A::HumanoidBody, A::LegLeft, A::Head] {
            assert_eq!(d(a, &stand).1, [0.0; 3]);
        }
        let spyglass = |crouching: bool| {
            let mut c = ctx([0.0; 2], 0.0);
            c.arm_poses.right = mobs::ArmPose::Spyglass;
            c.arm_poses.known = true;
            c.mob.crouching = crouching;
            c
        };
        let (spy, spy_crouch) = (spyglass(false), spyglass(true));
        let (s, c) = (
            d(A::HumanoidArmRight, &spy).0[0],
            d(A::HumanoidArmRight, &spy_crouch).0[0],
        );
        // The pose's −π/12, then the crouch block's +0.4.
        assert!((c - (s - PI / 12.0 + 0.4)).abs() < 1e-5, "{s} {c}");
    }

    /// M393 — at rowing time 0 a paddle rests where M223 baked it (no
    /// delta beyond a sine-table step); rowing, `animatePaddle`'s angles (the right's yaw mirrored);
    /// the hurt wobble's angle, negated past the flip, is the renderer's
    /// `rotX(θ)` before it: `S·Rx(−θ) = Rx(θ)·S` for `S = diag(−1, −1, 1)`.
    #[test]
    fn a_boat_rows_and_wobbles() {
        use mobs::Anim as A;
        use std::f32::consts::PI;
        // (`Mth`'s table sine against the bake's `f32::sin`: a table step.)
        for right in [false, true] {
            let d = anim_delta(A::BoatPaddle { right }, 1.0, &ctx([0.0; 2], 0.0)).0;
            assert!(d.iter().all(|v| v.abs() < 1e-4), "{d:?}");
        }
        let t = 1.3_f32;
        let c = ctx([t, t], 0.0);
        let rest_l = mobs::paddle_rest(false);
        let l = anim_delta(A::BoatPaddle { right: false }, 1.0, &c).0;
        let want_x = -PI / 3.0 + ((-t).sin() + 1.0) / 2.0 * (PI / 4.0);
        let want_y = -PI / 4.0 + ((1.0 - t).sin() + 1.0) / 2.0 * (PI / 2.0);
        assert!((l[0] + rest_l[0] - want_x).abs() < 1e-3, "{l:?}");
        assert!((l[1] + rest_l[1] - want_y).abs() < 1e-3);
        let rest_r = mobs::paddle_rest(true);
        let r = anim_delta(A::BoatPaddle { right: true }, 1.0, &c).0;
        assert!((r[1] + rest_r[1] - (PI - want_y)).abs() < 1e-3);
        // The wobble.
        let theta = 7.0_f32;
        let w = anim_delta(A::BoatHurt, 1.0, &ctx([0.0; 2], theta)).0;
        assert!((w[0] + theta * DEG).abs() < 1e-6);
        let s = [[-1.0, 0.0, 0.0], [0.0, -1.0, 0.0], [0.0, 0.0, 1.0]];
        let p = [0.3, -1.7, 2.9];
        let a = mat_apply(&mat_mul(s, rot_x(w[0])), p);
        let b = mat_apply(&mat_mul(rot_x(theta * DEG), s), p);
        for i in 0..3 {
            assert!((a[i] - b[i]).abs() < 1e-5, "{a:?} {b:?}");
        }
    }
}

#[cfg(test)]
mod piglin_arm_anim_tests {
    use super::*;

    /// M380 — standing still, a piglin's arms bob: z ±(cos(age·0.09)·0.05 +
    /// 0.05), x ±sin(age·0.067)·0.05.
    #[test]
    fn the_piglin_arms_bob() {
        let ctx = AnimCtx {
            pitch: 0.0,
            net: 0.0,
            f: 0.0,
            pos: 0.0,
            amt: 0.0,
            age: 21.0,
            gesture: Option::None,
            events: [Option::None; mobs::ModelEvent::COUNT],
            shell: false,
            body_yaw: 0.0,
            stand_pose: mobs::STAND_DEFAULT_POSE,
            allay_dance: Option::None,
            attack: mobs::SwingPose::NONE,
            arm_poses: mobs::ArmPoses::EMPTY,
            mob: mobs::MobCombat::default(),
            tendril: 0.0,
        };
        let r = anim_delta(mobs::Anim::PiglinArm { left: false }, 1.0, &ctx).0;
        let l = anim_delta(mobs::Anim::PiglinArm { left: true }, 1.0, &ctx).0;
        let z = (21.0_f32 * 0.09).cos() * 0.05 + 0.05;
        // `Mth`'s table trig, so a table step's tolerance.
        assert!((r[2] - z).abs() < 1e-4 && (l[2] + z).abs() < 1e-4);
        assert!(
            (r[0] - (21.0_f32 * 0.067).sin() * 0.05).abs() < 1e-4 && (l[0] + r[0]).abs() < 1e-4
        );
    }

    fn posed(pose: mobs::PiglinArmPose, age: f32) -> AnimCtx {
        AnimCtx {
            pitch: 0.2,
            net: 0.3,
            f: 0.0,
            pos: 0.0,
            amt: 0.0,
            age,
            gesture: Option::None,
            events: [Option::None; mobs::ModelEvent::COUNT],
            shell: false,
            body_yaw: 0.0,
            stand_pose: mobs::STAND_DEFAULT_POSE,
            allay_dance: Option::None,
            attack: mobs::SwingPose::NONE,
            arm_poses: mobs::ArmPoses::EMPTY,
            mob: mobs::MobCombat {
                piglin_pose: pose,
                ..Default::default()
            },
            tendril: 0.0,
        }
    }

    /// M390 — `PiglinModel.setupAnim`'s DANCING at age 60 (dancePos 1): the
    /// arms out at ±(70 + cos 40·10)°, raised and lowered by sin 40·0.5 ∓
    /// 0.5; the head bobbed (sin 10, sin 40 + 0.4); the body by sin 40·0.35;
    /// the ears assigned about π/6.
    #[test]
    fn a_dancing_piglin_throws_its_arms_out_and_bobs() {
        use mobs::Anim as A;
        use std::f32::consts::PI;
        let c = posed(mobs::PiglinArmPose::Dancing, 60.0);
        let (r, ro) = anim_delta(A::PiglinArm { left: false }, 1.0, &c);
        let (l, lo) = anim_delta(A::PiglinArm { left: true }, 1.0, &c);
        let z = (70.0 + 40.0_f32.cos() * 10.0) * DEG;
        assert!(
            (r[2] - z).abs() < 1e-3 && (l[2] + z).abs() < 1e-3,
            "{r:?} {l:?}"
        );
        let s = 40.0_f32.sin();
        assert!((ro[1] - (s * 0.5 - 0.5)).abs() < 1e-3);
        assert!((lo[1] - (s * 0.5 + 0.5)).abs() < 1e-3);
        let (h, ho) = anim_delta(A::PiglinHead, 1.0, &c);
        assert_eq!([h[0], h[1]], [0.2, 0.3], "the look stays");
        assert!((ho[0] - 10.0_f32.sin()).abs() < 1e-3 && (ho[1] - (s + 0.4)).abs() < 1e-3);
        let (_, bo) = anim_delta(A::PiglinBody, 1.0, &c);
        assert!((bo[1] - s * 0.35).abs() < 1e-3);
        let er = anim_delta(A::PiglinEar { left: false }, 1.0, &c).0[2];
        let el = anim_delta(A::PiglinEar { left: true }, 1.0, &c).0[2];
        // Deltas on the adult's ±30° rest.
        assert!((er + PI / 6.0 - (PI / 6.0 + DEG * 30.0_f32.sin() * 10.0)).abs() < 1e-3);
        assert!((el - PI / 6.0 - (-PI / 6.0 - DEG * 30.0_f32.cos() * 10.0)).abs() < 1e-3);
        // Not dancing, none of it.
        let still = posed(mobs::PiglinArmPose::Default, 60.0);
        assert_eq!(anim_delta(A::PiglinBody, 1.0, &still).1, [0.0; 3]);
        assert_eq!(anim_delta(A::PiglinHead, 1.0, &still).1, [0.0; 3]);
    }

    /// M390 — ADMIRING_ITEM: the head looks down at 0.5 and ahead, and the
    /// off arm (the left, right-handed) lifts to (−0.9, 0.5); the main arm
    /// keeps the bob.
    #[test]
    fn an_admiring_piglin_stares_at_its_off_hand() {
        use mobs::Anim as A;
        let c = posed(mobs::PiglinArmPose::AdmiringItem, 21.0);
        assert_eq!(&anim_delta(A::PiglinHead, 1.0, &c).0[..2], &[0.5, 0.0]);
        let l = anim_delta(A::PiglinArm { left: true }, 1.0, &c).0;
        assert_eq!([l[0], l[1]], [-0.9, 0.5]);
        let r = anim_delta(A::PiglinArm { left: false }, 1.0, &c).0;
        let bob = anim_delta(
            A::PiglinArm { left: false },
            1.0,
            &posed(mobs::PiglinArmPose::Default, 21.0),
        )
        .0;
        assert_eq!(r, bob);
        // Left-handed, the right arm admires.
        let mut lh = c;
        lh.mob.main_arm_left = true;
        let r = anim_delta(A::PiglinArm { left: false }, 1.0, &lh).0;
        assert_eq!([r[0], r[1]], [-0.9, -0.5]);
    }

    /// M392 — not attacking with a melee weapon, a piglin swings as
    /// `HumanoidModel` does (the arm, its moved pivot and the body's yaw:
    /// the player arm's very numbers); the baby's arm pivot is assigned the
    /// same ±5·ageScale from its ±4 rest. A held spear poses the arm.
    #[test]
    fn a_piglin_swings_and_holds_as_a_humanoid() {
        use mobs::Anim as A;
        let mut c = posed(mobs::PiglinArmPose::Default, 21.0);
        c.attack.attack_time = 0.3;
        c.attack.age_scale = 1.0;
        let pig = anim_delta(A::PiglinArm { left: false }, 1.0, &c);
        let hum = anim_delta(A::HumanoidArmRight, 1.0, &c);
        assert_eq!(pig, hum);
        assert_ne!(pig.1, [0.0; 3], "the swing moves the pivot");
        assert_eq!(
            anim_delta(A::PiglinBody, 1.0, &c).0,
            anim_delta(A::HumanoidBody, 1.0, &c).0
        );
        assert_ne!(anim_delta(A::PiglinBody, 1.0, &c).0[1], 0.0);
        // The baby: x = −cos(by)·5·0.5 assigned over its −4 rest.
        c.mob.is_baby = true;
        c.attack.age_scale = 0.5;
        let by = attack_body_yrot(&c.attack);
        let baby = anim_delta(A::PiglinArm { left: false }, 1.0, &c).1;
        assert!(
            (baby[0] - (-mth_cos(by as f64) * 2.5 + 4.0)).abs() < 1e-5,
            "{baby:?}"
        );
        // Swinging a melee weapon down: no body yaw (`swingWeaponDown`).
        let mut m = posed(mobs::PiglinArmPose::AttackingWithMeleeWeapon, 21.0);
        m.attack.attack_time = 0.3;
        assert_eq!(anim_delta(A::PiglinBody, 1.0, &m).0, [0.0; 3]);
        assert_eq!(
            anim_delta(A::PiglinArm { left: false }, 1.0, &m).1,
            [0.0; 3]
        );
        // A spear in the right hand: the hold pose the humanoid arm takes.
        let mut sp = posed(mobs::PiglinArmPose::Default, 21.0);
        sp.arm_poses.right = mobs::ArmPose::Spear;
        assert_eq!(
            anim_delta(A::PiglinArm { left: false }, 1.0, &sp),
            anim_delta(A::HumanoidArmRight, 1.0, &sp)
        );
        assert_ne!(
            anim_delta(A::PiglinArm { left: false }, 1.0, &sp).0,
            anim_delta(
                A::PiglinArm { left: false },
                1.0,
                &posed(mobs::PiglinArmPose::Default, 21.0)
            )
            .0
        );
    }

    /// M390 — ATTACKING_WITH_MELEE_WEAPON holds the weapon high (main arm
    /// −1.8) until the swing, which `swingWeaponDown` takes over; the
    /// crossbow poses aim along the look and crank by the use clock.
    #[test]
    fn a_piglin_raises_its_weapon_and_aims_its_crossbow() {
        use mobs::Anim as A;
        use std::f32::consts::PI;
        let c = posed(mobs::PiglinArmPose::AttackingWithMeleeWeapon, 21.0);
        assert_eq!(anim_delta(A::PiglinArm { left: false }, 1.0, &c).0[0], -1.8);
        assert_ne!(anim_delta(A::PiglinArm { left: true }, 1.0, &c).0[0], -1.8);
        let mut swing = c;
        swing.attack.attack_time = 0.5;
        let r = anim_delta(A::PiglinArm { left: false }, 1.0, &swing).0;
        assert!(r[0] != -1.8 && (r[1] - PI / 20.0).abs() < 1e-6, "{r:?}");
        let h = posed(mobs::PiglinArmPose::CrossbowHold, 21.0);
        let r = anim_delta(A::PiglinArm { left: false }, 1.0, &h).0;
        let l = anim_delta(A::PiglinArm { left: true }, 1.0, &h).0;
        assert!(
            (r[0] - (-PI / 2.0 + 0.2 + 0.1)).abs() < 1e-6 && (r[1] - (-0.3 + 0.3)).abs() < 1e-6
        );
        assert!((l[0] - (-1.5 + 0.2)).abs() < 1e-6 && (l[1] - (0.6 + 0.3)).abs() < 1e-6);
        let mut ch = posed(mobs::PiglinArmPose::CrossbowCharge, 21.0);
        ch.arm_poses.ticks_using_item = 25.0;
        let r = anim_delta(A::PiglinArm { left: false }, 1.0, &ch).0;
        let l = anim_delta(A::PiglinArm { left: true }, 1.0, &ch).0;
        assert_eq!([r[0], r[1]], [-0.970_796_35, -0.8]);
        assert!(
            (l[0] + PI / 2.0).abs() < 1e-6 && (l[1] - 0.85).abs() < 1e-6,
            "{l:?}"
        );
    }
}

#[cfg(test)]
mod riding_pose_anim_tests {
    use super::*;
    use std::f32::consts::PI;

    fn ctx(riding: bool) -> AnimCtx {
        AnimCtx {
            pitch: 0.0,
            net: 0.0,
            f: 0.0,
            pos: 0.0,
            amt: 0.0,
            age: 0.0,
            gesture: Option::None,
            events: [Option::None; mobs::ModelEvent::COUNT],
            shell: false,
            body_yaw: 0.0,
            stand_pose: mobs::STAND_DEFAULT_POSE,
            allay_dance: Option::None,
            attack: mobs::SwingPose::NONE,
            arm_poses: mobs::ArmPoses::EMPTY,
            mob: mobs::MobCombat {
                riding,
                ..Default::default()
            },
            tendril: 0.0,
        }
    }

    /// M381 — riding, the legs splay forward (±π/10, ±0.0785), a humanoid
    /// arm adds −π/5 (on top of its idle bob), an illager arm is −π/5
    /// outright; standing, none of it.
    #[test]
    fn a_rider_sits() {
        use mobs::Anim::*;
        let (r, s) = (ctx(true), ctx(false));
        assert_eq!(
            anim_delta(LegRight, 1.0, &r).0,
            [-1.4137167, PI / 10.0, 0.07853982]
        );
        assert_eq!(
            anim_delta(LegLeft, 1.0, &r).0,
            [-1.4137167, -PI / 10.0, -0.07853982]
        );
        assert_eq!(anim_delta(LegRight, 1.0, &s).0[1], 0.0);
        let ride = anim_delta(HumanoidArmRight, 1.0, &r).0[0];
        let stand = anim_delta(HumanoidArmRight, 1.0, &s).0[0];
        assert!((ride - stand + PI / 5.0).abs() < 1e-6);
        assert_eq!(anim_delta(IllagerArmLeft, 1.0, &r).0, [-PI / 5.0, 0.0, 0.0]);
    }
}

#[cfg(test)]
mod drowned_arm_anim_tests {
    use super::*;
    use std::f32::consts::PI;

    /// M382 — an arm posed `THROW_TRIDENT` takes `DrownedModel`'s post-step
    /// over the zombie arm (half the angle, less π; no yaw); an unposed
    /// drowned arm is the zombie's.
    #[test]
    fn the_drowned_raises_its_trident() {
        let ctx = |right: mobs::ArmPose| AnimCtx {
            pitch: 0.0,
            net: 0.0,
            f: 0.0,
            pos: 0.0,
            amt: 0.0,
            age: 0.0,
            gesture: Option::None,
            events: [Option::None; mobs::ModelEvent::COUNT],
            shell: false,
            body_yaw: 0.0,
            stand_pose: mobs::STAND_DEFAULT_POSE,
            allay_dance: Option::None,
            attack: mobs::SwingPose::NONE,
            arm_poses: mobs::ArmPoses {
                right,
                ..mobs::ArmPoses::EMPTY
            },
            mob: mobs::MobCombat::default(),
            tendril: 0.0,
        };
        let plain = ctx(mobs::ArmPose::Empty);
        assert_eq!(
            anim_delta(mobs::Anim::DrownedArmRight, 1.0, &plain),
            anim_delta(mobs::Anim::UndeadArmRight, 1.0, &plain)
        );
        let throw = ctx(mobs::ArmPose::ThrowTrident);
        let zombie = anim_delta(mobs::Anim::UndeadArmRight, 1.0, &throw).0;
        let drowned = anim_delta(mobs::Anim::DrownedArmRight, 1.0, &throw).0;
        assert!((drowned[0] - (zombie[0] * 0.5 - PI)).abs() < 1e-6);
        assert_eq!(drowned[1], 0.0);
    }
}
