//! `rewo titleshot --check` — the M79 title-overlay + HUD-gauge oracle.
//!
//! Seven packets that all land on vanilla's `Gui` / `Hud`. They are
//! `REWO_PACKET_COVERAGE.md` class **B** — the first of that class Rewo has
//! taken — and the point this gate makes is that **the class letter changes
//! the gate, not the standard**. Every one of the seven has an exact vanilla
//! oracle, so the model half is graded exactly like a class-A milestone, and a
//! pixel read-back half is added on top for the things only a renderer can
//! answer.
//!
//! The path under test, end to end:
//!
//! ```text
//! raw set_title_text / … / cooldown bodies (built here)
//!   -> rewo_net::route_hud_state          (with a REAL `Ids::resolve`d table)
//!   -> rewo_net::hud_state::HudState      (the production state machines)
//!   -> live_cmd::{title_lines, experience_level_lines, resolve_hud_gauges}
//!                                          (the SAME resolvers the frame path calls)
//!   -> WorldRenderer::{set_text, set_hud} -> TextPass / HudPass
//!   -> Offscreen::read_rgba                (real pixels)
//! ```
//!
//! ## Three rules this gate is built around, each earned elsewhere
//!
//! **The gate drives the real emitter.** M45's `install_shapes` failure and
//! M41's rotted `swingshot` fixture were both gates that had quietly stopped
//! testing their subject, because they reimplemented a slice of the app's
//! setup. So `route_hud_state` is called with a real `Ids` resolved from the
//! datagen report, and the three line builders in `live_cmd` are the ones the
//! windowed client calls — they were made session-free for exactly this, the
//! way M59 extracted `resolve_health_bar`.
//!
//! **The detector must not share a colour with its background.** Three
//! detector errors on this project were all the same shape — "non-black"
//! against a painted sky, brown against a brown hotbar, cyan against a blue
//! sky. So the title's subject is **magenta**, chosen through the packet's own
//! `{"color":"#FF00FF"}`, and `p1` asserts an otherwise-identical frame
//! carrying no title contains none of it. Nothing else the HUD draws is
//! magenta: the hearts are red (`b` low), the hotbar grey, the crosshair
//! white-ish, the XP bar green.
//!
//! **The fade is measured in the space it works in.** A title's alpha ramp is
//! a linear-space composite, so `p6` decodes the observed sRGB byte back to
//! linear and compares it to `a + 0.25·a·(1 − a)` — the glyph over its own
//! drop shadow over black — rather than eyeballing "is it dimmer".
//!
//! Magenta is also the one tint that needs **no colour-space assumption**:
//! `chat_style::rgb_f32` is a plain `/255`, and 0 and 1 are the two values
//! sRGB and linear agree on exactly.
//!
//! **Fail-closed** on a fixed [`EXPECTED_WITNESSES`] count.

use clap::Args as ClapArgs;

use rewo_data::assets;
use rewo_data::packets::Packets;
use rewo_data::DataPaths;
use rewo_gpu::hud::HudGauges;
use rewo_gpu::offscreen::Offscreen;
use rewo_gpu::world::{SkyMode, WorldRenderer};
use rewo_gpu::Gpu;
use rewo_net::hud_state::{ExperienceState, HudState, SetExperience, TitleOverlay};
use rewo_net::ids::Ids;

/// Total named properties this gate asserts. Locked so a skipped property
/// fails the run even when nothing mismatched.
const EXPECTED_WITNESSES: usize = 57;

const W: u32 = 640;
const H: u32 = 480;
/// `gui_scale(640, 480)` = `min(480/240, 640/320)` = 2, so GUI space is
/// 320×240 and one GUI pixel is two screen pixels. Asserted, not assumed —
/// every predicted coordinate below is multiplied by it.
const SCALE: i32 = 2;
const GUI_W: i32 = W as i32 / SCALE;
const GUI_H: i32 = H as i32 / SCALE;

#[derive(ClapArgs)]
pub struct TitleshotArgs {
    /// Assert every owned property. The oracle asserts unconditionally and a
    /// failure exits nonzero with or without this flag, so it only labels the
    /// run — the convention `eventshot`/`danceshot`/`healthbarshot` use.
    #[arg(long, default_value_t = false)]
    check: bool,
    #[arg(long, default_value = "26.2")]
    version: String,
    /// Write the rendered frames here for eyeballing. Never read back.
    #[arg(long)]
    out_dir: Option<std::path::PathBuf>,
}

struct Checker {
    witnessed: usize,
    failures: Vec<String>,
}

