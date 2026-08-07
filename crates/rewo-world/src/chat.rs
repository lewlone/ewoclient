//! `ChatComponent` — the chat HUD's model (M108).
//!
//! Before this, Rewo's chat was `chat_log: Vec<String>` on the play session and
//! a render that drew the last eight entries, each truncated at 80 characters,
//! on a fixed 10-px pitch. Nothing wrapped, nothing faded, nothing scrolled,
//! and a message carried no signature — which is why
//! `delete_chat` had nothing to delete from and sat in
//! `REWO_PACKET_COVERAGE.md` as class C.
//!
//! This module is `net/minecraft/client/gui/components/ChatComponent.java`
//! plus the two records it stores (`GuiMessage`, `GuiMessage.Line`) and the tag
//! table (`GuiMessageTag`). The *render* reads [`ChatComponent::visible_lines`];
//! the geometry it needs is computed here so a pixel gate and a unit test grade
//! the same arithmetic.
//!
//! # Two hundred-caps, not one
//!
//! `allMessages` and `trimmedMessages` are trimmed **independently**, both to
//! 100. They count different things — messages and *wrapped lines* — so a chat
//! full of long messages holds far fewer than 100 messages' worth of lines, and
//! `refreshTrimmedMessages` (which replays `allMessages` through the wrap) can
//! therefore drop the oldest messages entirely. Modelling one cap makes a
//! rescale silently lengthen the backlog.
//!
//! # The line list is newest-first, and its per-message order is reversed
//!
//! `addMessageToDisplayQueue` walks a message's wrapped lines **forwards** and
//! `addFirst`s each, so a three-line message lands as `[line2, line1, line0]`
//! at the head. The renderer then walks index 0 upward from the bottom of the
//! box, which puts `line0` above `line1` above `line2` — reading order. Storing
//! the lines in the order they were produced and reversing at draw time gives
//! the same picture for a one-line message and mirrors every multi-line one.
//!
//! `endOfEntry` is set on the **last** wrapped line (`i == lines.size() - 1`),
//! which is the one at index 0 — the *bottom* line of the message.

use crate::string_splitter::split_lines_wrapped;

/// `ChatComponent.MAX_CHAT_HISTORY` — applied separately to the message list
/// and the wrapped-line list.
pub const MAX_CHAT_HISTORY: usize = 100;

/// `ChatComponent.MESSAGE_INDENT` — the pose is translated by this before any
/// line is drawn, so every x below is relative to it and the backdrop starts at
/// `-4`.
pub const MESSAGE_INDENT: i32 = 4;

/// `ChatComponent.BOTTOM_MARGIN` — the gap between the bottom of the chat box
/// and the bottom of the screen, in *scaled* chat pixels.
pub const BOTTOM_MARGIN: i32 = 40;

/// `ChatComponent.TIME_BEFORE_MESSAGE_DELETION` — a message younger than this
/// many ticks cannot be replaced by the deleted marker yet; the request is
/// queued instead. See [`ChatComponent::delete_message`].
pub const TIME_BEFORE_MESSAGE_DELETION: i32 = 60;

/// The height of one message's text, in GUI pixels. Vanilla writes this as a
/// local `int messageHeight = 9` rather than a constant.
pub const MESSAGE_HEIGHT: i32 = 9;

/// Where a message came from — `GuiMessageSource`. Carried so the deleted
/// marker can be `SYSTEM_SERVER` regardless of what it replaced.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MessageSource {
    Player,
    SystemServer,
    SystemClient,
}

/// `GuiMessageTag` — the coloured bar drawn in the two pixels left of a
/// message, plus its hover text and log prefix.
///
/// The whole table is four constants; the only `Icon` variant vanilla has is
/// `CHAT_MODIFIED`, so the icon is modelled as a bool rather than an enum with
/// one member.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct MessageTag {
    /// `indicatorColor`, 0xRRGGBB.
    pub indicator_color: u32,
    /// Whether this tag carries `Icon.CHAT_MODIFIED` (9×9).
    pub icon: bool,
    /// `logTag` — the bracketed prefix on the client's own log line.
    pub log_tag: &'static str,
}

impl MessageTag {
    /// `GuiMessageTag.system()`.
    pub const SYSTEM: Self = Self {
        indicator_color: 13684944,
        icon: false,
        log_tag: "System",
    };
    /// `GuiMessageTag.systemSinglePlayer()` — the same colour and log tag as
    /// [`Self::SYSTEM`], differing only in hover text, which Rewo does not
    /// render. Kept distinct because `addClientSystemMessage` and
    /// `addServerSystemMessage` both use it and `createDeletedMarker` uses the
    /// other.
    pub const SYSTEM_SINGLE_PLAYER: Self = Self {
        indicator_color: 13684944,
        icon: false,
        log_tag: "System",
    };
    /// `GuiMessageTag.chatNotSecure()`.
    pub const CHAT_NOT_SECURE: Self = Self {
        indicator_color: 13684944,
        icon: false,
        log_tag: "Not Secure",
    };
    /// `GuiMessageTag.chatModified(..)` — the one tag with an icon.
    pub const CHAT_MODIFIED: Self = Self {
        indicator_color: 6316128,
        icon: true,
        log_tag: "Modified",
    };
    /// `GuiMessageTag.chatError()`.
    pub const CHAT_ERROR: Self = Self {
        indicator_color: 16733525,
        icon: true,
        log_tag: "Chat Error",
    };
}

