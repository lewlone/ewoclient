//! The advancements screen's render half (M178).
//!
//! [`rewo_world::advancements_screen`] owns the layout and the clocks; this
//! turns a session's `ClientAdvancements` snapshot into that model plus a
//! [`rewo_gpu::screen::ScreenDraw`], text lines and item icons. Split out of
//! `live_cmd` for the stats_view reason: **the gate must drive these
//! builders**, and a function buried in a 26k-line file is a function that
//! quietly grows a second caller.
//!
//! # Coordinate spaces, stated once
//!
//! Every [`SpriteDraw`] here is in WINDOW-relative GUI pixels — the caller
//! adds the window origin once when queueing, exactly like every other
//! screen. The scissor batch's rect is likewise window-relative; the pass
//! scales both together, so they cannot disagree.
//!
//! The MODEL works in contents-local coordinates (scroll applied); [`chrome`]
//! shifts by the inside origin when emitting, which is the same shift
//! vanilla's `pose().translate(windowLeft + 9, windowTop + 18)` does.
//!
//! # What the build resolves, once, at open
//!
//! Vanilla's screen attaches a listener and mutates as packets arrive; Rewo
//! snapshots at open (M178) — mid-screen updates are a recorded gap, not an
//! approximation of anything.
//!
//! The font-dependent pieces resolve here because this is where the metrics
//! live:
//!
//! - **Title lines** — `minecraft.font.split(title, 163)` (`AdvancementWidget`
//!   ctor, `:60`): greedy word-wrap at 163 px over the advance table.
//! - **Description lines** — `findOptimalLines` (`:96-115`) tries wrap widths
//!   `preferred - {0, 10, -10, 25, -25}`, keeps the FIRST whose longest line
//!   lands within 10 px of `preferred`, else the closest. `preferred =
//!   29 + titleWidth + maxProgressWidth`, where `titleWidth = max(80,
//!   widest title line)` and `maxProgressWidth` renders
//!   `advancements.progress` with the requirement count twice — zero at one
//!   group (`getMaxProgressWidth`, `:81-90`). `width = longest + 3 + 5`.
//! - **The progress counter** — suppressed entirely at `total <= 1`
//!   (`getProgressText`, `:139`).

use rewo_data::lang::Language;
use rewo_gpu::screen::{Fill, ScreenDraw, ScissorBatch, Sheet, SpriteDraw};
use rewo_gpu::world::OwnedTextLine;
use rewo_world::string_splitter::find_line_break;
use rewo_world::{
    advancements_screen::{self as asm, Frame, NodeInput, TabKind},
    chat_style,
};

use crate::live_cmd::srgb_bytes_to_linear_f;

/// Everything the advancements screen holds across frames.
pub struct AdvancementsView {
    pub screen: asm::AdvancementsScreen,
}

/// `AdvancementWidget.TITLE_MAX_WIDTH`.
const TITLE_MAX_WIDTH: i32 = 163;
/// `AdvancementWidget.TEST_SPLIT_OFFSETS`.
const TEST_SPLIT_OFFSETS: [i32; 5] = [0, 10, -10, 25, -25];