impl Checker {
    fn record(&mut self, name: &str, pass: bool, detail: impl std::fmt::Display) {
        println!(
            "[titleshot] {}  {name}: {detail}",
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

pub fn run(args: TitleshotArgs) -> Result<(), String> {
    let mode = if args.check { "check" } else { "report" };
    println!(
        "[titleshot] mode: {mode} (serverless; the oracle asserts \
         unconditionally). Model half against the 26.2 decompile; pixel half \
         against a synthetic magenta subject."
    );

    let paths = DataPaths::for_version(&args.version)
        .ok_or_else(|| "no config dir for version data".to_string())?;
    let packets = Packets::load(&paths.packets_json())?;
    let ids = Ids::resolve(&packets)?;
    let jar = client_jar(&args.version).ok_or("client jar not found")?;
    let baked = assets::bake(&jar, &paths.blocks_json())?;
    let items = rewo_data::items::Items::load(&paths.registries_json())?;

    let mut c = Checker {
        witnessed: 0,
        failures: Vec::new(),
    };
    check_transcription(&mut c);
    check_wire(&mut c, &ids);
    check_lines(&mut c, &baked, &items);
    check_pixels(&mut c, &args, &baked)?;

    println!(
        "[titleshot] witnesses observed: {} / {}",
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
    println!("[titleshot] PASS — {} witnesses", c.witnessed);
    Ok(())
}

// ---------------------------------------------------------------------------
// The decompile, transcribed here.
//
// Re-declared rather than imported from `rewo_gpu::hud`, for the reason
// `healthbarshot` states at the top of its own spec block: **a witness that
// asks the implementation what to expect asserts only that the implementation
// equals itself.** The first version of this gate did exactly that for the
// cooldown span, and the mutation battery caught it - inverting the span to
// grow downward from the top left every witness green, because the gate's
// prediction moved with it.
//
// Where a sample's answer is a small integer it is *also* pinned as a literal
// below, which is what stops a mis-transcription here from cancelling a
// mis-transcription there.
// ---------------------------------------------------------------------------

/// `Hud.extractTitle`'s ramp.
fn ref_title_alpha(title_time: i32, fade_in: i32, stay: i32, fade_out: i32, partial: f32) -> i32 {
    let t = title_time as f32 - partial;
    let mut alpha = 255;
    if title_time > fade_out + stay {
        let time = (fade_in + stay + fade_out) as f32 - t;
        alpha = (time * 255.0 / fade_in as f32) as i32;
    }
    if title_time <= fade_out {
        alpha = (t * 255.0 / fade_out as f32) as i32;
    }
    alpha.clamp(0, 255)
}

/// `translate(guiWidth / 2, guiHeight / 2); scale(4,4); text(.., -w / 2, -10)`.
fn ref_title_pos(gui_w: i32, gui_h: i32, width: i32) -> (i32, i32) {
    (gui_w / 2 + 4 * -(width / 2), gui_h / 2 + 4 * -10)
}

/// `scale(2,2); text(.., -w / 2, 5)` inside the same centre translate.
fn ref_subtitle_pos(gui_w: i32, gui_h: i32, width: i32) -> (i32, i32) {
    (gui_w / 2 + 2 * -(width / 2), gui_h / 2 + 2 * 5)
}

/// `translate(guiWidth / 2, guiHeight - 68); text(.., -w / 2, -4)`.
fn ref_action_bar_pos(gui_w: i32, gui_h: i32, width: i32) -> (i32, i32) {
    (gui_w / 2 + -(width / 2), gui_h - 68 - 4)
}

/// `ContextualBar.left` / `.top`.
fn ref_experience_bar_pos(gui_w: i32, gui_h: i32) -> (i32, i32) {
    ((gui_w - 182) / 2, gui_h - 24 - 5)
}

/// `int progress = (int)(experienceProgress * 183.0F)`.
fn ref_experience_progress_px(progress: f32) -> i32 {
    (progress * 183.0) as i32
}

/// `itemCooldown`'s rect, as offsets from the icon's top.
fn ref_cooldown_offsets(cooldown: f32) -> (i32, i32) {
    let top = (16.0f32 * (1.0 - cooldown)).floor() as i32;
    (top, top + (16.0f32 * cooldown).ceil() as i32)
}

/// The transcription against hand-computed literals **and** against the
/// production constants, as a recorded witness rather than a hard assert.
///
/// The literals are what stop a slip in both copies from cancelling out; the
/// production comparison is what makes the whole transcription load-bearing
/// instead of a second opinion nobody consults. Written as a witness because a
/// `assert!` inside the pixel section skips `destroy` on the way out and buries
/// the real failure under a Vulkan device-teardown VUID -- which is exactly how
/// the cooldown mutation reported itself in the battery's second round.
fn check_transcription(c: &mut Checker) {
    let literals = ref_cooldown_offsets(1.0) == (0, 16)
        && ref_cooldown_offsets(0.5) == (8, 16)
        && ref_cooldown_offsets(0.25) == (12, 16)
        && ref_experience_progress_px(0.5) == 91
        && ref_experience_progress_px(1.0) == 183
        && ref_title_pos(320, 240, 3) == (156, 80)
        && ref_subtitle_pos(320, 240, 3) == (158, 130)
        && ref_action_bar_pos(320, 240, 40) == (140, 168)
        && ref_experience_bar_pos(320, 240) == (69, 211)
        && ref_title_alpha(95, 10, 70, 20, 0.0) == 127
        && ref_title_alpha(10, 10, 70, 20, 0.0) == 127
        && ref_title_alpha(50, 10, 70, 20, 0.0) == 255;
    let production = (0..=100).all(|i| {
        let cd = i as f32 / 100.0;
        let want = if cd > 0.0 {
            Some(ref_cooldown_offsets(cd))
        } else {
            None
        };
        rewo_gpu::hud::cooldown_overlay_offsets(cd) == want
            && rewo_gpu::hud::experience_progress_px(cd) == ref_experience_progress_px(cd)
    }) && (1..=40).all(|w| {
        rewo_gpu::hud::title_pos(GUI_W, GUI_H, w) == ref_title_pos(GUI_W, GUI_H, w)
            && rewo_gpu::hud::subtitle_pos(GUI_W, GUI_H, w) == ref_subtitle_pos(GUI_W, GUI_H, w)
            && rewo_gpu::hud::action_bar_pos(GUI_W, GUI_H, w)
                == ref_action_bar_pos(GUI_W, GUI_H, w)
    }) && rewo_gpu::hud::experience_bar_pos(GUI_W, GUI_H) == ref_experience_bar_pos(GUI_W, GUI_H)
        && (1..=120).all(|tt| {
            rewo_gpu::hud::title_alpha(tt, 10, 70, 20, 0.3)
                == ref_title_alpha(tt, 10, 70, 20, 0.3)
        });
    c.record(
        "m0.the_transcription_matches_its_literals_and_the_production_arithmetic",
        literals && production,
        "the placement, the ramp, the 183 multiply and the cooldown offsets, \
         graded against hand-computed values and then against `rewo_gpu::hud` \
         over their whole useful ranges",
    );
}

// ---------------------------------------------------------------------------
// Wire bodies - built here, independent of any writer under test.
// ---------------------------------------------------------------------------

fn varint(v: i32, out: &mut Vec<u8>) {
    let mut n = v as u32;
    loop {
        let b = (n & 0x7F) as u8;
        n >>= 7;
        if n == 0 {
            out.push(b);
            break;
        }
        out.push(b | 0x80);
    }
}

/// A trusted `Component` on the wire: network NBT, so a bare tag byte + a
/// payload with no name. Tag 8 is String.
fn component_string(s: &str) -> Vec<u8> {
    let mut out = vec![8u8];
    out.extend_from_slice(&(s.len() as u16).to_be_bytes());
    out.extend_from_slice(s.as_bytes());
    out
}

/// `{"text": …, "color": …}` as a network-NBT compound, so the styled path is
/// exercised rather than the bare-string one.
/// A component carrying all five renderable `Style` flags, as NBT.
///
/// All five at once on purpose: the five are five separate booleans on the
/// wire and five separate fields on `TextStyle`, so a wiring that carried one
/// and dropped four would satisfy any single-flag witness.
fn component_styled(text: &str) -> Vec<u8> {
    let mut out = vec![10u8]; // TAG_Compound
    out.push(8); // TAG_String
    out.extend_from_slice(&4u16.to_be_bytes());
    out.extend_from_slice(b"text");
    out.extend_from_slice(&(text.len() as u16).to_be_bytes());
    out.extend_from_slice(text.as_bytes());
    for k in ["bold", "italic", "underlined", "strikethrough", "obfuscated"] {
        out.push(1); // TAG_Byte
        out.extend_from_slice(&(k.len() as u16).to_be_bytes());
        out.extend_from_slice(k.as_bytes());
        out.push(1);
    }
    out.push(0); // TAG_End
    out
}

fn component_colored(text: &str, color: &str) -> Vec<u8> {
    let mut out = vec![10u8]; // TAG_Compound
    for (k, v) in [("text", text), ("color", color)] {
        out.push(8); // TAG_String
        out.extend_from_slice(&(k.len() as u16).to_be_bytes());
        out.extend_from_slice(k.as_bytes());
        out.extend_from_slice(&(v.len() as u16).to_be_bytes());
        out.extend_from_slice(v.as_bytes());
    }
    out.push(0); // TAG_End
    out
}

/// `ClientboundSetTitlesAnimationPacket` — three fixed big-endian i32s.
fn animation_body(fade_in: i32, stay: i32, fade_out: i32) -> Vec<u8> {
    let mut out = Vec::with_capacity(12);
    out.extend_from_slice(&fade_in.to_be_bytes());
    out.extend_from_slice(&stay.to_be_bytes());
    out.extend_from_slice(&fade_out.to_be_bytes());
    out
}

/// `ClientboundSetExperiencePacket` — float, VarInt **level**, VarInt total.
/// Built in wire order here so a reader that used the declaration order fails.
fn experience_body(progress: f32, level: i32, total: i32) -> Vec<u8> {
    let mut out = progress.to_be_bytes().to_vec();
    varint(level, &mut out);
    varint(total, &mut out);
    out
}

/// `ClientboundCooldownPacket` — `Identifier` then VarInt duration.
fn cooldown_body(group: &str, duration: i32) -> Vec<u8> {
    let mut out = Vec::new();
    varint(group.len() as i32, &mut out);
    out.extend_from_slice(group.as_bytes());
    varint(duration, &mut out);
    out
}

// ---------------------------------------------------------------------------
// 1. The wire and the state machines, through the real router.
// ---------------------------------------------------------------------------

fn check_wire(c: &mut Checker, ids: &Ids) {
    // w1 — the report resolves all seven by name, and the router matches
    // exactly those ids.
    let seven = [
        ("clear_titles", ids.cb_play_clear_titles),
        ("cooldown", ids.cb_play_cooldown),
        ("set_action_bar_text", ids.cb_play_set_action_bar_text),
        ("set_experience", ids.cb_play_set_experience),
        ("set_subtitle_text", ids.cb_play_set_subtitle_text),
        ("set_title_text", ids.cb_play_set_title_text),
        ("set_titles_animation", ids.cb_play_set_titles_animation),
    ];
    let mut distinct: Vec<i32> = seven.iter().map(|(_, id)| *id).collect();
    distinct.sort_unstable();
    distinct.dedup();
    c.record(
        "w1.the_seven_ids_resolve_and_are_distinct",
        distinct.len() == 7,
        format!(
            "{}",
            seven
                .iter()
                .map(|(n, id)| format!("{n}={id}"))
                .collect::<Vec<_>>()
                .join(" ")
        ),
    );

    let mut state = HudState::default();
    let matched = seven
        .iter()
        .all(|(_, id)| rewo_net::route_hud_state(*id, &[0u8; 12], ids, &mut state));
    // MUTATION partner: dropping any arm from `hud_state::kind_for_id` makes
    // this false. An id outside the set must not be swallowed, or the arm
    // would shadow every later packet in `handle_packet`'s chain.
    let stray = rewo_net::route_hud_state(
        ids.cb_play_set_time,
        &[0u8; 12],
        ids,
        &mut HudState::default(),
    );
    c.record(
        "w1b.the_router_matches_those_seven_and_nothing_else",
        matched && !stray,
        format!("all seven matched={matched}, set_time swallowed={stray}"),
    );

    // w2 — a subtitle on its own arms no clock, so nothing shows.
    let mut s = HudState::default();
    assert_route(ids.cb_play_set_subtitle_text, &component_string("go"), ids, &mut s);
    c.record(
        "w2.a_subtitle_alone_arms_no_clock",
        s.titles.subtitle.is_some() && s.titles.title_time == 0 && !s.titles.showing_title(),
        format!(
            "subtitle set, titleTime={} (MUTATION: arming from `setSubtitle` \
             makes it 100 and shows a bare subtitle vanilla never shows)",
            s.titles.title_time
        ),
    );

    // w3 — the title that follows it shows both, at the full duration.
    assert_route(ids.cb_play_set_title_text, &component_string("main"), ids, &mut s);
    c.record(
        "w3.the_title_that_follows_shows_both",
        s.titles.showing_title() && s.titles.title_time == 100 && s.titles.subtitle.is_some(),
        format!(
            "titleTime={} = 10+70+20, subtitle survived",
            s.titles.title_time
        ),
    );

    // w4 — a negative axis is a skip, per axis; zero is a legal set.
    let mut s = HudState::default();
    assert_route(ids.cb_play_set_titles_animation, &animation_body(-1, -1, 40), ids, &mut s);
    let skipped = (s.titles.fade_in, s.titles.stay, s.titles.fade_out) == (10, 70, 40);
    assert_route(ids.cb_play_set_titles_animation, &animation_body(0, 0, 0), ids, &mut s);
    let zeroed = (s.titles.fade_in, s.titles.stay, s.titles.fade_out) == (0, 0, 0);
    c.record(
        "w4.a_negative_time_is_a_per_axis_skip_and_zero_is_not",
        skipped && zeroed,
        "(-1,-1,40) kept 10/70 and set 40; (0,0,0) set all three \
         (MUTATION: assigning unconditionally leaves fade_in at -1)",
    );

    // w5 — `setTimes` re-arms a LIVE title at its full duration.
    let mut s = HudState::default();
    assert_route(ids.cb_play_set_title_text, &component_string("hi"), ids, &mut s);
    for _ in 0..50 {
        s.tick();
    }
    let halfway = s.titles.title_time;
    assert_route(ids.cb_play_set_titles_animation, &animation_body(10, 70, 20), ids, &mut s);
    c.record(
        "w5.set_times_re_arms_a_live_title",
        halfway == 50 && s.titles.title_time == 100,
        format!(
            "{halfway} -> {} (MUTATION: dropping the trailing `if (titleTime > 0)` \
             leaves it at 50 — it retimes the remainder instead of restarting)",
            s.titles.title_time
        ),
    );

    // w5b — …and does NOT arm one that is not showing.
    let mut s = HudState::default();
    assert_route(ids.cb_play_set_titles_animation, &animation_body(1, 2, 3), ids, &mut s);
    c.record(
        "w5b.set_times_does_not_arm_an_absent_title",
        s.titles.title_time == 0,
        "the re-arm is guarded on `titleTime > 0`, so it cannot summon a title",
    );

    // w6 — clear keeps the durations; only `resetTimes` restores them.
    let mut s = HudState::default();
    assert_route(ids.cb_play_set_titles_animation, &animation_body(5, 6, 7), ids, &mut s);
    assert_route(ids.cb_play_set_title_text, &component_string("hi"), ids, &mut s);
    assert_route(ids.cb_play_clear_titles, &[0], ids, &mut s);
    let kept = (s.titles.fade_in, s.titles.stay, s.titles.fade_out) == (5, 6, 7)
        && s.titles.title.is_none()
        && s.titles.title_time == 0;
    assert_route(ids.cb_play_clear_titles, &[1], ids, &mut s);
    let reset = (s.titles.fade_in, s.titles.stay, s.titles.fade_out) == (10, 70, 20);
    c.record(
        "w6.clear_keeps_the_durations_and_reset_restores_them",
        kept && reset,
        "resetTimes=false left 5/6/7; resetTimes=true restored 10/70/20 \
         (MUTATION: resetting unconditionally makes the first assertion fail)",
    );

    // w7 — the expiry drops the subtitle too, and neither counter goes negative.
    let mut s = HudState::default();
    assert_route(ids.cb_play_set_titles_animation, &animation_body(0, 1, 0), ids, &mut s);
    assert_route(ids.cb_play_set_title_text, &component_string("t"), ids, &mut s);
    assert_route(ids.cb_play_set_subtitle_text, &component_string("s"), ids, &mut s);
    s.tick();
    let dropped = s.titles.title.is_none() && s.titles.subtitle.is_none();
    s.tick();
    c.record(
        "w7.the_expiry_drops_both_components_and_stops_at_zero",
        dropped && s.titles.title_time == 0,
        "a subtitle cannot outlive the title it was shown under",
    );

    // w8 — the action bar's own clock, and the flag the packet always clears.
    let mut s = HudState::default();
    assert_route(ids.cb_play_set_titles_animation, &animation_body(0, 5, 0), ids, &mut s);
    assert_route(ids.cb_play_set_action_bar_text, &component_string("bar"), ids, &mut s);
    let fresh = s.titles.overlay_message_time;
    for _ in 0..60 {
        s.tick();
    }
    c.record(
        "w8.the_action_bar_lives_sixty_ticks_regardless_of_the_title_times",
        fresh == 60 && s.titles.overlay_message_time == 0 && !s.titles.animate_overlay_message_color,
        format!(
            "armed at {fresh} with title times 0/5/0, expired at 0, animate=false \
             (MUTATION: `setOverlayMessage(text, true)` gives the jukebox rainbow)"
        ),
    );
    c.record(
        "w8b.the_expired_action_bar_string_is_not_cleared",
        s.titles.overlay_message.is_some(),
        "only `titleTime` nulls its components; `overlayMessageTime` just stops \
         the draw, which is why re-showing it needs a new packet",
    );

    // w9 — the experience wire order.
    let mut s = HudState::default();
    assert_route(
        ids.cb_play_set_experience,
        &experience_body(0.25, 7, 1000),
        ids,
        &mut s,
    );
    c.record(
        "w9.set_experience_reads_level_before_total",
        s.experience.level == 7 && s.experience.total == 1000 && s.experience.progress == 0.25,
        format!(
            "level={} total={} (MUTATION: the declaration order progress/total/level \
             swaps them and decodes without error — both are var-ints)",
            s.experience.level, s.experience.total
        ),
    );

    // w10 — the first change does not prioritise the bar.
    let mut s = HudState::default();
    for _ in 0..50 {
        s.tick();
    }
    assert_route(ids.cb_play_set_experience, &experience_body(0.5, 3, 30), ids, &mut s);
    let first = (s.experience.display_start_tick, s.experience.will_prioritize());
    assert_route(ids.cb_play_set_experience, &experience_body(0.6, 3, 32), ids, &mut s);
    let second = (s.experience.display_start_tick, s.experience.will_prioritize());
    c.record(
        "w10.the_first_experience_update_does_not_prioritise_the_bar",
        first == (i32::MIN + 1, false) && second == (50, true),
        format!(
            "first -> {first:?}, second -> {second:?} (MUTATION: writing tickCount \
             on the first change pops the XP bar over the locator bar on every join)"
        ),
    );

    // w11 — the re-arm keys on `progress` alone.
    let mut s = HudState::default();
    assert_route(ids.cb_play_set_experience, &experience_body(0.5, 1, 10), ids, &mut s);
    for _ in 0..200 {
        s.tick();
    }
    assert_route(ids.cb_play_set_experience, &experience_body(0.5, 9, 400), ids, &mut s);
    c.record(
        "w11.a_level_only_change_does_not_re_arm_the_window",
        s.experience.display_start_tick == i32::MIN + 1
            && s.experience.level == 9
            && s.experience.total == 400,
        format!(
            "start tick still the sentinel after a 1 -> 9 level change \
             (MUTATION: keying the re-arm on any field makes it 200)"
        ),
    );

    // w12 — duration 0 is a removal.
    let mut s = HudState::default();
    let group = "minecraft:ender_pearl";
    assert_route(ids.cb_play_cooldown, &cooldown_body(group, 40), ids, &mut s);
    let started = s.cooldowns.is_on_cooldown(group);
    assert_route(ids.cb_play_cooldown, &cooldown_body(group, 0), ids, &mut s);
    c.record(
        "w12.a_zero_duration_cooldown_is_a_removal",
        started && s.cooldowns.is_empty() && s.cooldowns.percent(group, 0.0) == 0.0,
        "started, then cancelled (MUTATION: routing 0 through `addCooldown` leaves \
         an instance whose percent is 0/0 — a NaN `Mth.clamp` does not rescue)",
    );

    // w13 — the countdown, and the tick order that decides when it is dropped.
    let mut s = HudState::default();
    assert_route(ids.cb_play_cooldown, &cooldown_body("g", 4), ids, &mut s);
    let ramp: Vec<f32> = (0..5)
        .map(|i| {
            if i > 0 {
                s.tick();
            }
            s.cooldowns.percent("g", 0.0)
        })
        .collect();
    c.record(
        "w13.the_cooldown_runs_down_and_is_dropped_the_tick_it_ends",
        ramp == vec![1.0, 0.75, 0.5, 0.25, 0.0] && s.cooldowns.is_empty(),
        format!(
            "{ramp:?} (MUTATION: sweeping before the increment keeps it one tick \
             longer, so the last frame shows a sliver)"
        ),
    );

    // w14 — the respawn asymmetry.
    let mut s = HudState::default();
    assert_route(ids.cb_play_set_titles_animation, &animation_body(5, 6, 7), ids, &mut s);
    assert_route(ids.cb_play_set_title_text, &component_string("welcome"), ids, &mut s);
    assert_route(ids.cb_play_cooldown, &cooldown_body("g", 40), ids, &mut s);
    assert_route(ids.cb_play_set_experience, &experience_body(0.5, 30, 900), ids, &mut s);
    s.reset_for_respawn();
    c.record(
        "w14.a_respawn_keeps_the_title_and_drops_the_player_state",
        s.titles.title.is_some()
            && s.titles.title_time == 18
            && (s.titles.fade_in, s.titles.stay, s.titles.fade_out) == (5, 6, 7)
            && s.cooldowns.is_empty()
            && s.experience.level == 0
            && s.experience.display_start_tick == i32::MIN,
        "the titles live on `Minecraft.gui.hud` (which `handleRespawn` never \
         reaches) and the gauges on the `LocalPlayer` it replaces — including \
         the display sentinel, so the post-respawn `set_experience` again fails \
         to prioritise the bar",
    );

    // w15 — a malformed body changes nothing.
    let mut s = HudState::default();
    assert_route(ids.cb_play_set_title_text, &component_string("kept"), ids, &mut s);
    let before = s.clone();
    let matched_anyway = rewo_net::route_hud_state(ids.cb_play_set_titles_animation, &[1, 2], ids, &mut s)
        && rewo_net::route_hud_state(ids.cb_play_clear_titles, &[], ids, &mut s)
        && rewo_net::route_hud_state(ids.cb_play_cooldown, &[], ids, &mut s);
    c.record(
        "w15.a_decode_failure_matches_the_id_and_changes_nothing",
        matched_anyway && s == before,
        "the router returns `true` for the id and `apply` leaves the state alone \
         — a `false` here would fall through to the next arm of the dispatch chain",
    );

    // w16 — the animation triple is twelve fixed bytes.
    c.record(
        "w16.the_animation_triple_is_twelve_fixed_bytes",
        animation_body(10, 70, 20).len() == 12
            && rewo_net::hud_state::read_titles_animation(&animation_body(10, 70, 20)).unwrap()
                == (10, 70, 20)
            && rewo_net::hud_state::read_titles_animation(&[10, 70, 20]).is_err(),
        "three `readInt`s, not var-ints — a var-int reader would accept the \
         three-byte body and read three plausible values out of it",
    );

    // w17 — the XP curve's three segments and its reachable-only-below-zero guard.
    let needed = |level: i32| {
        ExperienceState {
            level,
            ..ExperienceState::default()
        }
        .xp_needed_for_next_level()
    };
    c.record(
        "w17.the_xp_curve_has_three_segments_and_a_guard_only_a_bad_server_reaches",
        needed(0) == 7
            && needed(14) == 35
            && needed(15) == 37
            && needed(29) == 107
            && needed(30) == 112
            && needed(-3) == 1
            && needed(-4) == -1,
        "7+2L / 37+5(L-15) / 112+9(L-30); the `> 0` bar guard needs level <= -4",
    );
}

fn assert_route(id: i32, body: &[u8], ids: &Ids, state: &mut HudState) {
    assert!(
        rewo_net::route_hud_state(id, body, ids, state),
        "route_hud_state did not match id {id}"
    );
}

// ---------------------------------------------------------------------------
// 2. The emitted lines and gauges, through the app's own resolvers.
// ---------------------------------------------------------------------------

fn check_lines(c: &mut Checker, baked: &assets::BakedAssets, items: &rewo_data::items::Items) {
    let Some(font) = baked.font.as_ref() else {
        // Fail closed rather than silently skipping eleven witnesses.
        c.record("m0.the_font_baked", false, "no baked font in the client jar");
        return;
    };
    let advance = &font.advance;
    let px = SCALE as f32;
    let screen = (W as f32, H as f32);

    // m1 — nothing is emitted with no title and no action bar.
    let quiet = crate::live_cmd::title_lines(&TitleOverlay::default(), advance, px, screen, 0.0, None);
    c.record(
        "m1.a_quiet_hud_emits_no_title_lines",
        quiet.is_empty(),
        "so every count below is the overlay and nothing else",
    );

    // m2 — the title's placement, and the halving that happens before the
    // scale. The sample has to have an ODD width or the two readings agree and
    // the witness is vacuous (M76's "a sample must sit where the mutation
    // bites"), so the text is chosen for that property and the choice is
    // itself a witness.
    let odd = ["TITLE", "ODD", "GO!", "HI!", "I", "F1", "l"]
        .into_iter()
        .find(|s| rewo_gpu::text::width(s, advance) % 2 == 1);
    let mut t = TitleOverlay::default();
    match odd {
        Some(word) => {
            t.set_title(rewo_proto::nbt::Nbt::String(word.into()));
            // The very first frame of a title has alpha 0 — the fade-in is
            // measured from the start, so `titleTime == fadeIn + stay + fadeOut`
            // is the moment nothing is drawn yet. Tick into the hold.
            for _ in 0..15 {
                t.tick();
            }
            let lines = crate::live_cmd::title_lines(&t, advance, px, screen, 0.0, None);
            let width = rewo_gpu::text::width(word, advance);
            let (want_x, want_y) = ref_title_pos(GUI_W, GUI_H, width);
            let naive_x = GUI_W / 2 - (width * 4) / 2;
            // The production/transcription comparison lives in `m0`.
            c.record(
                "m2.the_title_centres_before_it_scales",
                lines.len() == 1
                    && lines[0].x == want_x as f32 * px
                    && lines[0].y == want_y as f32 * px
                    && lines[0].px == px * rewo_gpu::hud::TITLE_SCALE as f32,
                format!(
                    "\"{word}\" is {width} px -> x={want_x} at 4x (MUTATION:                      halving after the scale gives {naive_x}, {} GUI px away)",
                    (want_x - naive_x).abs()
                ),
            );
            c.record(
                "m2b.the_sample_sits_where_the_mutation_bites",
                width % 2 == 1 && want_x != naive_x,
                format!(
                    "an odd width ({width}) is the only place the two readings                      differ; an even one makes them agree and the witness vacuous"
                ),
            );
        }
        None => {
            c.record(
                "m2.the_title_centres_before_it_scales",
                false,
                "no candidate string has an odd width in this font",
            );
            c.record(
                "m2b.the_sample_sits_where_the_mutation_bites",
                false,
                "no candidate string has an odd width in this font",
            );
        }
    }

    // m3 — the subtitle draws only inside the title's block, at 2x and below.
    let mut t = TitleOverlay::default();
    t.set_subtitle(rewo_proto::nbt::Nbt::String("sub".into()));
    let alone = crate::live_cmd::title_lines(&t, advance, px, screen, 0.0, None);
    t.set_title(rewo_proto::nbt::Nbt::String("main".into()));
    for _ in 0..15 {
        t.tick();
    }
    let both = crate::live_cmd::title_lines(&t, advance, px, screen, 0.0, None);
    c.record(
        "m3.the_subtitle_draws_only_under_a_showing_title",
        alone.is_empty()
            && both.len() == 2
            && both[1].y > both[0].y
            // The literal 2, not `SUBTITLE_SCALE`: reading the constant back
            // asserts only that it equals itself, and a mutation to 4 walked
            // straight through the version that did.
            && both[1].px == px * 2.0
            && both[0].px == px * 4.0,
        format!(
            "alone={} lines, with a title={} lines, subtitle at y={} below the              title's {} and half its scale",
            alone.len(),
            both.len(),
            both.get(1).map(|l| l.y).unwrap_or(-1.0),
            both.first().map(|l| l.y).unwrap_or(-1.0)
        ),
    );

    // m4 — the subtitle carries the TITLE's alpha; it has no ramp of its own.
    let mut t = TitleOverlay::default();
    t.set_title(rewo_proto::nbt::Nbt::String("a".into()));
    t.set_subtitle(rewo_proto::nbt::Nbt::String("b".into()));
    for _ in 0..95 {
        t.tick();
    }
    let fading = crate::live_cmd::title_lines(&t, advance, px, screen, 0.0, None);
    c.record(
        "m4.the_subtitle_shares_the_titles_alpha",
        fading.len() == 2 && fading[0].alpha == fading[1].alpha && fading[0].alpha < 1.0,
        format!(
            "titleTime=5 into a 20-tick fade-out -> alpha {:.3} on both",
            fading[0].alpha
        ),
    );

    // m5 — a span's own colour replaces the RGB and keeps the fade's alpha.
    let mut t = TitleOverlay::default();
    let colored = rewo_net::hud_state::read_component(&component_colored("GO", "red")).unwrap();
    t.set_title(colored);
    for _ in 0..90 {
        t.tick();
    }
    let lines = crate::live_cmd::title_lines(&t, advance, px, screen, 0.0, None);
    let want_alpha = ref_title_alpha(10, 10, 70, 20, 0.0) as f32 / 255.0;
    c.record(
        "m5.a_coloured_title_takes_the_span_colour_and_still_fades",
        lines.len() == 1
            && lines[0].color_linear[0] > 0.9
            && lines[0].color_linear[1] < 0.4
            && (lines[0].alpha - want_alpha).abs() < 1e-6
            && want_alpha < 1.0,
        format!(
            "red {:?} at alpha {:.3} (MUTATION: taking the span's colour whole — \
             including its opaque alpha — gives a coloured title that snaps in \
             and out at full opacity while a white one fades)",
            lines[0].color_linear, lines[0].alpha
        ),
    );

    // m6 — the action bar is a separate block, not an `else`.
    let mut t = TitleOverlay::default();
    t.set_title(rewo_proto::nbt::Nbt::String("T".into()));
    t.set_overlay_message(rewo_proto::nbt::Nbt::String("A".into()), false);
    for _ in 0..15 {
        t.tick();
    }
    let together = crate::live_cmd::title_lines(&t, advance, px, screen, 0.0, None);
    let bar_y = ref_action_bar_pos(GUI_W, GUI_H, 0).1;
    c.record(
        "m6.a_title_and_an_action_bar_show_at_once",
        together.len() == 2
            && together[1].y == bar_y as f32 * px
            && together[1].px == px,
        format!(
            "two lines; the bar at y={bar_y} GUI px, unscaled (MUTATION: an \
             `else` would drop one of them)"
        ),
    );

    // m7 — the `if (alpha > 0)` guards.
    let mut t = TitleOverlay::default();
    t.set_times(0, 0, 1);
    t.set_title(rewo_proto::nbt::Nbt::String("x".into()));
    // titleTime = 1, fadeOut = 1: alpha = (1 - 1.0) * 255 = 0.
    let invisible = crate::live_cmd::title_lines(&t, advance, px, screen, 1.0, None);
    c.record(
        "m7.a_zero_alpha_frame_emits_nothing",
        invisible.is_empty(),
        "the draw's own `if (alpha > 0)`, separate from the clock's guard",
    );

    // m8 — the XP level number: five lines, four black at +/-1, one green, all
    // with `shadow = false`.
    let xp = ExperienceState {
        level: 7,
        ..ExperienceState::default()
    };
    let level = crate::live_cmd::experience_level_lines(&xp, true, None, advance, px, screen);
    // `-8323296` = `0xFF80FF20`, pushed through THIS FILE's own `srgb_decode`
    // rather than through `live_cmd`'s converter: the expectation must not be
    // computed by the function under test (M93q). Before M130 the line carried
    // the byte `/255`, so the number here was `rgb_f32`'s and the witness was
    // self-calibrating for the space as well as for the value.
    let want = rewo_gpu::hud::EXPERIENCE_LEVEL_COLOR;
    let green = [
        srgb_decode(((want >> 16) & 0xFF) as u8),
        srgb_decode(((want >> 8) & 0xFF) as u8),
        srgb_decode((want & 0xFF) as u8),
    ];
    let offsets: Vec<(f32, f32)> = level
        .iter()
        .map(|l| (l.x - level[4].x, l.y - level[4].y))
        .collect();
    c.record(
        "m8.the_level_number_is_five_shadowless_draws_with_a_four_way_outline",
        level.len() == 5
            && level.iter().all(|l| !l.shadow)
            && level[4]
                .color_linear
                .iter()
                .zip(green)
                .all(|(a, b)| (a - b).abs() < 1e-6)
            && level[..4].iter().all(|l| l.color_linear == [0.0, 0.0, 0.0])
            && offsets[..4].contains(&(px, 0.0))
            && offsets[..4].contains(&(-px, 0.0))
            && offsets[..4].contains(&(0.0, px))
            && offsets[..4].contains(&(0.0, -px)),
        format!(
            "offsets {offsets:?} (MUTATION: leaving `shadow = true` adds a dark \
             copy of every outline copy INSIDE the outline, thickening the glyph \
             instead of framing it)"
        ),
    );

    // m9 — the level number's gates.
    let zero = ExperienceState::default();
    let none_at_zero =
        crate::live_cmd::experience_level_lines(&zero, true, None, advance, px, screen);
    let none_in_creative =
        crate::live_cmd::experience_level_lines(&xp, false, None, advance, px, screen);
    c.record(
        "m9.the_level_number_needs_both_survival_and_a_level_above_zero",
        none_at_zero.is_empty() && none_in_creative.is_empty(),
        "`hasExperience() && experienceLevel > 0` — two separate gates, and \
         level 0 draws no number even in survival",
    );

    // m9b — a styled title carries all five `Style` flags, and is MEASURED
    // with bold rather than merely drawn with it (M130).
    //
    // `Hud.extractTitle` passes `this.title` — a `Component` — to
    // `textWithBackdrop`, which calls `graphics.text(font, str, …)`, which
    // takes `getVisualOrderText()`. There is no surface at which vanilla
    // honours a component's colour and drops its bold: `Font.PreparedTextBuilder
    // .accept` reads all five off the same `Style` it reads the colour off.
    //
    // The width half is the load-bearing one. `getBoldOffset()` is 1.0 charged
    // PER CHARACTER, so a five-character bold title is five pixels wider — and
    // `title_pos` centres by halving the width, so a style-blind measure is not
    // a subtle difference in the glyph, it is the whole line sitting two and a
    // half pixels off centre at 1x and ten at the title's 4x.
    let mut styled = TitleOverlay::default();
    styled.set_title(rewo_net::hud_state::read_component(&component_styled("BOLD")).unwrap());
    for _ in 0..15 {
        styled.tick();
    }
    let slines = crate::live_cmd::title_lines(&styled, advance, px, screen, 0.0, None);
    let plain_w = rewo_gpu::text::width("BOLD", advance);
    let (bold_x, _) = ref_title_pos(GUI_W, GUI_H, plain_w + 4);
    c.record(
        "m9b.a_styled_title_carries_the_five_flags_and_is_measured_with_bold",
        slines.len() == 1
            && slines[0].style
                == rewo_gpu::text::TextStyle {
                    bold: true,
                    italic: true,
                    underlined: true,
                    strikethrough: true,
                    obfuscated: true,
                }
            && (slines[0].x - bold_x as f32 * px).abs() < 1e-6,
        format!(
            "{:?} at x={} (bold width {} = plain {plain_w} + one px per \
             character; MUTATION: `TextStyle::PLAIN` drops all five, and \
             `width` instead of `width_styled` moves x by {} px)",
            slines[0].style,
            slines[0].x,
            plain_w + 4,
            (ref_title_pos(GUI_W, GUI_H, plain_w).0 - bold_x) * SCALE
        ),
    );

    // m9c — and a title with no styling still says so. The pair is what makes
    // m9b a claim about the WIRE rather than about a constant: a builder that
    // hard-coded every flag true would pass m9b alone.
    let mut bare = TitleOverlay::default();
    bare.set_title(rewo_proto::nbt::Nbt::String("BOLD".into()));
    for _ in 0..15 {
        bare.tick();
    }
    let blines = crate::live_cmd::title_lines(&bare, advance, px, screen, 0.0, None);
    c.record(
        "m9c.an_unstyled_title_is_plain_and_measured_without_bold",
        blines.len() == 1
            && blines[0].style == rewo_gpu::text::TextStyle::PLAIN
            && (blines[0].x - ref_title_pos(GUI_W, GUI_H, plain_w).0 as f32 * px).abs() < 1e-6,
        format!(
            "{:?} at x={} — the same string, four pixels wider and four flags \
             richer once the component carries them",
            blines[0].style, blines[0].x
        ),
    );

    // m10 — the gauges resolver.
    let mut hud = HudState::default();
    hud.experience.set_values(SetExperience {
        progress: 0.5,
        level: 12,
        total: 300,
    });
    let inv = rewo_world::inventory::Inventory::default();
    let g = crate::live_cmd::resolve_hud_gauges(&hud, &inv, items, true, 0.0);
    let creative = crate::live_cmd::resolve_hud_gauges(&hud, &inv, items, false, 0.0);
    c.record(
        "m10.the_gauges_carry_the_progress_and_the_level_curve",
        g.experience == Some(0.5)
            && g.xp_needed == 7 + 12 * 2
            && creative.experience.is_none()
            && creative.xp_needed == g.xp_needed,
        format!(
            "progress={:?} xpNeeded={} (creative -> {:?}); the curve is still \
             reported in creative because the *bar* is what `hasExperience` \
             gates, not the arithmetic",
            g.experience, g.xp_needed, creative.experience
        ),
    );

    // m11 — an empty hotbar has no cooldown sweep anywhere.
    c.record(
        "m11.an_empty_hotbar_sweeps_nothing",
        g.cooldowns.iter().all(|&v| v == 0.0),
        "a slot with no stack has no group, and `percent` on an absent group \
         is 0 — not a guess at the item id",
    );
}

// ---------------------------------------------------------------------------
// 3. The pixels.
// ---------------------------------------------------------------------------

/// The magenta detector. Nothing else the HUD draws is both red *and* blue:
/// hearts are red with a low blue, the hotbar and crosshair are neutral, the
/// XP bar is green. `p1` proves the frame carries none of it before a title
/// is set, which is what makes every count below the title and nothing else.
fn magenta(img: &[u8], x0: u32, y0: u32, x1: u32, y1: u32) -> u32 {
    let mut n = 0;
    for y in y0..y1.min(H) {
        for x in x0..x1.min(W) {
            let i = ((y * W + x) * 4) as usize;
            if img[i] > 150 && img[i + 2] > 150 && img[i + 1] < 90 {
                n += 1;
            }
        }
    }
    n
}

/// The brightest red byte in a window — the title's own channel.
fn max_red(img: &[u8], x0: u32, y0: u32, x1: u32, y1: u32) -> u8 {
    let mut m = 0u8;
    for y in y0..y1.min(H) {
        for x in x0..x1.min(W) {
            let i = ((y * W + x) * 4) as usize;
            m = m.max(img[i]);
        }
    }
    m
}

/// Green-dominant, measured on `g - r` because that is where the two XP
/// sprites separate with room to spare.
///
/// Over rows 1..3 of both 182x5 strips the **background**'s `g - r` spans
/// 7..13 and the **progress** sprite's spans 20..66, so a threshold of 16 sits
/// three clear of either side. The obvious alternative, `g - b`, spans 4..7
/// against 11..119: separable, but by two, and the progress sprite's own dark
/// right-edge column sits at the bottom of that range. Choosing the tighter
/// axis is how the first version of `p9` came up two GUI pixels short - it
/// stopped detecting the fill's last column.
fn green(img: &[u8], x0: u32, y0: u32, x1: u32, y1: u32) -> u32 {
    let mut n = 0;
    for y in y0..y1.min(H) {
        for x in x0..x1.min(W) {
            let i = ((y * W + x) * 4) as usize;
            let (r, g) = (img[i] as i32, img[i + 1] as i32);
            if g >= r + 16 {
                n += 1;
            }
        }
    }
    n
}

/// The rightmost column in a window carrying a green-dominant pixel.
fn green_right_edge(img: &[u8], x0: u32, y0: u32, x1: u32, y1: u32) -> Option<u32> {
    (x0..x1.min(W))
        .rev()
        .find(|&x| green(img, x, y0, x + 1, y1) > 0)
}

/// sRGB transfer, both directions — the attachment applies the encode on
/// store, so a linear-space prediction has to be pushed through it to be
/// compared against a byte.
fn srgb_encode(l: f32) -> f32 {
    if l <= 0.003_130_8 {
        l * 12.92
    } else {
        1.055 * l.powf(1.0 / 2.4) - 0.055
    }
}

fn srgb_decode(b: u8) -> f32 {
    let s = b as f32 / 255.0;
    if s <= 0.040_45 {
        s / 12.92
    } else {
        ((s + 0.055) / 1.055).powf(2.4)
    }
}

fn check_pixels(
    c: &mut Checker,
    args: &TitleshotArgs,
    baked: &assets::BakedAssets,
) -> Result<(), String> {
    let mut gpu = Gpu::new(None, true)?;
    println!(
        "[titleshot] Vulkan validation: {}",
        if gpu.validation_active { "ON" } else { "off" }
    );
    if args.check && !gpu.validation_active {
        return Err("titleshot: Vulkan validation requested but not active".into());
    }
    let mut off = Offscreen::new(&mut gpu, W, H)?;
    let white: Vec<u8> = vec![255u8; 16 * 16 * 4];
    let layers = [white];
    let mut wr = WorldRenderer::new(&mut gpu, off.format, 16, &layers)?;
    wr.set_sky_mode(SkyMode::None);
    let sprites = crate::live_cmd::hud_sprites(baked).ok_or("hud sprites missing from the jar")?;
    wr.init_hud(&mut gpu, &sprites)?;
    let font = crate::live_cmd::font_data(baked).ok_or("no baked font")?;
    wr.init_text(&mut gpu, &font)?;
    let advance = baked.font.as_ref().ok_or("no baked font")?.advance;

    // The GUI scale this whole file's coordinates assume, asserted rather than
    // trusted: at any other scale every predicted rect below is wrong.
    c.record(
        "p0.the_gui_scale_is_the_one_the_predictions_assume",
        rewo_gpu::hud::gui_scale(W as f32, H as f32) == SCALE as f32,
        format!("{W}x{H} -> scale {SCALE}, GUI space {GUI_W}x{GUI_H}"),
    );

    // The frame-time overlay is pushed off-screen: this gate measures the
    // HUD, and a strip chart in the corner would sit inside every window
    // below.
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

    let px = SCALE as f32;
    let screen = (W as f32, H as f32);

    // -- the title, in magenta ------------------------------------------------

    let mut shot = |gpu: &mut Gpu,
                    off: &mut Offscreen,
                    wr: &mut WorldRenderer,
                    lines: Vec<rewo_gpu::world::OwnedTextLine>,
                    gauges: HudGauges|
     -> Result<Vec<u8>, String> {
        wr.set_hud(20.0, 20, 0, gauges);
        wr.set_text(lines);
        off.render(gpu, Some((&mut *wr, vp)), &overlay_draw, clear)?;
        off.read_rgba(gpu)
    };

    let empty = shot(&mut gpu, &mut off, &mut wr, Vec::new(), HudGauges::default())?;
    c.record(
        "p1.a_frame_with_no_title_carries_no_magenta",
        magenta(&empty, 0, 0, W, H) == 0,
        "the hearts are red with a low blue, the hotbar and crosshair neutral, \
         so every magenta count below is the title and nothing else",
    );

    let mut t = TitleOverlay::default();
    let title = rewo_net::hud_state::read_component(&component_colored("TITLE", "#FF00FF")).unwrap();
    t.set_title(title.clone());
    // titleTime 100 is the very start of the fade-in, where alpha is 0 — tick
    // into the hold so the title is at full opacity.
    for _ in 0..15 {
        t.tick();
    }
    let full_alpha = ref_title_alpha(t.title_time, t.fade_in, t.stay, t.fade_out, 0.0);
    let lines = crate::live_cmd::title_lines(&t, &advance, px, screen, 0.0, None);
    let solid = shot(&mut gpu, &mut off, &mut wr, lines, HudGauges::default())?;

    let width = rewo_gpu::text::width("TITLE", &advance);
    let (tx, ty) = ref_title_pos(GUI_W, GUI_H, width);
    // The glyph cell is 8 font px, scaled by 4 (the title) and by 2 (the GUI).
    let cell = 8 * 4 * SCALE;
    let (px0, py0) = (tx * SCALE, ty * SCALE);
    let (px1, py1) = (px0 + width * 4 * SCALE, py0 + cell);
    let inside = magenta(
        &solid,
        px0.max(0) as u32,
        py0.max(0) as u32,
        (px1 + SCALE) as u32,
        (py1 + SCALE) as u32,
    );
    let total = magenta(&solid, 0, 0, W, H);
    c.record(
        "p2.every_title_pixel_lands_in_the_rect_the_placement_predicted",
        total > 200 && inside == total,
        format!(
            "{total} magenta px, all {inside} of them inside \
             ({px0},{py0})..({px1},{py1}) — the rect `title_pos` said, so the \
             model and the pass agree"
        ),
    );

    c.record(
        "p3.the_title_is_centred_and_above_the_middle",
        {
            let (mut sx, mut n) = (0u64, 0u64);
            for y in 0..H {
                for x in 0..W {
                    let i = ((y * W + x) * 4) as usize;
                    if solid[i] > 150 && solid[i + 2] > 150 && solid[i + 1] < 90 {
                        sx += x as u64;
                        n += 1;
                    }
                }
            }
            let cx = sx as f64 / n.max(1) as f64;
            (cx - W as f64 / 2.0).abs() < 8.0
                && magenta(&solid, 0, 0, W, H / 2) > magenta(&solid, 0, H / 2, W, H)
        },
        "the magenta centroid sits within eight pixels of the screen centre, \
         and the mass is above the midline (title_y = -10 at 4x)",
    );

    // p4 — the subtitle appears BELOW the midline when one is set, and the
    // title band is unchanged.
    t.set_subtitle(
        rewo_net::hud_state::read_component(&component_colored("SUB", "#FF00FF")).unwrap(),
    );
    let lines = crate::live_cmd::title_lines(&t, &advance, px, screen, 0.0, None);
    let with_sub = shot(&mut gpu, &mut off, &mut wr, lines, HudGauges::default())?;
    let title_band_before = magenta(&solid, 0, 0, W, H / 2);
    let title_band_after = magenta(&with_sub, 0, 0, W, H / 2);
    let sub_band = magenta(&with_sub, 0, H / 2, W, H);
    c.record(
        "p4.the_subtitle_lands_below_the_midline_and_leaves_the_title_alone",
        magenta(&solid, 0, H / 2, W, H) == 0
            && sub_band > 50
            && title_band_before == title_band_after,
        format!(
            "lower band 0 -> {sub_band} px while the title band stays at \
             {title_band_before} — the two are separate draws at separate scales"
        ),
    );

    // p4b -- and it is drawn at **2x**, not the title's 4x.
    //
    // The band count alone does not say so: a 4x subtitle is also "more than
    // fifty magenta pixels below the midline", and a mutation of
    // `SUBTITLE_SCALE` to 4 walked through the version that only counted. The
    // rect does say so, because both its width and its height double.
    let sub_w = rewo_gpu::text::width("SUB", &advance);
    let (sx, sy) = ref_subtitle_pos(GUI_W, GUI_H, sub_w);
    let inside_sub = magenta(
        &with_sub,
        (sx * SCALE).max(0) as u32,
        (sy * SCALE).max(0) as u32,
        ((sx + sub_w * 2) * SCALE + SCALE) as u32,
        ((sy + 8 * 2) * SCALE + SCALE) as u32,
    );
    c.record(
        "p4b.the_subtitle_is_drawn_at_two_times_not_the_titles_four",
        inside_sub == sub_band && sub_band > 50,
        format!(
            "all {sub_band} lower-band px sit inside the 2x rect at ({sx},{sy})              sized {}x{} GUI px (MUTATION: at 4x the glyphs spill past both              edges of it)",
            sub_w * 2,
            8 * 2
        ),
    );

    // p5 — the fade, predicted exactly in linear space.
    //
    // A fully-lit glyph texel composites over its own drop shadow over black:
    //     shadow: dst = 0.25 * a
    //     glyph:  dst = 1.0 * a + 0.25a * (1 - a)
    // and the attachment encodes that on store. Magenta is what makes the
    // prediction possible without a colour-space assumption: `rgb_f32` is a
    // plain /255, and 0 and 1 are the two values sRGB and linear agree on.
    let mut faded = TitleOverlay::default();
    faded.set_title(title);
    // Into the fade-out: titleTime 10 of a 20-tick fade-out.
    for _ in 0..90 {
        faded.tick();
    }
    let fade_alpha = ref_title_alpha(
        faded.title_time,
        faded.fade_in,
        faded.stay,
        faded.fade_out,
        0.0,
    );
    let lines = crate::live_cmd::title_lines(&faded, &advance, px, screen, 0.0, None);
    let dim = shot(&mut gpu, &mut off, &mut wr, lines, HudGauges::default())?;
    // Measured inside the title's OWN rect, which `p2` has just proved holds
    // every magenta pixel. The first version of this took the whole upper half
    // and read 255 at both alphas: the crosshair sits at the screen centre and
    // its top half is in that window, so the detector was measuring the
    // crosshair. Same shape as the three detector errors REWO_PLAN's M38 entry
    // collects - a signal measured against a background that already carries it.
    let (rx0, ry0) = (px0.max(0) as u32, py0.max(0) as u32);
    let (rx1, ry1) = ((px1 + SCALE) as u32, (py1 + SCALE) as u32);
    let observed_full = max_red(&solid, rx0, ry0, rx1, ry1);
    let observed_dim = max_red(&dim, rx0, ry0, rx1, ry1);
    let a = fade_alpha as f32 / 255.0;
    let predicted_linear = a + 0.25 * a * (1.0 - a);
    let predicted_byte = (srgb_encode(predicted_linear) * 255.0).round() as i32;
    c.record(
        "p5.the_title_is_opaque_in_the_hold",
        full_alpha == 255 && observed_full == 255,
        format!(
            "titleTime={} is past `fadeOut + stay`, alpha {full_alpha}, brightest \
             red {observed_full} — which also proves the glyph core texel is \
             fully opaque, so p6's prediction has a fully-lit sample to land on",
            t.title_time
        ),
    );
    c.record(
        "p6.the_fade_matches_the_linear_composite_it_predicts",
        (observed_dim as i32 - predicted_byte).abs() <= 2 && observed_dim < 250,
        format!(
            "titleTime={} alpha={fade_alpha}/255 -> linear {predicted_linear:.4} \
             -> byte {predicted_byte}, observed {observed_dim} \
             (MUTATION: computing the fade-in from the remaining `t` rather than \
             the elapsed count, or using `else if`, changes this alpha)",
            faded.title_time
        ),
    );
    c.record(
        "p6b.the_measurement_sits_where_the_ramp_bites",
        fade_alpha > 0 && fade_alpha < 255 && srgb_decode(observed_dim) < 0.99,
        format!(
            "alpha {fade_alpha} is strictly inside the ramp — a sample at 0 or \
             255 would pass for any ramp shape at all"
        ),
    );

    // p6c -- the FADE-IN, which p6 alone does not reach.
    //
    // The mutation battery found this gap rather than a reading did: replacing
    // the fade-in's `elapsed = total - t` with a bare `t` left every witness
    // green, because the only ramp sample was `titleTime = 10`, deep in the
    // fade-OUT branch. A sample must sit where the mutation bites (M76), and
    // the two branches are different code.
    //
    // `titleTime = 95` with 10/70/20 is five ticks into the fade-in:
    // `elapsed = 100 - 95 = 5`, `alpha = 5 * 255 / 10 = 127`. The mutation
    // reads `t * 255 / fadeIn` = 2422, which clamps to 255 -- an opaque title
    // where a nearly-invisible one belongs.
    let mut rising = TitleOverlay::default();
    rising.set_title(
        rewo_net::hud_state::read_component(&component_colored("TITLE", "#FF00FF")).unwrap(),
    );
    for _ in 0..5 {
        rising.tick();
    }
    let rise_alpha = ref_title_alpha(
        rising.title_time,
        rising.fade_in,
        rising.stay,
        rising.fade_out,
        0.0,
    );
    let lines = crate::live_cmd::title_lines(&rising, &advance, px, screen, 0.0, None);
    let rise = shot(&mut gpu, &mut off, &mut wr, lines, HudGauges::default())?;
    let observed_rise = max_red(&rise, rx0, ry0, rx1, ry1);
    let ra = rise_alpha as f32 / 255.0;
    let rise_predicted = (srgb_encode(ra + 0.25 * ra * (1.0 - ra)) * 255.0).round() as i32;
    c.record(
        "p6c.the_fade_in_ramps_from_the_elapsed_count_not_the_remaining_time",
        rising.title_time == 95
            && rise_alpha == 127
            && (observed_rise as i32 - rise_predicted).abs() <= 2,
        format!(
            "titleTime=95 -> alpha {rise_alpha}/255 -> byte {rise_predicted},              observed {observed_rise} (MUTATION: `t * 255 / fadeIn` reads 2422              here and clamps to 255)"
        ),
    );


    // p7 — the action bar sits low, and is a separate draw.
    let mut bar = TitleOverlay::default();
    bar.set_overlay_message(
        rewo_net::hud_state::read_component(&component_colored("BAR", "#FF00FF")).unwrap(),
        false,
    );
    let lines = crate::live_cmd::title_lines(&bar, &advance, px, screen, 0.0, None);
    let bar_frame = shot(&mut gpu, &mut off, &mut wr, lines, HudGauges::default())?;
    let (bx, by) = ref_action_bar_pos(GUI_W, GUI_H, rewo_gpu::text::width("BAR", &advance));
    let band_top = (by * SCALE).max(0) as u32;
    let band_bottom = ((by + 8) * SCALE + SCALE) as u32;
    c.record(
        "p7.the_action_bar_draws_in_its_own_band_low_on_the_screen",
        magenta(&bar_frame, 0, 0, W, band_top) == 0
            && magenta(&bar_frame, 0, band_top, W, band_bottom) > 30
            && magenta(&bar_frame, 0, band_bottom, W, H) == 0,
        format!(
            "all of it between y={band_top} and y={band_bottom}, i.e. GUI y={by} \
             = h - 68 - 4, with the x centred at {bx}"
        ),
    );

    // -- the XP bar -----------------------------------------------------------

    let bare = shot(&mut gpu, &mut off, &mut wr, Vec::new(), HudGauges::default())?;
    let (bar_left, bar_top) = ref_experience_bar_pos(GUI_W, GUI_H);
    // Rows 1..3 of the 5-row sprite: the interior, where the progress sprite's
    // green clears its background's by a wide margin. Rows 0 and 4 are both
    // sprites' dark edge and are deliberately excluded.
    let (row0, row1) = (
        ((bar_top + 1) * SCALE) as u32,
        ((bar_top + 4) * SCALE) as u32,
    );
    // **Restricted to the bar's own rows, and the restriction is load-bearing.**
    // A whole-frame green sweep is NOT a valid detector here: the hotbar
    // sprite's light frame highlight is `(221,240,216)`, a `g - r` of 19, so it
    // clears the threshold. The bar sits at GUI y 211..216 and the hotbar's top
    // is GUI y 218, so the window below is disjoint from it - which the second
    // half of this witness asserts rather than assumes.
    let hotbar_top_gui = GUI_H - 22;
    c.record(
        "p8.a_hud_with_no_xp_gauge_carries_no_green_in_the_bar_row",
        green(&bare, 0, row0, W, row1) == 0 && (row1 as i32) <= hotbar_top_gui * SCALE,
        format!(
            "rows {row0}..{row1} are clear, and end at or above the hotbar's top              ({}) - whose own highlight would otherwise read as green. The level              number is a TEXT line and this frame has none.",
            hotbar_top_gui * SCALE
        ),
    );

    let mut widths = Vec::new();
    for p in [0.0f32, 0.5, 1.0] {
        let g = HudGauges {
            experience: Some(p),
            xp_needed: 7,
            cooldowns: [0.0; 9],
        };
        let img = shot(&mut gpu, &mut off, &mut wr, Vec::new(), g)?;
        let edge = green_right_edge(&img, 0, row0, W, row1);
        widths.push((p, edge));
        if let Some(d) = &args.out_dir {
            std::fs::create_dir_all(d).map_err(|e| format!("out-dir: {e}"))?;
            let _ = off.save_png(&mut gpu, &d.join(format!("titleshot_xp_{p}.png")));
        }
    }
    let want = |gui_px: i32| (bar_left * SCALE + gui_px * SCALE - 1) as u32;
    c.record(
        "p9.the_progress_fill_ends_where_the_183_multiply_says",
        widths[0].1.is_none()
            && widths[1].1 == Some(want(91))
            && ref_experience_progress_px(0.5) == 91,
        format!(
            "0.0 -> no fill, 0.5 -> right edge {:?} = left + 91 GUI px, the \
             hand-computed `(int)(0.5 * 183)` (MUTATION: `* 182.0` gives 91 \
             here too -- p9b is where the two multiplies separate)",
            widths[1].1
        ),
    );
    // The 183 against a 182-wide sprite is vanilla's own overrun, and it is
    // NOT clamped: `blitSprite(.., 182, 5, 0, 0, left, top, progress, 5)`
    // computes `sprite.getU((textureX + width) / spriteWidth)` = `getU(183/182)`,
    // a UV past the sprite's right edge. In vanilla's *stitched* GUI atlas that
    // samples whatever was packed next; in Rewo's fixed layout it samples
    // transparent and is discarded. So the geometry is 183 GUI px wide and the
    // drawn fill stops at the sprite's own 182nd column, one short of it --
    // which is the honest reading of an unclamped overrun rather than a claim
    // about which neighbour vanilla happens to pick up.
    c.record(
        "p9b.the_183rd_column_samples_past_the_sprite_and_draws_nothing",
        rewo_gpu::hud::experience_progress_px(1.0) == 183
            && ref_experience_progress_px(1.0) == 183
            && widths[2].1 == Some(want(182))
            && widths[2].1 != Some(want(183)),
        format!(
            "progress_px(1.0)={} but the fill reaches {:?}, i.e. left + 182 GUI \
             px -- the sprite's full width and no further",
            rewo_gpu::hud::experience_progress_px(1.0),
            widths[2].1
        ),
    );

    // p9c -- the bar's VERTICAL placement, which the fill's right edge cannot
    // see. Mutating `MARGIN_BOTTOM` from 24 to 22 moved the whole bar two GUI
    // rows down and every witness stayed green, because a horizontal edge does
    // not move when the bar does. The measurement window has to be one the
    // mutation can leave.
    let full = HudGauges {
        experience: Some(1.0),
        xp_needed: 7,
        cooldowns: [0.0; 9],
    };
    let full_img = shot(&mut gpu, &mut off, &mut wr, Vec::new(), full)?;
    let rows: Vec<u32> = (0..(hotbar_top_gui * SCALE) as u32)
        .filter(|&y| green(&full_img, 0, y, W, y + 1) > 0)
        .collect();
    let want_rows = (
        Some((bar_top * SCALE) as u32),
        Some(((bar_top + 5) * SCALE - 1) as u32),
    );
    c.record(
        "p9c.the_bar_occupies_the_five_rows_its_top_and_height_predict",
        (rows.first().copied(), rows.last().copied()) == want_rows
            && rows.len() as i32 == 5 * SCALE,
        format!(
            "green rows {:?}..{:?} ({} of them) against the predicted {:?} -              `top = h - 24 - 5` and a five-row sprite, measured above the              hotbar so its own highlight cannot enter the window",
            rows.first(),
            rows.last(),
            rows.len(),
            want_rows
        ),
    );

    let starved = HudGauges {
        experience: Some(1.0),
        xp_needed: 0,
        cooldowns: [0.0; 9],
    };
    let img = shot(&mut gpu, &mut off, &mut wr, Vec::new(), starved)?;
    c.record(
        "p10.a_non_positive_xp_requirement_draws_no_bar_at_all",
        green(&img, 0, row0, W, row1) == 0,
        "`if (xpNeededForNextLevel > 0)` wraps the background blit too, not just \
         the fill — so the frame is empty rather than showing an empty frame",
    );

    // -- the cooldown wash ----------------------------------------------------

    // **The slot-to-slot control was wrong and the witness said so.** The plan
    // was to lean on the hotbar sprite repeating every 20 GUI px, so one slot's
    // interior would be a pixel-exact control for its neighbour's. It is not:
    // the sprite is dithered, and two slots' interior columns differ by a few
    // bytes in scattered rows (`(4,4,4)` against `(6,6,4)`, `(29,32,4)` against
    // `(39,37,5)`). The first version also picked slot 0, which carries the
    // selection frame, and failed for that reason as well.
    //
    // What replaces it is an absolute detector, which is stronger: every slot
    // interior in the sprite is dark (its brightest byte is 51 at alpha 186, so
    // under 60 composited over black) and every washed pixel is at least
    // `0.498` linear by construction, i.e. a byte of 187 or more. A threshold
    // of 150 sits between them with room either side, and `p11` proves the
    // untouched frame carries nothing above it -- the same "an empty frame
    // contains none of the subject" shape `p1` uses for the title.
    // The same helper the item pass and the HUD both call, so the gate cannot
    // measure a slot the renderer did not draw (M45's rule).
    let slots = rewo_gpu::hud::hotbar_slot_rects(182.0, 22.0, W as f32, H as f32);
    // Slots **1 and 2**, not 0 and 1: `set_hud(.., slot = 0, ..)` paints the
    // selection frame over slot 0, so it is the one slot in the row that is not
    // a copy of its neighbours. The first version used 0 and 1, the control
    // failed - correctly - and it took two more witnesses down with it, because
    // every row of slot 0 differs from slot 1 whatever the cooldown does.
    const CD_SLOT: usize = 1;
    const CTRL_SLOT: usize = 2;
    let col0 = (slots[CD_SLOT].0 + slots[CD_SLOT].2 / 2.0) as u32;
    let col1 = (slots[CTRL_SLOT].0 + slots[CTRL_SLOT].2 / 2.0) as u32;
    let slot_top = slots[CD_SLOT].1 as u32;
    let slot_bottom = (slots[CD_SLOT].1 + slots[CD_SLOT].2) as u32;
    /// Half-transparent white over a dark slot lands at 187 or above; the
    /// brightest byte the hotbar sprite itself can put in a slot interior is
    /// well under 60.
    const WASH_MIN: u8 = 150;
    let washed = |img: &[u8], col: u32, y: u32| -> bool {
        img[((y * W + col) * 4) as usize] >= WASH_MIN
    };
    let bare_max = (slot_top..slot_bottom)
        .flat_map(|y| [col0, col1].map(move |c| (c, y)))
        .map(|(c, y)| bare[((y * W + c) * 4) as usize])
        .max()
        .unwrap_or(0);
    c.record(
        "p11.an_unwashed_slot_carries_nothing_the_detector_can_see",
        bare_max < WASH_MIN,
        format!(
            "the brightest byte in columns {col0}/{col1} over rows              {slot_top}..{slot_bottom} is {bare_max}, against a {WASH_MIN}              threshold - so every row counted below is the wash and nothing else"
        ),
    );

    for (cooldown, name, want_off) in [
        (1.0f32, "p12", (0i32, 16i32)),
        (0.5, "p13", (8, 16)),
        (0.25, "p14", (12, 16)),
    ] {
        let mut cd = [0.0f32; 9];
        cd[CD_SLOT] = cooldown;
        let g = HudGauges {
            experience: None,
            xp_needed: 7,
            cooldowns: cd,
        };
        let img = shot(&mut gpu, &mut off, &mut wr, Vec::new(), g)?;
        let differs: Vec<u32> = (slot_top..slot_bottom)
            .filter(|&y| washed(&img, col0, y))
            .collect();
        // The offsets are hand-computed literals in the loop table above,
        // cross-checked against this file's own transcription, and the
        // production function is graded against them rather than consulted for
        // them. That is the whole difference between this witness and the
        // version the mutation battery walked straight through.
        let icon_top_gui = slots[CD_SLOT].1 as i32 / SCALE;
        // `m0` has already graded the production offsets against these same
        // hand-computed literals over the whole 0..1 range, so this uses the
        // literal directly and consults nothing under test.
        let (want_top, want_bottom) = (icon_top_gui + want_off.0, icon_top_gui + want_off.1);
        let observed = (differs.first().copied(), differs.last().copied());
        let contiguous = differs
            .windows(2)
            .all(|w| w[1] == w[0] + 1);
        c.record(
            &format!("{name}.the_cooldown_wash_covers_the_bottom_of_the_slot_at_{cooldown}"),
            !differs.is_empty()
                && contiguous
                && observed.0.map(|v| v as i32) == Some(want_top * SCALE)
                && observed.1.map(|v| v as i32) == Some(want_bottom * SCALE - 1),
            format!(
                "rows {observed:?} vs the predicted {}..{} \
                 (MUTATION: `top = y` with height `16 * cooldown` draws it \
                 growing DOWN from the top and inverts both edges)",
                want_top * SCALE,
                want_bottom * SCALE - 1
            ),
        );
        if let Some(d) = &args.out_dir {
            let _ = off.save_png(&mut gpu, &d.join(format!("titleshot_cd_{cooldown}.png")));
        }
    }

    // p15 — the wash brightens rather than replacing: half-transparent white
    // over the slot, not opaque white.
    let mut cd = [0.0f32; 9];
    cd[CD_SLOT] = 1.0;
    let g = HudGauges {
        experience: None,
        xp_needed: 7,
        cooldowns: cd,
    };
    let wash_frame = shot(&mut gpu, &mut off, &mut wr, Vec::new(), g)?;
    let probe = slot_top + 4;
    let i = ((probe * W + col0) * 4) as usize;
    let under = ((probe * W + col1) * 4) as usize;
    let base = srgb_decode(bare[under]);
    let seen = srgb_decode(wash_frame[i]);
    let alpha = (rewo_gpu::hud::COOLDOWN_OVERLAY_ARGB >> 24) as f32 / 255.0;
    let predicted = alpha + base * (1.0 - alpha);
    c.record(
        "p15.the_wash_is_half_transparent_white_not_opaque_white",
        (seen - predicted).abs() < 0.03 && wash_frame[i] < 255 && wash_frame[i] > bare[under],
        format!(
            "base {:.3} + {alpha:.3} alpha white -> predicted {predicted:.3}, \
             observed {seen:.3} (MUTATION: reading `Integer.MAX_VALUE` as opaque \
             white pins this at 1.0 and hides the icon under it)",
            base
        ),
    );

    // p16 — the sweep is per slot.
    let mut cd = [0.0f32; 9];
    cd[5] = 1.0;
    let g = HudGauges {
        experience: None,
        xp_needed: 7,
        cooldowns: cd,
    };
    let one_slot = shot(&mut gpu, &mut off, &mut wr, Vec::new(), g)?;
    let col5 = (slots[5].0 + slots[5].2 / 2.0) as u32;
    let lit = |img: &[u8], col: u32| -> bool {
        (slot_top..slot_bottom).any(|y| washed(img, col, y))
    };
    c.record(
        "p16.the_sweep_lands_on_the_slot_it_was_indexed_by",
        lit(&one_slot, col5) && !lit(&one_slot, col0) && !lit(&one_slot, col1),
        "slot 5 washed, slots 1 and 2 untouched — the 20 GUI px slot pitch",
    );

    if let Some(d) = &args.out_dir {
        std::fs::create_dir_all(d).map_err(|e| format!("out-dir: {e}"))?;
        let _ = std::fs::write(d.join("titleshot_note.txt"), "M79 titleshot frames\n");
    }

    wr.destroy(&mut gpu);
    off.destroy(&mut gpu);
    Ok(())
}
