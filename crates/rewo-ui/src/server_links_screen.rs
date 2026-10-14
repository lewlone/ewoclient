//! `ServerLinksDialogScreen` for the built-in `Dialogs.SERVER_LINKS` (M85) —
//! where the links actually list.
//!
//! Reached from [`crate::pause_screen`]'s one button, never opened directly.
//!
//! # This is one dialog, not a dialog system
//!
//! Vanilla's is a `DialogScreen<ServerLinksDialog>`, and `Dialog` is a
//! **datapack registry** whose entries a server can define and push with
//! `ClientboundShowDialogPacket`. Rewo builds none of that: no `Dialog`
//! registry, no `DialogScreens` codec table, no `DialogControlSet`, no
//! `ClickEvent` dispatch. What it transcribes is the *layout one specific
//! registered instance produces*, and that instance is a compile-time constant
//! in `Dialogs.bootstrap`:
//!
//! ```java
//! context.register(SERVER_LINKS, new ServerLinksDialog(
//!    new CommonDialogData(
//!       Component.translatable("menu.server_links.title"),
//!       Optional.of(Component.translatable("menu.server_links")),   // externalTitle
//!       true,                                                       // canCloseWithEscape
//!       true,                                                       // pause
//!       DialogAction.CLOSE, List.of(), List.of()),
//!    Optional.of(DEFAULT_BACK_BUTTON),                              // gui.back, width 200
//!    1,                                                             // columns
//!    310));                                                         // buttonWidth
//! ```
//!
//! So: title `menu.server_links.title`, Esc closes it, **one** column of
//! **310**-wide buttons, and a 200-wide `gui.back` in the footer. The 310 is
//! what forced `blitNineSlicedSprite` into
//! [`rewo_gpu::screen`](../../rewo_gpu/screen/index.html) — M82 shipped a 1:1
//! blit and asserted that any other size was skipped, and this is the milestone
//! that hit the assertion.
//!
//! # The layout
//!
//! `DialogScreen.init` builds a `HeaderAndFooterLayout` (33-px bands):
//!
//! ```java
//! LinearLayout body = LinearLayout.vertical().spacing(10);
//! body.defaultCellSetting().alignHorizontallyCenter();
//! this.layout.addToHeader(this.createTitleWithWarningButton());
//! …
//! this.populateBodyElements(body, …);                 // ButtonListDialogScreen: the link buttons
//! this.bodyScroll = new ScrollableLayout(minecraft, body, this.layout.getContentHeight());
//! this.layout.addToContents(this.bodyScroll);
//! this.updateHeaderAndFooter(…);                      // the exit action, or footerHeight = 5
//! ```
//!
//! and `ButtonListDialogScreen.populateBodyElements` adds
//! `packControlsIntoColumns(buttons, 1)` — a `GridLayout` with `columnSpacing(2)
//! .rowSpacing(2)` and `alignHorizontallyCenter`, filled row by row. With one
//! column, `count / columns == count`, so `countInFullRows == count` and the
//! `LinearLayout` last-row branch never runs: every button is its own grid row,
//! 2 px apart.
//!
//! Three things that are easy to get wrong:
//!
//! * **The header holds a `LinearLayout.horizontal().spacing(10)` of
//!   `[title, warningButton]`, not the title alone.** The warning button is
//!   20×20, so the title is displaced left by `(20 + 10) / 2 = 15` from the
//!   centre. Omitting the button centres the title and moves it 15 px right.
//!   Rewo reserves the button's geometry and draws nothing in it — see
//!   [`crate::screen::Widget::reserved`], and the reason is the same one M82
//!   gave for the death screen's `ConfirmScreen`: the button opens
//!   `DialogScreen.WarningScreen`, a nested `ConfirmScreen` with a
//!   `BooleanConsumer`, and half of that is worse than none.
//! * **The footer's exit action is present**, so `setFooterHeight(5)` does
//!   *not* run and the footer band stays 33.
//! * **`ScrollableLayout` is not transcribed.** Vanilla wraps the body in one
//!   with `maxHeight = getContentHeight()`; Rewo places the body directly. With
//!   ten links the body is `10*20 + 9*2 = 218` px against a content band of
//!   `height - 66`, so it fits at any GUI height from 284 up — and when it does
//!   *not* fit, `HeaderAndFooterLayout`'s `min` has no lower bound and the body
//!   overflows upward instead of scrolling. Named rather than approximated: a
//!   scroll container is a widget shape (a scrollbar, a wheel handler, a
//!   scissor rect) and none of the three exists yet.
//!
//! # Ground truth (bundled 26.2 decompile)
//!
//! - `net/minecraft/client/gui/screens/dialog/{DialogScreen,
//!   ButtonListDialogScreen,ServerLinksDialogScreen}.java`
//! - `net/minecraft/server/dialog/{Dialogs,ServerLinksDialog,CommonDialogData}.java`
//! - `net/minecraft/client/gui/layouts/HeaderAndFooterLayout.java`

