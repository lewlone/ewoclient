//! `rewo mobtexshot --check` — the **real-texture, multi-entity** mob gate.
//!
//! # Why this exists
//!
//! `REWO_PLAN.md` §0.0 has carried an open issue since M46: *"a mob can render
//! with another mob's texture when more than one is in the scene"*, with the
//! note that `mobshot --check` is **structurally blind** to it because it
//! substitutes per-face debug colours (`REWO_MOB_DEBUG_TEX`) and so grades
//! UV/face correspondence rather than *which sheet* was sampled.
//!
//! That is right and it is only half of the blindness. Every one of `mobshot`'s
//! five `--*check` modes renders **exactly one entity draw per frame**
//! (`mobshot_cmd.rs:341/344`, and `std::slice::from_ref(&d)` in the other four),
//! and *more than one entity in the scene* is the axis the reported symptom is
//! defined by. So the gate could not see the bug on either axis. This one
//! renders many entities in one `set_entities`, with the **real jar textures**,
//! and asks of every rendered pixel: *could this colour have come from this
//! mob's own sheet?*
//!
//! # The oracle, and why it is exact rather than fuzzy
//!
//! Four properties of the entity pass make the answer a closed form rather
//! than a similarity score, and every one was measured rather than assumed:
//!
//! * the entity atlas sampler is `NEAREST`/`NEAREST` with `mip_levels(1)`, so a
//!   pixel samples exactly one texel — there is no filtering blend to model;
//! * the solid range's pipeline is `blend_enable(!solid)`, i.e. **off**, so an
//!   entity pixel is written rather than mixed with the sky behind it;
//! * `entity.frag` is `out = texel * v_color.rgb * v_light_hurt.rgb` with no
//!   fog term at all, and this gate draws at `light = [1,1,1]`, so the light
//!   factor is the identity;
//! * `v_color.rgb` is `q.shade * slot_tint`, and `mobs::shade_for` returns one
//!   of **five** values — re-declared as [`SHADES`] here rather than imported,
//!   per M93r: a witness that reads the constant the renderer reads grades
//!   everything about the draw except the constant.
//!
//! So for a neutral draw the whole render is
//! `pixel = srgb_encode(srgb_decode(texel) * shade * tint)`, and the set of
//! byte triples a given sheet can produce is finite and computable on the CPU
//! from the jar's own decoded PNG bytes. A pixel outside that set did not come
//! from that sheet. The one slack allowed is ±1 per channel, because the GPU's
//! encode-on-store and this CPU one need not agree in the last bit.
//!
//! # Attribution without geometry
//!
//! Which pixels belong to which of the N mobs in a frame is answered by
//! **leave-one-out**: render the frame, then render it again with mob *i*
//! removed, and take the pixels that differ. Those are exactly the pixels mob
//! *i* covered — including the ones where it occluded a neighbour — and it
//! needs no projection, no bounding box and no ray-cast, so nothing in the
//! attribution can drift from what the renderer actually drew.
//!
//! # What a green run does NOT assert
//!
//! It says a mob sampled *its own sheet*. It does not say it sampled the
//! *right texel* of that sheet — that is `mobshot --check`'s facelabel job, and
//! the two are complements. And it grades vanilla's **default** state only:
//! M175 closed the baby-sheet half of this gap (same-size swaps render via
//! generated offsets); no charging/suffocating/invulnerable/angry
//! sheet at all, which `m8` measures rather than hides.
//!
//! Two more limits, both structural rather than incidental:
//!
//! * **It grades atlas ADDRESSING, not the mob→sheet DECLARATION.** "This
//!   mob's own sheets" is read from `MobDef::textures` plus
//!   `mobs::emissive_layers` — the same two declarations the atlas packer and
//!   `emit_model` read — so a mob declared to use the *wrong* sheet is
//!   invisible here: the renderer and the oracle would agree and the gate
//!   would be green. That is sound for the bug this exists for (an addressing
//!   bug) and it is not the same claim as "the right sheet".
//! * **[`TINTS`] is an over-approximation.** All three neutral tints are
//!   applied to every sheet rather than only to the two kinds that can carry
//!   them, so each mob's acceptance set is about twice as wide as that mob can
//!   actually produce. It only ever widens, so it cannot manufacture a pass
//!   for a colour that is outside every sheet — but the module's "exact rather
//!   than fuzzy" claim is about the *shade/blend algebra*, not about this.
//!   `m6` measures how sharp the result still is (76 of 81 kinds are uniquely
//!   explained by their own sheet) and pins it so it cannot decay quietly.
//!
//! # The pools
//!
//! `m10` and `m11` are not about mob sheets at all: they grade the two
//! demand-filled atlas pools' **call sites**. `SlotRing`'s own unit tests
//! cover `claim`; the bug is a *caller* keeping a key that addresses a
//! recycled slot, and a helper's tests say nothing about that. Both witnesses
//! over-fill a pool by exactly one and require that the evicted key stops
//! resolving.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use clap::Args as ClapArgs;
use glam::{Mat4, Vec3};
use rewo_data::{assets, DataPaths};
use rewo_gpu::entities::EntityDraw;
use rewo_gpu::mobs::EntityModelKind;
use rewo_gpu::offscreen::Offscreen;
use rewo_gpu::overlay::OverlayDraw;
use rewo_gpu::world::WorldRenderer;
use rewo_gpu::Gpu;

use crate::stats::OverlayRing;

/// Declared witness count — the gate fails closed if the run does not produce
/// exactly this many, so adding one without bumping this turns it red rather
/// than silently shrinking the coverage.
const EXPECTED_WITNESSES: usize = 17;

const BG: [f32; 4] = [0.5, 0.5, 0.5, 1.0];

/// `rewo_gpu::mobs::shade_for`'s five outputs, **re-declared** here.
///
/// Importing the function would make this witness self-calibrating on the one
/// thing it multiplies by (M93r). These are the literals in `mobs.rs`; if that
/// table changes, this gate must be updated deliberately.
const SHADES: [f32; 5] = [1.0, 0.5, 0.80, 0.62, 0.68];

/// The per-slot tints a *neutral* draw can carry, sRGB, re-declared.
///
/// `emit_model` multiplies a tinted slot by `lin3(colour)`. With `dye: None`
/// the sheep's wool takes `SHEEP_WOOL_COLORS[0]` and with `fish_dye: None` the
/// tropical fish takes `DYE_DIFFUSE_COLORS[0]` — so a neutral frame has three
/// possible tints, not one, and a gate that assumed the identity would report
/// every sheep pixel as unexplained.
const TINTS: [[u8; 3]; 3] = [
    [255, 255, 255],    // untinted
    [230, 230, 230],    // SHEEP_WOOL_COLORS[0]
    [0xF9, 0xFF, 0xFE], // DYE_DIFFUSE_COLORS[0]
];

/// Below this many attributed pixels a kind is reported as too small to grade
/// rather than graded on noise.
const MIN_PIXELS: usize = 40;

/// `m12`'s spacing, in blocks — one zombie's own width.
///
/// §0.0's repro summons two zombies **at one spot**; two entities at one point
/// push apart to about their own width by the time they settle, and exactly
/// coincident draws would leave the hidden one with no attributed pixels at
/// all, which grades nothing. This is as close as a gradeable pile gets.
const PILE_SPACING: f32 = 0.6;

