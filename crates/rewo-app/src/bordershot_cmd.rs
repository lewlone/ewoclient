//! `rewo bordershot --check` — the world-border oracle (M80).
//!
//! Four layers, graded differently because the three halves of this feature
//! fail in different ways:
//!
//! - **`s*` — the state machine.** Pure arithmetic, so every expectation is an
//!   *independent* transcription of the decompiled formula written out again
//!   here. Nothing below reads `rewo_world::border`'s own arithmetic as its
//!   expectation.
//! - **`w*` — the wire.** Bodies assembled by hand and driven through the
//!   production `route_border`, with the six ids resolved by name out of the
//!   real `packets.json`.
//! - **`p*` — the physics.** Measured as a **displacement**, never as a
//!   correction count: M67's audit recorded that `rewo play`'s `CORRECTIONS 0`
//!   is structurally unable to see a movement the client fails to *stop*, and
//!   a border is exactly that kind of consequence.
//! - **`g*` — the wall.** Vulkan read-back through the production
//!   `WorldRenderer`, validation on.
//!
//! # The detector
//!
//! Three false measurements on this project have all been the same shape: a
//! signal hunted against a background that could already produce it. The wall
//! is a scrolling tinted texture over sky and terrain, so both are removed —
//! `SkyMode::None`, no columns, a **black** clear — and `g1` asserts that an
//! invisible border leaves the frame byte-for-byte black before any other
//! pixel witness runs. Everything non-black after that is the wall and nothing
//! else. The forcefield texture is **synthetic**, never the jar's: an oracle
//! whose expected value depends on an asset's contents is testing the asset.
//!
//! Over a black clear at alpha 1 the arithmetic collapses to something exact
//! and convention-independent. `BlendFunction.OVERLAY` weights the source by
//! its own alpha and adds — `dst = 0 + linear(tint) * 1` — and the sRGB
//! attachment re-encodes on store, so the stored byte is the **status colour's
//! byte**, whether the blend happens in linear or gamma space. `g3` predicts
//! it on the CPU rather than comparing two renders.

use std::path::PathBuf;

use clap::Args as ClapArgs;
use rewo_data::packets::Packets;
use rewo_data::DataPaths;
use rewo_gpu::offscreen::Offscreen;
use rewo_gpu::overlay::OverlayDraw;
use rewo_gpu::world::{perspective_reverse_z, SkyMode, WorldRenderer};
use rewo_gpu::Gpu;
use rewo_net::ids::Ids;
use rewo_proto::varint::{write_varint, write_varlong};
use rewo_world::abilities::Abilities;
use rewo_world::border::{BorderStatus, WorldBorder};
use rewo_world::physics::{tick_with, PlayerState, TickInput, PLAYER_HALF_WIDTH};

use crate::stats::OverlayRing;

/// A witness that stops running is a failure, not a quieter pass.
const EXPECTED_WITNESSES: usize = 31;

const CLEAR: [f32; 4] = [0.0, 0.0, 0.0, 1.0];
const W: u32 = 128;
const H: u32 = 128;

// ── Independent transcriptions ────────────────────────────────────────────
// Every constant below is read off the decompile, not off `rewo_world::border`.

/// `BorderStatus`'s three `getColor()` ints, as vanilla spells them.
const GROWING_COLOR: u32 = 4_259_712;
const SHRINKING_COLOR: u32 = 16_724_016;
const STATIONARY_COLOR: u32 = 2_138_367;
/// `WorldBorder.absoluteMaxSize`'s field initializer.
const ABSOLUTE_MAX: f64 = 29_999_984.0;
/// `Mth.lerp(alpha, p0, p1)`.
fn lerp(alpha: f64, p0: f64, p1: f64) -> f64 {
    p0 + alpha * (p1 - p0)
}

/// `MovingBorderExtent.calculateSize`, transcribed a second time.
fn expected_size(from: f64, to: f64, duration: f64, progress: i64) -> f64 {
    let p = (duration - progress as f64) / duration;
    if p < 1.0 {
        lerp(p, from, to)
    } else {
        to
    }
}

/// Channel tolerance in stored 0..255 units. `u1` below proves the differences
/// this gate resolves are far larger.
const CHANNEL_TOL: f32 = 2.0;

#[derive(ClapArgs, Debug)]
pub struct BordershotArgs {
    /// Grade every witness and exit nonzero on any failure.
    #[arg(long)]
    pub check: bool,
    /// Version whose `packets.json` resolves the six border ids.
    #[arg(long, default_value = "26.2")]
    pub version: String,
    /// Skip the Vulkan validation-layer requirement (the pixel layer only).
    #[arg(long)]
    pub no_validation: bool,
    /// Write the graded frames here as PNGs.
    #[arg(long)]
    pub out_dir: Option<PathBuf>,
}

struct Checker {
    witnessed: usize,
    failures: Vec<String>,
}

