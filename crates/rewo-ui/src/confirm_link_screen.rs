//! `ConfirmLinkScreen` — the "are you sure you want to open this website?"
//! prompt that stands between a clicked chat link and the desktop browser.
//!
//! Until this screen existed Rewo opened a clicked `open_url` straight away,
//! which `uri_open.rs` recorded as a divergence "in the user's favour only if
//! you think a confirmation is noise". It is not noise: the URL is a string a
//! remote server chose, and the prompt is the one place the player sees it.
//!
//! # Where the prompt comes from
//!
//! ```java
//! // Screen.clickUrlAction
//! if (!minecraft.options.chatLinks().get()) { return false; }
//! if (minecraft.options.chatLinksPrompt().get()) {
//!    minecraft.gui.setScreen(new ConfirmLinkScreen(result -> {
//!       if (result) { Util.getPlatform().openUri(uri); }
//!       minecraft.gui.setScreen(screen);
//!    }, uri.toString(), false));
//! } else { Util.getPlatform().openUri(uri); }
//! ```
//!
//! Both options default **on** (`createBoolean("options.chat.links", true)`
//! and `"options.chat.links.prompt", true`), so the stock behaviour is the
//! prompt. [`click_url_action`] is that decision as a pure function; the app
//! reads the two booleans from `options.txt` (`chatLinks`, `chatLinksPrompt`).
//!
//! **`trusted` is `false` on this path.** That is the detail the module's
//! labels turn on: an untrusted link shows `chat.link.confirm` as the title,
//! the URL as the message, a red `chat.link.warning` line, and the buttons
//! "Yes" / "Copy to Clipboard" / "No". The trusted variant ("Open in Browser"
//! / "Cancel", `chat.link.confirmTrusted`, no warning) is what the pause
//! screen's bug-report button and the disconnect screen use; it is built here
//! too because it is the same class, but no Rewo path asks for it yet.
//!
//! # The layout
//!
//! ```java
//! protected final LinearLayout layout = LinearLayout.vertical().spacing(8);
//! init():
//!   layout.defaultCellSetting().alignHorizontallyCenter();
//!   layout.addChild(new StringWidget(this.title, this.font));
//!   layout.addChild(new MultiLineTextWidget(message, font)
//!        .setMaxWidth(this.width - 50).setMaxRows(15).setCentered(true));
//!   addAdditionalText();                       // the warning, when untrusted
//!   LinearLayout buttons = layout.addChild(LinearLayout.horizontal().spacing(4));
//!   buttons.defaultCellSetting().paddingTop(16);
//!   addButtons(buttons);                       // three, each width(100)
//! repositionElements(): layout.arrangeElements(); FrameLayout.centerInRectangle(..)
//! ```
//!
//! * The vertical `spacing(8)` is **row** spacing; the horizontal `spacing(4)`
//!   is **column** spacing, and `paddingTop(16)` is on the button cells, not on
//!   the row — so the 16 px lives inside the row's height.
//! * The message is capped at **15 rows**; when it is longer the last kept row
//!   is clipped and ends in `...` ([`wrap_message`]). A URL is one long word, so
//!   this is reachable: a 2,000-character link wraps to far more than 15 rows.
//! * `shouldCloseOnEsc()` is **false**, and `keyPressed` instead turns Esc into
//!   `callback.accept(false)` ([`key_pressed`]). The visible effect is the
//!   same, but the *route* differs: it is the "No" answer, which for a link
//!   prompt goes back to the screen underneath rather than to the world.
//!
//! # Ground truth (bundled 26.2 decompile)
//!
//! - `net/minecraft/client/gui/screens/ConfirmLinkScreen.java`
//! - `net/minecraft/client/gui/screens/ConfirmScreen.java`
//! - `net/minecraft/client/gui/screens/Screen.java` — `clickUrlAction`,
//!   `defaultHandleClickEvent`
//! - `net/minecraft/client/Options.java` — `chatLinks`, `chatLinksPrompt`
//! - `net/minecraft/client/gui/components/MultiLineLabel.java` — the row cap
//! - `net/minecraft/network/chat/CommonComponents.java`

