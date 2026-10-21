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
const EXPECTED_WITNESSES: usize = 101;
/// M383 — m101's floor (px of stray silhouette beyond the skeleton's).
/// M383 measured 913; the clothing on 2-wide limbs (the pre-M383 mutant)
/// 536.
const STRAY_CLOTHING_MIN: usize = 725;
/// M382 — m100's floor. M382 measured 237 px; the zombie arm without the
/// post-step (the pre-M382 mutant, whose arm the zombie override holds
/// forward whatever the pose) 0.
const DROWNED_MIN: usize = 120;
/// M381 — m99's floor (each rider). M381 measured 661 (zombie) and 679
/// (pillager); no riding pose (the pre-M381 mutant) 0 and 0.
const RIDER_MIN: usize = 330;
/// M380 — m98's floor. M380 measured 89 px; still ears and the bare
/// `ArmRight`/`ArmLeft` swing (the pre-M380 mutant) 0.
const PIGLIN_MIN: usize = 40;
/// M378 — m96's floors: sitting, walking. M378 measured 1004 and 85; the
/// pre-M378 parrot (no sitting, still legs and tail) 0 and 0.
const PARROT_MIN: [usize; 2] = [500, 40];
/// M379 — m97's floors: the drinking witch's nose (pixels changed), the
/// enderman's arms. M379 measured 144 and 772; the pre-M379 mutant 0 and 0.
const WITCH_ENDERMAN_MIN: [usize; 2] = [70, 380];
/// M376 — m94's floor (a breath). M376 measured 289 px; a still ribcage
/// and tail (the pre-M376 mutant) 0.
const WITHER_MIN: usize = 140;
/// M377 — m95's floor for the tadpole's tail. M377 measured 26 px; the
/// pre-M377 mutant 0 (and the camel, breeze and bat 409, 441 and 107 px
/// where m95 wants 0).
const TADPOLE_MIN: usize = 12;
/// M374 — m92's floor (the standing sway). M374 measured 361 px (and 0
/// between speeds 0.2 and 1.0); the pre-M374 warden (the biped walk,
/// unclamped, no sway) 0 (and 1957).
const WARDEN_MIN: usize = 180;
/// M375 — m93's band for the ravager's stride (px). M375 measured 1413; the
/// quadruped's full 1.4 swing (the pre-M375 mutant) 2273.
const RAVAGER_STRIDE: [usize; 2] = [700, 1800];
/// M372 — m90's floor (the 10° look). M372 measured 304 px (and 0 between
/// 30° and 10°); a still body (the pre-M372 mutant) 0.
const NAUTILUS_MIN: usize = 150;
/// M373 — m91's floor (a second of tentacle swing, seen from below). M373
/// measured 41 px; still tentacles (the pre-M373 mutant) 0.
const HAPPY_GHAST_MIN: usize = 20;
/// M370 — m88's floors: the look, the tail a second on. M370 measured 2521
/// and 384; a still body and tail (the pre-M370 mutant) 0 and 17 (the
/// spikes' wobble alone).
const GUARDIAN_MIN: [usize; 2] = [1200, 150];
/// M371 — m89's floors: the look, swimming vs still. M371 measured 1423 and
/// 737; a still body (the pre-M371 mutant, its tail stroking) 0 and 83.
const DOLPHIN_MIN: [usize; 2] = [700, 350];
/// M368 — m86's floor. M368 measured 245 px; the head turning alone (the
/// pre-M368 mutant, a still upper body and arms) 23.
const SNOW_GOLEM_MIN: usize = 120;
/// M369 — m87's floors: flying vs hovering, holding vs empty-handed. M369
/// measured 2008 and 414; the wings alone (the pre-M369 mutant, a still body
/// and arms) 1454 and 0.
const ALLAY_MIN: [usize; 2] = [1700, 200];
/// M367 — m85's floors: the headbutt, the walk. M367 measured 1090 and
/// 778 (and 0 px for the pitch); a head that pitches with the look and no
/// headbutt (the pre-M367 mutant) read 485 px for the pitch and 0.
const HOGLIN_MIN: [usize; 2] = [500, 350];
/// M366 — m84's floors: stride vs standing, the idle bristle sway. M366
/// measured 578 and 118; the pre-M366 strider (legs on the generic 1.4
/// walk, unclamped; body and bristles still) 788 and 0, which is why the
/// stride is graded by its clamp (1.0 vs 0.25: 0 px) rather than its size.
const STRIDER_MIN: [usize; 2] = [300, 60];
/// M364 — m82's floor (px of silhouette the look moves): adult, kit. M364
/// measured 307 and 750; a still adult head (the pre-M364 mutant) 0.
const RABBIT_LOOK_MIN: [usize; 2] = [150, 300];
/// M365 — m83's floors: stride vs standing, swim vs land, egg vs none. M365
/// measured 2012, 1682 and 5787; static flippers and an always-drawn belly
/// (the pre-M365 mutant) 0, 0 and 0.
const TURTLE_MIN: [usize; 3] = [1000, 800, 2500];

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
///
/// M329 re-measured all four at the crowd's 2400 x 1350 (see `lw`/`lh`).
/// At 1600 x 900 every new kind had begun moving other kinds between the
/// buckets: the camera backs off by a row's depth per ten kinds, so each row
/// shrank every cell, and M321 and M328 each pushed a kind that still drew
/// fine under the pixel floor. The bigger frame grades 104 kinds where the
/// smaller graded 91.
const MIN_GRADED: usize = 104;
/// M334: 88, the adult donkey's render now explained by the foal's sheet.
const MIN_DISCRIMINATING: usize = 88;
/// Kinds too small to grade in the 2400 x 1350 crowd, measured at M329: fox,
/// tadpole, cod, tropical_fish x2, minecart and armor_stand (mobshot grades
/// the last two from three views), and the babies under a quarter block or
/// thin enough to lose their attributed pixels to their neighbours: turtle,
/// husk, villager, drowned, piglin, piglet, chick, dolphin calf, zombie
/// villager and glow squid, (M330) the kit, (M332) the ocelot kitten,
/// (M333) the baby bee, (M336) the rabbit kit, (M337) the goat kid, (M340)
/// the kitten and (M341) the puppy. Each baby is graded up close by its own
/// witness (m42..m66, n1/n2). M349: the cold chicken (graded up close by
/// m74); M350: the coral zombie nautilus, mostly hidden by its neighbours in
/// the crowd (graded up close by m75). A twenty-seventh means something
/// stopped drawing.
const MAX_SMALL: usize = 26;
/// Kinds whose whole render some *other* single sheet could also explain,
/// measured at M329: skeleton/skeleton_horse, enderman/ender_dragon,
/// piglin/piglin_brute, ghast and happy_ghast/each other's sheets,
/// boat/chest_boat and raft/chest_raft (one wood sheet on two models), and the
/// adult/baby pairs whose baby sheet repaints the adult's palette: dolphin,
/// polar bear (both ways), squid (both ways), cow_baby, sheep_baby,
/// strider_baby, (M331) panda_baby and (M334) donkey (both ways) and
/// mule_baby. The imposter named for a kind can change between runs (the
/// sheets are a hash map; any one that explains it is reported), the count
/// does not. Sound but not discriminating, so the count is pinned rather than
/// left to drift.
const MAX_AMBIGUOUS: usize = 19;

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

/// The gate runs on its own thread with a 64 MiB stack. `run_check` is one
/// long function and a debug build gives every witness's `Stage` (a whole
/// `WorldRenderer` by value) its own slot, so by m47 (M321) its frame
/// overflowed the 1 MiB Windows main thread.
pub fn run(args: MobtexshotArgs) -> Result<(), String> {
    std::thread::Builder::new()
        .name("mobtexshot".into())
        .stack_size(64 << 20)
        .spawn(move || run_gate(args))
        .map_err(|e| format!("mobtexshot: spawn: {e}"))?
        .join()
        .map_err(|_| "mobtexshot: the gate thread panicked".to_string())?
}

