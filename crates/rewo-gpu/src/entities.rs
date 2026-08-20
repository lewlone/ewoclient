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
//! Buffers are a ring flipped on each `set_draws`, [`RING`] slots long. The
//! comment that stood here said "2-slot … with the frame driver fence-pacing at
//! most 2 frames in flight, the slot being rewritten retired two submissions
//! ago" — which was the wrong count *and* the wrong reasoning: `set_draws` runs
//! before the frame's fence wait, not after it, so a slot must survive
//! `fif + 1` frames rather than `fif`. See [`RING`] (M86).
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
/// ~500 capsules' worth (a flat-world slime herd alone reaches 129
/// entities ≈ 65k verts). 9.4 MB × 2 ring slots — cheap; the CPU soup
/// build is the real ceiling long before this is.
const MAX_VERTS: usize = 262_144;
/// Slots in the vertex ring — `MAX_FRAMES_IN_FLIGHT + 1`, the same rule
/// [`crate::buf_ring::BUF_RING`] states and for the same reason.
///
/// **This was 2 until M86, and 2 is one short at the default `--fif 2`.**
/// `set_draws` runs in the app's frame loop *before* `Renderer::render`, so the
/// most recent fence wait was the previous frame's: frames `n-1` and `n-2` may
/// both still be reading when frame `n` writes. A 2-slot ring hands frame `n`
/// the slot frame `n-2` is on. Nothing catches it — the write is a CPU memcpy
/// into a mapped allocation, not a `vkDestroy`, so core validation is silent
/// and the only symptom is entity geometry that is briefly some other frame's.
///
/// Argued from that derivation, not measured: this milestone has no gate that
/// can see a CPU/GPU data race, and the ~27 MB it costs
/// (`MAX_VERTS × VERTEX_STRIDE × 2` extra slots, per `EntityPass`) buys
/// correctness at every `--fif` the knob permits rather than at one.
const RING: usize = crate::MAX_FRAMES_IN_FLIGHT + 1;
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
/// inside the emitter.
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

pub struct EntityDraw<'a> {
    /// Feet-center world position.
    pub pos: [f32; 3],
    pub width: f32,
    pub height: f32,
    /// Linear-space base color (capsules; the player model is textured).
    pub color: [f32; 3],
    /// Nametag text (players); `None` draws no tag.
    pub name: Option<&'a str>,
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
    /// Base tint × directional face shade. **The world light is deliberately
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
const ATLAS_H: u32 = 1600;

/// Dynamic player-skin pool: 32 slots of 64×64 in the atlas's bottom two
/// rows, filled at runtime as players' skins arrive. The mob packer is capped
/// above the dynamic bands so it never collides.
/// The most textures any one `mobs::MobDef` lists — the sheep's base + fleece
/// + undercoat (M68). Only sizes the per-slot tint scratch in `emit_model`; a
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
/// Every dynamic pool in this file hands out `next % cap` and increments, and
/// the field comment on `skin_next` states the hazard as if it were the design:
/// *"a small network never fills 32, and wrap-around just recycles the oldest
/// slot."* Recycling is fine for a pool nothing remembers. It is a real
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
/// the whole suite and every gate green, which is exactly how the pre-M161
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

const SKIN_SLOT: u32 = 64;
const SKIN_POOL_COLS: u32 = ATLAS_W / SKIN_SLOT; // 16
const SKIN_POOL_ROWS: u32 = 2;
const SKIN_SLOTS: u32 = SKIN_POOL_COLS * SKIN_POOL_ROWS; // 32
const SKIN_POOL_Y: u32 = TRIM_POOL_Y - SKIN_POOL_ROWS * SKIN_SLOT; // 1152

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
const TRIM_POOL_Y: u32 = CAPE_POOL_Y - TRIM_POOL_ROWS * TRIM_SLOT_H; // 1280

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
const CAPE_POOL_Y: u32 = ATLAS_H - CAPE_POOL_ROWS * CAPE_SLOT_H; // 1536

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
const ITEM_POOL_Y: u32 = SKIN_POOL_Y - ITEM_POOL_ROWS * ITEM_SLOT; // 896

/// Atlas origin of dynamic skin slot `i` (0..SKIN_SLOTS).
fn skin_slot_origin(i: u32) -> (u32, u32) {
    (
        (i % SKIN_POOL_COLS) * SKIN_SLOT,
        SKIN_POOL_Y + (i / SKIN_POOL_COLS) * SKIN_SLOT,
    )
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
        shade: q.shade,
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
        let Some(&(ox, oy)) = pack_emissive.get(key) else { continue };
        let Some(&(_, _, tw, th)) = slots.get(key) else { continue };
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
        out.push(EmissiveDraw { quads, alpha: mobs::EmissiveAlpha::Always, cutout: true });
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
        let Some(&(_, _, bw, bh)) = slots.get(def.textures[0]) else { continue };
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
        out.push(EmissiveDraw { quads, alpha: layer.alpha, cutout: layer.cutout });
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
}

pub struct EntityPass {
    layout: vk::PipelineLayout,
    set_layout: vk::DescriptorSetLayout,
    solid_pipeline: vk::Pipeline,
    text_pipeline: vk::Pipeline,
    /// Vanilla's `EYES` / `ENTITY_TRANSLUCENT_EMISSIVE` (M57): translucent,
    /// depth-write off, and `GREATER_OR_EQUAL` so a layer lands on the base
    /// model's own depth instead of being rejected by it.
    emissive_pipeline: vk::Pipeline,
    emissive_verts: u32,
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
    bufs: [vk::Buffer; RING],
    allocs: [Option<Allocation>; RING],
    cursor: usize,
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
    /// `<asset>/<layer>` → the sheet's packed rect in the entity atlas (M46).
    armor_slots: std::collections::HashMap<String, (u32, u32, u32, u32)>,
    /// Sprite path → its origin in the trim pool (M48).
    trim_slots: std::collections::HashMap<String, (u32, u32)>,
    trim_ring: SlotRing<String>,
    /// Round-robin ring over the item pool, like `trim_ring`.
    item_ring: SlotRing<u16>,
    /// Next free dynamic skin slot, wrapping at `SKIN_SLOTS` (32).
    ///
    /// **This still aliases, and the alias is the caller's to see.** Unlike the
    /// trim and item pools — which cache key → slot here and so could be fixed
    /// here, by [`SlotRing`] evicting the stale key — this pool hands the
    /// address *out* and the app stores it per-uuid forever
    /// (`live_cmd`'s skin registry). The 33rd distinct player of a session
    /// therefore overwrites slot 0 while player #1's stored `skin_uv` still
    /// points at it, and player #1 renders wearing player #33's skin with
    /// nothing anywhere reporting it. Closing it needs the slot returned
    /// alongside the UV and the app dropping the evicted uuid's entry, which is
    /// an API change through `WorldRenderer` and two registries, so it is
    /// recorded rather than half-done.
    skin_next: u32,
    /// Round-robin cursor into the cape pool (M60) — the same, at 32 slots,
    /// and worse in kind: a cape origin is an absolute texel address rather
    /// than a delta, so a recycled one samples a *fixed* wrong rectangle.
    cape_next: u32,
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
}

