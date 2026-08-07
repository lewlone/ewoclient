//! `DisconnectedScreen` (M85) — **the one screen with no session behind it.**
//!
//! Every screen Rewo had before this one is opened while a world is loaded: the
//! inventory needs a player, the death screen needs a `player_combat_kill`, and
//! [`crate::pause_screen`] is Esc *during* play. This one exists precisely when
//! there is nothing left — the socket is gone, `PlaySession` is dropped, and
//! anything the screen needs must already have been copied somewhere that
//! outlives it.
//!
//! That is the shape `REWO_PLAN.md` §0.0 gotcha 13 names one packet over: a
//! handler that opens by looking something up in state that the event destroys
//! is wrong, and a gate that *builds* that state cannot notice. Here the state
//! is the server's links: they arrive during configuration or play, they live
//! on `SessionState` (which is `ClientCommonPacketListenerImpl`'s own home for
//! them), and `PlaySession` owns that. **A links list read off the session at
//! disconnect time is read off a session that has ended.** So the app mirrors
//! them out while the session is alive, and this module takes them by value.
//!
//! # The links on this screen are one link, and only ever `BUG_REPORT`
//!
//! `DisconnectedScreen` never mentions `ServerLinks`. What it renders is
//!
//! ```java
//! this.details.bugReportLink().ifPresent(bugReportLink ->
//!    this.layout.addChild(Button.builder(REPORT_TO_SERVER_TITLE, ConfirmLinkScreen.confirmLink(this, bugReportLink, false)).width(200).build()));
//! ```
//!
//! and `DisconnectionDetails.bugReportLink` is filled in exactly two places,
//! both of them *client-side error* paths on the common listener:
//!
//! ```java
//! // onPacketError — a handler threw
//! Optional<URI> bugReportLink = this.serverLinks.findKnownType(KnownLinkType.BUG_REPORT).map(Entry::link);
//! this.connection.disconnect(new DisconnectionDetails(Component.translatable("disconnect.packetError"), report, bugReportLink));
//! // createDisconnectionInfo — Connection.exceptionCaught
//! return new DisconnectionDetails(reason, report, bugReportUrl);
//! ```
//!
//! A server-sent `ClientboundDisconnectPacket` goes through
//! `connection.disconnect(packet.reason())`, which is
//! `disconnect(new DisconnectionDetails(reason))` — the one-argument
//! constructor, **both optionals empty**. So a server that kicks you politely
//! shows no link however many it advertised, and one whose packet crashes your
//! client shows exactly one. [`DisconnectCause`] is that distinction, and it is
//! the whole reason this screen is where `server_links` lands rather than
//! somewhere the packet is merely stored.
//!
//! # The layout
//!
//! ```java
//! private final LinearLayout layout = LinearLayout.vertical();      // no spacing()
//! this.layout.defaultCellSetting().alignHorizontallyCenter().padding(10);
//! this.layout.addChild(new StringWidget(this.title, this.font));
//! this.layout.addChild(new MultiLineTextWidget(this.details.reason(), this.font).setMaxWidth(this.width - 50).setCentered(true));
//! this.layout.defaultCellSetting().padding(2);
//! …optional buttons…
//! this.layout.addChild(backButton);                                 // width 200
//! this.layout.arrangeElements();
//! …
//! FrameLayout.centerInRectangle(this.layout, this.getRectangle());
//! ```
//!
//! * **`LinearLayout.vertical()` with no `spacing()`** — the row spacing is
//!   **0** and every gap comes from the cell padding. Adding a spacing would
//!   push the whole stack apart.
//! * **`defaultCellSetting()` is mutated mid-build**, and `addChild` copies it
//!   at add time — so the title and the reason carry `padding(10)` and every
//!   button below carries `padding(2)`. Reading the second call as replacing
//!   the first retroactively would pull the title down 8 px.
//! * `padding(2)` does **not** clear `alignHorizontallyCenter` — the two-,
//!   three- and four-argument `padding` overloads only touch padding.
//! * **`shouldCloseOnEsc()` is false**, like the death screen. There is nothing
//!   to go back to.
//!
//! # The reason wraps
//!
//! `setMaxWidth(this.width - 50)` and `MultiLineLabel.create(font, message,
//! maxWidth)` → `StringSplitter.splitLines`. [`split_lines`] transcribes the
//! observable rule; the module docs there say what it does not.
//!
//! # Ground truth (bundled 26.2 decompile)
//!
//! - `net/minecraft/client/gui/screens/DisconnectedScreen.java`
//! - `net/minecraft/network/DisconnectionDetails.java`
//! - `net/minecraft/network/Connection.java` — `disconnect(Component)`,
//!   `channelInactive`, `exceptionCaught`
//! - `net/minecraft/client/multiplayer/ClientCommonPacketListenerImpl.java` —
//!   `handleDisconnect`, `onPacketError`, `createDisconnectionInfo`,
//!   `createDisconnectScreen`
//! - `net/minecraft/client/StringSplitter.java` — `splitLines`,
//!   `LineBreakFinder`