impl Checker {
    fn record(&mut self, name: &str, pass: bool, detail: impl std::fmt::Display) {
        println!(
            "[bordershot] {}  {name}: {detail}",
            if pass { " ok " } else { "FAIL" }
        );
        if pass {
            self.witnessed += 1;
        } else {
            self.failures.push(name.to_string());
        }
    }
}

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(c: f32) -> f32 {
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

/// The stored byte a fully-opaque white texel produces over a black clear, for
/// one tint channel at one modulator alpha. `OVERLAY` is `(SRC_ALPHA, ONE)`, so
/// `dst = 0 + linear(tint) * alpha`, then the attachment encodes.
fn expected_channel(tint_byte: u8, alpha: f32) -> f32 {
    let lin = srgb_to_linear(tint_byte as f32 / 255.0) * alpha;
    linear_to_srgb(lin.clamp(0.0, 1.0)) * 255.0
}

// ── §s the state machine ──────────────────────────────────────────────────

fn check_state(c: &mut Checker) {
    // MUTATION: in `WorldBorder::tick`, recompute `size` before decrementing
    // `progress`. Every sample below is taken ON a bound — the first tick, the
    // last, and one past — because a lerp's off-by-one hides in the middle.
    let mut b = WorldBorder::default();
    b.lerp_size_between(100.0, 300.0, 8);
    let mut seen = vec![b.size()];
    for _ in 0..10 {
        b.tick();
        seen.push(b.size());
    }
    let want: Vec<f64> = (0..=8)
        .map(|n| expected_size(100.0, 300.0, 8.0, 8 - n))
        .chain([300.0, 300.0])
        .collect();
    c.record(
        "s1.the_lerp_walks_one_tick_at_a_time_from_the_source_to_the_target",
        seen.iter().zip(&want).all(|(a, b)| (a - b).abs() < 1e-9),
        format!(
            "sizes over eleven ticks {seen:?} match an independent \
             `lerp((duration - progress)/duration, from, to)` at every step \
             including tick 0, tick 8 and tick 9 — the three the off-by-one \
             mutation moves"
        ),
    );

    // MUTATION: `progress < 0` in place of `<= 0`.
    let mut b = WorldBorder::default();
    b.lerp_size_between(100.0, 300.0, 3);
    let statuses: Vec<BorderStatus> = (0..4)
        .map(|_| {
            let s = b.status();
            b.tick();
            s
        })
        .collect();
    c.record(
        "s2.the_extent_collapses_to_static_on_the_last_tick_not_after_it",
        statuses == vec![
            BorderStatus::Growing,
            BorderStatus::Growing,
            BorderStatus::Growing,
            BorderStatus::Stationary,
        ],
        format!(
            "{statuses:?} — three moving ticks for a three-tick lerp, then \
             stationary. An `< 0` guard would give a fourth moving tick with \
             the size already at the target"
        ),
    );

    // MUTATION: make `min_x(partial)` read `size` unconditionally.
    let mut b = WorldBorder::default();
    b.lerp_size_between(100.0, 200.0, 10);
    b.tick(); // size 110, previousSize 100
    let (p0, ph, p1) = (b.min_x(0.0), b.min_x(0.5), b.min_x(1.0));
    c.record(
        "s3.partial_zero_reads_the_previous_tick_and_partial_one_reads_this_one",
        (p0 + 50.0).abs() < 1e-9 && (ph + 52.5).abs() < 1e-9 && (p1 + 55.0).abs() < 1e-9,
        format!(
            "minX at partial 0 / 0.5 / 1 is {p0} / {ph} / {p1} against sizes \
             100 / 105 / 110. `getMinX()` is `getMinX(0.0F)`, so collision, \
             `isWithinBounds` and the HUD vignette all measure the PREVIOUS \
             tick's box while the renderer measures this one"
        ),
    );

    // MUTATION: make `set_size` retarget the running extent instead of
    // replacing it.
    let mut b = WorldBorder::default();
    b.lerp_size_between(1000.0, 100.0, 1000);
    b.tick();
    b.set_size(256.0);
    let after = b.size();
    b.tick();
    c.record(
        "s4.set_size_cancels_an_in_flight_lerp_rather_than_retargeting_it",
        after == 256.0 && b.size() == 256.0 && b.status() == BorderStatus::Stationary,
        format!(
            "mid-shrink, `setSize(256)` gives {after} and stays there — it \
             assigns a whole new `StaticBorderExtent`, so the animation is \
             thrown away, not redirected"
        ),
    );

    // MUTATION: swap the two arms of `to < from`.
    let mut grow = WorldBorder::default();
    grow.lerp_size_between(10.0, 20.0, 5);
    let mut shrink = WorldBorder::default();
    shrink.lerp_size_between(20.0, 10.0, 5);
    c.record(
        "s5.the_status_is_derived_from_the_lerp_direction_and_never_sent",
        grow.status() == BorderStatus::Growing
            && shrink.status() == BorderStatus::Shrinking
            && WorldBorder::default().status() == BorderStatus::Stationary,
        "10→20 is GROWING, 20→10 is SHRINKING, no lerp is STATIONARY — nothing \
         on the wire carries this; `MovingBorderExtent.getStatus` computes it"
            .to_string(),
    );

    // MUTATION: any digit of the three colour constants.
    c.record(
        "s6.the_three_status_colours_are_the_decompiled_integers",
        BorderStatus::Growing.color() == GROWING_COLOR
            && BorderStatus::Shrinking.color() == SHRINKING_COLOR
            && BorderStatus::Stationary.color() == STATIONARY_COLOR,
        format!(
            "GROWING {GROWING_COLOR} (0x{GROWING_COLOR:06X}, green), SHRINKING \
             {SHRINKING_COLOR} (0x{SHRINKING_COLOR:06X}, red), STATIONARY \
             {STATIONARY_COLOR} (0x{STATIONARY_COLOR:06X}, blue) — transcribed \
             here as decimals so a hex typo in either place disagrees"
        ),
    );

    // MUTATION: write `MAX_SIZE` as its decimal spelling, 59_999_970.0.
    let fresh = WorldBorder::default();
    c.record(
        "s7.max_size_is_the_float_literal_widened_not_its_decimal_spelling",
        fresh.size() == 59_999_968.0 && fresh.size() / 2.0 == ABSOLUTE_MAX,
        format!(
            "`5.999997E7F` rounds to {} as an f32 before widening, and its half \
             is exactly `absoluteMaxSize` {ABSOLUTE_MAX}. Reading the literal \
             as 59999970 gives a default border two blocks wider that the \
             clamp then silently corrects — invisible in every render",
            fresh.size()
        ),
    );

    // MUTATION: drop the `Mth.clamp` in `getMinX`/`getMaxX`.
    let mut b = WorldBorder::default();
    b.set_size(1.0e9);
    c.record(
        "s8.the_box_is_clamped_to_the_absolute_max_size",
        b.min_x(0.0) == -ABSOLUTE_MAX && b.max_x(0.0) == ABSOLUTE_MAX,
        format!(
            "a billion-block border reports [{}, {}] — `Mth.clamp(centre ± \
             size/2, ±absoluteMaxSize)` binds",
            b.min_x(0.0),
            b.max_x(0.0)
        ),
    );

    // MUTATION: `abs()` the result of `distance_to_border`.
    let mut b = WorldBorder::default();
    b.set_center(0.0, 0.0);
    b.set_size(20.0);
    let (inside, at, outside) = (
        b.distance_to_border(3.0, 0.0),
        b.distance_to_border(10.0, 0.0),
        b.distance_to_border(13.5, 0.0),
    );
    c.record(
        "s9.the_distance_to_the_border_goes_negative_outside_it",
        (inside - 7.0).abs() < 1e-9 && at == 0.0 && (outside + 3.5).abs() < 1e-9,
        format!(
            "at x = 3 / 10 / 13.5 in a ±10 border the distance is {inside} / \
             {at} / {outside}. The sign is what makes the wall's alpha saturate \
             outside and what the damage tick reads server-side"
        ),
    );

    // MUTATION: swap the `min` and the `max` in `warning_distance`. The three
    // samples are chosen so a *different one of the three terms wins each
    // time* — a single sample cannot distinguish the two operators.
    let mut b = WorldBorder::default();
    b.set_warning_blocks(5);
    b.set_warning_time(60);
    // 200 → 100 over 100 ticks, so `lerpSpeed` is 1 block of diameter per tick
    // and `lerpSpeed × warningTime` is 60.
    b.lerp_size_between(200.0, 100.0, 100);
    let by_speed = b.warning_distance();
    for _ in 0..50 {
        b.tick();
    }
    let by_travel = b.warning_distance();
    for _ in 0..47 {
        b.tick();
    }
    let by_flat = b.warning_distance();
    c.record(
        "s10.the_warning_threshold_is_max_of_the_flat_blocks_and_the_capped_travel",
        by_speed == 60.0 && by_travel == 50.0 && by_flat == 5.0,
        format!(
            "one sample per regime: fresh, {by_speed} — `lerpSpeed × \
             warningTime` (1 × 60), under the 100 blocks still to travel. \
             Half-way, {by_travel} — the remaining travel now caps the 60. \
             Nearly done, {by_flat} — three blocks left, so the flat \
             `warningBlocks` floor takes over. The delay and the distance are \
             ONE threshold, not two code paths, and each of its three terms \
             wins exactly once above"
        ),
    );

    // MUTATION: compute the strength wholly in f64 and narrow at the end.
    let mut b = WorldBorder::default();
    b.set_center(0.0, 0.0);
    b.set_size(20.0);
    b.set_warning_blocks(5);
    let s = b.warning_strength(6.0, 0.0);
    c.record(
        "s11.the_warning_strength_narrows_through_f32_the_way_the_hud_does",
        s == 1.0f32 - 0.8f32 && s != 0.2f32,
        format!(
            "four blocks from a five-block threshold gives {s:?}, which is \
             `1.0F - (float)(4.0/5.0)` and NOT 0.2 — `Hud.extractVignette` \
             narrows the distance to a float first and subtracts in float"
        ),
    );

    // MUTATION: drop `calculateSize`'s `progress < 1.0` guard.
    let mut b = WorldBorder::default();
    b.lerp_size_between(100.0, 200.0, 0);
    c.record(
        "s12.a_zero_duration_lerp_reports_the_target_rather_than_a_nan",
        b.size() == 200.0 && b.lerp_speed().is_infinite(),
        format!(
            "size {} and lerp speed {} — the progress is 0/0, and `NaN < 1.0` \
             is false, so the `else` arm hands back `to`. Reachable from the \
             wire, because `set_border_lerp_size` has no zero guard",
            b.size(),
            b.lerp_speed()
        ),
    );
}

// ── §w the wire ───────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn init_body(
    cx: f64,
    cz: f64,
    old: f64,
    new: f64,
    lerp: i64,
    abs_max: i32,
    blocks: i32,
    time: i32,
) -> Vec<u8> {
    let mut b = Vec::new();
    for v in [cx, cz, old, new] {
        b.extend_from_slice(&v.to_be_bytes());
    }
    write_varlong(&mut b, lerp);
    write_varint(&mut b, abs_max);
    write_varint(&mut b, blocks);
    write_varint(&mut b, time);
    b
}

