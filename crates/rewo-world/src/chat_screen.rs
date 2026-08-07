//! `ChatScreen` (M110) — the box you type into.
//!
//! M108 built the chat store and M109 its backdrop; both are read-only. This
//! is the half that writes: opening on `T` or `/`, an [`crate::edit_box`] at
//! the bottom, the send-history the arrow keys walk, the draft that survives a
//! close, and the scroll — which is also what makes M109's *scrollbar*
//! reachable, since `ChatComponent.extractRenderState` gates it on
//! `isForeground` and only this screen passes `DisplayMode.FOREGROUND`.
//!
//! # `normalizeChatMessage` collapses internal whitespace
//!
//! ```java
//! return StringUtil.trimChatMessage(StringUtils.normalizeSpace(message.trim()));
//! ```
//!
//! `StringUtils.normalizeSpace` is Apache Commons and does **more than trim**:
//! it replaces every run of whitespace *inside* the string with a single
//! space. `"a     b"` is sent as `"a b"`. A `.trim()` alone — the obvious
//! reading, and the one the method name suggests — leaves the run intact, and
//! the difference is invisible on every message that has no double space in
//! it, which is most of them. `trimChatMessage` then truncates to 256.
//!
//! # The history has one slot past its end
//!
//! `historyPos` starts at `recentChat.size()`, one past the last entry, and
//! that slot means "what you were typing". Moving off it **saves the input**
//! into `historyBuffer`; moving back to it restores that rather than the last
//! sent message. Modelling the position as an index into the list loses the
//! buffer, and the symptom is that pressing Down after Up eats a
//! half-composed message.
//!
//! # A draft is not just remembered text
//!
//! `ChatComponent.createScreen` restores a draft only when the method allows
//! it: `MESSAGE` takes any draft, `COMMAND` takes only a command draft — so
//! pressing `/` after starting an ordinary message gives you a fresh `/`
//! rather than your half-typed sentence with a slash bolted on. While a
//! restored draft is untouched it renders **grey and italic**, and
//! **backspace clears the whole field** rather than deleting one character.
//! Both stop at the first edit.
//!
//! # What is deliberately not here
//!
//! **`CommandSuggestions`.** It needs the `commands` packet (the Brigadier
//! tree), which is class C in `REWO_PACKET_COVERAGE.md`. Vanilla's own
//! `init` calls `setAllowSuggestions(false)` and only `onEdited` turns it on,
//! so a screen that never suggests is a state vanilla itself passes through —
//! but the four `keyPressed` / `mouseScrolled` / `mouseClicked` early-outs it
//! owns are recorded at their call sites rather than silently absent.
//!
//! **Clickable chat text.** `mouseClicked` runs a `ClickableStyleFinder` over
//! the rendered lines; Rewo flattens a component to plain text at the wire, so
//! there is no `Style` left to find. That is the same limitation
//! [`crate::chat`] records for the decoration.
//!
//! **Chat abilities and the restricted prompt.** `ChatAbilities` arrives on a
//! packet Rewo does not decode, so `displayMode` is always `FOREGROUND` and
//! never `FOREGROUND_RESTRICTED`.

use crate::edit_box::{key, EditBox, Input};

/// `ChatScreen.MOUSE_SCROLL_SPEED`.
pub const MOUSE_SCROLL_SPEED: f64 = 7.0;

/// `input.setMaxLength(256)`, and also `trimChatMessage`'s cap.
pub const MAX_CHAT_LENGTH: usize = 256;

/// `ChatComponent.ChatMethod` — which key opened the screen.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ChatMethod {
    /// `T` — prefix `""`.
    Message,
    /// `/` — prefix `"/"`.
    Command,
}

impl ChatMethod {
    pub fn prefix(self) -> &'static str {
        match self {
            Self::Message => "",
            Self::Command => "/",
        }
    }

    /// `isDraftRestorable` — **asymmetric on purpose**. `MESSAGE` returns a
    /// bare `true`, so `T` restores any draft including a half-typed command.
    /// `COMMAND` returns `this == draft.chatMethod`, so `/` restores only a
    /// command draft and otherwise starts fresh — which is what stops `/`
    /// producing `"/"` prepended to an unrelated sentence.
    pub fn is_draft_restorable(self, draft: &Draft) -> bool {
        match self {
            Self::Message => true,
            Self::Command => self == draft.method,
        }
    }
}

