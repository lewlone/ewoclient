//! The statistics screen's render half (M84).
//!
//! `rewo_world::stats_screen` owns the layout and the rows; this turns them
//! into a [`rewo_gpu::screen::ScreenDraw`] and a list of text runs. Split out
//! of `live_cmd` rather than added to it for one reason worth stating: the gate
//! must drive **these** builders, and a function buried in a 10k-line file with
//! three concurrent editors is a function that quietly grows a second caller.
//!
//! # What is drawn, in vanilla's own order
//!
//! 1. `extractMenuBackground` — `tab_header_background` tiled over the header
//!    strip, then `inworld_menu_background` tiled over the body.
//! 2. `MenuTabBar.extractWidgetRenderState` — the two header separators, then
//!    the tabs themselves.
//! 3. The list's rows (`extractListBackground` and `extractListSeparators` are
//!    **overridden empty** by all three statistics lists, so there is no list
//!    background and no separator around it).
//! 4. `extractScrollbar`.
//! 5. `StatsScreen.extractRenderState` — the footer separator.
//! 6. The footer's `Done` button.
//!
//! # Three tiling call sites, three different declared texture sizes
//!
//! `graphics.blit(pipeline, texture, x, y, u, v, w, h, texW, texH)` samples
//! `u .. u + w` out of a `texW × texH` texture, so `texW`/`texH` set the tile
//! period — and they are **not always the file's own size**:
//!
//! | call | file | declared | effect |
//! |---|---|---|---|
//! | `TAB_HEADER_BACKGROUND` | 16×16 | 16×16 | 1:1, repeats every 16 |
//! | `INWORLD_MENU_BACKGROUND` | 16×16 | **32×32** | drawn at 2×, repeats every 32 |
//! | `HEADER_SEPARATOR` | 32×2 | 32×2 | 1:1 |
//!
//! Taking the file's size for the second draws the body background at half
//! scale, which reads as "slightly noisier" and nothing else.
//!
//! # Two deviations, both named
//!
//! * **Rows are not scissored.** Vanilla wraps the row draw in
//!   `enableScissor(list rect)`, so a row straddling the top or bottom edge is
//!   half-drawn. Rewo's text pass has no scissor, so a row whose text baseline
//!   would fall outside the band is **skipped entirely**. The visible
//!   difference is a row popping in rather than sliding in.
//! * **An items row draws its registry name** where vanilla draws only an item
//!   icon. See `rewo_world::stats_screen`'s module docs for why.

use rewo_gpu::screen::{ButtonDraw, ButtonSprite, Fill, ScreenDraw, Sheet, SpriteDraw};
use rewo_gpu::world::OwnedTextLine;
use rewo_world::screen::{Screen, Sprite, WidgetKind, SCROLLBAR_WIDTH};
use rewo_world::stats_screen::{self as ss, StatsLabels, StatsModel, StatsTab};

/// Everything the statistics screen holds across frames.
pub struct StatsView {
    pub model: StatsModel,
    pub labels: StatsLabels,
    /// The `StatsCounter::updates` watermark the model was built from, so a
    /// later `award_stats` rebuilds it exactly once.
    pub built_from: u64,
}

/// `Font.lineHeight`.
const LINE_HEIGHT: i32 = 9;

impl StatsView {
    /// `StatsScreen`'s constructor + `init()` + `onStatsUpdated()`.
    #[allow(clippy::too_many_arguments)]
    pub fn build(
        counter: &rewo_world::stats::StatsCounter,
        reg: &rewo_data::stats::StatRegistries,
        items: &rewo_data::items::Items,
        types: &rewo_data::entity_types::EntityTypes,
        lang: &rewo_data::lang::Language,
        labels: StatsLabels,
        tab: StatsTab,
        sort: (Option<usize>, i32),
        gui_w: i32,
        gui_h: i32,
    ) -> (Self, Screen) {
        let model = StatsModel::build(
            counter, reg, items, types, lang, tab, sort, gui_w, gui_h,
        );
        let screen = model.build_screen(&labels, gui_w, gui_h);
        let built_from = model.built_from;
        (
            Self {
                model,
                labels,
                built_from,
            },
            screen,
        )
    }

