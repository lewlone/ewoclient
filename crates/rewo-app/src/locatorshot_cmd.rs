//! `rewo locatorshot --check` — the M83 locator-bar oracle.
//!
//! One packet (`waypoint`, id 138) and one HUD strip. `REWO_PACKET_COVERAGE.md`
//! class **B**, and the fourth milestone of that class, so the standard is the
//! one M79/M80/M81 set: an exact vanilla oracle graded line by line, with a
//! pixel read-back half on top of the model half.
//!
//! The path under test, end to end:
//!
//! ```text
//! raw `waypoint` bodies (built here)
//!   -> rewo_net::route_waypoint            (with a REAL `Ids::resolve`d table)
//!   -> rewo_net::waypoints::WaypointStore   (the production map semantics)
//!   -> live_cmd::locator_bar_state          (the SAME resolver the frame calls,
//!                                            through its session-free half)
//!   -> rewo_gpu::locator_bar::markers       (the production placement)
//!   -> WorldRenderer::set_locator_bar -> LocatorBarPass
//!   -> Offscreen::read_rgba                 (real pixels)
//! ```
//!
//! ## The detector
//!
//! Fourteen detector errors on this project have all been one class: measuring
//! a signal against a background that already contains it. The locator bar's
//! dot is 9x9 GUI pixels — small — so the subject here is a **synthetic
//! magenta tint**, delivered through the packet's own `icon.color`, and `p1`
//! asserts that an otherwise identical frame with no bar contains none of it.
//! Nothing else on screen is magenta: the bar's own background is dark green
//! (14,17,16), the dot sprites are white/grey with a black outline, the hearts
//! are red with a low blue, and the hotbar is neutral.
//!
//! Magenta is also what makes `p3` sharp. It asserts not "some magenta
//! appeared" but that **every** magenta pixel in the frame lies inside the 9x9
//! rect the model predicted — a claim the bar itself cannot satisfy.
//!
//! ## Where the samples sit
//!
//! M76 reordered a clamp and its gate stayed green because every sample's step
//! was under the bound; and a rotation witness measured through `Mth.rotLerp`,
//! which normalises into (-180, 180] and erases the sign an off-by-360 would
//! introduce. A bearing-to-screen-x mapping is exactly that shape, so `m2`
//! sweeps the camera yaw through a full turn **including the wrap at ±180**
//! and compares against an independently derived column.
//!
//! ## Fail-closed
//!
//! On a fixed [`EXPECTED_WITNESSES`] count, and on Vulkan validation being
//! active under `--check`.

use clap::Args as ClapArgs;

use rewo_data::assets;
use rewo_data::packets::Packets;
use rewo_data::DataPaths;
use rewo_gpu::locator_bar as lb;
use rewo_gpu::offscreen::Offscreen;
use rewo_gpu::world::{SkyMode, WorldRenderer};
use rewo_gpu::Gpu;
use rewo_net::ids::Ids;
use rewo_net::waypoints::{WaypointContents, WaypointId, WaypointStore};

/// Total named properties this gate asserts. Locked so a skipped property
/// fails the run even when nothing mismatched.
const EXPECTED_WITNESSES: usize = 49;

const W: u32 = 640;
const H: u32 = 480;
/// `gui_scale(640, 480)`, asserted rather than assumed by `p0`.
const SCALE: i32 = 2;
const GUI_W: i32 = W as i32 / SCALE;
const GUI_H: i32 = H as i32 / SCALE;

#[derive(ClapArgs, Debug)]
pub struct LocatorshotArgs {
    /// Label the run and require Vulkan validation. The assertions run either
    /// way; a failure exits non-zero either way.
    #[arg(long)]
    pub check: bool,
    #[arg(long, default_value = "26.2")]
    pub version: String,
    /// Write the measured frames here for eyeballing.
    #[arg(long)]
    pub out_dir: Option<String>,
}

struct Checker {
    witnessed: usize,
    failures: Vec<String>,
}

impl Checker {
    fn record(&mut self, name: &str, pass: bool, detail: impl std::fmt::Display) {
        println!(
            "[locatorshot] {}  {name}: {detail}",
            if pass { " ok " } else { "FAIL" }
        );
        if pass {
            self.witnessed += 1;
        } else {
            self.failures.push(name.to_string());
        }
    }
}

fn client_jar(version: &str) -> Option<std::path::PathBuf> {
    let mut p = dirs::config_dir()?;
    p.push("EwoClient");
    p.push("shared/versions");
    p.push(version);
    p.push(format!("{version}.jar"));
    p.exists().then_some(p)
}

pub fn run(args: LocatorshotArgs) -> Result<(), String> {
    println!(
        "[locatorshot] M83 locator bar — mode: {}",
        if args.check { "check" } else { "report" }
    );
    let paths = DataPaths::for_version(&args.version)
        .ok_or("no config dir for the rewo data root")?;
    let packets = Packets::load(&paths.packets_json())?;
    let ids = Ids::resolve(&packets)?;
    let jar = client_jar(&args.version)
        .ok_or_else(|| format!("no client jar for {} — run `rewo net` first", args.version))?;
    let baked = assets::bake(&jar, &paths.blocks_json())?;

    let mut c = Checker {
        witnessed: 0,
        failures: Vec::new(),
    };
    check_transcription(&mut c);
    check_wire(&mut c, &ids);
    check_model(&mut c);
    check_pixels(&mut c, &args, &baked, &ids)?;

    println!(
        "[locatorshot] witnesses observed: {} / {}",
        c.witnessed, EXPECTED_WITNESSES
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
            "witness count {} != expected {EXPECTED_WITNESSES} — a named \
             property was skipped (fail-closed)",
            c.witnessed
        ));
    }
    println!("[locatorshot] PASS — {} witnesses", c.witnessed);
    Ok(())
}

// ── The decompile, re-transcribed ───────────────────────────────────────────
//
// These re-derive from `LocatorBar.java` / `ContextualBar.java` /
// `WaypointStyle.java` / `Mth.java` rather than importing
// `rewo_gpu::locator_bar`. A witness that asks the implementation what to
// expect asserts only that the implementation equals itself.

/// `ContextualBar.left` / `.top`.
fn ref_bar_origin(gui_w: i32, gui_h: i32) -> (i32, i32) {
    ((gui_w - 182) / 2, gui_h - 24 - 5)
}

/// `Mth.ceil((graphics.guiWidth() - 9) / 2.0F)`.
fn ref_screen_middle(gui_w: i32) -> i32 {
    (((gui_w - 9) as f32) / 2.0).ceil() as i32
}

/// `Mth.floor(angle * 173.0 / 2.0 / 60.0)`.
fn ref_dot_offset(angle: f64) -> i32 {
    (angle * 173.0 / 2.0 / 60.0).floor() as i32
}

/// `!(angle <= -60.0) && !(angle > 60.0)`.
fn ref_visible(angle: f64) -> bool {
    !(angle <= -60.0) && !(angle > 60.0)
}

/// `Mth.wrapDegrees(to - from)`.
fn ref_degrees_difference(from: f32, to: f32) -> f32 {
    let mut a = (to - from) % 360.0;
    if a >= 180.0 {
        a -= 360.0;
    }
    if a < -180.0 {
        a += 360.0;
    }
    a
}

/// `java.util.UUID.hashCode()`.
fn ref_uuid_hash(uuid: u128) -> i32 {
    let hilo = ((uuid >> 64) as u64) ^ (uuid as u64);
    ((hilo >> 32) as u32 as i32) ^ (hilo as u32 as i32)
}

/// `WaypointStyle.sprite(float)`, over the sprite *indices* of a style.
fn ref_sprite(near: i32, far: i32, n: usize, distance: f32) -> usize {
    if distance < near as f32 {
        return 0;
    }
    if distance >= far as f32 {
        return n - 1;
    }
    if n == 1 {
        return 0;
    }
    if n == 3 {
        return 1;
    }
    let alpha = (distance - near as f32) / (far - near) as f32;
    // `Mth.lerpInt(alpha, 1, n - 1)` = `1 + floor(alpha * ((n - 1) - 1))`.
    (1 + (alpha * (n as i32 - 2) as f32).floor() as i32) as usize
}

/// `GameRenderer.projectHorizonToScreen`.
fn ref_horizon(pitch: f32, fov: f32) -> f64 {
    if pitch <= -90.0 {
        return f64::NEG_INFINITY;
    }
    if pitch >= 90.0 {
        return f64::INFINITY;
    }
    (pitch as f64).to_radians().tan() / ((fov / 2.0) as f64).to_radians().tan()
}