/// `ChatComponent.Draft`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Draft {
    pub text: String,
    pub method: ChatMethod,
}

impl Draft {
    /// `saveAsDraft` — the method is derived from the text, not from how the
    /// screen was opened. Typing `/foo` into a screen opened with `T` saves a
    /// COMMAND draft.
    pub fn of(text: &str) -> Self {
        Self {
            text: text.to_string(),
            method: if text.starts_with('/') {
                ChatMethod::Command
            } else {
                ChatMethod::Message
            },
        }
    }
}

/// `ChatScreen.ExitReason`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ExitReason {
    /// `onClose` — Esc.
    Intentional,
    /// The field's initial value: the screen went away without being closed
    /// (a disconnect, a dimension change, the window losing the world).
    Interrupted,
    /// A message was submitted.
    Done,
}

/// What a key or wheel event asks the caller to do.
///
/// The screen owns no socket and no chat store, so every effect leaves as a
/// value — the seam M97's lesson keeps producing, and the reason every rule
/// below is reachable from a test.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChatAction {
    /// The event was consumed and changed nothing the caller must act on.
    None,
    /// The event was not the screen's — let it fall through.
    NotHandled,
    /// `connection.sendChat(msg)`.
    Send(String),
    /// `connection.sendCommand(msg)` — **without** the leading slash, which
    /// `handleChatInput` strips with `substring(1)`.
    Command(String),
    /// `ChatComponent.scrollChat(n)`.
    Scroll(i32),
    /// `minecraft.gui.setScreen(null)`.
    Close,
}

/// The screen.
pub struct ChatScreen {
    pub input: EditBox,
    /// `isDraft` — set while a restored draft is untouched. Drives the grey
    /// italic rendering and backspace's clear-everything behaviour.
    is_draft: bool,
    /// `historyBuffer` — what was being typed before the arrows walked away.
    history_buffer: String,
    /// `historyPos`, in `0..=recent.len()`. The last slot is the buffer.
    history_pos: usize,
    exit_reason: ExitReason,
    /// `closeOnSubmit` — false only for the (unshipped) command-block screens.
    close_on_submit: bool,
}

impl ChatScreen {
    /// `ChatComponent.createScreen` + `ChatScreen.init`.
    ///
    /// The draft is consulted through [`ChatMethod::is_draft_restorable`]; when
    /// it does not apply the initial text is the method's bare prefix.
    pub fn open(method: ChatMethod, draft: Option<&Draft>, recent_len: usize) -> Self {
        let (initial, is_draft) = match draft {
            Some(d) if method.is_draft_restorable(d) => (d.text.clone(), true),
            _ => (method.prefix().to_string(), false),
        };
        let mut input = EditBox::new(MAX_CHAT_LENGTH);
        input.set_value(&initial);
        // `setCanLoseFocus(false)`, so the caret never stops blinking while
        // the screen is up — the same call the anvil's field makes and the
        // reason M101's blink rule has two consumers.
        input.set_can_lose_focus(false);
        input.set_focused(true);
        Self {
            input,
            is_draft,
            history_buffer: String::new(),
            history_pos: recent_len,
            exit_reason: ExitReason::Interrupted,
            close_on_submit: true,
        }
    }

    pub fn is_draft(&self) -> bool {
        self.is_draft
    }

    pub fn exit_reason(&self) -> ExitReason {
        self.exit_reason
    }

    pub fn history_pos(&self) -> usize {
        self.history_pos
    }

    /// `onEdited` — any edit stops the text being a draft.
    fn on_edited(&mut self) {
        self.is_draft = false;
    }