impl AdvancementsView {
    /// `AdvancementsScreen`'s constructor + `init()`'s listener replay, over a
    /// session snapshot.
    pub fn build(
        adv: &rewo_net::advancements::ClientAdvancements,
        lang: &Language,
        advance: &[u8; 256],
    ) -> Self {
        let measure = |t: &str| rewo_gpu::text::width(t, advance);
        let mut screen = asm::AdvancementsScreen::default();
        for root in adv.tabs() {
            let display = root.advancement.display.as_ref().expect("tabs have displays");
            let done = adv.is_done(&root.id).unwrap_or(false);
            let percent = adv
                .progress(&root.id)
                .map(|p| p.percent(&root.advancement.requirements))
                .unwrap_or(0.0);
            let counter = adv
                .progress(&root.id)
                .and_then(|p| p.progress_text(&root.advancement.requirements))
                .map(|(complete, total)| {
                    (
                        lang.translate(
                            "advancements.progress",
                            &[&complete.to_string(), &total.to_string()],
                        ),
                        0,
                    )
                })
                .map(|(text, _)| {
                    let w = measure(&text);
                    (text, w)
                });
            let input = NodeInput {
                id: root.id.clone(),
                parent: None,
                display: Some(resolve_display(
                    display,
                    lang,
                    advance,
                    &measure,
                    done,
                    percent,
                    &root.advancement.requirements,
                    counter,
                )),
                done,
                percent,
            };
            let root_id = root.id.clone();
            screen.add_root(&input);
            for task in adv.tab_tasks(&root_id) {
                let task_done = adv.is_done(&task.id).unwrap_or(false);
                let task_percent = task
                    .advancement
                    .display
                    .as_ref()
                    .map(|_| {
                        adv.progress(&task.id)
                            .map(|p| p.percent(&task.advancement.requirements))
                            .unwrap_or(0.0)
                    })
                    .unwrap_or(0.0);
                let counter = adv
                    .progress(&task.id)
                    .and_then(|p| p.progress_text(&task.advancement.requirements))
                    .map(|(complete, total)| {
                        let text = lang.translate(
                            "advancements.progress",
                            &[&complete.to_string(), &total.to_string()],
                        );
                        let w = measure(&text);
                        (text, w)
                    });
                let input = NodeInput {
                    id: task.id.clone(),
                    parent: task.parent.clone(),
                    display: task.advancement.display.as_ref().map(|d| {
                        resolve_display(
                            d,
                            lang,
                            advance,
                            &measure,
                            task_done,
                            task_percent,
                            &task.advancement.requirements,
                            counter,
                        )
                    }),
                    done: task_done,
                    percent: task_percent,
                };
                screen.add_task(&root_id, &input);
            }
        }
        // init(): nothing remembered selects the FIRST tab, telling the
        // server (the packet send is the caller's job).
        if !screen.tabs.is_empty() {
            screen.select(Some(0));
        }
        Self { screen }
    }

    /// The selected tab's root id — what the OPENED_TAB packet carries.
    pub fn selected_root_id(&self) -> Option<&str> {
        self.screen
            .selected
            .and_then(|i| self.screen.tabs.get(i))
            .map(|t| t.root_id.as_str())
    }

    /// `AdvancementTab.tick`, driven per frame with the contents-relative
    /// mouse (`mouseX - leftPos - 9, mouseY - topPos - 18`).
    pub fn tick(&mut self, inside_mouse: Option<(i32, i32)>) {
        let Some(sel) = self.screen.selected else {
            return;
        };
        let Some(tab) = self.screen.tabs.get_mut(sel) else {
            return;
        };
        match inside_mouse {
            Some((mx, my)) => tab.tick(mx, my),
            None => tab.tick(-1, -1),
        }
    }

    /// `extractContents`' centring latch, called before chrome each frame.
    pub fn ensure_centered(&mut self) {
        if let Some(tab) = self
            .screen
            .selected
            .and_then(|i| self.screen.tabs.get_mut(i))
        {
            tab.ensure_centered();
        }
    }

    /// A left-click on a tab cell — `mouseClicked`'s loop, which runs only
    /// while MORE THAN ONE tab exists. Returns the tab-list index hit.
    pub fn tab_click(&self, gui_w: i32, gui_h: i32, mx: f64, my: f64) -> Option<usize> {
        if self.screen.tabs.len() <= 1 {
            return None;
        }
        let (xo, yo) = asm::window_origin(gui_w, gui_h);
        for (i, t) in self.screen.tabs.iter().enumerate() {
            if t.kind.is_mouse_over(xo, yo, t.index, mx, my) {
                return Some(i);
            }
        }
        None
    }
}