fn check_transcription(c: &mut Checker) {
    // ── t1: the two origins, over a sweep of GUI sizes. ────────────────────
    let mut origins_ok = true;
    for gw in [200i32, 319, 320, 321, 640, 1000] {
        for gh in [180i32, 240, 241, 480] {
            let want = ref_bar_origin(gw, gh);
            if (lb::bar_left(gw), lb::bar_top(gh)) != want {
                origins_ok = false;
            }
        }
    }
    c.record(
        "t1.the_bar_origin_is_the_contextual_bars",
        origins_ok,
        "(guiW - 182) / 2 and guiH - 24 - 5 over 24 sizes (MUTATION: centring \
         the bar on `screen_middle` instead moves it a pixel at odd widths)",
    );

    // ── t2: the OTHER centre, which is a float ceil. ───────────────────────
    let mids_ok = (100i32..=1000).all(|gw| lb::screen_middle(gw) == ref_screen_middle(gw));
    c.record(
        "t2.the_dot_column_is_centred_by_a_float_ceil",
        mids_ok && lb::screen_middle(320) == 156 && lb::bar_left(320) == 69,
        format!(
            "ceil((guiW - 9) / 2.0f) over 901 widths; at 320 the strip starts \
             at {} and the dot column is {} (MUTATION: an integer (guiW - 9) / 2 \
             is 155, a pixel left, at every odd (guiW - 9))",
            lb::bar_left(320),
            lb::screen_middle(320)
        ),
    );

    // ── t3: the 173-pixel travel. ──────────────────────────────────────────
    let mut travel_ok = true;
    let mut i = -6000i64;
    while i <= 6000 {
        let a = i as f64 / 100.0;
        if lb::dot_offset(a) != ref_dot_offset(a) {
            travel_ok = false;
        }
        i += 1;
    }
    c.record(
        "t3.the_dot_travels_173_pixels",
        travel_ok
            && lb::dot_offset(60.0) == 86
            && lb::dot_offset(-60.0) == -87
            && lb::DOT_TRAVEL == 173.0,
        format!(
            "±60° -> {}..{} over 12001 samples (MUTATION: 182.0 instead of \
             173.0 gives ±91 and pushes half the dot off each end)",
            lb::dot_offset(-60.0),
            lb::dot_offset(60.0)
        ),
    );

    // ── t4: the half-open window, and the three plausible wrong ones. ──────
    let alts_differ = {
        // `abs() <= 60` accepts -60; `abs() < 60` rejects +60; either rejects NaN.
        let closed = |a: f64| a.abs() <= 60.0;
        let open = |a: f64| a.abs() < 60.0;
        closed(-60.0) != ref_visible(-60.0)
            && open(60.0) != ref_visible(60.0)
            && closed(f64::NAN) != ref_visible(f64::NAN)
    };
    c.record(
        "t4.the_visible_window_is_half_open_and_nan_passing",
        lb::is_visible(60.0)
            && !lb::is_visible(-60.0)
            && lb::is_visible(f64::NAN)
            && !lb::is_visible(60.001)
            && lb::is_visible(-59.999)
            && alts_differ,
        "(-60, 60], and NaN passes both halves (MUTATION: `angle.abs() <= 60.0` \
         accepts -60 exactly and rejects NaN, which hides every EMPTY waypoint)",
    );

    // ── t5: `degreesDifference`, sampled ACROSS the wrap. ──────────────────
    let mut diff_ok = true;
    for f in (-360i32..=360).step_by(7) {
        for t in (-360i32..=360).step_by(11) {
            let (f, t) = (f as f32, t as f32);
            if (lb::degrees_difference(f, t) - ref_degrees_difference(f, t)).abs() > 1e-3 {
                diff_ok = false;
            }
        }
    }
    c.record(
        "t5.degrees_difference_wraps_into_a_half_open_turn",
        diff_ok && lb::degrees_difference(179.0, -179.0) == 2.0,
        format!(
            "179 -> -179 is +{}° over 6000 pairs spanning the wrap (MUTATION: a \
             bare `to - from` gives -358 and throws the dot to the far end)",
            lb::degrees_difference(179.0, -179.0)
        ),
    );

    // ── t6: `Mth.atan2`, and what its approximation costs in PIXELS. ───────
    let (mut worst_rad, mut worst_px) = (0.0f64, 0i32);
    for i in 0..36000 {
        let t = i as f64 * 0.01 * std::f64::consts::PI / 180.0;
        let (y, x) = (t.sin() * 128.0, t.cos() * 128.0);
        let m = lb::mth_atan2(y, x);
        let p = y.atan2(x);
        worst_rad = worst_rad.max((m - p).abs());
        let d = (lb::dot_offset(m.to_degrees()) - lb::dot_offset(p.to_degrees())).abs();
        worst_px = worst_px.max(d);
    }
    c.record(
        "t6.mth_atan2_is_transcribed_and_its_error_is_measured",
        worst_rad < 1e-5,
        format!(
            "worst |Mth.atan2 - platform| over 36000 bearings: {worst_rad:.3e} rad, \
             which moves the dot by at most {worst_px} px (MUTATION: substituting \
             f64::atan2 is a real divergence, and this is how big it is — the \
             number is the finding, not the pass)"
        ),
    );

    // ── t7: the hash fallback. ─────────────────────────────────────────────
    // The fixture has to be one where `^` and `+` DISAGREE. The first version
    // used a UUID whose low half was zero, for which the two are identical —
    // the mutation survived, which is the "a sample must sit where the mutation
    // bites" failure, not a property that cannot fail.
    const UUID_FIXTURE: u128 = 0x1234_5678_9ABC_DEF0_1111_2222_3333_4444;
    let (msb, lsb) = ((UUID_FIXTURE >> 64) as u64, UUID_FIXTURE as u64);
    c.record(
        "t7.the_colour_fallback_hashes_the_identifier_javas_way",
        lb::java_uuid_hash(0) == 0
            && lb::java_uuid_hash(UUID_FIXTURE) == ref_uuid_hash(UUID_FIXTURE)
            && (msb ^ lsb) != msb.wrapping_add(lsb)
            && lb::java_string_hash("") == 0
            && lb::java_string_hash("Notch") == 75_456_088
            && lb::java_string_hash("abc") == 96354,
        format!(
            "UUID.hashCode() = {:#x} on a fixture where xor and add differ \
             ({:#x} vs {:#x}); \"Notch\".hashCode() = {}, \"abc\" = {} (MUTATION: \
             hashing the two halves with `+` gives a different colour for every \
             player and nothing on screen says so)",
            lb::java_uuid_hash(UUID_FIXTURE),
            msb ^ lsb,
            msb.wrapping_add(lsb),
            lb::java_string_hash("Notch"),
            lb::java_string_hash("abc")
        ),
    );

    // ── t8: `setBrightness`, against a hand-derived HSV round trip. ────────
    let bright_ok = {
        // Pure hues: the round trip must keep the hue and take the value.
        let red = lb::argb_set_brightness(0xFF80_0000, 0.9);
        let green = lb::argb_set_brightness(0xFF00_4000, 0.9);
        let blue = lb::argb_set_brightness(0xFF00_0020, 0.9);
        red == 0xFFE6_0000 && green == 0xFF00_E600 && blue == 0xFF00_00E6
    };
    // The hazard, computed here rather than asserted about: `0.9f * 255.0f`
    // rounds UP to exactly 229.5f in f32, but widening the operands first
    // gives 229.49999392. `Math.round` is `floor(x + 0.5)`, so the two land on
    // opposite sides of the half.
    let prod_f32 = 0.9f32 * 255.0f32;
    let prod_widened = 0.9f32 as f64 * 255.0f64;
    let f32_rounds_up = (prod_f32 + 0.5).floor() as i32 == 230;
    let widened_rounds_down = (prod_widened + 0.5).floor() as i32 == 229;
    c.record(
        "t8.set_brightness_keeps_hue_and_replaces_value",
        bright_ok
            && lb::argb_set_brightness(0xFF00_0000, 0.9) == 0xFFE6_E6E6
            && f32_rounds_up
            && widened_rounds_down,
        format!(
            "a dim pure red/green/blue all come back at 0xE6; `0.9f * 255.0f` is \
             EXACTLY {prod_f32} in f32 and `Math.round`'s floor(x+0.5) lands on \
             230, where widening the operands first gives {prod_widened:.8} and \
             lands on 229 (MUTATION: truncating instead of rounding gives 229 \
             too — the two errors are indistinguishable from the output alone, \
             which is why the arithmetic is pinned rather than the result)"
        ),
    );

    // ── t9: the style bands. ───────────────────────────────────────────────
    let style = lb::WaypointStyle {
        key: "t".into(),
        near_distance: 128,
        far_distance: 332,
        sprites: vec![0, 1, 2, 3],
    };
    let mut bands_ok = true;
    for d in 0..40000 {
        let d = d as f32 / 100.0;
        if style.sprite(d) as usize != ref_sprite(128, 332, 4, d) {
            bands_ok = false;
        }
    }
    c.record(
        "t9.the_style_bands_are_lerp_int_not_a_uniform_split",
        bands_ok && style.sprite(331.9) == 2 && style.sprite(332.0) == 3,
        "just inside farDistance the band is still 2; index 3 is reachable only \
         through the `>= far` early return, because `lerpInt(a, 1, n-1)` is \
         `1 + floor(a * (n-2))` (MUTATION: reading lerpInt's p1 as inclusive \
         makes the last band one sprite wide and unreachable)",
    );

    // ── t10: the constants. ────────────────────────────────────────────────
    c.record(
        "t10.the_constants_are_the_decompiles",
        lb::BAR_W == 182
            && lb::BAR_H == 5
            && lb::MARGIN_BOTTOM == 24
            && lb::DOT_SIZE == 9
            && lb::VISIBLE_DEGREE_RANGE == 60.0
            && lb::ARROW_W == 7
            && lb::ARROW_H == 5
            && lb::ARROW_LEFT == 1
            && lb::ARROW_PADDING == 1
            && lb::DEFAULT_NEAR_DISTANCE == 128
            && lb::DEFAULT_FAR_DISTANCE == 332,
        "WIDTH 182, HEIGHT 5, MARGIN_BOTTOM 24, DOT_SIZE 9, VISIBLE_DEGREE_RANGE \
         60, ARROW 7x5 +1 +1, near/far 128/332 (MUTATION: any one of them is a \
         silent layout shift)",
    );

    // ── t11: the nine-slice expansion. ─────────────────────────────────────
    let mut src = vec![0u8; 12 * 5 * 4];
    for y in 0..5usize {
        for x in 0..12usize {
            src[(y * 12 + x) * 4] = x as u8;
            src[(y * 12 + x) * 4 + 3] = 255;
        }
    }
    let out = lb::expand_nine_slice(
        &rewo_gpu::hud::HudSpriteData {
            rgba: &src,
            w: 12,
            h: 5,
        },
        182,
    );
    let col = |x: usize| out[x * 4];
    let tiled = (0..172usize).all(|i| col(5 + i) == (5 + i % 2) as u8);
    let borders = (0..5usize).all(|i| col(i) == i as u8)
        && (0..5usize).all(|i| col(182 - 5 + i) == (7 + i) as u8);
    c.record(
        "t11.the_background_is_a_tiled_nine_slice_not_a_182_wide_sprite",
        tiled && borders && out.len() == 182 * 5 * 4,
        "locator_bar_background.png is 12x5 with border 5/5 and no stretch_inner, \
         so blitSprite(182, 5) takes the horizontal three-slice and TILES the \
         2-px middle 86 times (MUTATION: stretch_inner smears a 2-px pattern \
         across 172, and blitting the 12-wide sprite at 182 draws one squashed copy)",
    );

    // ── t12: the arrow's two-frame animation. ──────────────────────────────
    let frames: Vec<usize> = (0..28).map(lb::arrow_frame).collect();
    let want: Vec<usize> = (0..28)
        .map(|t: i64| if t % 14 < 10 { 0 } else { 1 })
        .collect();
    c.record(
        "t12.the_arrow_is_a_two_frame_animation_on_a_14_tick_cycle",
        frames == want && lb::arrow_frame(-1) == 1,
        "locator_bar_arrow_up.png is 7x10 — two 7x5 frames, index 0 for 10 ticks \
         then index 1 for 4 (MUTATION: treating the file as one 7x10 sprite blits \
         both frames at once, and reading only the top one never blinks)",
    );

    // ── t13: the horizon. ──────────────────────────────────────────────────
    let mut hz_ok = true;
    for p in -890..=890 {
        let p = p as f32 / 10.0;
        let a = lb::project_horizon_to_screen(p, 70.0);
        let b = ref_horizon(p, 70.0);
        if (a - b).abs() > 1e-9 {
            hz_ok = false;
        }
    }
    c.record(
        "t13.the_horizon_is_tan_pitch_over_tan_half_fov",
        hz_ok
            && lb::project_horizon_to_screen(90.0, 70.0) == f64::INFINITY
            && lb::project_horizon_to_screen(-90.0, 70.0) == f64::NEG_INFINITY
            && lb::project_horizon_to_screen(60.0, 70.0) > 1.0,
        "positive xRot is looking DOWN, which puts the horizon ABOVE the screen \
         and reads as UP (MUTATION: negating the pitch swaps every arrow)",
    );

    // ── t14: which bar owns the contextual slot. ───────────────────────────
    c.record(
        "t14.the_xp_bar_outranks_the_locator_bar_for_100_ticks",
        lb::contextual_bar(true, false, false)
            && lb::contextual_bar(true, true, false)
            && !lb::contextual_bar(true, true, true)
            && !lb::contextual_bar(false, true, true)
            && !lb::contextual_bar(false, false, false),
        "hasWaypoints does NOT win — `willPrioritizeExperienceInfo` takes the slot \
         for 100 ticks after every XP change (MUTATION: returning `has_waypoints` \
         alone pins the strip on and the XP bar never reappears)",
    );
}