/// `*baby*.png` under `assets/minecraft/textures/entity/` in the pinned 26.2
/// client jar, counted recursively.
///
/// **Asserted, not printed.** `m8`'s first version tested `jar_babies > 0`
/// while its name — and three prose sites — claimed it pinned the size, so a
/// version bump that took the jar from 147 baby sheets to 12 would have left
/// it green. It could only rot in one direction. This is the number, and it
/// goes red in both.
const JAR_BABY_SHEETS: usize = 147;

/// The two demand-filled atlas pools' capacities and slot sizes,
/// **re-declared** from `entities.rs`'s private `TRIM_SLOTS` / `ITEM_SLOTS` /
/// `TRIM_SLOT_W` / `TRIM_SLOT_H` / `ITEM_SLOT` (M93r — a witness that reads the
/// constant the renderer reads grades everything about the pool except the
/// constant). `m10`/`m11` assert the *observed* pool size equals these, so a
/// capacity change turns the gate red rather than shrinking its coverage.
const TRIM_POOL: usize = 64;
const ITEM_POOL: usize = 1024;
const TRIM_W: u32 = 64;
const TRIM_H: u32 = 32;
const ITEM_PX: u32 = 16;

/// Floors the sweep must clear, so the gate cannot quietly become vacuous by
/// grading nothing or by grading only kinds it cannot tell apart.
///
/// `MIN_GRADED` was 60 against a measured 81, which tolerated **21 kinds
/// silently vanishing**: a UV or atlas break that lands a mob's quads on an
/// empty atlas region makes it draw fewer than [`MIN_PIXELS`] attributed
/// pixels, and that bucket is a printed `SKIP` rather than a failure. So the
/// floor is now stated the other way round — the two escape buckets are
/// **pinned at their measured sizes** and the sweep must account for every
/// kind — which turns "a mob stopped rendering" from a SKIP into a red gate.
const MIN_GRADED: usize = 78;
const MIN_DISCRIMINATING: usize = 70;
/// Kinds too small to grade at 1600x900, measured: fox, tadpole, cod, salmon,
/// tropical_fish x2, parrot, allay, copper_golem. A tenth means something
/// stopped drawing.
const MAX_SMALL: usize = 9;
/// Kinds whose whole render some *other* single sheet could also explain,
/// measured: skeleton/skeleton_horse, enderman/ender_dragon, ghast<->happy_ghast
/// (both directions) and piglin/piglin_brute — plus, since M175 baked the
/// same-size baby sheets, dolphin/dolphin_baby (the baby shares the adult's
/// grey palette outright). Sound but not discriminating, so the count is
/// pinned rather than left to drift.
const MAX_AMBIGUOUS: usize = 6;

#[derive(ClapArgs)]
pub struct MobtexshotArgs {
    #[arg(long, default_value = "26.2")]
    version: String,
    /// Run the gate. Nonzero exit on any failure.
    #[arg(long, default_value_t = false)]
    check: bool,
    /// Also dump the rendered frames here.
    #[arg(long)]
    out_dir: Option<PathBuf>,
    #[arg(long, default_value_t = false)]
    no_validation: bool,
}

pub fn run(args: MobtexshotArgs) -> Result<(), String> {
    if !args.check {
        return Err("mobtexshot: pass --check (this command is a gate)".into());
    }
    let jar = client_jar(&args.version).ok_or("client jar not found")?;
    let paths = DataPaths::for_version(&args.version).ok_or("no config dir")?;
    let baked = assets::bake(&jar, &paths.blocks_json())?;
    let mut gpu = Gpu::new(None, cfg!(debug_assertions) && !args.no_validation)?;
    run_check(&mut gpu, &baked, &jar, &args)
}

// ---------------------------------------------------------------------------
// The colour algebra
// ---------------------------------------------------------------------------

fn srgb_decode(b: u8) -> f32 {
    let c = b as f32 / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn srgb_encode(c: f32) -> u8 {
    let c = c.clamp(0.0, 1.0);
    let s = if c <= 0.0031308 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    };
    (s * 255.0 + 0.5).floor().clamp(0.0, 255.0) as u8
}

fn pack(r: u8, g: u8, b: u8) -> u32 {
    ((r as u32) << 16) | ((g as u32) << 8) | b as u32
}

/// Every sRGB byte triple one decoded sheet can put on screen.
///
/// Built from the **jar's own PNG bytes** (`baked.mob_textures`), never from
/// `MobModel.quads` — those UVs are the code under test.
fn producible(rgba: &[u8]) -> HashSet<u32> {
    // Distinct opaque texels first: a mob sheet has a few dozen colours over
    // thousands of texels, so this makes the product tiny.
    let mut texels: HashSet<u32> = HashSet::new();
    for px in rgba.chunks_exact(4) {
        // `entity.frag` discards `a < 0.004`. Byte alpha 1 is 0.00392, which
        // is *also* below that, so the shader drops it too and this keeps it.
        // The mismatch only ever WIDENS the acceptance set, so it cannot
        // manufacture a pass for a colour outside every sheet — but the
        // stated equivalence with "byte alpha 0" was wrong.
        if px[3] != 0 {
            texels.insert(pack(px[0], px[1], px[2]));
        }
    }
    let mut out = HashSet::with_capacity(texels.len() * SHADES.len() * TINTS.len());
    for t in texels {
        let lin = [
            srgb_decode((t >> 16) as u8),
            srgb_decode((t >> 8) as u8),
            srgb_decode(t as u8),
        ];
        for s in SHADES {
            for tint in TINTS {
                let f = [
                    srgb_decode(tint[0]),
                    srgb_decode(tint[1]),
                    srgb_decode(tint[2]),
                ];
                out.insert(pack(
                    srgb_encode(lin[0] * s * f[0]),
                    srgb_encode(lin[1] * s * f[1]),
                    srgb_encode(lin[2] * s * f[2]),
                ));
            }
        }
    }
    out
}

/// The same product, kept in **linear** light and carrying each texel's own
/// alpha — the space a Vulkan blend on an SRGB attachment works in, and the
/// second factor its blend weight needs.
///
/// `entity.frag` ends `a = t.a * v_color.a`, so an emissive layer's blend
/// weight is *per texel*: the layer's alpha function times the texel's own.
/// Modelling only the layer's alpha explains some of the warden's spots and not
/// the rest, which is exactly what the second run of this gate reported.
fn producible_linear(rgba: &[u8]) -> Vec<([f32; 3], f32)> {
    let mut texels: HashSet<u32> = HashSet::new();
    for px in rgba.chunks_exact(4) {
        if px[3] != 0 {
            texels.insert(pack(px[0], px[1], px[2]) | ((px[3] as u32) << 24));
        }
    }
    let mut out = Vec::with_capacity(texels.len() * SHADES.len() * TINTS.len());
    for t in texels {
        let a = (t >> 24) as f32 / 255.0;
        let lin = [
            srgb_decode(((t >> 16) & 255) as u8),
            srgb_decode(((t >> 8) & 255) as u8),
            srgb_decode((t & 255) as u8),
        ];
        for s in SHADES {
            for tint in TINTS {
                out.push((
                    [
                        lin[0] * s * srgb_decode(tint[0]),
                        lin[1] * s * srgb_decode(tint[1]),
                        lin[2] * s * srgb_decode(tint[2]),
                    ],
                    a,
                ));
            }
        }
    }
    out
}

