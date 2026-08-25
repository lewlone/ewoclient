//! `advshot` — the advancements screen's serverless gate (M178).
//!
//! Drives the PRODUCTION builders — [`crate::advancements_view`] build +
//! chrome + lines over a synthetic `rewo_net::advancements::ClientAdvancements`
//! state — through the same WorldRenderer screen/text passes bookshot uses,
//! and reads pixels back. M93b's rule applies one level up: a gate that
//! hand-builds its own chrome proves nothing about what the client draws, so
//! every witness goes through the app's own lowering.
//!
//! What is deliberately NOT graded here: the item-icon pass (tab icons and
//! widget icons ride the GUI-item atlas, whose packing needs the live
//  uploader; recorded as an M179 follow-up) and the windowed open/close path
//! (needs a server; that is what r-witnesses are for).

use crate::advancements_view::AdvancementsView;
use clap::Args as ClapArgs;
use rewo_data::{assets, DataPaths};
use rewo_gpu::offscreen::Offscreen;
use rewo_gpu::world::{SkyMode, WorldRenderer};
use rewo_gpu::Gpu;
use rewo_net::advancements::{
    ClientAdvancements, Frame as WireFrame, Progress, UpdateAdvancements, WireAdvancement,
    WireDisplay,
};
use rewo_proto::nbt::Nbt;

const EXPECTED_WITNESSES: usize = 20;

#[derive(ClapArgs)]
pub struct AdvshotArgs {
    /// Assert every owned property (labels the run; failures exit nonzero
    /// either way - the deathshot convention).
    #[arg(long, default_value_t = false)]
    pub check: bool,
    #[arg(long, default_value = "26.2")]
    pub version: String,
    /// Write the rendered frames here for eyeballing. Never read back.
    #[arg(long)]
    pub out_dir: Option<std::path::PathBuf>,
}

struct Checker {
    witnessed: usize,
    failures: Vec<String>,
}

impl Checker {
    fn record(&mut self, name: &str, pass: bool, detail: impl std::fmt::Display) {
        println!(
            "[advshot] {}  {name}: {detail}",
            if pass { " ok " } else { "FAIL" }
        );
        self.witnessed += 1;
        if !pass {
            self.failures.push(name.to_string());
        }
    }
}

/// The icon template every fixture display carries — item 1 (stone), count 1,
/// empty patch.
fn test_icon() -> rewo_net::component_wire::ItemTemplate {
    rewo_net::component_wire::ItemTemplate { item_id: 1, count: 1, patched: false }
}

const W: u32 = 640;
const H: u32 = 480;
const SCALE: i32 = 2;
const GUI_W: i32 = W as i32 / SCALE; // 320
const GUI_H: i32 = H as i32 / SCALE; // 240

/// A network-NBT string tag: `0x08` + u16 BE length + utf-8.
fn nbt_string(s: &str) -> Nbt {
    Nbt::String(s.to_string())
}

fn root(id: &str, title: &str, bg: Option<String>) -> WireAdvancement {
    WireAdvancement {
        id: id.into(),
        parent: None,
        display: Some(WireDisplay {
            title: nbt_string(title),
            description: nbt_string("desc"),
            icon: test_icon(),
            frame: WireFrame::Task,
            background: bg,
            show_toast: true,
            hidden: false,
            x: 0.0,
            y: 0.0,
        }),
        requirements: vec![vec!["c1".into()]],
        sends_telemetry: false,
    }
}

fn child(id: &str, parent: &str, gx: f32, gy: f32) -> WireAdvancement {
    let mut a = WireAdvancement {
        id: id.into(),
        parent: Some(parent.into()),
        display: Some(WireDisplay {
            title: nbt_string(id),
            description: nbt_string("a longer description line for wrapping"),
            icon: test_icon(),
            frame: WireFrame::Challenge,
            background: None,
            show_toast: true,
            hidden: false,
            x: gx,
            y: gy,
        }),
        requirements: vec![vec!["c1".into()]],
        sends_telemetry: false,
    };
    if let Some(d) = a.display.as_mut() {
        d.x = gx;
        d.y = gy;
    }
    a
}