fn check_wire(c: &mut Checker, ids: &Ids) {
    let six = [
        ("initialize_border", ids.cb_play_initialize_border, 43),
        ("set_border_center", ids.cb_play_set_border_center, 88),
        ("set_border_lerp_size", ids.cb_play_set_border_lerp_size, 89),
        ("set_border_size", ids.cb_play_set_border_size, 90),
        ("set_border_warning_delay", ids.cb_play_set_border_warning_delay, 91),
        (
            "set_border_warning_distance",
            ids.cb_play_set_border_warning_distance,
            92,
        ),
    ];
    // MUTATION: resolve any of the six by a neighbouring name.
    c.record(
        "w1.all_six_ids_resolve_by_name_out_of_the_real_report",
        six.iter().all(|(_, got, want)| got == want),
        format!(
            "{:?} — resolved through the production `Ids::resolve` against \
             `packets.json`, so a renumber fails loud instead of misfiring",
            six.iter().map(|(n, g, _)| (*n, *g)).collect::<Vec<_>>()
        ),
    );

    // MUTATION: read `lerpTime` with `varint()` instead of `varlong()`.
    let mut b = WorldBorder::default();
    let body = init_body(12.5, -33.25, 700.0, 250.0, 400, 29_999_984, 9, 260);
    let claimed = rewo_net::route_border(ids.cb_play_initialize_border, &body, ids, &mut b);
    c.record(
        "w2.initialize_border_decodes_all_eight_fields_in_order",
        claimed
            && b.center_x() == 12.5
            && b.center_z() == -33.25
            && b.size() == 700.0
            && b.lerp_target() == 250.0
            && b.lerp_time() == 400
            && b.absolute_max_size() == 29_999_984
            && b.warning_blocks() == 9
            && b.warning_time() == 260,
        format!(
            "centre ({}, {}), size {} → {} over {} ticks, absMax {}, warn \
             {}b/{}t. The var-long sits between four f64s and three VarInts, so \
             a var-int reader would leave the three trailing fields shifted",
            b.center_x(),
            b.center_z(),
            b.size(),
            b.lerp_target(),
            b.lerp_time(),
            b.absolute_max_size(),
            b.warning_blocks(),
            b.warning_time()
        ),
    );

    // The sensitivity partner for w2: a duration a var-int cannot hold.
    let mut b = WorldBorder::default();
    let big = 1i64 << 34;
    let body = init_body(0.0, 0.0, 700.0, 250.0, big, 29_999_984, 9, 260);
    rewo_net::route_border(ids.cb_play_initialize_border, &body, ids, &mut b);
    c.record(
        "w3.the_lerp_duration_really_is_a_var_long",
        b.lerp_time() == big && b.warning_time() == 260,
        format!(
            "a {big}-tick duration survives AND the two VarInts after it still \
             read 9 and {} — a five-byte var-int would stop mid-field and drag \
             everything behind it out of position",
            b.warning_time()
        ),
    );

    // MUTATION: add the `lerpTime > 0` guard to `handleSetBorderLerpSize` too.
    // The *same numbers* on the two packets, so nothing but the guard differs.
    let mut lerp_body = Vec::new();
    lerp_body.extend_from_slice(&500.0f64.to_be_bytes());
    lerp_body.extend_from_slice(&64.0f64.to_be_bytes());
    write_varlong(&mut lerp_body, 0);
    let mut via_lerp = WorldBorder::default();
    rewo_net::route_border(ids.cb_play_set_border_lerp_size, &lerp_body, ids, &mut via_lerp);
    let mut via_init = WorldBorder::default();
    rewo_net::route_border(
        ids.cb_play_initialize_border,
        &init_body(0.0, 0.0, 500.0, 64.0, 0, 29_999_984, 5, 300),
        ids,
        &mut via_init,
    );
    c.record(
        "w4.only_initialize_border_guards_on_a_zero_lerp_time",
        via_lerp.status() == BorderStatus::Shrinking
            && via_init.status() == BorderStatus::Stationary
            && via_lerp.size() == 64.0
            && via_init.size() == 64.0,
        format!(
            "(500 → 64 over 0 ticks) leaves `set_border_lerp_size` {:?} and \
             `initialize_border` {:?}, at the same size. `handleInitializeBorder` \
             writes `if (lerpTime > 0L) … else setSize(newSize)`; \
             `handleSetBorderLerpSize` calls `lerpSizeBetween` flat",
            via_lerp.status(),
            via_init.status()
        ),
    );

    // MUTATION: swap the two arms — write `warningBlocks` from the delay packet
    // and `warningTime` from the distance one. Both bodies are one VarInt, so
    // the swap decodes cleanly and is silent.
    let mut b = WorldBorder::default();
    let mut delay = Vec::new();
    write_varint(&mut delay, 123);
    let mut distance = Vec::new();
    write_varint(&mut distance, 45);
    rewo_net::route_border(ids.cb_play_set_border_warning_delay, &delay, ids, &mut b);
    rewo_net::route_border(ids.cb_play_set_border_warning_distance, &distance, ids, &mut b);
    c.record(
        "w5.the_two_warning_packets_cross_their_names_onto_the_right_fields",
        b.warning_time() == 123 && b.warning_blocks() == 45,
        format!(
            "`warning_delay` 123 → warningTime {}, `warning_distance` 45 → \
             warningBlocks {}. Identical bodies and different fields, so only \
             the packet id can tell them apart",
            b.warning_time(),
            b.warning_blocks()
        ),
    );

    // MUTATION: apply the fields decoded so far before hitting the short read.
    let mut b = WorldBorder::default();
    b.set_size(77.0);
    let before = b;
    let short = rewo_net::route_border(ids.cb_play_initialize_border, &[0u8; 12], ids, &mut b);
    c.record(
        "w6.a_truncated_body_is_claimed_by_the_router_and_writes_nothing",
        short && b == before,
        "twelve bytes of a thirty-plus-byte `initialize_border`: the router \
         still reports the id as handled (so the ladder does not fall through \
         to another arm) and the border is untouched"
            .to_string(),
    );

    // MUTATION: make `kind_for_id` return a default kind instead of `None`.
    let mut b = WorldBorder::default();
    let stolen = rewo_net::route_border(ids.cb_play_set_time, &[0u8; 32], ids, &mut b);
    c.record(
        "w7.the_router_declines_an_id_that_is_not_one_of_the_six",
        !stolen && b == WorldBorder::default(),
        format!(
            "`set_time` (id {}) is not claimed, so it reaches its own arm \
             further down the ladder",
            ids.cb_play_set_time
        ),
    );
}