// ── Wire-body builders ──────────────────────────────────────────────────────

fn varint(v: i32, out: &mut Vec<u8>) {
    let mut v = v as u32;
    loop {
        if v & !0x7F == 0 {
            out.push(v as u8);
            return;
        }
        out.push((v as u8 & 0x7F) | 0x80);
        v >>= 7;
    }
}

fn utf(s: &str, out: &mut Vec<u8>) {
    varint(s.len() as i32, out);
    out.extend_from_slice(s.as_bytes());
}

/// operation + `Either` + icon, up to the type tag.
fn head(op: i32, id: &WaypointId, style: &str, color: Option<(u8, u8, u8)>) -> Vec<u8> {
    let mut b = Vec::new();
    varint(op, &mut b);
    match id {
        WaypointId::Uuid(u) => {
            b.push(1);
            b.extend_from_slice(&u.to_be_bytes());
        }
        WaypointId::Name(n) => {
            b.push(0);
            utf(n, &mut b);
        }
    }
    utf(style, &mut b);
    match color {
        Some((r, g, bl)) => {
            b.push(1);
            b.extend_from_slice(&[r, g, bl]);
        }
        None => b.push(0),
    }
    b
}

fn body(
    op: i32,
    id: &WaypointId,
    style: &str,
    color: Option<(u8, u8, u8)>,
    contents: WaypointContents,
) -> Vec<u8> {
    let mut b = head(op, id, style, color);
    varint(contents.type_id(), &mut b);
    match contents {
        WaypointContents::Empty => {}
        WaypointContents::Vec3i { x, y, z } => {
            varint(x, &mut b);
            varint(y, &mut b);
            varint(z, &mut b);
        }
        WaypointContents::Chunk { x, z } => {
            varint(x, &mut b);
            varint(z, &mut b);
        }
        WaypointContents::Azimuth { radians } => b.extend_from_slice(&radians.to_be_bytes()),
    }
    b
}

