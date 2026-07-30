//! `PauseScreen` (M85) — Esc, with a session behind it.
//!
//! The screen `server_links` (137) actually reaches the player through. Not as
//! a list: `PauseScreen.getCustomAdditions()` ends
//!
//! ```java
//! ServerLinks serverLinks = this.minecraft.player.connection.serverLinks();
//! return !serverLinks.isEmpty() ? dialogRegistry.get(Dialogs.SERVER_LINKS) : Optional.empty();
//! ```
//!
//! and `addCustomDialogButtons` turns the result into **one** 204-wide button
//! labelled `dialog.value().common().computeExternalTitle()` — for the built-in
//! `Dialogs.SERVER_LINKS` that is `Component.translatable("menu.server_links")`,
//! *"Server Links..."*. Pressing it opens
//! [`crate::server_links_screen`]. So the packet's whole effect on the pause
//! menu is one button appearing, and the row it appears in pushes everything
//! below it down.
//!
//! # The layout is a `GridLayout`, not arithmetic
//!
//! ```java
//! GridLayout gridLayout = new GridLayout();
//! gridLayout.defaultCellSetting().padding(4, 4, 4, 0);
//! GridLayout.RowHelper helper = gridLayout.createRowHelper(2);
//! helper.addChild(Button(RETURN_TO_GAME).width(204), 2, gridLayout.newCellSettings().paddingTop(50));
//! helper.addChild(openScreenButton(ADVANCEMENTS));   // width 98
//! helper.addChild(openScreenButton(STATS));          // width 98
//! …iconButtonRow (LinearLayout.horizontal().spacing(4), four width-20 buttons)…
//! helper.addChild(iconButtonRow, 2, gridLayout.newCellSettings().alignHorizontallyCenter());
//! additions.ifPresent(d -> helper.addChild(Button(externalTitle).width(204), 2));
//! …options…
//! helper.addChild(Button(disconnectLabel).width(204), 2);
//! gridLayout.arrangeElements();
//! FrameLayout.alignInRectangle(gridLayout, 0, 0, this.width, this.height, 0.5F, 0.25F);
//! ```
//!
//! Four details that are not guessable from a screenshot:
//!
//! * **`padding(4, 4, 4, 0)` is the four-argument overload** — left 4, top 4,
//!   right 4, **bottom 0**. The three-argument form does not exist and the
//!   two-argument one is `(horizontal, vertical)`, so reading it as symmetric
//!   padding adds 4 px to every row gap.
//! * **`newCellSettings()` is a *copy* of the default**, so the first row's
//!   `paddingTop(50)` is `padding(4, 50, 4, 0)` and not a bare top padding, and
//!   the icon row's `alignHorizontallyCenter()` keeps the 4/4/4/0 padding.
//! * **`alignInRectangle(…, 0.5F, 0.25F)`** — horizontally centred, but a
//!   **quarter** of the way down, not centred. And `alignInDimension`
//!   truncates.
//! * **`hasSingleplayerServer()` is false for Rewo**, always, so the Options
//!   row is the single 204-wide button spanning both columns rather than the
//!   two 98-wide ones. That is not a simplification — it is the branch a
//!   multiplayer client takes.
//!
//! # The icon row is reserved, not drawn
//!
//! `iconButtonRow`'s four `SpriteIconButton`s are Report Bugs, Send Feedback,
//! Friends and Player Reporting. The first two are
//! `ConfirmLinkScreen.confirmLink` — a browser, which
//! [`rewo_net::server_links`] records this project as not doing; the third is
//! Realms authentication; the fourth is the chat-report subsystem. All four are
//! things Rewo cannot do, so the row is a [`crate::screen::Widget::reserved`]:
//! its 92×20 rectangle occupies the cell so every row beneath it — including
//! the server-links button and Disconnect — lands where vanilla puts it, and
//! nothing is drawn inside it. Four dead buttons would be worse; omitting the
//! row would move five widgets.
//!
//! # What the buttons do
//!
//! `Return to Game` closes the screen and re-grabs the mouse, `Server Links...`
//! opens the dialog, and `Disconnect` leaves the session — those three are the
//! whole of what this screen means in Rewo. `Advancements`, `Statistics` and
//! `Options` are rendered as vanilla renders them (enabled, correct labels) and
//! log on press: `Statistics` is a sibling milestone's (`award_stats`), and the
//! other two are screens Rewo has not got. Rendering them *disabled* was the
//! alternative and it is a worse lie — the grey sprite and grey label are a
//! specific claim ("the server won't let you"), where an enabled button that
//! does nothing is a gap.
//!
//! # Ground truth (bundled 26.2 decompile)
//!
//! - `net/minecraft/client/gui/screens/PauseScreen.java`
//! - `net/minecraft/server/dialog/Dialogs.java` — the `SERVER_LINKS` registration
//! - `net/minecraft/server/dialog/CommonDialogData.java` — `computeExternalTitle`
//! - `net/minecraft/network/chat/CommonComponents.java` — `disconnectButtonLabel`