use crate::layout::{Element, HeaderAndFooter, Linear, Settings};
use crate::screen::{Screen, ScreenKind, Widget, WidgetId, BUTTON_HEIGHT, LINE_HEIGHT};

/// `Dialogs.SERVER_LINKS`'s `buttonWidth`.
pub const LINK_BUTTON_WIDTH: i32 = 310;
/// `Dialogs.SERVER_LINKS`'s `columns`.
pub const COLUMNS: i32 = 1;
/// `DEFAULT_BACK_BUTTON`'s width — `new CommonButtonData(CommonComponents.GUI_BACK, 200)`.
pub const BACK_BUTTON_WIDTH: i32 = 200;
/// `packControlsIntoColumns`' `columnSpacing(2).rowSpacing(2)`.
pub const BUTTON_SPACING: i32 = 2;
/// `DialogScreen.WARNING_BUTTON_SIZE`.
pub const WARNING_BUTTON_SIZE: i32 = 20;
/// `createTitleWithWarningButton`'s `LinearLayout.horizontal().spacing(10)`.
pub const HEADER_SPACING: i32 = 10;

/// `CommonDialogData.title` for the built-in dialog.
pub const KEY_TITLE: &str = "menu.server_links.title";
/// `CommonComponents.GUI_BACK`.
pub const KEY_BACK: &str = "gui.back";

/// The footer's exit action.
pub const BACK: WidgetId = 0;
/// The reserved warning `ImageButton` in the header.
pub const WARNING: WidgetId = 1;
/// The title `StringWidget`.
pub const TITLE: WidgetId = 2;
/// The first link button. Link *n* is `LINK_BASE + n`.
pub const LINK_BASE: WidgetId = 16;

/// Which link a widget id names, if it is one.
pub fn link_index(id: WidgetId) -> Option<usize> {
    (id >= LINK_BASE).then(|| (id - LINK_BASE) as usize)
}

/// The strings this screen needs, resolved by the caller.
///
/// `links` are the buttons' labels **in wire order** — `createListActions` is
/// `connectionAccess.serverLinks().entries().stream().map(…)`, so the order is
/// the server's and nothing sorts it. Each is
/// `ServerLinks.Entry.displayName()`, which is
/// `type.map(KnownLinkType::displayName, r -> r)`: the language map's
/// `known_server_link.<name>` for a known type, and the server's own component
/// for a custom one.
#[derive(Clone, Debug, PartialEq)]
pub struct ServerLinksLabels {
    pub title: String,
    pub back: String,
    pub links: Vec<String>,
}