use crate::layout::{center_in_rectangle, Element, Linear, Settings};
use crate::screen::{Screen, ScreenKind, Widget, WidgetId, BUTTON_HEIGHT, LINE_HEIGHT};

/// `gui.toMenu` — "Back to Server List".
pub const BACK: WidgetId = 0;
/// `gui.report_to_server` — present only when [`DisconnectDetails::bug_report_link`] is.
pub const REPORT: WidgetId = 1;
/// The `disconnect.lost` title.
pub const TITLE: WidgetId = 2;
/// The wrapped reason.
pub const REASON: WidgetId = 3;

/// `DisconnectedScreen`'s cell padding, before and after the mid-build change.
pub const TEXT_PADDING: i32 = 10;
pub const BUTTON_PADDING: i32 = 2;
/// `Button.builder(...).width(200)` for every button on this screen.
pub const BUTTON_WIDTH: i32 = 200;
/// `setMaxWidth(this.width - 50)`.
pub const REASON_MARGIN: i32 = 50;

/// `ClientCommonPacketListenerImpl.GENERIC_DISCONNECT_MESSAGE`.
pub const KEY_TITLE: &str = "disconnect.lost";
/// `DisconnectedScreen.TO_SERVER_LIST`.
pub const KEY_BACK: &str = "gui.toMenu";
/// `DisconnectedScreen.REPORT_TO_SERVER_TITLE`.
pub const KEY_REPORT: &str = "gui.report_to_server";
/// `Connection.channelInactive`'s reason.
pub const KEY_END_OF_STREAM: &str = "disconnect.endOfStream";

/// Why the connection ended — which is what decides whether the server's
/// bug-report link is offered.
///
/// Vanilla does not have this enum; it has three call sites that build a
/// `DisconnectionDetails` differently. Naming them is what makes the
/// distinction assertable, and the distinction is the milestone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DisconnectCause {
    /// `handleDisconnect` — a `ClientboundDisconnectPacket`.
    /// `connection.disconnect(packet.reason())` → the one-argument
    /// `DisconnectionDetails`, **no link**.
    ServerRequested,
    /// `Connection.channelInactive` — the socket closed with no packet.
    /// `disconnect(Component.translatable("disconnect.endOfStream"))`, also the
    /// one-argument constructor and also **no link**.
    EndOfStream,
    /// `onPacketError` / `Connection.exceptionCaught` → `createDisconnectionInfo`,
    /// which is the *only* producer that fills `bugReportLink`.
    ClientError,
}

impl DisconnectCause {
    /// Whether this cause reaches `createDisconnectionInfo`.
    pub fn fills_bug_report_link(self) -> bool {
        matches!(self, DisconnectCause::ClientError)
    }
}

/// `DisconnectionDetails`, minus the crash-report `Path` Rewo never writes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DisconnectDetails {
    /// The reason, already flattened to plain text.
    pub reason: String,
    /// `serverLinks.findKnownType(BUG_REPORT).map(Entry::link)`, and **only**
    /// on a [`DisconnectCause::ClientError`].
    pub bug_report_link: Option<String>,
}

impl DisconnectDetails {
    /// The two producers, in one place: a cause, and the `BUG_REPORT` link the
    /// connection had when it ended.
    ///
    /// `candidate` is the caller's
    /// `serverLinks.findKnownType(BUG_REPORT).map(Entry::link)` — resolved by
    /// the app, not here, because **`rewo-net` depends on `rewo-world`** and
    /// the reverse edge would be a cycle. That is not a workaround: the app is
    /// also the layer that has to have *kept* the links past the session's
    /// death (see the module docs), so the lookup belongs where the durable
    /// copy lives.
    ///
    /// The cause is what gates it, and that gate is the whole transcription:
    /// `createDisconnectionInfo` fills the field and
    /// `new DisconnectionDetails(reason)` does not.
    pub fn new(
        cause: DisconnectCause,
        reason: impl Into<String>,
        candidate: Option<&str>,
    ) -> Self {
        Self {
            reason: reason.into(),
            bug_report_link: cause
                .fills_bug_report_link()
                .then(|| candidate.map(str::to_string))
                .flatten(),
        }
    }
}