/// `MessageSignature` — 256 bytes, compared by value.
///
/// Boxed because a `GuiMessage` is cloned into the trimmed-line list and 256
/// bytes inline would dominate it.
pub type Signature = [u8; 256];

/// `GuiMessage` — one chat entry as stored, before wrapping.
#[derive(Clone, Debug)]
pub struct GuiMessage {
    /// `addedTime` — the GUI tick the message arrived on. The fade and the
    /// deletion delay both measure against it.
    pub added_time: i32,
    /// `content`, already flattened to plain text (Rewo has no styled chat
    /// pipeline into the HUD yet — see the module docs of
    /// [`crate::string_splitter`]).
    pub content: String,
    /// `signature` — `None` for system messages and for unsigned player chat.
    /// This is what [`ChatComponent::delete_message`] keys on.
    pub signature: Option<Box<Signature>>,
    pub source: MessageSource,
    pub tag: Option<MessageTag>,
}

/// `GuiMessage.Line` — one wrapped line of a message.
#[derive(Clone, Debug)]
pub struct GuiMessageLine {
    pub text: String,
    /// `parent.addedTime()`.
    pub added_time: i32,
    /// `endOfEntry` — set on the message's **last** wrapped line, which is the
    /// one nearest the bottom of the box.
    pub end_of_entry: bool,
    pub tag: Option<MessageTag>,
}

/// `ComponentRenderUtils.wrapComponents`.
///
/// Three rules, none of which belongs to the splitter underneath:
///
/// 1. **A width-wrapped continuation is prefixed with one space** —
///    `FormattedCharSequence.composite(INDENT, reorderedText)` where `INDENT`
///    is `codepoint(32, Style.EMPTY)`. Applied *after* wrapping, so the space
///    does not count toward `maxWidth` and a continuation line is one space
///    wider than the box it was wrapped to. A line that follows an explicit
///    `\n` gets none — see [`crate::string_splitter`] for why that is not the
///    same as "every line after the first".
/// 2. **An empty result becomes one empty line** —
///    `result.isEmpty() ? [FormattedCharSequence.EMPTY] : result`. An empty
///    message still occupies a row.
/// 3. `stripColor` drops formatting when `options.chatColors` is off. Rewo has
///    no options file and the vanilla default is on, so this is the identity
///    and is not modelled; it is named here rather than silently omitted.
pub fn wrap_components(text: &str, max_width: i32, width_of: &dyn Fn(&str) -> i32) -> Vec<String> {
    let mut out: Vec<String> = split_lines_wrapped(text, max_width, width_of)
        .into_iter()
        .map(|l| {
            if l.wrapped {
                let mut s = String::with_capacity(l.text.len() + 1);
                s.push(' ');
                s.push_str(&l.text);
                s
            } else {
                l.text
            }
        })
        .collect();
    if out.is_empty() {
        out.push(String::new());
    }
    out
}

/// `ChatComponent.getWidth(double)` — the box's width in GUI pixels.
///
/// The method declares `int max = 320; int min = 40;` and **uses neither**;
/// the body is `Mth.floor(pct * 280.0 + 40.0)`. Two dead locals, like the
/// `int border = 4;` M104 found in `extractRenderState`. Transcribed as the
/// expression, not as a clamp — a `clamp(40, 320)` reading agrees for every
/// `pct` in 0..=1 and diverges the moment a caller passes anything else.
pub fn width_of_pct(pct: f64) -> i32 {
    (pct * 280.0 + 40.0).floor() as i32
}

/// `ChatComponent.getHeight(double)` — same shape, same two dead locals
/// (`max = 180`, `min = 20`), body `Mth.floor(pct * 160.0 + 20.0)`.
pub fn height_of_pct(pct: f64) -> i32 {
    (pct * 160.0 + 20.0).floor() as i32
}

/// `ChatComponent.defaultUnfocusedPct()` — `70.0 / (getHeight(1.0) - 20)`.
///
/// `getHeight(1.0)` is 180, so this is `70/160`, the fraction that makes an
/// unfocused box 90 px tall. Written as the division rather than the constant
/// because that is what vanilla evaluates.
pub fn default_unfocused_pct() -> f64 {
    70.0 / (height_of_pct(1.0) - 20) as f64
}

/// The four `options` values the chat geometry reads.
///
/// Rewo has no options file, so these are vanilla's defaults; they are a struct
/// rather than constants because `getHeight` picks between the focused and
/// unfocused heights per call and a gate needs to drive both.
#[derive(Clone, Copy, Debug)]
pub struct ChatOptions {
    /// `options.chatScale()`.
    pub scale: f64,
    /// `options.chatWidth()`.
    pub width_pct: f64,
    /// `options.chatHeightFocused()`.
    pub height_focused_pct: f64,
    /// `options.chatHeightUnfocused()`.
    pub height_unfocused_pct: f64,
    /// `options.chatLineSpacing()`.
    pub line_spacing: f64,
    /// `options.chatOpacity()`.
    pub opacity: f64,
    /// `options.textBackgroundOpacity()`.
    pub text_background_opacity: f64,
}