/// `DialogScreen.init` for `Dialogs.SERVER_LINKS`.
///
/// `title_width` is `font.width(title)`.
pub fn build(labels: &ServerLinksLabels, title_width: i32, gui_w: i32, gui_h: i32) -> Screen {
    let mut hf = HeaderAndFooter::new(gui_w, gui_h);

    // Header: `LinearLayout.horizontal().spacing(10)`, centred and middled,
    // holding the title and the warning button.
    let mut header = Linear::horizontal()
        .spacing(HEADER_SPACING)
        .default_cell(Settings::defaults().align_h_center().align_v_middle());
    header.add1(Element::leaf(TITLE, title_width, LINE_HEIGHT));
    header.add1(Element::leaf(
        WARNING,
        WARNING_BUTTON_SIZE,
        WARNING_BUTTON_SIZE,
    ));
    hf.header.add(header.into_element());

    // Contents: `LinearLayout.vertical().spacing(10)` holding one
    // `packControlsIntoColumns(buttons, 1)` grid. With one column that grid is
    // a single column of rows 2 px apart.
    let mut body = Linear::vertical()
        .spacing(10)
        .default_cell(Settings::defaults().align_h_center());
    let mut column = Linear::vertical()
        .spacing(BUTTON_SPACING)
        .default_cell(Settings::defaults().align_h_center());
    for (i, _) in labels.links.iter().enumerate() {
        column.add1(Element::leaf(
            LINK_BASE + i as WidgetId,
            LINK_BUTTON_WIDTH,
            BUTTON_HEIGHT,
        ));
    }
    body.add1(column.into_element());
    hf.contents.add(body.into_element());

    // Footer: the exit action. Present, so `setFooterHeight(5)` does not run.
    hf.footer
        .add(Element::leaf(BACK, BACK_BUTTON_WIDTH, BUTTON_HEIGHT));

    hf.arrange();

    let widgets = hf
        .leaves()
        .into_iter()
        .map(|(key, x, y, w, h)| match key {
            TITLE => Widget::label(key, x, y, w, labels.title.clone()),
            WARNING => Widget::reserved(key, x, y, w, h),
            BACK => Widget::button(key, x, y, w, h, labels.back.clone()),
            _ => {
                let i = link_index(key).unwrap_or(0);
                let text = labels.links.get(i).cloned().unwrap_or_default();
                Widget::button(key, x, y, w, h, text)
            }
        })
        .collect();

    Screen::new(ScreenKind::ServerLinks, gui_w, gui_h)
        .with_widgets(widgets)
        // `DialogScreen.shouldCloseOnEsc()` is `common().canCloseWithEscape()`,
        // and the built-in dialog sets it true. `isPauseScreen()` is
        // `common().pause()`, also true.
        .with_close_on_esc(true)
        .with_pause(true)
        .with_menu_background(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screen::{KeyResult, MouseResult, WidgetKind};

    fn labels(n: usize) -> ServerLinksLabels {
        ServerLinksLabels {
            title: "Server Links".into(),
            back: "Back".into(),
            links: (0..n).map(|i| format!("Link {i}")).collect(),
        }
    }

    fn built(n: usize) -> Screen {
        build(&labels(n), 66, 320, 240)
    }

    /// One 310-wide button per link, stacked 2 px apart, in wire order.
    ///
    /// MUTATION: `packControlsIntoColumns`' last-row `LinearLayout` branch
    /// taken for a single column (`count != countInFullRows`). With
    /// `columns == 1` that branch is unreachable — `count / 1 * 1 == count` —
    /// and taking it would put every button on one horizontal row.
    #[test]
    fn each_link_is_a_three_hundred_and_ten_wide_button_two_pixels_below_the_last() {
        let s = built(3);
        let b = |i: u32| s.widget(LINK_BASE + i).unwrap();
        for i in 0..3 {
            assert_eq!(b(i).width, LINK_BUTTON_WIDTH);
            assert_eq!(b(i).height, BUTTON_HEIGHT);
            assert_eq!(b(i).message, format!("Link {i}"));
        }
        assert_eq!(b(1).y - b(0).y, BUTTON_HEIGHT + BUTTON_SPACING);
        assert_eq!(b(2).y - b(1).y, BUTTON_HEIGHT + BUTTON_SPACING);
        assert_eq!(b(0).x, b(1).x, "one column, so every x agrees");
        // Centred in the 320-wide screen.
        assert_eq!(b(0).x, (320 - LINK_BUTTON_WIDTH) / 2);
    }

    /// The header's warning button displaces the title left of centre.
    ///
    /// MUTATION: dropping the warning button from the header layout. The title
    /// then centres, 15 px to the right of where vanilla puts it — a shift no
    /// witness that only looks at the *buttons* would ever see.
    #[test]
    fn the_reserved_warning_button_pushes_the_title_left_of_centre() {
        let s = built(2);
        let t = s.widget(TITLE).unwrap();
        let w = s.widget(WARNING).unwrap();
        assert_eq!(w.kind, WidgetKind::Reserved);
        assert_eq!((w.width, w.height), (20, 20));
        // title | 10 | warning, the group centred: group width 66 + 10 + 20 = 96.
        let group_left = (320 - 96) / 2;
        assert_eq!(t.x, group_left);
        assert_eq!(w.x, group_left + 66 + HEADER_SPACING);
        assert!(
            t.x + t.width / 2 < 160,
            "the title sits left of the screen centre"
        );
        // Both inside the 33-px header band.
        assert!(t.y >= 0 && t.y + t.height <= 33);
        assert!(w.y >= 0 && w.y + w.height <= 33);
    }

    /// The Back button is 200 wide and sits in the 33-px footer band.
    ///
    /// MUTATION: `setFooterHeight(5)` (the `ifPresentOrElse` else branch). The
    /// dialog *has* an exit action, so the band stays 33; taking the else would
    /// move the button 14 px down and let the body grow into the space.
    #[test]
    fn the_back_button_sits_in_a_thirty_three_pixel_footer() {
        let s = built(2);
        let b = s.widget(BACK).unwrap();
        assert_eq!(b.width, BACK_BUTTON_WIDTH);
        assert_eq!(b.message, "Back");
        let footer_top = 240 - 33;
        assert!(b.y >= footer_top, "{} >= {footer_top}", b.y);
        // The vertical centring rounds: a 13-px leftover gives 7, not 6.
        assert_eq!(b.y, footer_top + 7);
        assert_eq!(b.x, (320 - BACK_BUTTON_WIDTH) / 2);
    }

    /// The body sits 30 px below the header when it fits, and Esc closes.
    #[test]
    fn the_body_starts_thirty_pixels_below_the_header_band() {
        let mut s = built(2);
        let first = s.widget(LINK_BASE).unwrap();
        // header 33 + CONTENT_MARGIN_TOP 30, and the body's own height is
        // 2*20 + 2 = 42, so the `min` picks the preferred 63.
        assert_eq!(first.y, 63);
        assert!(s.close_on_esc);
        assert!(s.pause);
        assert_eq!(s.key_pressed(256, false), KeyResult::Close);
    }

    /// A zero-link screen is still a well-formed screen — and it is not one the
    /// pause menu can reach, because the button that opens it is gated on
    /// `!serverLinks.isEmpty()`.
    #[test]
    fn an_empty_link_list_still_builds_a_header_and_a_back_button() {
        let s = built(0);
        assert!(s.widget(LINK_BASE).is_none());
        assert!(s.widget(BACK).is_some());
        assert!(s.widget(TITLE).is_some());
        assert_eq!(link_index(BACK), None);
        assert_eq!(link_index(LINK_BASE + 5), Some(5));
    }

    /// A click on a link presses that link; a click on the reserved warning
    /// button falls through.
    #[test]
    fn a_link_is_pressable_and_the_warning_button_is_not() {
        let mut s = built(3);
        let b = s.widget(LINK_BASE + 1).unwrap().clone();
        assert_eq!(
            s.mouse_clicked(b.x as f64 + 5.0, b.y as f64 + 5.0, 0),
            MouseResult::Pressed(LINK_BASE + 1)
        );
        let w = s.widget(WARNING).unwrap().clone();
        assert_eq!(
            s.mouse_clicked(w.x as f64 + 5.0, w.y as f64 + 5.0, 0),
            MouseResult::Ignored
        );
    }
}