/// `entities::emissive_alpha`'s four `AlphaFunction` bodies, **re-declared**,
/// evaluated at this gate's neutral state (`age = 0`,
/// `EmissiveState::default()`).
///
/// Three of the five are 0 or 1 there and so need nothing: `Always` is opaque,
/// `Tendril` reads `state.tendril = 0.0`, `EyesGlowing` reads
/// `state.eyes_glow = false`, and `Heart`'s countdown is at its restart, i.e.
/// 1.0. The warden's two `PulsatingSpots` layers are the exception —
/// `max(0, cos(age * 0.045 + phase) * 0.25)` is **0.25** at phase 0 and 0 at
/// phase π — so exactly one layer in the whole cast blends fractionally, and a
/// gate built only on opaque draws reports its 660 spot pixels as coming from
/// no sheet at all. That was this gate's second first-run failure and, like the
/// first, it was the oracle rather than the renderer.
fn neutral_alpha(a: rewo_gpu::mobs::EmissiveAlpha) -> f32 {
    use rewo_gpu::mobs::EmissiveAlpha as A;
    match a {
        A::Always => 1.0,
        A::PulsatingSpots {
            phase,
        } => ((0.0f32 * 0.045 + phase).cos() * 0.25).max(0.0),
        A::Tendril => 0.0,
        A::Heart => 1.0,
        A::EyesGlowing => 0.0,
    }
}

/// Whether a rendered colour is one this sheet could produce, allowing the
/// one-LSB slack between the GPU's sRGB encode-on-store and this CPU one.
fn explains(set: &HashSet<u32>, c: u32) -> bool {
    let (r, g, b) = ((c >> 16) as i32, ((c >> 8) & 255) as i32, (c & 255) as i32);
    for dr in -1i32..=1 {
        for dg in -1i32..=1 {
            for db in -1i32..=1 {
                let (r, g, b) = (r + dr, g + dg, b + db);
                if (0..=255).contains(&r)
                    && (0..=255).contains(&g)
                    && (0..=255).contains(&b)
                    && set.contains(&pack(r as u8, g as u8, b as u8))
                {
                    return true;
                }
            }
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Frames
// ---------------------------------------------------------------------------

fn overlay_offscreen(ring: &OverlayRing) -> OverlayDraw<'_> {
    OverlayDraw {
        samples_ms: &ring.data,
        head: ring.head(),
        scale_ms: 20.0,
        origin: [-4000.0, -4000.0],
        size: [8.0, 8.0],
    }
}

fn neutral_draw(kind: EntityModelKind) -> EntityDraw<'static> {
    EntityDraw {
        pos: [0.0; 3],
        width: 1.0,
        height: 2.0,
        color: [1.0, 0.0, 1.0],
        name: None,
        health: None,
        kind,
        yaw: 0.0,
        death_time: 0.0,
        ground_item: None,
        armor: [None; 4],
        held_glint: [false; 2],
        ground_glint: false,
        ground_count: 0,
        bob_offset: 0.0,
        ground_seed: 0,
        ground_age: None,
        head_yaw: 0.0,
        pitch: 0.0,
        limb_swing: 0.0,
        limb_amount: 0.0,
        gesture: None,
        events: [None; rewo_gpu::mobs::ModelEvent::COUNT],
        shell: false,
        allay_dance: None,
        attack: rewo_gpu::mobs::SwingPose::NONE,
        arm_poses: rewo_gpu::mobs::ArmPoses::EMPTY,
        mob: rewo_gpu::mobs::MobCombat::default(),
        hurt: false,
        held: [None, None],
        skin_uv: None,
        scale_mul: 1.0,
        mount: None,
        anim_id: 0.0,
        light: [1.0, 1.0, 1.0],
        emissive: rewo_gpu::entities::EmissiveState::default(),
        variant: 0,
        dye: None,
        sheared: false,
        undercoat: false,
        fish_dye: None,
        cape: None,
    }
}

/// A scene as data, so a frame and its leave-one-out sibling are built from
/// the same declaration rather than from two edits of a draw list.
/// `EntityDraw` is deliberately not `Clone`, so this is also the only way to
/// say "the same scene minus one".
#[derive(Clone, Copy)]
struct Spec {
    kind: EntityModelKind,
    pos: [f32; 3],
    skin_uv: Option<[f32; 2]>,
    /// M175 — draw the mob with `MobCombat::is_baby`, which is what selects
    /// the whole-sheet swap (the texture half; the pose-scale half lives in
    /// live_cmd and is deliberately NOT set here, so the graded pixel area
    /// stays maximal).
    baby: bool,
}

fn build(specs: &[Spec], skip: Option<usize>) -> Vec<EntityDraw<'static>> {
    specs
        .iter()
        .enumerate()
        .filter(|(i, _)| Some(*i) != skip)
        .map(|(_, s)| {
            let mut d = neutral_draw(s.kind);
            d.pos = s.pos;
            d.skin_uv = s.skin_uv;
            d.mob.is_baby = s.baby;
            d
        })
        .collect()
}

/// One camera + target, so every frame in a comparison is byte-comparable.
struct Stage {
    off: Offscreen,
    wr: WorldRenderer,
    vp: [[f32; 4]; 4],
    right: [f32; 3],
    up: [f32; 3],
}

impl Stage {
    fn shoot(&mut self, gpu: &mut Gpu, specs: &[Spec], skip: Option<usize>) -> Result<Vec<u8>, String> {
        self.shoot_at(gpu, specs, skip, 0.0)
    }

    /// The same, at a chosen animation clock.
    ///
    /// `time` is `ageInTicks / 20`, so `0.0` is the neutral state every
    /// witness but `m12` grades. It matters for the oracle and not only for
    /// the pose: [`neutral_alpha`] evaluates the emissive alpha functions **at
    /// age 0**, so a sweep at another clock would need those re-derived. `m12`
    /// stays on kinds with no emissive layer for exactly that reason.
    fn shoot_at(
        &mut self,
        gpu: &mut Gpu,
        specs: &[Spec],
        skip: Option<usize>,
        time: f32,
    ) -> Result<Vec<u8>, String> {
        let draws = build(specs, skip);
        let ring = OverlayRing::default();
        let ov = overlay_offscreen(&ring);
        self.wr.set_entities(&draws, self.right, self.up, time);
        self.off.render(gpu, Some((&mut self.wr, self.vp)), &ov, BG)?;
        self.off.read_rgba(gpu)
    }

    fn destroy(&mut self, gpu: &mut Gpu) {
        self.wr.destroy(gpu);
        self.off.destroy(gpu);
    }
}

/// Pixels of `full` that vanish when one draw is removed — that draw's own.
fn attributed(full: &[u8], without: &[u8]) -> Vec<u32> {
    let mut out = Vec::new();
    for i in (0..full.len()).step_by(4) {
        if full[i..i + 3] != without[i..i + 3] {
            out.push(pack(full[i], full[i + 1], full[i + 2]));
        }
    }
    out
}

// ---------------------------------------------------------------------------
// The gate
// ---------------------------------------------------------------------------

struct Witness {
    name: &'static str,
    ok: bool,
    detail: String,
}