use crate::layout::{align_in_rectangle, Element, Grid, Linear, RowHelper, Settings};
use crate::screen::{Screen, ScreenKind, Widget, WidgetId, BUTTON_HEIGHT};

/// `menu.returnToGame` — closes the screen.
pub const RETURN_TO_GAME: WidgetId = 0;
/// `gui.advancements`.
pub const ADVANCEMENTS: WidgetId = 1;
/// `gui.stats` — the sibling milestone's screen.
pub const STATS: WidgetId = 2;
/// The four-icon row. Reserved geometry; see the module docs.
pub const ICON_ROW: WidgetId = 3;
/// The custom-dialog button — for Rewo, always the server-links one. Present
/// only when the server advertised at least one link.
pub const SERVER_LINKS: WidgetId = 4;
/// `menu.options`.
pub const OPTIONS: WidgetId = 5;
/// `menu.disconnect`.
pub const DISCONNECT: WidgetId = 6;
/// The `menu.game` title, placed outside the grid.
pub const TITLE: WidgetId = 7;

/// `BUTTON_WIDTH_FULL`.
pub const BUTTON_WIDTH_FULL: i32 = 204;
/// `BUTTON_WIDTH_HALF`.
pub const BUTTON_WIDTH_HALF: i32 = 98;
/// `MENU_PADDING_TOP`.
pub const MENU_PADDING_TOP: i32 = 50;
/// `BUTTON_PADDING`.
pub const BUTTON_PADDING: i32 = 4;
/// `COLUMNS`.
pub const COLUMNS: i32 = 2;
/// The four `SpriteIconButton`s are `width(20)` and `Button.DEFAULT_HEIGHT`
/// tall, in a `LinearLayout.horizontal().spacing(4)`.
pub const ICON_BUTTON_SIZE: i32 = 20;
pub const ICON_BUTTON_COUNT: i32 = 4;

/// `menu.game` — the title when `showPauseMenu` is true. (`menu.paused` is the
/// *other* branch, the one used while a level is still loading, which Rewo
/// never reaches.)
pub const KEY_TITLE: &str = "menu.game";
pub const KEY_RETURN_TO_GAME: &str = "menu.returnToGame";
pub const KEY_ADVANCEMENTS: &str = "gui.advancements";
pub const KEY_STATS: &str = "gui.stats";
pub const KEY_OPTIONS: &str = "menu.options";
/// `CommonComponents.disconnectButtonLabel(isLocalServer)` — Rewo is never a
/// local server, so it is always this one and never `menu.returnToMenu`.
pub const KEY_DISCONNECT: &str = "menu.disconnect";
/// `Dialogs.SERVER_LINKS`'s `externalTitle`, which
/// `CommonDialogData.computeExternalTitle` prefers over the `title`.
pub const KEY_SERVER_LINKS_BUTTON: &str = "menu.server_links";

/// The strings the screen needs, resolved through the language map by the
/// caller (the same split [`crate::death_screen::DeathLabels`] uses, and for
/// the same reason: this module stays free of the asset pipeline).
#[derive(Clone, Debug, PartialEq)]
pub struct PauseLabels {
    pub title: String,
    pub return_to_game: String,
    pub advancements: String,
    pub stats: String,
    pub options: String,
    pub disconnect: String,
    pub server_links: String,
}

impl PauseLabels {
    pub fn resolve(lang: &rewo_data::lang::Language) -> Self {
        Self {
            title: lang.or_key(KEY_TITLE).to_string(),
            return_to_game: lang.or_key(KEY_RETURN_TO_GAME).to_string(),
            advancements: lang.or_key(KEY_ADVANCEMENTS).to_string(),
            stats: lang.or_key(KEY_STATS).to_string(),
            options: lang.or_key(KEY_OPTIONS).to_string(),
            disconnect: lang.or_key(KEY_DISCONNECT).to_string(),
            server_links: lang.or_key(KEY_SERVER_LINKS_BUTTON).to_string(),
        }
    }
}