    /// A widget press. Returns `true` when the screen must be rebuilt (a tab
    /// change moves the sort buttons and reselects a sheet).
    pub fn press(&mut self, id: rewo_world::screen::WidgetId) -> bool {
        if let Some(tab) = StatsTab::from_widget(id) {
            if self.model.tab_active(tab) {
                self.model.tab = tab;
            }
            return true;
        }
        if (ss::SORT_FIRST..ss::SORT_FIRST + ss::COLUMNS.len() as u32).contains(&id) {
            self.model.sort_by_column((id - ss::SORT_FIRST) as usize);
            return true;
        }
        false
    }
}

/// The screen's chrome for one frame.
pub fn chrome(
    view: &StatsView,
    screen: &Screen,
    mouse: Option<(f64, f64)>,
    advance: Option<&[u8; 256]>,
) -> ScreenDraw {
    let (gui_w, gui_h) = (screen.width, screen.height);
    let m = &view.model;
    let mut sprites: Vec<SpriteDraw> = Vec::new();
    let put = |v: &mut Vec<SpriteDraw>, x, y, width, height, sheet, fill| {
        v.push(SpriteDraw {
            x,
            y,
            width,
            height,
            sheet,
            fill,
            color: [1.0; 4],
        })
    };

    // 1. `StatsScreen.extractMenuBackground`.
    put(
        &mut sprites,
        0,
        0,
        gui_w,
        ss::HEADER_HEIGHT,
        Sheet::TabHeaderBackground,
        Fill::Tiled(16, 16),
    );
    // `extractMenuBackground(graphics, 0, headerHeight, this.width, this.height)`
    // — the *height*, not `height - headerHeight`, so the body background
    // overruns the bottom of the screen by exactly the header's height.
    // Transcribed rather than tidied: it is invisible, and "fixing" it is the
    // kind of edit that is right until a screen puts something under it.
    put(
        &mut sprites,
        0,
        ss::HEADER_HEIGHT,
        gui_w,
        gui_h,
        Sheet::InworldMenuBackground,
        Fill::Tiled(32, 32),
    );

    // 2. The tab bar's two header separators, at
    //    `layout.getY() + layout.getHeight() - 2`.
    let (tab_x, tab_w) = ss::tab_bar_layout(gui_w, StatsTab::ALL.len() as i32);
    let sep_y = ss::HEADER_HEIGHT - 2;
    put(
        &mut sprites,
        0,
        sep_y,
        tab_x,
        2,
        Sheet::InworldHeaderSeparator,
        Fill::Tiled(32, 2),
    );
    let after_last = tab_x + tab_w * StatsTab::ALL.len() as i32;
    put(
        &mut sprites,
        after_last,
        sep_y,
        gui_w,
        2,
        Sheet::InworldHeaderSeparator,
        Fill::Tiled(32, 2),
    );

    // 3. The widgets that are not `widget/button`: the tabs and, on the items
    //    tab, the six sort buttons.
    let focused = screen.focused();
    let mut buttons = Vec::new();
    for w in screen.widgets.iter().filter(|w| w.visible) {
        let hovered_or_focused = w.is_hovered(mouse) || focused == Some(w.id);
        match &w.kind {
            // M85's `Label` / `MultiLabel` / `Reserved` are chrome-less by
            // construction and the statistics screen builds none of them; they
            // are named rather than caught by a wildcard so a screen that does
            // start using one fails here instead of drawing nothing.
            WidgetKind::Label { .. }
            | WidgetKind::MultiLabel { .. }
            | WidgetKind::Reserved
            // M173: the statistics screen builds no sliders either.
            | WidgetKind::Slider { .. } => {}
            WidgetKind::Button => buttons.push(ButtonDraw {
                x: w.x,
                y: w.y,
                width: w.width,
                height: w.height,
                sprite: match w.sprite(w.is_hovered(mouse), focused == Some(w.id)) {
                    rewo_world::screen::ButtonSprite::Enabled => ButtonSprite::Enabled,
                    rewo_world::screen::ButtonSprite::Disabled => ButtonSprite::Disabled,
                    rewo_world::screen::ButtonSprite::Highlighted => ButtonSprite::Highlighted,
                },
            }),
            WidgetKind::Sprites {
                sprites: s,
                first,
                overlay,
                ..
            } => {
                let base = s.get(*first, hovered_or_focused);
                let (sheet, fill) = lower(base);
                put(&mut sprites, w.x, w.y, w.width, w.height, sheet, fill);
                // A selected tab paints `Screen.MENU_BACKGROUND` inside itself
                // — `renderMenuBackground(x + 2, y + 2, right - 2, bottom)` —
                // and then a 1-px focus underline.
                if matches!(base, Sprite::TabSelected | Sprite::TabSelectedHighlighted) {
                    put(
                        &mut sprites,
                        w.x + 2,
                        w.y + 2,
                        w.width - 4,
                        w.height - 2,
                        Sheet::MenuBackground,
                        Fill::Tiled(32, 32),
                    );
                }
                if let Some(o) = overlay {
                    let (sheet, fill) = lower(*o);
                    put(&mut sprites, w.x, w.y, w.width, w.height, sheet, fill);
                }
            }
        }
    }

    // The selected tab's focus underline, drawn last of the tab chrome so it
    // sits over the menu-background patch:
    //   width = min(font.width(message), getWidth() - 4)
    //   left  = getX() + (getWidth() - width) / 2
    //   top   = getY() + getHeight() - 2, height 1
    // The colour is `active ? -1 : -6250336`, i.e. the same inactive grey a
    // dead button's *label* takes.
    if let Some(w) = screen
        .widgets
        .iter()
        .find(|w| w.visible && StatsTab::from_widget(w.id) == Some(m.tab))
    {
        // `Math.min(font.width(getMessage()), getWidth() - 4)`. With no font
        // the underline degenerates to the clamp, which is what a bare
        // `getWidth() - 4` would have been anyway.
        let label = advance
            .map(|a| rewo_gpu::text::width(&w.message, a))
            .unwrap_or(i32::MAX);
        let width = label.min(w.width - 4).max(0);
        let color = if w.active {
            [1.0, 1.0, 1.0, 1.0]
        } else {
            [160.0 / 255.0, 160.0 / 255.0, 160.0 / 255.0, 1.0]
        };
        sprites.push(SpriteDraw {
            x: w.x + (w.width - width) / 2,
            y: w.y + w.height - 2,
            width,
            height: 1,
            sheet: Sheet::White,
            fill: Fill::Stretch,
            color,
        });
    }

    // 4. The rows' own sprites: an item row's slot, and the sort arrow.
    if m.tab == StatsTab::Items && !m.items.is_empty() {
        let list = m.list();
        for row in 0..list.rows.len() {
            let (cx, cy, ..) = list.content_rect(row);
            if !row_visible(list, row) {
                continue;
            }
            if row == 0 {
                if let Some(col) = m.sort_column {
                    let sheet = if m.sort_order == 1 {
                        Sheet::SortUp
                    } else {
                        Sheet::SortDown
                    };
                    put(
                        &mut sprites,
                        cx + ss::column_x(col) - 36,
                        cy + 1,
                        ss::SLOT_SIZE,
                        ss::SLOT_SIZE,
                        sheet,
                        Fill::Stretch,
                    );
                }
            } else {
                put(
                    &mut sprites,
                    cx,
                    cy,
                    ss::SLOT_SIZE,
                    ss::SLOT_SIZE,
                    Sheet::Slot,
                    Fill::Stretch,
                );
            }
        }
    }

    // 5. The scrollbar. `scrollable()` is strictly `maxScrollAmount() > 0`, so
    //    a list that exactly fills its box draws nothing at all — not an empty
    //    track.
    let list = m.list();
    if list.scrollable() {
        put(
            &mut sprites,
            list.scroll_bar_x(),
            list.y,
            SCROLLBAR_WIDTH,
            list.height,
            Sheet::ScrollerBackground,
            Fill::NineSlice(rewo_gpu::screen::SCROLLER_BORDER),
        );
        put(
            &mut sprites,
            list.scroll_bar_x(),
            list.scroll_bar_y(),
            SCROLLBAR_WIDTH,
            list.scroller_height(),
            Sheet::Scroller,
            Fill::NineSlice(rewo_gpu::screen::SCROLLER_BORDER),
        );
    }

    // 6. The footer separator, at `height - footerHeight`.
    put(
        &mut sprites,
        0,
        gui_h - ss::FOOTER_HEIGHT,
        gui_w,
        2,
        Sheet::InworldFooterSeparator,
        Fill::Tiled(32, 2),
    );

    ScreenDraw {
        // `isInGameUi()` is false, so `extractBackground` takes the menu branch
        // and there is no gradient at all — the tiles above are the whole of it.
        backdrop: None,
        // Nor M85's full-screen `menu_background`: `StatsScreen` **overrides**
        // `extractMenuBackground` and paints two sheets of its own, a
        // `tab_header_background` strip over the header and
        // `inworld_menu_background` below it. Setting M85's field here would
        // draw a third, full-screen copy underneath both.
        menu_background: None,
        buttons,
        sprites,
    }
}