// ── §p the physics ────────────────────────────────────────────────────────

/// Flat ground at y < 0 and nothing else, so the only thing that can stop a
/// walk is the border.
fn ground(_x: i32, y: i32, _z: i32) -> &'static [[f32; 6]] {
    const CUBE: &[[f32; 6]] = &[[0.0, 0.0, 0.0, 1.0, 1.0, 1.0]];
    if y < 0 {
        CUBE
    } else {
        &[]
    }
}

/// Walk forward for `ticks` and hand back the final z.
fn walk(start_z: f64, border: Option<rewo_world::border::BorderCollision>, no_clip: bool) -> f64 {
    let mut p = PlayerState::at(0.0, 0.0, start_z);
    let input = TickInput {
        forward: 1.0,
        ..Default::default()
    };
    let mut ab = Abilities::default();
    ab.flying = no_clip;
    for _ in 0..240 {
        tick_with(&mut p, &input, &ab, no_clip, border, &ground);
    }
    p.z
}

fn check_physics(c: &mut Checker) {
    let mut b = WorldBorder::default();
    b.set_center(0.0, 0.0);
    b.set_size(20.0);
    let wall = b.collision();

    // MUTATION: drop the border clamp from `physics::collide`. Measured as a
    // DISPLACEMENT, not as a correction count — vanilla's move check flags a
    // client that moves too *much*, and a client that ignores the wall moves
    // too *little*, so the correction meter is structurally blind here (M67's
    // audit finding, M68's remedy).
    let stopped = walk(0.0, Some(wall), false);
    let free = walk(0.0, None, false);
    c.record(
        "p1.the_border_stops_a_walking_player_at_the_wall",
        stopped <= 10.0 - PLAYER_HALF_WIDTH + 1e-6 && stopped > 9.0 && free > 30.0,
        format!(
            "240 ticks of holding forward from the centre of a ±10 border ends \
             at z = {stopped:.4}, with the box's far face on the wall; the same \
             240 ticks with no border reach z = {free:.1}. The gap is {:.1} \
             blocks of displacement, which is the witness — not a correction \
             count",
            free - stopped
        ),
    );

    // MUTATION: drop `isInsideCloseToBorder` and add the shape unconditionally.
    //
    // The sample has to be chosen with care, and the first one was wrong. A
    // player four blocks outside the +Z wall walking *back* toward it is
    // unblocked with or without the gate, because `clip_border` inherits
    // `clip_axis`'s `gap >= -EPS` rule and a face behind you never clips you —
    // so that witness could not fail, and it was measuring the gap guard
    // rather than the gate. What the gate actually decides is whether the
    // **other** axis's walls apply out there: `isWithinBounds(x, z, bbMax)`
    // fails on the z term, so the whole shape is withheld and the player walks
    // freely past the x walls they are nowhere near.
    let mut sideways = PlayerState::at(0.0, 0.0, 14.0);
    let strafe = TickInput {
        strafe: -1.0,
        ..Default::default()
    };
    for _ in 0..240 {
        tick_with(
            &mut sideways,
            &strafe,
            &Abilities::default(),
            false,
            Some(wall),
            &ground,
        );
    }
    let mut inward = PlayerState::at(0.0, 0.0, 14.0);
    let north = TickInput {
        forward: -1.0,
        ..Default::default()
    };
    for _ in 0..240 {
        tick_with(
            &mut inward,
            &north,
            &Abilities::default(),
            false,
            Some(wall),
            &ground,
        );
    }
    c.record(
        "p2.the_shape_is_withheld_from_a_player_who_is_already_outside",
        sideways.x.abs() > 20.0 && inward.z < 11.0,
        format!(
            "four blocks past the +Z wall of a ±10 border, strafing reaches x = \
             {:.1} — straight through where that side wall would be, because \
             `isWithinBounds(x, z, bbMax)` fails on its z term and the collider \
             is never added at all. Walking back inward still works too (z = \
             {:.3}). Without the gate the shape is an infinite complement, and \
             the x walls would fence in a player standing outside the z ones",
            sideways.x, inward.z
        ),
    );

    // MUTATION: pass the border through the `no_clip` arm as well.
    let ghost = walk(0.0, Some(wall), true);
    c.record(
        "p3.no_clip_passes_through_the_wall",
        ghost > 30.0,
        format!(
            "a spectator reaches z = {ghost:.1} — `Entity.move`'s `noPhysics` \
             arm never builds a collider list, so the border is skipped along \
             with the blocks"
        ),
    );

    // MUTATION: use the exact bounds as the collision planes instead of
    // flooring and ceiling them.
    let mut frac = WorldBorder::default();
    frac.set_center(0.0, 0.0);
    frac.set_size(20.5); // ±10.25
    let stopped = walk(0.0, Some(frac.collision()), false);
    c.record(
        "p4.you_collide_with_the_floored_wall_not_the_one_you_can_see",
        stopped > 10.25 - PLAYER_HALF_WIDTH && stopped <= 11.0 - PLAYER_HALF_WIDTH + 1e-6,
        format!(
            "a ±10.25 border stops the player at z = {stopped:.4} — past the \
             *visible* wall at 10.25 and against the *floored* one at 11. \
             `getCollisionShape` snaps its box outward with `Math.floor` / \
             `Math.ceil`, so the two disagree for any fractional border"
        ),
    );
}