fn run_check(
    gpu: &mut Gpu,
    baked: &assets::BakedAssets,
    jar: &Path,
    args: &MobtexshotArgs,
) -> Result<(), String> {
    if let Some(d) = &args.out_dir {
        std::fs::create_dir_all(d).map_err(|e| format!("out-dir: {e}"))?;
    }
    // Producible-colour set per baked sheet key, straight off the jar's PNGs.
    let sheets: HashMap<&str, HashSet<u32>> = baked
        .mob_textures
        .iter()
        .map(|t| (t.key, producible(&t.rgba)))
        .collect();
    // Kind -> its own sheet keys, from the model registry's declaration.
    //
    // `MobDef::textures` is not the whole list: a vanilla emissive layer
    // (`EyesLayer`, `LivingEntityEmissiveLayer`) re-renders the same geometry
    // against a *separate* sheet — `spider_eyes`, `enderman_eyes`,
    // `phantom_eyes`, the warden's four — and those are declared in
    // `mobs::emissive_layers` instead. The first run of this gate reported five
    // kinds as unexplained for exactly that reason, which was the oracle being
    // incomplete rather than the renderer being wrong; a mob's *own* sheets are
    // the union of the two declarations.
    let own: HashMap<EntityModelKind, OwnSheets> = rewo_gpu::mobs::MOBS
        .iter()
        .map(|d| {
            let mut keys = d.textures.to_vec();
            let mut blends = Vec::new();
            for l in rewo_gpu::mobs::emissive_layers(d.kind) {
                if !keys.contains(&l.tex) {
                    keys.push(l.tex);
                }
                // Every layer that draws at all, not just the fractional ones:
                // an `Always` layer's *layer* alpha is 1.0 but `entity.frag`
                // multiplies by the texel's own, and `emit_model`'s own comment
                // records that a vanilla emissive texture is "either fully
                // transparent or at least 0.19 opaque". 0.19 is a blend. That
                // was the gate's third first-run failure and the third time the
                // oracle was the thing that was incomplete.
                let a = neutral_alpha(l.alpha);
                if a > 0.0 {
                    blends.push((l.tex, a));
                }
            }
            (
                d.kind,
                OwnSheets {
                    keys,
                    blends,
                },
            )
        })
        .collect();
    let lin: HashMap<&str, Vec<([f32; 3], f32)>> = baked
        .mob_textures
        .iter()
        .map(|t| (t.key, producible_linear(&t.rgba)))
        .collect();

    let mut w: Vec<Witness> = Vec::new();

    // ---- the small frames -----------------------------------------------
    let (sw, sh) = (768u32, 512u32);
    let mut stage = stage_from(
        gpu,
        baked,
        sw,
        sh,
        Vec3::new(0.0, 1.4, 12.0),
        Vec3::new(0.0, 1.2, 0.0),
    )?;
    let kinds_all = stage
        .wr
        .entity_pass()
        .ok_or("no entity pass")?
        .available_kinds();

    let z0 = row(EntityModelKind::Zombie, 0);
    let solo = [z0];
    let trio = [
        row(EntityModelKind::Zombie, 0),
        row(EntityModelKind::Zombie, 1),
        row(EntityModelKind::Villager, 2),
    ];

    // m0: the control. Two renders of one scene must be byte-identical, or
    // every differential witness below is measuring noise.
    let a0 = stage.shoot(gpu, &solo, None)?;
    let b0 = stage.shoot(gpu, &solo, None)?;
    w.push(wit(
        "m0.the_same_scene_renders_byte_identically",
        a0 == b0,
        format!("{} differing bytes", diff_bytes(&a0, &b0)),
    ));

    // m1: the reported symptom's exact shape — two of one kind plus a third,
    // in ONE `set_entities`. Zombie and villager are the pair M46 reported.
    let full = stage.shoot(gpu, &trio, None)?;
    if let Some(d) = &args.out_dir {
        save_png(&full, sw, sh, &d.join("trio.png"))?;
    }
    let mut trio_bad: Vec<String> = Vec::new();
    let mut trio_px = Vec::new();
    let mut zombie0: Vec<u8> = Vec::new();
    let mut villager_px: Vec<u32> = Vec::new();
    for (i, s) in trio.iter().enumerate() {
        let wo = stage.shoot(gpu, &trio, Some(i))?;
        let px = attributed(&full, &wo);
        trio_px.push(px.len());
        let keys = &own[&s.kind];
        let bad = first_unexplained(&px, keys, &sheets, &lin);
        if let Some(bad) = bad {
            trio_bad.push(format!(
                "{}#{i}: {} px, colour #{:06X} is not producible by {:?}",
                s.kind.name(),
                px.len(),
                bad,
                keys.keys
            ));
        }
        if i == 0 {
            zombie0 = wo;
        }
        if i == 2 {
            villager_px = px;
        }
    }
    w.push(wit(
        "m1.two_of_one_kind_plus_a_third_each_sample_their_own_sheet",
        trio_bad.is_empty() && trio_px.iter().all(|n| *n >= MIN_PIXELS),
        if trio_bad.is_empty() {
            format!("pixels {trio_px:?}")
        } else {
            trio_bad.join("; ")
        },
    ));

    // m2: the count axis, isolated. The same mob at the same place with the
    // same camera, alone, must be byte-identical to its appearance in the
    // crowd — otherwise m1 cannot tell "more than one entity" from "this kind
    // is simply wrong".
    let solo_img = stage.shoot(gpu, &solo, None)?;
    let mut moved = 0usize;
    let mut attributed_px = 0usize;
    for i in (0..full.len()).step_by(4) {
        if full[i..i + 3] != zombie0[i..i + 3] {
            attributed_px += 1;
            if full[i..i + 3] != solo_img[i..i + 3] {
                moved += 1;
            }
        }
    }
    w.push(wit(
        "m2.one_instance_is_byte_identical_alone_and_in_a_crowd",
        moved == 0 && attributed_px >= MIN_PIXELS,
        format!("{moved} of {attributed_px} attributed pixels differ"),
    ));

    // m3: `EntityDraw::skin_uv`'s doc says "Ignored for non-player models".
    // `emit_model` ignored nothing — the invariant was held only by
    // `live_cmd`'s `if is_player`, i.e. by a caller. Grade it in the pass.
    let mut skinned = solo;
    skinned[0].skin_uv = Some([0.125, 0.0625]);
    let with_skin = stage.shoot(gpu, &skinned, None)?;
    w.push(wit(
        "m3.skin_uv_does_not_move_a_non_player_mob",
        with_skin == solo_img,
        format!("{} differing bytes", diff_bytes(&with_skin, &solo_img)),
    ));

    // m4: the other direction. A witness that only said "skin_uv changes
    // nothing" would pass against a pass that ignored it for players too, and
    // the player skin is the one thing the field exists for.
    let player = [row(EntityModelKind::Player, 0)];
    let pbase = stage.shoot(gpu, &player, None)?;
    let mut pskin = player;
    pskin[0].skin_uv = Some([0.125, 0.0625]);
    let pmoved = stage.shoot(gpu, &pskin, None)?;
    w.push(wit(
        "m4.skin_uv_still_moves_the_player_model",
        pmoved != pbase,
        format!("{} differing bytes", diff_bytes(&pmoved, &pbase)),
    ));
    stage.destroy(gpu);

    // ---- the whole cast in one draw list ---------------------------------
    let cols = 10usize;
    let (lw, lh) = (1600u32, 900u32);
    let rows = kinds_all.len().div_ceil(cols) as f32;
    let center = Vec3::new(0.0, 2.0, -(rows - 1.0) * GRID_Z / 2.0);
    let eye = center + Vec3::new(0.0, 13.0, rows * GRID_Z * 0.6 + 16.0);
    let mut big = stage_from(gpu, baked, lw, lh, eye, center)?;
    let cast: Vec<Spec> = kinds_all
        .iter()
        .enumerate()
        .map(|(i, k)| grid(*k, i, cols))
        .collect();
    let full = big.shoot(gpu, &cast, None)?;
    if let Some(d) = &args.out_dir {
        save_png(&full, lw, lh, &d.join("cast.png"))?;
    }
    let mut bad: Vec<String> = Vec::new();
    let mut small: Vec<&str> = Vec::new();
    let mut ambiguous: Vec<(&str, &str)> = Vec::new();
    let mut graded = 0usize;
    let mut discriminating = 0usize;
    for (i, s) in cast.iter().enumerate() {
        let wo = big.shoot(gpu, &cast, Some(i))?;
        let px = attributed(&full, &wo);
        let Some(keys) = own.get(&s.kind) else {
            // The other fail-open path: `continue` here dropped a kind from
            // the sweep without printing it and without counting it in
            // `small`, so it was the one silent exit in this file. Unreachable
            // today (81 graded + 9 too small = 90 = `available_kinds().len()`)
            // — which is exactly why it has to be an assertion rather than a
            // comment saying it cannot happen.
            bad.push(format!(
                "{}: renders, but `mobs::MOBS` declares no sheets for it",
                s.kind.name()
            ));
            continue;
        };
        if px.len() < MIN_PIXELS {
            small.push(s.kind.name());
            continue;
        }
        graded += 1;
        if let Some(c) = first_unexplained(&px, keys, &sheets, &lin) {
            bad.push(format!(
                "{}: colour #{:06X} of {} px is not producible by {:?}",
                s.kind.name(),
                c,
                px.len(),
                keys.keys
            ));
            continue;
        }
        // Could some *other* single sheet have produced this whole render? If
        // so a wrong-sheet sample onto it would be invisible here — sound, but
        // not discriminating, so say so out loud and count it.
        let distinct: HashSet<u32> = px.into_iter().collect();
        let mut imposter = None;
        for (k, set) in &sheets {
            if keys.keys.contains(k) {
                continue;
            }
            if distinct.iter().all(|c| explains(set, *c)) {
                imposter = Some(*k);
                break;
            }
        }
        match imposter {
            Some(k) => ambiguous.push((s.kind.name(), k)),
            None => discriminating += 1,
        }
    }
    big.destroy(gpu);
    for k in &small {
        println!("[mobtexshot] SKIP {k}: fewer than {MIN_PIXELS} attributed pixels");
    }
    for (k, imp) in &ambiguous {
        println!(
            "[mobtexshot] NOTE {k}: sheet {imp:?} could produce every colour it drew — sound, not discriminating"
        );
    }
    // The accounting identity is half of this witness and the reason is that
    // "drew nothing" used to pass: every kind in the draw list must land in
    // exactly one of the two buckets, and the SKIP bucket is pinned, so a mob
    // that stops rendering moves from `graded` to `small` and turns this red
    // instead of printing one more SKIP line.
    let accounted = graded + small.len() == kinds_all.len();
    w.push(wit(
        "m5.every_kind_in_one_draw_list_samples_only_its_own_sheets",
        bad.is_empty() && accounted && graded >= MIN_GRADED && small.len() <= MAX_SMALL,
        if bad.is_empty() {
            format!(
                "{graded} kinds graded in one set_entities, {} too small to grade \
                 (max {MAX_SMALL}), {} in the draw list",
                small.len(),
                kinds_all.len()
            )
        } else {
            bad.join("; ")
        },
    ));
    // The gate must be able to tell sheets apart or m1/m5 are vacuous.
    w.push(wit(
        "m6.the_oracle_can_tell_most_sheets_apart",
        discriminating >= MIN_DISCRIMINATING && ambiguous.len() <= MAX_AMBIGUOUS,
        format!(
            "{discriminating} of {graded} kinds are uniquely explained by their own \
             sheet, {} ambiguous (max {MAX_AMBIGUOUS})",
            ambiguous.len()
        ),
    ));

    // m7: the oracle's own sensitivity, proven rather than asserted. §0.0's
    // report names a zombie rendering with a villager's brown head, and the
    // atlas puts those two sheets on one shelf — so the check that matters is
    // that villager colours are NOT zombie colours. If they were, m1 could not
    // have caught the reported bug however it was written.
    let z = sheets.get("zombie").ok_or("no zombie sheet")?;
    let v = sheets.get("villager").ok_or("no villager sheet")?;
    let zombie_cannot: usize = v.iter().filter(|c| !explains(z, **c)).count();
    w.push(wit(
        "m7.a_villager_colour_is_not_a_zombie_colour",
        zombie_cannot > 0,
        format!(
            "{zombie_cannot} of {} villager-producible colours are outside zombie's set",
            v.len()
        ),
    ));

    // m9: the NEGATIVE control, and the reason it exists is worth writing down.
    //
    // A mutation battery on the first build of this gate replaced
    // `first_unexplained`'s "the kind's own sheets" with "every sheet in the
    // atlas" — and **m1, m2 and m5 all stayed green**. So what they proved was
    // that a mob sampled *some* sheet, which is a materially weaker claim than
    // the one this gate is named for and is satisfied by the exact bug it was
    // written to catch. The witnesses could not see it because a witness cannot
    // observe its own predicate being widened.
    //
    // This one can: it feeds the VILLAGER's rendered pixels to the ZOMBIE's
    // sheet set, through the same function, and requires it to come back
    // `Some`. A predicate that ignores the sheet set it was handed answers
    // `None` here and the gate goes red.
    let mis = first_unexplained(&villager_px, &own[&EntityModelKind::Zombie], &sheets, &lin);
    w.push(wit(
        "m9.grading_one_mobs_pixels_against_anothers_sheets_reports_a_miss",
        mis.is_some() && !villager_px.is_empty(),
        match mis {
            Some(c) => format!(
                "villager pixel #{c:06X} is not producible by the zombie's sheets, over {} px",
                villager_px.len()
            ),
            None => format!(
                "NO miss over {} villager pixels — the check is not reading the \
                 sheet set it is handed, so m1/m5 mean 'some sheet' rather than \
                 'its own'",
                villager_px.len()
            ),
        },
    ));

    // m8: the state-conditional gap, CLOSED in M175 and now pinned from both
    // sides. 44 of vanilla's 91 `getTextureLocation` overrides switch sheet on
    // render state, and the largest single family is `isBaby`
    // (`AbstractZombieRenderer.java:25-27`). The generated
    // `baby_texture_table` encodes exactly those swaps; only the ones whose
    // baby sheet tiles like its adult are baked (the renderer re-points UVs by
    // an atlas offset, exact only at equal size) — a differently-sized baby
    // sheet rides vanilla's separate BABY model layer, which Rewo does not
    // build yet, so those are counted as skips rather than rendered wrong.
    let jar_babies = count_jar_baby_sheets(jar)?;
    let expected_same_size = rewo_data::baby_texture_table::BABY_SWAPS
        .iter()
        .filter(|s| {
            let rel = s.adult_path.strip_prefix("textures/").unwrap_or(s.adult_path);
            rewo_data::assets::mob_texture_specs()
                .any(|(_, p, w, h)| p == rel && w == s.w && h == s.h)
        })
        .count();
    let baked_babies = baked
        .mob_textures
        .iter()
        .filter(|t| t.key.ends_with("_baby"))
        .count();
    let total_swaps = rewo_data::baby_texture_table::BABY_SWAPS.len();
    w.push(wit(
        "m8.the_baby_swaps_are_baked_up_to_their_pinned_split",
        jar_babies == JAR_BABY_SHEETS
            && expected_same_size > 0
            && baked_babies == expected_same_size
            && baked.baby_swap_skips + baked_babies == total_swaps,
        format!(
            "jar has {jar_babies} *baby*.png (pinned at {JAR_BABY_SHEETS}); table has \
             {total_swaps} swaps -> {expected_same_size} same-size baked + \
             {} deferred to the BABY-model milestone",
            baked.baby_swap_skips
        ),
    ));

    // n1/n2 — a BABY zombie wears `zombie_baby.png`, not a shrunken adult
    // wearing `zombie.png`. Attribution is leave-one-out over one mob; the
    // claim splits in two because either half alone can pass against a swap
    // that silently did not happen: every attributed pixel must be PRODUCIBLE
    // by the baby sheet (n1), and at least one must be IMPOSSIBLE for the
    // adult sheet (n2 — zombie.png stays in the atlas for the adults). A OWN
    // stage, because the sweep's `stage` was destroyed at m4's end and only
    // `stack`/`pools` exist past it — the first cut re-shot the corpse and
    // died on a destroyed command pool (0xC0000005, no Rust panic).
    let mut baby_stage = stage_from(
        gpu,
        baked,
        sw,
        sh,
        Vec3::new(0.0, 1.4, 12.0),
        Vec3::new(0.0, 1.2, 0.0),
    )?;
    let zrow = row(EntityModelKind::Zombie, 0);
    let full_a = baby_stage.shoot(gpu, &[zrow], None)?;
    let sans_a = baby_stage.shoot(gpu, &[zrow], Some(0))?;
    let adult_px = attributed(&full_a, &sans_a);
    let mut zb = zrow;
    zb.baby = true;
    let full_b = baby_stage.shoot(gpu, &[zb], None)?;
    let sans_b = baby_stage.shoot(gpu, &[zb], Some(0))?;
    let baby_px = attributed(&full_b, &sans_b);
    let baby_set = sheets
        .get("zombie_baby")
        .ok_or("zombie_baby not baked — m8's split says it should be")?;
    let n1 = !baby_px.is_empty() && baby_px.iter().all(|c| explains(baby_set, *c));
    w.push(wit(
        "n1.a_baby_zombies_pixels_are_all_producible_by_its_own_baby_sheet",
        n1,
        format!(
            "{} attributed px, {} unexplained by zombie_baby",
            baby_px.len(),
            baby_px.iter().filter(|c| !explains(baby_set, **c)).count()
        ),
    ));
    let adult_set = sheets
        .get("zombie")
        .ok_or("zombie not baked")?;
    let n2 = baby_px.iter().any(|c| !explains(adult_set, *c));
    w.push(wit(
        "n2.the_swap_actually_happened_adult_sheet_cannot_explain_the_baby",
        n2,
        format!(
            "{} of {} baby px are impossible for zombie.png (a no-op swap would put this at 0)",
            baby_px.iter().filter(|c| !explains(adult_set, **c)).count(),
            baby_px.len()
        ),
    ));
    // n3 — the ADULT control: the same scene without `baby` still grades
    // clean against zombie.png, so n1/n2 cannot be satisfied by some global
    // texture break.
    let n3 = !adult_px.is_empty() && adult_px.iter().all(|c| explains(adult_set, *c));
    w.push(wit(
        "n3.the_adult_control_still_grades_against_zombie_png",
        n3,
        format!(
            "{} attributed px, {} unexplained by zombie.png",
            adult_px.len(),
            adult_px.iter().filter(|c| !explains(adult_set, **c)).count()
        ),
    ));
    // n4 — the DEFERRED half is deliberate: a size-mismatched swap (happy
    // ghast's 64² baby against a 128² adult) is NOT baked, because offsetting
    // UVs across different sheet sizes samples the wrong texels. Its absence
    // from the atlas is the assertion.
    let deferred_missing = !baked.mob_textures.iter().any(|t| t.key == "happy_ghast_baby");
    w.push(wit(
        "n4.size_mismatched_baby_sheets_stay_unbaked_on_purpose",
        deferred_missing && baked.baby_swap_skips > 0,
        format!(
            "happy_ghast_baby absent: {deferred_missing}, skips {} (need the BABY model layer first)",
            baked.baby_swap_skips
        ),
    ));
    baby_stage.destroy(gpu);

    // m12: the recorded repro's own geometry and clock, which m1 did not have.
    //
    // §0.0's repro is `REWO_PRECMD` two `summon zombie` **at one spot** with
    // `REWO_SETTLE=13`; m1's trio sits `ROW_X` = 3.6 blocks apart at `t = 0`,
    // so it eliminated the *count* axis and left the *co-location* and *settle*
    // ones untouched. This one puts the pair inside each other's bounding box
    // (0.6 blocks, one entity width — what two mobs summoned at one point have
    // pushed apart to by the time they settle) and renders at `t = 13.0`.
    //
    // Leave-one-out does not care that they overlap: the pixels that vanish
    // when mob *i* is removed are exactly the ones it covered, *including*
    // where it occluded a neighbour, which is the property that makes the
    // attribution work on a pile at all.
    let mut stack = stage_from(
        gpu,
        baked,
        sw,
        sh,
        Vec3::new(0.0, 1.4, 12.0),
        Vec3::new(0.0, 1.2, 0.0),
    )?;
    let pile = [
        Spec {
            kind: EntityModelKind::Zombie,
            pos: [0.0, 0.0, 0.0],
            skin_uv: None,
            baby: false,
        },
        Spec {
            kind: EntityModelKind::Zombie,
            pos: [PILE_SPACING, 0.0, -PILE_SPACING],
            skin_uv: None,
            baby: false,
        },
        Spec {
            kind: EntityModelKind::Villager,
            pos: [2.0 * PILE_SPACING, 0.0, -2.0 * PILE_SPACING],
            skin_uv: None,
            baby: false,
        },
    ];
    const SETTLED: f32 = 13.0;
    let pile_full = stack.shoot_at(gpu, &pile, None, SETTLED)?;
    if let Some(d) = &args.out_dir {
        save_png(&pile_full, sw, sh, &d.join("pile.png"))?;
    }
    let mut pile_bad: Vec<String> = Vec::new();
    let mut pile_px = Vec::new();
    for (i, sp) in pile.iter().enumerate() {
        let wo = stack.shoot_at(gpu, &pile, Some(i), SETTLED)?;
        let px = attributed(&pile_full, &wo);
        pile_px.push(px.len());
        let keys = &own[&sp.kind];
        if let Some(bad) = first_unexplained(&px, keys, &sheets, &lin) {
            pile_bad.push(format!(
                "{}#{i}: {} px, colour #{:06X} is not producible by {:?}",
                sp.kind.name(),
                px.len(),
                bad,
                keys.keys
            ));
        }
    }
    stack.destroy(gpu);
    w.push(wit(
        "m12.two_zombies_at_one_spot_after_a_settle_still_sample_their_own_sheets",
        pile_bad.is_empty() && pile_px.iter().all(|n| *n >= MIN_PIXELS),
        if pile_bad.is_empty() {
            format!("pixels {pile_px:?} at {PILE_SPACING} block spacing, t = {SETTLED}s")
        } else {
            pile_bad.join("; ")
        },
    ));

    // ---- the dynamic atlas pools ----------------------------------------
    //
    // m10/m11 are the witnesses `SlotRing`'s unit tests are NOT. The pools'
    // bug is a *caller* that recycles a slot and leaves the key that addressed
    // it in the map, so past capacity two keys resolve to one slot and the
    // older one silently addresses the newer upload — "renders with a texture
    // that is not its own", one atlas band over from the mob sheets. `claim`'s
    // tests grade the ring; deleting **both** `remove(&old)` blocks at the two
    // call sites left this gate at 10/10, `rewo-gpu` at 293/293, `itemshot`
    // and `mobshot` green. These two run the production `upload_trim` and
    // `prepare_held_items` on a real device, over-fill each pool by exactly
    // one, and state the invariant the eviction exists for.
    let mut pools = stage_from(
        gpu,
        baked,
        64,
        64,
        Vec3::new(0.0, 1.4, 12.0),
        Vec3::new(0.0, 1.2, 0.0),
    )?;

    // m10 — the trim pool (64 slots), through `upload_trim` itself.
    let trim_key = |i: usize| format!("rewo:mobtexshot/pool/{i}");
    let trim_sprite = |i: usize| -> Vec<u8> {
        let mut v = vec![0u8; (TRIM_W * TRIM_H * 4) as usize];
        for (n, px) in v.chunks_exact_mut(4).enumerate() {
            px[0] = (i & 0xFF) as u8;
            px[1] = (n & 0xFF) as u8;
            px[2] = 0x40;
            px[3] = 0xFF;
        }
        v
    };
    let mut origins: Vec<Option<(u32, u32)>> = Vec::new();
    for i in 0..TRIM_POOL {
        origins.push(pools.wr.upload_entity_trim(
            gpu,
            &trim_key(i),
            &trim_sprite(i),
            TRIM_W,
            TRIM_H,
        ));
    }
    // One past capacity: this claim must recycle slot 0 and evict key 0.
    let over = pools.wr.upload_entity_trim(
        gpu,
        &trim_key(TRIM_POOL),
        &trim_sprite(TRIM_POOL),
        TRIM_W,
        TRIM_H,
    );
    let trim_pairs = pools
        .wr
        .entity_pass()
        .map(|p| p.trim_slot_pairs())
        .unwrap_or_default();
    let distinct_origins: HashSet<(u32, u32)> = origins.iter().flatten().copied().collect();
    let live_origins: HashSet<(u32, u32)> = trim_pairs.iter().map(|(_, o)| *o).collect();
    let evicted_still_resident = trim_pairs.iter().any(|(k, _)| *k == trim_key(0));
    let m10 = origins.iter().all(|o| o.is_some())
        && distinct_origins.len() == TRIM_POOL
        && over.is_some()
        && over == origins[0]
        && trim_pairs.len() == TRIM_POOL
        && live_origins.len() == TRIM_POOL
        && !evicted_still_resident;
    w.push(wit(
        "m10.the_trim_pools_wrap_evicts_the_key_that_addressed_the_slot",
        m10,
        format!(
            "{} distinct origins over {TRIM_POOL} slots; the {}th claim {} slot 0's \
             origin; {} live key(s) over {} distinct slot(s); the evicted key is {}",
            distinct_origins.len(),
            TRIM_POOL + 1,
            if over == origins[0] { "recycled" } else { "did NOT recycle" },
            trim_pairs.len(),
            live_origins.len(),
            if evicted_still_resident {
                "STILL RESIDENT — it now addresses the sprite uploaded over it"
            } else {
                "gone"
            }
        ),
    ));

    // m11 — the item pool (1,024 slots), through `prepare_held_items`.
    //
    // Driven with the jar's REAL held items rather than a synthetic fixture,
    // because the fixture would be one more thing that could agree with the
    // code under test. The names are sorted and truncated to the shortest
    // prefix that offers exactly `ITEM_POOL + 1` distinct 16x16 textures, so
    // the wrap happens exactly once, the claim order is deterministic, and the
    // run costs 1,025 uploads rather than every item in the jar.
    let items = crate::live_cmd::to_gpu_held_items(&baked.held_items);
    let mut all_names: Vec<String> = items
        .models
        .keys()
        .chain(items.block_entities.keys())
        .cloned()
        .collect();
    all_names.sort();
    let mut offered: Vec<u16> = Vec::new();
    let mut seen: HashSet<u16> = HashSet::new();
    let mut names: Vec<String> = Vec::new();
    'outer: for n in &all_names {
        names.push(n.clone());
        let Some(m) = items.any(n) else { continue };
        for q in &m.quads {
            let Some(t) = items.textures.get(q.tex as usize) else {
                continue;
            };
            if t.w != ITEM_PX || t.h != ITEM_PX || !seen.insert(q.tex) {
                continue;
            }
            offered.push(q.tex);
            if offered.len() > ITEM_POOL {
                break 'outer;
            }
        }
    }
    pools.wr.set_held_items(items);
    let refs: Vec<&str> = names.iter().map(|s| s.as_str()).collect();
    pools.wr.prepare_held_items(gpu, &refs)?;
    let item_pairs = pools
        .wr
        .entity_pass()
        .map(|p| p.item_slot_pairs())
        .unwrap_or_default();
    let live_slots: HashSet<u32> = item_pairs.iter().map(|(_, s)| *s).collect();
    // The first texture claimed is the one slot 0 holds, so the claim past
    // capacity takes its slot; it is the key that used to alias.
    let first_tex = offered.first().copied();
    let first_still_resident = first_tex.is_some_and(|t| item_pairs.iter().any(|(k, _)| *k == t));
    let m11 = offered.len() == ITEM_POOL + 1
        && item_pairs.len() == ITEM_POOL
        && live_slots.len() == ITEM_POOL
        && !first_still_resident;
    w.push(wit(
        "m11.the_item_pools_wrap_evicts_the_key_that_addressed_the_slot",
        m11,
        format!(
            "{} distinct 16x16 textures offered over {ITEM_POOL} slots from {} item \
             name(s); {} live key(s) over {} distinct slot(s); the evicted texture \
             {:?} is {}",
            offered.len(),
            refs.len(),
            item_pairs.len(),
            live_slots.len(),
            first_tex,
            if first_still_resident {
                "STILL RESIDENT — it now addresses the sprite uploaded over it"
            } else {
                "gone"
            }
        ),
    ));
    pools.destroy(gpu);

    // ---- report ----------------------------------------------------------
    let pass = w.iter().filter(|x| x.ok).count();
    for x in &w {
        println!(
            "[mobtexshot] {} {} — {}",
            if x.ok { "ok  " } else { "FAIL" },
            x.name,
            x.detail
        );
    }
    println!("[mobtexshot] {pass}/{} witnesses", w.len());
    if w.len() != EXPECTED_WITNESSES {
        return Err(format!(
            "mobtexshot: produced {} witnesses, expected {EXPECTED_WITNESSES}",
            w.len()
        ));
    }
    if pass != w.len() {
        return Err(format!(
            "mobtexshot: {} of {} witnesses failed",
            w.len() - pass,
            w.len()
        ));
    }
    Ok(())
}