fn check_wire(c: &mut Checker, ids: &Ids) {
    // ── w1: the id resolves BY NAME from the report. ───────────────────────
    c.record(
        "w1.the_packet_id_resolves_by_name",
        ids.cb_play_waypoint == 138,
        format!(
            "clientbound-play `waypoint` -> {} (MUTATION: hard-coding 138 would \
             survive a renumbered protocol and misfire on whatever took the slot)",
            ids.cb_play_waypoint
        ),
    );

    // ── w2: the router matches that id and nothing else. ───────────────────
    let mut s = WaypointStore::default();
    let one = body(
        0,
        &WaypointId::Uuid(1),
        "minecraft:default",
        None,
        WaypointContents::Vec3i {
            x: 0,
            y: 64,
            z: 10,
        },
    );
    let matched = rewo_net::route_waypoint(ids.cb_play_waypoint, &one, ids, &mut s);
    let neighbours = [
        ids.cb_play_set_experience,
        ids.cb_play_cooldown,
        ids.cb_play_clear_titles,
    ]
    .iter()
    .any(|&id| rewo_net::route_waypoint(id, &one, ids, &mut s));
    c.record(
        "w2.the_router_matches_138_and_nothing_else",
        matched && !neighbours && s.len() == 1,
        "route_waypoint returns whether the ID matched, not whether the body \
         decoded (MUTATION: an arm that swallowed every id would eat the seven \
         M79 packets below it in the ladder)",
    );

    // ── w3: all four body shapes decode. ───────────────────────────────────
    let mut s = WaypointStore::default();
    let shapes = [
        (
            WaypointId::Uuid(10),
            WaypointContents::Empty,
        ),
        (
            WaypointId::Uuid(11),
            WaypointContents::Vec3i {
                x: -5,
                y: 70,
                z: 300,
            },
        ),
        (
            WaypointId::Uuid(12),
            WaypointContents::Chunk { x: -3, z: 9 },
        ),
        (
            WaypointId::Uuid(13),
            WaypointContents::Azimuth { radians: 1.25 },
        ),
    ];
    for (id, contents) in &shapes {
        rewo_net::route_waypoint(
            ids.cb_play_waypoint,
            &body(0, id, "minecraft:default", None, *contents),
            ids,
            &mut s,
        );
    }
    let all_four = shapes
        .iter()
        .all(|(id, contents)| s.get(id).map(|w| w.contents) == Some(*contents));
    c.record(
        "w3.every_body_shape_decodes",
        all_four && s.len() == 4,
        "EMPTY carries no body at all, VEC3I three var-ints, CHUNK two (x and z, \
         no y), AZIMUTH one big-endian f32 (MUTATION: giving CHUNK a y reads the \
         next packet's first byte as its z and desyncs the stream)",
    );

    // ── w4: the Either flag. ───────────────────────────────────────────────
    let mut s = WaypointStore::default();
    rewo_net::route_waypoint(
        ids.cb_play_waypoint,
        &body(
            0,
            &WaypointId::Name("north".into()),
            "minecraft:default",
            None,
            WaypointContents::Empty,
        ),
        ids,
        &mut s,
    );
    rewo_net::route_waypoint(
        ids.cb_play_waypoint,
        &body(
            0,
            &WaypointId::Uuid(0x0123_4567_89AB_CDEF_0123_4567_89AB_CDEF),
            "minecraft:default",
            None,
            WaypointContents::Empty,
        ),
        ids,
        &mut s,
    );
    c.record(
        "w4.the_identifier_flag_is_true_for_the_uuid",
        s.get(&WaypointId::Name("north".into())).is_some()
            && s.get(&WaypointId::Uuid(0x0123_4567_89AB_CDEF_0123_4567_89AB_CDEF))
                .is_some()
            && s.len() == 2,
        "`FriendlyByteBuf.writeEither` writes TRUE for the LEFT alternative, and \
         the left is the UUID (MUTATION: reading true as the string form takes a \
         length prefix out of the UUID's first byte)",
    );

    // ── w5: the colour is three raw bytes and opaque. ──────────────────────
    let mut s = WaypointStore::default();
    rewo_net::route_waypoint(
        ids.cb_play_waypoint,
        &body(
            0,
            &WaypointId::Uuid(20),
            "minecraft:default",
            Some((0x12, 0x34, 0x56)),
            WaypointContents::Empty,
        ),
        ids,
        &mut s,
    );
    rewo_net::route_waypoint(
        ids.cb_play_waypoint,
        &body(
            0,
            &WaypointId::Uuid(21),
            "minecraft:default",
            None,
            WaypointContents::Empty,
        ),
        ids,
        &mut s,
    );
    c.record(
        "w5.the_colour_is_three_raw_bytes_and_absent_is_not_black",
        s.get(&WaypointId::Uuid(20)).unwrap().icon.color == Some(0xFF12_3456)
            && s.get(&WaypointId::Uuid(21)).unwrap().icon.color.is_none(),
        "`ByteBufCodecs.RGB_COLOR` is r/g/b bytes through `ARGB.color(r,g,b)`, \
         which is the THREE-argument overload and forces alpha 255 (MUTATION: an \
         `Option<u32>` defaulted to 0 paints every uncoloured waypoint black \
         instead of hashing its identifier)",
    );

    // ── w6: the operation WRAPS. ───────────────────────────────────────────
    let mut s = WaypointStore::default();
    let mk = |op: i32, id: u128| {
        body(
            op,
            &WaypointId::Uuid(id),
            "minecraft:default",
            None,
            WaypointContents::Empty,
        )
    };
    // 3 wraps to TRACK.
    rewo_net::route_waypoint(ids.cb_play_waypoint, &mk(3, 30), ids, &mut s);
    let tracked_by_3 = s.len() == 1;
    // 4 wraps to UNTRACK.
    rewo_net::route_waypoint(ids.cb_play_waypoint, &mk(4, 30), ids, &mut s);
    let untracked_by_4 = s.is_empty();
    // -1 wraps to UPDATE, which for an absent id is inert.
    rewo_net::route_waypoint(ids.cb_play_waypoint, &mk(-1, 31), ids, &mut s);
    let update_by_minus_one = s.is_empty();
    // i32::MIN floorMods to 1 = UNTRACK, so it must also not insert.
    rewo_net::route_waypoint(ids.cb_play_waypoint, &mk(i32::MIN, 32), ids, &mut s);
    c.record(
        "w6.the_operation_wraps_and_rejects_nothing",
        tracked_by_3 && untracked_by_4 && update_by_minus_one && s.is_empty(),
        "`ByIdMap.continuous(WRAP)` is `Math.floorMod(id, 3)`: 3 -> TRACK, \
         4 -> UNTRACK, -1 -> UPDATE, Integer.MIN_VALUE -> UNTRACK (MUTATION: \
         Rust's `%` gives -1 for -1 and panics on the index; a `readEnum`-style \
         array index would reject 3, which vanilla accepts)",
    );

    // ── w7: the TYPE tag, one field later, is rejected out of range. ───────
    let mut s = WaypointStore::default();
    let mut bad = head(0, &WaypointId::Uuid(40), "minecraft:default", None);
    varint(4, &mut bad);
    let bad_matched = rewo_net::route_waypoint(ids.cb_play_waypoint, &bad, ids, &mut s);
    let bad_empty = s.is_empty();
    let mut ok = head(0, &WaypointId::Uuid(40), "minecraft:default", None);
    varint(3, &mut ok);
    ok.extend_from_slice(&0.5f32.to_be_bytes());
    rewo_net::route_waypoint(ids.cb_play_waypoint, &ok, ids, &mut s);
    c.record(
        "w7.the_type_tag_is_the_enum_that_rejects",
        bad_matched && bad_empty && s.len() == 1,
        "`byteBuf.readEnum` is `getEnumConstants()[readVarInt()]` — a bare array \
         index that THROWS out of range, one field after an enum that wraps \
         (MUTATION: wrapping the type too turns tag 4 into EMPTY and silently \
         drops a body the stream still contains)",
    );

    // ── w8: UPDATE writes only the position. ───────────────────────────────
    let mut s = WaypointStore::default();
    rewo_net::route_waypoint(
        ids.cb_play_waypoint,
        &body(
            0,
            &WaypointId::Uuid(50),
            "minecraft:default",
            None,
            WaypointContents::Vec3i {
                x: 10,
                y: 64,
                z: 10,
            },
        ),
        ids,
        &mut s,
    );
    rewo_net::route_waypoint(
        ids.cb_play_waypoint,
        &body(
            2,
            &WaypointId::Uuid(50),
            "minecraft:bowtie",
            Some((255, 0, 0)),
            WaypointContents::Vec3i {
                x: 11,
                y: 65,
                z: 12,
            },
        ),
        ids,
        &mut s,
    );
    let w = s.get(&WaypointId::Uuid(50)).unwrap().clone();
    c.record(
        "w8.update_moves_the_waypoint_and_cannot_restyle_it",
        w.contents
            == WaypointContents::Vec3i {
                x: 11,
                y: 65,
                z: 12,
            }
            && w.icon.style == "minecraft:default"
            && w.icon.color.is_none(),
        "`updateWaypoint` is `get(id).update(other)` and each override assigns \
         only its own position field; icon and type are final (MUTATION: reusing \
         the TRACK path for UPDATE applies all three, so a colour set once by \
         /waypoint modify would be undone by the next position update)",
    );

    // ── w9: a cross-type UPDATE changes nothing at all. ────────────────────
    rewo_net::route_waypoint(
        ids.cb_play_waypoint,
        &body(
            2,
            &WaypointId::Uuid(50),
            "minecraft:default",
            None,
            WaypointContents::Chunk { x: 0, z: 0 },
        ),
        ids,
        &mut s,
    );
    c.record(
        "w9.a_cross_type_update_is_refused_whole",
        s.get(&WaypointId::Uuid(50)).unwrap().contents
            == WaypointContents::Vec3i {
                x: 11,
                y: 65,
                z: 12,
            },
        "the `instanceof` in each override fails and vanilla logs \
         \"Unsupported Waypoint update operation\" (MUTATION: assigning anyway \
         teleports a tracked player to chunk 0,0 whenever the server switches \
         them between the position and chunk tiers)",
    );

    // ── w10: UNTRACK reads only the identifier. ────────────────────────────
    rewo_net::route_waypoint(
        ids.cb_play_waypoint,
        &body(
            1,
            &WaypointId::Uuid(50),
            "minecraft:default",
            None,
            // `removeWaypoint` builds an EMPTY waypoint — the body of an
            // untrack says nothing about what is being removed.
            WaypointContents::Empty,
        ),
        ids,
        &mut s,
    );
    c.record(
        "w10.untrack_removes_by_identifier_alone",
        s.is_empty(),
        "`removeWaypoint(uuid)` sends an EMPTY waypoint with the NULL icon \
         (MUTATION: matching on the contents too would leave every real waypoint \
         tracked forever, because none of them is EMPTY)",
    );

    // ── w11: a truncated body writes nothing. ──────────────────────────────
    let full = body(
        0,
        &WaypointId::Uuid(60),
        "minecraft:default",
        Some((1, 2, 3)),
        WaypointContents::Vec3i {
            x: 100,
            y: 64,
            z: 100,
        },
    );
    let mut clean = true;
    for cut in 0..full.len() {
        let mut s = WaypointStore::default();
        rewo_net::route_waypoint(ids.cb_play_waypoint, &full[..cut], ids, &mut s);
        if !s.is_empty() {
            clean = false;
        }
    }
    let mut s = WaypointStore::default();
    rewo_net::route_waypoint(ids.cb_play_waypoint, &full, ids, &mut s);
    c.record(
        "w11.a_short_body_leaves_the_map_untouched",
        clean && s.len() == 1,
        format!(
            "every one of the {} prefixes decodes to nothing, and the whole body \
             to one waypoint (MUTATION: applying a partially-decoded waypoint \
             would insert one with a zeroed position at the world origin)",
            full.len()
        ),
    );

    // ── w12: the two identifier forms are different keys. ──────────────────
    let mut s = WaypointStore::default();
    rewo_net::route_waypoint(
        ids.cb_play_waypoint,
        &body(
            0,
            &WaypointId::Uuid(1),
            "minecraft:default",
            None,
            WaypointContents::Empty,
        ),
        ids,
        &mut s,
    );
    rewo_net::route_waypoint(
        ids.cb_play_waypoint,
        &body(
            0,
            &WaypointId::Name("1".into()),
            "minecraft:default",
            None,
            WaypointContents::Empty,
        ),
        ids,
        &mut s,
    );
    c.record(
        "w12.a_uuid_key_and_a_string_key_are_distinct",
        s.len() == 2,
        "the map is keyed by the `Either` itself (MUTATION: flattening both to a \
         string would let a named waypoint untrack a player's)",
    );

    // ── w13: azimuth is radians on the wire. ───────────────────────────────
    let mut s = WaypointStore::default();
    rewo_net::route_waypoint(
        ids.cb_play_waypoint,
        &body(
            0,
            &WaypointId::Uuid(70),
            "minecraft:default",
            None,
            WaypointContents::Azimuth {
                radians: std::f32::consts::FRAC_PI_4,
            },
        ),
        ids,
        &mut s,
    );
    let cam = test_camera();
    let angle = match s.get(&WaypointId::Uuid(70)).unwrap().contents {
        WaypointContents::Azimuth { radians } => {
            lb::yaw_angle_to_camera(&lb::WaypointSubject::Azimuth { radians }, &cam)
        }
        _ => f64::NAN,
    };
    c.record(
        "w13.the_azimuth_body_is_radians",
        (angle - 45.0).abs() < 1e-3,
        format!(
            "pi/4 on the wire reads {angle:.3}° (MUTATION: taking it as degrees \
             gives 0.785°, which pins every really-far player within a degree of \
             the crosshair — and a bar full of dots at the centre looks like a bug \
             in the sort, not in the units)"
        ),
    );
}