/// Generations an unseen entity's CEM state survives before being pruned —
/// long enough that a briefly-culled mob keeps its integrators.
const CEM_STATE_TTL: u64 = 600;

/// One mob model ready to draw: quads in part-local model px with
/// atlas-normalized UVs, the animated parts, and the px→block scale.
pub struct MobModel {
    quads: Vec<GpuQuad>,
    /// The texture slot vanilla renders through a dye tint (the sheep's
    /// wool), if any — `EntityDraw::dye` multiplies only that slot.
    tinted_slot: Option<u8>,
    /// The texture slot belonging to a render layer a shorn mob skips
    /// (`mobs::shearable_texture`) — `EntityDraw::sheared` drops it (M64).
    shearable_slot: Option<u8>,
    /// The texture slot of a **coplanar** render layer
    /// (`mobs::coplanar_layer_texture`) — the sheep's undercoat (M68). Its
    /// quads leave the solid range for the `EQUAL`, no-write trim range,
    /// because they land at exactly the base model's depth.
    coplanar_slot: Option<u8>,
    /// A tropical fish's `(body, pattern)` texture slots — the two vanilla
    /// tints from its packed variant (M68).
    fish_slots: Option<(u8, u8)>,
    /// ETF alternates: variant id → per-texture-slot UV offset, added to the
    /// quad's UVs to move it onto the alternate's atlas slot. A variant with
    /// no entry for a slot leaves that slot on the vanilla texture, which is
    /// what a pack varying only one of a mob's textures wants.
    variants: std::collections::HashMap<u16, Vec<[f32; 2]>>,
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
    /// Per-bone top-level flag — top-level parts and submodels state their
    /// animated translation in different frames, so the runtime must know
    /// which a bone is.
    cem_top: Vec<bool>,
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
    shade: f32,
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
        Self::new_with_cem(gpu, color_format, font, tex, std::collections::HashMap::new())
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
        let mut sizes: Vec<(u32, u32)> = order.iter().map(|&i| (tex.entries[i].w, tex.entries[i].h)).collect();
        sizes.extend(tex.variants.iter().map(|v| (v.w, v.h)));
        let origins = pack_shelves(&sizes);
        let mut slots: std::collections::HashMap<&str, (u32, u32, u32, u32)> =
            std::collections::HashMap::new();
        for (slot, &i) in order.iter().enumerate() {
            let e = &tex.entries[i];
            match origins[slot] {
                Some((x, y)) if blit_tex(&mut atlas, Some(e.rgba), x, y, e.w, e.h) => {
                    slots.insert(e.key, (x, y, e.w, e.h));
                }
                _ => log::warn!("entities: mob texture {} ({}×{}) didn't pack", e.key, e.w, e.h),
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
                Some((x, y)) if blit_tex(&mut atlas, Some(v.rgba), x, y, v.w, v.h) => {
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
        let debug_tex = std::env::var("REWO_MOB_DEBUG_TEX").map_or(false, |v| v == "1");

        // Build each registry mob whose textures are all present.
        let mut models: Vec<Option<MobModel>> = (0..EntityModelKind::COUNT).map(|_| None).collect();
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
                    log::info!("cem: {:?} using pack model ({} quads)", def.kind, m.quads.len());
                    m
                }
                None => (def.build)(),
            };
            if debug_tex {
                for q in &m.quads {
                    if !paint_debug_rect(&mut atlas, origins[q.tex], q, &mut painted) {
                        if ambiguous_tex.insert(def.textures[q.tex]) {
                            log::info!(
                                "mob debug-tex: {} ({:?} {:?} quad) repaints a texel with a new label",
                                def.textures[q.tex],
                                def.kind,
                                q.facing
                            );
                        }
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
            let quads = m
                .quads
                .iter()
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
                        shade: q.shade,
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
                let Some(slot) = def.textures.iter().position(|k| *k == base_key) else { continue };
                let (bx, by, _, _) = origins[slot];
                let e = variants
                    .entry(index as u16)
                    .or_insert_with(|| vec![[0.0, 0.0]; def.textures.len()]);
                e[slot] = [
                    (vx as f32 - bx as f32) / ATLAS_W as f32,
                    (vy as f32 - by as f32) / ATLAS_H as f32,
                ];
            }
            models[def.kind.index()] = Some(MobModel {
                quads,
                tinted_slot: mobs::tinted_texture(def.kind)
                    .and_then(|k| def.textures.iter().position(|t| *t == k))
                    .map(|s| s as u8),
                shearable_slot: mobs::shearable_texture(def.kind)
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
                emissive,
                parts: m.parts,
                keyframes: m.keyframes,
                event_rigs: m.event_rigs,
                scale: m.scale / 16.0,
                cem: m.cem,
                cem_translate: m.cem_translate,
                cem_top: m.cem_top,
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

            let pc = [vk::PushConstantRange::default()
                .stage_flags(vk::ShaderStageFlags::VERTEX)
                .offset(0)
                .size(64)];
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

            let mut bufs = [vk::Buffer::null(); RING];
            let mut allocs: [Option<Allocation>; RING] = std::array::from_fn(|_| None);
            for i in 0..RING {
                let buffer = device
                    .create_buffer(
                        &vk::BufferCreateInfo::default()
                            .size(MAX_VERTS as u64 * VERTEX_STRIDE)
                            .usage(vk::BufferUsageFlags::VERTEX_BUFFER)
                            .sharing_mode(vk::SharingMode::EXCLUSIVE),
                        None,
                    )
                    .map_err(|e| format!("entity vbuf: {e}"))?;
                let req = device.get_buffer_memory_requirements(buffer);
                let alloc = gpu
                    .allocator
                    .allocate(&AllocationCreateDesc {
                        name: "entity-verts",
                        requirements: req,
                        location: MemoryLocation::CpuToGpu,
                        linear: true,
                        allocation_scheme: AllocationScheme::GpuAllocatorManaged,
                    })
                    .map_err(|e| format!("entity vbuf alloc: {e}"))?;
                device
                    .bind_buffer_memory(buffer, alloc.memory(), alloc.offset())
                    .map_err(|e| format!("entity vbuf bind: {e}"))?;
                bufs[i] = buffer;
                allocs[i] = Some(alloc);
            }

            Ok(Self {
                layout,
                set_layout,
                solid_pipeline,
                text_pipeline,
                emissive_pipeline,
                emissive_verts: 0,
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
                bufs,
                allocs,
                cursor: 0,
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
                player_origin: slots.get("player").map(|&(x, y, _, _)| (x, y)).unwrap_or((0, 0)),
                // Armour sheets are looked up by name every frame — a mob's
                // model is fixed at build time, but what it wears is not.
                armor_slots: slots
                    .iter()
                    // `<layer dir>/<texture>` — both humanoid layer types
                    // start with `humanoid`, and no mob texture does.
                    .filter(|(k, _)| k.starts_with("humanoid"))
                    .map(|(k, v)| ((*k).to_string(), *v))
                    .collect(),
                skin_next: 0,
                cape_next: 0,
                held_items: None,
                item_slots: std::collections::HashMap::new(),
                item_ring: SlotRing::new(ITEM_SLOTS),
                cem_state: std::collections::HashMap::new(),
                generation: 0,
                frame_counter: 0.0,
                frame_dt: 1.0 / 20.0,
                prev_time: f32::NAN,
                cam_pos: [0.0; 3],
            })
        }
    }

    /// Reserve the next dynamic skin slot, upload a 64×64 RGBA skin into it,
    /// and return the normalized UV offset relocating the default player
    /// quads onto it (feed to `EntityDraw::skin_uv`). `rgba` must be
    /// `64*64*4` bytes. Stalls on `wait_idle` — skins arrive rarely (once
    /// per player at join), so the one-off is cheaper than tracking
    /// per-frame fences against the shared atlas.

    /// Install the baked held-item models (M22). Textures are *not* uploaded
    /// here: 26.2 ships 1233 of them and the atlas band holds 1024 slots, so
    /// they are paged in on demand by [`Self::prepare_held_items`].
    pub fn set_held_items(&mut self, items: crate::held::HeldItems) {
        self.held_items = Some(items);
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
    pub fn prepare_held_items(
        &mut self,
        gpu: &mut Gpu,
        names: &[&str],
    ) -> Result<(), String> {
        let Some(items) = self.held_items.take() else {
            return Ok(());
        };
        let mut result = Ok(());
        for name in names {
            let Some(model) = items.any(*name) else {
                continue;
            };
            for q in &model.quads {
                if self.item_slots.contains_key(&q.tex) {
                    continue;
                }
                let Some(tex) = items.textures.get(q.tex as usize) else {
                    continue;
                };
                // Slots are one texel-block; a larger sprite is skipped rather
                // than scaled, so it is visibly absent instead of subtly wrong.
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
        self.trim_slots.iter().map(|(k, v)| (k.clone(), *v)).collect()
    }

    /// Atlas UV rect `(u0, v0, du, dv)` of a resident item texture.
    fn item_uv(&self, tex: u16) -> Option<[f32; 4]> {
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
            log::warn!("entities: trim sprite {key} is {w}x{h}, expected {TRIM_SLOT_W}x{TRIM_SLOT_H}");
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

    pub fn upload_skin(&mut self, gpu: &mut Gpu, rgba: &[u8]) -> Result<[f32; 2], String> {
        let slot = self.skin_next % SKIN_SLOTS;
        self.skin_next += 1;
        let (sx, sy) = skin_slot_origin(slot);
        upload_region(gpu, self.image, rgba, sx, sy, SKIN_SLOT, SKIN_SLOT)?;
        let (px, py) = self.player_origin;
        Ok([
            (sx as f32 - px as f32) / ATLAS_W as f32,
            (sy as f32 - py as f32) / ATLAS_H as f32,
        ])
    }

    /// Claim a cape slot and upload one player's cape sheet into it (M60).
    ///
    /// Returns the slot's **atlas origin in texels**, not a UV delta. A skin
    /// relocates the player model's baked quads, so it answers with an offset
    /// to add to them; the cape has no baked quads to relocate — its emitter
    /// builds UVs from this origin directly, the way `emit_armor` does with a
    /// trim's.
    ///
    /// Round-robin with no eviction bookkeeping, as the pools before it: past
    /// 32 resident capes the oldest slot is overwritten.
    pub fn upload_cape(&mut self, gpu: &mut Gpu, rgba: &[u8]) -> Result<(u32, u32), String> {
        let want = (CAPE_SLOT_W * CAPE_SLOT_H * 4) as usize;
        if rgba.len() != want {
            return Err(format!(
                "cape must be {CAPE_SLOT_W}x{CAPE_SLOT_H} RGBA ({want} bytes), got {}",
                rgba.len()
            ));
        }
        let slot = self.cape_next % CAPE_SLOTS;
        self.cape_next += 1;
        let (sx, sy) = cape_slot_origin(slot);
        upload_region(gpu, self.image, rgba, sx, sy, CAPE_SLOT_W, CAPE_SLOT_H)?;
        Ok((sx, sy))
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
        world_text: &[WorldTextDraw<'_>],
        cam_right: [f32; 3],
        cam_up: [f32; 3],
        time: f32,
        cam_pos: [f32; 3],
    ) {
        self.cam_pos = cam_pos;
        self.cursor = (self.cursor + 1) % RING;
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
        let glint_offsets = crate::gui_item::glint_offsets(
            (time as f64) * 1000.0,
            crate::gui_item::GLINT_SPEED,
        );

        // Advance the animation clock. `frame_time` drives FA's integrators and
        // `frame_counter` its same-frame guard, so both must be real. `time` is
        // wall-clock seconds; a still render repeats the same value, hence the
        // fallback tick.
        let dt = if self.prev_time.is_finite() { time - self.prev_time } else { 0.0 };
        self.prev_time = time;
        self.frame_dt = if dt > 0.0 { dt.min(0.25) } else { 1.0 / 20.0 };
        self.frame_counter += 1.0;
        self.generation = self.generation.wrapping_add(1);
        // Taken out of `self` so `emit_model` can borrow it mutably alongside
        // the immutable model borrow; put back below.
        let mut cem_state = std::mem::take(&mut self.cem_state);

        // Fixed sun for capsule shading (matches the terrain's lit look).
        let sun = norm3([0.45, 0.8, 0.35]);
        for d in draws {
            // `OverlayTexture.v(hasRedOverlay)` as a 0/1 flag (M21).
            let hurt = if d.hurt { 1.0f32 } else { 0.0 };
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
                self.emit_model(
                    &mut verts,
                    &mut trim_verts,
                    &mut emissive_verts,
                    d,
                    model,
                    time,
                    &mut cem_state,
                    Some(&mut sink),
                    &mut armor_sink,
                );
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
            for (p, n) in &self.capsule {
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
                    color: [base[0] * shade, base[1] * shade, base[2] * shade, 1.0],
                    light_hurt: [light_r, light_g, light_b, hurt],
                });
            }
        }
        // M25b: block entities. Solid geometry, so they belong with the mob
        // and item quads rather than after the transparent nametags.
        self.emit_block_entities(&mut verts, block_entities);
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
                let Some(name) = d.name else { continue };
                self.push_tag(&mut verts, d, name, cam_right, cam_up);
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
        self.solid_verts = solid as u32;
        self.text_verts = (text_end - solid) as u32;
        self.glint_verts = (glint_end - text_end) as u32;
        self.trim_verts = (trim_end - glint_end) as u32;
        self.armor_glint_verts = (armor_glint_end - trim_end) as u32;
        self.emissive_verts = (verts.len() - armor_glint_end) as u32;
        let total = verts.len();
        if let Some(slice) = self.allocs[self.cursor]
            .as_mut()
            .and_then(|a| a.mapped_slice_mut())
        {
            // Clamp to the buffer. The per-emitter `MAX_VERTS` guards bound
            // each range on its own, but five ranges are appended after them,
            // so their sum can exceed the allocation — and the copy below would
            // panic rather than degrade. Whole vertices only, so a truncated
            // frame drops geometry instead of corrupting it.
            let total = total.min(MAX_VERTS);
            let bytes: &[u8] = unsafe {
                std::slice::from_raw_parts(verts.as_ptr() as *const u8, total * VERTEX_STRIDE as usize)
            };
            slice[..bytes.len()].copy_from_slice(bytes);
        }
    }

    /// Draw one mob model: per-part vanilla `setupAnim` rotation about the
    /// part's pivot in model space, then vanilla's entity transform —
    /// `rotY(180° − yaw) · scale(−1,−1,1) · translate(0,−1.501,0)` — scaled
    /// px→blocks and placed at the entity's feet. Texels ride the
    /// alpha-test (`discard`) path.

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
        scale: f32,
        st: f32,
        ct: f32,
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
                let e = [-v[0], mobs::MODEL_EYE_Y - v[1], v[2]];
                // The same death topple the body takes — `setupRotations`
                // rotates the entity, and `ItemInHandLayer` hangs off the arm
                // *inside* that rotation, so a corpse's sword falls with it.
                let (sr, cr) = death_roll(d).sin_cos();
                let e = [e[0] * cr - e[1] * sr, e[0] * sr + e[1] * cr, e[2]];
                let x = e[0] * ct + e[2] * st;
                let z = -e[0] * st + e[2] * ct;
                p4[i] = [
                    d.pos[0] + x * scale,
                    d.pos[1] + e[1] * scale,
                    d.pos[2] + z * scale,
                ];
            }
            let n = face_normal(&m4);
            let shade = mobs::shade_for(n);
            for &i in &[0usize, 1, 2, 0, 2, 3] {
                verts.push(Vertex {
                    pos: p4[i],
                    uv: [u0 + q.uv[i][0] * du, v0 + q.uv[i][1] * dv],
                    color: [shade, shade, shade, 1.0],
                    light_hurt: [light_r, light_g, light_b, hurt],
                });
                // Same position, same moment — see [`GlintSink`].
                if let Some(g) = glint.as_deref_mut() {
                    g.push(p4[i], q.uv[i]);
                }
            }
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
        if let Some(w) = cape.wavy.filter(|w| w.segments() >= 2) {
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
                let e = [-v[0], mobs::MODEL_EYE_Y - v[1], v[2]];
                let e = [e[0] * cr - e[1] * sr, e[0] * sr + e[1] * cr, e[2]];
                let x = e[0] * ct + e[2] * st;
                let z = -e[0] * st + e[2] * ct;
                let mut l = [x * scale, e[1] * scale, z * scale];
                if let Some(mm) = &d.mount {
                    l = [
                        mm[0][0] * l[0] + mm[0][1] * l[1] + mm[0][2] * l[2] + mm[0][3],
                        mm[1][0] * l[0] + mm[1][1] * l[1] + mm[1][2] * l[2] + mm[1][3],
                        mm[2][0] * l[0] + mm[2][1] * l[1] + mm[2][2] * l[2] + mm[2][3],
                    ];
                }
                p4[i] = [d.pos[0] + l[0], d.pos[1] + l[1], d.pos[2] + l[2]];
            }
            let n = face_normal(&p4);
            let shade = mobs::shade_for(n);
            let uv4 = cape_face_uv(cape.origin, &uvs);
            for &i in &[0usize, 1, 2, 0, 2, 3] {
                verts.push(Vertex {
                    pos: p4[i],
                    uv: uv4[i],
                    color: [shade, shade, shade, 1.0],
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
            let shade = mobs::shade_for(fnorm);
            let uv4 = cape_face_uv(cape.origin, &q.uv);
            for &i in &[0usize, 1, 2, 0, 2, 3] {
                verts.push(Vertex {
                    pos: p4[i],
                    uv: uv4[i],
                    color: [shade, shade, shade, 1.0],
                    // As the rigid path: `CapeLayer` submits with
                    // `NO_OVERLAY`, so a hurt wearer's cape does not flash.
                    light_hurt: [light_r, light_g, light_b, 0.0],
                });
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
        scale: f32,
        st: f32,
        ct: f32,
        sr: f32,
        cr: f32,
    ) {
        if !mobs::wears_humanoid_armor(d.kind) {
            return;
        }
        let [light_r, light_g, light_b] = d.light;
        let hurt = if d.hurt { 1.0f32 } else { 0.0 };
        for (slot, piece) in [
            (mobs::ArmorSlot::Chest, d.armor[1]),
            (mobs::ArmorSlot::Legs, d.armor[2]),
            (mobs::ArmorSlot::Feet, d.armor[3]),
            (mobs::ArmorSlot::Head, d.armor[0]),
        ] {
            let Some(piece) = piece else { continue };
            // In list order: `renderLayers` walks the layers in order, and an
            // overlay has to land on top of the base it covers. The trim is
            // appended as a further "layer" with its own origin and no tint —
            // `submitModel(..., -1, sprite, ...)` passes white, because a
            // trim's colour is baked into its palette-permuted sprite.
            let dyed = piece.layers.iter().flatten().map(|(k, t)| (Some(*k), None, *t));
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
            let verts: &mut Vec<Vertex> = if trim_origin.is_some() { trim_verts } else { verts };
            for b in mobs::armor_boxes(slot) {
                let Some(pi) = mobs::armor_part(&model.parts, b.part) else {
                    continue;
                };
                let (m, o) = &xf[pi];
                // The sheet is 64x32 in the classic armour layout, packed into
                // the shared atlas at `(ax, ay)`.
                for (_facing, pos, uvs) in
                    mobs::cube_faces(b.uv, b.min, b.dims, slot.grow() + b.extend, b.mirror)
                {
                    if verts.len() + 6 > MAX_VERTS {
                        return;
                    }
                    let mut p4 = [[0f32; 3]; 4];
                    for (i, corner) in pos.iter().enumerate() {
                        let r = mat_apply(m, *corner);
                        let v = [r[0] + o[0], r[1] + o[1], r[2] + o[2]];
                        let e = [-v[0], mobs::MODEL_EYE_Y - v[1], v[2]];
                        let e = [e[0] * cr - e[1] * sr, e[0] * sr + e[1] * cr, e[2]];
                        let x = e[0] * ct + e[2] * st;
                        let z = -e[0] * st + e[2] * ct;
                        let mut l = [x * scale, e[1] * scale, z * scale];
                        if let Some(mm) = &d.mount {
                            l = [
                                mm[0][0] * l[0] + mm[0][1] * l[1] + mm[0][2] * l[2] + mm[0][3],
                                mm[1][0] * l[0] + mm[1][1] * l[1] + mm[1][2] * l[2] + mm[1][3],
                                mm[2][0] * l[0] + mm[2][1] * l[1] + mm[2][2] * l[2] + mm[2][3],
                            ];
                        }
                        p4[i] = [d.pos[0] + l[0], d.pos[1] + l[1], d.pos[2] + l[2]];
                    }
                    let n = face_normal(&p4);
                    let shade = mobs::shade_for(n);
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
                            color: [shade * tint[0], shade * tint[1], shade * tint[2], 1.0],
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
            let frame = crate::cem::eval_program(
                prog,
                &mut actx,
                model.parts.len(),
                &model.cem_translate,
            );
            // Hand the (now-advanced) slots back for the next frame.
            cem_state
                .entry(cem_key(d))
                .or_default()
                .slots = std::mem::take(&mut actx.user);
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
        let xf = part_transforms(model, &ctx, cem_deltas, cem_scale);
        // Per-entity render scale (slime/magma size) on top of the baked px→
        // block scale — vanilla scales the whole model uniformly by `size`.
        let s = model.scale * if d.scale_mul > 0.0 { d.scale_mul } else { 1.0 };
        // Model-space quad corners -> world space, through this entity's pose.
        // Shared by the base pass and the emissive layers so a layer can never
        // drift off the model it is glowing on (M57).
        let place = |q: &GpuQuad| {
            let (m, o) = &xf[q.part as usize];
            let mut p4 = [[0f32; 3]; 4];
            for (i, corner) in q.pos.iter().enumerate() {
                let r = mat_apply(m, *corner);
                let v = [r[0] + o[0], r[1] + o[1], r[2] + o[2]];
                // model → entity local (px): scale(−1,−1,1) after the
                // −1.501-block translate.
                let e = [-v[0], mobs::MODEL_EYE_Y - v[1], v[2]];
                // rotZ(fall · flipDegrees) — the death topple, identity while
                // alive because `death_roll` is then exactly 0.
                let e = [e[0] * cr - e[1] * sr, e[0] * sr + e[1] * cr, e[2]];
                // rotY(180° − yaw).
                let x = e[0] * ct + e[2] * st;
                let z = -e[0] * st + e[2] * ct;
                // Feet-relative, in block units. A *mounted* entity — a
                // spawner's caged mob — runs that through one more affine
                // before the block position places it (M31).
                let mut l = [x * s, e[1] * s, z * s];
                if let Some(m) = &d.mount {
                    l = [
                        m[0][0] * l[0] + m[0][1] * l[1] + m[0][2] * l[2] + m[0][3],
                        m[1][0] * l[0] + m[1][1] * l[1] + m[1][2] * l[2] + m[1][3],
                        m[2][0] * l[0] + m[2][1] * l[1] + m[2][2] * l[2] + m[2][3],
                    ];
                }
                p4[i] = [d.pos[0] + l[0], d.pos[1] + l[1], d.pos[2] + l[2]];
            }
            p4
        };
        // Per-part visibility (CEM bone hides + the vanilla shell / gesture /
        // illager swaps). Vanilla's emissive layers run the *same* `setupAnim`
        // on a copy of the model, so they obey exactly the same rules.
        let visible = |part: usize| {
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
            }
        };
        // The pack variant this entity drew, if the atlas has a slot for it.
        let variant_uv = (d.variant != 0)
            .then(|| model.variants.get(&d.variant))
            .flatten()
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
        let hurt = if d.hurt { 1.0f32 } else { 0.0 };
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
        // M68: per-texture-slot layer colour, linearized once. Vanilla tints a
        // *layer*; Rewo bakes a layer as a texture slot, so one entry per
        // slot is the same statement. `1.0` is untinted, and `shade * 1.0` is
        // exact in IEEE — every mob that has no tinted layer renders the
        // bytes it did before this existed.
        let mut slot_tint = [[1.0f32; 3]; MAX_MOB_TEXTURES];
        let lin3 = |rgb: [u8; 3]| {
            [
                srgb_to_linear(rgb[0] as f32 / 255.0),
                srgb_to_linear(rgb[1] as f32 / 255.0),
                srgb_to_linear(rgb[2] as f32 / 255.0),
            ]
        };
        // `SheepWoolLayer` and `SheepWoolUndercoatLayer` both pass
        // `state.getWoolColor()`, so the two slots take the *same* colour —
        // that is why the undercoat needs no dye field of its own.
        let wool = mobs::SHEEP_WOOL_COLORS[(d.dye.unwrap_or(0) & 15) as usize];
        for s in [model.tinted_slot, model.coplanar_slot].into_iter().flatten() {
            if let Some(t) = slot_tint.get_mut(s as usize) {
                *t = lin3(wool);
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
        for q in &model.quads {
            if verts.len() + 6 > MAX_VERTS {
                return;
            }
            if !visible(q.part as usize) {
                continue;
            }
            if shorn_slot == Some(q.tex) {
                continue;
            }
            // The coplanar layer leaves the solid range: at the base model's
            // exact depth the solid pass's strict `GREATER` rejects every one
            // of its fragments. `trim_verts` is `CompareOp::EQUAL` with no
            // depth write, drawn after the solid range — the reversed-Z
            // reading of vanilla's `entityCutout` for coplanar geometry.
            let coplanar = coplanar_slot == Some(q.tex);
            if coplanar && !d.undercoat {
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
            let c = [q.shade * t[0], q.shade * t[1], q.shade * t[2]];
            // Two things can move a quad's UVs onto a different texture of the
            // same size: a player's uploaded skin, and a pack's ETF variant.
            // They never apply to the same mob (skins are the player model's),
            // so the variant wins where it exists.
            let du = variant_uv.map_or(skin_du, |v| v[q.tex as usize]);
            let out: &mut Vec<Vertex> = if coplanar { trim_verts } else { verts };
            for &i in &[0usize, 1, 2, 0, 2, 3] {
                out.push(Vertex {
                    pos: p4[i],
                    uv: [q.uv[i][0] + du[0], q.uv[i][1] + du[1]],
                    color: [c[0], c[1], c[2], 1.0],
                    light_hurt: [light_r, light_g, light_b, hurt],
                });
            }
        }
        // M46: worn armour, a render layer over the body, before the held
        // item so a chestplate does not paint over a sword.
        self.emit_armor(verts, trim_verts, armor_glint, d, model, &xf, s, st, ct, sr, cr);
        // M60: the cape, hanging off the same body. `AvatarRenderer` adds
        // `HumanoidArmorLayer` first and `CapeLayer` eleven layers later, so
        // this is vanilla's order — though nothing rests on it here, since
        // both are opaque `entitySolid` draws in one range and the depth test
        // settles them. What keeps the cape off a chestplate is the clearance
        // shift, not the sequence.
        self.emit_cape(verts, d, model, &xf, s, st, ct, sr, cr);
        // M22: whatever each arm holds, drawn after the body —
        // `ItemInHandLayer` is a render layer, so it sits on top of the
        // model it hangs off.
        for (i, left) in [(0usize, false), (1usize, true)] {
            if let Some(name) = d.held[i] {
                let hurt = if d.hurt { 1.0f32 } else { 0.0 };
                self.emit_held_item(
                    verts,
                    d,
                    model,
                    &xf,
                    left,
                    name,
                    s,
                    st,
                    ct,
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
                let c = if shaded { q.shade } else { 1.0 };
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
    }

    /// Glyph + background quads for one nametag, camera-billboarded.
    fn push_tag(
        &self,
        verts: &mut Vec<Vertex>,
        d: &EntityDraw<'_>,
        name: &str,
        right: [f32; 3],
        up: [f32; 3],
    ) {
        let cell = self.cell as f32;
        let scale = TAG_PX * (8.0 / cell);
        let total_px: f32 = name
            .bytes()
            .map(|b| self.advance[b as usize] as f32)
            .sum();
        let anchor = [
            d.pos[0],
            d.pos[1] + d.height + TAG_LIFT,
            d.pos[2],
        ];
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
    unsafe fn bind_common(&self, gpu: &Gpu, cb: vk::CommandBuffer, view_proj: [[f32; 4]; 4], extent: vk::Extent2D) {
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
        device.cmd_bind_vertex_buffers(cb, 0, &[self.bufs[self.cursor]], &[0]);
    }

    /// Opaque capsules — draw before any translucent content.
    pub fn draw_solid(&self, gpu: &Gpu, cb: vk::CommandBuffer, view_proj: [[f32; 4]; 4], extent: vk::Extent2D) {
        if self.solid_verts == 0 {
            return;
        }
        unsafe {
            self.bind_common(gpu, cb, view_proj, extent);
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
            self.bind_common(gpu, cb, view_proj, extent);
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
        let device = gpu.device.clone();
        let (image, image_alloc, view) = create_glint_texture(gpu, rgba, w, h)?;
        let sampler = unsafe {
            device
                .create_sampler(
                    &vk::SamplerCreateInfo::default()
                        .mag_filter(vk::Filter::LINEAR)
                        .min_filter(vk::Filter::LINEAR)
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
        if self.glint_pipeline.is_none() {
            self.glint_pipeline = Some(build_glint_pipeline(&device, self.layout, color_format)?);
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
            self.bind_common(gpu, cb, view_proj, extent);
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
            self.bind_common(gpu, cb, view_proj, extent);
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
    pub fn draw_text(&self, gpu: &Gpu, cb: vk::CommandBuffer, view_proj: [[f32; 4]; 4], extent: vk::Extent2D) {
        if self.text_verts == 0 {
            return;
        }
        unsafe {
            self.bind_common(gpu, cb, view_proj, extent);
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
            for sheet in [self.glint.take(), self.armor_glint.take()] {
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
            for b in self.bufs {
                device.destroy_buffer(b, None);
            }
        }
        for a in self.allocs.iter_mut().filter_map(|a| a.take()) {
            let _ = gpu.allocator.free(a);
        }
        if let Some(a) = self.image_alloc.take() {
            let _ = gpu.allocator.free(a);
        }
    }
}

/// Unit capsule triangle soup: y ∈ [0, 1], xz ∈ [−0.5, 0.5]. Profile =
/// quarter-sphere cap (3 bands) + cylinder + cap, swept around Y.
fn unit_capsule() -> Vec<([f32; 3], [f32; 3])> {
    // (radius, y, n_radial, n_y) per profile row.
    let mut profile: Vec<(f32, f32, f32, f32)> = Vec::new();
    for i in 0..=3 {
        let th = (-90.0 + 30.0 * i as f32).to_radians();
        profile.push((
            0.5 * th.cos(),
            0.25 * (1.0 + th.sin()),
            th.cos(),
            th.sin(),
        ));
    }
    profile.push((0.5, 0.75, 1.0, 0.0));
    for i in 1..=3 {
        let th = (30.0 * i as f32).to_radians();
        profile.push((
            0.5 * th.cos(),
            0.75 + 0.25 * th.sin(),
            th.cos(),
            th.sin(),
        ));
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
    use mobs::Anim::*;
    use std::f32::consts::PI;
    let mut rot = [0.0f32; 3];
    let mut off = [0.0f32; 3];
    match anim {
        None => {}
        Head => {
            rot[0] = c.pitch;
            rot[1] = c.net;
        }
        ArmRight => rot[0] = (c.f + PI).cos() * 2.0 * c.amt * 0.5 * amp,
        ArmLeft => rot[0] = c.f.cos() * 2.0 * c.amt * 0.5 * amp,
        LegRight | QuadHindRight | QuadFrontLeft => rot[0] = c.f.cos() * 1.4 * c.amt * amp,
        LegLeft | QuadHindLeft | QuadFrontRight => rot[0] = (c.f + PI).cos() * 1.4 * c.amt * amp,
        SpiderLeg { left, phase } => {
            let swing = -((c.f * 2.0 + phase).cos()) * 0.4 * c.amt;
            let step = (c.f + phase).sin().abs() * 0.4 * c.amt;
            let dir = if left { -1.0 } else { 1.0 };
            rot[1] = dir * swing;
            rot[2] = dir * step;
        }
        TailWagY => rot[1] = c.f.cos() * 1.4 * c.amt,
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
            rot[1] = if left { PI / 4.0 - flap } else { -PI / 4.0 + flap };
        }
        AllayRoot => {
            // AllayModel `state.isDancing` branch, `this.root`. Idle → 0 (the
            // model stays upright; the else-branch never touches root).
            if let Some(dance) = c.allay_dance {
                // danceSpeed = ageInTicks·8° + animationSpeed (walkAnimationSpeed).
                let dance_speed = c.age * 8.0 * DEG + c.amt;
                let spin = dance.spinning_progress;
                // root.yRot = isSpinning ? 4π·spin : 0 (unchanged reset value).
                rot[1] = if dance.is_spinning { PI * 4.0 * spin } else { 0.0 };
                // root.zRot = danceFrequency·(1 − spin), danceFreq = cos(dS)·16°.
                rot[2] = dance_speed.cos() * 16.0 * DEG * (1.0 - spin);
            }
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
            rot[2] = if left { -(PI / 5.0 + bob) } else { PI / 5.0 + bob };
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
        FishTail { amp: a, speed } => rot[1] = -a * (speed * c.age).sin(),
        PufferFin { left } => {
            let s = (c.age * 0.2).sin();
            rot[2] = if left { 0.2 - 0.4 * s } else { -0.2 + 0.4 * s };
        }
        DolphinTail { k } => {
            let gate = (c.amt * 6.0).min(1.0);
            rot[0] = -k * (c.age * 0.3).cos() * gate;
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
        // `SkeletonModel.setupAnim` = `super.setupAnim(state)` then its own
        // attack override, gated on aggressive && !holdingBow.
        SkeletonArmRight | SkeletonArmLeft => {
            let left = anim == SkeletonArmLeft;
            humanoid_arm(&mut rot, &mut off, left, c, amp);
            skeleton_attack_arm(&mut rot, left, c);
        }
        // `IllagerModel.setupAnim` assigns its own walk over everything
        // `super.setupAnim` left, then runs the arm-pose switch.
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
            rot[0] = phase.cos() * 2.0 * c.amt * 0.5 * amp;
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
fn animate_attack_arms(rot: &mut [f32; 3], left: bool, attack_time: f32, negate: bool, arm_drop: f32) {
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

/// `IllagerModel.setupAnim`'s arm stage.
///
/// Vanilla calls `super.setupAnim` first, then **assigns** its own walk over
/// both arms — which wipes the humanoid hold pose, the attack and the idle bob
/// outright — and only then runs the `IllagerArmPose` switch. So the humanoid
/// stage is not run here at all: everything it would contribute is overwritten
/// before it could be seen.
fn illager_arm(rot: &mut [f32; 3], off: &mut [f32; 3], left: bool, c: &AnimCtx) {
    use std::f32::consts::PI;
    use mobs::IllagerArmPose as P;
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
            rot[2] = if left { -PI * 3.0 / 4.0 } else { PI * 3.0 / 4.0 };
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
fn apply_arm_pose(
    rot: &mut [f32; 3],
    pose: mobs::ArmPose,
    pose_left: bool,
    target_left: bool,
    c: &AnimCtx,
) {
    apply_pose_effect(rot, pose, pose_left, target_left, c)
}

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
            rot[0] = rot[0] * 0.5 - 0.942_477_9
                + c.pitch.clamp(-(PI * 4.0 / 9.0), 0.436_332_32);
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
        // The crouch term is inert: Rewo does not model `isCrouching` (it is
        // declared unmodelled alongside swim and fall-flying), so the flag is
        // false and the subtraction is a no-op — the same "exactly vanilla for
        // the states we reach" argument the spear pose already makes.
        mobs::ArmPose::Spyglass => {
            rot[0] = (c.pitch - 1.919_862_2).clamp(-2.4, 3.3);
            rot[1] = c.net + if right_handed_pose { -PI / 12.0 } else { PI / 12.0 };
        }
        // ```text
        // arm.xRot = Mth.clamp(head.xRot, -1.2F, 1.2F) - 1.4835298F;
        // arm.yRot = head.yRot -/+ PI/6;
        // ```
        mobs::ArmPose::TootHorn => {
            rot[0] = c.pitch.clamp(-1.2, 1.2) - 1.483_529_8;
            rot[1] = c.net + if right_handed_pose { -PI / 6.0 } else { PI / 6.0 };
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
        self.seed = self
            .seed
            .wrapping_mul(Self::MULTIPLIER)
            .wrapping_add(11)
            & Self::MASK;
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
}

/// How many animated part groups one block-entity model may carry.
///
/// Group 0 is "as baked" and never indexes the arrays, so this is the number
/// of *animated* groups plus that one. Four covers the deepest model here — a
/// piglin head, whose two ears animate to different formulas at once.
pub const MAX_PARTS: usize = 4;

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
    pub fn emit_world_text(&self, verts: &mut Vec<Vertex>, draws: &[WorldTextDraw<'_>]) {
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
                    let [u0, v0, u1, v1] = self.glyph_uv(b);
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

    /// The width of a string in font pixels — `Font.width`, which is the sum
    /// of the per-glyph advances. Sign text centres on it.
    pub fn text_width(&self, text: &str) -> f32 {
        text.bytes().map(|b| self.advance[b as usize] as f32).sum()
    }

    /// The per-glyph advance table, for layout that happens before a draw.
    pub fn font_advance(&self) -> &[u8; 256] {
        &self.advance
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
    pub fn emit_block_entities(&self, verts: &mut Vec<Vertex>, draws: &[BlockEntityDraw<'_>]) {
        let Some(items) = self.held_items.as_ref() else {
            return;
        };
        for d in draws {
            let Some(model) = items.any(d.model) else {
                continue;
            };
            let m = &d.transform;
            let [light_r, light_g, light_b] = d.light;
            for q in &model.quads {
                if verts.len() + 6 > MAX_VERTS {
                    return;
                }
                let Some([u0, v0, du, dv]) = self.item_uv(q.tex) else {
                    continue;
                };
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
                let shade = mobs::shade_for(n);
                for &i in &[0usize, 1, 2, 0, 2, 3] {
                    verts.push(Vertex {
                        pos: p4[i],
                        uv: [u0 + q.uv[i][0] * du, v0 + q.uv[i][1] * dv],
                        color: [
                            shade * d.tint[0],
                            shade * d.tint[1],
                            shade * d.tint[2],
                            1.0,
                        ],
                        // A block entity is never hurt — the overlay flag is
                        // a `LivingEntity` property.
                        light_hurt: [light_r, light_g, light_b, 0.0],
                    });
                }
            }
        }
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
        let Some(item) = items.any(name) else {
            return;
        };
        let mut glint = glint;
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
                let shade = mobs::shade_for(n);
                for &i in &[0usize, 1, 2, 0, 2, 3] {
                    verts.push(Vertex {
                        pos: p4[i],
                        uv: [u0 + q.uv[i][0] * du, v0 + q.uv[i][1] * dv],
                        color: [shade, shade, shade, 1.0],
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
        WalkPlusAge { age_div, amt_add, speed_f, scale_f } => (
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
        allay_dance: inputs.allay_dance,
        attack: inputs.attack,
        arm_poses: inputs.arm_poses,
        mob: inputs.mob,
        tendril: inputs.tendril,
    };
    let (drots, doffs) = anim_deltas(&model.parts, &model.keyframes, &model.event_rigs, &ctx, None);
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
                (mat_mul(*pm, r), [gp[0] + po[0], gp[1] + po[1], gp[2] + po[2]])
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
    let vx = [
        [0.0, -v[2], v[1]],
        [v[2], 0.0, -v[0]],
        [-v[1], v[0], 0.0],
    ];
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
    *NOANIM.get_or_init(|| std::env::var("REWO_CEM_NOANIM").is_ok_and(|v| v == "1"))
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
    let (min_u, max_u) = q
        .uv
        .iter()
        .fold((f32::MAX, f32::MIN), |(lo, hi), p| (lo.min(p[0]), hi.max(p[0])));
    let (min_v, max_v) = q
        .uv
        .iter()
        .fold((f32::MAX, f32::MIN), |(lo, hi), p| (lo.min(p[1]), hi.max(p[1])));
    let color = q.facing.debug_color();
    let [r, g, b] = color;
    let (x0, x1) = ((min_u.floor().max(0.0)) as u32, (max_u.ceil().max(0.0) as u32).min(tw));
    let (y0, y1) = ((min_v.floor().max(0.0)) as u32, (max_v.ceil().max(0.0) as u32).min(th));
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

/// Blit a `w`×`h` RGBA entity skin into the atlas at (x, y). Returns whether
/// it was present + correctly sized.
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
/// → SHADER_READ_ONLY, fence-waited. `wait_idle` first so no in-flight
/// frame samples the atlas mid-write (rare call — see `upload_skin`).
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
        return Err(format!("upload_region: {} bytes, want {expect}", rgba.len()));
    }
    gpu.wait_idle();
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
        let to_dst = vk::ImageMemoryBarrier2::default()
            .src_stage_mask(vk::PipelineStageFlags2::FRAGMENT_SHADER)
            .src_access_mask(vk::AccessFlags2::SHADER_SAMPLED_READ)
            .dst_stage_mask(vk::PipelineStageFlags2::TRANSFER)
            .dst_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
            .old_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
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
                .image_offset(vk::Offset3D { x: x as i32, y: y as i32, z: 0 })
                .image_extent(vk::Extent3D { width, height, depth: 1 })],
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
        device.end_command_buffer(cb).map_err(|e| format!("skin end: {e}"))?;
        let fence = device
            .create_fence(&vk::FenceCreateInfo::default(), None)
            .map_err(|e| format!("skin fence: {e}"))?;
        let cbs = [vk::CommandBufferSubmitInfo::default().command_buffer(cb)];
        device
            .queue_submit2(
                gpu.graphics_queue,
                &[vk::SubmitInfo2::default().command_buffer_infos(&cbs)],
                fence,
            )
            .map_err(|e| format!("skin submit: {e}"))?;
        device
            .wait_for_fences(&[fence], true, u64::MAX)
            .map_err(|e| format!("skin wait: {e}"))?;
        device.destroy_fence(fence, None);
        device.destroy_command_pool(pool, None);
        device.destroy_buffer(staging, None);
        let _ = gpu.allocator.free(salloc);
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

/// Single-channel coverage texture — the Velvet glyph atlas (M52b).
///
/// `create_texture_fmt` sizes its staging buffer from `data.len()` rather than
/// assuming four bytes per pixel, so it is already format-agnostic. UNORM, not
/// SRGB: this is a mask, and pushing coverage through an sRGB decode would
/// make every glyph edge wrong.
pub(crate) fn create_texture_r8(
    gpu: &mut Gpu,
    data: &[u8],
    width: u32,
    height: u32,
) -> Result<(vk::Image, Allocation, vk::ImageView), String> {
    create_texture_fmt(gpu, data, width, height, vk::Format::R8_UNORM)
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

fn create_texture_fmt(
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
        salloc
            .mapped_slice_mut()
            .ok_or("font staging not mapped")?[..rgba.len()]
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
    unsafe {
        let vert = crate::overlay::create_shader(
            device,
            include_bytes!(concat!(env!("OUT_DIR"), "/entity.vert.spv")),
        )?;
        let frag = crate::overlay::create_shader(
            device,
            include_bytes!(concat!(env!("OUT_DIR"), "/entity_glint.frag.spv")),
        )?;
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
        let depth = vk::PipelineDepthStencilStateCreateInfo::default()
            .depth_test_enable(true)
            .depth_write_enable(false)
            .depth_compare_op(vk::CompareOp::EQUAL);
        let blend_attachments = [vk::PipelineColorBlendAttachmentState::default()
            .blend_enable(true)
            .src_color_blend_factor(vk::BlendFactor::SRC_COLOR)
            .dst_color_blend_factor(vk::BlendFactor::ONE)
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
fn build_pipeline(
    device: &ash::Device,
    layout: vk::PipelineLayout,
    color_format: vk::Format,
    solid: bool,
    compare: vk::CompareOp,
) -> Result<vk::Pipeline, String> {
    unsafe {
        let vert = crate::overlay::create_shader(
            device,
            include_bytes!(concat!(env!("OUT_DIR"), "/entity.vert.spv")),
        )?;
        let frag = crate::overlay::create_shader(
            device,
            include_bytes!(concat!(env!("OUT_DIR"), "/entity.frag.spv")),
        )?;
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
            .depth_write_enable(solid)
            .depth_compare_op(compare);
        let blend_attachments = [vk::PipelineColorBlendAttachmentState::default()
            .blend_enable(!solid)
            .src_color_blend_factor(vk::BlendFactor::SRC_ALPHA)
            .dst_color_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
            .color_blend_op(vk::BlendOp::ADD)
            .src_alpha_blend_factor(vk::BlendFactor::ONE)
            .dst_alpha_blend_factor(vk::BlendFactor::ZERO)
            .alpha_blend_op(vk::BlendOp::ADD)
            .color_write_mask(
                vk::ColorComponentFlags::R
                    | vk::ColorComponentFlags::G
                    | vk::ColorComponentFlags::B,
            )];
        let blend = vk::PipelineColorBlendStateCreateInfo::default().attachments(&blend_attachments);
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
    use super::*;

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
        let body_quads = m.quads.iter().filter(|q| m.parts[q.part].name == "body").count();
        assert!(body_quads > 0);
        // Layer 0 = bioluminescent (head + limbs, no body).
        assert!(
            layers[0].quads.iter().all(|q| m.parts[q.part as usize].name != "body"),
            "the bioluminescent layer must not include the body"
        );
        // Layer 3 = tendrils. Each tendril is a 16x16x0 plate, so only its two
        // zero-depth faces survive `cube_faces`' area filter.
        assert_eq!(layers[3].quads.len(), 4, "two tendril plates, two faces each");
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
        assert_eq!(layers.len(), 4, "the mismatched layer is dropped, the rest stay");
        // And a missing one, likewise.
        let mut slots = warden_slots();
        slots.remove("warden_pulsating_2");
        assert_eq!(build_emissive(&m, def, &slots, &no_pack_emissive()).len(), 4);
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
        assert_eq!(def.textures, &["sheep", "sheep_wool", "sheep_wool_undercoat"]);
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
        assert_eq!(layers[0].quads.len(), wool_quads, "only the wool's quads glow");
        assert!(layers[0].cutout, "OptiFine emissive textures are alpha-cutout");
        // And it samples the overlay's slot at the wool's own texels.
        for (got, raw) in layers[0].quads.iter().zip(m.quads.iter().filter(|q| q.tex == 1)) {
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
        assert!((heart(40.0) - 1.0).abs() < 1e-6, "the beat repeats every 40 ticks");
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
            allay_dance: None,
            attack: mobs::SwingPose::NONE,
            arm_poses: mobs::ArmPoses::EMPTY,
            mob: mobs::MobCombat::default(),
            tendril,
        };
        // At rest the tendrils do not move, whatever the age.
        for age in [0.0, 3.0, 17.5] {
            let (r, _) = anim_delta(mobs::Anim::WardenTendril { left: true }, 1.0, &ctx(0.0, age));
            assert_eq!(r, [0.0; 3]);
        }
        // Full countdown at age 0: cos(0) = 1 -> +/-pi*0.1, left positive.
        let peak = std::f32::consts::PI * 0.1;
        let (l, _) = anim_delta(mobs::Anim::WardenTendril { left: true }, 1.0, &ctx(1.0, 0.0));
        let (r, _) = anim_delta(mobs::Anim::WardenTendril { left: false }, 1.0, &ctx(1.0, 0.0));
        assert!((l[0] - peak).abs() < 1e-6 && (r[0] + peak).abs() < 1e-6);
        assert_eq!([l[1], l[2]], [0.0, 0.0], "tendrils rotate about X only");
        // Half a countdown scales the same angle.
        let (h, _) = anim_delta(mobs::Anim::WardenTendril { left: true }, 1.0, &ctx(0.5, 0.0));
        assert!((h[0] - peak * 0.5).abs() < 1e-6);
        // cos(age*2.25) = 0 at age = (pi/2)/2.25 — the still point the emissive
        // gate renders at.
        let (z, _) = anim_delta(
            mobs::Anim::WardenTendril { left: true },
            1.0,
            &ctx(1.0, std::f32::consts::FRAC_PI_2 / 2.25),
        );
        assert!(z[0].abs() < 1e-6, "the sway must vanish at its zero crossing");
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
        let xs: Vec<f32> = quads.iter().flat_map(|q| q.pos.iter().map(|p| p[0])).collect();
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