impl Default for ChatOptions {
    /// `Options`' own defaults: scale 1, width 1, focused height 1, unfocused
    /// height [`default_unfocused_pct`], line spacing 0, chat opacity 1,
    /// text-background opacity 0.5.
    fn default() -> Self {
        Self {
            scale: 1.0,
            width_pct: 1.0,
            height_focused_pct: 1.0,
            height_unfocused_pct: default_unfocused_pct(),
            line_spacing: 0.0,
            opacity: 1.0,
            text_background_opacity: 0.5,
        }
    }
}

impl ChatOptions {
    /// `ChatComponent.getWidth()`.
    pub fn width(&self) -> i32 {
        width_of_pct(self.width_pct)
    }

    /// `ChatComponent.getHeight()` — **the focused box is a different size**,
    /// so the caller must say which is up.
    pub fn height(&self, focused: bool) -> i32 {
        height_of_pct(if focused {
            self.height_focused_pct
        } else {
            self.height_unfocused_pct
        })
    }

    /// `ChatComponent.getLineHeight()` — `(int)(9.0 * (spacing + 1.0))`, a
    /// truncating cast.
    pub fn line_height(&self) -> i32 {
        (MESSAGE_HEIGHT as f64 * (self.line_spacing + 1.0)) as i32
    }

    /// `ChatComponent.getLinesPerPage()` — an integer division of the box
    /// height by the line height, so the last partial line is not shown.
    pub fn lines_per_page(&self, focused: bool) -> i32 {
        self.height(focused) / self.line_height()
    }

    /// `entryHeight` in `extractRenderState` — the same expression as
    /// [`Self::line_height`], recomputed locally by vanilla.
    pub fn entry_height(&self) -> i32 {
        self.line_height()
    }

    /// `entryBottomToMessageY` — `(int)Math.round(8.0 * (spacing + 1.0) - 4.0 * spacing)`.
    ///
    /// **Not** `entryHeight - 1`, and not a scaled 8: the two terms move in
    /// opposite directions with the spacing, which is what keeps the text
    /// centred in a taller row rather than pinned to its top.
    pub fn entry_bottom_to_message_y(&self) -> i32 {
        (8.0 * (self.line_spacing + 1.0) - 4.0 * self.line_spacing).round() as i32
    }

    /// The text opacity multiplier — `chatOpacity * 0.9 + 0.1`, so the floor is
    /// 0.1 rather than 0 even at the minimum setting.
    pub fn text_opacity(&self) -> f32 {
        (self.opacity * 0.9 + 0.1) as f32
    }
}

/// `ChatComponent.AlphaCalculator.timeBased` — how a message fades once chat is
/// unfocused.
///
/// ```java
/// double t = (currentTickTime - message.addedTime()) / 200.0;
/// t = 1.0 - t;
/// t *= 10.0;
/// t = Mth.clamp(t, 0.0, 1.0);
/// t *= t;
/// ```
///
/// The `*10` before the clamp is what makes this a hold-then-fade rather than a
/// 200-tick ramp: alpha is a flat 1.0 for the first **180** ticks and falls to
/// 0 over the last **20**, squared. Dropping the `*10` gives a message that is
/// already half-transparent after five seconds.
pub fn alpha_time_based(current_tick: i32, added_time: i32) -> f32 {
    let t = (current_tick - added_time) as f64 / 200.0;
    let t = ((1.0 - t) * 10.0).clamp(0.0, 1.0);
    (t * t) as f32
}

/// One line the renderer should draw, with everything it needs already resolved.
#[derive(Clone, Debug, PartialEq)]
pub struct VisibleLine {
    pub text: String,
    /// The line's index from the bottom of the box, 0 being the bottom row.
    pub index: i32,
    /// `alpha` from the calculator, **before** the text-opacity multiplier.
    pub alpha: f32,
    /// `GuiMessageTag.indicatorColor`, when the line's message carries a tag.
    pub tag_color: Option<u32>,
    pub end_of_entry: bool,
}

/// `ChatComponent` — the message store, the scroll, and the deletion queue.
#[derive(Clone, Debug, Default)]
pub struct ChatComponent {
    /// `allMessages`, newest first.
    all_messages: Vec<GuiMessage>,
    /// `trimmedMessages`, newest line first.
    trimmed_messages: Vec<GuiMessageLine>,
    /// `chatScrollbarPos`.
    scroll: i32,
    /// `newMessageSinceScroll` — drives the scrollbar's colour, and is cleared
    /// by scrolling back to the bottom rather than by reading a message.
    new_message_since_scroll: bool,
    /// `recentChat` — the send history the chat screen's up/down arrows walk.
    /// Capped at 100 and **deduplicated only against the immediately previous
    /// entry**.
    recent_chat: Vec<String>,
    /// `messageDeletionQueue`, as `(signature, deletableAfter)`.
    deletion_queue: Vec<(Box<Signature>, i32)>,
}