/// `AdvancementWidget`'s constructor, minus the model: everything measured or
/// flattened resolves here.
#[allow(clippy::too_many_arguments)]
fn resolve_display(
    d: &rewo_net::advancements::WireDisplay,
    _lang: &Language,
    advance: &[u8; 256],
    measure: &dyn Fn(&str) -> i32,
    _done: bool,
    _percent: f32,
    requirements: &[Vec<String>],
    progress_counter: Option<(String, i32)>,
) -> asm::DisplayInput {
    let title_text = chat_style::flatten(&d.title, Some(_lang));
    let title_lines = split_plain(&title_text, TITLE_MAX_WIDTH, measure);

    // getMaxProgressWidth: the counter's worst case, only past one group.
    let groups = requirements.len();
    let progress_worst = if groups > 1 {
        let fake = _lang.translate(
            "advancements.progress",
            &[&groups.to_string(), &groups.to_string()],
        );
        measure(&fake) + 8
    } else {
        0
    };
    let title_width = title_lines
        .iter()
        .map(|l| measure(l))
        .max()
        .unwrap_or(0)
        .max(80);
    let preferred = 29 + title_width + progress_worst;

    let desc_text = chat_style::flatten(&d.description, Some(_lang));
    let description_lines = find_optimal_lines(&desc_text, preferred, measure);
    let longest = description_lines
        .iter()
        .map(|l| measure(l))
        .max()
        .unwrap_or(0)
        .max(preferred);
    let width = longest + 3 + 5;

    asm::DisplayInput {
        frame: frame_of(d.frame),
        hidden: d.hidden,
        gx: d.x,
        gy: d.y,
        icon: asm::Icon {
            item: d.icon.item_id,
            count: d.icon.count,
        },
        background: d.background.clone(),
        title: title_text,
        title_lines,
        description_lines,
        width,
        progress_text: progress_counter,
    }
}

/// `AdvancementWidget.findOptimalLines` over plain text.
fn find_optimal_lines(
    input: &str,
    preferred: i32,
    measure: &dyn Fn(&str) -> i32,
) -> Vec<String> {
    let mut best: Option<(f32, Vec<String>)> = None;
    for off in TEST_SPLIT_OFFSETS {
        let split = split_plain(input, preferred - off, measure);
        let max_w = split.iter().map(|l| measure(l)).max().unwrap_or(0);
        let dist = (max_w - preferred).abs() as f32;
        if dist <= 10.0 {
            return split;
        }
        if best.as_ref().map_or(true, |(bd, _)| dist < *bd) {
            best = Some((dist, split));
        }
    }
    best.map(|(_, l)| l).unwrap_or_default()
}

/// `StringSplitter.splitLines` over plain text: greedy word wrap through the
/// shared line-break finder. A break AT a space leaves the space off both
/// sides; a break ON an overflowing character starts the next line there.
pub(crate) fn split_plain(
    input: &str,
    max_width: i32,
    measure: &dyn Fn(&str) -> i32,
) -> Vec<String> {
    let mut out = Vec::new();
    let chars: Vec<char> = input.chars().collect();
    let mut start = 0usize;
    while start < chars.len() {
        match find_line_break(&chars, start, max_width, measure) {
            Some(break_i) => {
                let had_space = chars.get(break_i) == Some(&' ');
                let end = if had_space { break_i } else { break_i };
                out.push(chars[start..end].iter().collect::<String>());
                start = if had_space { break_i + 1 } else { break_i };
            }
            None => {
                out.push(chars[start..].iter().collect::<String>());
                break;
            }
        }
    }
    out.retain(|l| !l.is_empty());
    out
}

fn frame_of(f: rewo_net::advancements::Frame) -> Frame {
    match f {
        rewo_net::advancements::Frame::Task => Frame::Task,
        rewo_net::advancements::Frame::Challenge => Frame::Challenge,
        rewo_net::advancements::Frame::Goal => Frame::Goal,
    }
}