    /// `ChatScreen.keyPressed`.
    ///
    /// The order is a contract. `commandSuggestions.keyPressed` would come
    /// first (absent here — see the module docs); then the **draft
    /// backspace**, which clears the whole field and returns *before*
    /// `super.keyPressed` ever reaches the `EditBox`; then the edit box; then
    /// confirmation; then the four navigation keys.
    ///
    /// Putting the draft backspace after the edit box would delete one
    /// character instead of the line, which is the behaviour a reader expects
    /// and not the one vanilla has.
    pub fn key_pressed(
        &mut self,
        input: Input,
        clipboard: &mut String,
        recent: &[String],
        lines_per_page: i32,
    ) -> ChatAction {
        if self.is_draft && input.key == key::BACKSPACE {
            self.input.set_value("");
            self.is_draft = false;
            return ChatAction::None;
        }
        let before = self.input.value();
        if self.input.key_pressed(input, clipboard) {
            if self.input.value() != before {
                self.on_edited();
            }
            return ChatAction::None;
        }
        if is_confirmation(input.key) {
            let msg = normalize_chat_message(&self.input.value());
            let action = if msg.is_empty() {
                ChatAction::None
            } else if let Some(cmd) = msg.strip_prefix('/') {
                ChatAction::Command(cmd.to_string())
            } else {
                ChatAction::Send(msg)
            };
            if self.close_on_submit {
                self.exit_reason = ExitReason::Done;
            } else {
                self.input.set_value("");
            }
            return action;
        }
        match input.key {
            KEY_DOWN => {
                self.move_in_history(1, recent);
                ChatAction::None
            }
            KEY_UP => {
                self.move_in_history(-1, recent);
                ChatAction::None
            }
            // `scrollChat(linesPerPage - 1)` — a page MINUS ONE, so a
            // page-scroll keeps one line of overlap and you never lose the
            // line you were reading.
            KEY_PAGE_UP => ChatAction::Scroll(lines_per_page - 1),
            KEY_PAGE_DOWN => ChatAction::Scroll(-lines_per_page + 1),
            _ => ChatAction::NotHandled,
        }
    }

    /// `charTyped` — a printable character.
    pub fn char_typed(&mut self, ch: char) -> bool {
        let before = self.input.value();
        let handled = self.input.char_typed(ch);
        if handled && self.input.value() != before {
            self.on_edited();
        }
        handled
    }

    /// `ChatScreen.mouseScrolled`.
    ///
    /// **The clamp comes first**, then the multiply — so one wheel notch is
    /// seven lines and a high-resolution trackpad reporting 4.0 still moves
    /// seven, not twenty-eight. Shift holds it to one line. Multiplying before
    /// clamping would make the speed a property of the input device.
    pub fn mouse_scrolled(&self, scroll_y: f64, shift: bool) -> ChatAction {
        let mut dy = scroll_y.clamp(-1.0, 1.0);
        if !shift {
            dy *= MOUSE_SCROLL_SPEED;
        }
        ChatAction::Scroll(dy as i32)
    }

    /// `onClose` — Esc.
    pub fn close(&mut self) {
        self.exit_reason = ExitReason::Intentional;
    }

    /// `ChatScreen.moveInHistory`.
    fn move_in_history(&mut self, dir: i32, recent: &[String]) {
        let max = recent.len();
        let new_pos = (self.history_pos as i32 + dir).clamp(0, max as i32) as usize;
        if new_pos == self.history_pos {
            return;
        }
        if new_pos == max {
            self.history_pos = max;
            let buffered = self.history_buffer.clone();
            self.input.set_value(&buffered);
        } else {
            // Leaving the live slot saves what was there — and only then, so
            // walking further up the list does not overwrite it.
            if self.history_pos == max {
                self.history_buffer = self.input.value();
            }
            self.input.set_value(&recent[new_pos]);
            self.history_pos = new_pos;
        }
    }

    /// `ChatScreen.removed` — what the store should do with the draft.
    ///
    /// `shouldDiscardDraft` is
    /// `exitReason != INTERRUPTED && (exitReason != INTENTIONAL || !saveChatDrafts)`,
    /// so with drafts enabled only a **submitted** message discards one; Esc
    /// keeps it, and so does an interruption. Reading it as "Esc throws the
    /// draft away" — which the name suggests — loses the text on the one exit
    /// a user takes deliberately.
    pub fn removed(&self, save_chat_drafts: bool) -> DraftOutcome {
        let text = self.input.value();
        let should_discard = self.exit_reason != ExitReason::Interrupted
            && (self.exit_reason != ExitReason::Intentional || !save_chat_drafts);
        if should_discard || text.trim().is_empty() {
            DraftOutcome::Discard
        } else if !self.is_draft {
            DraftOutcome::Save(Draft::of(&text))
        } else {
            // A draft that was restored and never edited is left exactly as it
            // was rather than re-saved, which is the `else if` and not an
            // `else`.
            DraftOutcome::Keep
        }
    }
}