// ── Model ───────────────────────────────────────────────────────────────────

fn test_camera() -> lb::LocatorCamera {
    lb::LocatorCamera {
        yaw: 0.0,
        pitch: 0.0,
        fov: 70.0,
        // A block centre, so a subject's own `atCenterOf` offset cancels.
        camera_pos: [0.5, 64.0, 0.5],
        entity_pos: [0.5, 62.38, 0.5],
        near: 0.05,
        far: 1024.0,
    }
}

fn default_styles() -> Vec<lb::WaypointStyle> {
    vec![lb::WaypointStyle {
        key: "minecraft:default".into(),
        near_distance: 128,
        far_distance: 332,
        sprites: vec![0, 1, 2, 3],
    }]
}

fn wp(subject: lb::WaypointSubject, color: u32) -> lb::LocatorWaypoint {
    lb::LocatorWaypoint {
        subject,
        color,
        style: 0,
        is_camera_entity: false,
    }
}

fn check_model(c: &mut Checker) {
    let styles = default_styles();
    let cam = test_camera();

    // ── m1: the bearing's sign convention. ─────────────────────────────────
    let north = lb::yaw_angle_to_camera(
        &lb::WaypointSubject::Vec3i {
            x: 0,
            y: 64,
            z: 100,
            entity_eye: None,
        },
        &cam,
    );
    let east = lb::yaw_angle_to_camera(
        &lb::WaypointSubject::Vec3i {
            x: 100,
            y: 64,
            z: 0,
            entity_eye: None,
        },
        &cam,
    );
    c.record(
        "m1.the_bearing_follows_minecrafts_yaw_convention",
        north.abs() < 0.01 && (east + 90.0).abs() < 0.01,
        format!(
            "facing +Z: a subject at +Z reads {north:.3}° and one at +X reads \
             {east:.3}° — NEGATIVE, because yaw grows from +Z toward -X, so east \
             is on your left (MUTATION: dropping `rotateClockwise90` rotates every \
             dot a quarter turn and a subject dead ahead sits at the strip's end)"
        ),
    );

    // ── m2: the bearing→column mapping, sampled ACROSS THE WRAP. ───────────
    //
    // The failure this is shaped against: a sample whose step never crosses a
    // boundary proves nothing. So the camera turns a full circle in 1° steps
    // and every visible frame's column is checked against an independent
    // derivation — including the two frames either side of ±180.
    let mut mapping_ok = true;
    let mut seen_wrap = false;
    let mut visible = 0usize;
    for yaw_i in -360i32..=360 {
        let mut cam = test_camera();
        cam.yaw = yaw_i as f32;
        let subject = lb::WaypointSubject::Vec3i {
            x: 0,
            y: 64,
            z: 100,
            entity_eye: None,
        };
        // Independent: the subject is due +Z, so its world bearing is 0 and
        // the camera-relative one is `degreesDifference(yaw, 0)`.
        let want_angle = ref_degrees_difference(yaw_i as f32, 0.0) as f64;
        let m = lb::markers(&[wp(subject, 0xFFFF_FFFF)], &styles, &cam, GUI_W, GUI_H);
        if ref_visible(want_angle) {
            visible += 1;
            let want_x = ref_screen_middle(GUI_W) + ref_dot_offset(want_angle);
            if m.len() != 1 || m[0].x != want_x {
                mapping_ok = false;
            }
        } else if !m.is_empty() {
            mapping_ok = false;
        }
        if yaw_i.abs() >= 179 && yaw_i.abs() <= 181 {
            seen_wrap = true;
        }
    }
    c.record(
        "m2.the_bearing_maps_to_a_column_across_the_whole_turn",
        mapping_ok && seen_wrap && visible == 241,
        format!(
            "721 camera yaws over a full turn, {visible} of them inside the window, \
             each column matched against an independent derivation, and the sweep \
             crosses ±180 (MUTATION: a mapping that dropped `wrapDegrees` agrees \
             everywhere except across the wrap, which is exactly where a sample \
             that never turns past 180° cannot see it)"
        ),
    );

    // ── m3: the window's two edges, exactly. ───────────────────────────────
    let edge = |angle: f64| {
        let mut cam = test_camera();
        // Turn the camera so the +Z subject lands at `angle`.
        cam.yaw = -angle as f32;
        !lb::markers(
            &[wp(
                lb::WaypointSubject::Azimuth { radians: 0.0 },
                0xFFFF_FFFF,
            )],
            &styles,
            &cam,
            GUI_W,
            GUI_H,
        )
        .is_empty()
    };
    c.record(
        "m3.the_window_admits_plus_sixty_and_refuses_minus_sixty",
        edge(60.0) && !edge(-60.0),
        "`!(angle <= -60) && !(angle > 60)` (MUTATION: `abs() <= 60` admits both \
         and `abs() < 60` refuses both; the asymmetry is only visible at the two \
         exact boundaries)",
    );

    // ── m4: EMPTY. ─────────────────────────────────────────────────────────
    let m = lb::markers(
        &[wp(lb::WaypointSubject::Empty, 0xFFFF_FFFF)],
        &styles,
        &cam,
        GUI_W,
        GUI_H,
    );
    c.record(
        "m4.an_empty_waypoint_draws_at_dead_centre_with_the_far_sprite",
        m.len() == 1 && m[0].x == ref_screen_middle(GUI_W) && m[0].sprite == 3,
        format!(
            "NaN bearing passes both halves of the guard, `Mth.floor(NaN)` is 0, \
             and its +Infinity distance takes the `>= farDistance` branch to the \
             LAST sprite (MUTATION: filtering NaN hides it entirely; treating \
             +Infinity as an error picks sprite {})",
            m.first().map(|x| x.sprite).unwrap_or(99)
        ),
    );

    // ── m5: draw order. ────────────────────────────────────────────────────
    let near = wp(
        lb::WaypointSubject::Vec3i {
            x: 0,
            y: 64,
            z: 10,
            entity_eye: None,
        },
        0xFF00_FF00,
    );
    let far = wp(
        lb::WaypointSubject::Vec3i {
            x: 0,
            y: 64,
            z: 500,
            entity_eye: None,
        },
        0xFFFF_0000,
    );
    let m = lb::markers(&[near, far], &styles, &cam, GUI_W, GUI_H);
    c.record(
        "m5.the_farthest_waypoint_is_emitted_first",
        m.len() == 2 && m[0].color == 0xFFFF_0000 && m[1].color == 0xFF00_FF00,
        "`Comparator.comparingDouble(distanceSquared).reversed()`, so the nearest \
         dot lands ON TOP (MUTATION: dropping `.reversed()` buries the player \
         standing next to you under everyone else on the server)",
    );

    // ── m6: the self-skip — REWO_PLAN §0.0 gotcha 13. ──────────────────────
    let mut me = wp(
        lb::WaypointSubject::Vec3i {
            x: 0,
            y: 64,
            z: 10,
            entity_eye: None,
        },
        0xFFFF_FFFF,
    );
    let drawn_before = lb::markers(std::slice::from_ref(&me), &styles, &cam, GUI_W, GUI_H).len();
    me.is_camera_entity = true;
    let drawn_after = lb::markers(&[me], &styles, &cam, GUI_W, GUI_H).len();
    c.record(
        "m6.the_camera_entitys_own_waypoint_is_skipped",
        drawn_before == 1 && drawn_after == 0,
        "`id.left().map(u -> u.equals(cameraEntity.getUUID()))` — and the camera \
         entity is the one thing `EntityTable` NEVER holds, so this flag cannot \
         be derived from the table (MUTATION: dropping the check looks correct on \
         a vanilla server, which never sends you your own waypoint, and paints a \
         permanent dot the moment a datapack does)",
    );

    // ── m7: the sprite tracks distance. ────────────────────────────────────
    let sprite_at = |z: i32| {
        lb::markers(
            &[wp(
                lb::WaypointSubject::Vec3i {
                    x: 0,
                    y: 62,
                    z,
                    entity_eye: None,
                },
                0xFFFF_FFFF,
            )],
            &styles,
            &cam,
            GUI_W,
            GUI_H,
        )[0]
        .sprite
    };
    c.record(
        "m7.the_sprite_shrinks_with_distance",
        sprite_at(10) == 0 && sprite_at(200) == 1 && sprite_at(400) == 3,
        format!(
            "10 / 200 / 400 blocks -> sprites {} / {} / {} (MUTATION: taking the \
             sqrt in f64 instead of `Mth.sqrt((float) d2)` moves the band edges by \
             a fraction of a block, which no screenshot could show)",
            sprite_at(10),
            sprite_at(200),
            sprite_at(400)
        ),
    );

    // ── m8: two points measure two quantities. ─────────────────────────────
    let mut shifted = test_camera();
    // Move the camera's EYE 60 blocks east while leaving the entity where it
    // is: the bearing must move and the sprite must not.
    //
    // The subject sits at 229 blocks from the feet, which is `alpha = 0.495`
    // through the 128..332 band — a hair below the 1→2 edge. The 60-block eye
    // shift raises the eye-measured distance to 236.7, `alpha = 0.533`, which
    // is over it. So the *sprite* is the observable, and the sample is placed
    // where a swap of the two points changes it. The first version used a
    // 40-block shift at 200 blocks, where both distances land in the same band
    // and the mutation survived — the M76 lesson, made concrete.
    shifted.camera_pos[0] += 60.0;
    let subject = lb::WaypointSubject::Vec3i {
        x: 0,
        y: 62,
        z: 229,
        entity_eye: None,
    };
    let base = lb::markers(&[wp(subject, 0xFFFF_FFFF)], &styles, &cam, GUI_W, GUI_H);
    let moved = lb::markers(&[wp(subject, 0xFFFF_FFFF)], &styles, &shifted, GUI_W, GUI_H);
    // The band edge the sample straddles, derived independently.
    let feet_d = lb::distance_squared(&subject, &cam).sqrt() as f32;
    let eye_d = {
        let p = [0.5, 62.5, 229.5];
        let d = [
            shifted.camera_pos[0] - p[0],
            shifted.camera_pos[1] - p[1],
            shifted.camera_pos[2] - p[2],
        ];
        (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt() as f32
    };
    c.record(
        "m8.the_bearing_and_the_distance_read_different_points",
        base[0].x != moved[0].x
            && base[0].sprite == moved[0].sprite
            && ref_sprite(128, 332, 4, feet_d) != ref_sprite(128, 332, 4, eye_d),
        format!(
            "the bearing is from `camera.position()` (the eye) and the distance \
             from `cameraEntity.position()` (the feet): {feet_d:.1} blocks -> \
             sprite {}, and the eye's {eye_d:.1} would be sprite {} (MUTATION: \
             using one point for both changes the sprite here, and would be \
             invisible anywhere the two land in the same band)",
            ref_sprite(128, 332, 4, feet_d),
            ref_sprite(128, 332, 4, eye_d)
        ),
    );

    // ── m9: the entity substitution and its Manhattan guard. ───────────────
    let raw = lb::WaypointSubject::Vec3i {
        x: 0,
        y: 62,
        z: 100,
        entity_eye: None,
    };
    let with_eye = lb::WaypointSubject::Vec3i {
        x: 0,
        y: 62,
        z: 100,
        entity_eye: Some([20.0, 63.0, 100.0]),
    };
    c.record(
        "m9.the_tracked_entitys_position_moves_the_bearing_not_the_distance",
        lb::yaw_angle_to_camera(&raw, &cam) != lb::yaw_angle_to_camera(&with_eye, &cam)
            && lb::distance_squared(&raw, &cam) == lb::distance_squared(&with_eye, &cam),
        "`Vec3iWaypoint.position` prefers the entity's interpolated eye, but \
         `distanceSquared` reads `this.vector` and never consults the entity \
         (MUTATION: substituting in both makes a nearby player's sprite flicker \
         between bands as it interpolates across the boundary)",
    );

    // ── m10: the pitch arrow. ──────────────────────────────────────────────
    let above = lb::WaypointSubject::Vec3i {
        x: 0,
        y: 200,
        z: 10,
        entity_eye: None,
    };
    let below = lb::WaypointSubject::Vec3i {
        x: 0,
        y: -60,
        z: 10,
        entity_eye: None,
    };
    let level = lb::WaypointSubject::Vec3i {
        x: 0,
        y: 63,
        z: 10,
        entity_eye: None,
    };
    let mut looking_down = test_camera();
    looking_down.pitch = 60.0;
    c.record(
        "m10.the_arrow_points_where_the_subject_left_the_screen",
        lb::pitch_direction(&above, &cam) == lb::PitchDirection::Up
            && lb::pitch_direction(&below, &cam) == lb::PitchDirection::Down
            && lb::pitch_direction(&level, &cam) == lb::PitchDirection::None
            && lb::pitch_direction(&lb::WaypointSubject::Chunk { x: 5, z: 5 }, &looking_down)
                == lb::PitchDirection::Up
            && lb::pitch_direction(&lb::WaypointSubject::Empty, &looking_down)
                == lb::PitchDirection::None,
        "VEC3I projects the point; CHUNK and AZIMUTH have no y and consult only \
         the horizon; EMPTY short-circuits to NONE without consulting anything \
         (MUTATION: giving CHUNK the point rule would need a y it does not have, \
         and using the horizon for VEC3I ignores where the subject actually is)",
    );

    // ── m11: reversed-Z made `isBehindCamera` false. ───────────────────────
    let behind = [0.5, 64.0, -10.0];
    let ahead = [0.5, 64.0, 10.0];
    let (_, z_behind) = lb::project_point_to_screen(behind, &cam);
    let (_, z_ahead) = lb::project_point_to_screen(ahead, &cam);
    let (_, z_inside_near) = lb::project_point_to_screen([0.5, 64.0, 0.52], &cam);
    c.record(
        "m11.reversed_z_means_behind_the_camera_is_negative_not_above_one",
        z_behind < 0.0 && (0.0..=1.0).contains(&z_ahead) && z_inside_near > 1.0,
        format!(
            "behind -> {z_behind:.4}, ahead -> {z_ahead:.4}, inside the near plane \
             -> {z_inside_near:.2}. 26.2's `Projection.getMatrix` passes \
             `near = this.zFar; far = this.zNear`, so `pointOnScreen.z > 1.0` — \
             the test vanilla still calls `isBehindCamera` — now means \"closer \
             than 0.05 blocks\" (MUTATION: building the matrix near-first makes \
             the name true again and takes a branch vanilla no longer takes)"
        ),
    );

    // ── m12: the arrow's own placement. ────────────────────────────────────
    let m = lb::markers(&[wp(above, 0xFFFF_FFFF)], &styles, &cam, GUI_W, GUI_H);
    let (ax, ay) = m[0].arrow.unwrap();
    let top = ref_bar_origin(GUI_W, GUI_H).1;
    c.record(
        "m12.the_arrow_sits_one_pixel_right_of_its_dot",
        ax == m[0].x + 1 && ay == top - 6 && m[0].y == top - 2,
        format!(
            "dot at y {} = top - 2, arrow at ({ax}, {ay}) = (dot + ARROW_LEFT, \
             top - HEIGHT - ARROW_PADDING) (MUTATION: centring the 7-wide arrow \
             on the 9-wide dot by (9-7)/2 gives the same 1 by coincidence; using \
             the dot's own x puts it a pixel left)",
            m[0].y
        ),
    );

    // ── m13: an unresolvable style. ────────────────────────────────────────
    let unknown = lb::LocatorWaypoint {
        subject: lb::WaypointSubject::Vec3i {
            x: 0,
            y: 62,
            z: 10,
            entity_eye: None,
        },
        color: 0xFFFF_FFFF,
        style: usize::MAX,
        is_camera_entity: false,
    };
    let m = lb::markers(&[unknown], &styles, &cam, GUI_W, GUI_H);
    c.record(
        "m13.an_unknown_style_still_draws_a_dot",
        m.len() == 1,
        "`waypointStyles.getOrDefault(id, MISSING)` — a style key the client has \
         no asset for resolves to the missing-texture style, not to nothing \
         (MUTATION: skipping the waypoint makes a server's custom style silently \
         invisible instead of visibly wrong)",
    );
}

// ── Pixels ──────────────────────────────────────────────────────────────────

/// A pixel is the synthetic subject when it is magenta: the tint's own red and
/// blue survive a white/grey texel unchanged and its green is zero. Nothing
/// the rest of the frame draws satisfies all three.
fn is_magenta(p: &[u8]) -> bool {
    let (r, g, b) = (p[0] as i32, p[1] as i32, p[2] as i32);
    g < 40 && r > 100 && b > 100 && (r - b).abs() < 24
}

fn magenta_pixels(buf: &[u8]) -> Vec<(u32, u32)> {
    let mut out = Vec::new();
    for y in 0..H {
        for x in 0..W {
            let i = ((y * W + x) * 4) as usize;
            if is_magenta(&buf[i..i + 4]) {
                out.push((x, y));
            }
        }
    }
    out
}

fn px(buf: &[u8], x: u32, y: u32) -> [u8; 4] {
    let i = ((y * W + x) * 4) as usize;
    [buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]
}

fn srgb_to_linear(c: f64) -> f64 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(c: f64) -> f64 {
    if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

fn check_pixels(
    c: &mut Checker,
    args: &LocatorshotArgs,
    baked: &assets::BakedAssets,
    ids: &Ids,
) -> Result<(), String> {
    let mut gpu = Gpu::new(None, true)?;
    println!(
        "[locatorshot] Vulkan validation: {}",
        if gpu.validation_active { "ON" } else { "off" }
    );
    if args.check && !gpu.validation_active {
        return Err("locatorshot: Vulkan validation requested but not active".into());
    }
    let mut off = Offscreen::new(&mut gpu, W, H)?;
    let white: Vec<u8> = vec![255u8; 16 * 16 * 4];
    let layers = [white];
    let mut wr = WorldRenderer::new(&mut gpu, off.format, 16, &layers)?;
    wr.set_sky_mode(SkyMode::None);
    // The same setup helpers the frame path uses — a gate that reimplements a
    // slice of the app's setup misses whatever the app adds to it (M45).
    let hud = crate::live_cmd::hud_sprites(baked).ok_or("hud sprites missing from the jar")?;
    wr.init_hud(&mut gpu, &hud)?;
    let sprites =
        crate::live_cmd::locator_sprites(baked).ok_or("locator sprites missing from the jar")?;
    let styles = sprites.styles.clone();
    wr.init_locator_bar(&mut gpu, &sprites)?;

    c.record(
        "p0.the_gui_scale_is_the_one_the_predictions_assume",
        rewo_gpu::hud::gui_scale(W as f32, H as f32) == SCALE as f32,
        format!("{W}x{H} -> scale {SCALE}, GUI space {GUI_W}x{GUI_H}"),
    );
    c.record(
        "p0b.the_jars_style_table_has_the_two_vanilla_styles",
        styles.len() == 2
            && styles.iter().any(|s| s.key == "minecraft:default")
            && styles.iter().any(|s| {
                s.key == "minecraft:bowtie" && s.near_distance == 64 && s.sprites.len() == 5
            }),
        format!(
            "{:?} (MUTATION: hard-coding the default four sprites drops bowtie's \
             near_distance override of 64 and its own leading sprite)",
            styles.iter().map(|s| s.key.as_str()).collect::<Vec<_>>()
        ),
    );

    let ring = crate::stats::OverlayRing::default();
    let overlay_draw = rewo_gpu::overlay::OverlayDraw {
        samples_ms: &ring.data,
        head: ring.head(),
        scale_ms: 20.0,
        origin: [-4000.0, -4000.0],
        size: [8.0, 8.0],
    };
    let vp = glam::Mat4::IDENTITY.to_cols_array_2d();
    let clear = [0.0, 0.0, 0.0, 1.0];

    // Everything below goes through the SAME resolver the windowed frame calls
    // (`live_cmd::locator_bar_state`) fed from a store the SAME router filled.
    let entities = rewo_world::entities::EntityTable::default();
    let shot = |gpu: &mut Gpu,
                    off: &mut Offscreen,
                    wr: &mut WorldRenderer,
                    store: &WaypointStore,
                    yaw: f32,
                    pitch: f32|
     -> Result<Vec<u8>, String> {
        let state = crate::live_cmd::locator_bar_state(
            crate::live_cmd::LocatorInputs {
                waypoints: store,
                own_uuid: None,
                entities: &entities,
                styles: &styles,
                eye: [0.5, 64.0, 0.5],
                feet: [0.5, 62.38, 0.5],
                yaw,
                pitch,
                fov: 70.0,
                has_experience: false,
                xp_prioritised: false,
                ticks: 0,
            },
            GUI_W,
            GUI_H,
            0.0,
        );
        wr.set_locator_bar(state);
        wr.set_hud(
            0,
            rewo_gpu::hud::HudGauges::default(),
            rewo_gpu::survival_hud::layout_for_screen(
                &rewo_gpu::survival_hud::SurvivalInputs::simple(20.0, 20),
                W as f32,
                H as f32,
            ),
        );
        off.render(gpu, Some((&mut *wr, vp)), &overlay_draw, clear)?;
        off.read_rgba(gpu)
    };

    // ── p1: the empty control. ─────────────────────────────────────────────
    let (bar_left, bar_top) = ref_bar_origin(GUI_W, GUI_H);
    let strip_band = |buf: &[u8]| -> usize {
        let mut n = 0;
        for y in (bar_top * SCALE) as u32..((bar_top + lb::BAR_H) * SCALE) as u32 {
            for x in (bar_left * SCALE) as u32..((bar_left + lb::BAR_W) * SCALE) as u32 {
                let p = px(buf, x, y);
                if p[0] > 4 || p[1] > 4 || p[2] > 4 {
                    n += 1;
                }
            }
        }
        n
    };

    let empty_store = WaypointStore::default();
    let none_frame = shot(&mut gpu, &mut off, &mut wr, &empty_store, 0.0, 0.0)?;
    c.record(
        "p1.a_frame_with_no_waypoints_paints_no_strip_and_no_magenta",
        magenta_pixels(&none_frame).is_empty() && strip_band(&none_frame) == 0,
        format!(
            "{} magenta pixels, {} lit pixels in the strip band (MUTATION: the \
             detector must find nothing here or every count below is measuring \
             the background — this is the empty frame `handshot` asserts)",
            magenta_pixels(&none_frame).len(),
            strip_band(&none_frame)
        ),
    );

    // ── p2: the background is a separate call from the loop. ───────────────
    //
    // A waypoint 300° away is tracked but outside the ±60° window, so
    // `extractRenderState`'s loop emits nothing while `extractBackground`
    // still paints the strip.
    let mut behind_store = WaypointStore::default();
    rewo_net::route_waypoint(
        ids.cb_play_waypoint,
        &body(
            0,
            &WaypointId::Uuid(1),
            "minecraft:default",
            Some((255, 0, 255)),
            WaypointContents::Vec3i {
                x: 0,
                y: 62,
                z: -200,
            },
        ),
        ids,
        &mut behind_store,
    );
    let behind_frame = shot(&mut gpu, &mut off, &mut wr, &behind_store, 0.0, 0.0)?;
    c.record(
        "p2.the_strip_paints_even_when_no_dot_is_in_the_window",
        strip_band(&behind_frame) > 500 && magenta_pixels(&behind_frame).is_empty(),
        format!(
            "{} lit strip pixels and {} magenta ones from a waypoint 180° behind \
             (MUTATION: gating the background on a non-empty marker list makes \
             the strip blink out whenever everyone is behind you)",
            strip_band(&behind_frame),
            magenta_pixels(&behind_frame).len()
        ),
    );

    // ── p3: the dot lands exactly where the model said. ────────────────────
    let mut store = WaypointStore::default();
    rewo_net::route_waypoint(
        ids.cb_play_waypoint,
        &body(
            0,
            &WaypointId::Uuid(1),
            "minecraft:default",
            Some((255, 0, 255)),
            WaypointContents::Vec3i {
                x: 0,
                y: 62,
                z: 200,
            },
        ),
        ids,
        &mut store,
    );
    let dot_frame = shot(&mut gpu, &mut off, &mut wr, &store, 0.0, 0.0)?;
    let hits = magenta_pixels(&dot_frame);
    // The model's prediction, in screen pixels.
    let predicted_x = ref_screen_middle(GUI_W) * SCALE;
    let predicted_y = (bar_top - 2) * SCALE;
    let inside = hits.iter().all(|&(x, y)| {
        (x as i32) >= predicted_x
            && (x as i32) < predicted_x + lb::DOT_SIZE * SCALE
            && (y as i32) >= predicted_y
            && (y as i32) < predicted_y + lb::DOT_SIZE * SCALE
    });
    c.record(
        "p3.every_magenta_pixel_lies_inside_the_predicted_dot_rect",
        !hits.is_empty() && inside,
        format!(
            "{} magenta pixels, all within the {}x{} rect at ({predicted_x}, \
             {predicted_y}) that `markers` predicted (MUTATION: this is the claim \
             the strip itself cannot satisfy — a count-only witness would pass \
             with the dot anywhere on screen)",
            hits.len(),
            lb::DOT_SIZE * SCALE,
            lb::DOT_SIZE * SCALE
        ),
    );

    // ── p4: turning the camera moves the dot by the predicted amount. ──────
    let mut ok_moves = true;
    let mut samples = Vec::new();
    for yaw in [-50.0f32, -25.0, 25.0, 50.0, 179.0, -179.0] {
        let frame = shot(&mut gpu, &mut off, &mut wr, &store, yaw, 0.0)?;
        let hits = magenta_pixels(&frame);
        let want_angle = ref_degrees_difference(yaw, 0.0) as f64;
        if !ref_visible(want_angle) {
            if !hits.is_empty() {
                ok_moves = false;
            }
            samples.push((yaw, None));
            continue;
        }
        let want_x = (ref_screen_middle(GUI_W) + ref_dot_offset(want_angle)) * SCALE;
        let min_x = hits.iter().map(|&(x, _)| x as i32).min();
        if min_x != Some(want_x) && !hits.is_empty() {
            // The dot's leftmost lit column can be inset from the rect, so
            // compare the rect the pixels occupy rather than its first column.
            let max_x = hits.iter().map(|&(x, _)| x as i32).max().unwrap();
            if !(min_x.unwrap() >= want_x
                && max_x < want_x + lb::DOT_SIZE * SCALE)
            {
                ok_moves = false;
            }
        }
        if hits.is_empty() {
            ok_moves = false;
        }
        samples.push((yaw, min_x));
    }
    c.record(
        "p4.turning_the_camera_slides_the_dot_across_the_wrap",
        ok_moves,
        format!(
            "leftmost magenta column per yaw: {samples:?} — including ±179, where \
             the two frames straddle the wrap (MUTATION: dropping `wrapDegrees` \
             agrees at every other sample and throws the dot off the strip here)"
        ),
    );

    // ── p5: the tint is a GAMMA-space multiply (M50's rule). ───────────────
    let mut grey_store = WaypointStore::default();
    rewo_net::route_waypoint(
        ids.cb_play_waypoint,
        &body(
            0,
            &WaypointId::Uuid(1),
            "minecraft:default",
            Some((32, 32, 32)),
            WaypointContents::Vec3i {
                x: 0,
                y: 62,
                z: 200,
            },
        ),
        ids,
        &mut grey_store,
    );
    let mut white_store = WaypointStore::default();
    rewo_net::route_waypoint(
        ids.cb_play_waypoint,
        &body(
            0,
            &WaypointId::Uuid(1),
            "minecraft:default",
            Some((255, 255, 255)),
            WaypointContents::Vec3i {
                x: 0,
                y: 62,
                z: 200,
            },
        ),
        ids,
        &mut white_store,
    );
    let white_frame = shot(&mut gpu, &mut off, &mut wr, &white_store, 0.0, 0.0)?;
    let grey_frame = shot(&mut gpu, &mut off, &mut wr, &grey_store, 0.0, 0.0)?;
    // The two rules are NOT generally far apart: sRGB is nearly a pure power,
    // and a power commutes with multiplication — `(a·b)^g == a^g · b^g`
    // exactly. The whole difference lives in sRGB's linear toe below 0.04045,
    // so it is only observable when the PRODUCT lands in the toe: a dark tint
    // over a lit texel. A mid-grey tint separates them by under a byte, which
    // would make this witness unable to fail — the first version used 128 and
    // measured a 0.78-byte gap against a 0.21-byte one.
    const TINT: f64 = 32.0;
    let (mut separable, mut matched_gamma, mut matched_linear) = (0usize, 0usize, 0usize);
    let (mut worst_gamma, mut best_linear) = (0.0f64, f64::INFINITY);
    for y in predicted_y..predicted_y + lb::DOT_SIZE * SCALE {
        for x in predicted_x..predicted_x + lb::DOT_SIZE * SCALE {
            let w = px(&white_frame, x as u32, y as u32)[0] as f64;
            // The outline is opaque black under either rule and agrees with
            // both predictions, so it is excluded rather than allowed to
            // dilute the measurement.
            if w < 64.0 {
                continue;
            }
            let g = px(&grey_frame, x as u32, y as u32)[0] as f64;
            let gamma = w * TINT / 255.0;
            let linear =
                linear_to_srgb(srgb_to_linear(w / 255.0) * srgb_to_linear(TINT / 255.0)) * 255.0;
            // Only the texels where the two rules actually disagree can
            // witness which one ran.
            if (gamma - linear).abs() < 2.0 {
                continue;
            }
            separable += 1;
            worst_gamma = worst_gamma.max((g - gamma).abs());
            best_linear = best_linear.min((g - linear).abs());
            if (g - gamma).abs() <= 1.0 {
                matched_gamma += 1;
            }
            if (g - linear).abs() <= 1.0 {
                matched_linear += 1;
            }
        }
    }
    c.record(
        "p5.the_dot_tint_multiplies_in_gamma_space",
        separable >= 8 && matched_gamma == separable && matched_linear == 0,
        format!(
            "{separable} texels where the two rules differ by >= 2 bytes: \
             {matched_gamma} match the gamma prediction within 1 byte (worst \
             {worst_gamma:.2}) and {matched_linear} match the LINEAR one (best \
             {best_linear:.2}) (MUTATION: sampling the atlas through an sRGB view \
             and multiplying in linear is M50's exact error — invisible at a mid \
             tint, and only a tint dark enough to push the product into sRGB's \
             toe can tell the two apart at all)"
        ),
    );

    // ── p6: the arrow, which is NOT tinted. ────────────────────────────────
    let mut high_store = WaypointStore::default();
    rewo_net::route_waypoint(
        ids.cb_play_waypoint,
        &body(
            0,
            &WaypointId::Uuid(1),
            "minecraft:default",
            Some((255, 0, 255)),
            WaypointContents::Vec3i {
                x: 0,
                y: 400,
                z: 60,
            },
        ),
        ids,
        &mut high_store,
    );
    let arrow_frame = shot(&mut gpu, &mut off, &mut wr, &high_store, 0.0, 0.0)?;
    // The UP arrow's own predicted rect, **not** the strip's full width.
    //
    // The first version of this witness scanned the whole strip and measured
    // 1000 lit pixels with no arrow present, because the health hearts (GUI x
    // 69..149 at `guiH - 39`) and the hunger row (x 170..250) both cross those
    // rows. That is the fourteenth instance of this project's one detector
    // error — a signal measured against a background that already contains it.
    // The arrow sits at GUI x 157..164, between the two rows.
    let arrow_x = ref_screen_middle(GUI_W) + lb::ARROW_LEFT;
    let arrow_band = |buf: &[u8]| -> (usize, usize) {
        let (mut lit, mut magenta) = (0, 0);
        for y in ((bar_top - 6) * SCALE) as u32..((bar_top - 6 + lb::ARROW_H) * SCALE) as u32 {
            for x in (arrow_x * SCALE) as u32..((arrow_x + lb::ARROW_W) * SCALE) as u32 {
                let p = px(buf, x, y);
                if p[0] > 40 || p[1] > 40 || p[2] > 40 {
                    lit += 1;
                }
                if is_magenta(&p) {
                    magenta += 1;
                }
            }
        }
        (lit, magenta)
    };
    let (arrow_lit, arrow_magenta) = arrow_band(&arrow_frame);
    let (flat_lit, _) = arrow_band(&dot_frame);
    c.record(
        "p6.an_off_screen_subject_grows_an_untinted_arrow",
        arrow_lit > 0 && arrow_magenta == 0 && flat_lit == 0,
        format!(
            "{arrow_lit} lit and {arrow_magenta} magenta pixels in the arrow's own rect \
             with the subject 336 blocks up, against {flat_lit} lit with it level \
             (MUTATION: `blitSprite`'s 6-argument overload passes color = -1, so \
             tinting the arrow with the dot's colour would make this magenta \
             count non-zero)"
        ),
    );

    // ── p7: the rendered strip is the source's own nine-slice. ─────────────
    //
    // Every one of the 182 columns is predicted independently from the jar's
    // 12-wide sprite — left 5 verbatim, right 5 verbatim, middle repeating
    // columns 5 and 6 — and compared against the frame, on the row through the
    // strip's centre.
    //
    // **What this witness cannot see, recorded rather than claimed.** The
    // mutation that stretches the middle instead of tiling it leaves this
    // green, because `locator_bar_background.png`'s columns 5 and 6 are
    // byte-identical in every row (the sprite is symmetric about its centre).
    // Tile and stretch therefore produce the same 172 columns for *this*
    // sprite, and the tiling rule is carried by `t11`, which runs the expander
    // on synthetic data where the two middle columns differ. What p7 does pin
    // is the width, the two borders, and that the pixels reaching the screen
    // are the source texels rather than a resample.
    let src = &baked.locator.as_ref().ok_or("no locator sprites")?.background;
    let row_in_sprite = 2u32;
    let src_col = |x: u32| -> [u8; 4] {
        let i = ((row_in_sprite * src.w + x) * 4) as usize;
        [
            src.rgba[i],
            src.rgba[i + 1],
            src.rgba[i + 2],
            src.rgba[i + 3],
        ]
    };
    let want_col = |gui_x: i32| -> [u8; 4] {
        if gui_x < 5 {
            src_col(gui_x as u32)
        } else if gui_x >= lb::BAR_W - 5 {
            src_col((src.w as i32 - (lb::BAR_W - gui_x)) as u32)
        } else {
            src_col(5 + ((gui_x - 5) % 2) as u32)
        }
    };
    let row = ((bar_top + row_in_sprite as i32) * SCALE) as u32;
    let col_at = |gui_x: i32| px(&behind_frame, ((bar_left + gui_x) * SCALE) as u32, row);
    let mut mismatched = 0usize;
    for gui_x in 0..lb::BAR_W {
        let (got, want) = (col_at(gui_x), want_col(gui_x));
        if got[..3] != want[..3] {
            mismatched += 1;
        }
    }
    let middles_are_identical = src_col(5) == src_col(6);
    c.record(
        "p7.the_rendered_strip_is_the_sources_own_nine_slice",
        mismatched == 0 && col_at(0)[..3] != col_at(2)[..3] && strip_band(&behind_frame) > 500,
        format!(
            "all {} columns match an independent expansion of the 12-wide source, \
             and the borders differ from the middle ({:?} vs {:?}); the source's \
             two middle columns are identical ({middles_are_identical}), so tile \
             and stretch are INDISTINGUISHABLE here and `t11` carries that rule \
             on synthetic data (MUTATION: blitting the 12-wide sprite squashed to \
             182 mismatches {} of them)",
            lb::BAR_W,
            col_at(0),
            col_at(2),
            lb::BAR_W - mismatched as i32
        ),
    );

    if let Some(dir) = &args.out_dir {
        std::fs::create_dir_all(dir).ok();
        for (name, buf) in [
            ("locator_none", &none_frame),
            ("locator_strip", &behind_frame),
            ("locator_dot", &dot_frame),
            ("locator_arrow", &arrow_frame),
        ] {
            let path = std::path::PathBuf::from(dir).join(format!("{name}.png"));
            let mut px = buf.clone();
            for p in px.chunks_exact_mut(4) {
                p[3] = 255;
            }
            let file = std::fs::File::create(&path).map_err(|e| format!("{path:?}: {e}"))?;
            let mut enc = png::Encoder::new(std::io::BufWriter::new(file), W, H);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            enc.write_header()
                .and_then(|mut w| w.write_image_data(&px))
                .map_err(|e| format!("{path:?}: {e}"))?;
            println!("[locatorshot] wrote {}", path.display());
        }
    }

    wr.destroy(&mut gpu);
    off.destroy(&mut gpu);
    Ok(())
}
