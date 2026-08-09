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
//! # Clickable chat text (M128)
//!
//! [`ChatScreen::mouse_clicked`] runs a `ClickableStyleFinder` — see
//! [`crate::active_text`] — and [`ChatScreen::handle_component_clicked`]
//! resolves what it found. The paragraph that used to sit under "what is
//! deliberately not here" said there was no `Style` left to find, because a
//! component was flattened to plain text at the wire; M126 made a chat line
//! styled spans and M128 put the events on them.
//!
//! # What is deliberately not here
//!
//! **`CommandSuggestions` — no longer true, and it is the field above.** This
//! entry said the popup needed the `commands` packet, which was class C. M113
//! decoded that packet, M114 built the popup, M115 drew it and M116 gave it a
//! dispatcher; [`ChatScreen::suggestions`] has been a real
//! [`crate::command_suggestions::CommandSuggestions`] since M114 and the four
//! early-outs this entry promised are wired. What survives of it is one
//! observation that is still vanilla's: `init` calls
//! `setAllowSuggestions(false)` and only `onEdited` turns it on, so a freshly
//! opened box shows no popup even with a restored draft.
//!
//! **Chat abilities and the restricted prompt.** This entry said
//! `ChatAbilities` "arrives on a packet Rewo does not decode". **It arrives on
//! no packet at all** — `Minecraft.computeChatAbilities()` builds it entirely
//! client-side from three inputs: the `chatVisibility` option
//! (`HIDDEN` → chat *and* commands disabled, `SYSTEM` → chat only), the
//! launcher's `allowsChat` flag, and the Mojang profile's `CHAT_ALLOWED` user
//! flag, the last two only `if (isMultiplayerServer())`. Rewo has none of the
//! three, so `displayMode` is always `FOREGROUND` and never
//! `FOREGROUND_RESTRICTED`, and `CommandSuggestions.setRestrictions` is never
//! called with anything but `(true, true)` — which is why
//! `chat_screen.commands_not_allowed` and `chat_screen.messages_not_allowed`
//! are unreachable in `rewo_net::command_format::usage_lines`. The blocker is
//! an options screen and an account fetch, not a decode.

use crate::chat_events::ClickEvent;
use crate::chat_style::ChatStyle;
use crate::edit_box::{key, EditBox, Input};

/// `ChatScreen.MOUSE_SCROLL_SPEED`.
pub const MOUSE_SCROLL_SPEED: f64 = 7.0;

/// `ChatComponent.QUEUE_EXPAND_ID` —
/// `Identifier.withDefaultNamespace("internal/expand_chat_queue")`, the
/// `Custom` id on the "N messages held back" link.
pub const QUEUE_EXPAND_ID: &str = "minecraft:internal/expand_chat_queue";

/// `ChatComponent.GO_TO_RESTRICTIONS_SCREEN` — the `Custom` id on the
/// restricted-chat prompt's red underlined line.
pub const GO_TO_RESTRICTIONS_SCREEN: &str = "minecraft:internal/go_to_restrictions_screen";

/// `Commands.trimOptionalPrefix` — `command.startsWith("/") ? command
/// .substring(1) : command`.
///
/// **One slash, not all of them**: `//co i` (a common plugin command) trims to
/// `/co i`, which is what the server expects on the wire.
pub fn trim_optional_prefix(command: &str) -> &str {
    command.strip_prefix('/').unwrap_or(command)
}

/// What a click on chat text asks the caller to do.
///
/// The screen owns no socket and no platform, so — like [`ChatAction`] — every
/// effect leaves as a value. The two it can perform itself
/// (`suggest_command`'s replace and the shift-insertion) it does, because it
/// owns the [`EditBox`] they write to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChatClick {
    /// `handleComponentClicked` returned false — the click was not consumed
    /// and `super.mouseClicked` should run. Also the shift-insertion path,
    /// **which mutates the field and still answers false**.
    NotHandled,
    /// Consumed, and everything it asked for has already happened.
    Handled,
    /// `Util.getPlatform().openUri(uri)`. Already gated to `http`/`https` at
    /// decode — see [`crate::chat_events`] — so the caller does not re-check
    /// and cannot forget to.
    OpenUrl(String),
    /// `connection.sendUnattendedCommand(command)`, the leading slash already
    /// trimmed.
    RunCommand(String),
    /// An action Rewo decodes and deliberately does not perform. The string
    /// says why, and is a `&'static str` so it cannot carry server text into a
    /// log line.
    Declined(&'static str),
}

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
    /// The autocomplete popup (M114). Vanilla's `ChatScreen` owns one too.
    pub suggestions: crate::command_suggestions::CommandSuggestions,
    /// A `/`-command the caller should ask the server about, produced by
    /// [`crate::command_suggestions::CommandInfo::Command`]. Drained rather
    /// than sent, because the screen owns no socket — the same seam
    /// [`ChatAction`] is.
    command_request: Option<String>,
}