// ── §g the wall ───────────────────────────────────────────────────────────

/// The camera sits five blocks inside the north wall of a ±20 border, looking
/// straight at it, with a render distance that reaches only that one wall.
const BORDER_HALF: f64 = 20.0;
const CAM: [f64; 3] = [0.0, 0.0, -15.0];
const RENDER_DISTANCE: f64 = 10.0;
const DEPTH_FAR: f32 = 512.0;

fn state(tint: u32, alpha: f64) -> rewo_gpu::border::BorderState {
    rewo_gpu::border::BorderState {
        min_x: -BORDER_HALF,
        max_x: BORDER_HALF,
        min_z: -BORDER_HALF,
        max_z: BORDER_HALF,
        tint,
        alpha,
    }
}

fn view_proj() -> [[f32; 4]; 4] {
    let proj = perspective_reverse_z(70f32.to_radians(), W as f32 / H as f32, 0.05);
    let eye = glam::Vec3::new(CAM[0] as f32, CAM[1] as f32, CAM[2] as f32);
    let view = glam::Mat4::look_at_rh(eye, eye - glam::Vec3::Z, glam::Vec3::Y);
    (glam::Mat4::from_cols_array_2d(&proj) * view).to_cols_array_2d()
}

/// A uniform RGBA texture.
fn flat_tex(rgba: [u8; 4], side: u32) -> Vec<u8> {
    (0..side * side).flat_map(|_| rgba).collect()
}

/// Half the rows opaque white, half at `low` — structure the scroll can move.
fn banded_tex(low: [u8; 4], side: u32) -> Vec<u8> {
    (0..side * side)
        .flat_map(|i| {
            if (i / side) < side / 2 {
                [255u8, 255, 255, 255]
            } else {
                low
            }
        })
        .collect()
}

fn overlay_offscreen(ring: &OverlayRing) -> OverlayDraw<'_> {
    OverlayDraw {
        samples_ms: &ring.data,
        head: ring.head(),
        scale_ms: 20.0,
        origin: [-4000.0, -4000.0],
        size: [8.0, 8.0],
    }
}

fn max_channel(px: &[u8]) -> u8 {
    px.chunks_exact(4)
        .flat_map(|p| p[..3].iter().copied())
        .max()
        .unwrap_or(0)
}

fn bytes_differing(a: &[u8], b: &[u8]) -> usize {
    a.iter().zip(b).filter(|(x, y)| x != y).count()
}

/// The mean of one channel over the pixels whose colour is not black.
fn mean_lit(px: &[u8], ch: usize) -> f32 {
    let mut sum = 0f64;
    let mut n = 0usize;
    for p in px.chunks_exact(4) {
        if p[0] | p[1] | p[2] != 0 {
            sum += p[ch] as f64;
            n += 1;
        }
    }
    if n == 0 {
        0.0
    } else {
        (sum / n as f64) as f32
    }
}