fn run_gate(args: MobtexshotArgs) -> Result<(), String> {
    if !args.check {
        return Err("mobtexshot: pass --check (this command is a gate)".into());
    }
    let jar = client_jar(&args.version).ok_or("client jar not found")?;
    let paths = DataPaths::for_version(&args.version).ok_or("no config dir")?;
    let baked = assets::bake(&jar, &paths.blocks_json())?;
    // Every build requests validation (see mobshot). `--check` is mandatory
    // here, so a requested-but-inactive layer must fail.
    let want_validation = !args.no_validation;
    let mut gpu = Gpu::new(None, want_validation)?;
    println!(
        "[mobtexshot] Vulkan validation: {}",
        if gpu.validation_active { "ON" } else { "off" }
    );
    if want_validation && !gpu.validation_active {
        return Err("mobtexshot: Vulkan validation requested but not active".into());
    }
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
    for px in rgba.as_chunks::<4>().0 {
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
    for px in rgba.as_chunks::<4>().0 {
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
        A::PulsatingSpots { phase } => ((0.0f32 * 0.045 + phase).cos() * 0.25).max(0.0),
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
        stand_parts: Default::default(),
        body_hidden: false,
        worn_head: None,
        white_overlay: 0.0,
        stretch: [1.0, 1.0],
        outline: None,
        powered: false,
        body_armor: None,
        saddle: None,
        ridden: false,
        chested: false,
        avatar_hidden: 0,
        body_armor_crack: None,
        leash_holder: false,
        on_fire: None,
        billboard: None,
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
        collar: None,
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
    fn shoot(
        &mut self,
        gpu: &mut Gpu,
        specs: &[Spec],
        skip: Option<usize>,
    ) -> Result<Vec<u8>, String> {
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
        self.off
            .render(gpu, Some((&mut self.wr, self.vp)), &ov, BG)?;
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
            // M306 — the sheets its `entityTranslucent` quads sample, read off
            // the model's own marker (no mob is named).
            let model = (d.build)();
            let mut translucent: Vec<&'static str> = Vec::new();
            if let Some(from) = model.translucent_from {
                for q in &model.quads[from..] {
                    let k = d.textures[q.tex];
                    if !translucent.contains(&k) {
                        translucent.push(k);
                    }
                }
            }
            (
                d.kind,
                OwnSheets {
                    keys,
                    blends,
                    translucent,
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
    // M329: 2400 x 1350, not 1600 x 900. The camera backs off by a row per
    // ten kinds, so a bigger cast shrinks every cell; at the smaller frame
    // new kinds had begun pushing others under `MIN_PIXELS`.
    let (lw, lh) = (2400u32, 1350u32);
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
        let under = attributed(&wo, &full);
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
        if let Some(c) = first_unexplained_over(&px, Some(&under), keys, &sheets, &lin) {
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
            let rel = s
                .adult_path
                .strip_prefix("textures/")
                .unwrap_or(s.adult_path);
            rewo_data::assets::mob_texture_specs()
                .any(|(_, p, w, h)| p == rel && w == s.w && h == s.h)
        })
        .count();
    // The sheets the same-size swaps baked (M314's ghastling sheet is baked
    // as its own model's texture, by no swap; M317's baby zombie and husk
    // sheets are still baked by their swaps, now for their own models).
    let baked_babies = baked
        .mob_textures
        .iter()
        .filter(|t| {
            rewo_data::baby_texture_table::BABY_SWAPS.iter().any(|s| {
                s.baby_key == t.key && {
                    let rel = s
                        .adult_path
                        .strip_prefix("textures/")
                        .unwrap_or(s.adult_path);
                    rewo_data::assets::mob_texture_specs()
                        .any(|(_, p, w, h)| p == rel && w == s.w && h == s.h)
                }
            })
        })
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
    // M317 — the baby is its own model (`BabyZombieModel`), which is what
    // the client now draws for one; until M317 this was the adult mesh with
    // the baby sheet's UV offset, which scrambled the sheet and still passed
    // n1/n2 (the colours were the right sheet's).
    let zb = row(EntityModelKind::ZombieBaby, 0);
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
    let adult_set = sheets.get("zombie").ok_or("zombie not baked")?;
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
            adult_px
                .iter()
                .filter(|c| !explains(adult_set, **c))
                .count()
        ),
    ));
    // n4 — the DEFERRED half is deliberate: a size-mismatched swap (happy
    // ghast's 64² baby against a 128² adult) is NOT baked as a swap, because
    // offsetting UVs across different sheet sizes samples the wrong texels.
    // M314 built the BABY model layer that sheet belongs to, so the sheet is
    // in the atlas now, as the ghastling kind's own texture at its own size,
    // and no swap offset rides the adult.
    let ghastling_own = baked
        .mob_textures
        .iter()
        .any(|t| t.key == "happy_ghast_baby" && t.w == 64 && t.h == 64);
    w.push(wit(
        "n4.size_mismatched_baby_sheets_stay_unbaked_on_purpose",
        ghastling_own && baked.baby_swap_skips > 0,
        format!(
            "happy_ghast_baby baked at 64x64 for its own model: {ghastling_own}, swaps skipped \
             {}",
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
        for (n, px) in v.as_chunks_mut::<4>().0.iter_mut().enumerate() {
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
            if over == origins[0] {
                "recycled"
            } else {
                "did NOT recycle"
            },
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

    // m18 — M273: a skeleton skull worn by an armour stand
    // (`CustomHeadLayer`'s skull branch). Every pixel it adds must be a
    // colour the jar's skull sheet can produce, and their centroid must sit
    // on the skull's centre as the decompile's literals place it: the stand's
    // head pivot is `PartPose.offset(0, 1, 0)` px under the model origin
    // (`translate(0, -1.501, 0)` after `scale(-1, -1, 1)`), and the skull's
    // 8 px cube (`y -8..0`) is scaled by 1.1875, so its centre is world
    // `(0, 1.501 - 1/16 + 4 * 1.1875 / 16, 0)`.
    const SKULL: &str = "rewo:be/skeleton_skull";
    let (sw, sh) = (128u32, 128u32);
    let mut st = stage_from(
        gpu,
        baked,
        sw,
        sh,
        Vec3::new(0.0, 1.735, 2.5),
        Vec3::new(0.0, 1.735, 0.0),
    )?;
    st.wr
        .set_held_items(crate::live_cmd::to_gpu_held_items(&baked.held_items));
    st.wr.prepare_held_items(gpu, &[SKULL])?;
    let ring = OverlayRing::default();
    let ov = overlay_offscreen(&ring);
    const PUMPKIN: &str = "minecraft:carved_pumpkin";
    st.wr.prepare_held_items(gpu, &[PUMPKIN])?;
    // M281 — the flames' two frames at animation time 0.
    let fire_frames = [16usize, 0];
    let fire_names: Vec<String> = (0..2)
        .map(|s| rewo_gpu::entities::fire_model_name(s, fire_frames[s]))
        .collect();
    let fire_refs: Vec<&str> = fire_names.iter().map(String::as_str).collect();
    st.wr.prepare_held_items(gpu, &fire_refs)?;
    let mut shot = |d: EntityDraw<'_>| -> Result<Vec<u8>, String> {
        st.wr.set_entities(&[d], st.right, st.up, 0.0);
        st.off.render(gpu, Some((&mut st.wr, st.vp)), &ov, BG)?;
        st.off.read_rgba(gpu)
    };
    let bare = shot(neutral_draw(EntityModelKind::ArmorStand))?;
    let worn = shot(EntityDraw {
        worn_head: Some(rewo_gpu::entities::WornHead {
            model: SKULL,
            item: false,
            y_offset: 0.0,
            skull_y_offset: 0.0,
            horizontal_scale: 1.0,
        }),
        ..neutral_draw(EntityModelKind::ArmorStand)
    })?;
    // m19 — M274: a carved pumpkin worn the same way, `CustomHeadLayer`'s
    // item branch: `translateToHead` puts the item's centre at
    // `(0, -0.25, 0)` head-local (the identity `head` transform of a block
    // item, `-0.5` centring), so world `(0, 1.501 - 1/16 + 0.25, 0)`, the
    // cube 0.625 wide.
    let pumpkin = shot(EntityDraw {
        worn_head: Some(rewo_gpu::entities::WornHead {
            model: PUMPKIN,
            item: true,
            y_offset: 0.0,
            skull_y_offset: 0.0,
            horizontal_scale: 1.0,
        }),
        ..neutral_draw(EntityModelKind::ArmorStand)
    })?;
    // m20 — M281: the stand burning. The flames are the two frames' texels
    // drawn at full light (the gate's light is `[1, 1, 1]`, block light 15
    // changes nothing here): every pixel they add must be one their textures
    // can produce, and there must be plenty of them, rising over the stand.
    let burning = shot(EntityDraw {
        powered: false,
        body_armor: None,
        saddle: None,
        ridden: false,
        chested: false,
        avatar_hidden: 0,
        body_armor_crack: None,
        leash_holder: false,
        on_fire: Some(rewo_gpu::entities::FireDraw {
            light: [1.0; 3],
            frames: fire_frames,
        }),
        ..neutral_draw(EntityModelKind::ArmorStand)
    })?;
    let fire_sheet: HashSet<u32> = fire_names
        .iter()
        .filter_map(|n| baked.held_items.any(n))
        .filter_map(|m| m.quads.first())
        .filter_map(|q| baked.held_items.textures.get(q.tex as usize))
        .flat_map(|t| producible(&t.rgba))
        .collect();
    let (mut fire_n, mut fire_bad) = (0usize, None);
    for (a, b) in burning
        .as_chunks::<4>()
        .0
        .iter()
        .zip(bare.as_chunks::<4>().0)
    {
        if a[..3] == b[..3] {
            continue;
        }
        fire_n += 1;
        let c = pack(a[0], a[1], a[2]);
        if fire_bad.is_none() && !explains(&fire_sheet, c) {
            fire_bad = Some(c);
        }
    }
    let skull_sheet: HashSet<u32> = baked
        .held_items
        .any(SKULL)
        .and_then(|m| m.quads.first())
        .and_then(|q| baked.held_items.textures.get(q.tex as usize))
        .map(|t| producible(&t.rgba))
        .unwrap_or_default();
    let (mut n, mut sx, mut sy, mut unexplained) = (0usize, 0f64, 0f64, None);
    for (i, (a, b)) in worn
        .as_chunks::<4>()
        .0
        .iter()
        .zip(bare.as_chunks::<4>().0)
        .enumerate()
    {
        if a[..3] == b[..3] {
            continue;
        }
        n += 1;
        sx += (i as u32 % sw) as f64 + 0.5;
        sy += (i as u32 / sw) as f64 + 0.5;
        let c = pack(a[0], a[1], a[2]);
        if unexplained.is_none() && !explains(&skull_sheet, c) {
            unexplained = Some(c);
        }
    }
    let centre = {
        let c = Mat4::from_cols_array_2d(&st.vp)
            * glam::Vec4::new(0.0, 1.501 - 1.0 / 16.0 + 4.0 * 1.1875 / 16.0, 0.0, 1.0);
        (
            (c.x / c.w * 0.5 + 0.5) * sw as f32,
            (0.5 - c.y / c.w * 0.5) * sh as f32,
        )
    };
    let got = (sx / n.max(1) as f64, sy / n.max(1) as f64);
    let off = ((got.0 - centre.0 as f64).powi(2) + (got.1 - centre.1 as f64).powi(2)).sqrt();
    // The same grading for the pumpkin, against every texture its quads
    // sample.
    let pumpkin_sheets: HashSet<u32> = baked
        .held_items
        .get(PUMPKIN)
        .map(|m| {
            let mut texs: Vec<u16> = m.quads.iter().map(|q| q.tex).collect();
            texs.sort_unstable();
            texs.dedup();
            texs.iter()
                .filter_map(|&t| baked.held_items.textures.get(t as usize))
                .flat_map(|t| producible(&t.rgba))
                .collect()
        })
        .unwrap_or_default();
    let (mut pn, mut px, mut py, mut p_unexplained) = (0usize, 0f64, 0f64, None);
    for (i, (a, b)) in pumpkin
        .as_chunks::<4>()
        .0
        .iter()
        .zip(bare.as_chunks::<4>().0)
        .enumerate()
    {
        if a[..3] == b[..3] {
            continue;
        }
        pn += 1;
        px += (i as u32 % sw) as f64 + 0.5;
        py += (i as u32 / sw) as f64 + 0.5;
        let c = pack(a[0], a[1], a[2]);
        if p_unexplained.is_none() && !explains(&pumpkin_sheets, c) {
            p_unexplained = Some(c);
        }
    }
    let p_centre = {
        let c = Mat4::from_cols_array_2d(&st.vp)
            * glam::Vec4::new(0.0, 1.501 - 1.0 / 16.0 + 0.25, 0.0, 1.0);
        (
            (c.x / c.w * 0.5 + 0.5) * sw as f32,
            (0.5 - c.y / c.w * 0.5) * sh as f32,
        )
    };
    let p_got = (px / pn.max(1) as f64, py / pn.max(1) as f64);
    let p_off =
        ((p_got.0 - p_centre.0 as f64).powi(2) + (p_got.1 - p_centre.1 as f64).powi(2)).sqrt();
    st.destroy(gpu);
    w.push(wit(
        "m18.a_worn_skull_draws_on_the_armour_stands_head_in_its_own_sheet",
        !skull_sheet.is_empty() && n >= 300 && unexplained.is_none() && off <= 1.0,
        format!(
            "{n} px added (want >= 300); centroid ({:.1}, {:.1}) vs the skull's centre \
             ({:.1}, {:.1}): {off:.2} px (want <= 1); first colour the skull sheet \
             cannot produce: {:?}",
            got.0,
            got.1,
            centre.0,
            centre.1,
            unexplained.map(|c| format!("#{c:06x}"))
        ),
    ));

    w.push(wit(
        "m19.a_worn_pumpkin_draws_on_the_armour_stands_head_in_its_own_textures",
        !pumpkin_sheets.is_empty() && pn >= 300 && p_unexplained.is_none() && p_off <= 1.0,
        format!(
            "{pn} px added (want >= 300); centroid ({:.1}, {:.1}) vs the item's centre \
             ({:.1}, {:.1}): {p_off:.2} px (want <= 1); first colour its textures \
             cannot produce: {:?}",
            p_got.0,
            p_got.1,
            p_centre.0,
            p_centre.1,
            p_unexplained.map(|c| format!("#{c:06x}"))
        ),
    ));

    w.push(wit(
        "m20.a_burning_stand_draws_its_flames_in_their_own_textures",
        fire_names.len() == 2 && !fire_sheet.is_empty() && fire_n >= 300 && fire_bad.is_none(),
        format!(
            "{fire_n} px added by the flames (want >= 300); first colour the two fire \
             frames cannot produce: {:?}",
            fire_bad.map(|c| format!("#{c:06x}"))
        ),
    ));

    // m21 — M285: a charged creeper's energy swirl. `ENERGY_SWIRL` adds
    // `texel · 0xFF808080` (`BlendFunction.ADDITIVE`) in vanilla's gamma
    // space, one layer per shell face the depth test lets through: no
    // culling, and `sortOnUpload` draws far quads first, so over the bare sky
    // a closed shell adds its back face AND its front face, while over the
    // body the body hides the back faces.
    //
    // Part one, the jar's `creeper_armor.png`: the swirl draws, never darkens
    // a pixel, reaches past the body onto the bare sky (the shell is inflated
    // 2 px; an empty frame says where the sky is) and scrolls with
    // `ageInTicks`.
    //
    // Part two, exactness, through the same production upload with a
    // uniform sheet of one colour `u` (the jar's texels are a self-similar
    // blue ramp, under which a doubled swirl looks like a brighter texel): a
    // layer adds exactly `r = round(u · 128 / 255)`, so every unsaturated
    // pixel must rise by `k · r` (±k; up to six where the leg, body and head
    // shells overlap), a body pixel mostly with `k = 1`, and every
    // sky pixel by `k ≥ 2` (graded on red and green, the sky's blue being
    // saturated). A white vertex colour gives `2r` everywhere, a linear-space
    // add a rise that depends on the pixel under it, a face shade a scaled
    // one, and a missing sort single layers on the sky.
    let Some(armor) = baked.creeper_armor.as_ref() else {
        return Err("mobtexshot: the jar has no creeper_armor.png".into());
    };
    const SWIRL_U: [u8; 3] = [60, 30, 10];
    let r1 = SWIRL_U.map(|c| (c as f32 * 128.0 / 255.0).round() as i32);
    let uniform: Vec<u8> = (0..armor.w * armor.h)
        .flat_map(|_| [SWIRL_U[0], SWIRL_U[1], SWIRL_U[2], 255])
        .collect();
    let (cw, ch) = (128u32, 128u32);
    let ring = OverlayRing::default();
    let ov = overlay_offscreen(&ring);
    let charged = || EntityDraw {
        powered: true,
        ..neutral_draw(EntityModelKind::Creeper)
    };
    // (empty, plain, charged at t = 0, charged at t = 27.4, the swirl's
    // vertex counts plain and charged), for one sheet.
    type Frames = (Vec<u8>, Vec<u8>, Vec<u8>, Vec<u8>, u32, u32);
    let mut frames = |sheet: &[u8]| -> Result<Frames, String> {
        let mut st = stage_from(
            gpu,
            baked,
            cw,
            ch,
            Vec3::new(1.2, 1.3, 2.4),
            Vec3::new(0.0, 0.85, 0.0),
        )?;
        st.wr.init_entity_swirl(
            gpu,
            rewo_gpu::entities::SwirlSheet::Creeper,
            sheet,
            armor.w,
            armor.h,
        )?;
        let mut shot = |d: &[EntityDraw<'_>], time: f32| -> Result<(Vec<u8>, u32), String> {
            st.wr.set_entities(d, st.right, st.up, time);
            st.off.render(gpu, Some((&mut st.wr, st.vp)), &ov, BG)?;
            Ok((
                st.off.read_rgba(gpu)?,
                st.wr
                    .swirl_vertex_count(rewo_gpu::entities::SwirlSheet::Creeper),
            ))
        };
        let (empty, _) = shot(&[], 0.0)?;
        let (plain, plain_n) = shot(&[neutral_draw(EntityModelKind::Creeper)], 0.0)?;
        let (lit, lit_n) = shot(&[charged()], 0.0)?;
        let (later, _) = shot(&[charged()], 1.37)?;
        st.destroy(gpu);
        Ok((empty, plain, lit, later, plain_n, lit_n))
    };
    let px = |v: &[u8]| -> Vec<[i32; 3]> {
        v.as_chunks::<4>()
            .0
            .iter()
            .map(|p| [p[0] as i32, p[1] as i32, p[2] as i32])
            .collect()
    };
    // Part one.
    let (empty, plain, lit, later, plain_n, lit_n) = frames(&armor.rgba)?;
    let (empty, plain, lit, later) = (px(&empty), px(&plain), px(&lit), px(&later));
    let (mut touched, mut outside, mut darker) = (0usize, 0usize, 0usize);
    for i in 0..lit.len() {
        if lit[i] == plain[i] {
            continue;
        }
        touched += 1;
        darker += (0..3).any(|c| lit[i][c] + 1 < plain[i][c]) as usize;
        outside += (plain[i] == empty[i]) as usize;
    }
    let scrolled = (0..lit.len()).filter(|&i| lit[i] != later[i]).count();
    // Part two.
    let (u_empty, u_plain, u_lit, _, _, _) = frames(&uniform)?;
    let (u_empty, u_plain, u_lit) = (px(&u_empty), px(&u_plain), px(&u_lit));
    let layers = |d: [i32; 3], chans: usize| {
        (1..=6).find(|&k| (0..chans).all(|c| (d[c] - k * r1[c]).abs() <= k))
    };
    let (mut body, mut body_single, mut sky, mut sky_single, mut bad) =
        (0usize, 0usize, 0usize, 0usize, None);
    for i in 0..u_lit.len() {
        if u_lit[i] == u_plain[i] {
            continue;
        }
        let d = [0, 1, 2].map(|c| u_lit[i][c] - u_plain[i][c]);
        let on_sky = u_plain[i] == u_empty[i];
        let chans = if on_sky { 2 } else { 3 };
        if (0..chans).any(|c| u_lit[i][c] == 255) {
            continue;
        }
        let k = layers(d, chans);
        if k.is_none() && bad.is_none() {
            bad = Some((d, on_sky));
        }
        if on_sky {
            sky += 1;
            sky_single += (k == Some(1)) as usize;
        } else {
            body += 1;
            body_single += (k == Some(1)) as usize;
        }
    }
    w.push(wit(
        "m21.a_charged_creeper_adds_its_scrolling_swirl_in_gamma_space",
        plain_n == 0
            && lit_n > 0
            && darker == 0
            && outside >= 50
            && scrolled >= 50
            && bad.is_none()
            && body >= 200
            && body_single * 2 >= body
            && sky >= 50
            && sky_single == 0,
        format!(
            "jar sheet: swirl vertices plain {plain_n} / charged {lit_n}, {touched} px \
             touched, {darker} darkened (want 0), {outside} on the bare sky (want >= 50), \
             {scrolled} changed after 1.37 s (want >= 50); uniform sheet {SWIRL_U:?} (a \
             layer adds {r1:?}): {body} body px graded (want >= 200), {body_single} single \
             layers (want >= half), {sky} sky px graded (want >= 50), {sky_single} single \
             layers (want 0, the back face adds too), first rise no layer count explains \
             (rise, on the sky): {bad:?}"
        ),
    ));

    // m22 — M286: the breeze's wind, `RenderPipelines.BREEZE_WIND`: a
    // TRANSLUCENT blend (`SRC_ALPHA, ONE_MINUS_SRC_ALPHA`) in vanilla's gamma
    // space of the texel times the lightmap (white here), one layer per
    // funnel face the depth test lets through (no culling, far quads first).
    // Graded through the production upload with a uniform sheet `u` at alpha
    // `a`: a layer takes a pixel `p` to `round(u · a + p · (1 - a))`, so every
    // pixel the wind touches must be that applied one to eight times to the
    // frame without the wind (±1 a layer; the funnel is concentric shells,
    // each with a front and a back face), with plenty at one layer and at
    // two. Sorted far to near, a face behind a nearer one always draws first,
    // so single layers happen only at the funnel's edges: at most a tenth.
    // An opaque wind (the body geometry it was before M286) gives the sheet
    // colour everywhere, explained only by many layers; an unsorted funnel
    // gives four in ten single layers; a linear-space blend misses the
    // formula. The frame without the wind is the same
    // breeze on a stage with no wind sheet, whose layer then draws nothing.
    let Some(wind_tex) = crate::live_cmd::breeze_wind_texture(baked) else {
        return Err("mobtexshot: the jar has no breeze_wind.png".into());
    };
    const WIND_U: [u8; 4] = [220, 120, 40, 128];
    let wind_sheet: Vec<u8> = (0..wind_tex.w * wind_tex.h).flat_map(|_| WIND_U).collect();
    let wind_frame = |gpu: &mut Gpu, sheet: Option<&[u8]>| -> Result<(Vec<u8>, u32), String> {
        let mut st = stage_from(
            gpu,
            baked,
            cw,
            ch,
            Vec3::new(1.0, 1.4, 2.6),
            Vec3::new(0.0, 0.8, 0.0),
        )?;
        if let Some(sheet) = sheet {
            st.wr.init_entity_wind(gpu, sheet, wind_tex.w, wind_tex.h)?;
        }
        st.wr.set_entities(
            &[neutral_draw(EntityModelKind::Breeze)],
            st.right,
            st.up,
            0.0,
        );
        st.off.render(gpu, Some((&mut st.wr, st.vp)), &ov, BG)?;
        let img = st.off.read_rgba(gpu)?;
        let n = st.wr.wind_vertex_count();
        st.destroy(gpu);
        Ok((img, n))
    };
    let (bare, _) = wind_frame(gpu, None)?;
    let (windy, wind_n) = wind_frame(gpu, Some(&wind_sheet))?;
    let a = WIND_U[3] as f32 / 255.0;
    let layer = |p: [f32; 3]| [0, 1, 2].map(|c| (WIND_U[c] as f32 * a + p[c] * (1.0 - a)).round());
    let (mut wind_px, mut wind_bad) = (0usize, None);
    let mut by_k = [0usize; 8];
    for (w2, b) in windy.as_chunks::<4>().0.iter().zip(bare.as_chunks::<4>().0) {
        if w2[..3] == b[..3] {
            continue;
        }
        wind_px += 1;
        let got = [0, 1, 2].map(|c| w2[c] as f32);
        let mut p = [0, 1, 2].map(|c| b[c] as f32);
        let mut k_found = None;
        for k in 1..=8 {
            p = layer(p);
            if (0..3).all(|c| (got[c] - p[c]).abs() <= k as f32) {
                k_found = Some(k);
                break;
            }
        }
        match k_found {
            Some(k) => by_k[k - 1] += 1,
            None if wind_bad.is_none() => {
                wind_bad = Some(([b[0], b[1], b[2]], [w2[0], w2[1], w2[2]]))
            }
            None => {}
        }
    }
    w.push(wit(
        "m22.a_breeze_blends_its_wind_as_vanilla_does",
        wind_n > 0
            && wind_px >= 300
            && wind_bad.is_none()
            && by_k[0] + by_k[1] >= 300
            && by_k[0] * 10 <= wind_px,
        format!(
            "wind vertices {wind_n}; uniform sheet {WIND_U:?}: {wind_px} px touched (want \
             >= 300), by layer count 1..8 {by_k:?} (want >= 300 at one or two, an \
             opaque wind reaching the sheet colour only after many; and at most a tenth \
             at one, the far faces drawing first), first pixel no \
             layer count explains (under, got): {wind_bad:?}"
        ),
    ));

    // m23 — M287: the powered wither's armour swirl (`WitherArmorLayer`, the
    // same `ENERGY_SWIRL` as m21's) on its OWN sheet. A uniform sheet goes to
    // the wither's slot alone (the creeper's stays empty): a powered wither
    // must add exactly `k · round(u · 128 / 255)` (mostly one layer over the
    // body) and an unpowered one nothing, so a wither sub-range drawn with
    // the creeper's sheet, or not drawn, fails.
    let Some(warmor) = baked.wither_armor.as_ref() else {
        return Err("mobtexshot: the jar has no wither_armor.png".into());
    };
    const WITHER_U: [u8; 3] = [20, 70, 40];
    let wr1 = WITHER_U.map(|c| (c as f32 * 128.0 / 255.0).round() as i32);
    let wither_sheet: Vec<u8> = (0..warmor.w * warmor.h)
        .flat_map(|_| [WITHER_U[0], WITHER_U[1], WITHER_U[2], 255])
        .collect();
    let mut wst = stage_from(
        gpu,
        baked,
        cw,
        ch,
        Vec3::new(2.0, 3.2, 5.0),
        Vec3::new(0.0, 2.6, 0.0),
    )?;
    wst.wr.init_entity_swirl(
        gpu,
        rewo_gpu::entities::SwirlSheet::Wither,
        &wither_sheet,
        warmor.w,
        warmor.h,
    )?;
    let mut wshot = |d: EntityDraw<'_>| -> Result<(Vec<u8>, u32), String> {
        wst.wr.set_entities(&[d], wst.right, wst.up, 0.0);
        wst.off.render(gpu, Some((&mut wst.wr, wst.vp)), &ov, BG)?;
        Ok((
            wst.off.read_rgba(gpu)?,
            wst.wr
                .swirl_vertex_count(rewo_gpu::entities::SwirlSheet::Wither),
        ))
    };
    let (w_plain, w_plain_n) = wshot(neutral_draw(EntityModelKind::Wither))?;
    let (w_lit, w_lit_n) = wshot(EntityDraw {
        powered: true,
        ..neutral_draw(EntityModelKind::Wither)
    })?;
    wst.destroy(gpu);
    let (mut w_body, mut w_single, mut w_touched, mut w_bad) = (0usize, 0usize, 0usize, None);
    for (a, b) in w_lit
        .as_chunks::<4>()
        .0
        .iter()
        .zip(w_plain.as_chunks::<4>().0)
    {
        if a[..3] == b[..3] {
            continue;
        }
        w_touched += 1;
        if a[..3].contains(&255) {
            continue;
        }
        let d = [0, 1, 2].map(|c| a[c] as i32 - b[c] as i32);
        match (1..=6).find(|&k| (0..3).all(|c| (d[c] - k * wr1[c]).abs() <= k)) {
            Some(k) => {
                w_body += 1;
                w_single += (k == 1) as usize;
            }
            None if w_bad.is_none() => w_bad = Some(d),
            None => {}
        }
    }
    w.push(wit(
        "m23.a_powered_wither_adds_its_swirl_from_its_own_sheet",
        w_plain_n == 0 && w_lit_n > 0 && w_bad.is_none() && w_body >= 200 && w_single * 2 >= w_body,
        format!(
            "wither swirl vertices plain {w_plain_n} / powered {w_lit_n}; {w_touched} px \
             touched, {w_body} graded (want >= 200) of which {w_single} single layers of \
             {wr1:?} (want >= half); first rise no layer count explains: {w_bad:?}"
        ),
    ));

    // m24 — M293: the equines' layer scales reach the screen. The horse and
    // the zombie horse share one mesh (the skeleton horse's sheet is
    // see-through between its bones, so its silhouette is not the mesh's), as
    // do the donkey and the mule, so side-on from far away
    // (near-orthographic) each pair's silhouette heights must stand in
    // vanilla's ratio: 1.1 (`livingHorseScale`) and 0.92 / 0.87
    // (`DonkeyModel.createBodyLayer`). All four drew at one size until M293.
    let (ew, eh) = (512u32, 512u32);
    let mut est = stage_from(
        gpu,
        baked,
        ew,
        eh,
        Vec3::new(9.0, 1.1, 0.0),
        Vec3::new(0.0, 1.1, 0.0),
    )?;
    let mut eshot = |d: &[EntityDraw<'_>]| -> Result<Vec<u8>, String> {
        est.wr.set_entities(d, est.right, est.up, 0.0);
        est.off.render(gpu, Some((&mut est.wr, est.vp)), &ov, BG)?;
        est.off.read_rgba(gpu)
    };
    let empty_eq = eshot(&[])?;
    let mut heights = Vec::new();
    for kind in [
        EntityModelKind::Horse,
        EntityModelKind::ZombieHorse,
        EntityModelKind::Mule,
        EntityModelKind::Donkey,
    ] {
        let img = eshot(&[neutral_draw(kind)])?;
        let rows: Vec<u32> = img
            .as_chunks::<4>()
            .0
            .iter()
            .zip(empty_eq.as_chunks::<4>().0)
            .enumerate()
            .filter(|(_, (a, b))| a[..3] != b[..3])
            .map(|(i, _)| i as u32 / ew)
            .collect();
        let h = match (rows.iter().min(), rows.iter().max()) {
            (Some(lo), Some(hi)) => (hi - lo + 1) as f32,
            _ => 0.0,
        };
        heights.push(h);
    }
    est.destroy(gpu);
    let horse_ratio = heights[0] / heights[1].max(1.0);
    let mule_ratio = heights[2] / heights[3].max(1.0);
    let want_mule = 0.92 / 0.87;
    w.push(wit(
        "m24.the_equines_draw_at_vanillas_layer_scales",
        heights.iter().all(|&h| h >= 30.0)
            && (horse_ratio - 1.1).abs() <= 0.03
            && (mule_ratio - want_mule).abs() <= 0.03,
        format!(
            "silhouette heights horse / zombie horse / mule / donkey {heights:?} px; horse \
             / zombie {horse_ratio:.3} (want 1.1 +- 0.03), mule / donkey {mule_ratio:.3} \
             (want {want_mule:.3} +- 0.03)"
        ),
    ));

    // m25 — M294: a horse in iron horse armour (`HORSE_BODY`'s
    // `SimpleEquipmentLayer` over the 0.1 shell). Every pixel the armour
    // changes must be a colour the jar's `horse_body/iron` sheet can produce
    // (the gate's light is full), there must be plenty of them, and the sheet
    // must be resident in the atlas. No horse armour drew until M294.
    const IRON: &str = "horse_body/iron";
    let iron_sheet: HashSet<u32> = baked
        .equipment
        .textures
        .iter()
        .find(|t| t.key == IRON)
        .map(|t| producible(&t.rgba))
        .unwrap_or_default();
    let mut ast = stage_from(
        gpu,
        baked,
        cw,
        ch,
        Vec3::new(3.2, 1.8, 2.2),
        Vec3::new(0.0, 1.0, 0.0),
    )?;
    let resident = ast
        .wr
        .entity_pass()
        .is_some_and(|p| p.armor_sheet_resident(IRON));
    let mut ashot = |d: EntityDraw<'_>| -> Result<Vec<u8>, String> {
        ast.wr.set_entities(&[d], ast.right, ast.up, 0.0);
        ast.off.render(gpu, Some((&mut ast.wr, ast.vp)), &ov, BG)?;
        ast.off.read_rgba(gpu)
    };
    let bare_horse = ashot(neutral_draw(EntityModelKind::Horse))?;
    let mut layers = [None; rewo_gpu::entities::MAX_ARMOR_SUBLAYERS];
    layers[0] = Some((IRON, [1.0, 1.0, 1.0]));
    let armoured = ashot(EntityDraw {
        body_armor: Some(rewo_gpu::entities::ArmorPiece {
            layers,
            ..Default::default()
        }),
        ..neutral_draw(EntityModelKind::Horse)
    })?;
    ast.destroy(gpu);
    let (mut armour_px, mut armour_bad) = (0usize, None);
    for (a, b) in armoured
        .as_chunks::<4>()
        .0
        .iter()
        .zip(bare_horse.as_chunks::<4>().0)
    {
        if a[..3] == b[..3] {
            continue;
        }
        armour_px += 1;
        let c = pack(a[0], a[1], a[2]);
        if armour_bad.is_none() && !explains(&iron_sheet, c) {
            armour_bad = Some(c);
        }
    }
    w.push(wit(
        "m25.a_horse_wears_its_body_armour_in_its_own_sheet",
        resident && !iron_sheet.is_empty() && armour_px >= 300 && armour_bad.is_none(),
        format!(
            "{IRON} resident: {resident}; {armour_px} px changed by the armour (want >= \
             300); first colour the sheet cannot produce: {:?}",
            armour_bad.map(|c| format!("#{c:06x}"))
        ),
    ));

    // m26 — M295: a saddled horse (`HORSE_SADDLE`'s layer over
    // `EquineSaddleModel`). The saddle's pixels must all be colours the jar's
    // `horse_saddle/saddle` sheet can produce, and riding adds the reins
    // (`visible = isRidden`), in the same sheet. No saddle drew until M295.
    const SADDLE: &str = "horse_saddle/saddle";
    let saddle_sheet: HashSet<u32> = baked
        .equipment
        .textures
        .iter()
        .find(|t| t.key == SADDLE)
        .map(|t| producible(&t.rgba))
        .unwrap_or_default();
    let mut sst = stage_from(
        gpu,
        baked,
        cw,
        ch,
        Vec3::new(2.6, 2.4, 1.2),
        Vec3::new(0.0, 1.3, -0.3),
    )?;
    let saddle_resident = sst
        .wr
        .entity_pass()
        .is_some_and(|p| p.armor_sheet_resident(SADDLE));
    let mut sshot = |d: EntityDraw<'_>| -> Result<Vec<u8>, String> {
        sst.wr.set_entities(&[d], sst.right, sst.up, 0.0);
        sst.off.render(gpu, Some((&mut sst.wr, sst.vp)), &ov, BG)?;
        sst.off.read_rgba(gpu)
    };
    let mut slayers = [None; rewo_gpu::entities::MAX_ARMOR_SUBLAYERS];
    slayers[0] = Some((SADDLE, [1.0, 1.0, 1.0]));
    let saddled = |ridden: bool| EntityDraw {
        saddle: Some(rewo_gpu::entities::ArmorPiece {
            layers: slayers,
            ..Default::default()
        }),
        ridden,
        ..neutral_draw(EntityModelKind::Horse)
    };
    let s_bare = sshot(neutral_draw(EntityModelKind::Horse))?;
    let s_on = sshot(saddled(false))?;
    let s_ridden = sshot(saddled(true))?;
    sst.destroy(gpu);
    let grade = |a: &[u8], b: &[u8]| {
        let (mut n, mut bad) = (0usize, None);
        for (x, y) in a.as_chunks::<4>().0.iter().zip(b.as_chunks::<4>().0) {
            if x[..3] == y[..3] {
                continue;
            }
            n += 1;
            let c = pack(x[0], x[1], x[2]);
            if bad.is_none() && !explains(&saddle_sheet, c) {
                bad = Some(c);
            }
        }
        (n, bad)
    };
    let (saddle_px, saddle_bad) = grade(&s_on, &s_bare);
    let (reins_px, reins_bad) = grade(&s_ridden, &s_on);
    w.push(wit(
        "m26.a_saddled_horse_wears_its_saddle_and_reins_when_ridden",
        saddle_resident
            && !saddle_sheet.is_empty()
            && saddle_px >= 150
            && saddle_bad.is_none()
            && reins_px >= 10
            && reins_bad.is_none(),
        format!(
            "{SADDLE} resident: {saddle_resident}; {saddle_px} px added by the saddle \
             (want >= 150), {reins_px} more by the reins when ridden (want >= 10); first \
             colour the sheet cannot produce: saddle {:?}, reins {:?}",
            saddle_bad.map(|c| format!("#{c:06x}")),
            reins_bad.map(|c| format!("#{c:06x}"))
        ),
    ));

    // m27 — M296: a chested donkey shows its two chests (`DonkeyModel`'s
    // boxes, `visible = hasChest`): every pixel they add must be a colour the
    // jar's donkey sheet can produce, and there must be plenty. No chest drew
    // until M296.
    let donkey_sheet: HashSet<u32> = baked
        .mob_textures
        .iter()
        .find(|t| t.key == "donkey")
        .map(|t| producible(&t.rgba))
        .unwrap_or_default();
    let mut dst = stage_from(
        gpu,
        baked,
        cw,
        ch,
        Vec3::new(2.8, 1.6, 0.6),
        Vec3::new(0.0, 1.0, 0.0),
    )?;
    let mut dshot = |d: EntityDraw<'_>| -> Result<Vec<u8>, String> {
        dst.wr.set_entities(&[d], dst.right, dst.up, 0.0);
        dst.off.render(gpu, Some((&mut dst.wr, dst.vp)), &ov, BG)?;
        dst.off.read_rgba(gpu)
    };
    let plain_donkey = dshot(neutral_draw(EntityModelKind::Donkey))?;
    let chested_donkey = dshot(EntityDraw {
        chested: true,
        avatar_hidden: 0,
        body_armor_crack: None,
        leash_holder: false,
        ..neutral_draw(EntityModelKind::Donkey)
    })?;
    dst.destroy(gpu);
    let (mut chest_px, mut chest_bad) = (0usize, None);
    for (a, b) in chested_donkey
        .as_chunks::<4>()
        .0
        .iter()
        .zip(plain_donkey.as_chunks::<4>().0)
    {
        if a[..3] == b[..3] {
            continue;
        }
        chest_px += 1;
        let c = pack(a[0], a[1], a[2]);
        if chest_bad.is_none() && !explains(&donkey_sheet, c) {
            chest_bad = Some(c);
        }
    }
    w.push(wit(
        "m27.a_chested_donkey_shows_its_chests_in_its_own_sheet",
        !donkey_sheet.is_empty() && chest_px >= 150 && chest_bad.is_none(),
        format!(
            "{chest_px} px added by the chests (want >= 150); first colour the donkey \
             sheet cannot produce: {:?}",
            chest_bad.map(|c| format!("#{c:06x}"))
        ),
    ));

    // m28 — M297: a llama's chests and carpet. Unchested, the bare llama
    // draws no chest (Rewo drew them on every llama until M297); chested,
    // they add pixels from the llama's own sheet; a red carpet
    // (`LLAMA_BODY`'s layer over the 0.5 decor shell) adds pixels from the
    // jar's `llama_body/red` alone.
    const CARPET: &str = "llama_body/red";
    let sheet_of = |key: &str, mob: bool| -> HashSet<u32> {
        if mob {
            baked
                .mob_textures
                .iter()
                .find(|t| t.key == key)
                .map(|t| producible(&t.rgba))
                .unwrap_or_default()
        } else {
            baked
                .equipment
                .textures
                .iter()
                .find(|t| t.key == key)
                .map(|t| producible(&t.rgba))
                .unwrap_or_default()
        }
    };
    let (llama_sheet, carpet_sheet) = (sheet_of("llama", true), sheet_of(CARPET, false));
    let mut lst = stage_from(
        gpu,
        baked,
        cw,
        ch,
        Vec3::new(2.6, 1.9, 0.9),
        Vec3::new(0.0, 1.1, 0.0),
    )?;
    let carpet_resident = lst
        .wr
        .entity_pass()
        .is_some_and(|p| p.armor_sheet_resident(CARPET));
    let mut lshot = |d: EntityDraw<'_>| -> Result<Vec<u8>, String> {
        lst.wr.set_entities(&[d], lst.right, lst.up, 0.0);
        lst.off.render(gpu, Some((&mut lst.wr, lst.vp)), &ov, BG)?;
        lst.off.read_rgba(gpu)
    };
    let mut clayers = [None; rewo_gpu::entities::MAX_ARMOR_SUBLAYERS];
    clayers[0] = Some((CARPET, [1.0, 1.0, 1.0]));
    let l_bare = lshot(neutral_draw(EntityModelKind::Llama))?;
    let l_chested = lshot(EntityDraw {
        chested: true,
        avatar_hidden: 0,
        body_armor_crack: None,
        leash_holder: false,
        ..neutral_draw(EntityModelKind::Llama)
    })?;
    let l_carpet = lshot(EntityDraw {
        body_armor: Some(rewo_gpu::entities::ArmorPiece {
            layers: clayers,
            ..Default::default()
        }),
        ..neutral_draw(EntityModelKind::Llama)
    })?;
    lst.destroy(gpu);
    let added = |a: &[u8], b: &[u8], sheet: &HashSet<u32>| {
        let (mut n, mut bad) = (0usize, None);
        for (x, y) in a.as_chunks::<4>().0.iter().zip(b.as_chunks::<4>().0) {
            if x[..3] == y[..3] {
                continue;
            }
            n += 1;
            let c = pack(x[0], x[1], x[2]);
            if bad.is_none() && !explains(sheet, c) {
                bad = Some(c);
            }
        }
        (n, bad)
    };
    let (chest_n, chest_bad) = added(&l_chested, &l_bare, &llama_sheet);
    let (carpet_n, carpet_bad) = added(&l_carpet, &l_bare, &carpet_sheet);
    w.push(wit(
        "m28.a_llama_shows_its_chests_only_chested_and_its_carpet",
        carpet_resident
            && !llama_sheet.is_empty()
            && !carpet_sheet.is_empty()
            && chest_n >= 150
            && chest_bad.is_none()
            && carpet_n >= 300
            && carpet_bad.is_none(),
        format!(
            "chests add {chest_n} px (want >= 150), first colour the llama sheet cannot \
             produce {:?}; {CARPET} resident {carpet_resident}, the carpet adds {carpet_n} px \
             (want >= 300), first colour its sheet cannot produce {:?}",
            chest_bad.map(|c| format!("#{c:06x}")),
            carpet_bad.map(|c| format!("#{c:06x}"))
        ),
    ));

    // m29 — M298: a saddled pig (`PIG_SADDLE` over the pig at 0.5) and a
    // saddled strider (`STRIDER_SADDLE` over its own mesh): each adds pixels
    // from its own jar saddle sheet alone. Neither drew until M298.
    let mut saddle_rows = Vec::new();
    for (kind, key, eye, at) in [
        (
            EntityModelKind::Pig,
            "pig_saddle/saddle",
            Vec3::new(1.8, 1.5, 1.0),
            Vec3::new(0.0, 0.6, 0.0),
        ),
        (
            EntityModelKind::Strider,
            "strider_saddle/saddle",
            Vec3::new(2.2, 2.4, 1.4),
            Vec3::new(0.0, 1.2, 0.0),
        ),
    ] {
        let sheet: HashSet<u32> = baked
            .equipment
            .textures
            .iter()
            .find(|t| t.key == key)
            .map(|t| producible(&t.rgba))
            .unwrap_or_default();
        let mut pst = stage_from(gpu, baked, cw, ch, eye, at)?;
        let resident = pst
            .wr
            .entity_pass()
            .is_some_and(|p| p.armor_sheet_resident(key));
        let mut pshot = |d: EntityDraw<'_>| -> Result<Vec<u8>, String> {
            pst.wr.set_entities(&[d], pst.right, pst.up, 0.0);
            pst.off.render(gpu, Some((&mut pst.wr, pst.vp)), &ov, BG)?;
            pst.off.read_rgba(gpu)
        };
        let mut layers = [None; rewo_gpu::entities::MAX_ARMOR_SUBLAYERS];
        layers[0] = Some((key, [1.0, 1.0, 1.0]));
        let bare = pshot(neutral_draw(kind))?;
        let saddled = pshot(EntityDraw {
            saddle: Some(rewo_gpu::entities::ArmorPiece {
                layers,
                ..Default::default()
            }),
            ..neutral_draw(kind)
        })?;
        pst.destroy(gpu);
        let (mut n, mut bad) = (0usize, None);
        for (a, b) in saddled
            .as_chunks::<4>()
            .0
            .iter()
            .zip(bare.as_chunks::<4>().0)
        {
            if a[..3] == b[..3] {
                continue;
            }
            n += 1;
            let c = pack(a[0], a[1], a[2]);
            if bad.is_none() && !explains(&sheet, c) {
                bad = Some(c);
            }
        }
        saddle_rows.push((key, resident, !sheet.is_empty(), n, bad));
    }
    w.push(wit(
        "m29.a_pig_and_a_strider_wear_their_saddles_in_their_own_sheets",
        saddle_rows
            .iter()
            .all(|&(_, resident, sheet, n, bad)| resident && sheet && n >= 100 && bad.is_none()),
        format!(
            "(sheet, resident, sheet read, px added (want >= 100), first colour it cannot \
             produce): {:?}",
            saddle_rows
                .iter()
                .map(|(k, r, s, n, b)| (k, r, s, n, b.map(|c| format!("#{c:06x}"))))
                .collect::<Vec<_>>()
        ),
    ));

    // m30 — M299: a wolf in armadillo-scute armour (`WOLF_BODY` over the
    // wolf at 0.2): every pixel the armour changes is a colour the jar's
    // `wolf_body/armadillo_scute` sheet can produce (the undyed overlay layer
    // is `onlyIfDyed`, so it draws nothing). No wolf armour drew until M299.
    const SCUTE: &str = "wolf_body/armadillo_scute";
    let scute_sheet: HashSet<u32> = baked
        .equipment
        .textures
        .iter()
        .find(|t| t.key == SCUTE)
        .map(|t| producible(&t.rgba))
        .unwrap_or_default();
    let mut wst = stage_from(
        gpu,
        baked,
        cw,
        ch,
        Vec3::new(1.4, 1.2, 0.9),
        Vec3::new(0.0, 0.55, 0.0),
    )?;
    let scute_resident = wst
        .wr
        .entity_pass()
        .is_some_and(|p| p.armor_sheet_resident(SCUTE));
    let mut wshot2 = |d: EntityDraw<'_>| -> Result<Vec<u8>, String> {
        wst.wr.set_entities(&[d], wst.right, wst.up, 0.0);
        wst.off.render(gpu, Some((&mut wst.wr, wst.vp)), &ov, BG)?;
        wst.off.read_rgba(gpu)
    };
    let mut wlayers = [None; rewo_gpu::entities::MAX_ARMOR_SUBLAYERS];
    wlayers[0] = Some((SCUTE, [1.0, 1.0, 1.0]));
    let bare_wolf = wshot2(neutral_draw(EntityModelKind::Wolf))?;
    let armoured_wolf = wshot2(EntityDraw {
        body_armor: Some(rewo_gpu::entities::ArmorPiece {
            layers: wlayers,
            ..Default::default()
        }),
        ..neutral_draw(EntityModelKind::Wolf)
    })?;
    wst.destroy(gpu);
    let (mut scute_px, mut scute_bad) = (0usize, None);
    for (a, b) in armoured_wolf
        .as_chunks::<4>()
        .0
        .iter()
        .zip(bare_wolf.as_chunks::<4>().0)
    {
        if a[..3] == b[..3] {
            continue;
        }
        scute_px += 1;
        let c = pack(a[0], a[1], a[2]);
        if scute_bad.is_none() && !explains(&scute_sheet, c) {
            scute_bad = Some(c);
        }
    }
    w.push(wit(
        "m30.a_wolf_wears_its_armour_in_its_own_sheet",
        scute_resident && !scute_sheet.is_empty() && scute_px >= 200 && scute_bad.is_none(),
        format!(
            "{SCUTE} resident: {scute_resident}; {scute_px} px changed by the armour (want \
             >= 200); first colour the sheet cannot produce: {:?}",
            scute_bad.map(|c| format!("#{c:06x}"))
        ),
    ));

    // m31 — M300: a saddled camel (`CAMEL_SADDLE` over `CamelSaddleModel`):
    // the saddle and bridle's pixels all come from the jar's
    // `camel_saddle/saddle`, and riding adds the reins from the same sheet.
    // No camel saddle drew until M300.
    const CAMEL_SADDLE: &str = "camel_saddle/saddle";
    let camel_sheet: HashSet<u32> = baked
        .equipment
        .textures
        .iter()
        .find(|t| t.key == CAMEL_SADDLE)
        .map(|t| producible(&t.rgba))
        .unwrap_or_default();
    let mut cst = stage_from(
        gpu,
        baked,
        cw,
        ch,
        Vec3::new(3.6, 3.4, -2.6),
        Vec3::new(0.0, 1.8, -0.6),
    )?;
    let camel_resident = cst
        .wr
        .entity_pass()
        .is_some_and(|p| p.armor_sheet_resident(CAMEL_SADDLE));
    let mut cshot = |d: EntityDraw<'_>| -> Result<Vec<u8>, String> {
        cst.wr.set_entities(&[d], cst.right, cst.up, 0.0);
        cst.off.render(gpu, Some((&mut cst.wr, cst.vp)), &ov, BG)?;
        cst.off.read_rgba(gpu)
    };
    let mut camel_layers = [None; rewo_gpu::entities::MAX_ARMOR_SUBLAYERS];
    camel_layers[0] = Some((CAMEL_SADDLE, [1.0, 1.0, 1.0]));
    let camel_saddled = |ridden: bool| EntityDraw {
        saddle: Some(rewo_gpu::entities::ArmorPiece {
            layers: camel_layers,
            ..Default::default()
        }),
        ridden,
        ..neutral_draw(EntityModelKind::Camel)
    };
    let c_bare = cshot(neutral_draw(EntityModelKind::Camel))?;
    let c_on = cshot(camel_saddled(false))?;
    let c_ridden = cshot(camel_saddled(true))?;
    cst.destroy(gpu);
    let camel_grade = |a: &[u8], b: &[u8]| {
        let (mut n, mut bad) = (0usize, None);
        for (x, y) in a.as_chunks::<4>().0.iter().zip(b.as_chunks::<4>().0) {
            if x[..3] == y[..3] {
                continue;
            }
            n += 1;
            let c = pack(x[0], x[1], x[2]);
            if bad.is_none() && !explains(&camel_sheet, c) {
                bad = Some(c);
            }
        }
        (n, bad)
    };
    let (camel_px, camel_bad) = camel_grade(&c_on, &c_bare);
    let (creins_px, creins_bad) = camel_grade(&c_ridden, &c_on);
    w.push(wit(
        "m31.a_saddled_camel_wears_its_saddle_and_reins_when_ridden",
        camel_resident
            && !camel_sheet.is_empty()
            && camel_px >= 200
            && camel_bad.is_none()
            && creins_px >= 10
            && creins_bad.is_none(),
        format!(
            "{CAMEL_SADDLE} resident: {camel_resident}; {camel_px} px added by the saddle and \
             bridle (want >= 200), {creins_px} more by the reins when ridden (want >= 10); \
             first colour the sheet cannot produce: saddle {:?}, reins {:?}",
            camel_bad.map(|c| format!("#{c:06x}")),
            creins_bad.map(|c| format!("#{c:06x}"))
        ),
    ));

    // m32 — M301: a nautilus in iron armour and a saddled one: each layer's
    // pixels come from its own jar sheet alone. Neither drew until M301.
    let mut naut_rows = Vec::new();
    for (key, saddle) in [
        ("nautilus_body/iron", false),
        ("nautilus_saddle/saddle", true),
    ] {
        let sheet: HashSet<u32> = baked
            .equipment
            .textures
            .iter()
            .find(|t| t.key == key)
            .map(|t| producible(&t.rgba))
            .unwrap_or_default();
        let mut nst = stage_from(
            gpu,
            baked,
            cw,
            ch,
            Vec3::new(2.4, 2.6, 2.2),
            Vec3::new(0.0, 1.0, 0.0),
        )?;
        let resident = nst
            .wr
            .entity_pass()
            .is_some_and(|p| p.armor_sheet_resident(key));
        let mut nshot = |d: EntityDraw<'_>| -> Result<Vec<u8>, String> {
            nst.wr.set_entities(&[d], nst.right, nst.up, 0.0);
            nst.off.render(gpu, Some((&mut nst.wr, nst.vp)), &ov, BG)?;
            nst.off.read_rgba(gpu)
        };
        let mut layers = [None; rewo_gpu::entities::MAX_ARMOR_SUBLAYERS];
        layers[0] = Some((key, [1.0, 1.0, 1.0]));
        let piece = Some(rewo_gpu::entities::ArmorPiece {
            layers,
            ..Default::default()
        });
        let bare = nshot(neutral_draw(EntityModelKind::Nautilus))?;
        let worn = nshot(if saddle {
            EntityDraw {
                saddle: piece,
                ..neutral_draw(EntityModelKind::Nautilus)
            }
        } else {
            EntityDraw {
                body_armor: piece,
                ..neutral_draw(EntityModelKind::Nautilus)
            }
        })?;
        nst.destroy(gpu);
        let (mut n, mut bad) = (0usize, None);
        for (a, b) in worn.as_chunks::<4>().0.iter().zip(bare.as_chunks::<4>().0) {
            if a[..3] == b[..3] {
                continue;
            }
            n += 1;
            let c = pack(a[0], a[1], a[2]);
            if bad.is_none() && !explains(&sheet, c) {
                bad = Some(c);
            }
        }
        naut_rows.push((key, resident, !sheet.is_empty(), n, bad));
    }
    w.push(wit(
        "m32.a_nautilus_wears_its_armour_and_saddle_in_their_own_sheets",
        naut_rows
            .iter()
            .all(|&(_, resident, sheet, n, bad)| resident && sheet && n >= 150 && bad.is_none()),
        format!(
            "(sheet, resident, sheet read, px added (want >= 150), first colour it cannot \
             produce): {:?}",
            naut_rows
                .iter()
                .map(|(k, r, s, n, b)| (k, r, s, n, b.map(|c| format!("#{c:06x}"))))
                .collect::<Vec<_>>()
        ),
    ));

    // m33 — M302: a happy ghast in a white harness: the harness and goggles'
    // pixels come from the jar's `happy_ghast_body/white_harness` alone, and
    // riding moves the goggles (the frame changes). No harness drew until
    // M302.
    const HARNESS: &str = "happy_ghast_body/white_harness";
    let harness_sheet: HashSet<u32> = baked
        .equipment
        .textures
        .iter()
        .find(|t| t.key == HARNESS)
        .map(|t| producible(&t.rgba))
        .unwrap_or_default();
    let mut hst = stage_from(
        gpu,
        baked,
        cw,
        ch,
        Vec3::new(6.5, 5.5, -6.5),
        Vec3::new(0.0, 2.6, 0.0),
    )?;
    let harness_resident = hst
        .wr
        .entity_pass()
        .is_some_and(|p| p.armor_sheet_resident(HARNESS));
    let mut hshot = |d: EntityDraw<'_>| -> Result<Vec<u8>, String> {
        hst.wr.set_entities(&[d], hst.right, hst.up, 0.0);
        hst.off.render(gpu, Some((&mut hst.wr, hst.vp)), &ov, BG)?;
        hst.off.read_rgba(gpu)
    };
    let mut hlayers = [None; rewo_gpu::entities::MAX_ARMOR_SUBLAYERS];
    hlayers[0] = Some((HARNESS, [1.0, 1.0, 1.0]));
    let harnessed = |ridden: bool| EntityDraw {
        body_armor: Some(rewo_gpu::entities::ArmorPiece {
            layers: hlayers,
            ..Default::default()
        }),
        ridden,
        ..neutral_draw(EntityModelKind::HappyGhast)
    };
    let g_bare = hshot(neutral_draw(EntityModelKind::HappyGhast))?;
    let g_up = hshot(harnessed(false))?;
    let g_down = hshot(harnessed(true))?;
    hst.destroy(gpu);
    let (mut harness_px, mut harness_bad) = (0usize, None);
    for (a, b) in g_up
        .as_chunks::<4>()
        .0
        .iter()
        .zip(g_bare.as_chunks::<4>().0)
    {
        if a[..3] == b[..3] {
            continue;
        }
        harness_px += 1;
        let c = pack(a[0], a[1], a[2]);
        if harness_bad.is_none() && !explains(&harness_sheet, c) {
            harness_bad = Some(c);
        }
    }
    let goggles_moved = g_up != g_down;
    w.push(wit(
        "m33.a_happy_ghast_wears_its_harness_and_its_goggles_drop_when_ridden",
        harness_resident
            && !harness_sheet.is_empty()
            && harness_px >= 300
            && harness_bad.is_none()
            && goggles_moved,
        format!(
            "{HARNESS} resident: {harness_resident}; {harness_px} px changed by the harness \
             (want >= 300); first colour the sheet cannot produce: {:?}; the goggles moved \
             when ridden: {goggles_moved}",
            harness_bad.map(|c| format!("#{c:06x}"))
        ),
    ));

    // m34 — M303: a camel husk draws as a camel on its own sheet (it drew as
    // a capsule until M303): its frame differs from a camel's, every pixel
    // is a colour the jar's `camel_husk` sheet can produce, and its saddle's
    // pixels come from `camel_husk_saddle/saddle`.
    const HUSK_SADDLE: &str = "camel_husk_saddle/saddle";
    let husk_sheet: HashSet<u32> = baked
        .mob_textures
        .iter()
        .find(|t| t.key == "camel_husk")
        .map(|t| producible(&t.rgba))
        .unwrap_or_default();
    let husk_saddle_sheet: HashSet<u32> = baked
        .equipment
        .textures
        .iter()
        .find(|t| t.key == HUSK_SADDLE)
        .map(|t| producible(&t.rgba))
        .unwrap_or_default();
    let mut kst = stage_from(
        gpu,
        baked,
        cw,
        ch,
        Vec3::new(3.6, 3.4, -2.6),
        Vec3::new(0.0, 1.8, -0.6),
    )?;
    let husk_saddle_resident = kst
        .wr
        .entity_pass()
        .is_some_and(|p| p.armor_sheet_resident(HUSK_SADDLE));
    kst.wr.set_entities(&[], kst.right, kst.up, 0.0);
    kst.off.render(gpu, Some((&mut kst.wr, kst.vp)), &ov, BG)?;
    let k_empty = kst.off.read_rgba(gpu)?;
    let mut kshot = |d: EntityDraw<'_>| -> Result<Vec<u8>, String> {
        kst.wr.set_entities(&[d], kst.right, kst.up, 0.0);
        kst.off.render(gpu, Some((&mut kst.wr, kst.vp)), &ov, BG)?;
        kst.off.read_rgba(gpu)
    };
    let mut husk_layers = [None; rewo_gpu::entities::MAX_ARMOR_SUBLAYERS];
    husk_layers[0] = Some((HUSK_SADDLE, [1.0, 1.0, 1.0]));
    let k_camel = kshot(neutral_draw(EntityModelKind::Camel))?;
    let k_bare = kshot(neutral_draw(EntityModelKind::CamelHusk))?;
    let k_saddled = kshot(EntityDraw {
        saddle: Some(rewo_gpu::entities::ArmorPiece {
            layers: husk_layers,
            ..Default::default()
        }),
        ..neutral_draw(EntityModelKind::CamelHusk)
    })?;
    kst.destroy(gpu);
    let (mut husk_px, mut husk_bad) = (0usize, None);
    for (p, e) in k_bare
        .as_chunks::<4>()
        .0
        .iter()
        .zip(k_empty.as_chunks::<4>().0)
    {
        if p[..3] == e[..3] {
            continue;
        }
        husk_px += 1;
        let c = pack(p[0], p[1], p[2]);
        if husk_bad.is_none() && !explains(&husk_sheet, c) {
            husk_bad = Some(c);
        }
    }
    let (mut hsad_px, mut hsad_bad) = (0usize, None);
    for (a, b) in k_saddled
        .as_chunks::<4>()
        .0
        .iter()
        .zip(k_bare.as_chunks::<4>().0)
    {
        if a[..3] == b[..3] {
            continue;
        }
        hsad_px += 1;
        let c = pack(a[0], a[1], a[2]);
        if hsad_bad.is_none() && !explains(&husk_saddle_sheet, c) {
            hsad_bad = Some(c);
        }
    }
    let husk_not_camel = k_bare != k_camel;
    w.push(wit(
        "m34.a_camel_husk_draws_as_a_camel_on_its_own_sheet_and_wears_its_saddle",
        !husk_sheet.is_empty()
            && !husk_saddle_sheet.is_empty()
            && husk_saddle_resident
            && husk_not_camel
            && husk_px >= 2000
            && husk_bad.is_none()
            && hsad_px >= 200
            && hsad_bad.is_none(),
        format!(
            "{husk_px} px drawn (want >= 2000), first colour `camel_husk` cannot produce: {:?}; \
             differs from a camel: {husk_not_camel}; {HUSK_SADDLE} resident: \
             {husk_saddle_resident}, {hsad_px} px added by the saddle (want >= 200), first \
             colour it cannot produce: {:?}",
            husk_bad.map(|c| format!("#{c:06x}")),
            hsad_bad.map(|c| format!("#{c:06x}"))
        ),
    ));

    // m35 — M304: a giant draws as a zombie six times the size (it was a
    // capsule until M304): every pixel is a colour the jar's zombie sheet can
    // produce, and beside a zombie at the same spot its silhouette stands
    // about six times as tall (perspective at this distance keeps the ratio
    // within 5..7).
    let zombie_sheet: HashSet<u32> = baked
        .mob_textures
        .iter()
        .find(|t| t.key == "zombie")
        .map(|t| producible(&t.rgba))
        .unwrap_or_default();
    let mut gst = stage_from(
        gpu,
        baked,
        cw,
        ch,
        Vec3::new(0.0, 6.0, -16.0),
        Vec3::new(0.0, 6.0, 0.0),
    )?;
    gst.wr.set_entities(&[], gst.right, gst.up, 0.0);
    gst.off.render(gpu, Some((&mut gst.wr, gst.vp)), &ov, BG)?;
    let g_empty = gst.off.read_rgba(gpu)?;
    let mut gshot = |d: EntityDraw<'_>| -> Result<Vec<u8>, String> {
        gst.wr.set_entities(&[d], gst.right, gst.up, 0.0);
        gst.off.render(gpu, Some((&mut gst.wr, gst.vp)), &ov, BG)?;
        gst.off.read_rgba(gpu)
    };
    let g_giant = gshot(neutral_draw(EntityModelKind::Giant))?;
    let g_zombie = gshot(neutral_draw(EntityModelKind::Zombie))?;
    gst.destroy(gpu);
    // The drawn rows' span, and the first colour the zombie sheet cannot
    // produce.
    let span = |f: &[u8]| {
        let (mut top, mut bottom, mut n, mut bad) = (u32::MAX, 0u32, 0usize, None);
        for (i, (p, e)) in f
            .as_chunks::<4>()
            .0
            .iter()
            .zip(g_empty.as_chunks::<4>().0)
            .enumerate()
        {
            if p[..3] == e[..3] {
                continue;
            }
            let y = i as u32 / cw;
            top = top.min(y);
            bottom = bottom.max(y);
            n += 1;
            let c = pack(p[0], p[1], p[2]);
            if bad.is_none() && !explains(&zombie_sheet, c) {
                bad = Some(c);
            }
        }
        (if n == 0 { 0 } else { bottom - top + 1 }, n, bad)
    };
    let (giant_h, giant_px, giant_bad) = span(&g_giant);
    let (zombie_h, _, _) = span(&g_zombie);
    let ratio = giant_h as f32 / zombie_h.max(1) as f32;
    w.push(wit(
        "m35.a_giant_draws_as_a_zombie_six_times_the_size",
        !zombie_sheet.is_empty()
            && giant_px >= 2000
            && giant_bad.is_none()
            && zombie_h >= 10
            && (5.0..=7.0).contains(&ratio),
        format!(
            "{giant_px} px drawn (want >= 2000), first colour the zombie sheet cannot produce: \
             {:?}; {giant_h} rows tall against a zombie's {zombie_h} (ratio {ratio:.2}, want \
             5..7)",
            giant_bad.map(|c| format!("#{c:06x}"))
        ),
    ));

    // m36 — M306: the slime's shell is `entityTranslucent` over its inner
    // body. With a ravager standing behind it, the shell's pixels outside the
    // core show the ravager through them and those over the core do not; an
    // opaque shell (all Rewo drew until M306) hides the ravager everywhere and
    // the core too. (The stage draws a sky, so a second clear colour would
    // show nothing.)
    let mut tst = stage_from(
        gpu,
        baked,
        cw,
        ch,
        Vec3::new(0.0, 1.6, -3.2),
        Vec3::new(0.0, 0.5, 0.0),
    )?;
    let mut tshot = |d: &[EntityDraw<'_>]| -> Result<(Vec<u8>, u32), String> {
        tst.wr.set_entities(d, tst.right, tst.up, 0.0);
        let n = tst
            .wr
            .entity_pass()
            .map_or(0, |p| p.translucent_vertex_count());
        tst.off.render(gpu, Some((&mut tst.wr, tst.vp)), &ov, BG)?;
        Ok((tst.off.read_rgba(gpu)?, n))
    };
    let behind = EntityDraw {
        pos: [0.0, 0.0, 1.6],
        ..neutral_draw(EntityModelKind::Ravager)
    };
    let slime = neutral_draw(EntityModelKind::Slime);
    let (empty, _) = tshot(&[])?;
    let (alone, tverts) = tshot(&[slime])?;
    let (back, _) = tshot(&[behind])?;
    let (both, _) = tshot(&[slime, behind])?;
    if let Some(d) = &args.out_dir {
        save_png(&both, cw, ch, &d.join("slime_over_ravager.png"))?;
    }
    tst.destroy(gpu);
    let (mut leak, mut held) = (0usize, 0usize);
    for (((a, b), e), r) in alone
        .as_chunks::<4>()
        .0
        .iter()
        .zip(both.as_chunks::<4>().0)
        .zip(empty.as_chunks::<4>().0)
        .zip(back.as_chunks::<4>().0)
    {
        // A slime pixel with the ravager behind it.
        if a[..3] == e[..3] || r[..3] == e[..3] {
            continue;
        }
        if a[..3] == b[..3] {
            held += 1;
        } else {
            leak += 1;
        }
    }
    w.push(wit(
        "m36.the_slimes_shell_is_translucent_over_its_inner_body",
        tverts > 0 && leak >= 100 && held >= 100,
        format!(
            "{tverts} translucent vertices; {leak} slime px show the ravager behind \
             (want >= 100; an opaque shell measured 34, through its cut-out texels), {held} \
             do not (the core through the shell, want >= 100)"
        ),
    ));

    // m37 — M307: each sulfur cube (it drew as a capsule until M307) is its
    // opaque inner box under a translucent outer box, the small model for a
    // baby: at size 2, with the ravager behind, the shell's pixels show it
    // and the core's do not, for both models.
    let mut cst = stage_from(
        gpu,
        baked,
        cw,
        ch,
        Vec3::new(0.0, 1.6, -3.2),
        Vec3::new(0.0, 0.5, 0.0),
    )?;
    let mut cshot = |d: &[EntityDraw<'_>]| -> Result<(Vec<u8>, u32), String> {
        cst.wr.set_entities(d, cst.right, cst.up, 0.0);
        let n = cst
            .wr
            .entity_pass()
            .map_or(0, |p| p.translucent_vertex_count());
        cst.off.render(gpu, Some((&mut cst.wr, cst.vp)), &ov, BG)?;
        Ok((cst.off.read_rgba(gpu)?, n))
    };
    let behind = EntityDraw {
        pos: [0.0, 0.0, 1.6],
        ..neutral_draw(EntityModelKind::Ravager)
    };
    let (cempty, _) = cshot(&[])?;
    let (cback, _) = cshot(&[behind])?;
    let mut cube_rows = Vec::new();
    let mut cubes_ok = true;
    for kind in [
        EntityModelKind::SulfurCube,
        EntityModelKind::SulfurCubeSmall,
    ] {
        let cube = EntityDraw {
            scale_mul: 2.0,
            ..neutral_draw(kind)
        };
        let (alone, tverts) = cshot(&[cube])?;
        let (both, _) = cshot(&[cube, behind])?;
        let (mut leak, mut held) = (0usize, 0usize);
        for (((a, b), e), r) in alone
            .as_chunks::<4>()
            .0
            .iter()
            .zip(both.as_chunks::<4>().0)
            .zip(cempty.as_chunks::<4>().0)
            .zip(cback.as_chunks::<4>().0)
        {
            if a[..3] == e[..3] || r[..3] == e[..3] {
                continue;
            }
            if a[..3] == b[..3] {
                held += 1;
            } else {
                leak += 1;
            }
        }
        cubes_ok &= tverts > 0 && leak >= 100 && held >= 100;
        cube_rows.push(format!(
            "{}: {tverts} translucent vertices, {leak} px show the ravager, {held} do not",
            kind.name()
        ));
    }
    cst.destroy(gpu);
    w.push(wit(
        "m37.both_sulfur_cubes_draw_a_core_under_a_translucent_shell",
        cubes_ok,
        format!("{} (want >= 100 of each)", cube_rows.join("; ")),
    ));

    // m38 — M310: an avatar's outer layers follow its customisation mask. In
    // wide Kai (the jar's default skin 12: a 296-texel jacket, sleeves, no
    // hat texels), hiding the jacket changes pixels, hiding all six layers
    // changes more, and hiding the hat (transparent in this skin) changes
    // nothing at all. Until M310 every layer always drew.
    use rewo_gpu::mobs::{AVATAR_HAT, AVATAR_JACKET};
    let kai = baked
        .default_skins
        .get(12)
        .and_then(|d| d.as_ref())
        .ok_or("mobtexshot: the jar has no wide/kai.png")?;
    let mut pst = stage_from(
        gpu,
        baked,
        cw,
        ch,
        Vec3::new(0.0, 1.0, -3.0),
        Vec3::new(0.0, 0.9, 0.0),
    )?;
    let kai_uv = pst
        .wr
        .upload_player_skin(gpu, 12, &kai.rgba)
        .ok_or("mobtexshot: no skin slot for Kai")?;
    let mut pshot = |hidden: u8| -> Result<Vec<u8>, String> {
        let d = EntityDraw {
            skin_uv: Some(kai_uv),
            avatar_hidden: hidden,
            ..neutral_draw(EntityModelKind::Player)
        };
        pst.wr.set_entities(&[d], pst.right, pst.up, 0.0);
        pst.off.render(gpu, Some((&mut pst.wr, pst.vp)), &ov, BG)?;
        pst.off.read_rgba(gpu)
    };
    let all_shown = pshot(0)?;
    let no_jacket = pshot(AVATAR_JACKET)?;
    let no_hat = pshot(AVATAR_HAT)?;
    let none_shown = pshot(0x7e)?;
    pst.destroy(gpu);
    let changed = |a: &[u8], b: &[u8]| {
        a.as_chunks::<4>()
            .0
            .iter()
            .zip(b.as_chunks::<4>().0)
            .filter(|(x, y)| x[..3] != y[..3])
            .count()
    };
    let (jacket_px, all_px, hat_px) = (
        changed(&all_shown, &no_jacket),
        changed(&all_shown, &none_shown),
        changed(&all_shown, &no_hat),
    );
    w.push(wit(
        "m38.an_avatars_outer_layers_follow_its_customisation_mask",
        jacket_px >= 50 && all_px > jacket_px && hat_px == 0,
        format!(
            "hiding the jacket changes {jacket_px} px (want >= 50), all six layers {all_px} \
             (want more), the transparent hat {hat_px} (want 0)"
        ),
    ));

    // m39 — M311: a damaged wolf armour shows its cracks
    // (`WolfArmorLayer.maybeRenderCracks`, `armorTranslucent`): the high
    // crack sheet adds translucent vertices and changes pixels, and every
    // changed pixel lies on the armour (it changed when the armour went on).
    // No crack drew until M311.
    const CRACK: &str = "wolf_armor_crackiness_high";
    let mut kst2 = stage_from(
        gpu,
        baked,
        cw,
        ch,
        Vec3::new(1.4, 1.2, 0.9),
        Vec3::new(0.0, 0.55, 0.0),
    )?;
    let crack_resident = kst2
        .wr
        .entity_pass()
        .is_some_and(|p| p.armor_sheet_resident(CRACK));
    let mut kshot2 = |d: EntityDraw<'_>| -> Result<(Vec<u8>, u32), String> {
        kst2.wr.set_entities(&[d], kst2.right, kst2.up, 0.0);
        let n = kst2
            .wr
            .entity_pass()
            .map_or(0, |p| p.translucent_vertex_count());
        kst2.off
            .render(gpu, Some((&mut kst2.wr, kst2.vp)), &ov, BG)?;
        Ok((kst2.off.read_rgba(gpu)?, n))
    };
    let mut clayers = [None; rewo_gpu::entities::MAX_ARMOR_SUBLAYERS];
    clayers[0] = Some(("wolf_body/armadillo_scute", [1.0, 1.0, 1.0]));
    let armoured = EntityDraw {
        body_armor: Some(rewo_gpu::entities::ArmorPiece {
            layers: clayers,
            ..Default::default()
        }),
        ..neutral_draw(EntityModelKind::Wolf)
    };
    let (c_bare, _) = kshot2(neutral_draw(EntityModelKind::Wolf))?;
    let (c_whole, _) = kshot2(armoured)?;
    let (c_cracked, c_verts) = kshot2(EntityDraw {
        body_armor_crack: Some(CRACK),
        ..armoured
    })?;
    kst2.destroy(gpu);
    let (mut crack_px, mut off_armour) = (0usize, 0usize);
    for ((a, b), c) in c_cracked
        .as_chunks::<4>()
        .0
        .iter()
        .zip(c_whole.as_chunks::<4>().0)
        .zip(c_bare.as_chunks::<4>().0)
    {
        if a[..3] == b[..3] {
            continue;
        }
        crack_px += 1;
        if b[..3] == c[..3] {
            off_armour += 1;
        }
    }
    w.push(wit(
        "m39.a_damaged_wolf_armour_shows_its_cracks",
        crack_resident && c_verts > 0 && crack_px >= 20 && off_armour == 0,
        format!(
            "{CRACK} resident: {crack_resident}; {c_verts} translucent vertices; {crack_px} px \
             changed by the cracks (want >= 20), {off_armour} of them off the armour (want 0)"
        ),
    ));

    // m40 — M312: a harnessed happy ghast holding a leash shows its ropes
    // (`RopesLayer`), every rope pixel a colour the jar's ropes sheet can
    // produce; holding a leash without a harness, or a harness without a
    // leash, shows none. No ropes drew until M312.
    let ropes_sheet: HashSet<u32> = baked
        .mob_textures
        .iter()
        .find(|t| t.key == "happy_ghast_ropes")
        .map(|t| producible(&t.rgba))
        .unwrap_or_default();
    let mut rst = stage_from(
        gpu,
        baked,
        cw,
        ch,
        Vec3::new(6.5, 5.5, -6.5),
        Vec3::new(0.0, 2.6, 0.0),
    )?;
    let mut rshot = |d: EntityDraw<'_>| -> Result<Vec<u8>, String> {
        rst.wr.set_entities(&[d], rst.right, rst.up, 0.0);
        rst.off.render(gpu, Some((&mut rst.wr, rst.vp)), &ov, BG)?;
        rst.off.read_rgba(gpu)
    };
    let mut rlayers = [None; rewo_gpu::entities::MAX_ARMOR_SUBLAYERS];
    rlayers[0] = Some(("happy_ghast_body/white_harness", [1.0, 1.0, 1.0]));
    let harnessed = EntityDraw {
        body_armor: Some(rewo_gpu::entities::ArmorPiece {
            layers: rlayers,
            ..Default::default()
        }),
        ..neutral_draw(EntityModelKind::HappyGhast)
    };
    let r_bare = rshot(neutral_draw(EntityModelKind::HappyGhast))?;
    let r_leash_only = rshot(EntityDraw {
        leash_holder: true,
        ..neutral_draw(EntityModelKind::HappyGhast)
    })?;
    let r_harness = rshot(harnessed)?;
    let r_ropes = rshot(EntityDraw {
        leash_holder: true,
        ..harnessed
    })?;
    rst.destroy(gpu);
    let (mut rope_px, mut rope_bad) = (0usize, None);
    for (a, b) in r_ropes
        .as_chunks::<4>()
        .0
        .iter()
        .zip(r_harness.as_chunks::<4>().0)
    {
        if a[..3] == b[..3] {
            continue;
        }
        rope_px += 1;
        let c = pack(a[0], a[1], a[2]);
        if rope_bad.is_none() && !explains(&ropes_sheet, c) {
            rope_bad = Some(c);
        }
    }
    let leash_alone_same = r_leash_only == r_bare;
    w.push(wit(
        "m40.a_harnessed_happy_ghast_holding_a_leash_shows_its_ropes",
        !ropes_sheet.is_empty() && rope_px >= 200 && rope_bad.is_none() && leash_alone_same,
        format!(
            "{rope_px} px added by the ropes (want >= 200), first colour the ropes sheet \
             cannot produce: {:?}; a leash without a harness changes nothing: \
             {leash_alone_same}",
            rope_bad.map(|c| format!("#{c:06x}"))
        ),
    ));

    // m41 — M314: the ghastling is its own model on its own sheet: beside
    // where an adult draws it stands about 0.2375 of the adult's height
    // (`BABY_TRANSFORMER`; perspective keeps the ratio within 0.18..0.30),
    // and every pixel is a colour the jar's baby sheet can produce. Until
    // M314 a baby happy ghast drew as the adult at half size.
    let baby_sheet: HashSet<u32> = baked
        .mob_textures
        .iter()
        .find(|t| t.key == "happy_ghast_baby")
        .map(|t| producible(&t.rgba))
        .unwrap_or_default();
    let mut bst = stage_from(
        gpu,
        baked,
        cw,
        ch,
        Vec3::new(6.5, 5.5, -6.5),
        Vec3::new(0.0, 2.6, 0.0),
    )?;
    bst.wr.set_entities(&[], bst.right, bst.up, 0.0);
    bst.off.render(gpu, Some((&mut bst.wr, bst.vp)), &ov, BG)?;
    let b_empty = bst.off.read_rgba(gpu)?;
    let mut bshot = |d: EntityDraw<'_>| -> Result<Vec<u8>, String> {
        bst.wr.set_entities(&[d], bst.right, bst.up, 0.0);
        bst.off.render(gpu, Some((&mut bst.wr, bst.vp)), &ov, BG)?;
        bst.off.read_rgba(gpu)
    };
    let b_adult = bshot(neutral_draw(EntityModelKind::HappyGhast))?;
    let b_baby = bshot(neutral_draw(EntityModelKind::HappyGhastBaby))?;
    bst.destroy(gpu);
    let rows = |f: &[u8], sheet: Option<&HashSet<u32>>| {
        let (mut top, mut bottom, mut n, mut bad) = (u32::MAX, 0u32, 0usize, None);
        for (i, (p, e)) in f
            .as_chunks::<4>()
            .0
            .iter()
            .zip(b_empty.as_chunks::<4>().0)
            .enumerate()
        {
            if p[..3] == e[..3] {
                continue;
            }
            let y = i as u32 / cw;
            top = top.min(y);
            bottom = bottom.max(y);
            n += 1;
            let c = pack(p[0], p[1], p[2]);
            if bad.is_none() && sheet.is_some_and(|s| !explains(s, c)) {
                bad = Some(c);
            }
        }
        (if n == 0 { 0 } else { bottom - top + 1 }, n, bad)
    };
    let (adult_h, _, _) = rows(&b_adult, None);
    let (baby_h, baby_px, baby_bad) = rows(&b_baby, Some(&baby_sheet));
    let ratio = baby_h as f32 / adult_h.max(1) as f32;
    w.push(wit(
        "m41.the_ghastling_is_its_own_model_on_its_own_sheet",
        !baby_sheet.is_empty()
            && baby_px >= 100
            && baby_bad.is_none()
            && (0.18..=0.30).contains(&ratio),
        format!(
            "the ghastling stands {baby_h} rows against the adult's {adult_h} (ratio \
             {ratio:.3}, want 0.18..0.30); {baby_px} px, first colour the baby sheet cannot \
             produce: {:?}",
            baby_bad.map(|c| format!("#{c:06x}"))
        ),
    ));

    // m42 — M315: the baby turtle is its own model on its own 16 x 16 sheet:
    // beside where an adult draws it covers well under a tenth of the adult's
    // pixels (Rewo's half-size adult covered a quarter), every one a colour
    // the baby sheet can produce. Until M315 it was the adult at half size.
    let tbaby_sheet: HashSet<u32> = baked
        .mob_textures
        .iter()
        .find(|t| t.key == "turtle_baby")
        .map(|t| producible(&t.rgba))
        .unwrap_or_default();
    let mut tbst = stage_from(
        gpu,
        baked,
        cw,
        ch,
        Vec3::new(1.6, 1.5, -1.6),
        Vec3::new(0.0, 0.2, 0.0),
    )?;
    tbst.wr.set_entities(&[], tbst.right, tbst.up, 0.0);
    tbst.off
        .render(gpu, Some((&mut tbst.wr, tbst.vp)), &ov, BG)?;
    let tb_empty = tbst.off.read_rgba(gpu)?;
    let mut tbshot = |d: EntityDraw<'_>| -> Result<Vec<u8>, String> {
        tbst.wr.set_entities(&[d], tbst.right, tbst.up, 0.0);
        tbst.off
            .render(gpu, Some((&mut tbst.wr, tbst.vp)), &ov, BG)?;
        tbst.off.read_rgba(gpu)
    };
    let tb_adult = tbshot(neutral_draw(EntityModelKind::Turtle))?;
    let tb_baby = tbshot(neutral_draw(EntityModelKind::TurtleBaby))?;
    tbst.destroy(gpu);
    let drawn = |f: &[u8], sheet: Option<&HashSet<u32>>| {
        let (mut n, mut bad) = (0usize, None);
        for (p, e) in f.as_chunks::<4>().0.iter().zip(tb_empty.as_chunks::<4>().0) {
            if p[..3] == e[..3] {
                continue;
            }
            n += 1;
            let c = pack(p[0], p[1], p[2]);
            if bad.is_none() && sheet.is_some_and(|s| !explains(s, c)) {
                bad = Some(c);
            }
        }
        (n, bad)
    };
    let (adult_px, _) = drawn(&tb_adult, None);
    let (baby_px, baby_bad) = drawn(&tb_baby, Some(&tbaby_sheet));
    w.push(wit(
        "m42.the_baby_turtle_is_its_own_model_on_its_own_sheet",
        !tbaby_sheet.is_empty() && baby_px >= 30 && baby_px * 10 < adult_px && baby_bad.is_none(),
        format!(
            "the baby covers {baby_px} px against the adult's {adult_px} (want >= 30 and under \
             a tenth); first colour the baby sheet cannot produce: {:?}",
            baby_bad.map(|c| format!("#{c:06x}"))
        ),
    ));

    // m43 — M316: the polar bear cub draws, every pixel a colour the cub
    // sheet can produce. (Its mesh is pinned box for box by the unit test
    // `the_polar_bear_cub_is_its_own_model`; a pixel comparison against the
    // half-size adult could not tell the meshes apart, since a wrong mesh on
    // the cub sheet differs by its cut-outs too.)
    let cub_sheet: HashSet<u32> = baked
        .mob_textures
        .iter()
        .find(|t| t.key == "polar_bear_baby")
        .map(|t| producible(&t.rgba))
        .unwrap_or_default();
    let mut pcst = stage_from(
        gpu,
        baked,
        cw,
        ch,
        Vec3::new(2.4, 1.6, -2.4),
        Vec3::new(0.0, 0.4, 0.0),
    )?;
    pcst.wr.set_entities(&[], pcst.right, pcst.up, 0.0);
    pcst.off
        .render(gpu, Some((&mut pcst.wr, pcst.vp)), &ov, BG)?;
    let pc_empty = pcst.off.read_rgba(gpu)?;
    let cub = neutral_draw(EntityModelKind::PolarBearBaby);
    pcst.wr.set_entities(&[cub], pcst.right, pcst.up, 0.0);
    pcst.off
        .render(gpu, Some((&mut pcst.wr, pcst.vp)), &ov, BG)?;
    let pc_cub = pcst.off.read_rgba(gpu)?;
    pcst.destroy(gpu);
    let (mut cub_px, mut cub_bad) = (0usize, None);
    for (p, e) in pc_cub
        .as_chunks::<4>()
        .0
        .iter()
        .zip(pc_empty.as_chunks::<4>().0)
    {
        if p[..3] == e[..3] {
            continue;
        }
        cub_px += 1;
        let c = pack(p[0], p[1], p[2]);
        if cub_bad.is_none() && !explains(&cub_sheet, c) {
            cub_bad = Some(c);
        }
    }
    w.push(wit(
        "m43.the_polar_bear_cub_draws_from_its_own_sheet",
        !cub_sheet.is_empty() && cub_px >= 200 && cub_bad.is_none(),
        format!(
            "{cub_px} cub px (want >= 200), first colour the cub sheet cannot produce: {:?}",
            cub_bad.map(|c| format!("#{c:06x}"))
        ),
    ));

    // m44 — M318: the baby villager draws, every pixel producible by
    // `villager_baby`. (Its mesh and texel layout are pinned by the unit test
    // `the_baby_villager_is_its_own_model`; the baby's palette lies wholly
    // inside the adult's, so no pixel can tell the two sheets apart.) Until
    // M318 it was the adult mesh wearing the baby sheet's UV offset, which
    // scrambled it.
    let vb_sheet: HashSet<u32> = baked
        .mob_textures
        .iter()
        .find(|t| t.key == "villager_baby")
        .map(|t| producible(&t.rgba))
        .unwrap_or_default();
    let mut vbst = stage_from(
        gpu,
        baked,
        cw,
        ch,
        Vec3::new(0.0, 1.0, -2.2),
        Vec3::new(0.0, 0.5, 0.0),
    )?;
    vbst.wr.set_entities(&[], vbst.right, vbst.up, 0.0);
    vbst.off
        .render(gpu, Some((&mut vbst.wr, vbst.vp)), &ov, BG)?;
    let vb_empty = vbst.off.read_rgba(gpu)?;
    let vb_draw = neutral_draw(EntityModelKind::VillagerBaby);
    vbst.wr.set_entities(&[vb_draw], vbst.right, vbst.up, 0.0);
    vbst.off
        .render(gpu, Some((&mut vbst.wr, vbst.vp)), &ov, BG)?;
    let vb_frame = vbst.off.read_rgba(gpu)?;
    vbst.destroy(gpu);
    let (mut vb_px, mut vb_bad) = (0usize, None);
    for (p, e) in vb_frame
        .as_chunks::<4>()
        .0
        .iter()
        .zip(vb_empty.as_chunks::<4>().0)
    {
        if p[..3] == e[..3] {
            continue;
        }
        vb_px += 1;
        let c = pack(p[0], p[1], p[2]);
        if vb_bad.is_none() && !explains(&vb_sheet, c) {
            vb_bad = Some(c);
        }
    }
    w.push(wit(
        "m44.the_baby_villager_draws_from_its_own_sheet",
        !vb_sheet.is_empty() && vb_px >= 200 && vb_bad.is_none(),
        format!(
            "{vb_px} px (want >= 200), first colour `villager_baby` cannot produce: {:?}",
            vb_bad.map(|c| format!("#{c:06x}"))
        ),
    ));

    // m45 — M319: the baby drowned draws from its two sheets (the base and
    // the outer layer), and the outer layer shows: some drawn pixels are
    // colours only the outer layer's sheet can produce. Until M319 it was the
    // adult mesh wearing the baby sheet's UV offset, which scrambled it.
    let db_base: HashSet<u32> = baked
        .mob_textures
        .iter()
        .find(|t| t.key == "drowned_baby")
        .map(|t| producible(&t.rgba))
        .unwrap_or_default();
    let db_outer: HashSet<u32> = baked
        .mob_textures
        .iter()
        .find(|t| t.key == "drowned_outer_baby")
        .map(|t| producible(&t.rgba))
        .unwrap_or_default();
    let mut dbst = stage_from(
        gpu,
        baked,
        cw,
        ch,
        Vec3::new(0.0, 1.0, -2.2),
        Vec3::new(0.0, 0.5, 0.0),
    )?;
    dbst.wr.set_entities(&[], dbst.right, dbst.up, 0.0);
    dbst.off
        .render(gpu, Some((&mut dbst.wr, dbst.vp)), &ov, BG)?;
    let db_empty = dbst.off.read_rgba(gpu)?;
    let db_draw = neutral_draw(EntityModelKind::DrownedBaby);
    dbst.wr.set_entities(&[db_draw], dbst.right, dbst.up, 0.0);
    dbst.off
        .render(gpu, Some((&mut dbst.wr, dbst.vp)), &ov, BG)?;
    let db_frame = dbst.off.read_rgba(gpu)?;
    dbst.destroy(gpu);
    let (mut db_px, mut db_bad, mut db_outer_only) = (0usize, None, 0usize);
    for (p, e) in db_frame
        .as_chunks::<4>()
        .0
        .iter()
        .zip(db_empty.as_chunks::<4>().0)
    {
        if p[..3] == e[..3] {
            continue;
        }
        db_px += 1;
        let c = pack(p[0], p[1], p[2]);
        let (in_base, in_outer) = (explains(&db_base, c), explains(&db_outer, c));
        if db_bad.is_none() && !in_base && !in_outer {
            db_bad = Some(c);
        }
        if in_outer && !in_base {
            db_outer_only += 1;
        }
    }
    w.push(wit(
        "m45.the_baby_drowned_draws_its_base_and_outer_layer",
        !db_base.is_empty() && db_px >= 200 && db_bad.is_none() && db_outer_only > 0,
        format!(
            "{db_px} px (want >= 200), first colour neither sheet can produce: {:?}; {} only \
             the outer layer's sheet can produce (want > 0)",
            db_bad.map(|c| format!("#{c:06x}")),
            db_outer_only
        ),
    ));

    // m46 — M320: both baby piglins draw, every pixel producible by their
    // own sheet. (The mesh and texel layout are pinned by the unit test
    // `the_baby_piglin_is_its_own_model`.) Until M320 the baby piglin was the
    // half-size adult and the baby zombified piglin the adult mesh wearing
    // the baby sheet's UV offset.
    let mut pgst = stage_from(
        gpu,
        baked,
        cw,
        ch,
        Vec3::new(0.0, 1.0, -2.2),
        Vec3::new(0.0, 0.5, 0.0),
    )?;
    pgst.wr.set_entities(&[], pgst.right, pgst.up, 0.0);
    pgst.off
        .render(gpu, Some((&mut pgst.wr, pgst.vp)), &ov, BG)?;
    let pg_empty = pgst.off.read_rgba(gpu)?;
    let mut pg_rows = Vec::new();
    let mut pg_ok = true;
    for (kind, key) in [
        (EntityModelKind::PiglinBaby, "piglin_baby"),
        (
            EntityModelKind::ZombifiedPiglinBaby,
            "zombified_piglin_baby",
        ),
    ] {
        let sheet: HashSet<u32> = baked
            .mob_textures
            .iter()
            .find(|t| t.key == key)
            .map(|t| producible(&t.rgba))
            .unwrap_or_default();
        pgst.wr
            .set_entities(&[neutral_draw(kind)], pgst.right, pgst.up, 0.0);
        pgst.off
            .render(gpu, Some((&mut pgst.wr, pgst.vp)), &ov, BG)?;
        let frame = pgst.off.read_rgba(gpu)?;
        let (mut px, mut bad) = (0usize, None);
        for (p, e) in frame
            .as_chunks::<4>()
            .0
            .iter()
            .zip(pg_empty.as_chunks::<4>().0)
        {
            if p[..3] == e[..3] {
                continue;
            }
            px += 1;
            let c = pack(p[0], p[1], p[2]);
            if bad.is_none() && !explains(&sheet, c) {
                bad = Some(c);
            }
        }
        pg_ok &= !sheet.is_empty() && px >= 200 && bad.is_none();
        pg_rows.push(format!(
            "{key}: {px} px, first colour it cannot produce {:?}",
            bad.map(|c| format!("#{c:06x}"))
        ));
    }
    pgst.destroy(gpu);
    w.push(wit(
        "m46.both_baby_piglins_draw_from_their_own_sheets",
        pg_ok,
        format!("{} (want >= 200 px each, none foreign)", pg_rows.join("; ")),
    ));

    // m47 — M321: both calves draw, every pixel producible by their own
    // sheet (the meshes rest on the unit test `the_calf_is_its_own_model`).
    // Until M321 a calf was the half-size adult on the adult's sheet.
    let mut cbst = stage_from(
        gpu,
        baked,
        cw,
        ch,
        Vec3::new(1.6, 1.2, -1.6),
        Vec3::new(0.0, 0.5, 0.0),
    )?;
    cbst.wr.set_entities(&[], cbst.right, cbst.up, 0.0);
    cbst.off
        .render(gpu, Some((&mut cbst.wr, cbst.vp)), &ov, BG)?;
    let cb_empty = cbst.off.read_rgba(gpu)?;
    let mut cb_rows = Vec::new();
    let mut cb_ok = true;
    for (kind, key) in [
        (EntityModelKind::CowBaby, "cow_baby"),
        (EntityModelKind::MooshroomBaby, "mooshroom_baby"),
    ] {
        let sheet: HashSet<u32> = baked
            .mob_textures
            .iter()
            .find(|t| t.key == key)
            .map(|t| producible(&t.rgba))
            .unwrap_or_default();
        cbst.wr
            .set_entities(&[neutral_draw(kind)], cbst.right, cbst.up, 0.0);
        cbst.off
            .render(gpu, Some((&mut cbst.wr, cbst.vp)), &ov, BG)?;
        let frame = cbst.off.read_rgba(gpu)?;
        let (mut px, mut bad) = (0usize, None);
        for (p, e) in frame
            .as_chunks::<4>()
            .0
            .iter()
            .zip(cb_empty.as_chunks::<4>().0)
        {
            if p[..3] == e[..3] {
                continue;
            }
            px += 1;
            let c = pack(p[0], p[1], p[2]);
            if bad.is_none() && !explains(&sheet, c) {
                bad = Some(c);
            }
        }
        cb_ok &= !sheet.is_empty() && px >= 200 && bad.is_none();
        cb_rows.push(format!(
            "{key}: {px} px, first colour it cannot produce {:?}",
            bad.map(|c| format!("#{c:06x}"))
        ));
    }
    cbst.destroy(gpu);
    w.push(wit(
        "m47.both_calves_draw_from_their_own_sheets",
        cb_ok,
        format!("{} (want >= 200 px each, none foreign)", cb_rows.join("; ")),
    ));

    // m48 — M322: the piglet and the chick draw, every pixel producible by
    // their own sheet (the meshes rest on the unit tests in
    // `piglet_chick_tests`). Until M322 each was the half-size adult.
    let mut pcst = stage_from(
        gpu,
        baked,
        cw,
        ch,
        Vec3::new(0.9, 0.7, -0.9),
        Vec3::new(0.0, 0.2, 0.0),
    )?;
    pcst.wr.set_entities(&[], pcst.right, pcst.up, 0.0);
    pcst.off
        .render(gpu, Some((&mut pcst.wr, pcst.vp)), &ov, BG)?;
    let pc_empty = pcst.off.read_rgba(gpu)?;
    let mut pc_rows = Vec::new();
    let mut pc_ok = true;
    for (kind, key) in [
        (EntityModelKind::PigBaby, "pig_baby"),
        (EntityModelKind::ChickenBaby, "chicken_baby"),
    ] {
        let sheet: HashSet<u32> = baked
            .mob_textures
            .iter()
            .find(|t| t.key == key)
            .map(|t| producible(&t.rgba))
            .unwrap_or_default();
        pcst.wr
            .set_entities(&[neutral_draw(kind)], pcst.right, pcst.up, 0.0);
        pcst.off
            .render(gpu, Some((&mut pcst.wr, pcst.vp)), &ov, BG)?;
        let frame = pcst.off.read_rgba(gpu)?;
        let (mut px, mut bad) = (0usize, None);
        for (p, e) in frame
            .as_chunks::<4>()
            .0
            .iter()
            .zip(pc_empty.as_chunks::<4>().0)
        {
            if p[..3] == e[..3] {
                continue;
            }
            px += 1;
            let c = pack(p[0], p[1], p[2]);
            if bad.is_none() && !explains(&sheet, c) {
                bad = Some(c);
            }
        }
        pc_ok &= !sheet.is_empty() && px >= 200 && bad.is_none();
        pc_rows.push(format!(
            "{key}: {px} px, first colour it cannot produce {:?}",
            bad.map(|c| format!("#{c:06x}"))
        ));
    }
    pcst.destroy(gpu);
    w.push(wit(
        "m48.the_piglet_and_the_chick_draw_from_their_own_sheets",
        pc_ok,
        format!("{} (want >= 200 px each, none foreign)", pc_rows.join("; ")),
    ));

    // m49 — M323: the lamb draws from its own two sheets, its fleece draws,
    // and shearing takes it away: the unshorn and shorn frames must differ.
    // (A colour test cannot see the fleece: `sheep_baby.png` already carries
    // its greys.) The fleece is coplanar, so it draws only through the EQUAL
    // range and the shearing gate M323 put in front of it; the mesh rests on
    // `the_lamb_is_its_own_model_and_its_fleece_is_coplanar`.
    let mut lbst = stage_from(
        gpu,
        baked,
        cw,
        ch,
        Vec3::new(1.2, 0.9, -1.2),
        Vec3::new(0.0, 0.4, 0.0),
    )?;
    lbst.wr.set_entities(&[], lbst.right, lbst.up, 0.0);
    lbst.off
        .render(gpu, Some((&mut lbst.wr, lbst.vp)), &ov, BG)?;
    let lb_empty = lbst.off.read_rgba(gpu)?;
    let lb_own: HashSet<u32> = baked
        .mob_textures
        .iter()
        .filter(|t| t.key == "sheep_baby" || t.key == "sheep_wool_baby")
        .flat_map(|t| producible(&t.rgba))
        .collect();
    let mut lb_rows = Vec::new();
    let mut lb_frames = Vec::new();
    for sheared in [false, true] {
        let d = EntityDraw {
            sheared,
            ..neutral_draw(EntityModelKind::SheepBaby)
        };
        lbst.wr.set_entities(&[d], lbst.right, lbst.up, 0.0);
        lbst.off
            .render(gpu, Some((&mut lbst.wr, lbst.vp)), &ov, BG)?;
        let frame = lbst.off.read_rgba(gpu)?;
        let (mut px, mut foreign) = (0usize, 0usize);
        for (p, e) in frame
            .as_chunks::<4>()
            .0
            .iter()
            .zip(lb_empty.as_chunks::<4>().0)
        {
            if p[..3] == e[..3] {
                continue;
            }
            px += 1;
            foreign += usize::from(!explains(&lb_own, pack(p[0], p[1], p[2])));
        }
        lb_rows.push((px, foreign));
        lb_frames.push(frame);
    }
    lbst.destroy(gpu);
    let fleece_px = lb_frames[0]
        .as_chunks::<4>()
        .0
        .iter()
        .zip(lb_frames[1].as_chunks::<4>().0)
        .filter(|(a, b)| a[..3] != b[..3])
        .count();
    let [(px, foreign), (spx, sforeign)] = [lb_rows[0], lb_rows[1]];
    w.push(wit(
        "m49.the_lamb_draws_its_fleece_and_a_shorn_lamb_none",
        lb_own.len() > 1 && px >= 200 && spx >= 200 && fleece_px >= 200 && foreign + sforeign == 0,
        format!(
            "lamb {px} px, shorn {spx} px, {fleece_px} px differ; {foreign} + {sforeign} px \
             neither sheet can produce (want >= 200 px each, >= 200 differing, 0 foreign)"
        ),
    ));

    // m50 — M324: the dolphin calf draws, every pixel producible by its own
    // sheet. This does NOT tell the calf's sheet from the adult's: measured
    // at M324, the calf drawn on `dolphin` passes too, and in this view each
    // sheet produces every colour the other draws (`MAX_AMBIGUOUS` notes the
    // adult's half), so no colour test here can. It proves the calf draws
    // through the textured path in the dolphin's palette; the binding is its
    // `MobDef` row and the mesh rests on `the_dolphin_calf_is_its_own_model`.
    let mut dcst = stage_from(
        gpu,
        baked,
        cw,
        ch,
        Vec3::new(1.4, 0.9, -1.4),
        Vec3::new(0.0, 0.2, 0.0),
    )?;
    dcst.wr.set_entities(&[], dcst.right, dcst.up, 0.0);
    dcst.off
        .render(gpu, Some((&mut dcst.wr, dcst.vp)), &ov, BG)?;
    let dc_empty = dcst.off.read_rgba(gpu)?;
    let dc_sheet: HashSet<u32> = baked
        .mob_textures
        .iter()
        .find(|t| t.key == "dolphin_baby")
        .map(|t| producible(&t.rgba))
        .unwrap_or_default();
    dcst.wr.set_entities(
        &[neutral_draw(EntityModelKind::DolphinBaby)],
        dcst.right,
        dcst.up,
        0.0,
    );
    dcst.off
        .render(gpu, Some((&mut dcst.wr, dcst.vp)), &ov, BG)?;
    let dc_frame = dcst.off.read_rgba(gpu)?;
    dcst.destroy(gpu);
    let (mut dc_px, mut dc_bad) = (0usize, None);
    for (p, e) in dc_frame
        .as_chunks::<4>()
        .0
        .iter()
        .zip(dc_empty.as_chunks::<4>().0)
    {
        if p[..3] == e[..3] {
            continue;
        }
        dc_px += 1;
        let c = pack(p[0], p[1], p[2]);
        if dc_bad.is_none() && !explains(&dc_sheet, c) {
            dc_bad = Some(c);
        }
    }
    w.push(wit(
        "m50.the_dolphin_calf_draws_from_its_own_sheet",
        !dc_sheet.is_empty() && dc_px >= 200 && dc_bad.is_none(),
        format!(
            "{dc_px} px (want >= 200), first colour its sheet cannot produce {:?}",
            dc_bad.map(|c| format!("#{c:06x}"))
        ),
    ));

    // m51 — M325: the baby zombie villager draws, every pixel producible by
    // its own sheet; the mesh rests on
    // `the_baby_zombie_villager_is_its_own_model`.
    let mut zvst = stage_from(
        gpu,
        baked,
        cw,
        ch,
        Vec3::new(1.2, 1.0, -1.2),
        Vec3::new(0.0, 0.5, 0.0),
    )?;
    zvst.wr.set_entities(&[], zvst.right, zvst.up, 0.0);
    zvst.off
        .render(gpu, Some((&mut zvst.wr, zvst.vp)), &ov, BG)?;
    let zv_empty = zvst.off.read_rgba(gpu)?;
    let zv_sheet: HashSet<u32> = baked
        .mob_textures
        .iter()
        .find(|t| t.key == "zombie_villager_baby")
        .map(|t| producible(&t.rgba))
        .unwrap_or_default();
    zvst.wr.set_entities(
        &[neutral_draw(EntityModelKind::ZombieVillagerBaby)],
        zvst.right,
        zvst.up,
        0.0,
    );
    zvst.off
        .render(gpu, Some((&mut zvst.wr, zvst.vp)), &ov, BG)?;
    let zv_frame = zvst.off.read_rgba(gpu)?;
    zvst.destroy(gpu);
    let (mut zv_px, mut zv_bad) = (0usize, None);
    for (p, e) in zv_frame
        .as_chunks::<4>()
        .0
        .iter()
        .zip(zv_empty.as_chunks::<4>().0)
    {
        if p[..3] == e[..3] {
            continue;
        }
        zv_px += 1;
        let c = pack(p[0], p[1], p[2]);
        if zv_bad.is_none() && !explains(&zv_sheet, c) {
            zv_bad = Some(c);
        }
    }
    w.push(wit(
        "m51.the_baby_zombie_villager_draws_from_its_own_sheet",
        !zv_sheet.is_empty() && zv_px >= 200 && zv_bad.is_none(),
        format!(
            "{zv_px} px (want >= 200), first colour its sheet cannot produce {:?}",
            zv_bad.map(|c| format!("#{c:06x}"))
        ),
    ));

    // m52 — M326: both baby squids draw, every pixel producible by their
    // own sheet (the mesh rests on `the_baby_squid_is_its_own_model`).
    let mut sqst = stage_from(
        gpu,
        baked,
        cw,
        ch,
        Vec3::new(1.3, 1.3, -1.3),
        Vec3::new(0.0, 0.5, 0.0),
    )?;
    sqst.wr.set_entities(&[], sqst.right, sqst.up, 0.0);
    sqst.off
        .render(gpu, Some((&mut sqst.wr, sqst.vp)), &ov, BG)?;
    let sq_empty = sqst.off.read_rgba(gpu)?;
    let mut sq_rows = Vec::new();
    let mut sq_ok = true;
    for (kind, key) in [
        (EntityModelKind::SquidBaby, "squid_baby"),
        (EntityModelKind::GlowSquidBaby, "glow_squid_baby"),
    ] {
        let sheet: HashSet<u32> = baked
            .mob_textures
            .iter()
            .find(|t| t.key == key)
            .map(|t| producible(&t.rgba))
            .unwrap_or_default();
        sqst.wr
            .set_entities(&[neutral_draw(kind)], sqst.right, sqst.up, 0.0);
        sqst.off
            .render(gpu, Some((&mut sqst.wr, sqst.vp)), &ov, BG)?;
        let frame = sqst.off.read_rgba(gpu)?;
        let (mut px, mut bad) = (0usize, None);
        for (p, e) in frame
            .as_chunks::<4>()
            .0
            .iter()
            .zip(sq_empty.as_chunks::<4>().0)
        {
            if p[..3] == e[..3] {
                continue;
            }
            px += 1;
            let c = pack(p[0], p[1], p[2]);
            if bad.is_none() && !explains(&sheet, c) {
                bad = Some(c);
            }
        }
        sq_ok &= !sheet.is_empty() && px >= 200 && bad.is_none();
        sq_rows.push(format!(
            "{key}: {px} px, first colour it cannot produce {:?}",
            bad.map(|c| format!("#{c:06x}"))
        ));
    }
    sqst.destroy(gpu);
    w.push(wit(
        "m52.both_baby_squids_draw_from_their_own_sheets",
        sq_ok,
        format!("{} (want >= 200 px each, none foreign)", sq_rows.join("; ")),
    ));

    // m53 — M327: the hoglet and the baby zoglin draw, every pixel
    // producible by their own sheet (the mesh rests on
    // `the_hoglet_is_its_own_model`).
    let mut hgst = stage_from(
        gpu,
        baked,
        cw,
        ch,
        Vec3::new(1.4, 1.1, -1.4),
        Vec3::new(0.0, 0.4, 0.0),
    )?;
    hgst.wr.set_entities(&[], hgst.right, hgst.up, 0.0);
    hgst.off
        .render(gpu, Some((&mut hgst.wr, hgst.vp)), &ov, BG)?;
    let hg_empty = hgst.off.read_rgba(gpu)?;
    let mut hg_rows = Vec::new();
    let mut hg_ok = true;
    for (kind, key) in [
        (EntityModelKind::HoglinBaby, "hoglin_baby"),
        (EntityModelKind::ZoglinBaby, "zoglin_baby"),
    ] {
        let sheet: HashSet<u32> = baked
            .mob_textures
            .iter()
            .find(|t| t.key == key)
            .map(|t| producible(&t.rgba))
            .unwrap_or_default();
        hgst.wr
            .set_entities(&[neutral_draw(kind)], hgst.right, hgst.up, 0.0);
        hgst.off
            .render(gpu, Some((&mut hgst.wr, hgst.vp)), &ov, BG)?;
        let frame = hgst.off.read_rgba(gpu)?;
        let (mut px, mut bad) = (0usize, None);
        for (p, e) in frame
            .as_chunks::<4>()
            .0
            .iter()
            .zip(hg_empty.as_chunks::<4>().0)
        {
            if p[..3] == e[..3] {
                continue;
            }
            px += 1;
            let c = pack(p[0], p[1], p[2]);
            if bad.is_none() && !explains(&sheet, c) {
                bad = Some(c);
            }
        }
        hg_ok &= !sheet.is_empty() && px >= 200 && bad.is_none();
        hg_rows.push(format!(
            "{key}: {px} px, first colour it cannot produce {:?}",
            bad.map(|c| format!("#{c:06x}"))
        ));
    }
    hgst.destroy(gpu);
    w.push(wit(
        "m53.the_hoglet_and_the_baby_zoglin_draw_from_their_own_sheets",
        hg_ok,
        format!("{} (want >= 200 px each, none foreign)", hg_rows.join("; ")),
    ));

    // m54 — M328: the baby strider draws, every pixel producible by its own
    // sheet; the mesh rests on `the_baby_strider_is_its_own_model`.
    let mut bsst = stage_from(
        gpu,
        baked,
        cw,
        ch,
        Vec3::new(1.1, 0.9, -1.1),
        Vec3::new(0.0, 0.4, 0.0),
    )?;
    bsst.wr.set_entities(&[], bsst.right, bsst.up, 0.0);
    bsst.off
        .render(gpu, Some((&mut bsst.wr, bsst.vp)), &ov, BG)?;
    let bs_empty = bsst.off.read_rgba(gpu)?;
    let bs_sheet: HashSet<u32> = baked
        .mob_textures
        .iter()
        .find(|t| t.key == "strider_baby")
        .map(|t| producible(&t.rgba))
        .unwrap_or_default();
    bsst.wr.set_entities(
        &[neutral_draw(EntityModelKind::StriderBaby)],
        bsst.right,
        bsst.up,
        0.0,
    );
    bsst.off
        .render(gpu, Some((&mut bsst.wr, bsst.vp)), &ov, BG)?;
    let bs_frame = bsst.off.read_rgba(gpu)?;
    bsst.destroy(gpu);
    let (mut bs_px, mut bs_bad) = (0usize, None);
    for (p, e) in bs_frame
        .as_chunks::<4>()
        .0
        .iter()
        .zip(bs_empty.as_chunks::<4>().0)
    {
        if p[..3] == e[..3] {
            continue;
        }
        bs_px += 1;
        let c = pack(p[0], p[1], p[2]);
        if bs_bad.is_none() && !explains(&bs_sheet, c) {
            bs_bad = Some(c);
        }
    }
    w.push(wit(
        "m54.the_baby_strider_draws_from_its_own_sheet",
        !bs_sheet.is_empty() && bs_px >= 200 && bs_bad.is_none(),
        format!(
            "{bs_px} px (want >= 200), first colour its sheet cannot produce {:?}",
            bs_bad.map(|c| format!("#{c:06x}"))
        ),
    ));

    // m55 — M330: the kit draws, every pixel producible by its own sheet;
    // the mesh and its walk rig rest on
    // `the_kit_is_its_own_model_on_its_own_walk`.
    let mut kfst = stage_from(
        gpu,
        baked,
        cw,
        ch,
        Vec3::new(0.9, 0.7, -0.9),
        Vec3::new(0.0, 0.2, 0.0),
    )?;
    kfst.wr.set_entities(&[], kfst.right, kfst.up, 0.0);
    kfst.off
        .render(gpu, Some((&mut kfst.wr, kfst.vp)), &ov, BG)?;
    let kf_empty = kfst.off.read_rgba(gpu)?;
    let kf_sheet: HashSet<u32> = baked
        .mob_textures
        .iter()
        .find(|t| t.key == "fox_baby")
        .map(|t| producible(&t.rgba))
        .unwrap_or_default();
    kfst.wr.set_entities(
        &[neutral_draw(EntityModelKind::FoxBaby)],
        kfst.right,
        kfst.up,
        0.0,
    );
    kfst.off
        .render(gpu, Some((&mut kfst.wr, kfst.vp)), &ov, BG)?;
    let kf_frame = kfst.off.read_rgba(gpu)?;
    kfst.destroy(gpu);
    let (mut kf_px, mut kf_bad) = (0usize, None);
    for (p, e) in kf_frame
        .as_chunks::<4>()
        .0
        .iter()
        .zip(kf_empty.as_chunks::<4>().0)
    {
        if p[..3] == e[..3] {
            continue;
        }
        kf_px += 1;
        let c = pack(p[0], p[1], p[2]);
        if kf_bad.is_none() && !explains(&kf_sheet, c) {
            kf_bad = Some(c);
        }
    }
    w.push(wit(
        "m55.the_kit_draws_from_its_own_sheet",
        !kf_sheet.is_empty() && kf_px >= 200 && kf_bad.is_none(),
        format!(
            "{kf_px} px (want >= 200), first colour its sheet cannot produce {:?}",
            kf_bad.map(|c| format!("#{c:06x}"))
        ),
    ));

    // m56 — M331: the panda cub draws, every pixel producible by its own
    // sheet; the mesh rests on `the_panda_cub_is_its_own_model`.
    let mut pdst = stage_from(
        gpu,
        baked,
        cw,
        ch,
        Vec3::new(1.2, 0.9, -1.2),
        Vec3::new(0.0, 0.3, 0.0),
    )?;
    pdst.wr.set_entities(&[], pdst.right, pdst.up, 0.0);
    pdst.off
        .render(gpu, Some((&mut pdst.wr, pdst.vp)), &ov, BG)?;
    let pd_empty = pdst.off.read_rgba(gpu)?;
    let pd_sheet: HashSet<u32> = baked
        .mob_textures
        .iter()
        .find(|t| t.key == "panda_baby")
        .map(|t| producible(&t.rgba))
        .unwrap_or_default();
    pdst.wr.set_entities(
        &[neutral_draw(EntityModelKind::PandaBaby)],
        pdst.right,
        pdst.up,
        0.0,
    );
    pdst.off
        .render(gpu, Some((&mut pdst.wr, pdst.vp)), &ov, BG)?;
    let pd_frame = pdst.off.read_rgba(gpu)?;
    pdst.destroy(gpu);
    let (mut pd_px, mut pd_bad) = (0usize, None);
    for (p, e) in pd_frame
        .as_chunks::<4>()
        .0
        .iter()
        .zip(pd_empty.as_chunks::<4>().0)
    {
        if p[..3] == e[..3] {
            continue;
        }
        pd_px += 1;
        let c = pack(p[0], p[1], p[2]);
        if pd_bad.is_none() && !explains(&pd_sheet, c) {
            pd_bad = Some(c);
        }
    }
    w.push(wit(
        "m56.the_panda_cub_draws_from_its_own_sheet",
        !pd_sheet.is_empty() && pd_px >= 200 && pd_bad.is_none(),
        format!(
            "{pd_px} px (want >= 200), first colour its sheet cannot produce {:?}",
            pd_bad.map(|c| format!("#{c:06x}"))
        ),
    ));

    // m57 — M332: the ocelot kitten draws, every pixel producible by its
    // own sheet; the mesh and walk rest on
    // `the_kitten_is_its_own_model_on_vanilla_s_walk`.
    let mut ocst = stage_from(
        gpu,
        baked,
        cw,
        ch,
        Vec3::new(0.8, 0.6, -0.8),
        Vec3::new(0.0, 0.2, 0.0),
    )?;
    ocst.wr.set_entities(&[], ocst.right, ocst.up, 0.0);
    ocst.off
        .render(gpu, Some((&mut ocst.wr, ocst.vp)), &ov, BG)?;
    let oc_empty = ocst.off.read_rgba(gpu)?;
    let oc_sheet: HashSet<u32> = baked
        .mob_textures
        .iter()
        .find(|t| t.key == "ocelot_baby")
        .map(|t| producible(&t.rgba))
        .unwrap_or_default();
    ocst.wr.set_entities(
        &[neutral_draw(EntityModelKind::OcelotBaby)],
        ocst.right,
        ocst.up,
        0.0,
    );
    ocst.off
        .render(gpu, Some((&mut ocst.wr, ocst.vp)), &ov, BG)?;
    let oc_frame = ocst.off.read_rgba(gpu)?;
    ocst.destroy(gpu);
    let (mut oc_px, mut oc_bad) = (0usize, None);
    for (p, e) in oc_frame
        .as_chunks::<4>()
        .0
        .iter()
        .zip(oc_empty.as_chunks::<4>().0)
    {
        if p[..3] == e[..3] {
            continue;
        }
        oc_px += 1;
        let c = pack(p[0], p[1], p[2]);
        if oc_bad.is_none() && !explains(&oc_sheet, c) {
            oc_bad = Some(c);
        }
    }
    w.push(wit(
        "m57.the_ocelot_kitten_draws_from_its_own_sheet",
        !oc_sheet.is_empty() && oc_px >= 200 && oc_bad.is_none(),
        format!(
            "{oc_px} px (want >= 200), first colour its sheet cannot produce {:?}",
            oc_bad.map(|c| format!("#{c:06x}"))
        ),
    ));

    // m58 — M333: the baby bee draws, every pixel producible by its own
    // sheet; the mesh rests on `the_baby_bee_is_its_own_model`.
    let mut bbst = stage_from(
        gpu,
        baked,
        cw,
        ch,
        Vec3::new(0.7, 0.7, -0.7),
        Vec3::new(0.0, 0.25, 0.0),
    )?;
    bbst.wr.set_entities(&[], bbst.right, bbst.up, 0.0);
    bbst.off
        .render(gpu, Some((&mut bbst.wr, bbst.vp)), &ov, BG)?;
    let bb_empty = bbst.off.read_rgba(gpu)?;
    let bb_sheet: HashSet<u32> = baked
        .mob_textures
        .iter()
        .find(|t| t.key == "bee_baby")
        .map(|t| producible(&t.rgba))
        .unwrap_or_default();
    bbst.wr.set_entities(
        &[neutral_draw(EntityModelKind::BeeBaby)],
        bbst.right,
        bbst.up,
        0.0,
    );
    bbst.off
        .render(gpu, Some((&mut bbst.wr, bbst.vp)), &ov, BG)?;
    let bb_frame = bbst.off.read_rgba(gpu)?;
    bbst.destroy(gpu);
    let (mut bb_px, mut bb_bad) = (0usize, None);
    for (p, e) in bb_frame
        .as_chunks::<4>()
        .0
        .iter()
        .zip(bb_empty.as_chunks::<4>().0)
    {
        if p[..3] == e[..3] {
            continue;
        }
        bb_px += 1;
        let c = pack(p[0], p[1], p[2]);
        if bb_bad.is_none() && !explains(&bb_sheet, c) {
            bb_bad = Some(c);
        }
    }
    w.push(wit(
        "m58.the_baby_bee_draws_from_its_own_sheet",
        !bb_sheet.is_empty() && bb_px >= 200 && bb_bad.is_none(),
        format!(
            "{bb_px} px (want >= 200), first colour its sheet cannot produce {:?}",
            bb_bad.map(|c| format!("#{c:06x}"))
        ),
    ));

    // m59 — M334: both foals draw, every pixel producible by their own
    // sheet (the mesh rests on `the_foal_is_its_own_model`).
    let mut flst = stage_from(
        gpu,
        baked,
        cw,
        ch,
        Vec3::new(1.8, 1.4, -1.8),
        Vec3::new(0.0, 0.6, 0.0),
    )?;
    flst.wr.set_entities(&[], flst.right, flst.up, 0.0);
    flst.off
        .render(gpu, Some((&mut flst.wr, flst.vp)), &ov, BG)?;
    let fl_empty = flst.off.read_rgba(gpu)?;
    let mut fl_rows = Vec::new();
    let mut fl_ok = true;
    for (kind, key) in [
        (EntityModelKind::DonkeyBaby, "donkey_baby"),
        (EntityModelKind::MuleBaby, "mule_baby"),
    ] {
        let sheet: HashSet<u32> = baked
            .mob_textures
            .iter()
            .find(|t| t.key == key)
            .map(|t| producible(&t.rgba))
            .unwrap_or_default();
        flst.wr
            .set_entities(&[neutral_draw(kind)], flst.right, flst.up, 0.0);
        flst.off
            .render(gpu, Some((&mut flst.wr, flst.vp)), &ov, BG)?;
        let frame = flst.off.read_rgba(gpu)?;
        let (mut px, mut bad) = (0usize, None);
        for (p, e) in frame
            .as_chunks::<4>()
            .0
            .iter()
            .zip(fl_empty.as_chunks::<4>().0)
        {
            if p[..3] == e[..3] {
                continue;
            }
            px += 1;
            let c = pack(p[0], p[1], p[2]);
            if bad.is_none() && !explains(&sheet, c) {
                bad = Some(c);
            }
        }
        fl_ok &= !sheet.is_empty() && px >= 200 && bad.is_none();
        fl_rows.push(format!(
            "{key}: {px} px, first colour it cannot produce {:?}",
            bad.map(|c| format!("#{c:06x}"))
        ));
    }
    flst.destroy(gpu);
    w.push(wit(
        "m59.both_foals_draw_from_their_own_sheets",
        fl_ok,
        format!("{} (want >= 200 px each, none foreign)", fl_rows.join("; ")),
    ));

    // m60 — M335: the camel calf draws, every pixel producible by its own
    // sheet; the mesh and walk rest on
    // `the_camel_calf_is_its_own_model_on_its_own_walk`.
    let mut ccst = stage_from(
        gpu,
        baked,
        cw,
        ch,
        Vec3::new(2.0, 1.6, -2.0),
        Vec3::new(0.0, 0.8, 0.0),
    )?;
    ccst.wr.set_entities(&[], ccst.right, ccst.up, 0.0);
    ccst.off
        .render(gpu, Some((&mut ccst.wr, ccst.vp)), &ov, BG)?;
    let cc_empty = ccst.off.read_rgba(gpu)?;
    let cc_sheet: HashSet<u32> = baked
        .mob_textures
        .iter()
        .find(|t| t.key == "camel_baby")
        .map(|t| producible(&t.rgba))
        .unwrap_or_default();
    ccst.wr.set_entities(
        &[neutral_draw(EntityModelKind::CamelBaby)],
        ccst.right,
        ccst.up,
        0.0,
    );
    ccst.off
        .render(gpu, Some((&mut ccst.wr, ccst.vp)), &ov, BG)?;
    let cc_frame = ccst.off.read_rgba(gpu)?;
    ccst.destroy(gpu);
    let (mut cc_px, mut cc_bad) = (0usize, None);
    for (p, e) in cc_frame
        .as_chunks::<4>()
        .0
        .iter()
        .zip(cc_empty.as_chunks::<4>().0)
    {
        if p[..3] == e[..3] {
            continue;
        }
        cc_px += 1;
        let c = pack(p[0], p[1], p[2]);
        if cc_bad.is_none() && !explains(&cc_sheet, c) {
            cc_bad = Some(c);
        }
    }
    w.push(wit(
        "m60.the_camel_calf_draws_from_its_own_sheet",
        !cc_sheet.is_empty() && cc_px >= 200 && cc_bad.is_none(),
        format!(
            "{cc_px} px (want >= 200), first colour its sheet cannot produce {:?}",
            cc_bad.map(|c| format!("#{c:06x}"))
        ),
    ));

    // m61 — M336: the rabbit kit draws, every pixel producible by its own
    // sheet. Like m50 this does NOT tell its sheet from the adult's: drawn
    // on `rabbit` it passes too, and measured at M336 none of its 4,630
    // pixels is one only the kit's sheet can produce (the brown coats cover
    // each other). The mesh and hop rest on
    // `the_rabbit_kit_is_its_own_model_on_its_own_hop`.
    let mut rkst = stage_from(
        gpu,
        baked,
        cw,
        ch,
        Vec3::new(0.7, 0.6, -0.7),
        Vec3::new(0.0, 0.2, 0.0),
    )?;
    rkst.wr.set_entities(&[], rkst.right, rkst.up, 0.0);
    rkst.off
        .render(gpu, Some((&mut rkst.wr, rkst.vp)), &ov, BG)?;
    let rk_empty = rkst.off.read_rgba(gpu)?;
    let rk_sheet: HashSet<u32> = baked
        .mob_textures
        .iter()
        .find(|t| t.key == "rabbit_baby")
        .map(|t| producible(&t.rgba))
        .unwrap_or_default();
    rkst.wr.set_entities(
        &[neutral_draw(EntityModelKind::RabbitBaby)],
        rkst.right,
        rkst.up,
        0.0,
    );
    rkst.off
        .render(gpu, Some((&mut rkst.wr, rkst.vp)), &ov, BG)?;
    let rk_frame = rkst.off.read_rgba(gpu)?;
    rkst.destroy(gpu);
    let (mut rk_px, mut rk_bad) = (0usize, None);
    for (p, e) in rk_frame
        .as_chunks::<4>()
        .0
        .iter()
        .zip(rk_empty.as_chunks::<4>().0)
    {
        if p[..3] == e[..3] {
            continue;
        }
        rk_px += 1;
        let c = pack(p[0], p[1], p[2]);
        if rk_bad.is_none() && !explains(&rk_sheet, c) {
            rk_bad = Some(c);
        }
    }
    w.push(wit(
        "m61.the_rabbit_kit_draws_from_its_own_sheet",
        !rk_sheet.is_empty() && rk_px >= 200 && rk_bad.is_none(),
        format!(
            "{rk_px} px (want >= 200), first colour its sheet cannot produce {:?}",
            rk_bad.map(|c| format!("#{c:06x}"))
        ),
    ));

    // m62 — M337: the goat kid draws, every pixel producible by its own
    // sheet; the mesh rests on `the_kid_is_its_own_model`.
    let mut gkst = stage_from(
        gpu,
        baked,
        cw,
        ch,
        Vec3::new(1.0, 0.8, -1.0),
        Vec3::new(0.0, 0.4, 0.0),
    )?;
    gkst.wr.set_entities(&[], gkst.right, gkst.up, 0.0);
    gkst.off
        .render(gpu, Some((&mut gkst.wr, gkst.vp)), &ov, BG)?;
    let gk_empty = gkst.off.read_rgba(gpu)?;
    let gk_sheet: HashSet<u32> = baked
        .mob_textures
        .iter()
        .find(|t| t.key == "goat_baby")
        .map(|t| producible(&t.rgba))
        .unwrap_or_default();
    gkst.wr.set_entities(
        &[neutral_draw(EntityModelKind::GoatBaby)],
        gkst.right,
        gkst.up,
        0.0,
    );
    gkst.off
        .render(gpu, Some((&mut gkst.wr, gkst.vp)), &ov, BG)?;
    let gk_frame = gkst.off.read_rgba(gpu)?;
    gkst.destroy(gpu);
    let (mut gk_px, mut gk_bad) = (0usize, None);
    for (p, e) in gk_frame
        .as_chunks::<4>()
        .0
        .iter()
        .zip(gk_empty.as_chunks::<4>().0)
    {
        if p[..3] == e[..3] {
            continue;
        }
        gk_px += 1;
        let c = pack(p[0], p[1], p[2]);
        if gk_bad.is_none() && !explains(&gk_sheet, c) {
            gk_bad = Some(c);
        }
    }
    w.push(wit(
        "m62.the_goat_kid_draws_from_its_own_sheet",
        !gk_sheet.is_empty() && gk_px >= 200 && gk_bad.is_none(),
        format!(
            "{gk_px} px (want >= 200), first colour its sheet cannot produce {:?}",
            gk_bad.map(|c| format!("#{c:06x}"))
        ),
    ));

    // m63 — M338: the baby llama draws its creamy base, and with the gray
    // coat's variant id (`mob_variants::llama_baby_texture(3)`, the id the
    // app routes a gray baby to) it draws the gray baby sheet: every pixel
    // of each frame is producible by its own sheet, and the two differ. The
    // mesh rests on `the_baby_llama_is_its_own_model_without_chests`.
    let mut lkst = stage_from(
        gpu,
        baked,
        cw,
        ch,
        Vec3::new(1.5, 1.2, -1.5),
        Vec3::new(0.0, 0.6, 0.0),
    )?;
    lkst.wr.set_entities(&[], lkst.right, lkst.up, 0.0);
    lkst.off
        .render(gpu, Some((&mut lkst.wr, lkst.vp)), &ov, BG)?;
    let lk_empty = lkst.off.read_rgba(gpu)?;
    let gray_id =
        rewo_data::mob_variants::variant_id(rewo_data::mob_variants::llama_baby_texture(3))
            .unwrap_or(0);
    let mut lk_rows = Vec::new();
    let mut lk_frames = Vec::new();
    let mut lk_ok = gray_id != 0;
    // The creamy base is `llama_baby`'s own sheet; the gray coat is the
    // variant sheet the bake made for that key and id.
    let lk_base: HashSet<u32> = baked
        .mob_textures
        .iter()
        .find(|t| t.key == "llama_baby")
        .map(|t| producible(&t.rgba))
        .unwrap_or_default();
    let lk_gray: HashSet<u32> = baked
        .mob_variant_textures
        .iter()
        .find(|t| t.key == "llama_baby" && t.index == gray_id)
        .map(|t| producible(&t.rgba))
        .unwrap_or_default();
    for (variant, rel, sheet) in [(0u16, "creamy", lk_base), (gray_id, "gray", lk_gray)] {
        let d = EntityDraw {
            variant,
            ..neutral_draw(EntityModelKind::LlamaBaby)
        };
        lkst.wr.set_entities(&[d], lkst.right, lkst.up, 0.0);
        lkst.off
            .render(gpu, Some((&mut lkst.wr, lkst.vp)), &ov, BG)?;
        let frame = lkst.off.read_rgba(gpu)?;
        let (mut px, mut bad) = (0usize, None);
        for (p, e) in frame
            .as_chunks::<4>()
            .0
            .iter()
            .zip(lk_empty.as_chunks::<4>().0)
        {
            if p[..3] == e[..3] {
                continue;
            }
            px += 1;
            let c = pack(p[0], p[1], p[2]);
            if bad.is_none() && !explains(&sheet, c) {
                bad = Some(c);
            }
        }
        lk_ok &= !sheet.is_empty() && px >= 200 && bad.is_none();
        lk_rows.push(format!(
            "{rel}: {px} px, first colour it cannot produce {:?}",
            bad.map(|c| format!("#{c:06x}"))
        ));
        lk_frames.push(frame);
    }
    lkst.destroy(gpu);
    let lk_diff = diff_bytes(&lk_frames[0], &lk_frames[1]);
    lk_ok &= lk_diff > 0;
    w.push(wit(
        "m63.the_baby_llama_wears_its_coat_s_baby_sheet",
        lk_ok,
        format!(
            "gray variant id {gray_id}; {}; {lk_diff} bytes differ between the coats \
             (want >= 200 px each, none foreign, the coats different)",
            lk_rows.join("; ")
        ),
    ));

    // m64 — M339: the horse foal draws its brown base, and on the black
    // coat's variant id (`horse_baby_texture(4)`) the black foal sheet: each
    // frame is producible by its own sheet, and the two differ. The mesh's
    // rest angles rest on `the_horse_foal_rests_where_setup_anim_puts_it`.
    let mut hfst = stage_from(
        gpu,
        baked,
        cw,
        ch,
        Vec3::new(1.8, 1.4, -1.8),
        Vec3::new(0.0, 0.7, 0.0),
    )?;
    hfst.wr.set_entities(&[], hfst.right, hfst.up, 0.0);
    hfst.off
        .render(gpu, Some((&mut hfst.wr, hfst.vp)), &ov, BG)?;
    let hf_empty = hfst.off.read_rgba(gpu)?;
    let black_id =
        rewo_data::mob_variants::variant_id(rewo_data::mob_variants::horse_baby_texture(4))
            .unwrap_or(0);
    let hf_base: HashSet<u32> = baked
        .mob_textures
        .iter()
        .find(|t| t.key == "horse_baby")
        .map(|t| producible(&t.rgba))
        .unwrap_or_default();
    let hf_black: HashSet<u32> = baked
        .mob_variant_textures
        .iter()
        .find(|t| t.key == "horse_baby" && t.index == black_id)
        .map(|t| producible(&t.rgba))
        .unwrap_or_default();
    let mut hf_rows = Vec::new();
    let mut hf_frames = Vec::new();
    let mut hf_ok = black_id != 0;
    for (variant, coat, sheet) in [(0u16, "brown", hf_base), (black_id, "black", hf_black)] {
        let d = EntityDraw {
            variant,
            ..neutral_draw(EntityModelKind::HorseBaby)
        };
        hfst.wr.set_entities(&[d], hfst.right, hfst.up, 0.0);
        hfst.off
            .render(gpu, Some((&mut hfst.wr, hfst.vp)), &ov, BG)?;
        let frame = hfst.off.read_rgba(gpu)?;
        let (mut px, mut bad) = (0usize, None);
        for (p, e) in frame
            .as_chunks::<4>()
            .0
            .iter()
            .zip(hf_empty.as_chunks::<4>().0)
        {
            if p[..3] == e[..3] {
                continue;
            }
            px += 1;
            let c = pack(p[0], p[1], p[2]);
            if bad.is_none() && !explains(&sheet, c) {
                bad = Some(c);
            }
        }
        hf_ok &= !sheet.is_empty() && px >= 200 && bad.is_none();
        hf_rows.push(format!(
            "{coat}: {px} px, first colour it cannot produce {:?}",
            bad.map(|c| format!("#{c:06x}"))
        ));
        hf_frames.push(frame);
    }
    hfst.destroy(gpu);
    let hf_diff = diff_bytes(&hf_frames[0], &hf_frames[1]);
    hf_ok &= hf_diff > 0;
    w.push(wit(
        "m64.the_horse_foal_wears_its_coat_s_baby_sheet",
        hf_ok,
        format!(
            "black variant id {black_id}; {}; {hf_diff} bytes differ between the coats \
             (want >= 200 px each, none foreign, the coats different)",
            hf_rows.join("; ")
        ),
    ));

    // m65 — M340: the kitten draws its tabby base, and on the black coat's
    // variant id (the id `vanilla_variant` gives a kitten whose registry
    // entry's `baby_asset_id` is `cat_black_baby`) the black kitten sheet:
    // each frame is producible by its own sheet, and the two differ.
    let mut ktst = stage_from(
        gpu,
        baked,
        cw,
        ch,
        Vec3::new(0.8, 0.6, -0.8),
        Vec3::new(0.0, 0.2, 0.0),
    )?;
    ktst.wr.set_entities(&[], ktst.right, ktst.up, 0.0);
    ktst.off
        .render(gpu, Some((&mut ktst.wr, ktst.vp)), &ov, BG)?;
    let kt_empty = ktst.off.read_rgba(gpu)?;
    let kt_black_id =
        rewo_data::mob_variants::variant_id("entity/cat/cat_black_baby.png").unwrap_or(0);
    let kt_base: HashSet<u32> = baked
        .mob_textures
        .iter()
        .find(|t| t.key == "cat_baby")
        .map(|t| producible(&t.rgba))
        .unwrap_or_default();
    let kt_black: HashSet<u32> = baked
        .mob_variant_textures
        .iter()
        .find(|t| t.key == "cat_baby" && t.index == kt_black_id)
        .map(|t| producible(&t.rgba))
        .unwrap_or_default();
    let mut kt_rows = Vec::new();
    let mut kt_frames = Vec::new();
    let mut kt_ok = kt_black_id != 0;
    for (variant, coat, sheet) in [(0u16, "tabby", kt_base), (kt_black_id, "black", kt_black)] {
        let d = EntityDraw {
            variant,
            ..neutral_draw(EntityModelKind::CatBaby)
        };
        ktst.wr.set_entities(&[d], ktst.right, ktst.up, 0.0);
        ktst.off
            .render(gpu, Some((&mut ktst.wr, ktst.vp)), &ov, BG)?;
        let frame = ktst.off.read_rgba(gpu)?;
        let (mut px, mut bad) = (0usize, None);
        for (p, e) in frame
            .as_chunks::<4>()
            .0
            .iter()
            .zip(kt_empty.as_chunks::<4>().0)
        {
            if p[..3] == e[..3] {
                continue;
            }
            px += 1;
            let c = pack(p[0], p[1], p[2]);
            if bad.is_none() && !explains(&sheet, c) {
                bad = Some(c);
            }
        }
        kt_ok &= !sheet.is_empty() && px >= 200 && bad.is_none();
        kt_rows.push(format!(
            "{coat}: {px} px, first colour it cannot produce {:?}",
            bad.map(|c| format!("#{c:06x}"))
        ));
        kt_frames.push(frame);
    }
    ktst.destroy(gpu);
    let kt_diff = diff_bytes(&kt_frames[0], &kt_frames[1]);
    kt_ok &= kt_diff > 0;
    w.push(wit(
        "m65.the_kitten_wears_its_coat_s_baby_sheet",
        kt_ok,
        format!(
            "black variant id {kt_black_id}; {}; {kt_diff} bytes differ between the coats \
             (want >= 200 px each, none foreign, the coats different)",
            kt_rows.join("; ")
        ),
    ));

    // m66 — M341: the puppy draws its pale base, and on the ashen coat's
    // variant id (`wolf_ashen_baby`, what a wild ashen puppy's registry
    // entry names) the ashen puppy sheet: each frame is producible by its
    // own sheet, and the two differ.
    let mut pust = stage_from(
        gpu,
        baked,
        cw,
        ch,
        Vec3::new(0.8, 0.6, -0.8),
        Vec3::new(0.0, 0.2, 0.0),
    )?;
    pust.wr.set_entities(&[], pust.right, pust.up, 0.0);
    pust.off
        .render(gpu, Some((&mut pust.wr, pust.vp)), &ov, BG)?;
    let pu_empty = pust.off.read_rgba(gpu)?;
    let pu_ashen_id =
        rewo_data::mob_variants::variant_id("entity/wolf/wolf_ashen_baby.png").unwrap_or(0);
    let pu_base: HashSet<u32> = baked
        .mob_textures
        .iter()
        .find(|t| t.key == "wolf_baby")
        .map(|t| producible(&t.rgba))
        .unwrap_or_default();
    let pu_ashen: HashSet<u32> = baked
        .mob_variant_textures
        .iter()
        .find(|t| t.key == "wolf_baby" && t.index == pu_ashen_id)
        .map(|t| producible(&t.rgba))
        .unwrap_or_default();
    let mut pu_rows = Vec::new();
    let mut pu_frames = Vec::new();
    let mut pu_ok = pu_ashen_id != 0;
    for (variant, coat, sheet) in [(0u16, "pale", pu_base), (pu_ashen_id, "ashen", pu_ashen)] {
        let d = EntityDraw {
            variant,
            ..neutral_draw(EntityModelKind::WolfBaby)
        };
        pust.wr.set_entities(&[d], pust.right, pust.up, 0.0);
        pust.off
            .render(gpu, Some((&mut pust.wr, pust.vp)), &ov, BG)?;
        let frame = pust.off.read_rgba(gpu)?;
        let (mut px, mut bad) = (0usize, None);
        for (p, e) in frame
            .as_chunks::<4>()
            .0
            .iter()
            .zip(pu_empty.as_chunks::<4>().0)
        {
            if p[..3] == e[..3] {
                continue;
            }
            px += 1;
            let c = pack(p[0], p[1], p[2]);
            if bad.is_none() && !explains(&sheet, c) {
                bad = Some(c);
            }
        }
        pu_ok &= !sheet.is_empty() && px >= 200 && bad.is_none();
        pu_rows.push(format!(
            "{coat}: {px} px, first colour it cannot produce {:?}",
            bad.map(|c| format!("#{c:06x}"))
        ));
        pu_frames.push(frame);
    }
    pust.destroy(gpu);
    let pu_diff = diff_bytes(&pu_frames[0], &pu_frames[1]);
    pu_ok &= pu_diff > 0;
    w.push(wit(
        "m66.the_puppy_wears_its_coat_s_baby_sheet",
        pu_ok,
        format!(
            "ashen variant id {pu_ashen_id}; {}; {pu_diff} bytes differ between the coats \
             (want >= 200 px each, none foreign, the coats different)",
            pu_rows.join("; ")
        ),
    ));

    // m67 — M342: a tame wolf and a tame puppy wear a blue collar: against
    // the same mob with no collar, at least 30 pixels change, and most of
    // them are blue-dominant (blue's diffuse colour #3c44aa); with no collar
    // nothing changes from the collarless frame (the gate).
    let mut clst = stage_from(
        gpu,
        baked,
        cw,
        ch,
        Vec3::new(0.9, 0.9, -0.9),
        Vec3::new(0.0, 0.35, 0.0),
    )?;
    let mut cl_rows = Vec::new();
    let mut cl_ok = true;
    for kind in [EntityModelKind::Wolf, EntityModelKind::WolfBaby] {
        let shoot = |st: &mut Stage, collar: Option<u8>| -> Result<Vec<u8>, String> {
            let d = EntityDraw {
                collar,
                ..neutral_draw(kind)
            };
            st.wr.set_entities(&[d], st.right, st.up, 0.0);
            st.off.render(gpu, Some((&mut st.wr, st.vp)), &ov, BG)?;
            st.off.read_rgba(gpu)
        };
        let bare = shoot(&mut clst, None)?;
        let blue = shoot(&mut clst, Some(11))?;
        // The gate: with no collar, every pixel the mob adds over the empty
        // stage is its coat sheet's (a collar drawn anyway adds the collar
        // sheet's greys).
        let coat_key = if kind == EntityModelKind::Wolf {
            "wolf"
        } else {
            "wolf_baby"
        };
        let coat: HashSet<u32> = baked
            .mob_textures
            .iter()
            .find(|t| t.key == coat_key)
            .map(|t| producible(&t.rgba))
            .unwrap_or_default();
        clst.wr.set_entities(&[], clst.right, clst.up, 0.0);
        clst.off
            .render(gpu, Some((&mut clst.wr, clst.vp)), &ov, BG)?;
        let empty = clst.off.read_rgba(gpu)?;
        let bare_foreign = bare
            .as_chunks::<4>()
            .0
            .iter()
            .zip(empty.as_chunks::<4>().0)
            .filter(|(p, e)| p[..3] != e[..3] && !explains(&coat, pack(p[0], p[1], p[2])))
            .count();
        let (mut px, mut bluish) = (0usize, 0usize);
        for (p, e) in blue.as_chunks::<4>().0.iter().zip(bare.as_chunks::<4>().0) {
            if p[..3] == e[..3] {
                continue;
            }
            px += 1;
            bluish += usize::from(p[2] > p[0] && p[2] > p[1]);
        }
        cl_ok &= !coat.is_empty() && px >= 30 && bluish * 10 >= px * 8 && bare_foreign == 0;
        cl_rows.push(format!(
            "{}: {px} px change, {bluish} blue-dominant, {bare_foreign} px of the \
             collarless one foreign to its coat",
            kind.name()
        ));
    }
    clst.destroy(gpu);
    w.push(wit(
        "m67.a_tame_wolf_and_puppy_wear_a_dyed_collar",
        cl_ok,
        format!(
            "{} (want >= 30, >= 80% blue-dominant, and 0 foreign without a collar)",
            cl_rows.join("; ")
        ),
    ));

    // m68 — M343: a tame cat and a tame kitten wear a blue collar: against
    // the same mob with no collar, at least 30 pixels change, and most of
    // them are blue-dominant (blue's diffuse colour #3c44aa); with no collar
    // nothing changes from the collarless frame (the gate).
    let mut ccst = stage_from(
        gpu,
        baked,
        cw,
        ch,
        Vec3::new(0.9, 0.9, -0.9),
        Vec3::new(0.0, 0.35, 0.0),
    )?;
    let mut cc_rows = Vec::new();
    let mut cc_ok = true;
    for kind in [EntityModelKind::Cat, EntityModelKind::CatBaby] {
        let shoot = |st: &mut Stage, collar: Option<u8>| -> Result<Vec<u8>, String> {
            let d = EntityDraw {
                collar,
                ..neutral_draw(kind)
            };
            st.wr.set_entities(&[d], st.right, st.up, 0.0);
            st.off.render(gpu, Some((&mut st.wr, st.vp)), &ov, BG)?;
            st.off.read_rgba(gpu)
        };
        let bare = shoot(&mut ccst, None)?;
        let blue = shoot(&mut ccst, Some(11))?;
        // The gate: with no collar, every pixel the mob adds over the empty
        // stage is its coat sheet's (a collar drawn anyway adds the collar
        // sheet's greys).
        let coat_key = if kind == EntityModelKind::Cat {
            "cat"
        } else {
            "cat_baby"
        };
        let coat: HashSet<u32> = baked
            .mob_textures
            .iter()
            .find(|t| t.key == coat_key)
            .map(|t| producible(&t.rgba))
            .unwrap_or_default();
        ccst.wr.set_entities(&[], ccst.right, ccst.up, 0.0);
        ccst.off
            .render(gpu, Some((&mut ccst.wr, ccst.vp)), &ov, BG)?;
        let empty = ccst.off.read_rgba(gpu)?;
        let bare_foreign = bare
            .as_chunks::<4>()
            .0
            .iter()
            .zip(empty.as_chunks::<4>().0)
            .filter(|(p, e)| p[..3] != e[..3] && !explains(&coat, pack(p[0], p[1], p[2])))
            .count();
        let (mut px, mut bluish) = (0usize, 0usize);
        for (p, e) in blue.as_chunks::<4>().0.iter().zip(bare.as_chunks::<4>().0) {
            if p[..3] == e[..3] {
                continue;
            }
            px += 1;
            bluish += usize::from(p[2] > p[0] && p[2] > p[1]);
        }
        cc_ok &= !coat.is_empty() && px >= 30 && bluish * 10 >= px * 8 && bare_foreign == 0;
        cc_rows.push(format!(
            "{}: {px} px change, {bluish} blue-dominant, {bare_foreign} px of the \
             collarless one foreign to its coat",
            kind.name()
        ));
    }
    ccst.destroy(gpu);
    w.push(wit(
        "m68.a_tame_cat_and_kitten_wear_a_dyed_collar",
        cc_ok,
        format!(
            "{} (want >= 30, >= 80% blue-dominant, and 0 foreign without a collar)",
            cc_rows.join("; ")
        ),
    ));

    // m69 — M344: the parrot draws its red-blue base, and on the blue
    // colour's variant id (`parrot_texture(1)`) the blue sheet: each frame
    // is producible by its own sheet, and the two differ.
    let mut prst = stage_from(
        gpu,
        baked,
        cw,
        ch,
        Vec3::new(0.8, 0.8, -0.8),
        Vec3::new(0.0, 0.35, 0.0),
    )?;
    prst.wr.set_entities(&[], prst.right, prst.up, 0.0);
    prst.off
        .render(gpu, Some((&mut prst.wr, prst.vp)), &ov, BG)?;
    let pr_empty = prst.off.read_rgba(gpu)?;
    let pr_blue_id =
        rewo_data::mob_variants::variant_id(rewo_data::mob_variants::parrot_texture(1))
            .unwrap_or(0);
    let pr_base: HashSet<u32> = baked
        .mob_textures
        .iter()
        .find(|t| t.key == "parrot")
        .map(|t| producible(&t.rgba))
        .unwrap_or_default();
    let pr_blue: HashSet<u32> = baked
        .mob_variant_textures
        .iter()
        .find(|t| t.key == "parrot" && t.index == pr_blue_id)
        .map(|t| producible(&t.rgba))
        .unwrap_or_default();
    let mut pr_rows = Vec::new();
    let mut pr_frames = Vec::new();
    let mut pr_ok = pr_blue_id != 0;
    for (variant, colour, sheet) in [(0u16, "red-blue", pr_base), (pr_blue_id, "blue", pr_blue)] {
        let d = EntityDraw {
            variant,
            ..neutral_draw(EntityModelKind::Parrot)
        };
        prst.wr.set_entities(&[d], prst.right, prst.up, 0.0);
        prst.off
            .render(gpu, Some((&mut prst.wr, prst.vp)), &ov, BG)?;
        let frame = prst.off.read_rgba(gpu)?;
        let (mut px, mut bad) = (0usize, None);
        for (p, e) in frame
            .as_chunks::<4>()
            .0
            .iter()
            .zip(pr_empty.as_chunks::<4>().0)
        {
            if p[..3] == e[..3] {
                continue;
            }
            px += 1;
            let c = pack(p[0], p[1], p[2]);
            if bad.is_none() && !explains(&sheet, c) {
                bad = Some(c);
            }
        }
        pr_ok &= !sheet.is_empty() && px >= 200 && bad.is_none();
        pr_rows.push(format!(
            "{colour}: {px} px, first colour it cannot produce {:?}",
            bad.map(|c| format!("#{c:06x}"))
        ));
        pr_frames.push(frame);
    }
    prst.destroy(gpu);
    let pr_diff = diff_bytes(&pr_frames[0], &pr_frames[1]);
    pr_ok &= pr_diff > 0;
    w.push(wit(
        "m69.the_parrot_wears_its_colour",
        pr_ok,
        format!(
            "blue variant id {pr_blue_id}; {}; {pr_diff} bytes differ (want >= 200 px \
             each, none foreign, the colours different)",
            pr_rows.join("; ")
        ),
    ));

    // m70 — M345: the fox and the kit draw their red base, and on the snow
    // coat's variant id their snow sheet: each frame is producible by its
    // own sheet, and red and snow differ.
    let mut fxst = stage_from(
        gpu,
        baked,
        cw,
        ch,
        Vec3::new(1.0, 0.8, -1.0),
        Vec3::new(0.0, 0.3, 0.0),
    )?;
    fxst.wr.set_entities(&[], fxst.right, fxst.up, 0.0);
    fxst.off
        .render(gpu, Some((&mut fxst.wr, fxst.vp)), &ov, BG)?;
    let fx_empty = fxst.off.read_rgba(gpu)?;
    let mut fx_rows = Vec::new();
    let mut fx_ok = true;
    for (kind, key, kit) in [
        (EntityModelKind::Fox, "fox", false),
        (EntityModelKind::FoxBaby, "fox_baby", true),
    ] {
        let snow_id =
            rewo_data::mob_variants::variant_id(rewo_data::mob_variants::fox_texture(1, kit))
                .unwrap_or(0);
        let red: HashSet<u32> = baked
            .mob_textures
            .iter()
            .find(|t| t.key == key)
            .map(|t| producible(&t.rgba))
            .unwrap_or_default();
        let snow: HashSet<u32> = baked
            .mob_variant_textures
            .iter()
            .find(|t| t.key == key && t.index == snow_id)
            .map(|t| producible(&t.rgba))
            .unwrap_or_default();
        let mut frames = Vec::new();
        fx_ok &= snow_id != 0;
        for (variant, sheet) in [(0u16, red), (snow_id, snow)] {
            let d = EntityDraw {
                variant,
                ..neutral_draw(kind)
            };
            fxst.wr.set_entities(&[d], fxst.right, fxst.up, 0.0);
            fxst.off
                .render(gpu, Some((&mut fxst.wr, fxst.vp)), &ov, BG)?;
            let frame = fxst.off.read_rgba(gpu)?;
            let (mut px, mut bad) = (0usize, None);
            for (p, e) in frame
                .as_chunks::<4>()
                .0
                .iter()
                .zip(fx_empty.as_chunks::<4>().0)
            {
                if p[..3] == e[..3] {
                    continue;
                }
                px += 1;
                let c = pack(p[0], p[1], p[2]);
                if bad.is_none() && !explains(&sheet, c) {
                    bad = Some(c);
                }
            }
            fx_ok &= !sheet.is_empty() && px >= 200 && bad.is_none();
            fx_rows.push(format!(
                "{key} variant {variant}: {px} px, first colour it cannot produce {:?}",
                bad.map(|c| format!("#{c:06x}"))
            ));
            frames.push(frame);
        }
        fx_ok &= diff_bytes(&frames[0], &frames[1]) > 0;
    }
    fxst.destroy(gpu);
    w.push(wit(
        "m70.the_fox_and_kit_wear_their_coat",
        fx_ok,
        format!(
            "{} (want >= 200 px each, none foreign, the coats different)",
            fx_rows.join("; ")
        ),
    ));

    // m71 — M346: the rabbit and the kit draw their brown base, the white
    // coat and the Toast sheet on their variant ids, each frame producible
    // by its own sheet and every pair of them different.
    let mut rbst = stage_from(
        gpu,
        baked,
        cw,
        ch,
        Vec3::new(0.8, 0.7, -0.8),
        Vec3::new(0.0, 0.25, 0.0),
    )?;
    rbst.wr.set_entities(&[], rbst.right, rbst.up, 0.0);
    rbst.off
        .render(gpu, Some((&mut rbst.wr, rbst.vp)), &ov, BG)?;
    let rb_empty = rbst.off.read_rgba(gpu)?;
    let mut rb_rows = Vec::new();
    let mut rb_ok = true;
    for (kind, key, kit) in [
        (EntityModelKind::Rabbit, "rabbit", false),
        (EntityModelKind::RabbitBaby, "rabbit_baby", true),
    ] {
        let mut frames = Vec::new();
        for (name, id, toast) in [("brown", 0, false), ("white", 1, false), ("toast", 0, true)] {
            let variant = rewo_data::mob_variants::variant_id(
                &rewo_data::mob_variants::rabbit_texture(id, kit, toast),
            )
            .unwrap_or(0);
            let sheet: HashSet<u32> = if variant == 0 {
                baked
                    .mob_textures
                    .iter()
                    .find(|t| t.key == key)
                    .map(|t| producible(&t.rgba))
            } else {
                baked
                    .mob_variant_textures
                    .iter()
                    .find(|t| t.key == key && t.index == variant)
                    .map(|t| producible(&t.rgba))
            }
            .unwrap_or_default();
            rb_ok &= (variant == 0) == (name == "brown");
            let d = EntityDraw {
                variant,
                ..neutral_draw(kind)
            };
            rbst.wr.set_entities(&[d], rbst.right, rbst.up, 0.0);
            rbst.off
                .render(gpu, Some((&mut rbst.wr, rbst.vp)), &ov, BG)?;
            let frame = rbst.off.read_rgba(gpu)?;
            let (mut px, mut bad) = (0usize, None);
            for (p, e) in frame
                .as_chunks::<4>()
                .0
                .iter()
                .zip(rb_empty.as_chunks::<4>().0)
            {
                if p[..3] == e[..3] {
                    continue;
                }
                px += 1;
                let c = pack(p[0], p[1], p[2]);
                if bad.is_none() && !explains(&sheet, c) {
                    bad = Some(c);
                }
            }
            rb_ok &= !sheet.is_empty() && px >= 200 && bad.is_none();
            rb_rows.push(format!(
                "{key} {name}: {px} px, first colour it cannot produce {:?}",
                bad.map(|c| format!("#{c:06x}"))
            ));
            frames.push(frame);
        }
        rb_ok &= diff_bytes(&frames[0], &frames[1]) > 0
            && diff_bytes(&frames[0], &frames[2]) > 0
            && diff_bytes(&frames[1], &frames[2]) > 0;
    }
    rbst.destroy(gpu);
    w.push(wit(
        "m71.the_rabbit_and_kit_wear_their_coat_or_are_toast",
        rb_ok,
        format!(
            "{} (want >= 200 px each, none foreign, all different)",
            rb_rows.join("; ")
        ),
    ));

    // m72 — M347: the mooshroom and the calf draw their red base, and on the
    // brown colour's variant id their brown sheet: each frame is producible
    // by its own sheet, and red and brown differ.
    let mut msst = stage_from(
        gpu,
        baked,
        cw,
        ch,
        Vec3::new(1.6, 1.3, -1.6),
        Vec3::new(0.0, 0.7, 0.0),
    )?;
    msst.wr.set_entities(&[], msst.right, msst.up, 0.0);
    msst.off
        .render(gpu, Some((&mut msst.wr, msst.vp)), &ov, BG)?;
    let ms_empty = msst.off.read_rgba(gpu)?;
    let mut ms_rows = Vec::new();
    let mut ms_ok = true;
    for (kind, key, calf) in [
        (EntityModelKind::Mooshroom, "mooshroom", false),
        (EntityModelKind::MooshroomBaby, "mooshroom_baby", true),
    ] {
        let brown_id = rewo_data::mob_variants::variant_id(
            rewo_data::mob_variants::mooshroom_texture(1, calf),
        )
        .unwrap_or(0);
        let red: HashSet<u32> = baked
            .mob_textures
            .iter()
            .find(|t| t.key == key)
            .map(|t| producible(&t.rgba))
            .unwrap_or_default();
        let brown: HashSet<u32> = baked
            .mob_variant_textures
            .iter()
            .find(|t| t.key == key && t.index == brown_id)
            .map(|t| producible(&t.rgba))
            .unwrap_or_default();
        let mut frames = Vec::new();
        ms_ok &= brown_id != 0;
        for (variant, sheet) in [(0u16, red), (brown_id, brown)] {
            let d = EntityDraw {
                variant,
                ..neutral_draw(kind)
            };
            msst.wr.set_entities(&[d], msst.right, msst.up, 0.0);
            msst.off
                .render(gpu, Some((&mut msst.wr, msst.vp)), &ov, BG)?;
            let frame = msst.off.read_rgba(gpu)?;
            let (mut px, mut bad) = (0usize, None);
            for (p, e) in frame
                .as_chunks::<4>()
                .0
                .iter()
                .zip(ms_empty.as_chunks::<4>().0)
            {
                if p[..3] == e[..3] {
                    continue;
                }
                px += 1;
                let c = pack(p[0], p[1], p[2]);
                if bad.is_none() && !explains(&sheet, c) {
                    bad = Some(c);
                }
            }
            ms_ok &= !sheet.is_empty() && px >= 200 && bad.is_none();
            ms_rows.push(format!(
                "{key} variant {variant}: {px} px, first colour it cannot produce {:?}",
                bad.map(|c| format!("#{c:06x}"))
            ));
            frames.push(frame);
        }
        ms_ok &= diff_bytes(&frames[0], &frames[1]) > 0;
    }
    msst.destroy(gpu);
    w.push(wit(
        "m72.the_mooshroom_and_calf_are_red_or_brown",
        ms_ok,
        format!(
            "{} (want >= 200 px each, none foreign, the colours different)",
            ms_rows.join("; ")
        ),
    ));

    // m73 — M348: the temperate, cold and warm cows each draw their own
    // mesh's sheet (each frame producible by it, the three different), and
    // the calf draws the cold and warm calf sheets on their variant ids.
    let mut cvst = stage_from(
        gpu,
        baked,
        cw,
        ch,
        Vec3::new(1.6, 1.3, -1.6),
        Vec3::new(0.0, 0.7, 0.0),
    )?;
    cvst.wr.set_entities(&[], cvst.right, cvst.up, 0.0);
    cvst.off
        .render(gpu, Some((&mut cvst.wr, cvst.vp)), &ov, BG)?;
    let cv_empty = cvst.off.read_rgba(gpu)?;
    let calf = |p: &str| rewo_data::mob_variants::variant_id(p).unwrap_or(0);
    let (cold_calf, warm_calf) = (
        calf("entity/cow/cow_cold_baby.png"),
        calf("entity/cow/cow_warm_baby.png"),
    );
    let mut cv_ok = cold_calf != 0 && warm_calf != 0;
    let mut cv_rows = Vec::new();
    let mut cv_frames = Vec::new();
    for (kind, key, variant, name) in [
        (EntityModelKind::Cow, "cow", 0u16, "cow"),
        (EntityModelKind::ColdCow, "cold_cow", 0, "cold cow"),
        (EntityModelKind::WarmCow, "warm_cow", 0, "warm cow"),
        (EntityModelKind::CowBaby, "cow_baby", 0, "calf"),
        (EntityModelKind::CowBaby, "cow_baby", cold_calf, "cold calf"),
        (EntityModelKind::CowBaby, "cow_baby", warm_calf, "warm calf"),
    ] {
        let sheet: HashSet<u32> = (if variant == 0 {
            baked
                .mob_textures
                .iter()
                .find(|t| t.key == key)
                .map(|t| producible(&t.rgba))
        } else {
            baked
                .mob_variant_textures
                .iter()
                .find(|t| t.key == key && t.index == variant)
                .map(|t| producible(&t.rgba))
        })
        .unwrap_or_default();
        let d = EntityDraw {
            variant,
            ..neutral_draw(kind)
        };
        cvst.wr.set_entities(&[d], cvst.right, cvst.up, 0.0);
        cvst.off
            .render(gpu, Some((&mut cvst.wr, cvst.vp)), &ov, BG)?;
        let frame = cvst.off.read_rgba(gpu)?;
        let (mut px, mut bad) = (0usize, None);
        for (p, e) in frame
            .as_chunks::<4>()
            .0
            .iter()
            .zip(cv_empty.as_chunks::<4>().0)
        {
            if p[..3] == e[..3] {
                continue;
            }
            px += 1;
            let c = pack(p[0], p[1], p[2]);
            if bad.is_none() && !explains(&sheet, c) {
                bad = Some(c);
            }
        }
        cv_ok &= !sheet.is_empty() && px >= 200 && bad.is_none();
        cv_rows.push(format!(
            "{name}: {px} px, first colour it cannot produce {:?}",
            bad.map(|c| format!("#{c:06x}"))
        ));
        cv_frames.push(frame);
    }
    for (a, b) in [(0, 1), (0, 2), (1, 2), (3, 4), (3, 5), (4, 5)] {
        cv_ok &= diff_bytes(&cv_frames[a], &cv_frames[b]) > 0;
    }
    cvst.destroy(gpu);
    w.push(wit(
        "m73.the_cold_and_warm_cows_and_calves_wear_their_own",
        cv_ok,
        format!(
            "{} (want >= 200 px each, none foreign, adults and calves each all different)",
            cv_rows.join("; ")
        ),
    ));

    // m74 — M349: the pig and the chicken each draw temperate, cold (its
    // own mesh's sheet) and warm (the base mesh's alternate), and their young
    // the three young sheets: every frame producible by its own sheet, and
    // each group of three all different.
    let vid = |p: &str| rewo_data::mob_variants::variant_id(p).unwrap_or(0);
    let mut fa_ok = true;
    let mut fa_rows = Vec::new();
    for (eye, look, groups) in [
        (
            Vec3::new(1.2, 1.0, -1.2),
            Vec3::new(0.0, 0.45, 0.0),
            [
                [
                    (EntityModelKind::Pig, "pig", 0u16, "pig"),
                    (EntityModelKind::ColdPig, "cold_pig", 0, "cold pig"),
                    (
                        EntityModelKind::Pig,
                        "pig",
                        vid("entity/pig/pig_warm.png"),
                        "warm pig",
                    ),
                ],
                [
                    (EntityModelKind::PigBaby, "pig_baby", 0, "piglet"),
                    (
                        EntityModelKind::PigBaby,
                        "pig_baby",
                        vid("entity/pig/pig_cold_baby.png"),
                        "cold piglet",
                    ),
                    (
                        EntityModelKind::PigBaby,
                        "pig_baby",
                        vid("entity/pig/pig_warm_baby.png"),
                        "warm piglet",
                    ),
                ],
            ],
        ),
        (
            Vec3::new(0.8, 0.7, -0.8),
            Vec3::new(0.0, 0.35, 0.0),
            [
                [
                    (EntityModelKind::Chicken, "chicken", 0u16, "chicken"),
                    (
                        EntityModelKind::ColdChicken,
                        "cold_chicken",
                        0,
                        "cold chicken",
                    ),
                    (
                        EntityModelKind::Chicken,
                        "chicken",
                        vid("entity/chicken/chicken_warm.png"),
                        "warm chicken",
                    ),
                ],
                [
                    (EntityModelKind::ChickenBaby, "chicken_baby", 0, "chick"),
                    (
                        EntityModelKind::ChickenBaby,
                        "chicken_baby",
                        vid("entity/chicken/chicken_cold_baby.png"),
                        "cold chick",
                    ),
                    (
                        EntityModelKind::ChickenBaby,
                        "chicken_baby",
                        vid("entity/chicken/chicken_warm_baby.png"),
                        "warm chick",
                    ),
                ],
            ],
        ),
    ] {
        let mut st = stage_from(gpu, baked, cw, ch, eye, look)?;
        st.wr.set_entities(&[], st.right, st.up, 0.0);
        st.off.render(gpu, Some((&mut st.wr, st.vp)), &ov, BG)?;
        let empty = st.off.read_rgba(gpu)?;
        for group in groups {
            let mut frames = Vec::new();
            for (kind, key, variant, name) in group {
                let sheet: HashSet<u32> = (if variant == 0 {
                    baked
                        .mob_textures
                        .iter()
                        .find(|t| t.key == key)
                        .map(|t| producible(&t.rgba))
                } else {
                    baked
                        .mob_variant_textures
                        .iter()
                        .find(|t| t.key == key && t.index == variant)
                        .map(|t| producible(&t.rgba))
                })
                .unwrap_or_default();
                // An alternate (every warm sheet, and the cold young's) must
                // have resolved to a variant id; a mesh's base must not.
                let alternate = name.starts_with("warm")
                    || (name.starts_with("cold") && key.ends_with("_baby"));
                fa_ok &= (variant != 0) == alternate;
                let d = EntityDraw {
                    variant,
                    ..neutral_draw(kind)
                };
                st.wr.set_entities(&[d], st.right, st.up, 0.0);
                st.off.render(gpu, Some((&mut st.wr, st.vp)), &ov, BG)?;
                let frame = st.off.read_rgba(gpu)?;
                let (mut px, mut bad) = (0usize, None);
                for (p, e) in frame
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .zip(empty.as_chunks::<4>().0)
                {
                    if p[..3] == e[..3] {
                        continue;
                    }
                    px += 1;
                    let c = pack(p[0], p[1], p[2]);
                    if bad.is_none() && !explains(&sheet, c) {
                        bad = Some(c);
                    }
                }
                fa_ok &= !sheet.is_empty() && px >= 150 && bad.is_none();
                fa_rows.push(format!(
                    "{name}: {px} px, first colour it cannot produce {:?}",
                    bad.map(|c| format!("#{c:06x}"))
                ));
                frames.push(frame);
            }
            fa_ok &= diff_bytes(&frames[0], &frames[1]) > 0
                && diff_bytes(&frames[0], &frames[2]) > 0
                && diff_bytes(&frames[1], &frames[2]) > 0;
        }
        st.destroy(gpu);
    }
    w.push(wit(
        "m74.the_pigs_and_chickens_wear_their_variants",
        fa_ok,
        format!(
            "{} (want >= 150 px each, none foreign, each group of three all different)",
            fa_rows.join("; ")
        ),
    ));

    // m75 — M350: the coral zombie nautilus draws from its own sheet, its
    // corals reach past the plain zombie nautilus's silhouette, and body
    // armour hides them (`corals.visible = bodyArmorItem.isEmpty()`): the
    // pixels only the corals cover come back as background once armoured
    // (the armour is the shell at 0.01, inside the plain silhouette). Most of
    // each plane sits inside the shell, so four views are summed.
    let mut layers = [None; rewo_gpu::entities::MAX_ARMOR_SUBLAYERS];
    layers[0] = Some(("nautilus_body/iron", [1.0, 1.0, 1.0]));
    let iron = Some(rewo_gpu::entities::ArmorPiece {
        layers,
        ..Default::default()
    });
    let coral_sheet: HashSet<u32> = baked
        .mob_textures
        .iter()
        .find(|t| t.key == "zombie_nautilus_coral")
        .map(|t| producible(&t.rgba))
        .unwrap_or_default();
    let (mut drawn, mut foreign, mut coral_only, mut kept) = (0usize, None, 0usize, 0usize);
    for eye in [
        Vec3::new(2.4, 2.6, 2.2),
        Vec3::new(-2.4, 2.0, 2.4),
        Vec3::new(2.4, 1.2, -2.4),
        Vec3::new(-2.4, 0.6, -2.4),
    ] {
        let mut znst = stage_from(gpu, baked, cw, ch, eye, Vec3::new(0.0, 1.0, 0.0))?;
        znst.wr.set_entities(&[], znst.right, znst.up, 0.0);
        znst.off
            .render(gpu, Some((&mut znst.wr, znst.vp)), &ov, BG)?;
        let empty = znst.off.read_rgba(gpu)?;
        let mut zshot = |d: EntityDraw<'_>| -> Result<Vec<u8>, String> {
            znst.wr.set_entities(&[d], znst.right, znst.up, 0.0);
            znst.off
                .render(gpu, Some((&mut znst.wr, znst.vp)), &ov, BG)?;
            znst.off.read_rgba(gpu)
        };
        let plain = zshot(neutral_draw(EntityModelKind::ZombieNautilus))?;
        let coral = zshot(neutral_draw(EntityModelKind::ZombieNautilusCoral))?;
        let armoured = zshot(EntityDraw {
            body_armor: iron,
            ..neutral_draw(EntityModelKind::ZombieNautilusCoral)
        })?;
        znst.destroy(gpu);
        let at = |f: &[u8], i: usize| f[i * 4..i * 4 + 3] != empty[i * 4..i * 4 + 3];
        for i in 0..empty.len() / 4 {
            if !at(&coral, i) {
                continue;
            }
            drawn += 1;
            let c = pack(coral[i * 4], coral[i * 4 + 1], coral[i * 4 + 2]);
            if foreign.is_none() && !explains(&coral_sheet, c) {
                foreign = Some(c);
            }
            if !at(&plain, i) {
                coral_only += 1;
                kept += usize::from(at(&armoured, i));
            }
        }
    }
    w.push(wit(
        "m75.the_coral_nautilus_wears_its_corals_and_hides_them_under_armour",
        !coral_sheet.is_empty()
            && drawn >= 4000
            && foreign.is_none()
            && coral_only >= 100
            && kept * 20 <= coral_only,
        format!(
            "{drawn} px drawn over four views (want >= 4000), first colour its sheet cannot \
             produce {:?}; {coral_only} px only the corals cover (want >= 100), {kept} of \
             them still covered in iron armour (want at most a twentieth)",
            foreign.map(|c| format!("#{c:06x}"))
        ),
    ));

    // m76 — M354: the bee and the baby each draw their four sheets (plain,
    // angry, nectar, angry nectar) from their own sheets, all four
    // different, and a bee that has stung loses its stinger (the frames
    // differ, by a stinger's worth of pixels at most).
    let mut bst = stage_from(
        gpu,
        baked,
        cw,
        ch,
        Vec3::new(0.7, 0.6, -0.9),
        Vec3::new(0.0, 0.3, 0.0),
    )?;
    bst.wr.set_entities(&[], bst.right, bst.up, 0.0);
    bst.off.render(gpu, Some((&mut bst.wr, bst.vp)), &ov, BG)?;
    let b_empty = bst.off.read_rgba(gpu)?;
    let mut b_ok = true;
    let mut b_rows = Vec::new();
    let mut stinger = 0usize;
    for (kind, key, baby) in [
        (EntityModelKind::Bee, "bee", false),
        (EntityModelKind::BeeBaby, "bee_baby", true),
    ] {
        let mut frames = Vec::new();
        for (angry, nectar) in [(false, false), (true, false), (false, true), (true, true)] {
            let variant = rewo_data::mob_variants::variant_id(
                rewo_data::mob_variants::bee_texture(baby, angry, nectar),
            )
            .unwrap_or(0);
            let sheet: HashSet<u32> = (if variant == 0 {
                baked
                    .mob_textures
                    .iter()
                    .find(|t| t.key == key)
                    .map(|t| producible(&t.rgba))
            } else {
                baked
                    .mob_variant_textures
                    .iter()
                    .find(|t| t.key == key && t.index == variant)
                    .map(|t| producible(&t.rgba))
            })
            .unwrap_or_default();
            b_ok &= (variant == 0) == (!angry && !nectar);
            let d = EntityDraw {
                variant,
                ..neutral_draw(kind)
            };
            bst.wr.set_entities(&[d], bst.right, bst.up, 0.0);
            bst.off.render(gpu, Some((&mut bst.wr, bst.vp)), &ov, BG)?;
            let frame = bst.off.read_rgba(gpu)?;
            let (mut px, mut bad) = (0usize, None);
            for (p, e) in frame
                .as_chunks::<4>()
                .0
                .iter()
                .zip(b_empty.as_chunks::<4>().0)
            {
                if p[..3] == e[..3] {
                    continue;
                }
                px += 1;
                let c = pack(p[0], p[1], p[2]);
                if bad.is_none() && !explains(&sheet, c) {
                    bad = Some(c);
                }
            }
            b_ok &= !sheet.is_empty() && px >= 200 && bad.is_none();
            b_rows.push(format!(
                "{key} angry {angry} nectar {nectar}: {px} px, first colour it cannot produce {:?}",
                bad.map(|c| format!("#{c:06x}"))
            ));
            frames.push(frame);
        }
        for (a, b) in [(0, 1), (0, 2), (0, 3), (1, 2), (1, 3), (2, 3)] {
            b_ok &= diff_bytes(&frames[a], &frames[b]) > 0;
        }
        // The stinger: a bee that has stung draws without it.
        let stung = EntityDraw {
            mob: rewo_gpu::mobs::MobCombat {
                bee_stung: true,
                ..Default::default()
            },
            ..neutral_draw(kind)
        };
        bst.wr.set_entities(&[stung], bst.right, bst.up, 0.0);
        bst.off.render(gpu, Some((&mut bst.wr, bst.vp)), &ov, BG)?;
        let without = bst.off.read_rgba(gpu)?;
        let changed = frames[0]
            .as_chunks::<4>()
            .0
            .iter()
            .zip(without.as_chunks::<4>().0)
            .filter(|(a, b)| a[..3] != b[..3])
            .count();
        b_ok &= changed > 0 && changed < 200;
        stinger += changed;
        b_rows.push(format!("{key} without its stinger: {changed} px change"));
    }
    bst.destroy(gpu);
    w.push(wit(
        "m76.the_bee_and_baby_wear_their_mood_and_nectar_and_lose_the_stinger",
        b_ok && stinger > 0,
        format!(
            "{} (want >= 200 px each, none foreign, all four different; the stinger some px)",
            b_rows.join("; ")
        ),
    ));

    // m77 — M355: the wolf and the puppy draw their angry sheets (pale's)
    // from their own, unlike the wild one, and an angry wolf's raised tail
    // moves pixels.
    let mut wst = stage_from(
        gpu,
        baked,
        cw,
        ch,
        Vec3::new(1.2, 0.9, -1.2),
        Vec3::new(0.0, 0.4, 0.0),
    )?;
    wst.wr.set_entities(&[], wst.right, wst.up, 0.0);
    wst.off.render(gpu, Some((&mut wst.wr, wst.vp)), &ov, BG)?;
    let w_empty = wst.off.read_rgba(gpu)?;
    let mut w_ok = true;
    let mut w_rows = Vec::new();
    for (kind, key, path) in [
        (EntityModelKind::Wolf, "wolf", "entity/wolf/wolf_angry.png"),
        (
            EntityModelKind::WolfBaby,
            "wolf_baby",
            "entity/wolf/wolf_angry_baby.png",
        ),
    ] {
        let angry = rewo_data::mob_variants::variant_id(path).unwrap_or(0);
        let sheet: HashSet<u32> = baked
            .mob_variant_textures
            .iter()
            .find(|t| t.key == key && t.index == angry)
            .map(|t| producible(&t.rgba))
            .unwrap_or_default();
        let mut shot = |d: EntityDraw<'_>| -> Result<Vec<u8>, String> {
            wst.wr.set_entities(&[d], wst.right, wst.up, 0.0);
            wst.off.render(gpu, Some((&mut wst.wr, wst.vp)), &ov, BG)?;
            wst.off.read_rgba(gpu)
        };
        let wild = shot(neutral_draw(kind))?;
        let mad = shot(EntityDraw {
            variant: angry,
            ..neutral_draw(kind)
        })?;
        let raised = shot(EntityDraw {
            variant: angry,
            mob: rewo_gpu::mobs::MobCombat {
                wolf_tail: Some(1.539_380_4),
                wolf_angry: true,
                ..Default::default()
            },
            ..neutral_draw(kind)
        })?;
        let (mut px, mut bad) = (0usize, None);
        for (p, e) in mad
            .as_chunks::<4>()
            .0
            .iter()
            .zip(w_empty.as_chunks::<4>().0)
        {
            if p[..3] == e[..3] {
                continue;
            }
            px += 1;
            let c = pack(p[0], p[1], p[2]);
            if bad.is_none() && !explains(&sheet, c) {
                bad = Some(c);
            }
        }
        let tail = diff_bytes(&mad, &raised);
        w_ok &= angry != 0
            && !sheet.is_empty()
            && px >= 200
            && bad.is_none()
            && diff_bytes(&wild, &mad) > 0
            && tail > 0;
        w_rows.push(format!(
            "{key} angry: {px} px, first colour it cannot produce {:?}, the raised tail moves {tail} bytes",
            bad.map(|c| format!("#{c:06x}"))
        ));
    }
    wst.destroy(gpu);
    w.push(wit(
        "m77.the_wolf_and_puppy_wear_their_anger_and_raise_the_tail",
        w_ok,
        format!(
            "{} (want >= 200 px, none foreign, unlike the wild sheet, the tail moving)",
            w_rows.join("; ")
        ),
    ));

    // m78 — M356: a sitting wolf and puppy draw from their own sheets, and
    // their silhouette moves from a standing one's by a body's worth of
    // pixels (the rump drops and the legs fold).
    let mut sst = stage_from(
        gpu,
        baked,
        cw,
        ch,
        Vec3::new(1.4, 0.8, -0.2),
        Vec3::new(0.0, 0.4, 0.0),
    )?;
    sst.wr.set_entities(&[], sst.right, sst.up, 0.0);
    sst.off.render(gpu, Some((&mut sst.wr, sst.vp)), &ov, BG)?;
    let s_empty = sst.off.read_rgba(gpu)?;
    let mut s_ok = true;
    let mut s_rows = Vec::new();
    for (kind, key, scale) in [
        (EntityModelKind::Wolf, "wolf", 1.0f32),
        (EntityModelKind::WolfBaby, "wolf_baby", 0.5),
    ] {
        let sheet: HashSet<u32> = baked
            .mob_textures
            .iter()
            .find(|t| t.key == key)
            .map(|t| producible(&t.rgba))
            .unwrap_or_default();
        let mut shot = |d: EntityDraw<'_>| -> Result<Vec<u8>, String> {
            sst.wr.set_entities(&[d], sst.right, sst.up, 0.0);
            sst.off.render(gpu, Some((&mut sst.wr, sst.vp)), &ov, BG)?;
            sst.off.read_rgba(gpu)
        };
        let standing = shot(neutral_draw(kind))?;
        let sitting = shot(EntityDraw {
            mob: rewo_gpu::mobs::MobCombat {
                sitting: true,
                age_scale: scale,
                ..Default::default()
            },
            ..neutral_draw(kind)
        })?;
        let mask = |f: &[u8]| -> Vec<bool> {
            f.as_chunks::<4>()
                .0
                .iter()
                .zip(s_empty.as_chunks::<4>().0)
                .map(|(p, e)| p[..3] != e[..3])
                .collect()
        };
        let (a, b) = (mask(&standing), mask(&sitting));
        let moved = a.iter().zip(&b).filter(|(x, y)| x != y).count();
        let (mut px, mut bad) = (0usize, None);
        for (p, e) in sitting
            .as_chunks::<4>()
            .0
            .iter()
            .zip(s_empty.as_chunks::<4>().0)
        {
            if p[..3] == e[..3] {
                continue;
            }
            px += 1;
            let c = pack(p[0], p[1], p[2]);
            if bad.is_none() && !explains(&sheet, c) {
                bad = Some(c);
            }
        }
        s_ok &= !sheet.is_empty() && px >= 200 && bad.is_none() && moved >= 100;
        s_rows.push(format!(
            "{key} sitting: {px} px, first colour it cannot produce {:?}, {moved} px of silhouette moved",
            bad.map(|c| format!("#{c:06x}"))
        ));
    }
    sst.destroy(gpu);
    w.push(wit(
        "m78.the_wolf_and_puppy_sit",
        s_ok,
        format!(
            "{} (want >= 200 px, none foreign, >= 100 px of silhouette moved)",
            s_rows.join("; ")
        ),
    ));

    // m79 — M357: a sitting cat and kitten draw from their own sheets, and
    // their silhouette moves from a standing one's by a body's worth of
    // pixels.
    let mut cst = stage_from(
        gpu,
        baked,
        cw,
        ch,
        Vec3::new(1.2, 0.6, -0.2),
        Vec3::new(0.0, 0.3, 0.0),
    )?;
    cst.wr.set_entities(&[], cst.right, cst.up, 0.0);
    cst.off.render(gpu, Some((&mut cst.wr, cst.vp)), &ov, BG)?;
    let c_empty = cst.off.read_rgba(gpu)?;
    let mut c_ok = true;
    let mut c_rows = Vec::new();
    for (kind, key, scale) in [
        (EntityModelKind::Cat, "cat", 1.0f32),
        (EntityModelKind::CatBaby, "cat_baby", 0.5),
    ] {
        let sheet: HashSet<u32> = baked
            .mob_textures
            .iter()
            .find(|t| t.key == key)
            .map(|t| producible(&t.rgba))
            .unwrap_or_default();
        let mut shot = |d: EntityDraw<'_>| -> Result<Vec<u8>, String> {
            cst.wr.set_entities(&[d], cst.right, cst.up, 0.0);
            cst.off.render(gpu, Some((&mut cst.wr, cst.vp)), &ov, BG)?;
            cst.off.read_rgba(gpu)
        };
        let standing = shot(neutral_draw(kind))?;
        let sitting = shot(EntityDraw {
            mob: rewo_gpu::mobs::MobCombat {
                sitting: true,
                age_scale: scale,
                ..Default::default()
            },
            ..neutral_draw(kind)
        })?;
        let mask = |f: &[u8]| -> Vec<bool> {
            f.as_chunks::<4>()
                .0
                .iter()
                .zip(c_empty.as_chunks::<4>().0)
                .map(|(p, e)| p[..3] != e[..3])
                .collect()
        };
        let moved = mask(&standing)
            .iter()
            .zip(&mask(&sitting))
            .filter(|(x, y)| x != y)
            .count();
        let (mut px, mut bad) = (0usize, None);
        for (p, e) in sitting
            .as_chunks::<4>()
            .0
            .iter()
            .zip(c_empty.as_chunks::<4>().0)
        {
            if p[..3] == e[..3] {
                continue;
            }
            px += 1;
            let c = pack(p[0], p[1], p[2]);
            if bad.is_none() && !explains(&sheet, c) {
                bad = Some(c);
            }
        }
        c_ok &= !sheet.is_empty() && px >= 200 && bad.is_none() && moved >= 100;
        c_rows.push(format!(
            "{key} sitting: {px} px, first colour it cannot produce {:?}, {moved} px of silhouette moved",
            bad.map(|c| format!("#{c:06x}"))
        ));
    }
    cst.destroy(gpu);
    w.push(wit(
        "m79.the_cat_and_kitten_sit",
        c_ok,
        format!(
            "{} (want >= 200 px, none foreign, >= 100 px of silhouette moved)",
            c_rows.join("; ")
        ),
    ));

    // m80 — M358: a grazing sheep and lamb (the eat clock held at 20) draw
    // from their own sheets with the head lowered: the silhouette moves.
    let mut gst = stage_from(
        gpu,
        baked,
        cw,
        ch,
        Vec3::new(1.4, 0.8, -0.2),
        Vec3::new(0.0, 0.5, 0.0),
    )?;
    gst.wr.set_entities(&[], gst.right, gst.up, 0.0);
    gst.off.render(gpu, Some((&mut gst.wr, gst.vp)), &ov, BG)?;
    let g_empty = gst.off.read_rgba(gpu)?;
    let mut g_ok = true;
    let mut g_rows = Vec::new();
    for (kind, scale) in [
        (EntityModelKind::Sheep, 1.0f32),
        (EntityModelKind::SheepBaby, 0.5),
    ] {
        let mut shot = |d: EntityDraw<'_>| -> Result<Vec<u8>, String> {
            gst.wr.set_entities(&[d], gst.right, gst.up, 0.0);
            gst.off.render(gpu, Some((&mut gst.wr, gst.vp)), &ov, BG)?;
            gst.off.read_rgba(gpu)
        };
        let calm = shot(neutral_draw(kind))?;
        let grazing = shot(EntityDraw {
            mob: rewo_gpu::mobs::MobCombat {
                sheep_eat: crate::live_cmd::sheep_eat_pose(20, 0.0),
                age_scale: scale,
                ..Default::default()
            },
            ..neutral_draw(kind)
        })?;
        let mask = |f: &[u8]| -> Vec<bool> {
            f.as_chunks::<4>()
                .0
                .iter()
                .zip(g_empty.as_chunks::<4>().0)
                .map(|(p, e)| p[..3] != e[..3])
                .collect()
        };
        let moved = mask(&calm)
            .iter()
            .zip(&mask(&grazing))
            .filter(|(x, y)| x != y)
            .count();
        g_ok &= moved >= 100;
        g_rows.push(format!("{}: {moved} px of silhouette moved", kind.name()));
    }
    gst.destroy(gpu);
    w.push(wit(
        "m80.a_grazing_sheep_and_lamb_lower_their_heads",
        g_ok,
        format!("{} (want >= 100 each)", g_rows.join("; ")),
    ));

    // m81 — M359: a horse that looks down and to the side moves its head
    // group (silhouette), and a saddled one's bridle goes with it: the
    // saddle sheet's pixels move as far as the head's do.
    let mut hst = stage_from(
        gpu,
        baked,
        cw,
        ch,
        Vec3::new(2.4, 1.8, -1.0),
        Vec3::new(0.0, 1.2, 0.0),
    )?;
    hst.wr.set_entities(&[], hst.right, hst.up, 0.0);
    hst.off.render(gpu, Some((&mut hst.wr, hst.vp)), &ov, BG)?;
    let h_empty = hst.off.read_rgba(gpu)?;
    let mut hshot = |d: EntityDraw<'_>| -> Result<Vec<u8>, String> {
        hst.wr.set_entities(&[d], hst.right, hst.up, 0.0);
        hst.off.render(gpu, Some((&mut hst.wr, hst.vp)), &ov, BG)?;
        hst.off.read_rgba(gpu)
    };
    let ahead = hshot(neutral_draw(EntityModelKind::Horse))?;
    let looking = hshot(EntityDraw {
        pitch: 35.0,
        head_yaw: 15.0,
        ..neutral_draw(EntityModelKind::Horse)
    })?;
    let mask = |f: &[u8]| -> Vec<bool> {
        f.as_chunks::<4>()
            .0
            .iter()
            .zip(h_empty.as_chunks::<4>().0)
            .map(|(p, e)| p[..3] != e[..3])
            .collect()
    };
    let moved = mask(&ahead)
        .iter()
        .zip(&mask(&looking))
        .filter(|(x, y)| x != y)
        .count();
    let mut layers = [None; rewo_gpu::entities::MAX_ARMOR_SUBLAYERS];
    layers[0] = Some(("horse_saddle/saddle", [1.0, 1.0, 1.0]));
    let saddle = Some(rewo_gpu::entities::ArmorPiece {
        layers,
        ..Default::default()
    });
    let saddled = hshot(EntityDraw {
        saddle,
        ..neutral_draw(EntityModelKind::Horse)
    })?;
    let saddled_looking = hshot(EntityDraw {
        saddle,
        pitch: 35.0,
        head_yaw: 15.0,
        ..neutral_draw(EntityModelKind::Horse)
    })?;
    hst.destroy(gpu);
    // The bridle's own pixels: what the saddle adds over the bare horse, in
    // each pose; looking, the added set must move (a bridle left behind on a
    // turned head adds the same pixels in both).
    let added = |with: &[u8], bare: &[u8]| -> Vec<bool> {
        with.as_chunks::<4>()
            .0
            .iter()
            .zip(bare.as_chunks::<4>().0)
            .map(|(a, b)| a[..3] != b[..3])
            .collect()
    };
    let (a0, a1) = (added(&saddled, &ahead), added(&saddled_looking, &looking));
    let bridle_moved = a0.iter().zip(&a1).filter(|(x, y)| x != y).count();
    w.push(wit(
        "m81.a_horse_looks_and_its_bridle_goes_with_its_head",
        // A bridle left on the static body (the M359 mutant) still moves 89
        // px by occlusion as the head turns over it; the real one 292.
        moved >= 200 && bridle_moved >= 200,
        format!(
            "{moved} px of silhouette moved by the look (want >= 200), {bridle_moved} px of the saddle layer moved with it (want >= 200; a bridle left behind measured 89)"
        ),
    ));

    // m82 — M364: a rabbit (and its kit) that looks down and aside moves its
    // head (`RabbitModel.setupAnim` assigns the look); the silhouette moves.
    let mut rst = stage_from(
        gpu,
        baked,
        cw,
        ch,
        Vec3::new(0.9, 0.55, -0.6),
        Vec3::new(0.0, 0.25, 0.0),
    )?;
    rst.wr.set_entities(&[], rst.right, rst.up, 0.0);
    rst.off.render(gpu, Some((&mut rst.wr, rst.vp)), &ov, BG)?;
    let r_empty = rst.off.read_rgba(gpu)?;
    let r_mask = |f: &[u8]| -> Vec<bool> {
        f.as_chunks::<4>()
            .0
            .iter()
            .zip(r_empty.as_chunks::<4>().0)
            .map(|(p, e)| p[..3] != e[..3])
            .collect()
    };
    let mut r_rows = Vec::new();
    let mut r_moved = Vec::new();
    for kind in [EntityModelKind::Rabbit, EntityModelKind::RabbitBaby] {
        let mut shot = |d: EntityDraw<'_>| -> Result<Vec<u8>, String> {
            rst.wr.set_entities(&[d], rst.right, rst.up, 0.0);
            rst.off.render(gpu, Some((&mut rst.wr, rst.vp)), &ov, BG)?;
            rst.off.read_rgba(gpu)
        };
        let ahead = shot(neutral_draw(kind))?;
        let looking = shot(EntityDraw {
            pitch: 30.0,
            head_yaw: 25.0,
            ..neutral_draw(kind)
        })?;
        let moved = r_mask(&ahead)
            .iter()
            .zip(&r_mask(&looking))
            .filter(|(x, y)| x != y)
            .count();
        r_rows.push(format!("{}: {moved} px", kind.name()));
        r_moved.push(moved);
    }
    rst.destroy(gpu);
    w.push(wit(
        "m82.a_rabbit_and_its_kit_look",
        r_moved[0] >= RABBIT_LOOK_MIN[0] && r_moved[1] >= RABBIT_LOOK_MIN[1],
        format!(
            "silhouette moved by the look: {} (want >= {:?})",
            r_rows.join("; "),
            RABBIT_LOOK_MIN
        ),
    ));

    // m83 — M365: a turtle on land mid-stride sweeps its flippers (against
    // the same turtle standing), swimming moves them differently (against
    // the same stride on land), and an egg shows its belly plate and lifts
    // it 1 px (against no egg).
    let mut tst = stage_from(
        gpu,
        baked,
        cw,
        ch,
        Vec3::new(1.3, 1.5, -1.1),
        Vec3::new(0.0, 0.15, 0.0),
    )?;
    tst.wr.set_entities(&[], tst.right, tst.up, 0.0);
    tst.off.render(gpu, Some((&mut tst.wr, tst.vp)), &ov, BG)?;
    let t_empty = tst.off.read_rgba(gpu)?;
    let mut tshot = |d: EntityDraw<'_>| -> Result<Vec<u8>, String> {
        tst.wr.set_entities(&[d], tst.right, tst.up, 0.0);
        tst.off.render(gpu, Some((&mut tst.wr, tst.vp)), &ov, BG)?;
        tst.off.read_rgba(gpu)
    };
    let turtle = |on_land: bool, egg: bool, amount: f32| EntityDraw {
        limb_swing: 0.3,
        limb_amount: amount,
        mob: rewo_gpu::mobs::MobCombat {
            turtle_on_land: on_land,
            turtle_egg: egg,
            ..Default::default()
        },
        ..neutral_draw(EntityModelKind::Turtle)
    };
    let standing = tshot(turtle(true, false, 0.0))?;
    let striding = tshot(turtle(true, false, 0.8))?;
    let swimming = tshot(turtle(false, false, 0.8))?;
    let with_egg = tshot(turtle(true, true, 0.0))?;
    tst.destroy(gpu);
    let t_mask = |f: &[u8]| -> Vec<bool> {
        f.as_chunks::<4>()
            .0
            .iter()
            .zip(t_empty.as_chunks::<4>().0)
            .map(|(p, e)| p[..3] != e[..3])
            .collect()
    };
    let differ = |a: &[u8], b: &[u8]| {
        t_mask(a)
            .iter()
            .zip(&t_mask(b))
            .filter(|(x, y)| x != y)
            .count()
    };
    let (stride, swim) = (differ(&standing, &striding), differ(&striding, &swimming));
    let egg_px = standing
        .as_chunks::<4>()
        .0
        .iter()
        .zip(with_egg.as_chunks::<4>().0)
        .filter(|(a, b)| a[..3] != b[..3])
        .count();
    w.push(wit(
        "m83.a_turtle_paddles_on_land_swims_in_water_and_carries_its_egg",
        stride >= TURTLE_MIN[0] && swim >= TURTLE_MIN[1] && egg_px >= TURTLE_MIN[2],
        format!(
            "stride vs standing {stride} px, swimming vs the stride on land {swim} px, egg vs none {egg_px} px (want >= {TURTLE_MIN:?})"
        ),
    ));

    // m84 — M366: a strider mid-stride moves (body roll and bob, legs, the
    // bristles' flow) against the same strider standing; `StriderModel`
    // clamps the walk speed at 0.25, so a strider at 1.0 draws exactly as at
    // 0.25 (the pre-M366 legs, unclamped, did not); and standing still its
    // bristles sway with the age alone.
    let mut sst = stage_from(
        gpu,
        baked,
        cw,
        ch,
        Vec3::new(2.2, 1.6, -1.6),
        Vec3::new(0.0, 0.9, 0.0),
    )?;
    sst.wr.set_entities(&[], sst.right, sst.up, 0.0);
    sst.off.render(gpu, Some((&mut sst.wr, sst.vp)), &ov, BG)?;
    let s_empty = sst.off.read_rgba(gpu)?;
    // The age is the frame's time argument (seconds; ageInTicks = 20·t).
    let mut sshot = |d: EntityDraw<'_>, t: f32| -> Result<Vec<u8>, String> {
        sst.wr.set_entities(&[d], sst.right, sst.up, t);
        sst.off.render(gpu, Some((&mut sst.wr, sst.vp)), &ov, BG)?;
        sst.off.read_rgba(gpu)
    };
    let strider = |amount: f32| EntityDraw {
        limb_swing: 0.7,
        limb_amount: amount,
        ..neutral_draw(EntityModelKind::Strider)
    };
    let s_still = sshot(strider(0.0), 0.0)?;
    let s_walk = sshot(strider(0.25), 0.0)?;
    let s_fast = sshot(strider(1.0), 0.0)?;
    let s_later = sshot(strider(0.0), 0.2)?;
    sst.destroy(gpu);
    let s_mask = |f: &[u8]| -> Vec<bool> {
        f.as_chunks::<4>()
            .0
            .iter()
            .zip(s_empty.as_chunks::<4>().0)
            .map(|(p, e)| p[..3] != e[..3])
            .collect()
    };
    let s_differ = |a: &[u8], b: &[u8]| {
        s_mask(a)
            .iter()
            .zip(&s_mask(b))
            .filter(|(x, y)| x != y)
            .count()
    };
    let (s_stride, s_sway) = (s_differ(&s_still, &s_walk), s_differ(&s_still, &s_later));
    let s_clamp = s_walk
        .as_chunks::<4>()
        .0
        .iter()
        .zip(s_fast.as_chunks::<4>().0)
        .filter(|(a, b)| a[..3] != b[..3])
        .count();
    w.push(wit(
        "m84.a_strider_strides_and_its_bristles_sway",
        s_stride >= STRIDER_MIN[0] && s_sway >= STRIDER_MIN[1] && s_clamp == 0,
        format!(
            "stride vs standing {s_stride} px, standing 4 ticks later {s_sway} px (want >= {STRIDER_MIN:?}); speed 1.0 vs 0.25 {s_clamp} px (want 0)"
        ),
    ));

    // m85 — M367: a hoglin's head holds its pitch whatever the look's
    // (`HoglinModel` assigns `xRot`), so looking down draws exactly as
    // looking ahead; the headbutt swings it; the walk flaps the ears and
    // swings the legs.
    let mut hgst = stage_from(
        gpu,
        baked,
        cw,
        ch,
        Vec3::new(2.4, 1.6, -1.8),
        Vec3::new(0.0, 0.8, 0.0),
    )?;
    hgst.wr.set_entities(&[], hgst.right, hgst.up, 0.0);
    hgst.off
        .render(gpu, Some((&mut hgst.wr, hgst.vp)), &ov, BG)?;
    let hg_empty = hgst.off.read_rgba(gpu)?;
    let mut hgshot = |d: EntityDraw<'_>| -> Result<Vec<u8>, String> {
        hgst.wr.set_entities(&[d], hgst.right, hgst.up, 0.0);
        hgst.off
            .render(gpu, Some((&mut hgst.wr, hgst.vp)), &ov, BG)?;
        hgst.off.read_rgba(gpu)
    };
    let hg_ahead = hgshot(neutral_draw(EntityModelKind::Hoglin))?;
    let hg_down = hgshot(EntityDraw {
        pitch: 35.0,
        ..neutral_draw(EntityModelKind::Hoglin)
    })?;
    let hg_butt = hgshot(EntityDraw {
        mob: rewo_gpu::mobs::MobCombat {
            headbutt: 1.0,
            ..Default::default()
        },
        ..neutral_draw(EntityModelKind::Hoglin)
    })?;
    let hg_walk = hgshot(EntityDraw {
        limb_swing: 1.1,
        limb_amount: 0.8,
        ..neutral_draw(EntityModelKind::Hoglin)
    })?;
    hgst.destroy(gpu);
    let hg_mask = |f: &[u8]| -> Vec<bool> {
        f.as_chunks::<4>()
            .0
            .iter()
            .zip(hg_empty.as_chunks::<4>().0)
            .map(|(p, e)| p[..3] != e[..3])
            .collect()
    };
    let hg_differ = |a: &[u8], b: &[u8]| {
        hg_mask(a)
            .iter()
            .zip(&hg_mask(b))
            .filter(|(x, y)| x != y)
            .count()
    };
    let hg_pitch = hg_ahead
        .as_chunks::<4>()
        .0
        .iter()
        .zip(hg_down.as_chunks::<4>().0)
        .filter(|(a, b)| a[..3] != b[..3])
        .count();
    let (hg_b, hg_w) = (
        hg_differ(&hg_ahead, &hg_butt),
        hg_differ(&hg_ahead, &hg_walk),
    );
    w.push(wit(
        "m85.a_hoglin_holds_its_head_headbutts_and_walks",
        hg_pitch == 0 && hg_b >= HOGLIN_MIN[0] && hg_w >= HOGLIN_MIN[1],
        format!(
            "looking down 35 vs ahead {hg_pitch} px (want 0); headbutt {hg_b} px, walk {hg_w} px (want >= {HOGLIN_MIN:?})"
        ),
    ));

    // m86 — M368: a snow golem looking aside turns its upper body and arms a
    // quarter of the way with its head. m87 — M369: an allay flying pitches
    // its body (against hovering), and one holding an item reaches its arms
    // forward (against empty-handed).
    let mut gost = stage_from(
        gpu,
        baked,
        cw,
        ch,
        Vec3::new(1.6, 1.6, -2.4),
        Vec3::new(0.0, 0.9, 0.0),
    )?;
    gost.wr.set_entities(&[], gost.right, gost.up, 0.0);
    gost.off
        .render(gpu, Some((&mut gost.wr, gost.vp)), &ov, BG)?;
    let go_empty = gost.off.read_rgba(gpu)?;
    let mut goshot = |d: EntityDraw<'_>| -> Result<Vec<u8>, String> {
        gost.wr.set_entities(&[d], gost.right, gost.up, 0.0);
        gost.off
            .render(gpu, Some((&mut gost.wr, gost.vp)), &ov, BG)?;
        gost.off.read_rgba(gpu)
    };
    let go_ahead = goshot(neutral_draw(EntityModelKind::SnowGolem))?;
    let go_aside = goshot(EntityDraw {
        head_yaw: 70.0,
        ..neutral_draw(EntityModelKind::SnowGolem)
    })?;
    gost.destroy(gpu);
    let go_mask = |f: &[u8]| -> Vec<bool> {
        f.as_chunks::<4>()
            .0
            .iter()
            .zip(go_empty.as_chunks::<4>().0)
            .map(|(p, e)| p[..3] != e[..3])
            .collect()
    };
    let go_moved = go_mask(&go_ahead)
        .iter()
        .zip(&go_mask(&go_aside))
        .filter(|(x, y)| x != y)
        .count();
    w.push(wit(
        "m86.a_snow_golem_turns_its_body_and_arms_with_its_look",
        go_moved >= SNOW_GOLEM_MIN,
        format!("looking 70 aside vs ahead {go_moved} px of silhouette (want >= {SNOW_GOLEM_MIN})"),
    ));

    let mut alst = stage_from(
        gpu,
        baked,
        cw,
        ch,
        Vec3::new(0.9, 0.6, -0.7),
        Vec3::new(0.0, 0.3, 0.0),
    )?;
    alst.wr.set_entities(&[], alst.right, alst.up, 0.0);
    alst.off
        .render(gpu, Some((&mut alst.wr, alst.vp)), &ov, BG)?;
    let al_empty = alst.off.read_rgba(gpu)?;
    let mut alshot = |d: EntityDraw<'_>| -> Result<Vec<u8>, String> {
        alst.wr.set_entities(&[d], alst.right, alst.up, 0.0);
        alst.off
            .render(gpu, Some((&mut alst.wr, alst.vp)), &ov, BG)?;
        alst.off.read_rgba(gpu)
    };
    let allay = |amount: f32, holding: f32| EntityDraw {
        limb_amount: amount,
        mob: rewo_gpu::mobs::MobCombat {
            allay_holding: holding,
            ..Default::default()
        },
        ..neutral_draw(EntityModelKind::Allay)
    };
    let al_hover = alshot(allay(0.0, 0.0))?;
    let al_fly = alshot(allay(0.6, 0.0))?;
    let al_hold = alshot(allay(0.0, 1.0))?;
    alst.destroy(gpu);
    let al_mask = |f: &[u8]| -> Vec<bool> {
        f.as_chunks::<4>()
            .0
            .iter()
            .zip(al_empty.as_chunks::<4>().0)
            .map(|(p, e)| p[..3] != e[..3])
            .collect()
    };
    let al_differ = |a: &[u8], b: &[u8]| {
        al_mask(a)
            .iter()
            .zip(&al_mask(b))
            .filter(|(x, y)| x != y)
            .count()
    };
    let (al_f, al_h) = (
        al_differ(&al_hover, &al_fly),
        al_differ(&al_hover, &al_hold),
    );
    w.push(wit(
        "m87.an_allay_pitches_into_flight_and_reaches_for_what_it_holds",
        al_f >= ALLAY_MIN[0] && al_h >= ALLAY_MIN[1],
        format!("flying vs hovering {al_f} px, holding vs empty-handed {al_h} px (want >= {ALLAY_MIN:?})"),
    ));

    // m88 — M370: a guardian's body (vanilla's `head`) follows its look, and
    // its tail sways over a second. m89 — M371: a dolphin's body follows its
    // look, and swimming bobs it and strokes its tail (against still).
    let mut gdst = stage_from(
        gpu,
        baked,
        cw,
        ch,
        Vec3::new(2.0, 1.4, -1.8),
        Vec3::new(0.0, 0.5, 0.0),
    )?;
    gdst.wr.set_entities(&[], gdst.right, gdst.up, 0.0);
    gdst.off
        .render(gpu, Some((&mut gdst.wr, gdst.vp)), &ov, BG)?;
    let gd_empty = gdst.off.read_rgba(gpu)?;
    let mut gdshot = |d: EntityDraw<'_>, t: f32| -> Result<Vec<u8>, String> {
        gdst.wr.set_entities(&[d], gdst.right, gdst.up, t);
        gdst.off
            .render(gpu, Some((&mut gdst.wr, gdst.vp)), &ov, BG)?;
        gdst.off.read_rgba(gpu)
    };
    let gd_ahead = gdshot(neutral_draw(EntityModelKind::Guardian), 0.0)?;
    let gd_look = gdshot(
        EntityDraw {
            pitch: 25.0,
            head_yaw: 30.0,
            ..neutral_draw(EntityModelKind::Guardian)
        },
        0.0,
    )?;
    let gd_later = gdshot(neutral_draw(EntityModelKind::Guardian), 1.0)?;
    let dl_ahead = gdshot(neutral_draw(EntityModelKind::Dolphin), 0.0)?;
    let dl_look = gdshot(
        EntityDraw {
            pitch: 20.0,
            head_yaw: 30.0,
            ..neutral_draw(EntityModelKind::Dolphin)
        },
        0.0,
    )?;
    let dl_swim = gdshot(
        EntityDraw {
            mob: rewo_gpu::mobs::MobCombat {
                dolphin_moving: true,
                ..Default::default()
            },
            ..neutral_draw(EntityModelKind::Dolphin)
        },
        0.0,
    )?;
    gdst.destroy(gpu);
    let gd_mask = |f: &[u8]| -> Vec<bool> {
        f.as_chunks::<4>()
            .0
            .iter()
            .zip(gd_empty.as_chunks::<4>().0)
            .map(|(p, e)| p[..3] != e[..3])
            .collect()
    };
    let gd_differ = |a: &[u8], b: &[u8]| {
        gd_mask(a)
            .iter()
            .zip(&gd_mask(b))
            .filter(|(x, y)| x != y)
            .count()
    };
    let (gd_l, gd_t) = (
        gd_differ(&gd_ahead, &gd_look),
        gd_differ(&gd_ahead, &gd_later),
    );
    w.push(wit(
        "m88.a_guardian_looks_and_its_tail_sways",
        gd_l >= GUARDIAN_MIN[0] && gd_t >= GUARDIAN_MIN[1],
        format!("looking vs ahead {gd_l} px, a second later {gd_t} px (want >= {GUARDIAN_MIN:?})"),
    ));
    let (dl_l, dl_s) = (
        gd_differ(&dl_ahead, &dl_look),
        gd_differ(&dl_ahead, &dl_swim),
    );
    w.push(wit(
        "m89.a_dolphin_looks_and_strokes_as_it_swims",
        dl_l >= DOLPHIN_MIN[0] && dl_s >= DOLPHIN_MIN[1],
        format!(
            "looking vs ahead {dl_l} px, swimming vs still {dl_s} px (want >= {DOLPHIN_MIN:?})"
        ),
    ));

    // m90 — M372: a nautilus's soft body follows a small look, and stops at
    // 10°: looking 30° aside draws exactly as looking 10° aside. m91 — M373:
    // a happy ghast's tentacles swing over a second.
    let mut nast = stage_from(
        gpu,
        baked,
        cw,
        ch,
        Vec3::new(1.8, 1.2, -1.8),
        Vec3::new(0.0, 0.6, 0.0),
    )?;
    nast.wr.set_entities(&[], nast.right, nast.up, 0.0);
    nast.off
        .render(gpu, Some((&mut nast.wr, nast.vp)), &ov, BG)?;
    let na_empty = nast.off.read_rgba(gpu)?;
    let mut nashot = |d: EntityDraw<'_>, t: f32| -> Result<Vec<u8>, String> {
        nast.wr.set_entities(&[d], nast.right, nast.up, t);
        nast.off
            .render(gpu, Some((&mut nast.wr, nast.vp)), &ov, BG)?;
        nast.off.read_rgba(gpu)
    };
    let look = |yaw: f32| EntityDraw {
        head_yaw: yaw,
        ..neutral_draw(EntityModelKind::Nautilus)
    };
    let na_ahead = nashot(look(0.0), 0.0)?;
    let na_ten = nashot(look(10.0), 0.0)?;
    let na_thirty = nashot(look(30.0), 0.0)?;
    nast.destroy(gpu);
    let na_mask = |f: &[u8]| -> Vec<bool> {
        f.as_chunks::<4>()
            .0
            .iter()
            .zip(na_empty.as_chunks::<4>().0)
            .map(|(p, e)| p[..3] != e[..3])
            .collect()
    };
    let na_look = na_mask(&na_ahead)
        .iter()
        .zip(&na_mask(&na_ten))
        .filter(|(x, y)| x != y)
        .count();
    let na_clamp = na_ten
        .as_chunks::<4>()
        .0
        .iter()
        .zip(na_thirty.as_chunks::<4>().0)
        .filter(|(a, b)| a[..3] != b[..3])
        .count();
    w.push(wit(
        "m90.a_nautilus_body_follows_its_look_within_ten_degrees",
        na_look >= NAUTILUS_MIN && na_clamp == 0,
        format!(
            "looking 10 aside vs ahead {na_look} px (want >= {NAUTILUS_MIN}), 30 vs 10 {na_clamp} px (want 0)"
        ),
    ));

    let mut hgh = stage_from(
        gpu,
        baked,
        cw,
        ch,
        // Below the body: the tentacles hang under the entity's feet line.
        Vec3::new(7.0, -2.5, -7.0),
        Vec3::new(0.0, 0.0, 0.0),
    )?;
    hgh.wr.set_entities(&[], hgh.right, hgh.up, 0.0);
    hgh.off.render(gpu, Some((&mut hgh.wr, hgh.vp)), &ov, BG)?;
    let hh_empty = hgh.off.read_rgba(gpu)?;
    let mut hhshot = |t: f32| -> Result<Vec<u8>, String> {
        hgh.wr.set_entities(
            &[neutral_draw(EntityModelKind::HappyGhast)],
            hgh.right,
            hgh.up,
            t,
        );
        hgh.off.render(gpu, Some((&mut hgh.wr, hgh.vp)), &ov, BG)?;
        hgh.off.read_rgba(gpu)
    };
    let (hh0, hh1) = (hhshot(0.0)?, hhshot(1.0)?);
    hgh.destroy(gpu);
    let hh_mask = |f: &[u8]| -> Vec<bool> {
        f.as_chunks::<4>()
            .0
            .iter()
            .zip(hh_empty.as_chunks::<4>().0)
            .map(|(p, e)| p[..3] != e[..3])
            .collect()
    };
    let hh_moved = hh_mask(&hh0)
        .iter()
        .zip(&hh_mask(&hh1))
        .filter(|(x, y)| x != y)
        .count();
    w.push(wit(
        "m91.a_happy_ghasts_tentacles_swing",
        hh_moved >= HAPPY_GHAST_MIN,
        format!("a second apart {hh_moved} px of silhouette (want >= {HAPPY_GHAST_MIN})"),
    ));

    // m92 — M374: the warden's gait caps its speed term (any speed above 1/6
    // draws alike: 0.2 vs 1.0 is 0 px), and standing it sways with the age.
    // m93 — M375: a ravager's stride swings its legs at vanilla's 0.4·speed,
    // not the quadruped's 1.4 (graded from above: the old swing was 3.5×).
    let mut wdst = stage_from(
        gpu,
        baked,
        cw,
        ch,
        Vec3::new(3.4, 2.0, -3.0),
        Vec3::new(0.0, 1.4, 0.0),
    )?;
    wdst.wr.set_entities(&[], wdst.right, wdst.up, 0.0);
    wdst.off
        .render(gpu, Some((&mut wdst.wr, wdst.vp)), &ov, BG)?;
    let wd_empty = wdst.off.read_rgba(gpu)?;
    let mut wdshot = |d: EntityDraw<'_>, t: f32| -> Result<Vec<u8>, String> {
        wdst.wr.set_entities(&[d], wdst.right, wdst.up, t);
        wdst.off
            .render(gpu, Some((&mut wdst.wr, wdst.vp)), &ov, BG)?;
        wdst.off.read_rgba(gpu)
    };
    let walker = |kind: EntityModelKind, amount: f32| EntityDraw {
        limb_swing: 1.3,
        limb_amount: amount,
        ..neutral_draw(kind)
    };
    let wd_slow = wdshot(walker(EntityModelKind::Warden, 0.2), 0.0)?;
    let wd_fast = wdshot(walker(EntityModelKind::Warden, 1.0), 0.0)?;
    let wd_still = wdshot(walker(EntityModelKind::Warden, 0.0), 0.0)?;
    let wd_later = wdshot(walker(EntityModelKind::Warden, 0.0), 1.5)?;
    let rv_still = wdshot(walker(EntityModelKind::Ravager, 0.0), 0.0)?;
    let rv_walk = wdshot(walker(EntityModelKind::Ravager, 0.7), 0.0)?;
    wdst.destroy(gpu);
    let wd_mask = |f: &[u8]| -> Vec<bool> {
        f.as_chunks::<4>()
            .0
            .iter()
            .zip(wd_empty.as_chunks::<4>().0)
            .map(|(p, e)| p[..3] != e[..3])
            .collect()
    };
    let wd_differ = |a: &[u8], b: &[u8]| {
        wd_mask(a)
            .iter()
            .zip(&wd_mask(b))
            .filter(|(x, y)| x != y)
            .count()
    };
    let wd_cap = wd_slow
        .as_chunks::<4>()
        .0
        .iter()
        .zip(wd_fast.as_chunks::<4>().0)
        .filter(|(a, b)| a[..3] != b[..3])
        .count();
    let wd_sway = wd_differ(&wd_still, &wd_later);
    w.push(wit(
        "m92.a_warden_caps_its_stride_and_sways_standing",
        wd_cap == 0 && wd_sway >= WARDEN_MIN,
        format!("speed 0.2 vs 1.0 {wd_cap} px (want 0), standing 30 ticks later {wd_sway} px (want >= {WARDEN_MIN})"),
    ));
    let rv_stride = wd_differ(&rv_still, &rv_walk);
    w.push(wit(
        "m93.a_ravager_strides_at_vanillas_reach",
        (RAVAGER_STRIDE[0]..=RAVAGER_STRIDE[1]).contains(&rv_stride),
        format!("stride vs standing {rv_stride} px (want {RAVAGER_STRIDE:?})"),
    ));

    // m94 — M376: the wither breathes (ribcage and tail) across a breath.
    // m95 — M377: a camel looking 80° aside draws as at 30° (its clamp); a
    // breeze and a flying bat looking aside draw as looking ahead (their
    // heads do not look); a tadpole's tail sways.
    let mut smst = stage_from(
        gpu,
        baked,
        cw,
        ch,
        Vec3::new(2.4, 1.8, -2.4),
        Vec3::new(0.0, 1.0, 0.0),
    )?;
    smst.wr.set_entities(&[], smst.right, smst.up, 0.0);
    smst.off
        .render(gpu, Some((&mut smst.wr, smst.vp)), &ov, BG)?;
    let sm_empty = smst.off.read_rgba(gpu)?;
    let mut smshot = |d: EntityDraw<'_>, t: f32| -> Result<Vec<u8>, String> {
        smst.wr.set_entities(&[d], smst.right, smst.up, t);
        smst.off
            .render(gpu, Some((&mut smst.wr, smst.vp)), &ov, BG)?;
        smst.off.read_rgba(gpu)
    };
    let aside = |kind: EntityModelKind, yaw: f32| EntityDraw {
        head_yaw: yaw,
        ..neutral_draw(kind)
    };
    let wi0 = smshot(neutral_draw(EntityModelKind::Wither), 0.0)?;
    let wi1 = smshot(neutral_draw(EntityModelKind::Wither), 1.6)?;
    let ca30 = smshot(aside(EntityModelKind::Camel, 30.0), 0.0)?;
    let ca80 = smshot(aside(EntityModelKind::Camel, 80.0), 0.0)?;
    let br0 = smshot(aside(EntityModelKind::Breeze, 0.0), 0.0)?;
    let br40 = smshot(aside(EntityModelKind::Breeze, 40.0), 0.0)?;
    let ba0 = smshot(aside(EntityModelKind::Bat, 0.0), 0.0)?;
    let ba40 = smshot(aside(EntityModelKind::Bat, 40.0), 0.0)?;
    let ta0 = smshot(neutral_draw(EntityModelKind::Tadpole), 0.0)?;
    let ta1 = smshot(neutral_draw(EntityModelKind::Tadpole), 0.25)?;
    smst.destroy(gpu);
    let sm_mask = |f: &[u8]| -> Vec<bool> {
        f.as_chunks::<4>()
            .0
            .iter()
            .zip(sm_empty.as_chunks::<4>().0)
            .map(|(p, e)| p[..3] != e[..3])
            .collect()
    };
    let sm_moved = |a: &[u8], b: &[u8]| {
        sm_mask(a)
            .iter()
            .zip(&sm_mask(b))
            .filter(|(x, y)| x != y)
            .count()
    };
    let sm_same = |a: &[u8], b: &[u8]| {
        a.as_chunks::<4>()
            .0
            .iter()
            .zip(b.as_chunks::<4>().0)
            .filter(|(p, q)| p[..3] != q[..3])
            .count()
    };
    let wi_breath = sm_moved(&wi0, &wi1);
    w.push(wit(
        "m94.the_wither_breathes",
        wi_breath >= WITHER_MIN,
        format!("across a breath {wi_breath} px of silhouette (want >= {WITHER_MIN})"),
    ));
    let (ca, br, ba, ta) = (
        sm_same(&ca30, &ca80),
        sm_same(&br0, &br40),
        sm_same(&ba0, &ba40),
        sm_moved(&ta0, &ta1),
    );
    w.push(wit(
        "m95.clamped_and_still_heads_and_a_swaying_tadpole",
        ca == 0 && br == 0 && ba == 0 && ta >= TADPOLE_MIN,
        format!(
            "camel 80 vs 30 aside {ca} px, breeze aside vs ahead {br} px, bat {ba} px (want 0 each); tadpole 5 ticks apart {ta} px (want >= {TADPOLE_MIN})"
        ),
    ));

    // m96 — M378: a parrot sits (against standing) and walks its legs and
    // tail (against standing). m97 — M379: a drinking witch lifts her nose
    // (against not; the idle wobble, a few degrees, is sub-pixel here and
    // left to the unit test), and an enderman's arms bob with the humanoid
    // idle.
    let mut pwst = stage_from(
        gpu,
        baked,
        cw,
        ch,
        Vec3::new(0.9, 0.7, -0.9),
        Vec3::new(0.0, 0.35, 0.0),
    )?;
    pwst.wr.set_entities(&[], pwst.right, pwst.up, 0.0);
    pwst.off
        .render(gpu, Some((&mut pwst.wr, pwst.vp)), &ov, BG)?;
    let pw_empty = pwst.off.read_rgba(gpu)?;
    let mut pwshot = |d: EntityDraw<'_>, t: f32| -> Result<Vec<u8>, String> {
        pwst.wr.set_entities(&[d], pwst.right, pwst.up, t);
        pwst.off
            .render(gpu, Some((&mut pwst.wr, pwst.vp)), &ov, BG)?;
        pwst.off.read_rgba(gpu)
    };
    let pa_stand = pwshot(neutral_draw(EntityModelKind::Parrot), 0.0)?;
    let pa_sit = pwshot(
        EntityDraw {
            mob: rewo_gpu::mobs::MobCombat {
                sitting: true,
                ..Default::default()
            },
            ..neutral_draw(EntityModelKind::Parrot)
        },
        0.0,
    )?;
    let pa_walk = pwshot(
        EntityDraw {
            limb_swing: 1.1,
            limb_amount: 0.8,
            ..neutral_draw(EntityModelKind::Parrot)
        },
        0.0,
    )?;
    pwst.destroy(gpu);
    let mut wnst = stage_from(
        gpu,
        baked,
        cw,
        ch,
        // In front: a mob at yaw 0 faces +z, and the nose is on its face.
        Vec3::new(1.4, 2.4, 1.6),
        Vec3::new(0.0, 1.9, 0.0),
    )?;
    wnst.wr.set_entities(&[], wnst.right, wnst.up, 0.0);
    wnst.off
        .render(gpu, Some((&mut wnst.wr, wnst.vp)), &ov, BG)?;
    let wn_empty = wnst.off.read_rgba(gpu)?;
    let mut wnshot = |d: EntityDraw<'_>, t: f32| -> Result<Vec<u8>, String> {
        wnst.wr.set_entities(&[d], wnst.right, wnst.up, t);
        wnst.off
            .render(gpu, Some((&mut wnst.wr, wnst.vp)), &ov, BG)?;
        wnst.off.read_rgba(gpu)
    };
    let witch = |holding: bool| EntityDraw {
        mob: rewo_gpu::mobs::MobCombat {
            witch_nose_rate: 0.09,
            witch_holding: holding,
            ..Default::default()
        },
        ..neutral_draw(EntityModelKind::Witch)
    };
    let wn0 = wnshot(witch(false), 0.0)?;
    let wn1 = wnshot(witch(true), 0.0)?;
    let en0 = wnshot(neutral_draw(EntityModelKind::Enderman), 0.0)?;
    let en1 = wnshot(neutral_draw(EntityModelKind::Enderman), 2.0)?;
    wnst.destroy(gpu);
    let moved = |empty: &[u8], a: &[u8], b: &[u8]| {
        let mask = |f: &[u8]| -> Vec<bool> {
            f.as_chunks::<4>()
                .0
                .iter()
                .zip(empty.as_chunks::<4>().0)
                .map(|(p, e)| p[..3] != e[..3])
                .collect()
        };
        mask(a).iter().zip(&mask(b)).filter(|(x, y)| x != y).count()
    };
    let (pa_s, pa_w) = (
        moved(&pw_empty, &pa_stand, &pa_sit),
        moved(&pw_empty, &pa_stand, &pa_walk),
    );
    w.push(wit(
        "m96.a_parrot_sits_and_walks",
        pa_s >= PARROT_MIN[0] && pa_w >= PARROT_MIN[1],
        format!(
            "sitting vs standing {pa_s} px, walking vs standing {pa_w} px (want >= {PARROT_MIN:?})"
        ),
    ));
    // The nose moves inside the head's outline from here: count changed
    // pixels, not silhouette.
    let wn = wn0
        .as_chunks::<4>()
        .0
        .iter()
        .zip(wn1.as_chunks::<4>().0)
        .filter(|(a, b)| a[..3] != b[..3])
        .count();
    let en = moved(&wn_empty, &en0, &en1);
    w.push(wit(
        "m97.a_witchs_nose_wobbles_and_an_endermans_arms_bob",
        wn >= WITCH_ENDERMAN_MIN[0] && en >= WITCH_ENDERMAN_MIN[1],
        format!("witch drinking vs not {wn} px changed, enderman 40 ticks apart {en} px (want >= {WITCH_ENDERMAN_MIN:?})"),
    ));

    // m98 — M380: a piglin standing still flaps its ears and bobs its arms
    // over a second (the creeper's swapped leg phases are invisible by
    // design and left to the unit test).
    let mut pgst = stage_from(
        gpu,
        baked,
        cw,
        ch,
        Vec3::new(1.6, 2.2, 2.0),
        Vec3::new(0.0, 1.3, 0.0),
    )?;
    pgst.wr.set_entities(&[], pgst.right, pgst.up, 0.0);
    pgst.off
        .render(gpu, Some((&mut pgst.wr, pgst.vp)), &ov, BG)?;
    let pg_empty = pgst.off.read_rgba(gpu)?;
    let mut pgshot = |t: f32| -> Result<Vec<u8>, String> {
        pgst.wr.set_entities(
            &[neutral_draw(EntityModelKind::Piglin)],
            pgst.right,
            pgst.up,
            t,
        );
        pgst.off
            .render(gpu, Some((&mut pgst.wr, pgst.vp)), &ov, BG)?;
        pgst.off.read_rgba(gpu)
    };
    let (pg0, pg1) = (pgshot(0.0)?, pgshot(1.0)?);
    pgst.destroy(gpu);
    let pg_mask = |f: &[u8]| -> Vec<bool> {
        f.as_chunks::<4>()
            .0
            .iter()
            .zip(pg_empty.as_chunks::<4>().0)
            .map(|(p, e)| p[..3] != e[..3])
            .collect()
    };
    let pg_moved = pg_mask(&pg0)
        .iter()
        .zip(&pg_mask(&pg1))
        .filter(|(x, y)| x != y)
        .count();
    w.push(wit(
        "m98.a_standing_piglin_flaps_its_ears_and_bobs_its_arms",
        pg_moved >= PIGLIN_MIN,
        format!("a second apart {pg_moved} px of silhouette (want >= {PIGLIN_MIN})"),
    ));

    // m99 — M381: a seated rider: a zombie (a `HumanoidModel`) and a
    // pillager (an `IllagerModel`) riding against standing.
    let mut rdst = stage_from(
        gpu,
        baked,
        cw,
        ch,
        Vec3::new(2.2, 1.6, 2.2),
        Vec3::new(0.0, 0.9, 0.0),
    )?;
    rdst.wr.set_entities(&[], rdst.right, rdst.up, 0.0);
    rdst.off
        .render(gpu, Some((&mut rdst.wr, rdst.vp)), &ov, BG)?;
    let rd_empty = rdst.off.read_rgba(gpu)?;
    let mut rdshot = |d: EntityDraw<'_>| -> Result<Vec<u8>, String> {
        rdst.wr.set_entities(&[d], rdst.right, rdst.up, 0.0);
        rdst.off
            .render(gpu, Some((&mut rdst.wr, rdst.vp)), &ov, BG)?;
        rdst.off.read_rgba(gpu)
    };
    let seated = |kind: EntityModelKind, riding: bool| EntityDraw {
        mob: rewo_gpu::mobs::MobCombat {
            riding,
            ..Default::default()
        },
        ..neutral_draw(kind)
    };
    let mut rd_rows = Vec::new();
    let mut rd_ok = true;
    for kind in [EntityModelKind::Zombie, EntityModelKind::Pillager] {
        let stand = rdshot(seated(kind, false))?;
        let ride = rdshot(seated(kind, true))?;
        let mask = |f: &[u8]| -> Vec<bool> {
            f.as_chunks::<4>()
                .0
                .iter()
                .zip(rd_empty.as_chunks::<4>().0)
                .map(|(p, e)| p[..3] != e[..3])
                .collect()
        };
        let n = mask(&stand)
            .iter()
            .zip(&mask(&ride))
            .filter(|(x, y)| x != y)
            .count();
        rd_ok &= n >= RIDER_MIN;
        rd_rows.push(format!("{}: {n} px", kind.name()));
    }
    rdst.destroy(gpu);
    w.push(wit(
        "m99.a_rider_sits",
        rd_ok,
        format!(
            "riding vs standing {} (want >= {RIDER_MIN} each)",
            rd_rows.join("; ")
        ),
    ));

    // m100 — M382: a drowned with its right arm posed `THROW_TRIDENT` raises
    // it (`DrownedModel`'s post-step over the zombie arm), against unposed.
    let mut drst = stage_from(
        gpu,
        baked,
        cw,
        ch,
        Vec3::new(2.2, 1.8, 2.2),
        Vec3::new(0.0, 1.1, 0.0),
    )?;
    drst.wr.set_entities(&[], drst.right, drst.up, 0.0);
    drst.off
        .render(gpu, Some((&mut drst.wr, drst.vp)), &ov, BG)?;
    let dr_empty = drst.off.read_rgba(gpu)?;
    let mut drshot = |d: EntityDraw<'_>| -> Result<Vec<u8>, String> {
        drst.wr.set_entities(&[d], drst.right, drst.up, 0.0);
        drst.off
            .render(gpu, Some((&mut drst.wr, drst.vp)), &ov, BG)?;
        drst.off.read_rgba(gpu)
    };
    let dr_plain = drshot(neutral_draw(EntityModelKind::Drowned))?;
    let dr_throw = drshot(EntityDraw {
        arm_poses: rewo_gpu::mobs::ArmPoses {
            right: rewo_gpu::mobs::ArmPose::ThrowTrident,
            known: true,
            ..rewo_gpu::mobs::ArmPoses::EMPTY
        },
        ..neutral_draw(EntityModelKind::Drowned)
    })?;
    drst.destroy(gpu);
    let dr_mask = |f: &[u8]| -> Vec<bool> {
        f.as_chunks::<4>()
            .0
            .iter()
            .zip(dr_empty.as_chunks::<4>().0)
            .map(|(p, e)| p[..3] != e[..3])
            .collect()
    };
    let dr_moved = dr_mask(&dr_plain)
        .iter()
        .zip(&dr_mask(&dr_throw))
        .filter(|(x, y)| x != y)
        .count();
    w.push(wit(
        "m100.a_drowned_raises_its_trident",
        dr_moved >= DROWNED_MIN,
        format!("throwing vs unposed {dr_moved} px of silhouette (want >= {DROWNED_MIN})"),
    ));

    // m101 — M383: the stray's clothing is the humanoid mesh (4-wide limbs at
    // 0.25), so its silhouette covers far more than the plain skeleton's.
    let mut skst = stage_from(
        gpu,
        baked,
        cw,
        ch,
        Vec3::new(0.0, 1.1, 2.6),
        Vec3::new(0.0, 1.0, 0.0),
    )?;
    skst.wr.set_entities(&[], skst.right, skst.up, 0.0);
    skst.off
        .render(gpu, Some((&mut skst.wr, skst.vp)), &ov, BG)?;
    let sk_empty = skst.off.read_rgba(gpu)?;
    let mut skshot = |kind: EntityModelKind| -> Result<Vec<u8>, String> {
        skst.wr
            .set_entities(&[neutral_draw(kind)], skst.right, skst.up, 0.0);
        skst.off
            .render(gpu, Some((&mut skst.wr, skst.vp)), &ov, BG)?;
        skst.off.read_rgba(gpu)
    };
    let sk_area = |f: &[u8]| {
        f.as_chunks::<4>()
            .0
            .iter()
            .zip(sk_empty.as_chunks::<4>().0)
            .filter(|(p, e)| p[..3] != e[..3])
            .count()
    };
    let (sk, st) = (
        sk_area(&skshot(EntityModelKind::Skeleton)?),
        sk_area(&skshot(EntityModelKind::Stray)?),
    );
    skst.destroy(gpu);
    let sk_gain = st.saturating_sub(sk);
    w.push(wit(
        "m101.a_strays_clothing_is_the_humanoid_mesh",
        sk_gain >= STRAY_CLOTHING_MIN,
        format!("stray {st} px against skeleton {sk} px: {sk_gain} px of clothing (want >= {STRAY_CLOTHING_MIN})"),
    ));

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
    Witness { name, ok, detail }
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
    /// M306 — the sheets of the model's `entityTranslucent` tail, blended in
    /// gamma space ([`translucent_explains`]).
    translucent: Vec<&'static str>,
}

