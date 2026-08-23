//! `BookViewScreen` — the written-book reader (M171).
//!
//! Opened by `open_book`; the pages come from the held item's
//! `written_book_content` (captured in `StackComponents::book_pages`, M171),
//! resolved to styled [`ChatLine`]s and wrapped to `TEXT_WIDTH` by the app
//! before construction (wrapping needs the font's width provider, which the
//! model has no access to). This module is the layout and page navigation,
//! pure and testable; the render is in `live_cmd`.
//!
//! Layout, verbatim from `BookViewScreen.java`: the `book.png` background is
//! `192 x 192` at `((width - 192) / 2, 2)` — **top is a fixed 2, not centred**;
//! the page text starts at `(left + 36, top + 30)` and steps `9` px, at most
//! `128 / 9 = 14` lines; the right-aligned page indicator is at `(left + 148,
//! top + 16)`; the back/forward `PageButton`s are `23 x 13` at `(left + 43,
//! top + 157)` and `(left + 116, top + 157)`.

use crate::chat_style::ChatLine;

pub const IMAGE_W: i32 = 192;
pub const IMAGE_H: i32 = 192;
pub const BACKGROUND_TOP: i32 = 2;
/// `TEXT_WIDTH` — the wrap width the app splits pages to.
pub const TEXT_WIDTH: i32 = 114;
/// `TEXT_HEIGHT / 9` — the most lines a page shows.
pub const MAX_LINES: usize = 128 / 9; // 14
pub const LINE_HEIGHT: i32 = 9;
pub const PAGE_TEXT_X_OFFSET: i32 = 36;
pub const PAGE_TEXT_Y_OFFSET: i32 = 30;
pub const PAGE_INDICATOR_X_OFFSET: i32 = 148;
pub const PAGE_INDICATOR_TEXT_Y_OFFSET: i32 = 16;
pub const PAGE_BUTTON_Y: i32 = 157;
pub const PAGE_BACK_BUTTON_X: i32 = 43;
pub const PAGE_FORWARD_BUTTON_X: i32 = 116;
pub const PAGE_BUTTON_W: i32 = 23;
pub const PAGE_BUTTON_H: i32 = 13;
/// `PAGE_TEXT_STYLE` — black (`0xFF000000` → `0x000000` opaque), no shadow.
pub const PAGE_TEXT_COLOR: u32 = 0x0000_00;

/// GLFW `GLFW_KEY_PAGE_UP` / `GLFW_KEY_PAGE_DOWN` — the two keys the screen
/// binds (NOT the arrow keys).
pub const KEY_PAGE_UP: i32 = 266;
pub const KEY_PAGE_DOWN: i32 = 267;

/// A screen rect `(x, y, w, h)`.
pub type Rect = (i32, i32, i32, i32);

/// The written-book reader. `pages` are pre-wrapped styled lines, one inner
/// `Vec` per page.
#[derive(Clone, Debug, Default)]
pub struct BookViewScreen {
    pub pages: Vec<Vec<ChatLine>>,
    current_page: usize,
}

impl BookViewScreen {
    pub fn new(pages: Vec<Vec<ChatLine>>) -> Self {
        Self {
            pages,
            current_page: 0,
        }
    }

    /// `getNumPages` / `bookAccess.getPageCount`.
    pub fn page_count(&self) -> usize {
        self.pages.len()
    }

    pub fn current_page(&self) -> usize {
        self.current_page
    }

    /// `pageBack` — decrement, floored at 0.
    pub fn page_back(&mut self) {
        if self.current_page > 0 {
            self.current_page -= 1;
        }
    }

    /// `pageForward` — increment, capped at the last page.
    pub fn page_forward(&mut self) {
        if self.current_page + 1 < self.page_count() {
            self.current_page += 1;
        }
    }

    /// `updateButtonVisibility` — the forward button shows while there is a
    /// next page.
    pub fn forward_visible(&self) -> bool {
        self.current_page + 1 < self.page_count()
    }