/// What [`ChatScreen::removed`] asks of the chat store.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DraftOutcome {
    /// `discardDraft()`.
    Discard,
    /// `saveAsDraft(text)`.
    Save(Draft),
    /// Neither branch ran — the stored draft stands.
    Keep,
}

/// `ChatScreen.normalizeChatMessage`.
///
/// `StringUtils.normalizeSpace(s.trim())` then a 256-char cap. `normalizeSpace`
/// is the half that surprises: it collapses every internal run of whitespace to
/// a single space, so a message is never sent with a double space in it.
pub fn normalize_chat_message(message: &str) -> String {
    let normalized: String = message
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    normalized.chars().take(MAX_CHAT_LENGTH).collect()
}

/// GLFW `KEY_UP`.
pub const KEY_UP: i32 = 265;
/// GLFW `KEY_DOWN`.
pub const KEY_DOWN: i32 = 264;
/// GLFW `KEY_PAGE_UP`.
pub const KEY_PAGE_UP: i32 = 266;
/// GLFW `KEY_PAGE_DOWN`.
pub const KEY_PAGE_DOWN: i32 = 267;
/// GLFW `KEY_ENTER` / `KEY_KP_ENTER` — `KeyEvent.isConfirmation()`.
pub const KEY_ENTER: i32 = 257;
pub const KEY_KP_ENTER: i32 = 335;

/// `KeyEvent.isConfirmation` — Enter **or** keypad Enter. Checking only 257
/// leaves the numpad dead, which is the kind of gap nobody reports.
pub fn is_confirmation(key: i32) -> bool {
    key == KEY_ENTER || key == KEY_KP_ENTER
}

/// The input field's rect, in GUI pixels: `EditBox(font, 4, height - 12, width - 4, 12)`.
///
/// **The width is `width - 4`, not `width - 8`.** The box starts 4 px in from
/// the left and its width reaches the screen's right edge, so it overhangs by
/// exactly the left inset. That is vanilla's, and the backdrop below is the
/// symmetric one.
pub fn input_rect(gui_w: i32, gui_h: i32) -> (i32, i32, i32, i32) {
    (4, gui_h - 12, gui_w - 4, 12)
}

/// The bar behind the input: `fill(2, height - 14, width - 2, height - 2, …)`.
///
/// Two pixels in on **both** sides and two clear of the bottom, so it is
/// symmetric where the field it holds is not. Returns `(x, y, w, h)`.
pub fn input_backdrop_rect(gui_w: i32, gui_h: i32) -> (i32, i32, i32, i32) {
    (2, gui_h - 14, gui_w - 4, 12)
}

/// The input backdrop's alpha, 0..1.
///
/// `getBackgroundColor(Integer.MIN_VALUE)` is
/// `backgroundForChatOnly ? defaultColor : colorFromFloat(textBackgroundOpacity, 0, 0, 0)`,
/// and `backgroundForChatOnly` **defaults to true** — so this bar is a fixed
/// `0x80000000`, alpha 128, and does **not** follow the text-background slider
/// that M109's chat-row backdrops do. Turning "Text Background" to 0 clears the
/// rows behind the chat and leaves this bar exactly as dark as before.
pub const INPUT_BACKDROP_ALPHA: f32 = 128.0 / 255.0;

#[cfg(test)]
mod tests {
    use super::*;

    fn k(key: i32) -> Input {
        Input { key, modifiers: 0 }
    }

    fn screen(recent_len: usize) -> ChatScreen {
        ChatScreen::open(ChatMethod::Message, None, recent_len)
    }

    // ── normalizeChatMessage ─────────────────────────────────────────────

    #[test]
    fn internal_whitespace_runs_collapse_to_one_space() {
        // `StringUtils.normalizeSpace`, which a `.trim()` reading misses.
        assert_eq!(normalize_chat_message("a     b"), "a b");
        assert_eq!(normalize_chat_message("  hello   world  "), "hello world");
        assert_eq!(normalize_chat_message("a\tb\nc"), "a b c");
    }