/// The strings this screen needs.
#[derive(Clone, Debug, PartialEq)]
pub struct DisconnectLabels {
    pub title: String,
    pub back: String,
    pub report: String,
}

impl DisconnectLabels {
    pub fn resolve(lang: &rewo_data::lang::Language) -> Self {
        Self {
            title: lang.or_key(KEY_TITLE).to_string(),
            back: lang.or_key(KEY_BACK).to_string(),
            report: lang.or_key(KEY_REPORT).to_string(),
        }
    }
}

/// `StringSplitter.splitLines` — greedy wrap at the last space that fits, hard
/// break when there is none, and `\n` always breaks.
///
/// ```java
/// case 10:  return this.finishIteration(adjustedPosition, style);       // '\n'
/// case 32:  this.lastSpace = adjustedPosition; …                        // ' '
/// default:  this.width += charWidth;
///           if (!this.hadNonZeroWidthChar || !(this.width > this.maxWidth)) { … keep going … }
///           else return this.lastSpace != -1 ? finishIteration(this.lastSpace, …)
///                                            : finishIteration(adjustedPosition, style);
/// ```
///
/// and the outer loop drops the break character **only when it is a space or a
/// newline**:
///
/// ```java
/// int adjustedBreak = firstTailChar != '\n' && firstTailChar != ' ' ? lineBreak : lineBreak + 1;
/// ```
///
/// Three details that are not the obvious greedy-wrap:
///
/// * **The overflow test is `width > maxWidth` measured *after* adding the
///   character**, so a line whose width exactly equals `maxWidth` still fits.
/// * **`hadNonZeroWidthChar` guarantees progress**: the first visible character
///   of a line is always accepted, even if it alone is wider than `maxWidth`,
///   so a very narrow box makes one-character lines rather than looping.
/// * **`maxWidth` is floored at 1** (`Math.max(maxWidth, 1.0F)`), which is what
///   stops a zero or negative width from doing the same.
///
/// **What is not transcribed:** style runs (Rewo flattens a `Component` to
/// plain text before it gets here, so there is one style), the `§`-code
/// re-emission `StringDecomposer` does, surrogate pairs, and bidi. Every one of
/// those changes *where* a break lands only for text this screen does not
/// receive — a disconnect reason is a short plain sentence — and each is a
/// larger subsystem than the wrap.
///
/// **This is the `String` overload.** M108 needed the `FormattedText` one, and
/// they are not the same function — see [`crate::string_splitter`], which owns
/// the shared `LineBreakFinder` both call and documents where the two diverge.
/// The sweep below moved there verbatim; every break lands where it did.
pub fn split_lines(text: &str, max_width: i32, width_of: &dyn Fn(&str) -> i32) -> Vec<String> {
    let bytes: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut start = 0usize;
    while start < bytes.len() {
        let Some(line_break) = crate::string_splitter::find_line_break(
            &bytes, start, max_width, width_of,
        ) else {
            // `endOfText` — the whole remainder is one line.
            out.push(bytes[start..].iter().collect());
            break;
        };
        let tail = bytes[line_break];
        let adjusted = if tail != '\n' && tail != ' ' {
            line_break
        } else {
            line_break + 1
        };
        out.push(bytes[start..line_break].iter().collect());
        start = adjusted;
    }
    if out.is_empty() {
        // A zero-length message is one empty line, which is what
        // `MultiLineLabel.create` on an empty component produces: `getLineCount`
        // is 1, so the widget is 9 px tall rather than 0.
        out.push(String::new());
    }
    out
}