    /// The back button shows while there is a previous page.
    pub fn back_visible(&self) -> bool {
        self.current_page > 0
    }

    /// The lines of the current page, capped to [`MAX_LINES`] as vanilla's
    /// `Math.min(128 / 9, size)` does.
    pub fn current_lines(&self) -> &[ChatLine] {
        let page = self.pages.get(self.current_page).map(Vec::as_slice).unwrap_or(&[]);
        &page[..page.len().min(MAX_LINES)]
    }

    /// The one-based page indicator numbers `(current, max(count, 1))`, as
    /// `Component.translatable("book.pageIndicator", currentPage + 1,
    /// max(numPages, 1))`.
    pub fn indicator(&self) -> (usize, usize) {
        (self.current_page + 1, self.page_count().max(1))
    }

    pub fn background_left(width: i32) -> i32 {
        (width - IMAGE_W) / 2
    }

    /// The page-text top-left in screen pixels.
    pub fn text_origin(width: i32) -> (i32, i32) {
        (
            Self::background_left(width) + PAGE_TEXT_X_OFFSET,
            BACKGROUND_TOP + PAGE_TEXT_Y_OFFSET,
        )
    }

    /// The RIGHT edge the page indicator is aligned to.
    pub fn indicator_right(width: i32) -> (i32, i32) {
        (
            Self::background_left(width) + PAGE_INDICATOR_X_OFFSET,
            BACKGROUND_TOP + PAGE_INDICATOR_TEXT_Y_OFFSET,
        )
    }

    pub fn back_rect(width: i32) -> Rect {
        (
            Self::background_left(width) + PAGE_BACK_BUTTON_X,
            BACKGROUND_TOP + PAGE_BUTTON_Y,
            PAGE_BUTTON_W,
            PAGE_BUTTON_H,
        )
    }

    pub fn forward_rect(width: i32) -> Rect {
        (
            Self::background_left(width) + PAGE_FORWARD_BUTTON_X,
            BACKGROUND_TOP + PAGE_BUTTON_Y,
            PAGE_BUTTON_W,
            PAGE_BUTTON_H,
        )
    }

    /// A click at `(mx, my)`. Turns the page if it lands on a VISIBLE button,
    /// and returns whether it did (so the caller can consume the click).
    pub fn click(&mut self, mx: i32, my: i32, width: i32) -> bool {
        if self.forward_visible() && in_rect(mx, my, Self::forward_rect(width)) {
            self.page_forward();
            return true;
        }
        if self.back_visible() && in_rect(mx, my, Self::back_rect(width)) {
            self.page_back();
            return true;
        }
        false
    }

    /// A key press. `PageUp`/`PageDown` turn the page; returns whether handled.
    pub fn key(&mut self, key: i32) -> bool {
        match key {
            KEY_PAGE_UP => {
                self.page_back();
                true
            }
            KEY_PAGE_DOWN => {
                self.page_forward();
                true
            }
            _ => false,
        }
    }
}

/// `menuControlsTop()` — `backgroundTop() + 192 + 2`, where the Done button
/// sits.
pub const MENU_CONTROLS_TOP: i32 = BACKGROUND_TOP + IMAGE_H + 2; // 196

/// The Done button's widget id.
pub const DONE: crate::screen::WidgetId = 0;

/// Build the framework [`crate::screen::Screen`] for the reader (M172): the
/// transparent-gradient backdrop (`isInGameUi()` is true, so vanilla draws
/// ONLY the `0xC0101010 -> 0xD0101010` gradient — no blur, no tiled menu
/// background) and the one standard widget, the 200-wide centred Done button
/// at `menuControlsTop()`. The page ARROWS are deliberately NOT widgets: a
/// `PageButton` draws its own 23x13 sprites rather than the nine-sliced
/// button chrome, so they render through [`draws`] and hit-test through
/// [`BookViewScreen::click`].
pub fn build_screen(gui_w: i32, gui_h: i32, done_label: &str) -> crate::screen::Screen {
    crate::screen::Screen::new(crate::screen::ScreenKind::BookView, gui_w, gui_h)
        .with_backdrop(crate::screen::Backdrop::TRANSPARENT)
        .with_widgets(vec![crate::screen::Widget::button(
            DONE,
            (gui_w - crate::screen::BUTTON_WIDTH) / 2,
            MENU_CONTROLS_TOP,
            crate::screen::BUTTON_WIDTH,
            crate::screen::BUTTON_HEIGHT,
            done_label,
        )])
}