    #[test]
    fn a_blank_message_normalizes_to_empty() {
        assert_eq!(normalize_chat_message("   "), "");
        assert_eq!(normalize_chat_message(""), "");
    }

    #[test]
    fn the_message_is_capped_at_two_hundred_and_fifty_six() {
        let long = "a".repeat(300);
        assert_eq!(normalize_chat_message(&long).chars().count(), 256);
    }

    // ── opening, and the draft ───────────────────────────────────────────

    #[test]
    fn a_command_screen_starts_with_a_slash_and_a_message_screen_empty() {
        assert_eq!(
            ChatScreen::open(ChatMethod::Command, None, 0).input.value(),
            "/"
        );
        assert_eq!(screen(0).input.value(), "");
    }

    #[test]
    fn a_message_screen_restores_any_draft_and_a_command_screen_only_a_command() {
        let msg_draft = Draft::of("hello there");
        let cmd_draft = Draft::of("/time set");
        // `MESSAGE.isDraftRestorable` is a bare `true`.
        assert_eq!(
            ChatScreen::open(ChatMethod::Message, Some(&msg_draft), 0).input.value(),
            "hello there"
        );
        assert_eq!(
            ChatScreen::open(ChatMethod::Message, Some(&cmd_draft), 0).input.value(),
            "/time set"
        );
        // `COMMAND`'s is `this == draft.chatMethod`, so a message draft is not
        // restored — otherwise `/` would give "/hello there".
        assert_eq!(
            ChatScreen::open(ChatMethod::Command, Some(&msg_draft), 0).input.value(),
            "/"
        );
        assert_eq!(
            ChatScreen::open(ChatMethod::Command, Some(&cmd_draft), 0).input.value(),
            "/time set"
        );
    }

    #[test]
    fn a_drafts_method_comes_from_its_text_not_from_how_it_was_typed() {
        // `saveAsDraft` is `text.startsWith("/") ? COMMAND : MESSAGE`.
        assert_eq!(Draft::of("/give").method, ChatMethod::Command);
        assert_eq!(Draft::of("give").method, ChatMethod::Message);
    }

    #[test]
    fn backspace_on_an_untouched_draft_clears_the_whole_field() {
        let draft = Draft::of("half a sentence");
        let mut s = ChatScreen::open(ChatMethod::Message, Some(&draft), 0);
        assert!(s.is_draft());
        let mut clip = String::new();
        s.key_pressed(k(key::BACKSPACE), &mut clip, &[], 10);
        assert_eq!(s.input.value(), "");
        assert!(!s.is_draft());
    }

    #[test]
    fn backspace_after_an_edit_deletes_one_character() {
        // The draft flag stops at the first edit, and then backspace is the
        // edit box's again.
        let draft = Draft::of("abc");
        let mut s = ChatScreen::open(ChatMethod::Message, Some(&draft), 0);
        let mut clip = String::new();
        s.char_typed('d');
        assert!(!s.is_draft());
        assert_eq!(s.input.value(), "abcd");
        s.key_pressed(k(key::BACKSPACE), &mut clip, &[], 10);
        assert_eq!(s.input.value(), "abc");
    }

    // ── submitting ───────────────────────────────────────────────────────

    #[test]
    fn enter_sends_a_message_and_the_keypad_enter_does_too() {
        for key in [KEY_ENTER, KEY_KP_ENTER] {
            let mut s = screen(0);
            let mut clip = String::new();
            s.input.set_value("hello  world");
            assert_eq!(
                s.key_pressed(k(key), &mut clip, &[], 10),
                // Normalized on the way out.
                ChatAction::Send("hello world".into()),
            );
            assert_eq!(s.exit_reason(), ExitReason::Done);
        }
    }

    #[test]
    fn a_slash_message_becomes_a_command_without_its_slash() {
        // `sendCommand(msg.substring(1))` — sending the slash too would make
        // the server see "//time".
        let mut s = screen(0);
        let mut clip = String::new();
        s.input.set_value("/time set day");
        assert_eq!(
            s.key_pressed(k(KEY_ENTER), &mut clip, &[], 10),
            ChatAction::Command("time set day".into()),
        );
    }