// ─── Chrome ──────────────────────────────────────────────────────────────────

/// Which baked backdrop a wire identifier names, as its sheet index. Unknown
/// paths answer `None` — the renderer draws NO tiles rather than guessing
/// (vanilla falls back to its intentional-missing texture, unbaked here).
pub fn background_index(path: &str) -> Option<u8> {
    const PREFIX: &str = "minecraft:textures/gui/advancements/backgrounds/";
    let rest = path.strip_prefix(PREFIX)?;
    let name = rest.strip_suffix(".png")?;
    rewo_data::assets::ADV_BACKGROUNDS
        .iter()
        .position(|n| *n == name)
        .map(|i| i as u8)
}

/// One frame's chrome: the scissored contents, the window, the tab strip, the
/// fade overlay and the hover tooltip. All window-relative GUI px. Text comes
/// from [`lines`], item icons from [`icons`].
///
/// `screen_width` feeds the tooltip's leftSide flip, which measures against
/// the SCREEN, not the window contents.
#[allow(clippy::too_many_arguments)]
pub fn chrome(view: &AdvancementsView, gui_w: i32, gui_h: i32, screen_width: i32) -> ScreenDraw {
    let mut draw = ScreenDraw::default();

    let Some(sel) = view.screen.selected else {
        return draw;
    };
    let Some(tab) = view.screen.tabs.get(sel) else {
        return draw;
    };

    let (win_x, win_y) = asm::window_origin(gui_w, gui_h);
    let (in_x, in_y) = (win_x + asm::INSIDE_X, win_y + asm::INSIDE_Y);
    let (sx, sy) = tab.scroll_int();

    // extractInside — scissored to the 234x113 contents area.
    let mut batch = ScissorBatch {
        rect: (in_x, in_y, asm::INSIDE_W, asm::INSIDE_H),
        sprites: Vec::new(),
    };
    if let Some(path) = tab.background.as_deref().and_then(background_index) {
        // scroll mod 16 — Java % truncates toward zero, so negative scrolls
        // give negative offsets, exactly like vanilla's int arithmetic.
        let left = sx.rem_euclid(asm::BACKGROUND_TILE);
        let top = sy.rem_euclid(asm::BACKGROUND_TILE);
        for tx in -1..=15 {
            for ty in -1..=8 {
                batch.sprites.push(SpriteDraw {
                    x: in_x + left + asm::BACKGROUND_TILE * tx,
                    y: in_y + top + asm::BACKGROUND_TILE * ty,
                    width: asm::BACKGROUND_TILE,
                    height: asm::BACKGROUND_TILE,
                    sheet: Sheet::AdvBackground(path),
                    fill: Fill::Stretch,
                    color: [1.0; 4],
                });
            }
        }
    }
    for bg in [true, false] {
        let black = bg;
        for (x, y, w, h) in tab.connectivity(sx, sy, bg) {
            batch.sprites.push(SpriteDraw {
                x: in_x + x,
                y: in_y + y,
                width: w,
                height: h,
                sheet: Sheet::White,
                fill: Fill::Stretch,
                color: if black { [0.0, 0.0, 0.0, 1.0] } else { [1.0; 4] },
            });
        }
    }
    for w in &tab.widgets {
        if !w.visible {
            continue;
        }
        batch.sprites.push(SpriteDraw {
            x: in_x + sx + w.x + 3,
            y: in_y + sy + w.y,
            width: 26,
            height: 26,
            sheet: frame_sheet(w.frame, w.percent >= 1.0),
            fill: Fill::Stretch,
            color: [1.0; 4],
        });
    }
    draw.scissored.push(batch);

    // extractWindow — the 252x140 frame over the clipped contents, then the
    // tab strip (only past one tab), then the title text lives in lines().
    draw.sprites.push(SpriteDraw {
        x: win_x,
        y: win_y,
        width: asm::WINDOW_W,
        height: asm::WINDOW_H,
        sheet: Sheet::AdvWindow,
        fill: Fill::Stretch,
        color: [1.0; 4],
    });
    if view.screen.tabs.len() > 1 {
        for (i, t) in view.screen.tabs.iter().enumerate() {
            let selected = view.screen.selected == Some(i);
            let (x, y, w, h) = t.kind.rect_at(t.index);
            draw.sprites.push(SpriteDraw {
                x: win_x + x,
                y: win_y + y,
                width: w,
                height: h,
                sheet: tab_sheet(t.kind, t.kind.sprite_cap(t.index), selected),
                fill: Fill::Stretch,
                color: [1.0; 4],
            });
        }
    }

    // extractTooltips — the fade overlay covers the INSIDE area, BLACK at the
    // fade alpha, drawn after the window (nextStratum) and unscissored.
    if tab.fade > 0.0 {
        draw.sprites.push(SpriteDraw {
            x: in_x,
            y: in_y,
            width: asm::INSIDE_W,
            height: asm::INSIDE_H,
            sheet: Sheet::White,
            fill: Fill::Stretch,
            color: [0.0, 0.0, 0.0, tab.fade],
        });
    }
    // The hovered tooltip itself. The flip test mixes the WINDOW left with
    // contents coordinates: `screenxo + xo + x + width + 26 >= screenWidth`.
    if let Some(g) = asm::tooltip_geom(tab, win_x, screen_width) {
        let w = &tab.widgets[g.widget];
        if g.show_box {
            draw.sprites.push(SpriteDraw {
                x: in_x + g.title_left,
                y: in_y + g.box_y,
                width: w.width,
                height: g.box_height,
                sheet: Sheet::AdvTitleBox,
                fill: Fill::NineSlice([10, 10, 10, 10]),
                color: [1.0; 4],
            });
        }
        push_bar(&mut draw.sprites, w, &g, in_x, in_y);
        draw.sprites.push(SpriteDraw {
            x: in_x + g.frame_pos.0,
            y: in_y + g.frame_pos.1,
            width: 26,
            height: 26,
            sheet: frame_sheet(w.frame, g.icon_obtained),
            fill: Fill::Stretch,
            color: [1.0; 4],
        });
    }
    draw
}