fn first_unexplained(
    px: &[u32],
    own: &OwnSheets,
    sheets: &HashMap<&str, HashSet<u32>>,
    lin: &HashMap<&str, Vec<([f32; 3], f32)>>,
) -> Option<u32> {
    first_unexplained_over(px, None, own, sheets, lin)
}

/// [`first_unexplained`], with `beneath[i]` the colour the frame without
/// this mob has where `px[i]` was drawn: what a translucent layer blended
/// over when nothing of the mob's own lay under it (M306; the stage draws a
/// sky, so there is no constant to assume).
fn first_unexplained_over(
    px: &[u32],
    beneath: Option<&[u32]>,
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
    let mut seen: HashSet<u64> = HashSet::new();
    let mut translucent_tables: Option<(Vec<([f32; 3], f32)>, Vec<[f32; 3]>)> = None;
    for (j, c) in px.iter().enumerate() {
        let under = beneath.and_then(|b| b.get(j).copied());
        if !seen.insert((*c as u64) << 32 | under.unwrap_or(u32::MAX) as u64) {
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
        if blended {
            continue;
        }
        // M306 — a translucent layer's pixel: its texel blended in gamma space
        // over what lay beneath: this mob's own colour, the frame without the
        // mob at this pixel, or (a shell's back face first) the same layer
        // over that pixel. The tables are built once, on the first colour that
        // needs them.
        if !own.translucent.is_empty() {
            let (src, own_dst) = translucent_tables.get_or_insert_with(|| {
                let src: Vec<([f32; 3], f32)> = own
                    .translucent
                    .iter()
                    .filter_map(|k| lin.get(k))
                    .flatten()
                    .filter(|(_, a)| *a >= 0.1)
                    .copied()
                    .collect();
                let own_dst: Vec<[f32; 3]> = sets
                    .iter()
                    .flat_map(|s| s.iter())
                    .map(|c| unpack_gamma(*c))
                    .collect();
                (src, own_dst)
            });
            let over_under = under.is_some_and(|u| {
                let u = unpack_gamma(u);
                let mut dst = vec![u];
                dst.extend(
                    src.iter()
                        .map(|(s, a)| gamma_blend(*s, *a, u).map(|b| b as f32 / 255.0)),
                );
                translucent_explains(*c, src, &dst)
            });
            if over_under || translucent_explains(*c, src, own_dst) {
                continue;
            }
        }
        return Some(*c);
    }
    None
}

/// M306 — `ENTITY_TRANSLUCENT`'s store: `entity.frag` (GAMMA_OUT) leaves the
/// lit colour sRGB-encoded and the UNORM view blends it with the stored byte,
/// `a * src + (1 - a) * dst`, rounded to a byte.
fn gamma_blend(lin: [f32; 3], a: f32, dst: [f32; 3]) -> [u8; 3] {
    let enc = |c: f32| {
        let c = c.clamp(0.0, 1.0);
        if c <= 0.0031308 {
            c * 12.92
        } else {
            1.055 * c.powf(1.0 / 2.4) - 0.055
        }
    };
    [0, 1, 2].map(|i| ((a * enc(lin[i]) + (1.0 - a) * dst[i]) * 255.0).round() as u8)
}

fn unpack_gamma(c: u32) -> [f32; 3] {
    [
        ((c >> 16) & 255) as f32 / 255.0,
        ((c >> 8) & 255) as f32 / 255.0,
        (c & 255) as f32 / 255.0,
    ]
}

/// Whether `c` is some translucent texel over some candidate beneath, ±1.
fn translucent_explains(c: u32, src: &[([f32; 3], f32)], dst: &[[f32; 3]]) -> bool {
    let want = [(c >> 16) as u8, ((c >> 8) & 255) as u8, (c & 255) as u8];
    src.iter().any(|(s, a)| {
        dst.iter().any(|d| {
            let got = gamma_blend(*s, *a, *d);
            (0..3).all(|i| (got[i] as i32 - want[i] as i32).abs() <= 1)
        })
    })
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
            Ok(e) => match e.name() {
                Ok(n) => n.into_owned(),
                Err(_) => continue,
            },
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
    let view = crate::view_math::look_to_rh(eye, dir, Vec3::Y);
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