    #[test]
    fn submitting_a_blank_message_sends_nothing_but_still_closes() {
        let mut s = screen(0);
        let mut clip = String::new();
        s.input.set_value("   ");
        assert_eq!(s.key_pressed(k(KEY_ENTER), &mut clip, &[], 10), ChatAction::None);
        // `handleChatInput`'s `if (!msg.isEmpty())` guards the send, not the
        // close — the screen goes away either way.
        assert_eq!(s.exit_reason(), ExitReason::Done);
    }

    // ── history ──────────────────────────────────────────────────────────

    #[test]
    fn up_walks_backwards_through_the_send_history() {
        let recent = vec!["first".to_string(), "second".to_string()];
        let mut s = screen(recent.len());
        let mut clip = String::new();
        assert_eq!(s.history_pos(), 2);
        s.key_pressed(k(KEY_UP), &mut clip, &recent, 10);
        assert_eq!(s.input.value(), "second");
        s.key_pressed(k(KEY_UP), &mut clip, &recent, 10);
        assert_eq!(s.input.value(), "first");
        // …and stops at the top rather than wrapping.
        s.key_pressed(k(KEY_UP), &mut clip, &recent, 10);
        assert_eq!(s.input.value(), "first");
    }

    #[test]
    fn the_slot_past_the_end_holds_what_you_were_typing() {
        // The finding: `historyPos` starts one PAST the list, and that slot is
        // a buffer rather than an entry. Modelling it as an index into the list
        // loses a half-composed message the moment Up is pressed.
        let recent = vec!["earlier".to_string()];
        let mut s = screen(recent.len());
        let mut clip = String::new();
        s.input.set_value("half a thought");
        s.key_pressed(k(KEY_UP), &mut clip, &recent, 10);
        assert_eq!(s.input.value(), "earlier");
        s.key_pressed(k(KEY_DOWN), &mut clip, &recent, 10);
        assert_eq!(s.input.value(), "half a thought");
    }

    #[test]
    fn the_buffer_is_saved_only_when_leaving_the_live_slot() {
        // Walking further up must not overwrite it with an entry.
        let recent = vec!["a".to_string(), "b".to_string()];
        let mut s = screen(recent.len());
        let mut clip = String::new();
        s.input.set_value("mine");
        s.key_pressed(k(KEY_UP), &mut clip, &recent, 10); // -> "b"
        s.key_pressed(k(KEY_UP), &mut clip, &recent, 10); // -> "a"
        s.key_pressed(k(KEY_DOWN), &mut clip, &recent, 10); // -> "b"
        s.key_pressed(k(KEY_DOWN), &mut clip, &recent, 10); // -> the buffer
        assert_eq!(s.input.value(), "mine");
    }

    #[test]
    fn down_at_the_live_slot_does_nothing() {
        let recent = vec!["a".to_string()];
        let mut s = screen(recent.len());
        let mut clip = String::new();
        s.input.set_value("typing");
        s.key_pressed(k(KEY_DOWN), &mut clip, &recent, 10);
        // `newPos != historyPos` guards the whole body, so the buffer is not
        // saved and the field is untouched.
        assert_eq!(s.input.value(), "typing");
        assert_eq!(s.history_pos(), 1);
    }

    #[test]
    fn an_empty_history_makes_both_arrows_inert() {
        let mut s = screen(0);
        let mut clip = String::new();
        s.input.set_value("only this");
        s.key_pressed(k(KEY_UP), &mut clip, &[], 10);
        s.key_pressed(k(KEY_DOWN), &mut clip, &[], 10);
        assert_eq!(s.input.value(), "only this");
    }

    // ── scrolling ────────────────────────────────────────────────────────

    #[test]
    fn a_page_key_scrolls_a_page_minus_one_line() {
        let mut s = screen(0);
        let mut clip = String::new();
        assert_eq!(
            s.key_pressed(k(KEY_PAGE_UP), &mut clip, &[], 20),
            ChatAction::Scroll(19),
        );
        assert_eq!(
            s.key_pressed(k(KEY_PAGE_DOWN), &mut clip, &[], 20),
            ChatAction::Scroll(-19),
        );
    }

