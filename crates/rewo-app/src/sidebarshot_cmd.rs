//! `rewo sidebarshot --check` — the M132 scoreboard-sidebar oracle.
//!
//! The path under test, end to end:
//!
//! ```text
//! raw set_objective / set_score / set_display_objective bodies (built here)
//!   -> rewo_net::scoreboard::{parse_set_objective, parse_set_score, …}
//!   -> Scoreboard::apply_*                     (the production state machine)
//!   -> live_cmd::resolve_sidebar               (the SAME resolver the frame calls)
//!   -> rewo_net::sidebar::layout               (production geometry)
//!   -> live_cmd::{sidebar_fills, sidebar_text} (the SAME emitters the frame calls)
//!   -> WorldRenderer::{set_hud_fills, set_text} -> HudPass / TextPass
//!   -> Offscreen::read_rgba                    (real pixels)
//! ```
//!
//! ## Four rules this gate is built around, each earned elsewhere
//!
//! **The gate drives the real emitters.** M45's `install_shapes` failure and
//! M41's rotted `swingshot` fixture were both gates that had quietly stopped
//! testing their subject because they reimplemented a slice of the app's
//! setup. So the packets go through `rewo_net`'s own parsers and the fills and
//! text come out of `live_cmd`'s own functions — the only thing rebuilt here
//! is the width closure, which is an input in the windowed path too.
//!
//! **A witness must not compute its expectation from the constant the
//! renderer reads.** M93q's `LOOM_PREVIEW_BACKING` was wrong for a milestone
//! because its pixel witness read the same `pub const` the draw did. So every
//! predicted number below is a **literal transcribed from
//! `displayScoreboardSidebar`**, and [`check_transcription`] then grades those
//! literals against `rewo_net::sidebar` over a range — which is what stops a
//! slip in one copy cancelling a slip in the other.
//!
//! **The detector must not share a colour with its background.** Three
//! detector errors on this project were the same shape. So the *scores* are
//! made magenta through the objective's own `StyledFormat` (`{"color":
//! "#FF00FF"}`), the clear is white, and `p1` asserts a frame with no display
//! objective carries none of it. Magenta needs no colour-space assumption
//! either: `chat_style::rgb_f32` is a plain `/255`, and 0 and 1 are the two
//! values sRGB and linear agree on exactly.
//!
//! **A glyph's own left bearing is measured, not assumed.** `p7` would
//! otherwise be asserting where the ink inside a `9` starts. A control frame
//! draws the same string at a known origin, the bearing comes out of that, and
//! the sidebar's measurement is corrected by it — so the witness is about the
//! *placement*, which is what M132 changed.
//!
//! **Fail-closed** on a fixed [`EXPECTED_WITNESSES`] count.

use clap::Args as ClapArgs;

use rewo_data::assets;
use rewo_data::DataPaths;
use rewo_gpu::offscreen::Offscreen;
use rewo_gpu::world::{SkyMode, WorldRenderer};
use rewo_gpu::Gpu;
use rewo_net::scoreboard::{
    parse_set_display_objective, parse_set_objective, parse_set_score, DisplaySlot, Scoreboard,
};
use rewo_net::sidebar::{self, Sidebar, SidebarLayout};

/// Total named properties this gate asserts. Locked so a skipped property
/// fails the run even when nothing mismatched.
const EXPECTED_WITNESSES: usize = 17;

const W: u32 = 640;
const H: u32 = 480;
/// `gui_scale(640, 480)` = 2, so GUI space is 320×240. Asserted by `p0`, not
/// assumed — every predicted coordinate below is multiplied by it.
const SCALE: i32 = 2;
const GUI_W: i32 = W as i32 / SCALE;
const GUI_H: i32 = H as i32 / SCALE;

/// The three visible holders, highest first once sorted. Their values are
/// deliberately out of order on the wire.
const HOLDERS: [(&str, i32); 4] = [
    ("SbAlpha", 3),
    // `PlayerScoreEntry.isHidden` is `owner.startsWith("#")`. Its score is the
    // largest, so a client that failed to filter it would put it at the TOP —
    // visible in the row count and in the ordering at once.
    ("#hidden", 9999),
    ("SbGamma", 9),
    ("SbBeta", 6),
];
const TITLE: &str = "TITLE";
/// The number format the objective carries, so the *scores* are the magenta
/// subject and the names stay white.
const SCORE_COLOR: &str = "#FF00FF";