/// One book-chrome blit for the render, in GUI pixels. World-typed rather
/// than a `rewo_gpu` sprite so this stays testable with no GPU — the app maps
/// each to a `SpriteDraw` (the `screen_chrome` pattern).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BookDraw {
    /// The 192x192 `book.png` crop at `(x, BACKGROUND_TOP)`.
    Background { x: i32, y: i32 },
    /// A 23x13 `PageButton` sprite. `highlighted` is `isHoveredOrFocused()`
    /// — hover only here, since the arrows never take keyboard focus
    /// (`shouldTakeFocusAfterInteraction()` is false).
    Arrow {
        forward: bool,
        highlighted: bool,
        x: i32,
        y: i32,
    },
}

/// The book chrome to draw this frame: the background, then only the VISIBLE
/// arrows — `updateButtonVisibility` hides forward on the last page and back
/// on the first, and a hidden `PageButton` draws nothing at all.
pub fn draws(book: &BookViewScreen, gui_w: i32, mouse: Option<(i32, i32)>) -> Vec<BookDraw> {
    let mut out = vec![BookDraw::Background {
        x: BookViewScreen::background_left(gui_w),
        y: BACKGROUND_TOP,
    }];
    let hover = |r: Rect| mouse.is_some_and(|(mx, my)| in_rect(mx, my, r));
    if book.forward_visible() {
        let r = BookViewScreen::forward_rect(gui_w);
        out.push(BookDraw::Arrow {
            forward: true,
            highlighted: hover(r),
            x: r.0,
            y: r.1,
        });
    }
    if book.back_visible() {
        let r = BookViewScreen::back_rect(gui_w);
        out.push(BookDraw::Arrow {
            forward: false,
            highlighted: hover(r),
            x: r.0,
            y: r.1,
        });
    }
    out
}