    #[test]
    fn the_wheel_is_clamped_before_it_is_multiplied() {
        // One notch is seven lines however large the device's delta is — the
        // clamp is what makes the speed a property of the client rather than
        // of the mouse.
        let s = screen(0);
        assert_eq!(s.mouse_scrolled(1.0, false), ChatAction::Scroll(7));
        assert_eq!(s.mouse_scrolled(4.0, false), ChatAction::Scroll(7));
        assert_eq!(s.mouse_scrolled(-4.0, false), ChatAction::Scroll(-7));
        // Shift holds it to one line.
        assert_eq!(s.mouse_scrolled(4.0, true), ChatAction::Scroll(1));
    }

    // ── the draft on the way out ─────────────────────────────────────────

    #[test]
    fn esc_keeps_the_draft_and_submitting_discards_it() {
        let mut s = screen(0);
        s.input.set_value("unsent");
        s.close();
        assert_eq!(
            s.removed(true),
            DraftOutcome::Save(Draft::of("unsent")),
        );

        let mut s = screen(0);
        let mut clip = String::new();
        s.input.set_value("sent");
        s.key_pressed(k(KEY_ENTER), &mut clip, &[], 10);
        assert_eq!(s.removed(true), DraftOutcome::Discard);
    }

    #[test]
    fn with_drafts_disabled_esc_discards_too() {
        let mut s = screen(0);
        s.input.set_value("unsent");
        s.close();
        assert_eq!(s.removed(false), DraftOutcome::Discard);
    }

    #[test]
    fn an_interruption_keeps_the_draft_whatever_the_setting() {
        // `exitReason != INTERRUPTED` short-circuits the whole test, so the
        // `saveChatDrafts` option never gets a say.
        let mut s = screen(0);
        s.input.set_value("unsent");
        assert_eq!(s.exit_reason(), ExitReason::Interrupted);
        assert_eq!(s.removed(false), DraftOutcome::Save(Draft::of("unsent")));
    }

    #[test]
    fn a_blank_field_discards_rather_than_saving_whitespace() {
        let mut s = screen(0);
        s.input.set_value("   ");
        s.close();
        assert_eq!(s.removed(true), DraftOutcome::Discard);
    }

    #[test]
    fn an_untouched_restored_draft_is_kept_rather_than_re_saved() {
        let draft = Draft::of("original");
        let mut s = ChatScreen::open(ChatMethod::Message, Some(&draft), 0);
        s.close();
        assert_eq!(s.removed(true), DraftOutcome::Keep);
        // …but editing it makes it a save again.
        let mut s = ChatScreen::open(ChatMethod::Message, Some(&draft), 0);
        s.char_typed('!');
        s.close();
        assert_eq!(s.removed(true), DraftOutcome::Save(Draft::of("original!")));
    }

    // ── geometry ─────────────────────────────────────────────────────────

    #[test]
    fn the_field_overhangs_the_right_edge_and_its_backdrop_does_not() {
        // `EditBox(font, 4, height - 12, width - 4, 12)` against
        // `fill(2, height - 14, width - 2, height - 2, …)`. The field is
        // asymmetric (4 in on the left, reaching the right edge) and the bar
        // behind it is symmetric (2 on both sides).
        let (fx, fy, fw, fh) = input_rect(320, 240);
        assert_eq!((fx, fy, fw, fh), (4, 228, 316, 12));
        assert_eq!(fx + fw, 320, "the field reaches the screen's right edge");
        let (bx, by, bw, bh) = input_backdrop_rect(320, 240);
        assert_eq!((bx, by, bw, bh), (2, 226, 316, 12));
        assert_eq!(bx + bw, 318, "the bar stops two short of it");
    }

    #[test]
    fn the_input_bar_does_not_follow_the_text_background_slider() {
        // `backgroundForChatOnly` defaults true, so `getBackgroundColor(int)`
        // returns its fallback `Integer.MIN_VALUE` — a fixed alpha 128 — where
        // M109's chat-row backdrops read `textBackgroundOpacity`.
        assert!((INPUT_BACKDROP_ALPHA - 128.0 / 255.0).abs() < 1e-6);
    }

    // ── the unhandled case ───────────────────────────────────────────────

    #[test]
    fn an_unrelated_key_is_not_handled() {
        let mut s = screen(0);
        let mut clip = String::new();
        // F5, which the caller should still get.
        assert_eq!(s.key_pressed(k(294), &mut clip, &[], 10), ChatAction::NotHandled);
    }
}