use crate::layout::{center_in_rectangle, Element, Linear, Settings};
use crate::screen::{KeyResult, Screen, ScreenKind, Widget, WidgetId, BUTTON_HEIGHT, LINE_HEIGHT};

/// The title `StringWidget`.
pub const TITLE: WidgetId = 0;
/// The wrapped URL.
pub const MESSAGE: WidgetId = 1;
/// `WARNING_TEXT` — present only when the link is untrusted.
pub const WARNING: WidgetId = 2;
/// `callback.accept(true)`.
pub const YES: WidgetId = 3;
/// `copyToClipboard()` then `callback.accept(false)`.
pub const COPY: WidgetId = 4;
/// `callback.accept(false)`.
pub const NO: WidgetId = 5;

/// `LinearLayout.vertical().spacing(8)`.
pub const ROW_SPACING: i32 = 8;
/// The button row's `spacing(4)`.
pub const BUTTON_SPACING: i32 = 4;
/// The button cells' `paddingTop(16)`.
pub const BUTTON_PADDING_TOP: i32 = 16;
/// `Button.builder(..).width(100)`.
pub const BUTTON_WIDTH: i32 = 100;
/// `setMaxWidth(this.width - 50)`.
pub const MESSAGE_MARGIN: i32 = 50;
/// `setMaxRows(15)`.
pub const MAX_ROWS: usize = 15;
/// `WARNING_TEXT`'s colour: `withColor(-13108)`, i.e. `0xFFFFCCCC`.
pub const WARNING_COLOR: [f32; 3] = [1.0, 204.0 / 255.0, 204.0 / 255.0];

/// `CommonComponents.ELLIPSIS` — `Component.literal("...")`, three dots and not
/// the single `…` character.
pub const ELLIPSIS: &str = "...";

/// `chat.link.confirm` / `chat.link.confirmTrusted`.
pub const KEY_CONFIRM: &str = "chat.link.confirm";
pub const KEY_CONFIRM_TRUSTED: &str = "chat.link.confirmTrusted";
/// `chat.link.warning`.
pub const KEY_WARNING: &str = "chat.link.warning";
/// `CommonComponents.GUI_OPEN_IN_BROWSER`.
pub const KEY_OPEN: &str = "chat.link.open";
/// `CommonComponents.GUI_COPY_TO_CLIPBOARD`.
pub const KEY_COPY: &str = "chat.copy";
/// `CommonComponents.GUI_YES` / `GUI_NO` / `GUI_CANCEL`.
pub const KEY_YES: &str = "gui.yes";
pub const KEY_NO: &str = "gui.no";
pub const KEY_CANCEL: &str = "gui.cancel";

/// What `Screen.clickUrlAction` does with a clicked `open_url`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UrlClick {
    /// `chatLinks` is off: `return false` — nothing happens, and the screen
    /// underneath stays as it was.
    Refused,
    /// `chatLinksPrompt` is on: a [`ConfirmLinkScreen`] replaces the screen.
    Prompt,
    /// `chatLinksPrompt` is off: `openUri` straight away.
    OpenDirect,
}

/// `Screen.clickUrlAction`'s two option reads, in vanilla's order: `chatLinks`
/// first (it vetoes), then `chatLinksPrompt`.
pub fn click_url_action(chat_links: bool, chat_links_prompt: bool) -> UrlClick {
    if !chat_links {
        UrlClick::Refused
    } else if chat_links_prompt {
        UrlClick::Prompt
    } else {
        UrlClick::OpenDirect
    }
}

/// What the player answered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkDecision {
    /// `callback.accept(true)` — open the link.
    Open,
    /// The Copy button: `copyToClipboard()` and **then** `callback.accept(false)`,
    /// so the link is copied, not opened, and the prompt closes.
    CopyThenCancel,
    /// `callback.accept(false)` — the No/Cancel button, or Esc.
    Cancel,
}

/// The decision a pressed widget stands for.
pub fn decision_for(id: WidgetId) -> Option<LinkDecision> {
    match id {
        YES => Some(LinkDecision::Open),
        COPY => Some(LinkDecision::CopyThenCancel),
        NO => Some(LinkDecision::Cancel),
        _ => None,
    }
}