/// `PauseScreen.init` + `createPauseMenu`.
///
/// `has_server_links` is `!connection.serverLinks().isEmpty()`; `title_width`
/// is `font.width(this.title)`.
pub fn build(
    labels: &PauseLabels,
    has_server_links: bool,
    title_width: i32,
    gui_w: i32,
    gui_h: i32,
) -> Screen {
    // `gridLayout.defaultCellSetting().padding(4, 4, 4, 0)` — the FOUR-argument
    // overload. Bottom is 0.
    let default_cell = Settings::defaults().padding4(
        BUTTON_PADDING,
        BUTTON_PADDING,
        BUTTON_PADDING,
        0,
    );
    let grid = Grid::new().default_cell(default_cell);
    let mut helper = RowHelper::new(grid, COLUMNS);

    // Row 0: Return to Game, spanning both columns, with the 50-px top padding
    // that is the whole gap between the title and the menu.
    helper.add(
        Element::leaf(RETURN_TO_GAME, BUTTON_WIDTH_FULL, BUTTON_HEIGHT),
        2,
        default_cell.padding_top(MENU_PADDING_TOP),
    );
    // Row 1: the two half-width buttons.
    helper.add1(Element::leaf(ADVANCEMENTS, BUTTON_WIDTH_HALF, BUTTON_HEIGHT));
    helper.add1(Element::leaf(STATS, BUTTON_WIDTH_HALF, BUTTON_HEIGHT));

    // Row 2: the icon row — a horizontal LinearLayout of four 20x20 buttons
    // with 4 px of spacing, centred across both columns. Reserved, not drawn.
    let mut icons = Linear::horizontal().spacing(BUTTON_PADDING);
    icons.add1(Element::leaf(
        ICON_ROW,
        ICON_BUTTON_COUNT * ICON_BUTTON_SIZE + (ICON_BUTTON_COUNT - 1) * BUTTON_PADDING,
        ICON_BUTTON_SIZE,
    ));
    helper.add(icons.into_element(), 2, default_cell.align_h_center());

    // Row 3 (conditional): the custom-dialog button. **This is the packet.**
    if has_server_links {
        helper.add(
            Element::leaf(SERVER_LINKS, BUTTON_WIDTH_FULL, BUTTON_HEIGHT),
            2,
            default_cell,
        );
    }
    // `minecraft.hasSingleplayerServer()` is false for a remote client, so the
    // else branch: one 204-wide Options spanning both columns.
    helper.add(
        Element::leaf(OPTIONS, BUTTON_WIDTH_FULL, BUTTON_HEIGHT),
        2,
        default_cell,
    );
    helper.add(
        Element::leaf(DISCONNECT, BUTTON_WIDTH_FULL, BUTTON_HEIGHT),
        2,
        default_cell,
    );

    let mut root = Element::Grid(helper.grid);
    root.arrange();
    // `FrameLayout.alignInRectangle(gridLayout, 0, 0, width, height, 0.5F, 0.25F)`
    // — centred horizontally, a **quarter** of the way down vertically.
    align_in_rectangle(&mut root, 0, 0, gui_w, gui_h, 0.5, 0.25);

    let mut placed = Vec::new();
    root.leaves(&mut placed);
    let mut widgets: Vec<Widget> = placed
        .into_iter()
        .map(|(key, x, y, w, h)| match key {
            RETURN_TO_GAME => Widget::button(key, x, y, w, h, labels.return_to_game.clone()),
            ADVANCEMENTS => Widget::button(key, x, y, w, h, labels.advancements.clone()),
            STATS => Widget::button(key, x, y, w, h, labels.stats.clone()),
            SERVER_LINKS => Widget::button(key, x, y, w, h, labels.server_links.clone()),
            OPTIONS => Widget::button(key, x, y, w, h, labels.options.clone()),
            DISCONNECT => Widget::button(key, x, y, w, h, labels.disconnect.clone()),
            _ => Widget::reserved(key, x, y, w, h),
        })
        .collect();

    // `init()` adds the title **after** `createPauseMenu`, outside the grid:
    // `new StringWidget(this.width / 2 - textWidth / 2, 40, textWidth, 9, …)`.
    widgets.push(Widget::label(
        TITLE,
        gui_w / 2 - title_width / 2,
        TITLE_Y,
        title_width,
        labels.title.clone(),
    ));

    Screen::new(ScreenKind::Pause, gui_w, gui_h)
        .with_widgets(widgets)
        // `PauseScreen.extractBackground` with `showPauseMenu` →
        // `extractBlurredBackground` + `extractMenuBackground`, i.e. the
        // **tiled** texture and not the gradient the inventory and death screen
        // use. `in_world` is true: a pause screen has a level behind it by
        // definition, so it takes `INWORLD_MENU_BACKGROUND`.
        .with_menu_background(true)
    // `Screen.shouldCloseOnEsc()` and `isPauseScreen()` are both the defaults
    // (`true`), which is why `Screen::new` needs no override here — Esc closes
    // the pause screen, which is how you get back to the game.
}