fn in_rect(x: i32, y: i32, r: Rect) -> bool {
    x >= r.0 && x < r.0 + r.2 && y >= r.1 && y < r.1 + r.3
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat_style::{ChatLine, ChatSpan};

    fn line(text: &str) -> ChatLine {
        vec![ChatSpan {
            text: text.into(),
            color: [0.0, 0.0, 0.0],
            bold: false,
            italic: false,
            underlined: false,
            strikethrough: false,
            obfuscated: false,
            events: None,
        }]
    }

    fn book(pages: usize) -> BookViewScreen {
        BookViewScreen::new((0..pages).map(|i| vec![line(&format!("page {i}"))]).collect())
    }

    #[test]
    fn navigation_clamps_at_both_ends() {
        let mut b = book(3);
        assert_eq!(b.current_page(), 0);
        b.page_back(); // already at 0
        assert_eq!(b.current_page(), 0);
        assert!(b.forward_visible() && !b.back_visible());
        b.page_forward();
        assert_eq!(b.current_page(), 1);
        assert!(b.forward_visible() && b.back_visible());
        b.page_forward();
        assert_eq!(b.current_page(), 2);
        b.page_forward(); // last page
        assert_eq!(b.current_page(), 2, "capped at the last page");
        assert!(!b.forward_visible() && b.back_visible());
    }

    #[test]
    fn a_single_page_book_has_no_buttons() {
        let b = book(1);
        assert!(!b.forward_visible() && !b.back_visible());
        assert_eq!(b.indicator(), (1, 1));
    }

    #[test]
    fn an_empty_book_reports_one_page_in_the_indicator() {
        // max(numPages, 1) — the indicator never shows 0.
        let b = book(0);
        assert_eq!(b.indicator(), (1, 1));
        assert!(b.current_lines().is_empty());
    }

    #[test]
    fn a_page_shows_at_most_fourteen_lines() {
        let long: Vec<ChatLine> = (0..30).map(|i| line(&format!("l{i}"))).collect();
        let b = BookViewScreen::new(vec![long]);
        assert_eq!(b.current_lines().len(), 14, "128 / 9 = 14");
    }

    #[test]
    fn page_up_and_down_turn_the_page_but_arrows_do_not() {
        let mut b = book(3);
        assert!(b.key(KEY_PAGE_DOWN));
        assert_eq!(b.current_page(), 1);
        assert!(b.key(KEY_PAGE_UP));
        assert_eq!(b.current_page(), 0);
        assert!(!b.key(263), "left arrow is not bound");
    }

    #[test]
    fn a_click_only_turns_on_a_visible_button() {
        let mut b = book(2);
        let w = 320;
        // On page 0 the back button is hidden — a click in its rect does nothing.
        let (bx, by, _, _) = BookViewScreen::back_rect(w);
        assert!(!b.click(bx + 1, by + 1, w));
        assert_eq!(b.current_page(), 0);
        // The forward button is visible.
        let (fx, fy, _, _) = BookViewScreen::forward_rect(w);
        assert!(b.click(fx + 1, fy + 1, w));
        assert_eq!(b.current_page(), 1);
        // On the last page the FORWARD button is hidden — a click in its rect
        // is not consumed (this is what a `forward_visible()`-blind click gets
        // wrong: it would return true and no-op against the page clamp).
        assert!(!b.click(fx + 1, fy + 1, w), "a hidden forward button eats no click");
        assert_eq!(b.current_page(), 1);
        // Now back is visible, forward is not.
        assert!(b.click(bx + 1, by + 1, w));
        assert_eq!(b.current_page(), 0);
    }

    #[test]
    fn the_draw_list_shows_only_visible_arrows_and_hover_highlights() {
        let w = 320;
        let mut b = book(3);
        // Page 0: background + forward only.
        let d = draws(&b, w, None);
        assert_eq!(d.len(), 2);
        assert!(matches!(d[0], BookDraw::Background { .. }));
        assert!(matches!(
            d[1],
            BookDraw::Arrow { forward: true, highlighted: false, .. }
        ));
        // Hover over the forward rect highlights it.
        let (fx, fy, _, _) = BookViewScreen::forward_rect(w);
        let d = draws(&b, w, Some((fx + 1, fy + 1)));
        assert!(matches!(
            d[1],
            BookDraw::Arrow { forward: true, highlighted: true, .. }
        ));
        // Last page: background + back only.
        b.page_forward();
        b.page_forward();
        let d = draws(&b, w, None);
        assert_eq!(d.len(), 2);
        assert!(matches!(d[1], BookDraw::Arrow { forward: false, .. }));
    }

    #[test]
    fn the_screen_has_one_done_button_at_menu_controls_top() {
        let s = build_screen(320, 240, "Done");
        assert_eq!(s.kind, crate::screen::ScreenKind::BookView);
        assert_eq!(s.widgets.len(), 1);
        let w = &s.widgets[0];
        assert_eq!(w.id, DONE);
        assert_eq!((w.x, w.y), ((320 - 200) / 2, 196), "menuControlsTop = 2 + 192 + 2");
        assert_eq!((w.width, w.height), (200, 20));
    }

    #[test]
    fn layout_matches_the_decompile() {
        let w = 320;
        let left = (320 - 192) / 2; // 64
        assert_eq!(BookViewScreen::background_left(w), left);
        assert_eq!(BookViewScreen::text_origin(w), (left + 36, 2 + 30));
        assert_eq!(BookViewScreen::indicator_right(w), (left + 148, 2 + 16));
        assert_eq!(BookViewScreen::back_rect(w), (left + 43, 2 + 157, 23, 13));
        assert_eq!(BookViewScreen::forward_rect(w), (left + 116, 2 + 157, 23, 13));
    }
}