/// The hover tooltip's two-half progress bar — `extractHover`'s blit pair:
/// differing halves sample the OBTAINED sheet from u=0 and the UNOBTAINED
/// sheet ending at u=200; matching halves collapse to one full-width blit.
fn push_bar(
    out: &mut Vec<SpriteDraw>,
    w: &asm::Widget,
    g: &asm::TooltipGeom,
    ox: i32,
    oy: i32,
) {
    let (first_half_w, first_obt, second_obt, _) = w.bar_split(w.width);
    let second_bar_w = w.width - first_half_w;
    if first_obt != second_obt {
        out.push(SpriteDraw {
            x: ox + g.title_left,
            y: oy + g.title_top,
            width: first_half_w,
            height: g.title_bar_height,
            sheet: Sheet::AdvBoxObtained,
            fill: Fill::SubRect(0, 0, first_half_w.max(0), 26),
            color: [1.0; 4],
        });
        let second_u = 200 - second_bar_w;
        out.push(SpriteDraw {
            x: ox + g.title_left + first_half_w,
            y: oy + g.title_top,
            width: second_bar_w,
            height: g.title_bar_height,
            sheet: Sheet::AdvBoxUnobtained,
            fill: Fill::SubRect(second_u, 0, second_bar_w.max(0), 26),
            color: [1.0; 4],
        });
    } else {
        let sheet = if first_obt {
            Sheet::AdvBoxObtained
        } else {
            Sheet::AdvBoxUnobtained
        };
        out.push(SpriteDraw {
            x: ox + g.title_left,
            y: oy + g.title_top,
            width: w.width,
            height: g.title_bar_height,
            sheet,
            fill: Fill::Stretch,
            color: [1.0; 4],
        });
    }
}