fn lit_fraction(px: &[u8]) -> f32 {
    let n = px.chunks_exact(4).filter(|p| p[0] | p[1] | p[2] != 0).count();
    n as f32 / (px.len() / 4) as f32
}

fn check_pixels(c: &mut Checker, args: &BordershotArgs) -> Result<(), String> {
    let want_validation = !args.no_validation;
    let mut gpu = Gpu::new(None, want_validation)?;
    println!(
        "[bordershot] Vulkan validation: {}",
        if gpu.validation_active { "ON" } else { "off" }
    );
    if args.check && want_validation && !gpu.validation_active {
        return Err(
            "bordershot check: Vulkan validation requested but not active — install \
             the Vulkan SDK (VK_LAYER_KHRONOS_validation), or pass --no-validation"
                .into(),
        );
    }
    let mut off = Offscreen::new(&mut gpu, W, H)?;
    let ring = OverlayRing::default();
    let draw = overlay_offscreen(&ring);
    let vp = view_proj();

    let r = (|| -> Result<(), String> {
        // One white 16x16 texel sheet: the *only* structure in the frame comes
        // from the geometry and the tint, never from an asset.
        let white = flat_tex([255, 255, 255, 255], 16);
        let layers = [vec![255u8; 16 * 16 * 4]];
        let mut wr = WorldRenderer::new(&mut gpu, off.format, 16, &layers)?;
        wr.set_sky_mode(SkyMode::None);
        wr.set_camera([CAM[0] as f32, CAM[1] as f32, CAM[2] as f32]);
        wr.init_border(
            &mut gpu,
            &rewo_gpu::border::BorderImage {
                rgba: &white,
                w: 16,
                h: 16,
            },
        )?;

        let inner = (|| -> Result<(), String> {
            let mut shot = |gpu: &mut Gpu,
                            off: &mut Offscreen,
                            wr: &mut WorldRenderer,
                            d: Option<&rewo_gpu::border::BorderDraw>|
             -> Result<Vec<u8>, String> {
                wr.set_border(gpu, d)?;
                off.render(gpu, Some((&mut *wr, vp)), &draw, CLEAR)?;
                off.read_rgba(gpu)
            };

            // g1 — the detector. Nothing drawn must be byte-for-byte black.
            let empty = shot(&mut gpu, &mut off, &mut wr, None)?;
            // MUTATION: leave the previous frame's geometry in place when
            // `set_border` is handed `None`.
            c.record(
                "g1.an_invisible_border_leaves_the_frame_black",
                max_channel(&empty) == 0,
                format!(
                    "the brightest colour channel anywhere is {} with no border \
                     set, no sky pass and no terrain — so every non-black pixel \
                     below is the wall and nothing else. This is the guard \
                     against the detector error three earlier milestones made",
                    max_channel(&empty)
                ),
            );

            // g2 — the wall paints, and it covers the view.
            let stationary = rewo_gpu::border::BorderDraw::build(
                &state(STATIONARY_COLOR, 1.0),
                CAM,
                RENDER_DISTANCE,
                DEPTH_FAR,
                0,
            );
            let shot_stationary = shot(&mut gpu, &mut off, &mut wr, Some(&stationary))?;
            let lit = lit_fraction(&shot_stationary);
            // MUTATION: emit fewer than four vertices per wall, or drop the
            // index buffer.
            c.record(
                "g2.a_visible_border_fills_the_view_with_a_wall",
                lit > 0.99 && stationary.sides == vec![rewo_gpu::border::NORTH],
                format!(
                    "{:.1}% of the frame is lit, and the draw list is {:?} — one \
                     wall, because only the north one is inside the \
                     {RENDER_DISTANCE}-block render distance from a camera five \
                     blocks off it",
                    lit * 100.0,
                    stationary.sides
                ),
            );

            // g3 — the tint, predicted on the CPU for all three statuses.
            // MUTATION: transpose the `BorderStatus::color` table.
            let mut ok = true;
            let mut detail = Vec::new();
            for (name, tint) in [
                ("STATIONARY", STATIONARY_COLOR),
                ("GROWING", GROWING_COLOR),
                ("SHRINKING", SHRINKING_COLOR),
            ] {
                let d = rewo_gpu::border::BorderDraw::build(
                    &state(tint, 1.0),
                    CAM,
                    RENDER_DISTANCE,
                    DEPTH_FAR,
                    0,
                );
                let px = shot(&mut gpu, &mut off, &mut wr, Some(&d))?;
                let got = [mean_lit(&px, 0), mean_lit(&px, 1), mean_lit(&px, 2)];
                let want = [
                    ((tint >> 16) & 0xFF) as f32,
                    ((tint >> 8) & 0xFF) as f32,
                    (tint & 0xFF) as f32,
                ];
                let err = (0..3).map(|i| (got[i] - want[i]).abs()).fold(0f32, f32::max);
                ok &= err <= CHANNEL_TOL;
                detail.push(format!("{name} {got:?} vs {want:?} (err {err:.2})"));
                if let Some(dir) = &args.out_dir {
                    let _ = std::fs::create_dir_all(dir);
                    let _ = off.save_png(&gpu, &dir.join(format!("border_{name}.png")));
                }
            }
            c.record(
                "g3.the_wall_stores_the_status_colour_exactly",
                ok,
                format!(
                    "{}. At alpha 1 over a black clear the OVERLAY blend is \
                     `0 + linear(tint) × 1` and the sRGB attachment re-encodes, \
                     so the stored byte IS the status byte — an exact CPU \
                     prediction rather than a comparison of two renders, and \
                     one that holds whether the blend runs in linear or gamma",
                    detail.join("; ")
                ),
            );

            // g4 — alpha weights the addend.
            // MUTATION: drop `state.alpha` from the colour modulator.
            let half = rewo_gpu::border::BorderDraw::build(
                &state(STATIONARY_COLOR, 0.5),
                CAM,
                RENDER_DISTANCE,
                DEPTH_FAR,
                0,
            );
            let px = shot(&mut gpu, &mut off, &mut wr, Some(&half))?;
            let got = [mean_lit(&px, 0), mean_lit(&px, 1), mean_lit(&px, 2)];
            let want = [
                expected_channel(((STATIONARY_COLOR >> 16) & 0xFF) as u8, 0.5),
                expected_channel(((STATIONARY_COLOR >> 8) & 0xFF) as u8, 0.5),
                expected_channel((STATIONARY_COLOR & 0xFF) as u8, 0.5),
            ];
            let err = (0..3).map(|i| (got[i] - want[i]).abs()).fold(0f32, f32::max);
            c.record(
                "g4.the_extract_alpha_weights_the_wall_through_the_blend",
                err <= CHANNEL_TOL,
                format!(
                    "at alpha 0.5 the wall stores {got:?} against a predicted \
                     {want:?} (err {err:.2}) — `encode(decode(tint) × 0.5)`, \
                     which is NOT half the byte, because the weighting happens \
                     in linear space"
                ),
            );

            // g5 — the geometry is welded to the border, not to the camera.
            // MUTATION: emit vanilla's camera-relative positions without
            // cancelling the camera back out (M33's error).
            let near = rewo_gpu::border::BorderDraw::build(
                &state(STATIONARY_COLOR, 1.0),
                [0.0, 0.0, -19.0],
                RENDER_DISTANCE,
                DEPTH_FAR,
                0,
            );
            let z_far: Vec<f32> = stationary.verts[rewo_gpu::border::NORTH * 4..][..4]
                .iter()
                .map(|v| v.pos[2])
                .collect();
            let z_near: Vec<f32> = near.verts[rewo_gpu::border::NORTH * 4..][..4]
                .iter()
                .map(|v| v.pos[2])
                .collect();
            c.record(
                "g5.the_wall_stays_on_the_border_when_the_camera_moves",
                z_far == z_near && z_far == vec![-BORDER_HALF as f32; 4],
                format!(
                    "moving the camera four blocks toward the wall leaves the \
                     north quad at z = {z_near:?}. Vanilla's positions are \
                     camera-relative and its `ModelOffset` cancels the camera \
                     back out; porting only half of that would drag the whole \
                     border along behind the player"
                ),
            );

            // g6 — the scroll. Needs structure in the texture, so this shot
            // rebuilds the pass over a banded sheet.
            wr.destroy(&mut gpu);
            let banded = banded_tex([64, 64, 64, 255], 16);
            let mut wr2 = WorldRenderer::new(&mut gpu, off.format, 16, &layers)?;
            wr2.set_sky_mode(SkyMode::None);
            wr2.set_camera([CAM[0] as f32, CAM[1] as f32, CAM[2] as f32]);
            wr2.init_border(
                &mut gpu,
                &rewo_gpu::border::BorderImage {
                    rgba: &banded,
                    w: 16,
                    h: 16,
                },
            )?;
            let at = |ms: u64| {
                rewo_gpu::border::BorderDraw::build(
                    &state(STATIONARY_COLOR, 1.0),
                    CAM,
                    RENDER_DISTANCE,
                    DEPTH_FAR,
                    ms,
                )
            };
            let t0 = shot(&mut gpu, &mut off, &mut wr2, Some(&at(0)))?;
            let t_half = shot(&mut gpu, &mut off, &mut wr2, Some(&at(1500)))?;
            let t_wrap = shot(&mut gpu, &mut off, &mut wr2, Some(&at(3000)))?;
            let moved = bytes_differing(&t0, &t_half);
            let wrapped = bytes_differing(&t0, &t_wrap);
            // MUTATION: pin `tex_offset` to zero, or drive it from a tick
            // counter instead of `Util.getMillis()`.
            c.record(
                "g6.the_texture_scrolls_on_a_three_second_wall_clock",
                moved > t0.len() / 8 && wrapped == 0,
                format!(
                    "1500 ms into the cycle {moved} of {} bytes have changed \
                     with the camera, the border and the geometry all fixed; at \
                     3000 ms the frame is byte-identical to 0 ms ({wrapped} \
                     bytes differ). The period is `Util.getMillis() % 3000L`, \
                     the only wall-clock quantity in this feature — everything \
                     else is tick-derived",
                    t0.len()
                ),
            );

            // g7 — the transparent-texel discard, observed through the alpha
            // channel. `OVERLAY`'s alpha equation is `(ONE, ZERO)`, so a drawn
            // fragment *replaces* the destination alpha and a discarded one
            // leaves the clear's.
            let mut half_clear = flat_tex([255, 255, 255, 255], 16);
            for (i, px) in half_clear.chunks_exact_mut(4).enumerate() {
                if i / 16 >= 8 {
                    px[3] = 0;
                }
            }
            wr2.destroy(&mut gpu);
            let mut wr3 = WorldRenderer::new(&mut gpu, off.format, 16, &layers)?;
            wr3.set_sky_mode(SkyMode::None);
            wr3.set_camera([CAM[0] as f32, CAM[1] as f32, CAM[2] as f32]);
            wr3.init_border(
                &mut gpu,
                &rewo_gpu::border::BorderImage {
                    rgba: &half_clear,
                    w: 16,
                    h: 16,
                },
            )?;
            // The modulator's alpha is a **half**, which is what separates the
            // two populations: a drawn fragment writes `texA × 0.5`, so it can
            // never exceed 128, while a discarded one leaves the clear's 255.
            // (A first cut used alpha 1.0 and could not tell them apart — and
            // the sampler is LINEAR, so the texture's alpha band arrives as a
            // ramp rather than two values. Neither is a reason to change the
            // sampler; it is a reason to grade the two populations rather than
            // a pair of exact numbers.)
            let discard_draw = rewo_gpu::border::BorderDraw::build(
                &state(STATIONARY_COLOR, 0.5),
                CAM,
                RENDER_DISTANCE,
                DEPTH_FAR,
                0,
            );
            let px = shot(&mut gpu, &mut off, &mut wr3, Some(&discard_draw))?;
            let kept = px.chunks_exact(4).filter(|p| p[3] == 255).count();
            let written: Vec<u8> = px
                .chunks_exact(4)
                .map(|p| p[3])
                .filter(|a| *a != 255)
                .collect();
            let hottest = written.iter().copied().max().unwrap_or(0);
            // MUTATION: remove the `if (color.a == 0.0) discard;`. With this
            // blend the *colour* is unchanged either way — a zero-alpha texel
            // contributes `rgb × 0` — so the discard is only observable in the
            // alpha channel, which is why this witness reads alpha and not rgb.
            // Without the discard the whole 255 population disappears, because
            // `OVERLAY`'s alpha equation is `(ONE, ZERO)` and every fragment
            // would then overwrite the clear.
            c.record(
                "g7.a_fully_transparent_texel_is_discarded_and_leaves_the_alpha_alone",
                kept > 0 && !written.is_empty() && hottest <= 128,
                format!(
                    "{kept} pixels still carry the clear's alpha 255 while the \
                     other {} top out at {hottest} — a clean gap, because the \
                     modulator's 0.5 caps a drawn fragment at 128. Dropping the \
                     `== 0.0` discard would let every fragment write, and the \
                     255 population would vanish entirely",
                    written.len()
                ),
            );

            // g8 — nothing above read a stale buffer.
            let again = shot(&mut gpu, &mut off, &mut wr3, None)?;
            if again != empty {
                // Which channel moved is the whole diagnosis here — a stale
                // draw list shows up in rgb, a leaked destination alpha only in
                // the fourth. Printed only on failure so a green run stays quiet.
                let per_ch: Vec<usize> = (0..4)
                    .map(|ch| {
                        again
                            .chunks_exact(4)
                            .zip(empty.chunks_exact(4))
                            .filter(|(a, b)| a[ch] != b[ch])
                            .count()
                    })
                    .collect();
                println!("[bordershot] g8 differing pixels per channel: {per_ch:?}");
            }
            c.record(
                "g8.clearing_the_border_restores_the_empty_frame_byte_for_byte",
                again == empty,
                format!(
                    "after seven different walls, `set_border(None)` reproduces \
                     g1's frame exactly ({} bytes differ) — so no witness above \
                     was reading a buffer left over from the shot before it",
                    bytes_differing(&again, &empty)
                ),
            );
            wr3.destroy(&mut gpu);
            Ok(())
        })();
        inner
    })();

    // Before the device goes away, and on the error path too — a leaked
    // `Offscreen` is a fistful of `VUID-vkDestroyDevice-device-05137`s, and
    // this gate runs with validation ON.
    off.destroy(&mut gpu);
    r
}