/// Everything [`ChatScreen`] needs to drive its popup that it cannot own: the
/// field's screen geometry, a font measurement, the words plain chat completes
/// from, and the `autoSuggestions` option.
pub struct SuggestionEnv<'a> {
    pub metrics: crate::command_suggestions::InputMetrics,
    pub width: &'a dyn Fn(&str) -> i32,
    /// `getCustomTabSuggestions()` — the online players unioned with whatever
    /// `custom_chat_completions` has set.
    pub tab_words: &'a [String],
    /// `minecraft.options.autoSuggestions().get()`, which defaults **true**.
    pub auto_suggestions: bool,
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
            // `ChatScreen.init` ends with `setAllowHiding(false)` and
            // `setAllowSuggestions(false)`, so the popup cannot appear until
            // the first edit — but Tab can still force it, because hiding is
            // off. Both are in `CommandSuggestions::for_chat`.
            suggestions: crate::command_suggestions::CommandSuggestions::for_chat(),
            command_request: None,
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

    /// `onEdited` — the responder `ChatScreen.init` installs on the field.
    ///
    /// Three lines in vanilla, in this order:
    /// `setAllowSuggestions(true); updateCommandInfo(); isDraft = false;`.
    /// The first is why a freshly opened box shows no popup and the second
    /// keystroke does.
    fn on_edited(&mut self, env: &SuggestionEnv<'_>) {
        self.suggestions.set_allow_suggestions(true);
        self.update_command_info(env);
        self.is_draft = false;
    }

    /// `commandSuggestions.updateCommandInfo()`, plus the tail of
    /// `updateUsageInfo` that decides whether to open the popup.
    ///
    /// Vanilla reaches the second through a future's continuation; here the
    /// local path is already resolved, so the two run back to back. A
    /// `/`-command has no local answer (see [`crate::command_suggestions`]),
    /// and its text is parked for the caller to send.
    pub fn update_command_info(&mut self, env: &SuggestionEnv<'_>) {
        use crate::command_suggestions::CommandInfo;
        let words: Vec<&str> = env.tab_words.iter().map(String::as_str).collect();
        match self.suggestions.update_command_info(&mut self.input, words) {
            CommandInfo::Command(text) => self.command_request = Some(text),
            CommandInfo::Message => {
                self.command_request = None;
                self.suggestions
                    .auto_show(&mut self.input, env.metrics, env.auto_suggestions, env.width);
            }
            CommandInfo::Blank => self.command_request = None,
        }
    }

    /// The `/`-command text the caller should ask the server about, if any.
    ///
    /// Taken rather than read, so one edit produces at most one request — the
    /// single-slot pending id on the other side would drop a duplicate
    /// anyway, but sending one is still a packet.
    pub fn take_command_request(&mut self) -> Option<String> {
        self.command_request.take()
    }

    /// The server's reply, once its id matched the outstanding request.
    ///
    /// Vanilla completes a future here and its continuation calls
    /// `updateUsageInfo`, whose tail is the `showSuggestions` below.
    pub fn accept_suggestions(
        &mut self,
        suggestions: crate::suggestions::Suggestions,
        env: &SuggestionEnv<'_>,
    ) {
        self.suggestions.set_pending(Some(suggestions));
        self.suggestions
            .auto_show(&mut self.input, env.metrics, env.auto_suggestions, env.width);
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
        env: &SuggestionEnv<'_>,
    ) -> ChatAction {
        // `commandSuggestions.keyPressed` is the FIRST thing
        // `ChatScreen.keyPressed` calls — ahead of the draft backspace, the
        // edit box and the confirmation. That is what lets Up/Down walk the
        // popup rather than the send history whenever one is open.
        let before = self.input.value();
        if self.suggestions.key_pressed(
            input,
            &mut self.input,
            env.metrics,
            env.width,
        ) {
            if self.input.value() != before {
                // Applying a suggestion is an edit, so vanilla's responder
                // fires — but from inside `setValue`, while `keepSuggestions`
                // is true, so it recomputes the pending set and leaves the
                // list alone. `refresh_pending` is that call without the
                // teardown; running the whole of `on_edited` here would drop
                // the popup a Tab had just filled from.
                self.is_draft = false;
                self.suggestions.set_allow_suggestions(true);
                let words: Vec<&str> = env.tab_words.iter().map(String::as_str).collect();
                if let crate::command_suggestions::CommandInfo::Command(text) =
                    self.suggestions.refresh_pending(&mut self.input, words)
                {
                    self.command_request = Some(text);
                }
            }
            return ChatAction::None;
        }
        if self.is_draft && input.key == key::BACKSPACE {
            self.input.set_value("");
            self.is_draft = false;
            return ChatAction::None;
        }
        let before = self.input.value();
        if self.input.key_pressed(input, clipboard) {
            if self.input.value() != before {
                self.on_edited(env);
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
    pub fn char_typed(&mut self, ch: char, env: &SuggestionEnv<'_>) -> bool {
        let before = self.input.value();
        let handled = self.input.char_typed(ch);
        if handled && self.input.value() != before {
            self.on_edited(env);
        }
        handled
    }

    /// `ChatScreen.mouseClicked` — the popup gets first refusal, exactly as it
    /// does for keys, and clickable chat text is what it refuses *to* (M128).
    ///
    /// ```java
    /// if (this.commandSuggestions.mouseClicked(event)) return true;
    /// if (event.button() == 0) {
    ///    … ClickableStyleFinder … captureClickableText …
    ///    Style clicked = finder.result();
    ///    if (clicked != null && this.handleComponentClicked(clicked, this.insertionClickMode())) {
    ///       this.initial = this.input.getValue();
    ///       return true;
    ///    }
    /// }
    /// return super.mouseClicked(event, doubleClick);
    /// ```
    ///
    /// `hit` is a **closure**, not a value, so the hit test runs only after the
    /// popup declines — that ordering is vanilla's and is free to keep. It is
    /// the caller's because the chat store, the font and the box geometry all
    /// live outside this screen; [`crate::chat::clickable_style_at`] is the
    /// function it should be.
    ///
    /// `button` is the raw button index, because **only the left button
    /// looks** (`event.button() == 0`) — a right-click on a link does nothing
    /// at all.
    pub fn mouse_clicked(
        &mut self,
        x: i32,
        y: i32,
        button: u8,
        shift: bool,
        hit: &dyn Fn(bool) -> Option<ChatStyle>,
    ) -> ChatClick {
        if self.suggestions.mouse_clicked(x, y, &mut self.input) {
            return ChatClick::Handled;
        }
        if button != 0 {
            return ChatClick::NotHandled;
        }
        // `insertionClickMode()` is `minecraft.hasShiftDown()`, and it is
        // passed BOTH to the finder (so a shift-click can find a style whose
        // only affordance is an insertion) and to the handler (so it takes the
        // insertion branch). One flag, two jobs.
        let Some(style) = hit(shift) else {
            return ChatClick::NotHandled;
        };
        self.handle_component_clicked(&style, shift)
    }

    /// `ChatScreen.handleComponentClicked(Style, boolean allowInsertions)`.
    ///
    /// ```java
    /// ClickEvent event = clicked.getClickEvent();
    /// if (allowInsertions) {
    ///    if (clicked.getInsertion() != null) this.insertText(clicked.getInsertion(), false);
    /// } else if (event != null) {
    ///    switch (event) { … default: defaultHandleGameClickEvent(event, …); }
    ///    return true;
    /// }
    /// return false;
    /// ```
    ///
    /// **Shift REPLACES the click path; it does not prefer the insertion over
    /// it.** The `else if` means a shift-click never runs a command, whether or
    /// not the style carries an insertion — a "prefer insertion, else click"
    /// reading runs commands vanilla never runs, which is the difference
    /// between shift-clicking a player's name and executing whatever the
    /// server attached to it.
    ///
    /// **And the insertion branch returns `false`**, even when the insertion
    /// happened: the `return true` is inside the `else if`. So a shift-click
    /// falls through to `super.mouseClicked` and the widgets underneath still
    /// see it. `event` is read into a local before the branch and then never
    /// used on that path, which is what makes the shape easy to misread.
    pub fn handle_component_clicked(
        &mut self,
        clicked: &ChatStyle,
        allow_insertions: bool,
    ) -> ChatClick {
        if allow_insertions {
            if let Some(insertion) = clicked.insertion() {
                // `insertText(text, false)` — INSERT at the caret, not replace.
                let insertion = insertion.to_owned();
                self.input.insert_text(&insertion);
                self.is_draft = false;
            }
            return ChatClick::NotHandled;
        }
        let Some(event) = clicked.click() else {
            return ChatClick::NotHandled;
        };
        // **`this.initial = this.input.getValue()` on the consuming path is
        // inert**, and is not reproduced. `initial` has exactly two readers,
        // both in `removed()`, and `removed()` reassigns it from the same
        // expression on the line above them — so the assignment in
        // `mouseClicked` can never be observed. Same class as `int border = 4`
        // and `centerY`'s `+ 13` (M104).
        //
        // What DOES clear `isDraft` is the field's own responder:
        // `EditBox.setValue` and `EditBox.insertText` both end in
        // `onValueChange`, which `ChatScreen.init` wires to `onEdited`. So it
        // is cleared exactly when the click changed the field — the insertion
        // above and `suggest_command` below — and not for a `run_command`, an
        // `open_url` or a declined action.
        self.dispatch_click_event(event)
    }

    /// `Screen.defaultHandleGameClickEvent` followed by
    /// `Screen.defaultHandleClickEvent`, with `ChatScreen`'s two `Custom`
    /// interceptions in front of both.
    fn dispatch_click_event(&mut self, event: &ClickEvent) -> ChatClick {
        match event {
            // `case ClickEvent.Custom when id.equals(ChatComponent
            // .QUEUE_EXPAND_ID)` and `… GO_TO_RESTRICTIONS_SCREEN`. Both are
            // vanilla's own internal affordances — the delayed-message expand
            // link and the restricted-chat prompt — and both need a subsystem
            // Rewo lacks (the chat delay queue; `ChatAbilities`, which arrives
            // on **no packet at all** — see this module's own docs, where
            // M134c corrected exactly this sentence. M128 was written before
            // that correction existed and the two landed in this file from
            // different branches without conflicting, so the file contradicted
            // itself until the integration). Named rather than folded into
            // the generic `Custom` decline, because a server cannot produce
            // them and a later milestone will want them exactly here.
            ClickEvent::Custom { id, .. } if id == QUEUE_EXPAND_ID => {
                ChatClick::Declined("the chat delay queue is not modelled")
            }
            ClickEvent::Custom { id, .. } if id == GO_TO_RESTRICTIONS_SCREEN => {
                ChatClick::Declined("there is no restrictions screen")
            }
            // --- defaultHandleGameClickEvent ---
            ClickEvent::RunCommand(command) => {
                // `clickCommandAction` → `sendUnattendedCommand(Commands
                // .trimOptionalPrefix(command), screenAfterCommand)`.
                ChatClick::RunCommand(trim_optional_prefix(command).to_owned())
            }
            ClickEvent::ShowDialog(_) => ChatClick::Declined("there is no dialog screen"),
            ClickEvent::Custom { .. } => ChatClick::Declined("custom_click_action is not sent"),
            // --- defaultHandleClickEvent ---
            ClickEvent::OpenUrl(uri) => ChatClick::OpenUrl(uri.clone()),
            ClickEvent::SuggestCommand(command) => {
                // `activeScreen.insertText(command, true)` — REPLACE, where the
                // insertion path passes `false` and inserts.
                self.input.set_value(command);
                // `onValueChange` -> `onEdited`. Its other two lines
                // (`setAllowSuggestions(true)`, `updateCommandInfo()`) need
                // the `SuggestionEnv` this method does not take; the caller
                // refreshes the popup after a consumed click instead.
                self.is_draft = false;
                ChatClick::Handled
            }
            ClickEvent::CopyToClipboard(_) => {
                // Rewo's clipboard is in-process (M93t) — nothing outside the
                // chat field could paste it, so writing there would look like
                // success and not be it.
                ChatClick::Declined("the clipboard is in-process only")
            }
            // `default -> LOGGER.error("Don't know how to handle {}", event)`.
            // **`change_page` reaches this, in vanilla too**: it is a
            // `BookViewScreen` action (that screen's own `handleClickEvent`
            // switch takes it before delegating), and from chat it only logs.
            // Declining it is exact rather than a deviation.
            ClickEvent::ChangePage(_) => {
                ChatClick::Declined("change_page belongs to the book screen")
            }
        }
    }

    /// The hover half: moving the pointer over a row selects it.
    pub fn mouse_moved(&mut self, mouse: (i32, i32)) {
        self.suggestions.mouse_moved(mouse, &mut self.input);
    }

    /// `ChatScreen.mouseScrolled`.
    ///
    /// **The clamp comes first**, then the multiply — so one wheel notch is
    /// seven lines and a high-resolution trackpad reporting 4.0 still moves
    /// seven, not twenty-eight. Shift holds it to one line. Multiplying before
    /// clamping would make the speed a property of the input device.
    pub fn mouse_scrolled(&mut self, scroll_y: f64, shift: bool, mouse: (i32, i32)) -> ChatAction {
        // `commandSuggestions.mouseScrolled` first — a wheel over the popup
        // scrolls the popup, not the chat behind it.
        if self.suggestions.mouse_scrolled(scroll_y, mouse) {
            return ChatAction::None;
        }
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

    /// A width of 6 px per UTF-16 unit, no tab words, auto-suggest on. The
    /// tests below are about the screen, not the popup; the popup has its own
    /// module. An empty word list is what keeps them independent — with words
    /// in it every keystroke would open a popup and swallow the next key.
    fn w(s: &str) -> i32 {
        s.encode_utf16().count() as i32 * 6
    }

    const NO_WORDS: &[String] = &[];

    fn env() -> SuggestionEnv<'static> {
        SuggestionEnv {
            metrics: crate::command_suggestions::InputMetrics {
                x: 4,
                inner_width: 316,
                screen_height: 240,
            },
            width: &w,
            tab_words: NO_WORDS,
            auto_suggestions: true,
        }
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
        s.key_pressed(k(key::BACKSPACE), &mut clip, &[], 10, &env());
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
        s.char_typed('d', &env());
        assert!(!s.is_draft());
        assert_eq!(s.input.value(), "abcd");
        s.key_pressed(k(key::BACKSPACE), &mut clip, &[], 10, &env());
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
                s.key_pressed(k(key), &mut clip, &[], 10, &env()),
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
            s.key_pressed(k(KEY_ENTER), &mut clip, &[], 10, &env()),
            ChatAction::Command("time set day".into()),
        );
    }

    #[test]
    fn submitting_a_blank_message_sends_nothing_but_still_closes() {
        let mut s = screen(0);
        let mut clip = String::new();
        s.input.set_value("   ");
        assert_eq!(s.key_pressed(k(KEY_ENTER), &mut clip, &[], 10, &env()), ChatAction::None);
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
        s.key_pressed(k(KEY_UP), &mut clip, &recent, 10, &env());
        assert_eq!(s.input.value(), "second");
        s.key_pressed(k(KEY_UP), &mut clip, &recent, 10, &env());
        assert_eq!(s.input.value(), "first");
        // …and stops at the top rather than wrapping.
        s.key_pressed(k(KEY_UP), &mut clip, &recent, 10, &env());
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
        s.key_pressed(k(KEY_UP), &mut clip, &recent, 10, &env());
        assert_eq!(s.input.value(), "earlier");
        s.key_pressed(k(KEY_DOWN), &mut clip, &recent, 10, &env());
        assert_eq!(s.input.value(), "half a thought");
    }

    #[test]
    fn the_buffer_is_saved_only_when_leaving_the_live_slot() {
        // Walking further up must not overwrite it with an entry.
        let recent = vec!["a".to_string(), "b".to_string()];
        let mut s = screen(recent.len());
        let mut clip = String::new();
        s.input.set_value("mine");
        s.key_pressed(k(KEY_UP), &mut clip, &recent, 10, &env()); // -> "b"
        s.key_pressed(k(KEY_UP), &mut clip, &recent, 10, &env()); // -> "a"
        s.key_pressed(k(KEY_DOWN), &mut clip, &recent, 10, &env()); // -> "b"
        s.key_pressed(k(KEY_DOWN), &mut clip, &recent, 10, &env()); // -> the buffer
        assert_eq!(s.input.value(), "mine");
    }

    #[test]
    fn down_at_the_live_slot_does_nothing() {
        let recent = vec!["a".to_string()];
        let mut s = screen(recent.len());
        let mut clip = String::new();
        s.input.set_value("typing");
        s.key_pressed(k(KEY_DOWN), &mut clip, &recent, 10, &env());
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
        s.key_pressed(k(KEY_UP), &mut clip, &[], 10, &env());
        s.key_pressed(k(KEY_DOWN), &mut clip, &[], 10, &env());
        assert_eq!(s.input.value(), "only this");
    }

    // ── scrolling ────────────────────────────────────────────────────────

    #[test]
    fn a_page_key_scrolls_a_page_minus_one_line() {
        let mut s = screen(0);
        let mut clip = String::new();
        assert_eq!(
            s.key_pressed(k(KEY_PAGE_UP), &mut clip, &[], 20, &env()),
            ChatAction::Scroll(19),
        );
        assert_eq!(
            s.key_pressed(k(KEY_PAGE_DOWN), &mut clip, &[], 20, &env()),
            ChatAction::Scroll(-19),
        );
    }

    #[test]
    fn the_wheel_is_clamped_before_it_is_multiplied() {
        // One notch is seven lines however large the device's delta is — the
        // clamp is what makes the speed a property of the client rather than
        // of the mouse.
        let mut s = screen(0);
        assert_eq!(s.mouse_scrolled(1.0, false, (0, 0)), ChatAction::Scroll(7));
        assert_eq!(s.mouse_scrolled(4.0, false, (0, 0)), ChatAction::Scroll(7));
        assert_eq!(s.mouse_scrolled(-4.0, false, (0, 0)), ChatAction::Scroll(-7));
        // Shift holds it to one line.
        assert_eq!(s.mouse_scrolled(4.0, true, (0, 0)), ChatAction::Scroll(1));
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
        s.key_pressed(k(KEY_ENTER), &mut clip, &[], 10, &env());
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
        s.char_typed('!', &env());
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

    // ── the popup's place in the order ───────────────────────────────────

    /// A screen whose env offers three names, so the popup actually opens.
    fn with_words<'a>(words: &'a [String]) -> SuggestionEnv<'a> {
        SuggestionEnv {
            tab_words: words,
            ..env()
        }
    }

    #[test]
    fn up_walks_the_popup_rather_than_the_send_history_while_one_is_open() {
        // `commandSuggestions.keyPressed` is the first line of
        // `ChatScreen.keyPressed`, so with a popup open the arrows belong to
        // it. Ordered the other way round the history would move underneath a
        // visible list.
        let words: Vec<String> = ["Steve", "Steven"].iter().map(|s| s.to_string()).collect();
        let e = with_words(&words);
        let recent = vec!["earlier".to_string()];
        let mut s = ChatScreen::open(ChatMethod::Message, None, recent.len());
        let mut clip = String::new();
        for ch in "Ste".chars() {
            s.char_typed(ch, &e);
        }
        assert!(s.suggestions.is_visible());
        s.key_pressed(k(KEY_UP), &mut clip, &recent, 10, &e);
        // The field still holds what was typed; the history did not move.
        assert_eq!(s.input.value(), "Ste");
        assert_eq!(s.history_pos(), 1);
    }

    #[test]
    fn the_arrows_reach_the_history_again_once_the_popup_is_gone() {
        let words: Vec<String> = vec!["Steve".to_string()];
        let e = with_words(&words);
        let recent = vec!["earlier".to_string()];
        let mut s = ChatScreen::open(ChatMethod::Message, None, recent.len());
        let mut clip = String::new();
        s.char_typed('S', &e);
        assert!(s.suggestions.is_visible());
        s.key_pressed(k(crate::command_suggestions::KEY_ESCAPE), &mut clip, &recent, 10, &e);
        assert!(!s.suggestions.is_visible());
        s.key_pressed(k(KEY_UP), &mut clip, &recent, 10, &e);
        assert_eq!(s.input.value(), "earlier");
    }

    #[test]
    fn a_fresh_screen_shows_no_popup_until_the_first_edit() {
        // `init` calls `setAllowSuggestions(false)`; only `onEdited` turns it
        // on. So a restored draft sits there without a list.
        let words: Vec<String> = vec!["Steve".to_string()];
        let e = with_words(&words);
        let draft = Draft::of("Ste");
        let mut s = ChatScreen::open(ChatMethod::Message, Some(&draft), 0);
        s.update_command_info(&e);
        assert!(!s.suggestions.is_visible());
        s.char_typed('v', &e);
        assert!(s.suggestions.is_visible());
    }

    #[test]
    fn applying_a_suggestion_clears_the_draft_flag() {
        // Vanilla's responder fires from inside `setValue`, so `isDraft` goes
        // false even though the edit came from the popup rather than the
        // keyboard — which is what stops the next backspace wiping the line.
        let words: Vec<String> = vec!["Steve".to_string()];
        let e = with_words(&words);
        let draft = Draft::of("Ste");
        let mut s = ChatScreen::open(ChatMethod::Message, Some(&draft), 0);
        let mut clip = String::new();
        assert!(s.is_draft());
        s.update_command_info(&e);
        // The Tab that FORCES the popup open does not also fill it:
        // `CommandSuggestions.keyPressed` falls through to
        // `showSuggestions(true)` and returns, so the list is built and
        // nothing is applied. This witness asserted the one-press version
        // first and was wrong.
        let tab = k(crate::command_suggestions::KEY_TAB);
        s.key_pressed(tab, &mut clip, &[], 10, &e);
        assert!(s.suggestions.is_visible());
        assert_eq!(s.input.value(), "Ste");
        assert!(s.is_draft(), "opening the popup is not an edit");
        // The next one applies, and vanilla's responder — which fires from
        // inside `setValue` — is what clears the draft flag, so the next
        // backspace deletes one character rather than the line.
        s.key_pressed(tab, &mut clip, &[], 10, &e);
        assert_eq!(s.input.value(), "Steve");
        assert!(!s.is_draft());
    }

    #[test]
    fn a_slash_parks_a_request_for_the_caller_and_offers_nothing_locally() {
        let words: Vec<String> = vec!["Steve".to_string()];
        let e = with_words(&words);
        let mut s = screen(0);
        s.char_typed('/', &e);
        s.char_typed('g', &e);
        assert_eq!(s.take_command_request().as_deref(), Some("/g"));
        // Taken, so a second look is empty and no second packet goes out.
        assert_eq!(s.take_command_request(), None);
        assert!(!s.suggestions.is_visible());
    }

    #[test]
    fn a_wheel_over_the_popup_scrolls_it_rather_than_the_chat() {
        let words: Vec<String> = (0..20).map(|i| format!("Steve{i:02}")).collect();
        let e = with_words(&words);
        let mut s = screen(0);
        s.char_typed('S', &e);
        let rect = s.suggestions.list().unwrap().rect;
        assert_eq!(
            s.mouse_scrolled(-1.0, false, (rect.x + 2, rect.y + 2)),
            ChatAction::None
        );
        assert_eq!(s.suggestions.list().unwrap().offset(), 1);
        // Away from it, the chat scrolls as before.
        assert_eq!(
            s.mouse_scrolled(-1.0, false, (0, 0)),
            ChatAction::Scroll(-7)
        );
    }

    // ── the unhandled case ───────────────────────────────────────────────

    #[test]
    fn an_unrelated_key_is_not_handled() {
        let mut s = screen(0);
        let mut clip = String::new();
        // F5, which the caller should still get.
        assert_eq!(s.key_pressed(k(294), &mut clip, &[], 10, &env()), ChatAction::NotHandled);
    }

    // ---- M128: clickable chat text ---------------------------------------

    use crate::chat_events::{ChatEvents, ClickEvent};
    use std::sync::Arc;

    fn styled(events: ChatEvents) -> ChatStyle {
        ChatStyle { events: Some(Arc::new(events)), ..ChatStyle::WHITE }
    }

    fn with_click(click: ClickEvent) -> ChatStyle {
        styled(ChatEvents { click: Some(click), ..Default::default() })
    }


    /// `Commands.trimOptionalPrefix` takes ONE slash, so `//co i` survives as
    /// `/co i` — a plugin command, not a typo.
    #[test]
    fn trim_optional_prefix_takes_one_slash() {
        assert_eq!(trim_optional_prefix("/kill"), "kill");
        assert_eq!(trim_optional_prefix("//co i"), "/co i");
        assert_eq!(trim_optional_prefix("kill"), "kill");
        assert_eq!(trim_optional_prefix(""), "");
    }

    #[test]
    fn a_run_command_click_leaves_as_a_command_without_its_slash() {
        let mut s = screen(0);
        assert_eq!(
            s.handle_component_clicked(&with_click(ClickEvent::RunCommand("/kill @e".into())), false),
            ChatClick::RunCommand("kill @e".into())
        );
    }

    #[test]
    fn an_open_url_click_leaves_as_the_url() {
        let mut s = screen(0);
        assert_eq!(
            s.handle_component_clicked(
                &with_click(ClickEvent::OpenUrl("https://example.com".into())),
                false
            ),
            ChatClick::OpenUrl("https://example.com".into())
        );
    }

    /// `suggest_command` is `insertText(command, true)` — **replace**, so the
    /// field's previous contents go.
    #[test]
    fn a_suggest_command_click_replaces_the_field() {
        let mut s = screen(0);
        s.input.set_value("typed");
        assert_eq!(
            s.handle_component_clicked(
                &with_click(ClickEvent::SuggestCommand("/tp Steve".into())),
                false
            ),
            ChatClick::Handled
        );
        assert_eq!(s.input.value(), "/tp Steve");
    }

    /// The four Rewo does not perform, and the two `ChatComponent` ids in
    /// front of them.
    #[test]
    fn the_declined_actions_are_declined_rather_than_approximated() {
        let mut s = screen(0);
        for event in [
            ClickEvent::CopyToClipboard("x".into()),
            ClickEvent::ChangePage(3),
            ClickEvent::ShowDialog(rewo_proto::nbt::Nbt::String("d".into())),
            ClickEvent::Custom { id: "ns:thing".into(), payload: None },
            ClickEvent::Custom { id: QUEUE_EXPAND_ID.into(), payload: None },
            ClickEvent::Custom { id: GO_TO_RESTRICTIONS_SCREEN.into(), payload: None },
        ] {
            assert!(
                matches!(s.handle_component_clicked(&with_click(event.clone()), false), ChatClick::Declined(_)),
                "{event:?}"
            );
        }
    }

    /// **Shift REPLACES the click path.** The `else if` means a shift-click on
    /// a `run_command` link runs nothing, whether or not the style also
    /// carries an insertion — a "prefer insertion, else click" reading would
    /// run commands vanilla never runs.
    #[test]
    fn shift_never_runs_the_command() {
        let mut s = screen(0);
        let both = styled(ChatEvents {
            click: Some(ClickEvent::RunCommand("/kill".into())),
            insertion: Some("Steve".into()),
            ..Default::default()
        });
        assert_eq!(s.handle_component_clicked(&both, true), ChatClick::NotHandled);
        assert_eq!(s.input.value(), "Steve");

        // And with no insertion at all, a shift-click still does not run it.
        let mut s = screen(0);
        let click_only = with_click(ClickEvent::RunCommand("/kill".into()));
        assert_eq!(s.handle_component_clicked(&click_only, true), ChatClick::NotHandled);
        assert_eq!(s.input.value(), "");
    }

    /// `insertText(text, false)` — the insertion is INSERTED at the caret,
    /// where `suggest_command` replaces.
    #[test]
    fn a_shift_insertion_inserts_rather_than_replaces() {
        let mut s = screen(0);
        s.input.set_value("hi ");
        let style = styled(ChatEvents { insertion: Some("Steve".into()), ..Default::default() });
        assert_eq!(s.handle_component_clicked(&style, true), ChatClick::NotHandled);
        assert_eq!(s.input.value(), "hi Steve");
    }

    /// **`isDraft` is cleared by the field's responder, not by the click.**
    /// `EditBox.setValue` and `insertText` both end in `onValueChange`, which
    /// `init` wires to `onEdited`; so a click that changed the field clears
    /// it and a click that only ran a command does not. (And
    /// `mouseClicked`'s own `this.initial = this.input.getValue()` is inert:
    /// `initial`'s only two readers are in `removed()`, which reassigns it
    /// from the same expression one line above them.)
    #[test]
    fn only_a_click_that_changed_the_field_clears_the_draft_flag() {
        let restored = crate::chat_screen::Draft::of("held");
        let fresh = || ChatScreen::open(ChatMethod::Message, Some(&restored), 0);

        let mut s = fresh();
        assert!(s.is_draft());
        s.handle_component_clicked(&with_click(ClickEvent::RunCommand("/kill".into())), false);
        assert!(s.is_draft(), "a run_command does not touch the field");

        let mut s = fresh();
        s.handle_component_clicked(&with_click(ClickEvent::CopyToClipboard("x".into())), false);
        assert!(s.is_draft(), "a declined action does not touch the field");

        let mut s = fresh();
        s.handle_component_clicked(&with_click(ClickEvent::SuggestCommand("/tp".into())), false);
        assert!(!s.is_draft(), "suggest_command replaced the field");

        let mut s = fresh();
        let ins = styled(ChatEvents { insertion: Some("Steve".into()), ..Default::default() });
        s.handle_component_clicked(&ins, true);
        assert!(!s.is_draft(), "the shift insertion changed the field");
    }

    /// A style with nothing on it is not consumed either way.
    #[test]
    fn a_plain_style_is_not_consumed() {
        let mut s = screen(0);
        assert_eq!(s.handle_component_clicked(&ChatStyle::WHITE, false), ChatClick::NotHandled);
        assert_eq!(s.handle_component_clicked(&ChatStyle::WHITE, true), ChatClick::NotHandled);
    }

    /// `event.button() == 0` — a right-click on a link does not even look.
    #[test]
    fn only_the_left_button_looks() {
        let hit = |_shift: bool| Some(with_click(ClickEvent::RunCommand("/kill".into())));
        let mut s = screen(0);
        assert_eq!(s.mouse_clicked(0, 0, 1, false, &hit), ChatClick::NotHandled);
        let mut s = screen(0);
        assert_eq!(
            s.mouse_clicked(0, 0, 0, false, &hit),
            ChatClick::RunCommand("kill".into())
        );
    }

    /// The hit test runs only after the popup has declined, and the shift flag
    /// reaches it — `includeInsertions(this.insertionClickMode())`.
    #[test]
    fn the_hit_test_is_told_whether_shift_is_down() {
        let seen = std::cell::Cell::new(None);
        let hit = |shift: bool| {
            seen.set(Some(shift));
            None
        };
        let mut s = screen(0);
        assert_eq!(s.mouse_clicked(0, 0, 0, true, &hit), ChatClick::NotHandled);
        assert_eq!(seen.get(), Some(true));
    }
}