/// `Sheet::AdvFrame` index: `type*2 + obtained`.
fn frame_sheet(f: Frame, obtained: bool) -> Sheet {
    let t = match f {
        Frame::Task => 0u8,
        Frame::Challenge => 1,
        Frame::Goal => 2,
    };
    Sheet::AdvFrame(t * 2 + obtained as u8)
}

/// `Sheet::AdvTab` index: `kind*6 + cap*2 + selected` — the bake's order.
fn tab_sheet(kind: TabKind, cap: asm::Cap, selected: bool) -> Sheet {
    let k = match kind {
        TabKind::Above => 0u8,
        TabKind::Below => 1,
        TabKind::Left => 2,
        TabKind::Right => 3,
    };
    let c = match cap {
        asm::Cap::First => 0u8,
        asm::Cap::Middle => 1,
        asm::Cap::Last => 2,
    };
    Sheet::AdvTab(k * 6 + c * 2 + selected as u8)
}

// ─── Text ────────────────────────────────────────────────────────────────────

/// One frame's texts, all in absolute GUI px scaled by `px`. Vanilla's colours:
/// the header and tooltip titles are white (`-1`), the WINDOW title is
/// `-12566464` = `0xFF404040`, the description rides its frame's chat colour,
/// and the empty-state labels are white centred.
pub fn lines(
    view: &AdvancementsView,
    lang: &Language,
    gui_w: i32,
    gui_h: i32,
    screen_width: i32,
    px: f32,
    advance: &[u8; 256],
) -> Vec<OwnedTextLine> {
    let mut out = Vec::new();
    let measure = |t: &str| rewo_gpu::text::width(t, advance);
    let (win_x, win_y) = asm::window_origin(gui_w, gui_h);
    let (in_x, in_y) = (win_x + asm::INSIDE_X, win_y + asm::INSIDE_Y);

    // The layout header: `addTitleHeader(TITLE)` centres the 9px-tall text in
    // the 33-tall header band → y = 12.
    let mut push =
        |out: &mut Vec<OwnedTextLine>, text: &str, x: i32, y: i32, rgb: u32, centered: bool| {
            if text.is_empty() {
                return;
            }
            let x = if centered {
                x - measure(text) / 2
            } else {
                x
            };
            out.push(OwnedTextLine {
                x: x as f32 * px,
                y: y as f32 * px,
                px,
                color_linear: srgb_bytes_to_linear_f([
                    ((rgb >> 16) & 0xFF) as f32,
                    ((rgb >> 8) & 0xFF) as f32,
                    (rgb & 0xFF) as f32,
                ]),
                alpha: 1.0,
                shadow: true,
                style: rewo_gpu::text::TextStyle::PLAIN,
                text: text.to_string(),
            });
        };

    push(
        &mut out,
        &lang.or_key("gui.advancements"),
        screen_width / 2,
        12,
        0xFF_FFFF,
        true,
    );

    let sel = view.screen.selected.and_then(|i| view.screen.tabs.get(i));
    let scroll = sel.map(|t| t.scroll_int()).unwrap_or((0, 0));
    match sel {
        Some(tab) => {
            // extractWindow's title line, at (leftPos + 8, topPos + 6).
            push(
                &mut out,
                &tab.title,
                win_x + asm::TITLE_X,
                win_y + asm::TITLE_Y,
                0x40_4040,
                false,
            );
            // extractHover's three text groups for the hovered widget.
            if let Some(g) = asm::tooltip_geom(tab, win_x, screen_width) {
                let w = &tab.widgets[g.widget];
                for (li, line) in w.title_lines.iter().enumerate() {
                    let tx = if g.left_side {
                        g.description_left
                    } else {
                        sx_title_x(tab, g)
                    };
                    push(
                        &mut out,
                        line,
                        in_x + tx,
                        in_y + g.title_top + 9 + 9 * li as i32,
                        0xFF_FFFF,
                        false,
                    );
                }
                if let Some((text, tw)) = &w.progress_text {
                    let tx = if g.left_side {
                        scroll.0 + w.x - tw
                    } else {
                        scroll.0 + w.x + w.width - tw - 5
                    };
                    push(&mut out, text, in_x + tx, in_y + g.title_top + 9, 0xFF_FFFF, false);
                }
                for (li, line) in w.description_lines.iter().enumerate() {
                    push(
                        &mut out,
                        line,
                        in_x + g.description_left,
                        in_y + g.description_y + 9 * li as i32,
                        w.frame.chat_color(),
                        false,
                    );
                }
            }
        }
        None => {
            // extractInside's empty state — two centred labels. The window
            // title falls back to the plain TITLE component there.
            push(
                &mut out,
                &lang.or_key("advancements.empty"),
                in_x + asm::INSIDE_W / 2,
                in_y + 52,
                0xFF_FFFF,
                true,
            );
            push(
                &mut out,
                &lang.or_key("advancements.sad_label"),
                in_x + asm::INSIDE_W / 2,
                in_y + asm::INSIDE_H - 9,
                0xFF_FFFF,
                true,
            );
        }
    }
    out
}