/// `rewo_world::screen::Sprite` → this pass's sheet + how it fills.
fn lower(s: Sprite) -> (Sheet, Fill) {
    let nine = Fill::NineSlice(rewo_gpu::screen::TAB_BORDER);
    match s {
        Sprite::TabSelected => (Sheet::Tab(0), nine),
        Sprite::Tab => (Sheet::Tab(1), nine),
        Sprite::TabSelectedHighlighted => (Sheet::Tab(2), nine),
        Sprite::TabHighlighted => (Sheet::Tab(3), nine),
        // `container/slot` and the `statistics/*` sheets carry no `.mcmeta`,
        // so they are `Stretch`-scaled — and every one is drawn at its own
        // 18×18, where that is a 1:1 blit anyway.
        Sprite::StatHeader => (Sheet::StatHeader, Fill::Stretch),
        Sprite::Slot => (Sheet::Slot, Fill::Stretch),
        Sprite::StatColumn(i) => (Sheet::StatColumn(i), Fill::Stretch),
        Sprite::SortUp => (Sheet::SortUp, Fill::Stretch),
        Sprite::SortDown => (Sheet::SortDown, Fill::Stretch),
    }
}

/// Whether a row's content band is wholly inside the list's own box.
///
/// The scissor deviation, in one place — see the module docs.
fn row_visible(list: &rewo_world::screen::ScrollList, row: usize) -> bool {
    list.row_top(row) >= list.y && list.row_bottom(row) <= list.bottom()
}