impl ChatComponent {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn all_messages(&self) -> &[GuiMessage] {
        &self.all_messages
    }

    pub fn trimmed_messages(&self) -> &[GuiMessageLine] {
        &self.trimmed_messages
    }

    pub fn scroll(&self) -> i32 {
        self.scroll
    }

    pub fn new_message_since_scroll(&self) -> bool {
        self.new_message_since_scroll
    }

    pub fn recent_chat(&self) -> &[String] {
        &self.recent_chat
    }

    pub fn deletion_queue_len(&self) -> usize {
        self.deletion_queue.len()
    }

    /// `ChatComponent.addMessage` — the one entry point all four public adders
    /// funnel through.
    ///
    /// `wrap` measures and wraps; the caller owns the font, which is why it is
    /// passed rather than stored.
    pub fn add_message(&mut self, message: GuiMessage, ctx: &WrapContext<'_>) {
        self.add_message_to_display_queue(&message, ctx);
        self.add_message_to_queue(message);
    }

    /// `addMessageToDisplayQueue`.
    fn add_message_to_display_queue(&mut self, message: &GuiMessage, ctx: &WrapContext<'_>) {
        let max_width = (ctx.options.width() as f64 / ctx.options.scale).floor() as i32;
        let lines = wrap_components(&message.content, max_width, ctx.width_of);
        let last = lines.len() - 1;
        for (i, text) in lines.into_iter().enumerate() {
            // Scrolling up and then receiving a message keeps the view pinned:
            // vanilla scrolls by one per *line*, not per message, and only
            // while the chat screen is open.
            if ctx.focused && self.scroll > 0 {
                self.new_message_since_scroll = true;
                self.scroll_chat(1, ctx);
            }
            self.trimmed_messages.insert(
                0,
                GuiMessageLine {
                    text,
                    added_time: message.added_time,
                    end_of_entry: i == last,
                    tag: message.tag,
                },
            );
        }
        while self.trimmed_messages.len() > MAX_CHAT_HISTORY {
            self.trimmed_messages.pop();
        }
    }

    /// `addMessageToQueue`.
    fn add_message_to_queue(&mut self, message: GuiMessage) {
        self.all_messages.insert(0, message);
        while self.all_messages.len() > MAX_CHAT_HISTORY {
            self.all_messages.pop();
        }
    }

    /// `refreshTrimmedMessages` — replay `allMessages` **oldest first** through
    /// the wrap. Called after a rescale and after a deletion swaps in a marker.
    pub fn refresh_trimmed_messages(&mut self, ctx: &WrapContext<'_>) {
        self.trimmed_messages.clear();
        let messages = std::mem::take(&mut self.all_messages);
        for message in messages.iter().rev() {
            self.add_message_to_display_queue(message, ctx);
        }
        self.all_messages = messages;
    }

    /// `ChatComponent.rescaleChat`.
    pub fn rescale_chat(&mut self, ctx: &WrapContext<'_>) {
        self.reset_chat_scroll();
        self.refresh_trimmed_messages(ctx);
    }

    /// `ChatComponent.clearMessages`.
    pub fn clear_messages(&mut self, history: bool) {
        self.deletion_queue.clear();
        self.trimmed_messages.clear();
        self.all_messages.clear();
        if history {
            self.recent_chat.clear();
        }
    }

    /// `ChatComponent.addRecentChat`.
    ///
    /// The dedup is against `peekLast()` **only** — repeating a message you
    /// sent three messages ago adds it again. The cap is checked before the
    /// push and drops from the *front*.
    pub fn add_recent_chat(&mut self, message: &str) {
        if self.recent_chat.last().map(String::as_str) != Some(message) {
            if self.recent_chat.len() >= MAX_CHAT_HISTORY {
                self.recent_chat.remove(0);
            }
            self.recent_chat.push(message.to_string());
        }
    }

    /// `ChatComponent.resetChatScroll`.
    pub fn reset_chat_scroll(&mut self) {
        self.scroll = 0;
        self.new_message_since_scroll = false;
    }

    /// `ChatComponent.scrollChat`.
    ///
    /// **The two clamps are ordered and the order matters.** The top clamp runs
    /// first and can drive the position negative when there are fewer lines
    /// than fit on a page; the bottom clamp then pulls it back to 0. Swapping
    /// them leaves a negative scroll, which indexes before the start of the
    /// line list.
    pub fn scroll_chat(&mut self, dir: i32, ctx: &WrapContext<'_>) {
        self.scroll += dir;
        let max = self.trimmed_messages.len() as i32;
        let per_page = ctx.options.lines_per_page(ctx.focused);
        if self.scroll > max - per_page {
            self.scroll = max - per_page;
        }
        if self.scroll <= 0 {
            self.scroll = 0;
            self.new_message_since_scroll = false;
        }
    }