/// The non-flipped tooltip title x — `xo + this.x + 32`, contents space
/// (extractHover's `xo` IS the scroll offset).
fn sx_title_x(tab: &asm::Tab, g: asm::TooltipGeom) -> i32 {
    let (sx, _sy) = tab.scroll_int();
    let w = &tab.widgets[g.widget];
    sx + w.x + 32
}

// ─── Icons ───────────────────────────────────────────────────────────────────

/// One item icon the app owes the GUI-item pass this frame — lowered to a
/// [`rewo_gpu::gui_item::GuiItem`] by the caller, which owns the item table.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct IconDraw {
    pub item: i32,
    pub count: i32,
    /// Screen pixels.
    pub x: f32,
    pub y: f32,
    /// 16 GUI px scaled — `fakeItem`'s slot size.
    pub size: f32,
}

/// Every icon the advancements screen draws: one per tab cell (past one tab),
/// one per visible widget, and the hovered widget's duplicate over its
/// tooltip. Positions are ABSOLUTE screen px (the scale is baked in).
pub fn icon_draws(view: &AdvancementsView, gui_w: i32, gui_h: i32, px: f32) -> Vec<IconDraw> {
    let mut out = Vec::new();
    let (win_x, win_y) = asm::window_origin(gui_w, gui_h);
    let (in_x, in_y) = (win_x + asm::INSIDE_X, win_y + asm::INSIDE_Y);
    let s = px;
    let Some(sel) = view.screen.selected else {
        return out;
    };
    let Some(tab) = view.screen.tabs.get(sel) else {
        return out;
    };
    let (sx, sy) = tab.scroll_int();

    if view.screen.tabs.len() > 1 {
        for t in &view.screen.tabs {
            let (ox, oy) = t.kind.icon_offset();
            let (x, y, _, _) = t.kind.rect_at(t.index);
            out.push(IconDraw {
                item: t.icon.item,
                count: t.icon.count,
                x: (win_x + x + ox) as f32 * s,
                y: (win_y + y + oy) as f32 * s,
                size: 16.0 * s,
            });
        }
    }
    for w in &tab.widgets {
        if !w.visible {
            continue;
        }
        out.push(IconDraw {
            item: w.icon.item,
            count: w.icon.count,
            x: (in_x + sx + w.x + 8) as f32 * s,
            y: (in_y + sy + w.y + 5) as f32 * s,
            size: 16.0 * s,
        });
    }
    out
}