/// `ConfirmScreen.keyPressed` for a screen with no delay: Esc is
/// `callback.accept(false)`, anything else is `Screen.keyPressed`.
///
/// `shouldCloseOnEsc()` is false for this class, so the framework's own Esc
/// path (`KeyResult::Close`) is dead for it; the answer is delivered as a press
/// of [`NO`], which the app maps through [`decision_for`] like a click.
pub fn key_pressed(screen: &mut Screen, key: i32, shift: bool) -> KeyResult {
    if key == crate::screen::keys::ESCAPE {
        return KeyResult::Pressed(NO);
    }
    screen.key_pressed(key, shift)
}

/// The resolved strings for one prompt.
#[derive(Clone, Debug, PartialEq)]
pub struct ConfirmLinkLabels {
    pub title: String,
    pub warning: String,
    pub yes: String,
    pub copy: String,
    pub no: String,
}

impl ConfirmLinkLabels {
    /// `ConfirmLinkScreen`'s constructor: `trusted` picks the title key and the
    /// Yes/No captions.
    pub fn resolve(lang: &rewo_data::lang::Language, trusted: bool) -> Self {
        let get = |k: &str| lang.or_key(k).to_string();
        Self {
            title: get(if trusted {
                KEY_CONFIRM_TRUSTED
            } else {
                KEY_CONFIRM
            }),
            warning: get(KEY_WARNING),
            yes: get(if trusted { KEY_OPEN } else { KEY_YES }),
            copy: get(KEY_COPY),
            no: get(if trusted { KEY_CANCEL } else { KEY_NO }),
        }
    }
}

/// One open prompt: what the app keeps so a resize can rebuild it
/// (`Screen.resize` is `init()`).
#[derive(Clone, Debug, PartialEq)]
pub struct ConfirmLinkScreen {
    pub labels: ConfirmLinkLabels,
    /// `uri.toString()` — what is shown, copied and opened.
    pub url: String,
    pub trusted: bool,
}

impl ConfirmLinkScreen {
    pub fn new(lang: &rewo_data::lang::Language, url: impl Into<String>, trusted: bool) -> Self {
        Self {
            labels: ConfirmLinkLabels::resolve(lang, trusted),
            url: url.into(),
            trusted,
        }
    }
}

/// `MultiLineLabel.create(font, maxWidth, maxRows, message)`'s row list.
///
/// Rows come from `StringSplitter.splitLines`; when there are more than
/// `max_rows` the list is cut there and the last kept row is replaced by
/// `headByWidth(row, width(row) - width("..."))` followed by `...`. The cut
/// only applies when the row count *exceeds* the cap: a message of exactly
/// `max_rows` rows is untouched (`actualMaxLines != splitMessage.size()`).
pub fn wrap_message(
    message: &str,
    max_width: i32,
    max_rows: usize,
    width_of: &dyn Fn(&str) -> i32,
) -> Vec<String> {
    let mut rows = crate::disconnect_screen::split_lines(message, max_width, width_of);
    if rows.len() <= max_rows {
        return rows;
    }
    rows.truncate(max_rows);
    if let Some(last) = rows.last_mut() {
        let limit = width_of(last) - width_of(ELLIPSIS);
        *last = format!("{}{ELLIPSIS}", head_by_width(last, limit, width_of));
    }
    rows
}

/// `StringSplitter.headByWidth`: the longest prefix whose summed width stays
/// non-negative against `width` (`maxWidth -= w; if (maxWidth >= 0) keep`).
fn head_by_width(text: &str, width: i32, width_of: &dyn Fn(&str) -> i32) -> String {
    let mut left = width;
    let mut out = String::new();
    for c in text.chars() {
        left -= width_of(&c.to_string());
        if left < 0 {
            break;
        }
        out.push(c);
    }
    out
}