fn wit(name: &'static str, ok: bool, detail: String) -> Witness {
    Witness {
        name,
        ok,
        detail,
    }
}

fn diff_bytes(a: &[u8], b: &[u8]) -> usize {
    a.iter().zip(b).filter(|(x, y)| x != y).count()
}

/// What a kind is allowed to have sampled: the sheets it declares, plus the
/// fractional-alpha emissive layers it blends over them at this gate's neutral
/// state.
struct OwnSheets {
    keys: Vec<&'static str>,
    /// `(sheet key, blend factor)` for every emissive layer whose neutral alpha
    /// is strictly between 0 and 1 — derived from [`neutral_alpha`], so no mob
    /// is named and the list cannot rot into a way of hiding a failure.
    blends: Vec<(&'static str, f32)>,
}

fn first_unexplained(
    px: &[u32],
    own: &OwnSheets,
    sheets: &HashMap<&str, HashSet<u32>>,
    lin: &HashMap<&str, Vec<([f32; 3], f32)>>,
) -> Option<u32> {
    let sets: Vec<&HashSet<u32>> = own.keys.iter().filter_map(|k| sheets.get(k)).collect();
    if sets.is_empty() {
        // FAIL CLOSED. Empty means none of this kind's declared sheet keys
        // resolved in `baked.mob_textures`, and answering `None` there reads
        // as *every pixel explained* — the gate would be greenest exactly
        // where it knows least, which is the one thing a fail-closed gate must
        // not do. Report the first pixel instead; the caller prints the kind
        // and the (empty) key list beside it.
        return px.first().copied();
    }
    let mut seen: HashSet<u32> = HashSet::new();
    for c in px {
        if !seen.insert(*c) {
            continue;
        }
        if sets.iter().any(|s| explains(s, *c)) {
            continue;
        }
        // A translucent emissive layer is a real convex combination of its own
        // texel and whatever this mob had drawn underneath — both of which are
        // its own sheets. Only reached on a miss, so the cost rides on the
        // handful of colours the fast path cannot explain.
        let dst: Vec<&Vec<([f32; 3], f32)>> = own.keys.iter().filter_map(|k| lin.get(k)).collect();
        let blended = own.blends.iter().any(|(k, a)| {
            let Some(e) = lin.get(k) else {
                return false;
            };
            blend_explains(*c, e, &dst, *a)
        });
        if !blended {
            return Some(*c);
        }
    }
    None
}

/// Whether `c` is `encode(a * src + (1 - a) * dst)` for some producible pair,
/// where `a` is the layer's alpha times the *source texel's* own.
fn blend_explains(
    c: u32,
    src: &[([f32; 3], f32)],
    dst: &[&Vec<([f32; 3], f32)>],
    layer_alpha: f32,
) -> bool {
    let want = [(c >> 16) as u8, ((c >> 8) & 255) as u8, (c & 255) as u8];
    for (s, ta) in src {
        let a = layer_alpha * ta;
        // Vanilla's `ALPHA_CUTOUT` 0.1 on `entityTranslucentEmissive`.
        if a < 0.1 {
            continue;
        }
        let b = 1.0 - a;
        let sa = [s[0] * a, s[1] * a, s[2] * a];
        for group in dst {
            for (d, _) in group.iter() {
                let got = [
                    srgb_encode(sa[0] + d[0] * b),
                    srgb_encode(sa[1] + d[1] * b),
                    srgb_encode(sa[2] + d[2] * b),
                ];
                if (0..3).all(|i| (got[i] as i32 - want[i] as i32).abs() <= 1) {
                    return true;
                }
            }
        }
    }
    false
}

fn count_jar_baby_sheets(jar: &Path) -> Result<usize, String> {
    let f = std::fs::File::open(jar).map_err(|e| format!("open jar: {e}"))?;
    let mut z = zip::ZipArchive::new(f).map_err(|e| format!("jar: {e}"))?;
    let mut n = 0usize;
    for i in 0..z.len() {
        let name = match z.by_index(i) {
            Ok(e) => e.name().to_string(),
            Err(_) => continue,
        };
        if name.starts_with("assets/minecraft/textures/entity/")
            && name.ends_with(".png")
            && name.contains("baby")
        {
            n += 1;
        }
    }
    Ok(n)
}

// ---------------------------------------------------------------------------
// Layout
// ---------------------------------------------------------------------------

const ROW_X: f32 = 3.6;
const GRID_X: f32 = 4.6;
const GRID_Z: f32 = 7.5;

fn row(kind: EntityModelKind, slot: usize) -> Spec {
    Spec {
        kind,
        pos: [(slot as f32 - 1.0) * ROW_X, 0.0, 0.0],
        skin_uv: None,
        baby: false,
    }
}

fn grid(kind: EntityModelKind, i: usize, cols: usize) -> Spec {
    let (r, c) = (i / cols, i % cols);
    Spec {
        kind,
        pos: [
            (c as f32 - (cols as f32 - 1.0) / 2.0) * GRID_X,
        0.0,
        -(r as f32) * GRID_Z,
    ],
        skin_uv: None,
        baby: false,
    }
}

fn stage_from(
    gpu: &mut Gpu,
    baked: &assets::BakedAssets,
    w: u32,
    h: u32,
    eye: Vec3,
    center: Vec3,
) -> Result<Stage, String> {
    let off = Offscreen::new(gpu, w, h)?;
    let mut wr = WorldRenderer::new(gpu, off.format, assets::TEX_SIZE, &baked.layers)?;
    wr.init_entities(
        gpu,
        crate::live_cmd::font_data(baked),
        crate::live_cmd::entity_textures(baked),
    )?;
    let dir = (center - eye).normalize();
    wr.set_camera(eye.to_array());
    let view = Mat4::look_to_rh(eye, dir, Vec3::Y);
    let proj = Mat4::from_cols_array_2d(&rewo_gpu::world::perspective_reverse_z(
        55f32.to_radians(),
        w as f32 / h as f32,
        0.05,
    ));
    let vp = (proj * view).to_cols_array_2d();
    let right = dir.cross(Vec3::Y).normalize_or_zero();
    let up = right.cross(dir).normalize_or_zero();
    Ok(Stage {
        off,
        wr,
        vp,
        right: right.to_array(),
        up: up.to_array(),
    })
}

fn save_png(rgba: &[u8], w: u32, h: u32, path: &Path) -> Result<(), String> {
    let file = std::fs::File::create(path).map_err(|e| format!("create {path:?}: {e}"))?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()
        .and_then(|mut wtr| wtr.write_image_data(rgba))
        .map_err(|e| format!("png {path:?}: {e}"))?;
    Ok(())
}

fn client_jar(version: &str) -> Option<PathBuf> {
    let mut p = dirs::config_dir()?;
    p.push("EwoClient");
    p.push("shared/versions");
    p.push(version);
    p.push(format!("{version}.jar"));
    p.exists().then_some(p)
}