/// Every text run the statistics screen draws.
///
/// `px` is the GUI scale, and every coordinate below is in GUI pixels — the
/// same convention `death_screen_lines` uses.
pub fn lines(
    view: &StatsView,
    screen: &Screen,
    advance: &[u8; 256],
    px: f32,
) -> Vec<OwnedTextLine> {
    let m = &view.model;
    let mut out = Vec::new();
    // Every colour below (`ROW_DIM` 0xBABABA, `ROW_GREY` 0x808080,
    // `INACTIVE_LABEL` 0xA0A0A0) is vanilla's byte `/255`; the text pass wants
    // linear. One conversion, in the one closure every row goes through.
    let run = |out: &mut Vec<OwnedTextLine>, text: &str, x: i32, y: i32, color: [f32; 3]| {
        if text.is_empty() {
            return;
        }
        out.push(OwnedTextLine {
            x: x as f32 * px,
            y: y as f32 * px,
            px,
            color_linear: crate::live_cmd::srgb_bytes_to_linear_f(color),
            alpha: 1.0,
            shadow: true,
            style: rewo_gpu::text::TextStyle::PLAIN,
            text: text.to_string(),
        });
    };

    // Each widget's own label. A tab draws one; a statistics sort button does
    // not (its message is a tooltip).
    for w in screen.widgets.iter().filter(|w| w.visible) {
        let draws_label = match &w.kind {
            WidgetKind::Button => true,
            WidgetKind::Sprites { label, .. } => *label,
            // See `chrome`: the statistics screen builds none of M85's three.
            WidgetKind::Label { .. }
            | WidgetKind::MultiLabel { .. }
            | WidgetKind::Reserved
            | WidgetKind::Slider { .. } => false,
        };
        if !draws_label {
            continue;
        }
        let width = rewo_gpu::text::width(&w.message, advance);
        let (anchor, top) = match &w.kind {
            // `MenuTabButton.renderLabel` — its own box, and the top moves
            // down by 3 when the tab is *not* selected, which is what makes a
            // selected tab look taller.
            WidgetKind::Sprites { first, .. } => {
                let left = w.x + 1;
                let right = w.x + w.width - 1;
                let top = w.y + if *first { 0 } else { 3 };
                let bottom = w.y + w.height;
                let text_top = (top + bottom - LINE_HEIGHT) / 2 + 1;
                ((left + right) / 2, text_top)
            }
            WidgetKind::Button => w.label_anchor(width),
            _ => unreachable!("filtered by draws_label above"),
        };
        run(
            &mut out,
            &w.message,
            anchor - width / 2,
            top,
            w.label_color(),
        );
    }

    if m.loading {
        // `LoadingTab`'s own content: the pending string, centred in the
        // content band.
        let (y, h) = ss::content_band(screen.height);
        let width = rewo_gpu::text::width(&view.labels.pending, advance);
        run(
            &mut out,
            &view.labels.pending,
            screen.width / 2 - width / 2,
            y + h / 2 - LINE_HEIGHT / 2,
            ss::ROW_WHITE,
        );
        return out;
    }

    let list = m.list();
    match m.tab {
        StatsTab::General => {
            for (i, row) in m.general.iter().enumerate() {
                if !row_visible(list, i) {
                    continue;
                }
                let (cx, cy, cw, ch) = list.content_rect(i);
                // `getContentYMiddle() - 9 / 2`.
                let y = cy + ch / 2 - LINE_HEIGHT / 2;
                let color = if i % 2 == 0 { ss::ROW_WHITE } else { ss::ROW_DIM };
                run(&mut out, &row.label, cx + 2, y, color);
                let vw = rewo_gpu::text::width(&row.value, advance);
                run(&mut out, &row.value, cx + cw - vw - 4, y, color);
            }
        }
        StatsTab::Mobs => {
            for (i, row) in m.mobs.iter().enumerate() {
                if !row_visible(list, i) {
                    continue;
                }
                let (cx, cy, ..) = list.content_rect(i);
                run(&mut out, &row.name, cx + 2, cy + 1, ss::ROW_WHITE);
                let dim = |on: bool| if on { ss::ROW_DIM } else { ss::ROW_GREY };
                run(
                    &mut out,
                    &row.kills,
                    cx + 2 + 10,
                    cy + 1 + LINE_HEIGHT,
                    dim(row.has_kills),
                );
                run(
                    &mut out,
                    &row.killed_by,
                    cx + 2 + 10,
                    cy + 1 + LINE_HEIGHT * 2,
                    dim(row.was_killed_by),
                );
            }
        }
        StatsTab::Items => {
            for (row, item) in m.items.iter().enumerate() {
                let entry = row + 1; // row 0 is the header
                if !row_visible(list, entry) {
                    continue;
                }
                let (cx, cy, _, ch) = list.content_rect(entry);
                // `index % 2 == 0 ? -1 : -4539718`, where `index` counts the
                // header — so the first *item* row is index 1 and dim.
                let color = if entry % 2 == 0 {
                    ss::ROW_WHITE
                } else {
                    ss::ROW_DIM
                };
                let y = cy + ch / 2 - LINE_HEIGHT / 2;
                // Rewo's deviation: the row's name where vanilla has only an
                // icon. Placed just right of the 18-px slot.
                run(&mut out, &item.short_name, cx + ss::SLOT_SIZE + 2, y, color);
                for (col, cell) in item.cells.iter().enumerate() {
                    let text = cell.clone().unwrap_or_else(|| view.labels.none.clone());
                    let w = rewo_gpu::text::width(&text, advance);
                    // `graphics.text(font, msg, x - font.width(msg), y, …)` —
                    // right-aligned **on** the column, not left of it.
                    run(&mut out, &text, cx + ss::column_x(col) - w, y, color);
                }
            }
        }
    }
    out
}