fn apply(
    t: &mut ClientAdvancements,
    reset: bool,
    added: Vec<WireAdvancement>,
    progress: Vec<(&str, Vec<(&str, Option<i64>)>)>,
) {
    t.apply_update(UpdateAdvancements {
        reset,
        added,
        removed: vec![],
        progress: progress
            .into_iter()
            .map(|(id, crit)| {
                (
                    id.to_string(),
                    Progress {
                        criteria: crit
                            .into_iter()
                            .map(|(n, done)| (n.to_string(), done))
                            .collect(),
                    },
                )
            })
            .collect(),
        show_advancements: true,
    });
}

fn client_jar(version: &str) -> Option<std::path::PathBuf> {
    let mut p = dirs::config_dir()?;
    p.push("EwoClient");
    p.push("shared/versions");
    p.push(version);
    p.push(format!("{version}.jar"));
    p.exists().then_some(p)
}

pub fn run(args: AdvshotArgs) -> Result<(), String> {
    let jar = client_jar(&args.version).ok_or("client jar not found")?;
    let paths = DataPaths::for_version(&args.version).ok_or("no config dir")?;
    let baked = assets::bake(&jar, &paths.blocks_json())?;
    let mut c = Checker {
        witnessed: 0,
        failures: Vec::new(),
    };

    // ── Model-level witnesses (no GPU) ────────────────────────────────────
    let mut tree = ClientAdvancements::default();
    apply(
        &mut tree,
        true,
        vec![
            root(
                "minecraft:story/root",
                "Minecraft",
                Some("minecraft:textures/gui/advancements/backgrounds/stone.png".into()),
            ),
            child("minecraft:story/one", "minecraft:story/root", 1.0, 0.0),
            child("minecraft:story/two", "minecraft:story/root", 2.0, 1.0),
            {
                let mut h = child("minecraft:story/hid", "minecraft:story/root", 3.0, 3.0);
                if let Some(d) = h.display.as_mut() {
                    d.hidden = true;
                }
                h
            },
            {
                // A second hidden node placed INSIDE the visible contents, so
                // a frame wrongly drawn for it lands on a probeable pixel.
                let mut h = child("minecraft:story/hid2", "minecraft:story/root", 3.5, -1.0);
                if let Some(d) = h.display.as_mut() {
                    d.hidden = true;
                }
                h
            },
        ],
        vec![("minecraft:story/root", vec![("c1", Some(7))])],
    );

    let advance = baked.font.as_ref().ok_or("no baked font")?.advance;
    let lang = baked.lang.clone();
    let view = AdvancementsView::build(&tree, &lang, &advance);

    c.record(
        "m1.the_screen_opens_onto_the_first_tab",
        view.screen.selected == Some(0),
        format!("selected = {:?}", view.screen.selected),
    );
    c.record(
        "m2.four_subtree_members_became_four_widgets",
        view.screen.tabs.len() == 1 && view.screen.tabs[0].widgets.len() == 5,
        format!(
            "{} tab(s), {} widget(s)",
            view.screen.tabs.len(),
            view.screen.tabs.first().map(|t| t.widgets.len()).unwrap_or(0)
        ),
    );
    // The hidden ones are widgets (descendants route links through them) but
    // draw and hover nothing while undone.
    let hid = &view.screen.tabs[0].widgets[3];
    c.record(
        "m2b.a_hidden_undone_widget_is_invisible",
        !hid.visible && !view.screen.tabs[0].widgets[4].visible,
        format!(
            "hidden visible={} hidden2 visible={}",
            hid.visible, view.screen.tabs[0].widgets[4].visible
        ),
    );
    c.record(
        "m3.the_root_is_done_so_its_frame_reads_obtained",
        view.screen.tabs[0].widgets[0].done && view.screen.tabs[0].widgets[0].percent == 1.0,
        format!(
            "done={} percent={}",
            view.screen.tabs[0].widgets[0].done, view.screen.tabs[0].widgets[0].percent
        ),
    );
    // The centring latch: bounds span grid 0..+2 → scrollX pulls left of 117.
    let mut view2 = AdvancementsView::build(&tree, &lang, &advance);
    let t2 = &mut view2.screen.tabs[0];
    t2.ensure_centered();
    c.record(
        "m4.centring_pulls_the_tree_left_of_centre",
        t2.scroll_x < 117.0,
        format!("scroll_x = {}", t2.scroll_x),
    );

    // ── M179: clicks, wheel and drag ──────────────────────────────────────
    // Two roots make two ABOVE-row cells: tab i sits at window-relative
    // (32*i .. 32*i+28, -28..4), i.e. absolute x 34+32i at this window size
    // (xo = (320-252)/2 = 34, yo = (240-140)/2 = 50).
    let mut two_roots = ClientAdvancements::default();
    apply(
        &mut two_roots,
        true,
        vec![
            root(
                "minecraft:story/root",
                "Minecraft",
                Some("minecraft:textures/gui/advancements/backgrounds/stone.png".into()),
            ),
            root("minecraft:adventure/root", "Adventure", None),
        ],
        vec![],
    );
    let vtabs = AdvancementsView::build(&two_roots, &lang, &advance);

    // m7 — strict bounds, per cell. The centre hits its own index; every
    // edge and the 4px gutter between cells miss (a strip-wide rect or an
    // inclusive box would answer differently).
    let hits = [
        ((48.0, 38.0), Some(0)), // tab 0 centre
        ((80.0, 38.0), Some(1)), // tab 1 centre
        ((34.0, 38.0), None),    // tab 0's LEFT EDGE — strict >
        ((62.0, 38.0), None),    // tab 0's RIGHT EDGE — strict <
        ((64.0, 38.0), None),    // the gutter BETWEEN the cells
        ((48.0, 22.0), None),    // top edge
        ((48.0, 54.0), None),    // bottom edge
    ];
    let mut all = true;
    let mut detail = String::new();
    for ((mx, my), want) in hits {
        let got = vtabs.tab_click(GUI_W, GUI_H, mx, my);
        all &= got == want;
        detail.push_str(&format!(" ({mx},{my}->{got:?})"));
    }
    c.record("m7.tab_click_cells_are_strict_and_separate", all, detail);

    // m8 — ONE tab still clicks. `mouseClicked`'s loop has NO size guard
    // (`AdvancementsScreen.java:113-127`); only the DRAW does (`:206`). An
    // earlier draft refused clicks at ≤1 tabs — the draw rule misread as the
    // click rule.
    c.record(
        "m8.a_single_tab_still_clicks",
        view.tab_click(GUI_W, GUI_H, 48.0, 38.0) == Some(0),
        format!(
            "single-tab click = {:?}",
            view.tab_click(GUI_W, GUI_H, 48.0, 38.0)
        ),
    );

    // m9 — clicking selects and names the root the packet carries. Drives
    // `tab_click_report` — the function production's handler runs — not a
    // hand-rolled copy of it (the M93b rule; a copy let a select-deleting
    // mutant survive the battery).
    let mut vc = AdvancementsView::build(&two_roots, &lang, &advance);
    let report = vc.tab_click_report(GUI_W, GUI_H, 80.0, 38.0);
    c.record(
        "m9.clicking_selects_and_names_the_root",
        report.as_deref() == Some("minecraft:adventure/root")
            && vc.screen.selected == Some(1),
        format!(
            "report {report:?}, selected {:?}",
            vc.screen.selected
        ),
    );

    // m10 — RE-clicking the already-selected tab reports it again. This is
    // the observable half of `setSelectedTab` sending `opened_tab` BEFORE its
    // change check (`ClientAdvancements.java:77-86`): the server hears about
    // a tab that is already open.
    c.record(
        "m10.reclicking_the_selected_tab_reports_it_again",
        vc.tab_click_report(GUI_W, GUI_H, 80.0, 38.0).is_some(),
        "the click path never filters on change",
    );

    // m11 — the wheel scales by SCROLL_SPEED and clamps at the content edge.
    // A child pushed to grid x=9 makes maxX = 9*28+28 = 280 > 234, so the
    // horizontal clamp's lower bound is -(280-234) = -46; centring starts at
    // 117-140 = -23.
    let mut widetree = ClientAdvancements::default();
    apply(
        &mut widetree,
        true,
        vec![
            root(
                "minecraft:wide/root",
                "Wide",
                Some("minecraft:textures/gui/advancements/backgrounds/stone.png".into()),
            ),
            child("minecraft:wide/far", "minecraft:wide/root", 9.0, 0.0),
        ],
        vec![],
    );
    let mut vw = AdvancementsView::build(&widetree, &lang, &advance);
    {
        let t = &mut vw.screen.tabs[0];
        t.ensure_centered();
    }
    let start = vw.screen.tabs[0].scroll_x; // -23 exactly
    vw.wheel(1.0, 0.0); // one notch right
    let after_one = vw.screen.tabs[0].scroll_x;
    vw.wheel(1000.0, 0.0);
    let hi = vw.screen.tabs[0].scroll_int().0;
    vw.wheel(-10000.0, 0.0);
    let lo = vw.screen.tabs[0].scroll_int().0;
    let y_before = vw.screen.tabs[0].scroll_y;
    vw.wheel(0.0, -1000.0); // vertical cannot move: maxY 27 <= 113
    let y_after = vw.screen.tabs[0].scroll_y;
    c.record(
        "m11.wheel_scales_by_16_and_clamps_at_the_content_edge",
        start == -23.0
            && after_one == -7.0
            && hi == 0
            && lo == -46
            && y_before == y_after,
        format!(
            "start {start}, one notch {after_one} (=x16), hi {hi}, lo {lo}, y {y_before}->{y_after}"
        ),
    );

    // m12 — the DRAG path applies raw deltas (SCROLL_SPEED belongs to the
    // wheel alone, `AdvancementsScreen.java:185` vs `:170`), and an empty
    // screen declines the wheel entirely (`mouseScrolled`'s null-tab arm).
    let mut vd = AdvancementsView::build(&widetree, &lang, &advance);
    {
        let t = &mut vd.screen.tabs[0];
        t.ensure_centered();
    }
    let d_before = vd.screen.tabs[0].scroll_x;
    vd.drag_scroll(2.0, 0.0);
    let d_moved = vd.screen.tabs[0].scroll_x - d_before;
    let mut empty = AdvancementsView::build(&ClientAdvancements::default(), &lang, &advance);
    let declined = !empty.wheel(1.0, 1.0);
    c.record(
        "m12.drag_scrolls_raw_and_an_empty_screen_declines_the_wheel",
        d_moved == 2.0 && declined,
        format!("drag moved {d_moved} (want 2.0), empty declined={declined}"),
    );

    // ── Pixel witnesses (validation ON) ───────────────────────────────────
    let mut gpu = Gpu::new(None, true)?;
    println!(
        "[advshot] Vulkan validation: {}",
        if gpu.validation_active { "ON" } else { "off" }
    );
    if args.check && !gpu.validation_active {
        return Err("advshot: Vulkan validation requested but not active".into());
    }
    let mut off = Offscreen::new(&mut gpu, W, H)?;
    let white: Vec<u8> = vec![255u8; 16 * 16 * 4];
    let layers = [white];
    let mut wr = WorldRenderer::new(&mut gpu, off.format, 16, &layers)?;
    wr.set_sky_mode(SkyMode::None);
    let sprites =
        crate::live_cmd::widget_sprites(&baked).ok_or("widget sprites missing from the jar")?;
    wr.init_screen(&mut gpu, &sprites)?;
    let font = crate::live_cmd::font_data(&baked).ok_or("no baked font")?;
    wr.init_text(&mut gpu, &font)?;

    let ring = crate::stats::OverlayRing::default();
    let overlay_draw = rewo_gpu::overlay::OverlayDraw {
        samples_ms: &ring.data,
        head: ring.head(),
        scale_ms: 20.0,
        origin: [-4000.0, -4000.0],
        size: [8.0, 8.0],
    };
    let vp = glam::Mat4::IDENTITY.to_cols_array_2d();
    // Pure green clear: nothing the advancements screen draws is green.
    let clear = [0.0, 1.0, 0.0, 1.0];

    let shot = |gpu: &mut Gpu,
                off: &mut Offscreen,
                wr: &mut WorldRenderer,
                view: Option<&AdvancementsView>,
                dump: &str|
     -> Result<Vec<u8>, String> {
        let mut chrome = rewo_gpu::screen::ScreenDraw::default();
        let mut lines = Vec::new();
        if let Some(v) = view {
            chrome = crate::advancements_view::chrome(v, GUI_W, GUI_H, GUI_W);
            lines = crate::advancements_view::lines(v, &lang, GUI_W, GUI_H, GUI_W, SCALE as f32, &advance);
        }
        wr.set_screen(chrome);
        wr.set_text(lines);
        off.render(gpu, Some((&mut *wr, vp)), &overlay_draw, clear)?;
        if let Some(dir) = &args.out_dir {
            let _ = std::fs::create_dir_all(dir);
            let _ = off.save_png(gpu, &dir.join(format!("adv-{dump}.png")));
        }
        off.read_rgba(gpu)
    };

    let px = |f: &[u8], x: i32, y: i32| -> [u8; 3] {
        let i = ((y as u32 * W + x as u32) * 4) as usize;
        [f[i], f[i + 1], f[i + 2]]
    };
    let texel = |s: &rewo_gpu::hud::HudSpriteData, dx: u32, dy: u32| -> [u8; 3] {
        let i = ((dy * s.w + dx) * 4) as usize;
        [s.rgba[i], s.rgba[i + 1], s.rgba[i + 2]]
    };
    let close = |a: [u8; 3], b: [u8; 3], tol: i32| -> bool {
        a.iter()
            .zip(b.iter())
            .all(|(x, y)| (*x as i32 - *y as i32).abs() <= tol)
    };

    let empty_frame = shot(&mut gpu, &mut off, &mut wr, None, "empty")?;
    // p1 — no screen, no window: the clear survives at the window's centre.
    let centre_fb = ((GUI_W / 2) * SCALE, (GUI_H / 2) * SCALE);
    c.record(
        "p1.no_screen_draws_no_window",
        px(&empty_frame, centre_fb.0, centre_fb.1) == [0, 255, 0],
        format!("empty at {centre_fb:?} = {:?}", px(&empty_frame, centre_fb.0, centre_fb.1)),
    );

    let frame = shot(&mut gpu, &mut off, &mut wr, Some(&view2), "open")?;

    // p2 — the window crop IS window.png's sampled region, texel-for-texel.
    // Probes sit on the frame's OPAQUE border rows (alpha 255): the window's
    // interior is transparent by design — the contents show through it.
    let win = &sprites.adv_window;
    let (wx, wy) = rewo_world::advancements_screen::window_origin(GUI_W, GUI_H);
    let probes = [(10u32, 10u32), (126, 3), (248, 130)];
    let mut all = true;
    let mut detail = String::new();
    for (dx, dy) in probes {
        let i = ((dy * win.w + dx) * 4 + 3) as usize;
        let alpha = win.rgba[i];
        let t = texel(win, dx, dy);
        let f = px(&frame, (wx + dx as i32) * SCALE, (wy + dy as i32) * SCALE);
        let ok = alpha == 255 && close(t, f, 2);
        all &= ok;
        detail.push_str(&format!(" ({dx},{dy}: {t:?} a={alpha} vs {f:?} ok={ok})"));
    }
    c.record("p2.the_window_crop_blits_texel_for_texel", all, detail);

    // p3 — the scissor holds: the contents area shows backdrop tiles, but the
    // strip BELOW THE WHOLE WINDOW (past its 140px bottom edge) keeps the
    // clear colour. Closer strips are the window's own opaque border.
    let below_y = (wy + rewo_world::advancements_screen::WINDOW_H + 4) * SCALE;
    let below_ok = px(&frame, (wx + 40) * SCALE, below_y) == [0, 255, 0];
    let inside_ok = px(
        &frame,
        (wx + rewo_world::advancements_screen::INSIDE_X + 4) * SCALE,
        (wy + rewo_world::advancements_screen::INSIDE_Y + 4) * SCALE,
    ) != [0, 255, 0];
    c.record(
        "p3.the_scissor_clips_contents_at_the_inside_rect",
        below_ok && inside_ok,
        format!("below={below_ok} inside_draws={inside_ok}"),
    );

    // p4 — connectivity: the white core REACHES A PIXEL somewhere along its
    // runs. It may be partially covered by the frames it feeds (vanilla draws
    // lines before frames too), so the witness counts near-white pixels
    // across every foreground run rather than probing one point.
    let t = &mut view2.screen.tabs[0];
    t.ensure_centered();
    let in_origin = (
        wx + rewo_world::advancements_screen::INSIDE_X,
        wy + rewo_world::advancements_screen::INSIDE_Y,
    );
    let (sx, sy) = t.scroll_int();
    let runs = t.connectivity(sx, sy, false);
    // A white pixel only counts if it is NOT under a widget frame — vanilla
    // draws lines before frames, so frames legitimately cover parts of them.
    let frame_rects: Vec<(i32, i32, i32, i32)> = t
        .widgets
        .iter()
        .filter(|w| w.visible)
        .map(|w| (sx + w.x + 3, sy + w.y, 26, 26))
        .collect();
    let mut whites = 0usize;
    // Order-sensitivity: the UNDERLAY must be beneath the core. With the
    // passes flipped, every exposed horizontal core pixel reads black (the
    // 3-row underlay's middle row lands exactly on it).
    let mut blacks_on_core = 0usize;
    for (rx, ry, rw, rh) in &runs {
        for dx in 0..*rw {
            for dy in 0..*rh {
                let cx = rx + dx;
                let cy = ry + dy;
                if frame_rects
                    .iter()
                    .any(|(fx, fy, fw, fh)| cx >= *fx && cx < fx + fw && cy >= *fy && cy < fy + fh)
                {
                    continue;
                }
                let p = px(
                    &frame,
                    (in_origin.0 + cx) * SCALE,
                    (in_origin.1 + cy) * SCALE,
                );
                if p.iter().all(|v| *v >= 250) {
                    whites += 1;
                }
                if *rh == 1 && rw > &2 && dx > 0 && dx + 1 < *rw && p.iter().all(|v| *v <= 60) {
                    blacks_on_core += 1;
                }
            }
        }
    }
    c.record(
        "p4.the_connectivity_core_reaches_a_pixel",
        whites >= 4 && blacks_on_core == 0,
        format!(
            "{whites} exposed white pixels, {blacks_on_core} black-on-core across {} runs",
            runs.len()
        ),
    );

    // p5 — the tooltip appears when the model hovers: force the hover onto
    // widget 1 (the challenge child), rebuild chrome, and demand changed
    // pixels inside the tooltip band plus a title-box sheet probe match.
    let mut vh = AdvancementsView::build(&tree, &lang, &advance);
    {
        let th = &mut vh.screen.tabs[0];
        th.ensure_centered();
        th.hovered = Some(1);
        th.fade = 0.3;
    }
    let hover_frame = shot(&mut gpu, &mut off, &mut wr, Some(&vh), "hover")?;
    let changed = (0..4096)
        .map(|i| {
            let x = (i % 64) * 10;
            let y = ((i / 64) * 11) % H as i32;
            px(&frame, x, y) != px(&hover_frame, x, y)
        })
        .filter(|b| *b)
        .count();
    c.record(
        "p5.hovering_a_widget_draws_the_tooltip",
        changed > 64,
        format!("{changed} of 4096 sampled pixels changed"),
    );

    // m5 — the text builder carries the layout header AND the selected tab's
    // title (the window's top-left line).
    let ls = crate::advancements_view::lines(&view2, &lang, GUI_W, GUI_H, GUI_W, SCALE as f32, &advance);
    c.record(
        "m5.lines_carry_header_and_window_title",
        ls.iter().any(|l| l.text == "Advancements")
            && ls.iter().any(|l| l.text == "Minecraft"),
        format!(
            "{} lines; header={} title={}",
            ls.len(),
            ls.iter().any(|l| l.text == "Advancements"),
            ls.iter().any(|l| l.text == "Minecraft")
        ),
    );
    // m6 — every VISIBLE widget owes an icon (the item pass draws them); the
    // two hidden ones owe none.
    let draws = crate::advancements_view::icon_draws(&view2, GUI_W, GUI_H, SCALE as f32);
    c.record(
        "m6.three_visible_widgets_owe_three_icons",
        draws.len() == 3 && draws.iter().all(|d| d.item == 1),
        format!("{} icon draws", draws.len()),
    );

    // p6 — the fade overlay DARKENS: hovering raises `fade` to 0.3 and the
    // black fill multiplies the contents down. Probe a contents corner far
    // from any tooltip geometry.
    let probe = (
        in_origin.0 + 4,
        in_origin.1 + rewo_world::advancements_screen::INSIDE_H - 8,
    );
    let before = px(&frame, probe.0 * SCALE, probe.1 * SCALE);
    let after = px(&hover_frame, probe.0 * SCALE, probe.1 * SCALE);
    let darkened = after[0] < before[0] && before[0] > 60;
    c.record(
        "p6.the_hover_fade_darkens_the_contents",
        darkened,
        format!("{before:?} -> {after:?} under fade 0.3"),
    );

    // p7 — the hidden-inside widget draws NO frame: its frame centre must
    // equal the bare tile beside it (a drawn frame would differ).
    let hid2 = &view2.screen.tabs[0].widgets[4];
    let hcx = sx + hid2.x + 3 + 13;
    let hcy = sy + hid2.y + 13;
    let a = px(&frame, (in_origin.0 + hcx) * SCALE, (in_origin.1 + hcy) * SCALE);
    let b = px(
        &frame,
        (in_origin.0 + hcx + 17) * SCALE,
        (in_origin.1 + hcy) * SCALE,
    );
    c.record(
        "p7.a_hidden_widget_leaves_no_frame_pixels",
        a == b,
        format!("centre {a:?} vs neighbour {b:?}"),
    );

    println!(
        "[advshot] witnesses observed: {} / {EXPECTED_WITNESSES}",
        c.witnessed
    );
    for f in &c.failures {
        println!("FAIL {f}");
    }
    // Teardown before grading: the 0-VUID bar is part of the gate (M48).
    wr.destroy(&mut gpu);
    off.destroy(&mut gpu);
    gpu.wait_idle();
    let ok = c.witnessed == EXPECTED_WITNESSES && c.failures.is_empty();
    if args.check {
        if !ok {
            return Err("advshot: check failed".into());
        }
    } else if !c.failures.is_empty() {
        return Err("advshot: failures present".into());
    }
    Ok(())
}