/// `ConfirmLinkScreen.init` + `repositionElements`.
///
/// `width_of` measures a string in the vanilla bitmap font.
pub fn build(
    screen: &ConfirmLinkScreen,
    gui_w: i32,
    gui_h: i32,
    width_of: &dyn Fn(&str) -> i32,
) -> Screen {
    let labels = &screen.labels;
    // `layout.defaultCellSetting().alignHorizontallyCenter()` — copied at each
    // `addChild`, and nothing changes it afterwards.
    let cell = Settings::defaults().align_h_center();
    let mut layout = Linear::vertical().spacing(ROW_SPACING).default_cell(cell);

    layout.add1(Element::leaf(TITLE, width_of(&labels.title), LINE_HEIGHT));

    let rows = wrap_message(&screen.url, gui_w - MESSAGE_MARGIN, MAX_ROWS, width_of);
    let message_w = rows.iter().map(|r| width_of(r)).max().unwrap_or(0);
    layout.add1(Element::leaf(
        MESSAGE,
        message_w,
        LINE_HEIGHT * rows.len() as i32,
    ));

    // `addAdditionalText()` — only an untrusted link carries the warning.
    if !screen.trusted {
        layout.add1(Element::leaf(
            WARNING,
            width_of(&labels.warning),
            LINE_HEIGHT,
        ));
    }

    // `buttonLayout.defaultCellSetting().paddingTop(16)` — a fresh settings
    // object, so no horizontal alignment: the row is aligned by its parent
    // cell and the buttons just sit side by side.
    let mut buttons = Linear::horizontal()
        .spacing(BUTTON_SPACING)
        .default_cell(Settings::defaults().padding_top(BUTTON_PADDING_TOP));
    for id in [YES, COPY, NO] {
        buttons.add1(Element::leaf(id, BUTTON_WIDTH, BUTTON_HEIGHT));
    }
    layout.add1(buttons.into_element());

    let mut root = layout.into_element();
    root.arrange();
    // `FrameLayout.centerInRectangle(this.layout, this.getRectangle())`.
    center_in_rectangle(&mut root, 0, 0, gui_w, gui_h);

    let mut placed = Vec::new();
    root.leaves(&mut placed);
    let widgets = placed
        .into_iter()
        .map(|(key, x, y, w, h)| match key {
            TITLE => Widget::label(key, x, y, w, labels.title.clone()),
            MESSAGE => Widget::multi_label(key, x, y, w, rows.clone(), true),
            WARNING => {
                Widget::label(key, x, y, w, labels.warning.clone()).with_color(WARNING_COLOR)
            }
            YES => Widget::button(key, x, y, w, h, labels.yes.clone()),
            COPY => Widget::button(key, x, y, w, h, labels.copy.clone()),
            _ => Widget::button(key, x, y, w, h, labels.no.clone()),
        })
        .collect();

    Screen::new(ScreenKind::ConfirmLink, gui_w, gui_h)
        .with_widgets(widgets)
        // `ConfirmScreen.shouldCloseOnEsc()` is false; Esc is answered by
        // `keyPressed` instead. See [`key_pressed`].
        .with_close_on_esc(false)
        // `Screen.extractBackground` with a level behind it:
        // blur + `INWORLD_MENU_BACKGROUND`.
        .with_menu_background(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screen::MouseResult;

    /// 6 px per character: readable wrap arithmetic.
    fn w6(s: &str) -> i32 {
        s.chars().count() as i32 * 6
    }

    fn lang() -> rewo_data::lang::Language {
        rewo_data::lang::Language::from_map(
            [
                (KEY_CONFIRM, "Are you sure?"),
                (KEY_CONFIRM_TRUSTED, "Open or copy?"),
                (
                    KEY_WARNING,
                    "Never open links from people that you don't trust!",
                ),
                (KEY_OPEN, "Open in Browser"),
                (KEY_COPY, "Copy to Clipboard"),
                (KEY_YES, "Yes"),
                (KEY_NO, "No"),
                (KEY_CANCEL, "Cancel"),
            ]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
        )
    }

    fn untrusted(url: &str) -> Screen {
        build(&ConfirmLinkScreen::new(&lang(), url, false), 320, 240, &w6)
    }

    /// `Screen.clickUrlAction`: `chatLinks` vetoes before `chatLinksPrompt`
    /// is even read, and the stock defaults (both on) are the prompt.
    ///
    /// MUTATION: testing `chat_links_prompt` first. `(false, true)` would then
    /// prompt for a link the player turned off.
    #[test]
    fn chat_links_vetoes_before_the_prompt_option_is_read() {
        assert_eq!(
            click_url_action(true, true),
            UrlClick::Prompt,
            "the default"
        );
        assert_eq!(click_url_action(true, false), UrlClick::OpenDirect);
        assert_eq!(click_url_action(false, true), UrlClick::Refused);
        assert_eq!(click_url_action(false, false), UrlClick::Refused);
    }

    /// An untrusted link: "Yes" / "Copy to Clipboard" / "No", the warning line.
    /// A trusted one: "Open in Browser" / "Cancel", and no warning widget.
    ///
    /// MUTATION: `trusted` hard-wired true — the click path would show the
    /// "trusted" wording and drop the warning.
    #[test]
    fn trust_picks_the_captions_and_the_warning() {
        let u = untrusted("https://example.com");
        assert_eq!(u.widget(TITLE).unwrap().message, "Are you sure?");
        assert_eq!(u.widget(YES).unwrap().message, "Yes");
        assert_eq!(u.widget(COPY).unwrap().message, "Copy to Clipboard");
        assert_eq!(u.widget(NO).unwrap().message, "No");
        let warn = u.widget(WARNING).expect("an untrusted link warns");
        assert_eq!(warn.label_color(), WARNING_COLOR);

        let t = build(
            &ConfirmLinkScreen::new(&lang(), "https://example.com", true),
            320,
            240,
            &w6,
        );
        assert_eq!(t.widget(TITLE).unwrap().message, "Open or copy?");
        assert_eq!(t.widget(YES).unwrap().message, "Open in Browser");
        assert_eq!(t.widget(NO).unwrap().message, "Cancel");
        assert!(t.widget(WARNING).is_none());
    }

    /// The vertical rhythm: title, 8, message, 8, warning, 8, then the button
    /// row whose cells carry 16 px of top padding — so the buttons sit
    /// `8 + 16` below the warning.
    ///
    /// MUTATION: `paddingTop(16)` moved onto the vertical cell (the buttons
    /// would sit 16 px lower than the row, not inside it) or the row spacing
    /// dropped to 4.
    #[test]
    fn rows_are_eight_apart_and_the_buttons_sit_sixteen_inside_their_row() {
        let s = untrusted("https://example.com"); // one row
        let y = |id| s.widget(id).unwrap().y;
        assert_eq!(y(MESSAGE) - y(TITLE), LINE_HEIGHT + ROW_SPACING);
        assert_eq!(y(WARNING) - y(MESSAGE), LINE_HEIGHT + ROW_SPACING);
        assert_eq!(
            y(YES) - y(WARNING),
            LINE_HEIGHT + ROW_SPACING + BUTTON_PADDING_TOP
        );
        // Three 100-wide buttons, 4 apart, one row.
        let x = |id| s.widget(id).unwrap().x;
        assert_eq!(x(COPY) - x(YES), BUTTON_WIDTH + BUTTON_SPACING);
        assert_eq!(x(NO) - x(COPY), BUTTON_WIDTH + BUTTON_SPACING);
        assert_eq!(y(YES), y(NO));
        for id in [YES, COPY, NO] {
            let b = s.widget(id).unwrap();
            assert_eq!((b.width, b.height), (BUTTON_WIDTH, BUTTON_HEIGHT));
        }
    }

    /// The whole stack is centred, and the button row (308 wide) is centred by
    /// its parent cell.
    #[test]
    fn the_stack_is_centred_in_the_screen() {
        let s = untrusted("https://example.com");
        let row_w = 3 * BUTTON_WIDTH + 2 * BUTTON_SPACING;
        assert_eq!(s.widget(YES).unwrap().x, (320 - row_w) / 2);
        let title = s.widget(TITLE).unwrap();
        assert_eq!(title.x, (320 - title.width) / 2);
        // Vertically: total = 9 + 8 + 9 + 8 + 9 + 8 + (16 + 20) = 87.
        assert_eq!(title.y, (240 - 87) / 2);
    }

    /// A link wraps at `width - 50` and the message widget grows with its rows.
    #[test]
    fn a_long_url_wraps_at_the_screen_width_minus_fifty() {
        let url = "https://example.com/".to_string() + &"a".repeat(100);
        let s = untrusted(&url);
        let m = s.widget(MESSAGE).unwrap();
        let crate::screen::WidgetKind::MultiLabel { lines, centered } = &m.kind else {
            panic!("the message is a MultiLineTextWidget");
        };
        assert!(*centered);
        assert!(lines.len() > 1);
        assert_eq!(lines.concat(), url, "wrapping drops nothing from a URL");
        for l in lines {
            assert!(w6(l) <= 320 - MESSAGE_MARGIN, "{l:?}");
        }
        assert_eq!(m.height, LINE_HEIGHT * lines.len() as i32);
    }

    /// `setMaxRows(15)`: more than 15 rows are cut, and the last kept row ends
    /// in `...` after being trimmed by the ellipsis's own width. Exactly 15
    /// rows are left alone.
    ///
    /// MUTATION: `>=` for the cap (a 15-row message would be clipped), or
    /// appending `...` without trimming (the last row would overflow by 18 px).
    #[test]
    fn rows_past_fifteen_are_cut_with_an_ellipsis() {
        // 10 chars per row at 6 px/char and width 60.
        let exactly = "x".repeat(150);
        let rows = wrap_message(&exactly, 60, MAX_ROWS, &w6);
        assert_eq!(rows.len(), 15);
        assert!(rows.iter().all(|r| r.len() == 10), "{rows:?}");

        let over = "x".repeat(151);
        let rows = wrap_message(&over, 60, MAX_ROWS, &w6);
        assert_eq!(rows.len(), 15);
        // Row width 60 - ellipsis 18 = 42 -> 7 chars kept, then "...".
        assert_eq!(rows[14], "xxxxxxx...");
        assert!(w6(&rows[14]) <= 60);
        assert!(rows[..14].iter().all(|r| r.len() == 10));
    }

    /// Yes opens; Copy copies THEN cancels (so it never opens); No cancels;
    /// nothing else decides.
    ///
    /// MUTATION: Copy mapped to `Open` — the button labelled "Copy" would
    /// launch the browser.
    #[test]
    fn each_button_maps_to_its_answer() {
        assert_eq!(decision_for(YES), Some(LinkDecision::Open));
        assert_eq!(decision_for(COPY), Some(LinkDecision::CopyThenCancel));
        assert_eq!(decision_for(NO), Some(LinkDecision::Cancel));
        for id in [TITLE, MESSAGE, WARNING] {
            assert_eq!(decision_for(id), None);
        }
    }

    /// A click on each button presses it; the text is click-through.
    #[test]
    fn the_buttons_are_pressable_and_the_text_is_not() {
        let mut s = untrusted("https://example.com");
        for id in [YES, COPY, NO] {
            let b = s.widget(id).unwrap().clone();
            assert_eq!(
                s.mouse_clicked(b.x as f64 + 2.0, b.y as f64 + 2.0, 0),
                MouseResult::Pressed(id)
            );
        }
        let m = s.widget(MESSAGE).unwrap().clone();
        assert_eq!(
            s.mouse_clicked(m.x as f64 + 1.0, m.y as f64 + 1.0, 0),
            MouseResult::Ignored
        );
    }

    /// Esc is the "No" answer, delivered as a press, and the framework's own
    /// close-on-Esc is off.
    ///
    /// MUTATION: leaving `close_on_esc` true — Esc would return
    /// `KeyResult::Close`, which the app turns into "close the view screen",
    /// stranding the chat the prompt was opened over.
    #[test]
    fn esc_answers_no() {
        let mut s = untrusted("https://example.com");
        assert!(!s.close_on_esc);
        assert_eq!(s.key_pressed(256, false), KeyResult::Ignored);
        assert_eq!(key_pressed(&mut s, 256, false), KeyResult::Pressed(NO));
        // Other keys are `Screen.keyPressed`: Tab moves focus.
        assert_eq!(key_pressed(&mut s, 258, false), KeyResult::Handled);
        assert_eq!(s.focused(), Some(YES));
    }

    /// `labels` follow the language map, falling back to the key.
    #[test]
    fn labels_fall_back_to_the_key() {
        let empty = rewo_data::lang::Language::from_map(Default::default());
        let l = ConfirmLinkLabels::resolve(&empty, false);
        assert_eq!(l.title, KEY_CONFIRM);
        assert_eq!(l.copy, KEY_COPY);
    }
}