/// The title's `y`: `this.showPauseMenu ? 40 : 10`.
pub const TITLE_Y: i32 = 40;

/// The menu background's tile size in GUI pixels.
///
/// `extractMenuBackgroundTexture` is
/// `graphics.blit(pipeline, texture, x, y, 0, 0, width, height, 32, 32)` — the
/// trailing `32, 32` is the *declared* texture size the UVs normalise against,
/// and the file is **16×16**. So the texture coordinate reaches 1.0 every 32
/// screen pixels and the sheet is drawn at 2× magnification, repeating. Reading
/// the 32 as the texture's size (and drawing one 16×16 tile per 16 px) halves
/// the pattern.
pub const MENU_BACKGROUND_TILE: i32 = 32;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screen::{KeyResult, MouseResult, WidgetKind};

    fn labels() -> PauseLabels {
        PauseLabels {
            title: "Game Menu".into(),
            return_to_game: "Back to Game".into(),
            advancements: "Advancements".into(),
            stats: "Statistics".into(),
            options: "Options...".into(),
            disconnect: "Disconnect".into(),
            server_links: "Server Links...".into(),
        }
    }

    fn built(links: bool) -> Screen {
        build(&labels(), links, 54, 320, 240)
    }

    /// The full-width column is 212 (204 + 4 + 4) and the grid is centred at
    /// `alignInRectangle(0.5)`; the rows stack at 24 px (20 + 4 top padding)
    /// with the first pushed down 50.
    ///
    /// MUTATION: reading `padding(4, 4, 4, 0)` as symmetric 4-px padding. Every
    /// row below the first then gains 4 px, so `Disconnect` moves 20 px down
    /// while `Back to Game` does not — a shift no single-widget assertion sees.
    #[test]
    fn the_rows_stack_at_twenty_four_pixels_and_the_first_is_pushed_down_fifty() {
        let s = built(false);
        let y = |id| s.widget(id).unwrap().y;
        assert_eq!(y(ADVANCEMENTS) - y(RETURN_TO_GAME), 24);
        assert_eq!(y(ICON_ROW) - y(ADVANCEMENTS), 24);
        assert_eq!(y(OPTIONS) - y(ICON_ROW), 24);
        assert_eq!(y(DISCONNECT) - y(OPTIONS), 24);
        // The grid's height: 50 + 20 (row 0) then four rows of 4 + 20.
        // alignInRectangle(0.25) then places it.
        let grid_h = (MENU_PADDING_TOP + BUTTON_HEIGHT) + 4 * (BUTTON_PADDING + BUTTON_HEIGHT);
        let top = (0.25 * (240 - grid_h) as f32) as i32;
        assert_eq!(y(RETURN_TO_GAME), top + MENU_PADDING_TOP);
    }

    /// The two half-width buttons sit against the halves of the 212-wide
    /// spanning column, which is the `Divisor` arithmetic.
    ///
    /// MUTATION: `childWidth / occupiedColumns` for both halves. 212 is even so
    /// the halves agree here — what separates them is the *column offset*: the
    /// second column starts at `col_widths[0]`, which is 106 either way. The
    /// assertion that bites is the pair's symmetry about the grid centre, which
    /// a wrong column width breaks.
    #[test]
    fn the_two_half_buttons_straddle_the_grid_centre() {
        let s = built(false);
        let full = s.widget(RETURN_TO_GAME).unwrap();
        let a = s.widget(ADVANCEMENTS).unwrap();
        let b = s.widget(STATS).unwrap();
        assert_eq!(a.x, full.x);
        assert_eq!(b.right(), full.right());
        // 204 = 98 + gap + 98 → the gap between them is 8 (4 right + 4 left).
        assert_eq!(b.x - a.right(), 8);
        assert_eq!(a.width, BUTTON_WIDTH_HALF);
        assert_eq!(full.width, BUTTON_WIDTH_FULL);
    }

    /// The whole point: the server-links button exists only when the server
    /// sent links, and it pushes Options and Disconnect down by one row.
    ///
    /// MUTATION: always adding the button, or adding it after Options. The
    /// first makes the pause menu claim links on a server that sent none; the
    /// second puts it below the options row, which is where a reader who
    /// skimmed `createPauseMenu` would put it.
    #[test]
    fn the_server_links_button_appears_only_with_links_and_pushes_the_rows_below() {
        let without = built(false);
        assert!(without.widget(SERVER_LINKS).is_none());
        let with = built(true);
        let b = with.widget(SERVER_LINKS).unwrap();
        assert_eq!(b.message, "Server Links...");
        assert_eq!(b.width, BUTTON_WIDTH_FULL);
        // It is between the icon row and Options.
        assert!(b.y > with.widget(ICON_ROW).unwrap().y);
        assert!(b.y < with.widget(OPTIONS).unwrap().y);
        assert_eq!(with.widget(OPTIONS).unwrap().y - b.y, 24);
        // And the extra row makes the grid taller, so `alignInRectangle(0.25)`
        // moves the whole menu **up**.
        assert!(with.widget(RETURN_TO_GAME).unwrap().y < without.widget(RETURN_TO_GAME).unwrap().y);
    }

    /// The icon row is reserved: right size, right place, click-through.
    ///
    /// MUTATION: dropping the row entirely. Every widget below it moves up 24
    /// px, and nothing about a rendered frame says which of the two layouts is
    /// vanilla's — which is why the row is asserted by *geometry* rather than
    /// by being drawn.
    #[test]
    fn the_icon_row_reserves_its_cell_and_swallows_nothing() {
        let mut s = built(false);
        let row = s.widget(ICON_ROW).unwrap().clone();
        assert_eq!(row.kind, WidgetKind::Reserved);
        // 4 * 20 + 3 * 4
        assert_eq!((row.width, row.height), (92, 20));
        assert!(!row.active, "reserved geometry is not interactive");
        // Centred across both columns.
        let full = s.widget(RETURN_TO_GAME).unwrap();
        assert_eq!(
            row.x + row.width / 2,
            full.x + full.width / 2,
            "alignHorizontallyCenter across the spanning cell"
        );
        // A click inside it falls through rather than being consumed.
        assert_eq!(
            s.mouse_clicked(row.x as f64 + 2.0, row.y as f64 + 2.0, 0),
            MouseResult::Ignored
        );
    }

    /// The title sits at y = 40, centred, and is a label rather than a button.
    #[test]
    fn the_title_is_a_centred_label_at_forty() {
        let s = built(false);
        let t = s.widget(TITLE).unwrap();
        assert_eq!(t.y, TITLE_Y);
        assert_eq!(t.x, 320 / 2 - 54 / 2);
        assert!(matches!(t.kind, WidgetKind::Label { .. }));
        assert!(!t.active, "StringWidget's constructor sets active = false");
        assert_eq!(t.message, "Game Menu");
    }

    /// Esc closes the pause screen — the default `shouldCloseOnEsc()`, and the
    /// opposite of the death screen's override.
    ///
    /// MUTATION: copying `DeathScreen`'s `with_close_on_esc(false)`. The pause
    /// screen would then be inescapable except through its own buttons, which
    /// is a very close relative of a hung client.
    #[test]
    fn esc_closes_the_pause_screen() {
        let mut s = built(true);
        assert!(s.close_on_esc);
        assert!(s.pause);
        assert_eq!(s.key_pressed(256, false), KeyResult::Close);
    }

    /// Tab reaches every button and skips the reserved row and the title.
    #[test]
    fn tab_reaches_the_buttons_and_skips_the_reserved_widgets() {
        let mut s = built(true);
        let mut seen = Vec::new();
        for _ in 0..6 {
            s.key_pressed(258, false);
            seen.push(s.focused().unwrap());
        }
        assert_eq!(
            seen,
            vec![
                RETURN_TO_GAME,
                ADVANCEMENTS,
                STATS,
                SERVER_LINKS,
                OPTIONS,
                DISCONNECT
            ]
        );
    }
}