// ── entry point ───────────────────────────────────────────────────────────

pub fn run(args: BordershotArgs) -> Result<(), String> {
    let mode = if args.check { "check" } else { "report" };
    println!(
        "[bordershot] mode: {mode} (serverless; the state, wire and physics \
         layers are CPU-only, the wall layer needs a Vulkan device)"
    );
    let paths = DataPaths::for_version(&args.version)
        .ok_or_else(|| "no config dir for version data".to_string())?;
    let packets = Packets::load(&paths.packets_json())?;
    let ids = Ids::resolve(&packets)?;

    let mut c = Checker {
        witnessed: 0,
        failures: Vec::new(),
    };
    check_state(&mut c);
    check_wire(&mut c, &ids);
    check_physics(&mut c);
    check_pixels(&mut c, &args)?;

    println!(
        "[bordershot] witnesses observed: {} / {EXPECTED_WITNESSES}",
        c.witnessed
    );
    if !c.failures.is_empty() {
        return Err(format!(
            "{} propert{} failed: {}",
            c.failures.len(),
            if c.failures.len() == 1 { "y" } else { "ies" },
            c.failures.join(", ")
        ));
    }
    if c.witnessed != EXPECTED_WITNESSES {
        return Err(format!(
            "bordershot observed {} witnesses, expected {EXPECTED_WITNESSES} — a \
             witness that stops running is a failure, not a quieter pass",
            c.witnessed
        ));
    }
    println!("[bordershot] PASS — {} witnesses", c.witnessed);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The tolerance must be far below the smallest difference `g3` resolves,
    /// or the witness would pass on the wrong tint.
    #[test]
    fn the_three_status_colours_are_further_apart_than_the_tolerance() {
        let ch = |t: u32, s: u32| ((t >> s) & 0xFF) as f32;
        for (a, b) in [
            (GROWING_COLOR, SHRINKING_COLOR),
            (GROWING_COLOR, STATIONARY_COLOR),
            (SHRINKING_COLOR, STATIONARY_COLOR),
        ] {
            let worst = (0..3)
                .map(|i| (ch(a, i * 8) - ch(b, i * 8)).abs())
                .fold(0f32, f32::max);
            assert!(
                worst > 20.0 * CHANNEL_TOL,
                "0x{a:06X} and 0x{b:06X} differ by only {worst}"
            );
        }
    }

    /// And `g4`'s alpha weighting must be resolvable too: half the linear
    /// value is not half the byte, and the gap has to clear the tolerance.
    #[test]
    fn halving_the_alpha_moves_a_channel_further_than_the_tolerance() {
        for byte in [0x20u8, 0xA0, 0xFF] {
            let full = expected_channel(byte, 1.0);
            let half = expected_channel(byte, 0.5);
            // The darkest channel graded (0x20, STATIONARY's red) moves by
            // ~11.5 bytes, which is the tightest case and still five times the
            // tolerance. The bright ones move by four times that.
            assert!(
                (full - half) > 4.0 * CHANNEL_TOL,
                "byte {byte:#04X}: {full} vs {half}"
            );
            // And it is genuinely not the naive halving.
            assert!(
                (half - full / 2.0).abs() > CHANNEL_TOL,
                "byte {byte:#04X}: linear halving {half} is indistinguishable \
                 from byte halving {}",
                full / 2.0
            );
        }
    }

    /// `expected_size` is the gate's own transcription, so it must at least
    /// disagree with the obvious wrong reading (progress counting up).
    #[test]
    fn the_independent_lerp_transcription_is_not_symmetric() {
        assert_eq!(expected_size(0.0, 100.0, 10.0, 10), 0.0);
        assert_eq!(expected_size(0.0, 100.0, 10.0, 5), 50.0);
        assert_eq!(expected_size(0.0, 100.0, 10.0, 0), 100.0);
        assert_ne!(
            expected_size(0.0, 100.0, 10.0, 8),
            expected_size(0.0, 100.0, 10.0, 2)
        );
    }
}