    /// `ChatComponent.tick` → `processMessageDeletionQueue`.
    ///
    /// An entry is dropped from the queue only once it is both due *and*
    /// actually applied — `removeIf(e -> time >= e.deletableAfter() ? delete(e) == null : false)`,
    /// where `delete` returns null when it either applied the marker or found
    /// no such message. A not-yet-due entry stays.
    pub fn tick(&mut self, current_tick: i32, ctx: &WrapContext<'_>) {
        if self.deletion_queue.is_empty() {
            return;
        }
        let queue = std::mem::take(&mut self.deletion_queue);
        let mut kept = Vec::new();
        for (signature, deletable_after) in queue {
            if current_tick >= deletable_after {
                if let Some(entry) = self.delete_message_or_delay(&signature, current_tick, ctx) {
                    kept.push(entry);
                }
            } else {
                kept.push((signature, deletable_after));
            }
        }
        self.deletion_queue = kept;
    }

    /// `ChatComponent.deleteMessage` — the `delete_chat` packet's consumer.
    pub fn delete_message(
        &mut self,
        signature: &Signature,
        current_tick: i32,
        ctx: &WrapContext<'_>,
    ) {
        if let Some(entry) = self.delete_message_or_delay(signature, current_tick, ctx) {
            self.deletion_queue.push(entry);
        }
    }

    /// `deleteMessageOrDelay` — replace the message with the deleted marker, or
    /// return the queue entry to retry once it is old enough.
    ///
    /// **A message younger than 60 ticks is not deleted now**, it is delayed —
    /// so a server that redacts a message the instant it sends it still shows
    /// it for three seconds. That is deliberate in vanilla (it stops a
    /// redaction from being a way to flash text past the reader) and reading it
    /// as "delete, but no earlier than 60 ticks after arrival" gets the same
    /// code for a different reason.
    fn delete_message_or_delay(
        &mut self,
        signature: &Signature,
        current_tick: i32,
        ctx: &WrapContext<'_>,
    ) -> Option<(Box<Signature>, i32)> {
        let found = self.all_messages.iter().position(|m| {
            m.signature
                .as_deref()
                .is_some_and(|s| s.as_slice() == signature.as_slice())
        })?;
        let deletable_after = self.all_messages[found].added_time + TIME_BEFORE_MESSAGE_DELETION;
        if current_tick >= deletable_after {
            self.all_messages[found] = deleted_marker(&self.all_messages[found], ctx);
            self.refresh_trimmed_messages(ctx);
            return None;
        }
        Some((Box::new(*signature), deletable_after))
    }

    /// `forEachLine` — the lines the renderer should draw, **top row first**.
    ///
    /// The loop counts *down* (`for (int i = min(size - scroll, perPage) - 1; i >= 0; i--)`)
    /// and the row's y is `chatBottom - i * entryHeight`, so the largest index
    /// is the highest row and the emission order is top to bottom. That is not
    /// cosmetic: the tag-icon pass accumulates `hoveredOverCurrentMessage`
    /// across a message's lines and resets it on `endOfEntry`, which is on the
    /// line at index 0 — the *bottom* one — so a bottom-first walk would clear
    /// the accumulator before filling it.
    ///
    /// A line whose alpha is `<= 1.0E-5` is **skipped and not counted**, and
    /// the count feeds the scrollbar's height and the restricted-prompt row, so
    /// a faded-out line shortens the scrollbar rather than leaving a gap.
    pub fn visible_lines(
        &self,
        current_tick: i32,
        focused: bool,
        options: &ChatOptions,
    ) -> Vec<VisibleLine> {
        let per_page = options.lines_per_page(focused);
        let available = self.trimmed_messages.len() as i32 - self.scroll;
        let mut out = Vec::new();
        for i in (0..available.min(per_page)).rev() {
            let line = &self.trimmed_messages[(i + self.scroll) as usize];
            let alpha = if focused {
                1.0
            } else {
                alpha_time_based(current_tick, line.added_time)
            };
            if alpha > 1.0e-5 {
                out.push(VisibleLine {
                    text: line.text.clone(),
                    index: i,
                    alpha,
                    tag_color: line.tag.map(|t| t.indicator_color),
                    end_of_entry: line.end_of_entry,
                });
            }
        }
        out
    }
}

/// `createDeletedMarker` — the replacement is a **system** message with no
/// signature, so it cannot itself be deleted, and it keeps the original's
/// `addedTime` so it fades on the original's clock rather than restarting it.
fn deleted_marker(message: &GuiMessage, ctx: &WrapContext<'_>) -> GuiMessage {
    GuiMessage {
        added_time: message.added_time,
        content: ctx.deleted_marker_text.to_string(),
        signature: None,
        source: MessageSource::SystemServer,
        tag: Some(MessageTag::SYSTEM),
    }
}