/// `DisconnectedScreen.init` + `repositionElements`.
///
/// `width_of` measures a string in the vanilla bitmap font; the caller owns the
/// advance table.
pub fn build(
    labels: &DisconnectLabels,
    details: &DisconnectDetails,
    gui_w: i32,
    gui_h: i32,
    width_of: &dyn Fn(&str) -> i32,
) -> Screen {
    // `LinearLayout.vertical()` — **no spacing**. Every gap is cell padding.
    let text_cell = Settings::defaults().align_h_center().padding(TEXT_PADDING);
    let button_cell = Settings::defaults()
        .align_h_center()
        .padding(BUTTON_PADDING);
    let mut layout = Linear::vertical();

    let title_w = width_of(&labels.title);
    layout.add(Element::leaf(TITLE, title_w, LINE_HEIGHT), text_cell);

    // `new MultiLineTextWidget(reason, font).setMaxWidth(this.width - 50)`:
    // `getWidth()` is the label's own width (the widest line), `getHeight()` is
    // `lineCount * 9`.
    let max_reason_width = gui_w - REASON_MARGIN;
    let lines = split_lines(&details.reason, max_reason_width, width_of);
    let reason_w = lines.iter().map(|l| width_of(l)).max().unwrap_or(0);
    layout.add(
        Element::leaf(REASON, reason_w, LINE_HEIGHT * lines.len() as i32),
        text_cell,
    );

    // `this.layout.defaultCellSetting().padding(2)` — from here down.
    if details.bug_report_link.is_some() {
        layout.add(
            Element::leaf(REPORT, BUTTON_WIDTH, BUTTON_HEIGHT),
            button_cell,
        );
    }
    // `this.details.report()` — Rewo writes no disconnect crash report, so the
    // "Open report directory" button never appears. Named, not silently
    // dropped: it is a real vanilla widget in this stack.
    layout.add(
        Element::leaf(BACK, BUTTON_WIDTH, BUTTON_HEIGHT),
        button_cell,
    );

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
            REASON => Widget::multi_label(key, x, y, w, lines.clone(), true),
            REPORT => Widget::button(key, x, y, w, h, labels.report.clone()),
            _ => Widget::button(key, x, y, w, h, labels.back.clone()),
        })
        .collect();

    Screen::new(ScreenKind::Disconnected, gui_w, gui_h)
        .with_widgets(widgets)
        // `shouldCloseOnEsc()` is overridden to false — the same override the
        // death screen carries, and for the same reason.
        .with_close_on_esc(false)
        // `minecraft.level == null` by the time this screen exists, so
        // `extractMenuBackground` takes `MENU_BACKGROUND` rather than the
        // in-world variant.
        .with_menu_background(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screen::{KeyResult, WidgetKind};

    /// A monospace-ish stand-in: 6 px per character, which is close enough to
    /// the vanilla font that the wrap arithmetic is readable in the test.
    fn w6(s: &str) -> i32 {
        s.chars().count() as i32 * 6
    }

    fn labels() -> DisconnectLabels {
        DisconnectLabels {
            title: "Connection Lost".into(),
            back: "Back to Server List".into(),
            report: "Report to Server".into(),
        }
    }

    /// What `serverLinks.findKnownType(BUG_REPORT).map(Entry::link)` yields on
    /// a server that advertised one. (The `findKnownType` half is graded in
    /// `rewo_net::server_links`; this module owns only the cause gate.)
    const BUG_LINK: Option<&str> = Some("https://bugs.example");

    /// Only a client-side error fills the bug-report link.
    ///
    /// MUTATION: filling it for every cause. A server that kicks you would then
    /// show a "Report to Server" button vanilla does not draw — and it would
    /// *look* right, because the link is real and the label is real. The
    /// distinction lives entirely in which of `DisconnectionDetails`' two
    /// constructors ran.
    #[test]
    fn only_a_client_side_error_offers_the_servers_bug_report_link() {
        assert_eq!(
            DisconnectDetails::new(DisconnectCause::ClientError, "boom", BUG_LINK).bug_report_link,
            Some("https://bugs.example".into())
        );
        for quiet in [DisconnectCause::ServerRequested, DisconnectCause::EndOfStream] {
            assert_eq!(
                DisconnectDetails::new(quiet, "You are banned", BUG_LINK).bug_report_link,
                None,
                "{quiet:?}"
            );
        }
        // And with no BUG_REPORT entry there is nothing to offer even on an
        // error — `findKnownType` returns empty, and `Optional.map` of empty is
        // empty.
        assert_eq!(
            DisconnectDetails::new(DisconnectCause::ClientError, "boom", None).bug_report_link,
            None
        );
    }

    /// The report button appears exactly when the link does, and pushes the
    /// back button down.
    #[test]
    fn the_report_button_appears_with_the_link_and_moves_the_back_button() {
        let quiet = DisconnectDetails::new(
            DisconnectCause::ServerRequested,
            "Server closed",
            BUG_LINK,
        );
        let noisy =
            DisconnectDetails::new(DisconnectCause::ClientError, "Server closed", BUG_LINK);
        let a = build(&labels(), &quiet, 320, 240, &w6);
        let b = build(&labels(), &noisy, 320, 240, &w6);
        assert!(a.widget(REPORT).is_none());
        let r = b.widget(REPORT).unwrap();
        assert_eq!(r.width, BUTTON_WIDTH);
        assert!(r.y < b.widget(BACK).unwrap().y);
        // 20 + 2 + 2 of padding between the two buttons.
        assert_eq!(b.widget(BACK).unwrap().y - r.y, BUTTON_HEIGHT + 4);
        // The extra row makes the stack taller, so centring moves it **up**.
        assert!(b.widget(TITLE).unwrap().y < a.widget(TITLE).unwrap().y);
    }

    /// The two paddings, and that the second call does not retroactively change
    /// the first two cells.
    ///
    /// MUTATION: applying `padding(2)` to every cell (the reading where
    /// `defaultCellSetting()` is consulted at arrange time rather than copied at
    /// add time). The title/reason gap collapses from 20 to 4 and the whole
    /// stack shrinks by 32 px.
    #[test]
    fn the_title_and_reason_carry_ten_pixel_padding_and_the_buttons_two() {
        let d = DisconnectDetails::new(DisconnectCause::EndOfStream, "Short", None);
        let s = build(&labels(), &d, 320, 240, &w6);
        let t = s.widget(TITLE).unwrap();
        let r = s.widget(REASON).unwrap();
        let b = s.widget(BACK).unwrap();
        // 9 + 10 (title bottom) + 10 (reason top)
        assert_eq!(r.y - t.y, LINE_HEIGHT + TEXT_PADDING * 2);
        // reason height 9 + 10 (reason bottom) + 2 (button top)
        assert_eq!(b.y - r.y, LINE_HEIGHT + TEXT_PADDING + BUTTON_PADDING);
    }

    /// The whole stack is centred in the screen, both axes.
    #[test]
    fn the_stack_is_centred_in_the_screen() {
        let d = DisconnectDetails::new(DisconnectCause::EndOfStream, "Short", None);
        let s = build(&labels(), &d, 320, 240, &w6);
        let b = s.widget(BACK).unwrap();
        assert_eq!(b.x, (320 - BUTTON_WIDTH) / 2);
        let t = s.widget(TITLE).unwrap();
        assert_eq!(t.x, (320 - w6("Connection Lost")) / 2);
    }

    /// Esc does nothing — there is nowhere to go.
    #[test]
    fn esc_cannot_dismiss_the_disconnect_screen() {
        let d = DisconnectDetails::new(DisconnectCause::EndOfStream, "x", None);
        let mut s = build(&labels(), &d, 320, 240, &w6);
        assert!(!s.close_on_esc);
        assert_eq!(s.key_pressed(256, false), KeyResult::Ignored);
    }

    /// The wrap: at the last space that fits, hard-breaking when there is none,
    /// and a line exactly `maxWidth` wide still fits.
    ///
    /// MUTATION: `width >= maxWidth` instead of `>`. "abc def" at 18 px (three
    /// 6-px characters) is exactly the boundary — with `>` the first line is
    /// "abc", with `>=` it is "ab".
    #[test]
    fn the_wrap_breaks_at_the_last_space_and_a_line_of_exactly_max_width_fits() {
        assert_eq!(split_lines("abc def", 18, &w6), vec!["abc", "def"]);
        assert_eq!(split_lines("abcdef", 18, &w6), vec!["abc", "def"]);
        assert_eq!(split_lines("abc", 18, &w6), vec!["abc"]);
        // The space at the break is dropped; a hard break's character is kept.
        assert_eq!(split_lines("aa bb cc", 18, &w6), vec!["aa", "bb", "cc"]);
        // `\n` always breaks and is dropped.
        assert_eq!(split_lines("a\nb", 600, &w6), vec!["a", "b"]);
        // `hadNonZeroWidthChar` guarantees progress rather than looping.
        assert_eq!(split_lines("abc", 1, &w6), vec!["a", "b", "c"]);
        // An empty message is one empty line, not zero lines.
        assert_eq!(split_lines("", 100, &w6), vec![""]);
    }

    /// A long reason really does become a multi-line widget of the right
    /// height, and it is a `MultiLabel` rather than N labels.
    #[test]
    fn a_long_reason_wraps_into_one_widget_nine_pixels_per_line() {
        let reason = "the server closed the connection because of a very long \
                      administrative reason indeed";
        let d = DisconnectDetails::new(DisconnectCause::ServerRequested, reason, None);
        let s = build(&labels(), &d, 320, 240, &w6);
        let r = s.widget(REASON).unwrap();
        let WidgetKind::MultiLabel { lines, centered } = &r.kind else {
            panic!("the reason is a MultiLineTextWidget");
        };
        assert!(lines.len() > 1, "{lines:?}");
        assert!(*centered);
        assert_eq!(r.height, LINE_HEIGHT * lines.len() as i32);
        // Every line fits `width - 50`.
        for l in lines {
            assert!(w6(l) <= 320 - REASON_MARGIN, "{l:?}");
        }
        assert!(!r.active, "a text widget is click-through");
    }
}