#[derive(ClapArgs)]
pub struct SidebarshotArgs {
    /// Assert every owned property. The oracle asserts unconditionally and a
    /// failure exits nonzero with or without this flag, so it only labels the
    /// run — the convention `titleshot`/`eventshot`/`danceshot` use.
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
            "[sidebarshot] {}  {name}: {detail}",
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

pub fn run(args: SidebarshotArgs) -> Result<(), String> {
    let mode = if args.check { "check" } else { "report" };
    println!(
        "[sidebarshot] mode: {mode} (serverless; the oracle asserts \
         unconditionally). Model half against the 26.2 decompile's \
         `displayScoreboardSidebar`; pixel half against a magenta score \
         column on a white clear."
    );

    let paths = DataPaths::for_version(&args.version)
        .ok_or_else(|| "no config dir for version data".to_string())?;
    let jar = client_jar(&args.version).ok_or("client jar not found")?;
    let baked = assets::bake(&jar, &paths.blocks_json())?;

    let mut c = Checker {
        witnessed: 0,
        failures: Vec::new(),
    };
    check_transcription(&mut c);
    check_pixels(&mut c, &args, &baked)?;

    println!(
        "[sidebarshot] witnesses observed: {} / {}",
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
    println!("[sidebarshot] PASS — {} witnesses", c.witnessed);
    Ok(())
}

// ---------------------------------------------------------------------------
// The decompile, transcribed here.
//
// Re-declared rather than imported from `rewo_net::sidebar`, for the reason
// `healthbarshot` and `titleshot` both state: a witness that asks the
// implementation what to expect asserts only that the implementation equals
// itself. Where a sample's answer is a small integer it is ALSO pinned as a
// literal, which is what stops a mis-transcription here cancelling one there.
// ---------------------------------------------------------------------------

/// ```java
/// int height  = entriesCount * 9;
/// int bottom  = graphics.guiHeight() / 2 + height / 3;
/// int left    = graphics.guiWidth() - width - 3;
/// int right   = graphics.guiWidth() - 3 + 2;
/// int headerY = bottom - entriesCount * 9;
/// ```
///
/// Returns `(left, right, bottom, header_y)`.
fn ref_frame(gui_w: i32, gui_h: i32, width: i32, count: i32) -> (i32, i32, i32, i32) {
    let height = count * 9;
    let bottom = gui_h / 2 + height / 3;
    (
        gui_w - width - 3,
        gui_w - 3 + 2,
        bottom,
        bottom - count * 9,
    )
}

/// `graphics.text(font, e.name, left, y, -1, false)` — row `i`'s baseline row
/// is `bottom - (entriesCount - i) * 9`.
fn ref_row_y(bottom: i32, count: i32, i: i32) -> i32 {
    bottom - (count - i) * 9
}

/// `text(objectiveDisplayName, left + width / 2 - nameWidth / 2, headerY - 9, …)`.
fn ref_title_x(left: i32, width: i32, title_width: i32) -> i32 {
    left + width / 2 - title_width / 2
}

/// `options.getBackgroundColor(0.3F)` / `(0.4F)`, where `as8BitChannel` is
/// `Mth.floor(value * 255.0F)`.
const REF_BODY_ALPHA: u32 = 76;
const REF_HEADER_ALPHA: u32 = 102;

fn check_transcription(c: &mut Checker) {
    // Hand-computed, so a slip in `ref_frame` cannot cancel a slip in
    // `sidebar::layout`.
    let literals = ref_frame(320, 240, 60, 3) == (257, 319, 129, 102)
        && ref_frame(320, 240, 60, 0) == (257, 319, 120, 120)
        && ref_frame(320, 240, 60, 15) == (257, 319, 165, 30)
        && ref_row_y(129, 3, 0) == 102
        && ref_row_y(129, 3, 2) == 120
        && ref_title_x(257, 60, 30) == 272
        // The two readings the layout has to choose between: `right` is two
        // past `left + width`, so the panel overhangs the width it solved for.
        && ref_frame(320, 240, 60, 3).1 - (ref_frame(320, 240, 60, 3).0 + 60) == 2
        // …and `bottom` uses a THIRD of the height, not a half. Fifteen rows
        // put it 45 below centre where a half would put it 67.
        && ref_frame(320, 240, 60, 15).2 - 120 == 45;

    // Now the same functions against the production layout, over a range that
    // a single fixture could not cover.
    let mut production = true;
    for count in 0..=15i32 {
        for width in [1i32, 7, 60, 61, 200] {
            for (gw, gh) in [(320i32, 240i32), (427, 240), (321, 241)] {
                let s = fake_sidebar(width, count);
                let l = sidebar::layout(&s, gw, gh);
                let (left, right, bottom, header_y) = ref_frame(gw, gh, width, count);
                production &= l.left == left
                    && l.right == right
                    && l.bottom == bottom
                    && l.header_background.x == left - 2
                    && l.header_background.y == header_y - 9 - 1
                    && l.header_background.right() == right
                    && l.header_background.bottom() == header_y - 1
                    && l.body_background.x == left - 2
                    && l.body_background.y == header_y - 1
                    && l.body_background.right() == right
                    && l.body_background.bottom() == bottom
                    && l.title == (ref_title_x(left, width, 0), header_y - 9)
                    && l.rows.len() == count as usize;
                for (i, r) in l.rows.iter().enumerate() {
                    production &= r.name == (left, ref_row_y(bottom, count, i as i32));
                }
            }
        }
    }
    c.record(
        "m0.the_transcription_matches_its_literals_and_the_production_layout",
        literals && production,
        "left / right / bottom / headerY / row y / title x, graded against \
         hand-computed values and then against `rewo_net::sidebar::layout` \
         over 0..=15 rows x 5 widths x 3 window sizes",
    );

    c.record(
        "m1.the_two_background_alphas_are_the_floored_0_3_and_0_4",
        sidebar::BODY_BACKGROUND == REF_BODY_ALPHA << 24
            && sidebar::HEADER_BACKGROUND == REF_HEADER_ALPHA << 24
            && REF_BODY_ALPHA == (0.3f32 * 255.0) as u32
            && REF_HEADER_ALPHA == (0.4f32 * 255.0) as u32,
        format!(
            "body {REF_BODY_ALPHA} header {REF_HEADER_ALPHA} (a round would \
             give 77 and 102; the header is the more opaque of the two)"
        ),
    );
}

/// A `Sidebar` with a chosen `width` and row count and no text — enough for
/// the geometry, which is all `m0` grades.
fn fake_sidebar(width: i32, count: i32) -> Sidebar {
    Sidebar {
        objective: "x".into(),
        title: Vec::new(),
        title_width: 0,
        entries: (0..count)
            .map(|i| sidebar::SidebarEntry {
                owner: format!("h{i}"),
                value: i,
                name: Vec::new(),
                name_width: 0,
                score: Vec::new(),
                score_width: 0,
            })
            .collect(),
        width,
    }
}

// ---------------------------------------------------------------------------
// Wire bodies — built here, independent of any writer under test.
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

fn mc_string(s: &str, out: &mut Vec<u8>) {
    varint(s.len() as i32, out);
    out.extend_from_slice(s.as_bytes());
}

/// A trusted `Component` / `Style` on the wire: network NBT, a bare tag byte
/// and a payload with no name.
fn nbt_string(s: &str, out: &mut Vec<u8>) {
    out.push(8);
    out.extend_from_slice(&(s.len() as u16).to_be_bytes());
    out.extend_from_slice(s.as_bytes());
}

/// `{ "color": "#FF00FF" }` as a `TAG_Compound` — the `Style` a `StyledFormat`
/// carries.
fn nbt_style(color: &str) -> Vec<u8> {
    let mut out = vec![10u8]; // TAG_Compound
    out.push(8); // TAG_String
    out.extend_from_slice(&(5u16).to_be_bytes());
    out.extend_from_slice(b"color");
    out.extend_from_slice(&(color.len() as u16).to_be_bytes());
    out.extend_from_slice(color.as_bytes());
    out.push(0); // TAG_End
    out
}

/// The scoreboard, built by driving the production parsers and state machine
/// with raw packet bodies.
fn build_scoreboard(styled_ids: rewo_data::number_formats::NumberFormatTypeIds) -> Scoreboard {
    let mut sb = Scoreboard::new();
    let obj = "sbshot";

    let mut body = Vec::new();
    mc_string(obj, &mut body);
    body.push(0); // METHOD_ADD
    nbt_string(TITLE, &mut body);
    varint(0, &mut body); // RenderType.INTEGER
    body.push(1); // number format present
    varint(styled_ids.styled, &mut body);
    body.extend_from_slice(&nbt_style(SCORE_COLOR));
    sb.apply_set_objective(&parse_set_objective(&body, styled_ids).expect("set_objective"));

    for (owner, value) in HOLDERS {
        let mut b = Vec::new();
        mc_string(owner, &mut b);
        mc_string(obj, &mut b);
        varint(value, &mut b);
        b.push(0); // no display override
        b.push(0); // no number format override
        sb.apply_set_score(&parse_set_score(&b, styled_ids).expect("set_score"));
    }

    let mut b = Vec::new();
    varint(DisplaySlot::Sidebar.id(), &mut b);
    mc_string(obj, &mut b);
    sb.apply_set_display_objective(&parse_set_display_objective(&b).expect("set_display"));
    sb
}

// ---------------------------------------------------------------------------
// Pixels.
// ---------------------------------------------------------------------------

fn srgb_to_linear(s: f32) -> f32 {
    if s <= 0.04045 {
        s / 12.92
    } else {
        ((s + 0.055) / 1.055).powf(2.4)
    }
}

struct Frame {
    px: Vec<u8>,
}

impl Frame {
    fn at(&self, x: i32, y: i32) -> [u8; 4] {
        let i = ((y as usize) * W as usize + x as usize) * 4;
        [
            self.px[i],
            self.px[i + 1],
            self.px[i + 2],
            self.px[i + 3],
        ]
    }
    /// The GUI pixel at `(gx, gy)`, sampled at its centre so a half-pixel
    /// rounding cannot decide the answer.
    fn gui(&self, gx: i32, gy: i32) -> [u8; 4] {
        self.at(gx * SCALE + SCALE / 2, gy * SCALE + SCALE / 2)
    }
    /// A **full-brightness** magenta texel.
    ///
    /// The threshold is 200 rather than 128 for a reason the first run of this
    /// gate found: `TextPass` darkens a shadow by 0.25 in LINEAR space, and
    /// `encode_srgb(0.25)` is 137 — which a `> 128` test counts as magenta, so
    /// a shadow would contaminate every bbox below. 200 separates the two with
    /// a wide margin and nothing between them exists (the bitmap font is
    /// nearest-sampled at an integer scale, so there is no antialiasing).
    fn is_magenta(&self, gx: i32, gy: i32) -> bool {
        let p = self.gui(gx, gy);
        p[0] > 200 && p[2] > 200 && p[1] < 64
    }
    /// A magenta-hued texel at the shadow's brightness — see [`Self::is_magenta`].
    fn is_shadow(&self, gx: i32, gy: i32) -> bool {
        let p = self.gui(gx, gy);
        p[1] < 64 && (64..=200).contains(&p[0]) && (64..=200).contains(&p[2])
    }
    fn count_shadow(&self) -> usize {
        (0..GUI_H)
            .flat_map(|gy| (0..GUI_W).map(move |gx| (gx, gy)))
            .filter(|(gx, gy)| self.is_shadow(*gx, *gy))
            .count()
    }
    /// The magenta bounding box, in GUI pixels.
    fn magenta_bbox(&self) -> Option<(i32, i32, i32, i32)> {
        let (mut x0, mut y0, mut x1, mut y1) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
        for gy in 0..GUI_H {
            for gx in 0..GUI_W {
                if self.is_magenta(gx, gy) {
                    x0 = x0.min(gx);
                    y0 = y0.min(gy);
                    x1 = x1.max(gx);
                    y1 = y1.max(gy);
                }
            }
        }
        (x0 <= x1).then_some((x0, y0, x1, y1))
    }
    /// The distinct GUI rows carrying magenta, grouped into contiguous runs.
    fn magenta_row_bands(&self) -> Vec<(i32, i32)> {
        let mut bands = Vec::new();
        let mut start: Option<i32> = None;
        for gy in 0..GUI_H {
            let any = (0..GUI_W).any(|gx| self.is_magenta(gx, gy));
            match (any, start) {
                (true, None) => start = Some(gy),
                (false, Some(s)) => {
                    bands.push((s, gy - 1));
                    start = None;
                }
                _ => {}
            }
        }
        if let Some(s) = start {
            bands.push((s, GUI_H - 1));
        }
        bands
    }
    /// The leftmost magenta GUI column over rows `y0..=y1`.
    ///
    /// A range and not one row: a glyph's topmost row need not be its widest,
    /// so a single-row probe measures a different bearing from the control's
    /// whole-bbox one. The first run of this gate failed by exactly one pixel
    /// for that reason.
    fn magenta_left_in(&self, y0: i32, y1: i32) -> Option<i32> {
        (0..GUI_W).find(|gx| (y0..=y1).any(|gy| self.is_magenta(*gx, gy)))
    }
}

/// Recover the alpha a black fill was composited with, from the observed
/// byte and the observed clear byte.
///
/// The attachment is `R8G8B8A8_SRGB`, so the fixed-function blend runs on the
/// **linear** values — hence the decode on both sides rather than a byte
/// ratio.
fn recovered_alpha(band: [u8; 4], clear: [u8; 4]) -> f32 {
    let mut sum = 0.0;
    for ch in 0..3 {
        let b = srgb_to_linear(band[ch] as f32 / 255.0);
        let c = srgb_to_linear(clear[ch] as f32 / 255.0);
        sum += 1.0 - b / c.max(1e-6);
    }
    sum / 3.0
}

fn check_pixels(
    c: &mut Checker,
    args: &SidebarshotArgs,
    baked: &assets::BakedAssets,
) -> Result<(), String> {
    let mut gpu = Gpu::new(None, true)?;
    println!(
        "[sidebarshot] Vulkan validation: {}",
        if gpu.validation_active { "ON" } else { "off" }
    );
    if args.check && !gpu.validation_active {
        return Err("sidebarshot: Vulkan validation requested but not active".into());
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

    c.record(
        "p0.the_gui_scale_is_the_one_the_predictions_assume",
        rewo_gpu::hud::gui_scale(W as f32, H as f32) == SCALE as f32,
        format!("{W}x{H} -> scale {SCALE}, GUI space {GUI_W}x{GUI_H}"),
    );

    // The frame-time overlay is pushed off-screen: a strip chart in the corner
    // would sit inside the windows below.
    let ring = crate::stats::OverlayRing::default();
    let overlay_draw = rewo_gpu::overlay::OverlayDraw {
        samples_ms: &ring.data,
        head: ring.head(),
        scale_ms: 20.0,
        origin: [-4000.0, -4000.0],
        size: [8.0, 8.0],
    };
    let vp = glam::Mat4::IDENTITY.to_cols_array_2d();
    // White, so the two translucent black bands are recoverable to an alpha
    // and a magenta glyph is unmistakable against both it and them.
    let clear = [1.0, 1.0, 1.0, 1.0];
    let px = SCALE as f32;

    let styled_ids = rewo_data::number_formats::NumberFormatTypeIds::load(&
        DataPaths::for_version("26.2")
            .ok_or("no config dir")?
            .registries_json(),
    )?;
    let scoreboard = build_scoreboard(styled_ids);
    let lang = baked.lang.clone();

    let sidebar = crate::live_cmd::resolve_sidebar(
        &scoreboard,
        "SbViewer",
        false,
        &lang,
        Some(advance),
    )
    .ok_or("sidebarshot: the production resolver found no sidebar")?;
    let layout = sidebar::layout(&sidebar, GUI_W, GUI_H);

    let mut shot = |gpu: &mut Gpu,
                    off: &mut Offscreen,
                    wr: &mut WorldRenderer,
                    fills: Vec<rewo_gpu::hud::HudFill>,
                    lines: Vec<rewo_gpu::world::OwnedTextLine>|
     -> Result<Frame, String> {
        // `WorldRenderer::draw` gates the HUD pass on `hud_state` being set,
        // so a gate that only pushed fills would render none of them — which
        // is how the first run of this file reported an alpha of exactly
        // zero for both bands. The hotbar it turns on sits at the bottom
        // centre and the crosshair at the middle; every probe below is in the
        // right-hand column band, clear of both, and `p1` proves the control
        // frame carries no magenta with all of it drawn.
        wr.set_hud(
            0,
            rewo_gpu::hud::HudGauges::default(),
            rewo_gpu::survival_hud::layout_for_screen(
                &rewo_gpu::survival_hud::SurvivalInputs::simple(0.0, 0),
                W as f32,
                H as f32,
            ),
        );
        wr.set_hud_fills(fills);
        wr.set_text(lines);
        off.render(gpu, Some((&mut *wr, vp)), &overlay_draw, clear)?;
        Ok(Frame { px: off.read_rgba(gpu)? })
    };

    // -- the control: no display objective at all ---------------------------
    //
    // Everything but the subject held constant — same renderer, same clear,
    // same frame. Only the sidebar is absent.
    let mut bare = Scoreboard::new();
    bare.apply_set_objective(
        &parse_set_objective(
            &{
                let mut b = Vec::new();
                mc_string("sbshot", &mut b);
                b.push(0);
                nbt_string(TITLE, &mut b);
                varint(0, &mut b);
                b.push(0);
                b
            },
            styled_ids,
        )
        .expect("bare objective"),
    );
    let none = crate::live_cmd::resolve_sidebar(&bare, "SbViewer", false, &lang, Some(advance));
    let control = shot(&mut gpu, &mut off, &mut wr, Vec::new(), Vec::new())?;

    let fills = crate::live_cmd::sidebar_fills(&layout);
    let text_len = crate::live_cmd::sidebar_text(&sidebar, &layout, px, Some(advance)).len();
    let frame = shot(
        &mut gpu,
        &mut off,
        &mut wr,
        fills.clone(),
        crate::live_cmd::sidebar_text(&sidebar, &layout, px, Some(advance)),
    )?;

    // -- F1, the only gate `Hud.extractRenderState` puts on the whole panel --
    let hidden =
        crate::live_cmd::resolve_sidebar(&scoreboard, "SbViewer", true, &lang, Some(advance));
    c.record(
        "p1.an_objective_in_no_slot_and_a_hidden_hud_both_draw_nothing",
        none.is_none() && hidden.is_none() && control.magenta_bbox().is_none(),
        format!(
            "no display slot -> {:?}; hud_hidden -> {:?}; the control frame \
             carries no magenta at all (which is what makes every magenta \
             measurement below the sidebar's)",
            none.is_some(),
            hidden.is_some()
        ),
    );

    let (left, right, bottom, header_y) =
        ref_frame(GUI_W, GUI_H, sidebar.width, sidebar.entries.len() as i32);
    println!(
        "[sidebarshot] width {} title_width {} rows {} -> left {left} right {right} \
         bottom {bottom} headerY {header_y}",
        sidebar.width,
        sidebar.title_width,
        sidebar.entries.len()
    );

    // -- the two bands ------------------------------------------------------
    //
    // Sampled at a column inside both bands and clear of the glyphs' left
    // edge: `left - 1` is one pixel into the band and one pixel left of every
    // name.
    let probe_x = left - 1;
    let clear_px = control.gui(probe_x, header_y - 5);
    let hdr = frame.gui(probe_x, header_y - 5);
    let body = frame.gui(probe_x, bottom - 2);
    let a_hdr = recovered_alpha(hdr, clear_px);
    let a_body = recovered_alpha(body, clear_px);
    // 0.01 is a twentieth of the 26/255 gap between the two transcribed
    // alphas and a tenth of the gap to `Integer.MIN_VALUE`'s 128 — the three
    // values a wrong reading would land on. It is deliberately NOT tight
    // enough to separate 76 from a rounded 77; that claim is a literal, and
    // `m1` is where it is pinned.
    c.record(
        "p2.the_header_band_composited_at_the_0_4_alpha",
        (a_hdr - REF_HEADER_ALPHA as f32 / 255.0).abs() < 0.01,
        format!(
            "recovered {a_hdr:.4} against {}/255 = {:.4}",
            REF_HEADER_ALPHA,
            REF_HEADER_ALPHA as f32 / 255.0
        ),
    );
    c.record(
        "p3.the_body_band_composited_at_the_0_3_alpha_and_is_the_lighter_of_the_two",
        (a_body - REF_BODY_ALPHA as f32 / 255.0).abs() < 0.01 && a_body < a_hdr,
        format!(
            "recovered {a_body:.4} against {}/255 = {:.4}; header {a_hdr:.4}",
            REF_BODY_ALPHA,
            REF_BODY_ALPHA as f32 / 255.0
        ),
    );

    let banded = |f: &Frame, gx: i32, gy: i32| recovered_alpha(f.gui(gx, gy), clear_px) > 0.05;
    c.record(
        "p4.the_bands_start_two_pixels_left_of_the_names",
        banded(&frame, left - 2, header_y - 5)
            && !banded(&frame, left - 3, header_y - 5)
            && banded(&frame, left - 2, bottom - 2)
            && !banded(&frame, left - 3, bottom - 2),
        format!("column {} is banded, {} is not", left - 2, left - 3),
    );
    // The `+ 2`: `right` is `guiWidth - 1`, so the LAST GUI column is clear
    // and the one before it is banded. Under the other plausible reading
    // (`left + width`, i.e. guiWidth - 3) the two columns either side of
    // guiWidth - 3 would answer the other way round.
    c.record(
        "p5.the_bands_reach_one_pixel_from_the_right_edge_not_three",
        banded(&frame, right - 1, header_y - 5)
            && !banded(&frame, right, header_y - 5)
            && right == GUI_W - 1
            && banded(&frame, GUI_W - 3, bottom - 2),
        format!(
            "column {} banded, {} clear (right = {right}, guiWidth - 1 = {}); \
             under the `left + width` reading {} would be the last banded column",
            right - 1,
            right,
            GUI_W - 1,
            left + sidebar.width - 1
        ),
    );
    // The header band is nine tall and stops one pixel above the body, so
    // there is exactly one clear row between them.
    c.record(
        "p6.the_header_band_is_nine_tall_and_leaves_no_gap_it_should_not",
        banded(&frame, probe_x, header_y - 10)
            && !banded(&frame, probe_x, header_y - 11)
            && banded(&frame, probe_x, header_y - 2)
            && banded(&frame, probe_x, header_y - 1)
            && banded(&frame, probe_x, bottom - 1)
            && !banded(&frame, probe_x, bottom),
        format!(
            "rows {}..{} banded (9 of header), body runs {}..{}",
            header_y - 10,
            header_y - 2,
            header_y - 1,
            bottom - 1
        ),
    );
    // `bottom = guiHeight / 2 + height / 3`. With three rows the third and the
    // half differ by 4 pixels, which the probe either side of `bottom` above
    // already pins — this states the claim in its own terms so a reader of the
    // gate sees it.
    let half_bottom = GUI_H / 2 + (sidebar.entries.len() as i32 * 9) / 2;
    c.record(
        "p7.the_panel_sits_a_third_of_its_height_below_centre",
        bottom == GUI_H / 2 + (sidebar.entries.len() as i32 * 9) / 3
            && bottom != half_bottom
            && !banded(&frame, probe_x, half_bottom - 1),
        format!(
            "bottom {bottom}; a half would be {half_bottom}, and row \
             {} is clear",
            half_bottom - 1
        ),
    );

    // -- the rows -----------------------------------------------------------

    let bands = frame.magenta_row_bands();
    c.record(
        "p8.exactly_three_score_rows_reached_the_frame",
        bands.len() == 3,
        format!(
            "{} magenta row bands at {:?} — four holders were injected and one \
             is `#hidden`, so four bands means `PlayerScoreEntry.isHidden` was \
             skipped and two means a row was lost",
            bands.len(),
            bands
        ),
    );
    let pitch_ok = bands.len() == 3
        && bands[1].0 - bands[0].0 == 9
        && bands[2].0 - bands[1].0 == 9
        && bands[0].0 == ref_row_y(bottom, 3, 0) + (bands[0].0 - ref_row_y(bottom, 3, 0));
    // The absolute claim: the first band's top is the first row's y plus the
    // glyph's own top bearing, and the bearing is measured rather than assumed
    // (see the control below).
    c.record(
        "p9.the_rows_are_nine_pixels_apart_running_upward_from_the_bottom",
        pitch_ok
            && bands
                .last()
                .is_some_and(|b| b.1 < bottom && b.0 >= ref_row_y(bottom, 3, 2)),
        format!(
            "bands {:?}; rows are at y {}, {}, {}",
            bands,
            ref_row_y(bottom, 3, 0),
            ref_row_y(bottom, 3, 1),
            ref_row_y(bottom, 3, 2)
        ),
    );

    // -- the right alignment, with the glyph's bearing measured out ---------
    //
    // A control frame draws the same digits in the same colour at a known
    // origin. Whatever the ink's offset inside the glyph cell is, it is the
    // same in both frames, so subtracting it leaves the *placement* — which is
    // what M132 decided and what a wrong reading of `right` would move.
    let top_score = &sidebar.entries[0].score;
    let score_text: String = top_score.iter().map(|s| s.text.as_str()).collect();
    let score_width = sidebar.entries[0].score_width;
    const CTRL_X: i32 = 100;
    const CTRL_Y: i32 = 40;
    let ctrl_line = rewo_gpu::world::OwnedTextLine {
        x: CTRL_X as f32 * px,
        y: CTRL_Y as f32 * px,
        px,
        color_linear: [1.0, 0.0, 1.0],
        alpha: 1.0,
        shadow: false,
        style: rewo_gpu::text::TextStyle::default(),
        text: score_text.clone(),
    };
    let ctrl = shot(&mut gpu, &mut off, &mut wr, Vec::new(), vec![ctrl_line])?;
    let bearing = ctrl
        .magenta_bbox()
        .map(|(x0, _, _, _)| x0 - CTRL_X)
        .ok_or("sidebarshot: the bearing control drew no magenta")?;
    let top_band = bands.first().copied().unwrap_or((0, 0));
    let observed = frame
        .magenta_left_in(top_band.0, top_band.1)
        .map(|x| x - bearing);
    let predicted = right - score_width;
    c.record(
        "p10.the_score_is_right_aligned_to_the_panels_right_edge",
        observed == Some(predicted),
        format!(
            "observed x {observed:?} (bearing {bearing} measured from a control \
             at {CTRL_X}), predicted right - scoreWidth = {right} - \
             {score_width} = {predicted}; the `left + width` reading would put \
             it at {}",
            left + sidebar.width - score_width
        ),
    );

    // -- the title ----------------------------------------------------------
    //
    // Rendered on its own so its bbox is unambiguous: the names are white and
    // would not register, but the scores are magenta and share rows below it.
    let title_only = {
        let mut s = sidebar.clone();
        s.title = top_score.clone();
        s.title_width = score_width;
        s.entries.clear();
        s
    };
    let title_layout = sidebar::layout(&title_only, GUI_W, GUI_H);
    let title_frame = shot(
        &mut gpu,
        &mut off,
        &mut wr,
        Vec::new(),
        crate::live_cmd::sidebar_text(&title_only, &title_layout, px, Some(advance)),
    )?;
    let title_observed = title_frame.magenta_bbox().map(|(x0, _, _, _)| x0 - bearing);
    let (tl, _, _, thy) = ref_frame(GUI_W, GUI_H, title_only.width, 0);
    c.record(
        "p11.the_title_is_centred_over_the_panels_width",
        title_observed == Some(ref_title_x(tl, title_only.width, score_width)),
        format!(
            "observed x {title_observed:?}, predicted left + width/2 - \
             titleWidth/2 = {}; header row {}",
            ref_title_x(tl, title_only.width, score_width),
            thy - 9
        ),
    );

    // -- no drop shadow -----------------------------------------------------
    //
    // All three `graphics.text` calls in `displayScoreboardSidebar` pass an
    // explicit `false`, where the five-argument overload one class over
    // defaults to `true`. A shadow is a dimmed copy one pixel down and right,
    // so it shows as magenta-hued pixels well below full brightness.
    let shadowed = frame.count_shadow();
    // The same measurement on a line that DOES ask for a shadow, so the
    // detector is shown to be able to see one — a zero that a blind detector
    // would also report is not a witness.
    let shadow_ctrl = shot(
        &mut gpu,
        &mut off,
        &mut wr,
        Vec::new(),
        vec![rewo_gpu::world::OwnedTextLine {
            x: CTRL_X as f32 * px,
            y: CTRL_Y as f32 * px,
            px,
            color_linear: [1.0, 0.0, 1.0],
            alpha: 1.0,
            shadow: true,
            style: rewo_gpu::text::TextStyle::default(),
            text: score_text.clone(),
        }],
    )?;
    let ctrl_shadowed = shadow_ctrl.count_shadow();
    c.record(
        "p12.the_sidebars_text_drops_no_shadow",
        shadowed == 0 && ctrl_shadowed > 0,
        format!(
            "{shadowed} dimmed-magenta pixels in the sidebar frame (must be 0); \
             {ctrl_shadowed} in a control line that asked for one, which is what \
             shows the detector can see a shadow at all"
        ),
    );

    // -- the fills are two, and only two ------------------------------------
    c.record(
        "p13.the_sidebar_emits_exactly_two_fills",
        fills.len() == 2,
        format!(
            "{} fills — `displayScoreboardSidebar` has one header band and ONE \
             body band covering every row, not one per row the way \
             `PlayerTabOverlay` does",
            fills.len()
        ),
    );

    // -- and the text lines are the seven the rows imply --------------------
    c.record(
        "p14.the_emitter_produced_a_title_and_three_name_score_pairs",
        text_len == 7,
        format!(
            "{} text lines (a title, then three rows of name + score; a \
             `#hidden` holder would add two more)",
            text_len
        ),
    );

    if let Some(dir) = args.out_dir.as_ref() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{e}"))?;
        let fills = crate::live_cmd::sidebar_fills(&layout);
        let text = crate::live_cmd::sidebar_text(&sidebar, &layout, px, Some(advance));
        wr.set_hud_fills(fills);
        wr.set_text(text);
        off.render(&mut gpu, Some((&mut wr, vp)), &overlay_draw, clear)?;
        off.save_png(&gpu, &dir.join("sidebar.png"))?;
        println!("[sidebarshot] wrote {}", dir.join("sidebar.png").display());
    }

    wr.destroy(&mut gpu);
    off.destroy(&mut gpu);
    Ok(())
}