/// Everything the store needs from outside itself to wrap a message: the font,
/// the options, whether the chat screen is open, and the one translated string
/// it can produce on its own.
pub struct WrapContext<'a> {
    pub options: ChatOptions,
    /// `ChatComponent.isChatFocused()` — whether a `ChatScreen` is up. It
    /// changes the box height *and* whether an arriving message scrolls.
    pub focused: bool,
    pub width_of: &'a dyn Fn(&str) -> i32,
    /// `chat.deleted_marker`, resolved by the caller from the language map.
    pub deleted_marker_text: &'a str,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w6(s: &str) -> i32 {
        s.chars().count() as i32 * 6
    }

    fn ctx(focused: bool) -> WrapContext<'static> {
        WrapContext {
            options: ChatOptions::default(),
            focused,
            width_of: &w6,
            deleted_marker_text: "This chat message has been deleted by the server.",
        }
    }

    fn msg(added_time: i32, content: &str) -> GuiMessage {
        GuiMessage {
            added_time,
            content: content.into(),
            signature: None,
            source: MessageSource::SystemServer,
            tag: None,
        }
    }

    fn signed(added_time: i32, content: &str, byte: u8) -> GuiMessage {
        GuiMessage {
            signature: Some(Box::new([byte; 256])),
            source: MessageSource::Player,
            ..msg(added_time, content)
        }
    }

    // ── wrap_components ──────────────────────────────────────────────────

    #[test]
    fn a_width_wrapped_continuation_gains_a_leading_space() {
        assert_eq!(wrap_components("abc def", 18, &w6), vec!["abc", " def"]);
    }

    #[test]
    fn a_line_after_an_explicit_newline_does_not() {
        // The distinction the `isWrapped = !isNewLine` rule exists to make.
        assert_eq!(wrap_components("abc\ndef", 600, &w6), vec!["abc", "def"]);
    }

    #[test]
    fn the_indent_is_not_charged_against_max_width() {
        // Applied after wrapping, so a continuation is one space wider than the
        // box. `" def"` is 24 px against a max of 18.
        let lines = wrap_components("abc def", 18, &w6);
        assert_eq!(w6(&lines[1]), 24);
    }

    #[test]
    fn an_empty_message_is_one_empty_line() {
        assert_eq!(wrap_components("", 600, &w6), vec![String::new()]);
    }

    // ── geometry ─────────────────────────────────────────────────────────

    #[test]
    fn the_box_dimensions_are_the_expressions_not_the_declared_bounds() {
        // `getWidth`/`getHeight` declare max/min locals and use neither.
        assert_eq!(width_of_pct(0.0), 40);
        assert_eq!(width_of_pct(1.0), 320);
        assert_eq!(height_of_pct(0.0), 20);
        assert_eq!(height_of_pct(1.0), 180);
        // Outside 0..=1 the expression keeps going where a clamp would not —
        // this is what distinguishes the two readings.
        assert_eq!(width_of_pct(2.0), 600);
    }

    #[test]
    fn the_default_unfocused_box_is_ninety_pixels() {
        assert_eq!(height_of_pct(default_unfocused_pct()), 90);
    }

    #[test]
    fn the_focused_and_unfocused_heights_differ_by_default() {
        let o = ChatOptions::default();
        assert_eq!(o.height(true), 180);
        assert_eq!(o.height(false), 90);
        assert_eq!(o.lines_per_page(true), 20);
        assert_eq!(o.lines_per_page(false), 10);
    }

    #[test]
    fn entry_bottom_to_message_y_moves_opposite_to_the_row_height() {
        let mut o = ChatOptions::default();
        assert_eq!(o.entry_height(), 9);
        assert_eq!(o.entry_bottom_to_message_y(), 8);
        // At spacing 1 the row doubles to 18 but the text baseline offset goes
        // to 12, not 16 — `8*(s+1) - 4*s`. A reading of `8*(s+1)` alone, or of
        // `entryHeight - 1`, gives 16 and 17 respectively.
        o.line_spacing = 1.0;
        assert_eq!(o.entry_height(), 18);
        assert_eq!(o.entry_bottom_to_message_y(), 12);
    }

    #[test]
    fn text_opacity_has_a_floor_of_a_tenth() {
        let mut o = ChatOptions::default();
        assert_eq!(o.text_opacity(), 1.0);
        o.opacity = 0.0;
        assert!((o.text_opacity() - 0.1).abs() < 1e-6);
    }

    // ── the fade ─────────────────────────────────────────────────────────

    #[test]
    fn a_message_holds_full_alpha_for_one_hundred_and_eighty_ticks() {
        assert_eq!(alpha_time_based(0, 0), 1.0);
        assert_eq!(alpha_time_based(180, 0), 1.0);
        // The `* 10.0` before the clamp is what produces the hold; without it
        // this sample would be 0.1 rather than 1.0.
        assert!(alpha_time_based(181, 0) < 1.0);
    }

    #[test]
    fn a_message_is_gone_at_two_hundred_ticks() {
        assert_eq!(alpha_time_based(200, 0), 0.0);
        assert_eq!(alpha_time_based(10_000, 0), 0.0);
    }

    #[test]
    fn the_fade_is_squared() {
        // Half way through the 20-tick fade the linear term is 0.5, so alpha is
        // 0.25 rather than 0.5.
        assert!((alpha_time_based(190, 0) - 0.25).abs() < 1e-5);
    }

    // ── the store ────────────────────────────────────────────────────────

    #[test]
    fn a_multi_line_message_stores_its_last_line_nearest_the_bottom() {
        let mut c = ChatComponent::new();
        // 320-px box, 6-px font -> 53 characters per line.
        c.add_message(msg(0, &format!("{}{}", "a".repeat(53), "bbb")), &ctx(false));
        let lines = c.trimmed_messages();
        assert_eq!(lines.len(), 2);
        // Index 0 is the bottom row and is the message's *second* line.
        assert_eq!(lines[0].text, " bbb");
        assert!(lines[0].end_of_entry);
        assert_eq!(lines[1].text, "a".repeat(53));
        assert!(!lines[1].end_of_entry);
    }

    #[test]
    fn the_two_hundred_caps_are_independent() {
        let mut c = ChatComponent::new();
        // Each message wraps to two lines, so the line list hits its cap after
        // 50 messages while the message list is only half full.
        for i in 0..80 {
            c.add_message(msg(i, &format!("{}{}", "a".repeat(53), "bbb")), &ctx(false));
        }
        assert_eq!(c.all_messages().len(), 80);
        assert_eq!(c.trimmed_messages().len(), MAX_CHAT_HISTORY);
    }

    #[test]
    fn visible_lines_are_emitted_top_row_first_and_indexed_from_the_bottom() {
        // `forEachLine` counts *down*, and the row's y is
        // `chatBottom - i * entryHeight`, so the first line emitted carries the
        // largest index and sits highest. Both ends are asserted because
        // checking one alone cannot see a reversed walk.
        let mut c = ChatComponent::new();
        c.add_message(msg(0, "oldest"), &ctx(false));
        c.add_message(msg(0, "newest"), &ctx(false));
        let v = c.visible_lines(0, false, &ChatOptions::default());
        assert_eq!((v[0].text.as_str(), v[0].index), ("oldest", 1));
        assert_eq!((v[1].text.as_str(), v[1].index), ("newest", 0));
    }

    #[test]
    fn a_faded_line_is_skipped_and_uncounted() {
        let mut c = ChatComponent::new();
        c.add_message(msg(0, "old"), &ctx(false));
        c.add_message(msg(300, "new"), &ctx(false));
        // At tick 300 the first message is 300 ticks old and fully faded.
        let v = c.visible_lines(300, false, &ChatOptions::default());
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].text, "new");
        // Focused chat ignores the clock entirely.
        assert_eq!(c.visible_lines(300, true, &ChatOptions::default()).len(), 2);
    }

    #[test]
    fn only_lines_per_page_are_shown() {
        let mut c = ChatComponent::new();
        for i in 0..30 {
            c.add_message(msg(0, &format!("m{i}")), &ctx(false));
        }
        // Unfocused: 90/9 = 10 rows.
        assert_eq!(c.visible_lines(0, false, &ChatOptions::default()).len(), 10);
        // Focused: 180/9 = 20.
        assert_eq!(c.visible_lines(0, true, &ChatOptions::default()).len(), 20);
    }

    // ── scroll ───────────────────────────────────────────────────────────

    #[test]
    fn scrolling_up_shows_older_lines() {
        let mut c = ChatComponent::new();
        for i in 0..30 {
            c.add_message(msg(0, &format!("m{i}")), &ctx(false));
        }
        c.scroll_chat(5, &ctx(false));
        assert_eq!(c.scroll(), 5);
        // 25 lines remain above the scroll, ten fit: indices 14 (top) down to
        // 5 (bottom). Asserting both ends pins the window's position *and* its
        // direction; the top alone would survive an off-by-`perPage`.
        let v = c.visible_lines(0, false, &ChatOptions::default());
        assert_eq!(v.first().unwrap().text, "m15");
        assert_eq!(v.last().unwrap().text, "m24");
    }

    #[test]
    fn the_top_clamp_runs_before_the_bottom_one() {
        let mut c = ChatComponent::new();
        // Three lines against a ten-line page: `max - perPage` is -7, so the
        // first clamp drives the position negative and the second rescues it.
        // Swapping the two leaves -7, which would index before the list.
        for i in 0..3 {
            c.add_message(msg(0, &format!("m{i}")), &ctx(false));
        }
        c.scroll_chat(1, &ctx(false));
        assert_eq!(c.scroll(), 0);
    }

    #[test]
    fn scrolling_back_to_the_bottom_clears_the_new_message_flag() {
        let mut c = ChatComponent::new();
        for i in 0..30 {
            c.add_message(msg(0, &format!("m{i}")), &ctx(false));
        }
        c.scroll_chat(5, &ctx(true));
        // A message arriving while scrolled *and focused* pushes the view.
        c.add_message(msg(0, "arrival"), &ctx(true));
        assert!(c.new_message_since_scroll());
        assert_eq!(c.scroll(), 6);
        c.scroll_chat(-6, &ctx(true));
        assert_eq!(c.scroll(), 0);
        assert!(!c.new_message_since_scroll());
    }

    #[test]
    fn an_arriving_message_does_not_push_an_unfocused_view() {
        let mut c = ChatComponent::new();
        for i in 0..30 {
            c.add_message(msg(0, &format!("m{i}")), &ctx(false));
        }
        c.scroll_chat(5, &ctx(false));
        c.add_message(msg(0, "arrival"), &ctx(false));
        assert_eq!(c.scroll(), 5);
        assert!(!c.new_message_since_scroll());
    }

    // ── recent chat ──────────────────────────────────────────────────────

    #[test]
    fn recent_chat_dedups_only_against_the_previous_entry() {
        let mut c = ChatComponent::new();
        c.add_recent_chat("a");
        c.add_recent_chat("a");
        assert_eq!(c.recent_chat(), ["a"]);
        c.add_recent_chat("b");
        c.add_recent_chat("a");
        // "a" is back, because the dedup looks at `peekLast()` and nothing else.
        assert_eq!(c.recent_chat(), ["a", "b", "a"]);
    }

    #[test]
    fn recent_chat_drops_from_the_front_at_the_cap() {
        let mut c = ChatComponent::new();
        for i in 0..120 {
            c.add_recent_chat(&format!("m{i}"));
        }
        assert_eq!(c.recent_chat().len(), MAX_CHAT_HISTORY);
        assert_eq!(c.recent_chat()[0], "m20");
        assert_eq!(c.recent_chat()[99], "m119");
    }

    // ── deletion ─────────────────────────────────────────────────────────

    #[test]
    fn an_old_message_is_replaced_by_the_marker_immediately() {
        let mut c = ChatComponent::new();
        c.add_message(signed(0, "secret", 7), &ctx(false));
        c.delete_message(&[7u8; 256], 100, &ctx(false));
        assert_eq!(c.deletion_queue_len(), 0);
        assert!(c.all_messages()[0].content.contains("deleted"));
        assert!(c.all_messages()[0].signature.is_none());
        assert_eq!(c.trimmed_messages()[0].text, ctx(false).deleted_marker_text);
    }

    #[test]
    fn a_young_message_is_queued_rather_than_deleted() {
        let mut c = ChatComponent::new();
        c.add_message(signed(0, "secret", 7), &ctx(false));
        // 59 ticks old: below the 60-tick threshold.
        c.delete_message(&[7u8; 256], 59, &ctx(false));
        assert_eq!(c.deletion_queue_len(), 1);
        assert_eq!(c.all_messages()[0].content, "secret");
        // Not yet due.
        c.tick(59, &ctx(false));
        assert_eq!(c.deletion_queue_len(), 1);
        assert_eq!(c.all_messages()[0].content, "secret");
        // Due exactly at addedTime + 60.
        c.tick(60, &ctx(false));
        assert_eq!(c.deletion_queue_len(), 0);
        assert!(c.all_messages()[0].content.contains("deleted"));
    }

    #[test]
    fn the_marker_keeps_the_originals_clock() {
        let mut c = ChatComponent::new();
        c.add_message(signed(5, "secret", 7), &ctx(false));
        c.delete_message(&[7u8; 256], 100, &ctx(false));
        // Not 100: a redacted message must not get a fresh 10 seconds on screen.
        assert_eq!(c.all_messages()[0].added_time, 5);
        assert_eq!(c.trimmed_messages()[0].added_time, 5);
    }

    #[test]
    fn deleting_an_unknown_signature_does_nothing_and_queues_nothing() {
        let mut c = ChatComponent::new();
        c.add_message(signed(0, "kept", 7), &ctx(false));
        c.delete_message(&[9u8; 256], 0, &ctx(false));
        assert_eq!(c.deletion_queue_len(), 0);
        assert_eq!(c.all_messages()[0].content, "kept");
    }

    #[test]
    fn an_unsigned_message_is_never_matched() {
        let mut c = ChatComponent::new();
        // A system message has `signature: None`; a zero-filled signature must
        // not match it.
        c.add_message(msg(0, "system"), &ctx(false));
        c.delete_message(&[0u8; 256], 100, &ctx(false));
        assert_eq!(c.all_messages()[0].content, "system");
    }

    #[test]
    fn only_the_first_match_is_replaced() {
        // `listIterator` returns at the first hit, so a duplicate signature
        // leaves the older copy alone.
        let mut c = ChatComponent::new();
        c.add_message(signed(0, "one", 7), &ctx(false));
        c.add_message(signed(0, "two", 7), &ctx(false));
        c.delete_message(&[7u8; 256], 100, &ctx(false));
        // Newest first, so index 0 is "two".
        assert!(c.all_messages()[0].content.contains("deleted"));
        assert_eq!(c.all_messages()[1].content, "one");
    }

    #[test]
    fn refresh_replays_oldest_first_so_the_order_survives() {
        let mut c = ChatComponent::new();
        c.add_message(msg(0, "first"), &ctx(false));
        c.add_message(msg(0, "second"), &ctx(false));
        c.refresh_trimmed_messages(&ctx(false));
        assert_eq!(c.trimmed_messages()[0].text, "second");
        assert_eq!(c.trimmed_messages()[1].text, "first");
    }

    #[test]
    fn clear_messages_keeps_the_send_history_unless_asked() {
        let mut c = ChatComponent::new();
        c.add_message(msg(0, "m"), &ctx(false));
        c.add_recent_chat("/help");
        c.clear_messages(false);
        assert!(c.all_messages().is_empty());
        assert_eq!(c.recent_chat(), ["/help"]);
        c.clear_messages(true);
        assert!(c.recent_chat().is_empty());
    }
}
