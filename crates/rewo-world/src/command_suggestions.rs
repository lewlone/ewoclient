//! `CommandSuggestions` and its `SuggestionsList` — the autocomplete popup
//! (M114).
//!
//! M110 built the `ChatScreen` and marked the four early-outs this owns at
//! their call sites; this is what fills them. The popup itself is shared by
//! two paths that have nothing else in common: plain chat, which completes
//! player names locally from the set `custom_chat_completions` maintains, and
//! `/`-commands, which need a parse. Only the first is reachable here — see
//! "What is deliberately not here" below.
//!
//! # The list is positioned from the input, and can end up off-screen left
//!
//! ```java
//! int x = Mth.clamp(this.input.getScreenX(range.getStart()), 0,
//!                   this.input.getScreenX(0) + this.input.getInnerWidth() - maxSuggestionWidth);
//! ```
//!
//! `Mth.clamp(int, int, int)` is `Math.min(Math.max(value, min), max)`, which
//! for `max < min` returns **`max`** rather than `min`. So a suggestion wider
//! than the field pins the popup to a *negative* x and it hangs off the left
//! edge. That is vanilla's, it is reachable with a long enough entry, and a
//! clamp written the other way round (`max(min(...)))`) would silently pin it
//! to 0 instead.
//!
//! # `Rect2i.contains` and the hover test disagree, on purpose or not
//!
//! ```java
//! public boolean contains(int x, int y) {
//!    return x >= this.xPos && x <= this.xPos + this.width && ...;
//! }
//! ```
//!
//! — inclusive on **all four** sides. The per-row hover test in the render is
//! `mouseX > rect.getX() && mouseX < rect.getX() + rect.getWidth()`, strictly
//! inside. So the popup's leftmost column is clickable and never highlights,
//! and a click one pixel below its bottom edge is *inside* `contains`, lands
//! on `line = (y - rect.y) / 12 + offset == limit + offset`, and — when the
//! list is longer than the window — **selects and applies an entry that is not
//! on screen**. Both behaviours are transcribed rather than tidied.
//!
//! # `lineStartOffset` exists to keep the selected row visible
//!
//! `cycle`'s downward branch is `offset = clamp(current + lineStartOffset -
//! suggestionLineLimit, …)`. The chat screen passes **1**, so the window
//! becomes `[current - 9, current]` and the newly-selected row is its last
//! line. With `0` it would be `[current - 10, current - 1]` and the row you
//! just moved onto would be one past the bottom — invisible. The upward
//! branch has no such term because `offset = current` already puts the row
//! first.
//!
//! # `sortSuggestions` compares a lower-cased needle against raw text
//!
//! ```java
//! String lastWord = partialCommand.substring(lastWordIndex).toLowerCase(Locale.ROOT);
//! ...
//! if (!suggestion.getText().startsWith(lastWord) && !suggestion.getText().startsWith("minecraft:" + lastWord))
//! ```
//!
//! Only the needle is lower-cased. So `Steve`, which `matchesSubStr` matched
//! case-insensitively a step earlier, fails `startsWith("st")` and is demoted
//! to the second bucket — **capitalised names sort after lower-case ones even
//! though both matched**. Lower-casing both sides is the obvious tidy-up and
//! changes the order of every mixed-case list.
//!
//! This is also the only place a namespace is considered, via the separate
//! `"minecraft:" + lastWord` test; [`crate::suggestions::matches_sub_str`]
//! deliberately does not split on `:`.
//!
//! # What is deliberately not here
//!
//! **The command path's local half.** `updateCommandInfo`'s `/` branch runs
//! `dispatcher.parse` and `getCompletionSuggestions` against a client-side
//! Brigadier `CommandDispatcher` built from the tree M113 decodes. Rewo has
//! the tree as data and no dispatcher, so it cannot answer a literal
//! locally. [`CommandInfo::Command`] reports the branch was taken and carries
//! the text a caller may ask the *server* about; M114d does exactly that.
//!
//! **The usage lines.** `updateUsageInfo` renders `getSmartUsage` and
//! Brigadier's parse exceptions under the field. Both need the same
//! dispatcher.
//!
//! **Syntax highlighting.** `formatChat` colours parsed arguments through five
//! rotating styles and the unparsed tail red; it reads `currentParse`, so it
//! needs the dispatcher too. Without one every character stays the field's
//! ordinary colour, which is a state vanilla also passes through — before the
//! first parse, `formatChat` returns `null`.
//!
//! **Narration.** Rewo has no narrator.

use crate::edit_box::{EditBox, Input};
use crate::suggestions::{suggest_matching, Suggestion, Suggestions, SuggestionsBuilder};

/// `CommandSuggestions.LINE_HEIGHT`.
pub const LINE_HEIGHT: i32 = 12;
/// `CommandSuggestions.USAGE_OFFSET_FROM_BOTTOM`.
pub const USAGE_OFFSET_FROM_BOTTOM: i32 = 27;
/// The colour of the selected row's text — `-256`, i.e. `0xFFFFFF00`.
pub const SELECTED_COLOR: u32 = 0xFFFF_FF00;
/// Every other row — `-5592406`, i.e. `0xFFAAAAAA`.
///
/// `extractRenderState` also assigns this to a local `int unselectedColor`
/// which it then never reads, using the literal instead. Dead, like
/// `extractRenderState`'s `int border = 4` in the recipe book; kept here as
/// one constant because there is nothing to be faithful to.
pub const UNSELECTED_COLOR: u32 = 0xFFAA_AAAA;
/// The scroll-indicator dashes — `-1`.
pub const INDICATOR_COLOR: u32 = 0xFFFF_FFFF;

/// The ten constructor arguments `ChatScreen` passes, minus the four this
/// module does not need (the narrator, the screen, the font, and
/// `commandsOnly`/`onlyShowIfCursorPastError`, which only the unreachable
/// command branch reads).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SuggestionsConfig {
    /// `lineStartOffset` — see the module docs; it is what keeps the selected
    /// row on screen when cycling downwards.
    pub line_start_offset: i32,
    pub suggestion_line_limit: usize,
    pub anchor_to_bottom: bool,
    pub fill_color: u32,
    /// `input.isBordered()`. The chat field calls `setBordered(false)`, and
    /// the list shifts one pixel left and one down because of it.
    pub bordered: bool,
}

impl SuggestionsConfig {
    /// `new CommandSuggestions(minecraft, this, input, font, false, false, 1, 10, true, -805306368)`
    /// from `ChatScreen.init`, with `setBordered(false)` on the field.
    pub const CHAT: Self = Self {
        line_start_offset: 1,
        suggestion_line_limit: 10,
        anchor_to_bottom: true,
        fill_color: 0xD000_0000,
        bordered: false,
    };
}

/// The input field's screen geometry, which the popup is positioned from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InputMetrics {
    /// `input.getX()`.
    pub x: i32,
    /// `input.getInnerWidth()` — `bordered ? width - 8 : width`.
    pub inner_width: i32,
    /// `screen.height`, which `anchorToBottom` measures down from.
    pub screen_height: i32,
}

/// `Rect2i`, with its own inclusive-on-all-sides `contains`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    /// `Rect2i.contains` — `>=` on the low edges and **`<=`** on the high
    /// ones, so it admits a pixel one past the right and bottom.
    pub fn contains(self, x: i32, y: i32) -> bool {
        x >= self.x && x <= self.x + self.w && y >= self.y && y <= self.y + self.h
    }
}

/// What `updateCommandInfo` decided the field currently holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommandInfo {
    /// The field is blank: `pendingSuggestions = null`, so nothing is offered
    /// and any open popup is already gone.
    Blank,
    /// Ordinary chat. The suggestions are computed locally and are already in
    /// [`CommandSuggestions::pending`].
    Message,
    /// The field starts with `/`. Rewo has no client-side dispatcher, so the
    /// local answer is unavailable; the string is the input up to the cursor,
    /// which is what a caller sends to the server. See the module docs.
    Command(String),
}

/// `CommandSuggestions`.
pub struct CommandSuggestions {
    cfg: SuggestionsConfig,
    /// `allowSuggestions` — false until the first edit. `ChatScreen.init`
    /// calls `setAllowSuggestions(false)` and only `onEdited` turns it on, so
    /// a freshly opened chat box shows no popup even with a restored draft.
    allow_suggestions: bool,
    /// `allowHiding`. Defaults **true** in the field initialiser, and
    /// `ChatScreen.init` sets it **false** — which is the whole reason Tab
    /// can force the popup open when nothing is visible.
    allow_hiding: bool,
    /// `keepSuggestions` — set while `useSuggestion` writes the field, so the
    /// resulting edit does not tear down the list that is applying it.
    keep_suggestions: bool,
    /// `pendingSuggestions`, already resolved. Rewo has no futures; the
    /// server path completes it from a packet and the local path from a
    /// builder, so the "is it done yet" test collapses to `is_some`.
    pending: Option<Suggestions>,
    list: Option<SuggestionsList>,
}

impl CommandSuggestions {
    pub fn new(cfg: SuggestionsConfig) -> Self {
        Self {
            cfg,
            allow_suggestions: false,
            allow_hiding: true,
            keep_suggestions: false,
            pending: None,
            list: None,
        }
    }

    /// `ChatScreen.init`'s pair of calls, as one constructor.
    pub fn for_chat() -> Self {
        let mut s = Self::new(SuggestionsConfig::CHAT);
        s.set_allow_hiding(false);
        s.set_allow_suggestions(false);
        s
    }

    /// `setAllowSuggestions` — turning it **off also drops the list**.
    pub fn set_allow_suggestions(&mut self, allow: bool) {
        self.allow_suggestions = allow;
        if !allow {
            self.list = None;
        }
    }

    pub fn set_allow_hiding(&mut self, allow: bool) {
        self.allow_hiding = allow;
    }

    pub fn is_visible(&self) -> bool {
        self.list.is_some()
    }

    pub fn list(&self) -> Option<&SuggestionsList> {
        self.list.as_ref()
    }

    /// The ten constructor arguments, for a renderer that needs the row limit
    /// and the fill colour.
    pub fn config(&self) -> SuggestionsConfig {
        self.cfg
    }

    /// `hide`.
    pub fn hide(&mut self) {
        self.list = None;
    }

    pub fn pending(&self) -> Option<&Suggestions> {
        self.pending.as_ref()
    }

    /// The server's reply, or a locally built set, arriving as
    /// `pendingSuggestions`.
    pub fn set_pending(&mut self, suggestions: Option<Suggestions>) {
        self.pending = suggestions;
    }

    /// `updateCommandInfo`, minus everything that needs a dispatcher.
    ///
    /// The order matters: the list is torn down **before** the branch runs
    /// (unless `keepSuggestions`), so a keystroke always drops the old popup
    /// even if the new branch produces nothing.
    ///
    /// `!command.isBlank()` guards the message branch — a field holding only
    /// spaces offers nothing, where a naive `is_empty` would offer the whole
    /// player list.
    pub fn update_command_info<'a>(
        &mut self,
        edit: &mut EditBox,
        tab_words: impl IntoIterator<Item = &'a str>,
    ) -> CommandInfo {
        if !self.keep_suggestions {
            edit.set_suggestion(None);
            self.list = None;
        }
        self.refresh_pending(edit, tab_words)
    }

    /// `updateCommandInfo` **minus** the teardown — what vanilla runs when
    /// `keepSuggestions` is set.
    ///
    /// It exists because vanilla fires the field's responder from *inside*
    /// `EditBox.setValue`, so `useSuggestion`'s own write re-enters
    /// `updateCommandInfo` while `keepSuggestions` is still true: the pending
    /// set is recomputed against the new text and the list doing the writing
    /// survives. Rewo has no responder, so the caller runs this *after*
    /// `useSuggestion` returns instead. The call order differs; the effect
    /// does not.
    pub fn refresh_pending<'a>(
        &mut self,
        edit: &mut EditBox,
        tab_words: impl IntoIterator<Item = &'a str>,
    ) -> CommandInfo {
        let units = edit.value_utf16();
        let cursor = edit.cursor_position().min(units.len());
        let value = String::from_utf16_lossy(&units);
        if value.starts_with('/') {
            // `commandsOnly || startsWithSlash`. Rewo has no dispatcher, so
            // the local half is unavailable; the caller may ask the server.
            self.pending = None;
            return CommandInfo::Command(String::from_utf16_lossy(&units[..cursor]));
        }
        if value.trim().is_empty() {
            self.pending = None;
            return CommandInfo::Blank;
        }
        let last_word = last_word_index(&units[..cursor]);
        let mut builder = SuggestionsBuilder::new(&units[..cursor], last_word);
        suggest_matching(tab_words, &mut builder);
        self.pending = Some(builder.build());
        CommandInfo::Message
    }

    /// `updateUsageInfo`'s tail: `suggestions = null;` then, when auto-suggest
    /// is on, `showSuggestions(false)`.
    ///
    /// Split out because the rest of `updateUsageInfo` is the dispatcher's.
    pub fn auto_show(
        &mut self,
        edit: &mut EditBox,
        metrics: InputMetrics,
        auto_suggestions: bool,
        width: &dyn Fn(&str) -> i32,
    ) {
        self.list = None;
        if self.allow_suggestions && auto_suggestions {
            self.show_suggestions(edit, metrics, width);
        }
    }

    /// `showSuggestions`.
    ///
    /// A resolved-but-**empty** set leaves the popup closed without clearing
    /// `pending`, which is why the guard is on `isEmpty()` and not on the
    /// option.
    pub fn show_suggestions(
        &mut self,
        edit: &mut EditBox,
        metrics: InputMetrics,
        width: &dyn Fn(&str) -> i32,
    ) {
        let Some(pending) = self.pending.clone() else {
            return;
        };
        if pending.is_empty() {
            return;
        }
        let max_width = pending
            .list
            .iter()
            .map(|s| width(&s.text))
            .max()
            .unwrap_or(0);
        let x = clamp_i32(
            screen_x(edit, metrics.x, pending.range.start, width),
            0,
            screen_x(edit, metrics.x, 0, width) + metrics.inner_width - max_width,
        );
        let y = if self.cfg.anchor_to_bottom {
            metrics.screen_height - 12
        } else {
            72
        };
        let sorted = sort_suggestions(edit, &pending);
        self.list = Some(SuggestionsList::new(
            self.cfg, x, y, max_width, sorted, edit,
        ));
    }

    /// `CommandSuggestions.keyPressed`.
    ///
    /// The second half reads
    /// `if (getFocused() != input || !isCycleFocus() || allowHiding && !isVisible) return false;`
    /// — so with `allowHiding` **false**, which is what `ChatScreen.init`
    /// sets, Tab reaches `showSuggestions(true)` even when nothing is on
    /// screen. Leaving `allowHiding` at its `true` default would make Tab a
    /// no-op until the popup had already appeared by itself.
    pub fn key_pressed(
        &mut self,
        input: Input,
        edit: &mut EditBox,
        metrics: InputMetrics,
        width: &dyn Fn(&str) -> i32,
    ) -> bool {
        let visible = self.list.is_some();
        if visible {
            let mut list = self.list.take().expect("checked");
            let handled = list.key_pressed(input, edit, self.cfg, &mut self.keep_suggestions);
            // `hide()` inside the list's Esc branch clears the field; put it
            // back only if it survived.
            if !list.hidden {
                self.list = Some(list);
            }
            if handled {
                return true;
            }
        }
        if !is_cycle_focus(input) || (self.allow_hiding && !visible) {
            return false;
        }
        self.show_suggestions(edit, metrics, width);
        true
    }

    /// `CommandSuggestions.mouseScrolled` — the clamp to ±1 happens **here**,
    /// before the list sees it, so one notch is one row however large the
    /// device's delta.
    pub fn mouse_scrolled(&mut self, scroll: f64, mouse: (i32, i32)) -> bool {
        let cfg = self.cfg;
        match self.list.as_mut() {
            Some(list) => list.mouse_scrolled(scroll.clamp(-1.0, 1.0), mouse, cfg),
            None => false,
        }
    }

    /// `CommandSuggestions.mouseClicked`.
    pub fn mouse_clicked(&mut self, x: i32, y: i32, edit: &mut EditBox) -> bool {
        let cfg = self.cfg;
        let Some(mut list) = self.list.take() else {
            return false;
        };
        let handled = list.mouse_clicked(x, y, edit, cfg, &mut self.keep_suggestions);
        self.list = Some(list);
        handled
    }

    /// The hover half of `extractRenderState`: moving the mouse over a row
    /// selects it, and **only** when it moved — hovering without moving does
    /// not steal the keyboard's selection.
    pub fn mouse_moved(&mut self, mouse: (i32, i32), edit: &mut EditBox) {
        let cfg = self.cfg;
        if let Some(list) = self.list.as_mut() {
            list.update_hover(mouse, edit, cfg);
        }
    }
}

/// `SuggestionsList`.
pub struct SuggestionsList {
    pub rect: Rect,
    /// `originalContents` — the field's value when the list was built, which
    /// every `Suggestion.apply` is measured against rather than the live
    /// value.
    original_contents: Vec<u16>,
    list: Vec<Suggestion>,
    offset: usize,
    current: usize,
    last_mouse: (i32, i32),
    /// `tabCycles` — false until a suggestion has been applied, so the first
    /// Tab fills and the next one moves.
    tab_cycles: bool,
    /// Set by the Esc branch so the owner can drop us; vanilla calls
    /// `CommandSuggestions.this.hide()` from inside the inner class.
    hidden: bool,
}

impl SuggestionsList {
    fn new(
        cfg: SuggestionsConfig,
        x: i32,
        y: i32,
        width: i32,
        list: Vec<Suggestion>,
        edit: &mut EditBox,
    ) -> Self {
        let shown = list.len().min(cfg.suggestion_line_limit) as i32;
        let list_x = x - if cfg.bordered { 0 } else { 1 };
        let list_y = if cfg.anchor_to_bottom {
            y - 3 - shown * LINE_HEIGHT
        } else {
            y - if cfg.bordered { 1 } else { 0 }
        };
        let mut me = Self {
            rect: Rect {
                x: list_x,
                y: list_y,
                w: width + 1,
                h: shown * LINE_HEIGHT,
            },
            original_contents: edit.value_utf16(),
            list,
            offset: 0,
            current: 0,
            last_mouse: (0, 0),
            tab_cycles: false,
            hidden: false,
        };
        me.select(0, edit);
        me
    }

    pub fn entries(&self) -> &[Suggestion] {
        &self.list
    }

    pub fn offset(&self) -> usize {
        self.offset
    }

    pub fn current(&self) -> usize {
        self.current
    }

    pub fn tab_cycles(&self) -> bool {
        self.tab_cycles
    }

    /// How many rows are drawn: `min(size, suggestionLineLimit)`.
    pub fn shown(&self, cfg: SuggestionsConfig) -> usize {
        self.list.len().min(cfg.suggestion_line_limit)
    }

    /// `select` — wraps **once** in each direction rather than taking a
    /// modulus. Equivalent for the ±1 steps `cycle` makes and for the
    /// in-range indices the constructor and the click pass, and not
    /// equivalent for anything larger; transcribed as written.
    fn select(&mut self, index: isize, edit: &mut EditBox) {
        let len = self.list.len() as isize;
        let mut current = index;
        if current < 0 {
            current += len;
        }
        if current >= len {
            current -= len;
        }
        self.current = current.max(0) as usize;
        let Some(suggestion) = self.list.get(self.current) else {
            return;
        };
        let applied = suggestion.apply(&self.original_contents);
        edit.set_suggestion(calculate_suggestion_suffix(&edit.value_utf16(), &applied));
    }

    /// `useSuggestion`.
    ///
    /// The cursor lands at `range.start + text.length()` — the end of the
    /// **inserted text**, not the end of the field, so completing a word in
    /// the middle of a line leaves the caret in the middle.
    ///
    /// `keepSuggestions` brackets the write so that the responder this fires
    /// does not tear down the list doing the writing.
    fn use_suggestion(&mut self, edit: &mut EditBox, keep: &mut bool) {
        let Some(suggestion) = self.list.get(self.current).cloned() else {
            return;
        };
        *keep = true;
        let applied = suggestion.apply(&self.original_contents);
        edit.set_value(&String::from_utf16_lossy(&applied));
        let end = suggestion.range.start + suggestion.text.encode_utf16().count();
        edit.set_cursor_position(end);
        edit.set_highlight_pos(end);
        self.select(self.current as isize, edit);
        *keep = false;
        self.tab_cycles = true;
    }

    /// `cycle` — move the selection, then scroll the window to it.
    fn cycle(&mut self, direction: isize, edit: &mut EditBox, cfg: SuggestionsConfig) {
        self.select(self.current as isize + direction, edit);
        let limit = cfg.suggestion_line_limit;
        let max_offset = self.list.len().saturating_sub(limit) as i32;
        let current = self.current as i32;
        let first = self.offset as i32;
        let last = first + limit as i32 - 1;
        if current < first {
            self.offset = clamp_i32(current, 0, max_offset).max(0) as usize;
        } else if current > last {
            self.offset =
                clamp_i32(current + cfg.line_start_offset - limit as i32, 0, max_offset).max(0)
                    as usize;
        }
    }

    fn key_pressed(
        &mut self,
        input: Input,
        edit: &mut EditBox,
        cfg: SuggestionsConfig,
        keep: &mut bool,
    ) -> bool {
        match input.key {
            KEY_UP => {
                self.cycle(-1, edit, cfg);
                self.tab_cycles = false;
                true
            }
            KEY_DOWN => {
                self.cycle(1, edit, cfg);
                self.tab_cycles = false;
                true
            }
            _ if is_cycle_focus(input) => {
                // The first Tab FILLS; only once something has been applied
                // does Tab move. Cycling on the first press would skip the
                // top entry, which is the one the popup is showing as
                // selected.
                if self.tab_cycles {
                    self.cycle(if input.has_shift() { -1 } else { 1 }, edit, cfg);
                }
                self.use_suggestion(edit, keep);
                true
            }
            KEY_ESCAPE => {
                self.hidden = true;
                edit.set_suggestion(None);
                true
            }
            _ => false,
        }
    }

    fn mouse_clicked(
        &mut self,
        x: i32,
        y: i32,
        edit: &mut EditBox,
        _cfg: SuggestionsConfig,
        keep: &mut bool,
    ) -> bool {
        if !self.rect.contains(x, y) {
            return false;
        }
        // `contains` admits `rect.y + height`, so this can name a row one past
        // the window — which, on a list longer than the window, is a real
        // entry that is not on screen. Guarded only against the list's own
        // bounds, exactly as vanilla is.
        let line = (y - self.rect.y) / LINE_HEIGHT + self.offset as i32;
        if line >= 0 && (line as usize) < self.list.len() {
            self.select(line as isize, edit);
            self.use_suggestion(edit, keep);
        }
        true
    }

    fn mouse_scrolled(&mut self, scroll: f64, mouse: (i32, i32), cfg: SuggestionsConfig) -> bool {
        if !self.rect.contains(mouse.0, mouse.1) {
            return false;
        }
        let max_offset = self.list.len().saturating_sub(cfg.suggestion_line_limit) as i32;
        self.offset = clamp_i32(self.offset as i32 - scroll as i32, 0, max_offset).max(0) as usize;
        true
    }

    /// The hover-selection half of `extractRenderState`.
    ///
    /// The row test is strictly inside the rect on x, which is not what
    /// `contains` does — see the module docs.
    fn update_hover(&mut self, mouse: (i32, i32), edit: &mut EditBox, cfg: SuggestionsConfig) {
        let (mx, my) = mouse;
        let moved = self.last_mouse != mouse;
        if moved {
            self.last_mouse = mouse;
        }
        if !moved {
            return;
        }
        let limit = self.shown(cfg);
        for i in 0..limit {
            let top = self.rect.y + LINE_HEIGHT * i as i32;
            if mx > self.rect.x
                && mx < self.rect.x + self.rect.w
                && my > top
                && my < top + LINE_HEIGHT
            {
                self.select((i + self.offset) as isize, edit);
                return;
            }
        }
    }
}

/// `CommandSuggestions.getLastWordIndex` — the end of the **last** run of
/// whitespace, or 0.
///
/// Not "the index after the last space": the pattern is `(\s+)` and the result
/// is `matcher.end()`, so a run of three spaces puts the word start after all
/// three. Taking `rfind(' ') + 1` agrees on single spaces and is one short on
/// every double one.
pub fn last_word_index(text: &[u16]) -> usize {
    let mut result = 0usize;
    let mut i = 0usize;
    while i < text.len() {
        if is_java_whitespace(text[i]) {
            let mut j = i;
            while j < text.len() && is_java_whitespace(text[j]) {
                j += 1;
            }
            result = j;
            i = j;
        } else {
            i += 1;
        }
    }
    result
}

/// `\s` in `java.util.regex` with no UNICODE_CHARACTER_CLASS flag is exactly
/// `[ \t\n\x0B\f\r]` — an **ASCII-only** class. A Unicode-aware whitespace
/// test would split words at a non-breaking space where Java does not.
fn is_java_whitespace(u: u16) -> bool {
    matches!(u, 0x20 | 0x09 | 0x0A | 0x0B | 0x0C | 0x0D)
}

/// `CommandSuggestions.calculateSuggestionSuffix` — the greyed ghost drawn
/// after the caret.
///
/// `null` when the applied result does not start with what is in the field,
/// which is how the ghost disappears the moment you type something the
/// selected entry cannot complete.
pub fn calculate_suggestion_suffix(contents: &[u16], suggestion: &[u16]) -> Option<String> {
    if suggestion.starts_with(contents) {
        Some(String::from_utf16_lossy(&suggestion[contents.len()..]))
    } else {
        None
    }
}

/// `CommandSuggestions.sortSuggestions` — partition into "starts with the
/// word being typed" and "everything else", preserving order within each.
///
/// See the module docs on why the comparison is asymmetric in case.
pub fn sort_suggestions(edit: &EditBox, suggestions: &Suggestions) -> Vec<Suggestion> {
    let units = edit.value_utf16();
    let cursor = edit.cursor_position().min(units.len());
    let partial = &units[..cursor];
    let last_word = String::from_utf16_lossy(&partial[last_word_index(partial)..]).to_lowercase();
    let namespaced = format!("minecraft:{last_word}");
    let mut matching = Vec::new();
    let mut rest = Vec::new();
    for s in &suggestions.list {
        if s.text.starts_with(&last_word) || s.text.starts_with(&namespaced) {
            matching.push(s.clone());
        } else {
            rest.push(s.clone());
        }
    }
    matching.extend(rest);
    matching
}

/// `EditBox.getScreenX` — `charIndex > value.length() ? getX() : getX() +
/// font.width(value.substring(0, charIndex))`.
///
/// The test is **`>`**, so an index exactly at the end measures the whole
/// string rather than falling back to the box's left edge.
fn screen_x(edit: &EditBox, box_x: i32, char_index: usize, width: &dyn Fn(&str) -> i32) -> i32 {
    let units = edit.value_utf16();
    if char_index > units.len() {
        box_x
    } else {
        box_x + width(&String::from_utf16_lossy(&units[..char_index]))
    }
}

/// `Mth.clamp(int, int, int)` — `Math.min(Math.max(value, min), max)`.
///
/// **Returns `max` when `max < min`**, which `max(min(v, max), min)` would not,
/// and which is reachable here whenever a suggestion is wider than the field.
fn clamp_i32(value: i32, min: i32, max: i32) -> i32 {
    value.max(min).min(max)
}

/// GLFW `KEY_ESCAPE`.
pub const KEY_ESCAPE: i32 = 256;
/// GLFW `KEY_TAB`.
pub const KEY_TAB: i32 = 258;
/// GLFW `KEY_UP` / `KEY_DOWN`, as [`crate::chat_screen`] names them.
pub const KEY_UP: i32 = 265;
pub const KEY_DOWN: i32 = 264;

/// `KeyEvent.isCycleFocus` — Tab, with or without shift.
pub fn is_cycle_focus(input: Input) -> bool {
    input.key == KEY_TAB
}

#[cfg(test)]
mod tests {
    use super::*;

    fn units(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    /// A width function of 6 px per UTF-16 unit, so every expectation below is
    /// arithmetic a reader can check rather than a font measurement.
    fn w(s: &str) -> i32 {
        s.encode_utf16().count() as i32 * 6
    }

    fn metrics() -> InputMetrics {
        InputMetrics {
            x: 4,
            inner_width: 316,
            screen_height: 240,
        }
    }

    fn field(value: &str) -> EditBox {
        let mut e = EditBox::new(256);
        e.set_value(value);
        e
    }

    fn k(key: i32) -> Input {
        Input { key, modifiers: 0 }
    }

    fn suggestions(texts: &[&str], start: usize, end: usize) -> Suggestions {
        use crate::suggestions::StringRange;
        let range = StringRange::between(start, end);
        Suggestions {
            range,
            list: texts
                .iter()
                .map(|t| Suggestion::new(range, *t))
                .collect(),
        }
    }

    fn open(cs: &mut CommandSuggestions, edit: &mut EditBox, s: Suggestions) {
        cs.set_allow_suggestions(true);
        cs.set_pending(Some(s));
        cs.show_suggestions(edit, metrics(), &w);
    }

    // ── getLastWordIndex ─────────────────────────────────────────────────

    #[test]
    fn the_last_word_starts_after_the_whole_run_of_whitespace() {
        // `matcher.end()` of the LAST `(\s+)` match. `rfind(' ') + 1` agrees
        // on a single space and is short by two on a triple one.
        assert_eq!(last_word_index(&units("hello wo")), 6);
        assert_eq!(last_word_index(&units("hello   wo")), 8);
        assert_eq!(last_word_index(&units("nospace")), 0);
        assert_eq!(last_word_index(&units("")), 0);
        // Trailing whitespace puts the word start at the very end, so the
        // suggestion replaces nothing and is inserted.
        assert_eq!(last_word_index(&units("hello ")), 6);
    }

    #[test]
    fn tabs_and_newlines_count_and_a_non_breaking_space_does_not() {
        // `\s` without UNICODE_CHARACTER_CLASS is ASCII-only.
        assert_eq!(last_word_index(&units("a\tb")), 2);
        assert_eq!(last_word_index(&units("a\u{00A0}b")), 0);
    }

    // ── the ghost suffix ─────────────────────────────────────────────────

    #[test]
    fn the_ghost_is_the_tail_beyond_what_is_typed_and_vanishes_when_it_diverges() {
        assert_eq!(
            calculate_suggestion_suffix(&units("Ste"), &units("Steve")),
            Some("ve".to_string())
        );
        assert_eq!(
            calculate_suggestion_suffix(&units("Stx"), &units("Steve")),
            None
        );
        // An exact match leaves an empty ghost, which is not the same as none.
        assert_eq!(
            calculate_suggestion_suffix(&units("Steve"), &units("Steve")),
            Some(String::new())
        );
    }

    // ── sortSuggestions ──────────────────────────────────────────────────

    #[test]
    fn entries_beginning_with_the_typed_word_come_first_in_their_existing_order() {
        let mut e = field("hello st");
        e.set_cursor_position(8);
        let s = suggestions(&["other", "stone", "another", "stick"], 6, 8);
        let sorted = sort_suggestions(&e, &s);
        let texts: Vec<&str> = sorted.iter().map(|x| x.text.as_str()).collect();
        assert_eq!(texts, ["stone", "stick", "other", "another"]);
    }

    #[test]
    fn a_capitalised_entry_is_demoted_even_though_it_matched() {
        // The asymmetry: only the needle is lower-cased, so `startsWith` is
        // case-sensitive against raw text. Lower-casing both sides is the
        // obvious tidy-up and reorders every mixed-case list.
        let mut e = field("st");
        e.set_cursor_position(2);
        let s = suggestions(&["Steve", "stone"], 0, 2);
        let sorted = sort_suggestions(&e, &s);
        let texts: Vec<&str> = sorted.iter().map(|x| x.text.as_str()).collect();
        assert_eq!(texts, ["stone", "Steve"]);
    }

    #[test]
    fn a_namespaced_id_is_promoted_by_the_separate_minecraft_prefix_test() {
        // The only place a namespace is considered; `matches_sub_str`
        // deliberately does not split on `:`.
        let mut e = field("stone");
        e.set_cursor_position(5);
        let s = suggestions(&["zzz", "minecraft:stone"], 0, 5);
        let sorted = sort_suggestions(&e, &s);
        assert_eq!(sorted[0].text, "minecraft:stone");
    }

    // ── positioning ──────────────────────────────────────────────────────

    #[test]
    fn the_popup_sits_above_the_field_and_one_pixel_left_of_the_word() {
        // listX = x - 1 because the chat field is unbordered;
        // listY = (height - 12) - 3 - rows*12.
        let mut e = field("hello wo");
        e.set_cursor_position(8);
        let mut cs = CommandSuggestions::for_chat();
        open(&mut cs, &mut e, suggestions(&["world", "wolf"], 6, 8));
        let rect = cs.list().unwrap().rect;
        // getScreenX(6) = 4 + width("hello ") = 4 + 36 = 40, then -1.
        assert_eq!(rect.x, 39);
        assert_eq!(rect.y, 240 - 12 - 3 - 2 * 12);
        // width + 1, where width is the widest entry ("world" = 30).
        assert_eq!((rect.w, rect.h), (31, 24));
    }

    #[test]
    fn a_suggestion_wider_than_the_field_pins_the_popup_off_the_left_edge() {
        // Mth.clamp is min(max(v, min), max), so max < min returns MAX — a
        // negative x. Writing the clamp the other way round pins it to 0 and
        // the popup would sit under the field's left edge instead.
        let mut e = field("");
        let mut cs = CommandSuggestions::for_chat();
        let huge = "x".repeat(100);
        open(&mut cs, &mut e, suggestions(&[huge.as_str()], 0, 0));
        // 4 + 316 - 600 = -280, then the unbordered -1.
        assert_eq!(cs.list().unwrap().rect.x, -281);
    }

    #[test]
    fn only_the_first_ten_rows_are_measured_into_the_height() {
        let mut e = field("");
        let mut cs = CommandSuggestions::for_chat();
        let many: Vec<String> = (0..25).map(|i| format!("n{i}")).collect();
        let refs: Vec<&str> = many.iter().map(String::as_str).collect();
        open(&mut cs, &mut e, suggestions(&refs, 0, 0));
        assert_eq!(cs.list().unwrap().rect.h, 10 * LINE_HEIGHT);
        assert_eq!(cs.list().unwrap().entries().len(), 25);
    }

    #[test]
    fn an_empty_set_opens_nothing_and_keeps_the_pending_slot() {
        let mut e = field("");
        let mut cs = CommandSuggestions::for_chat();
        cs.set_allow_suggestions(true);
        cs.set_pending(Some(Suggestions::empty()));
        cs.show_suggestions(&mut e, metrics(), &w);
        assert!(!cs.is_visible());
        assert!(cs.pending().is_some());
    }

    // ── selection and cycling ────────────────────────────────────────────

    #[test]
    fn opening_selects_the_first_entry_and_writes_its_ghost() {
        let mut e = field("Ste");
        e.set_cursor_position(3);
        let mut cs = CommandSuggestions::for_chat();
        open(&mut cs, &mut e, suggestions(&["Steve", "Steven"], 0, 3));
        assert_eq!(cs.list().unwrap().current(), 0);
        assert_eq!(e.suggestion(), Some("ve"));
    }

    #[test]
    fn up_from_the_first_entry_wraps_to_the_last() {
        let mut e = field("");
        let mut cs = CommandSuggestions::for_chat();
        open(&mut cs, &mut e, suggestions(&["a", "b", "c"], 0, 0));
        cs.key_pressed(k(KEY_UP), &mut e, metrics(), &w);
        assert_eq!(cs.list().unwrap().current(), 2);
        cs.key_pressed(k(KEY_DOWN), &mut e, metrics(), &w);
        assert_eq!(cs.list().unwrap().current(), 0);
    }

    #[test]
    fn cycling_down_past_the_window_keeps_the_selected_row_as_its_last_line() {
        // `current + lineStartOffset - limit` with the chat screen's
        // lineStartOffset of 1 gives `current - 9`, so row `current` is the
        // tenth and last visible. With 0 it would be one past the bottom.
        let mut e = field("");
        let mut cs = CommandSuggestions::for_chat();
        let many: Vec<String> = (0..20).map(|i| format!("n{i}")).collect();
        let refs: Vec<&str> = many.iter().map(String::as_str).collect();
        open(&mut cs, &mut e, suggestions(&refs, 0, 0));
        for _ in 0..10 {
            cs.key_pressed(k(KEY_DOWN), &mut e, metrics(), &w);
        }
        let list = cs.list().unwrap();
        assert_eq!(list.current(), 10);
        assert_eq!(list.offset(), 1);
        assert!(list.current() >= list.offset() && list.current() < list.offset() + 10);
    }

    #[test]
    fn cycling_up_past_the_window_puts_the_selected_row_first() {
        let mut e = field("");
        let mut cs = CommandSuggestions::for_chat();
        let many: Vec<String> = (0..20).map(|i| format!("n{i}")).collect();
        let refs: Vec<&str> = many.iter().map(String::as_str).collect();
        open(&mut cs, &mut e, suggestions(&refs, 0, 0));
        // Wrap upwards to the very end, then walk back up into the window.
        cs.key_pressed(k(KEY_UP), &mut e, metrics(), &w);
        assert_eq!(cs.list().unwrap().current(), 19);
        assert_eq!(cs.list().unwrap().offset(), 10);
        for _ in 0..10 {
            cs.key_pressed(k(KEY_UP), &mut e, metrics(), &w);
        }
        let list = cs.list().unwrap();
        assert_eq!(list.current(), 9);
        assert_eq!(list.offset(), 9);
    }

    // ── applying ─────────────────────────────────────────────────────────

    #[test]
    fn the_first_tab_fills_and_the_next_one_moves() {
        // `tabCycles` is false until something has been applied, so the first
        // Tab takes the entry the popup is already showing as selected rather
        // than skipping it.
        let mut e = field("Ste");
        e.set_cursor_position(3);
        let mut cs = CommandSuggestions::for_chat();
        open(&mut cs, &mut e, suggestions(&["Steve", "Steven"], 0, 3));
        cs.key_pressed(k(KEY_TAB), &mut e, metrics(), &w);
        assert_eq!(e.value(), "Steve");
        cs.key_pressed(k(KEY_TAB), &mut e, metrics(), &w);
        assert_eq!(e.value(), "Steven");
    }

    #[test]
    fn shift_tab_moves_backwards_once_cycling_has_begun() {
        let mut e = field("");
        let mut cs = CommandSuggestions::for_chat();
        open(&mut cs, &mut e, suggestions(&["a", "b", "c"], 0, 0));
        cs.key_pressed(k(KEY_TAB), &mut e, metrics(), &w);
        assert_eq!(e.value(), "a");
        let shift = Input {
            key: KEY_TAB,
            modifiers: crate::edit_box::modifier::SHIFT,
        };
        cs.key_pressed(shift, &mut e, metrics(), &w);
        assert_eq!(e.value(), "c");
    }

    #[test]
    fn applying_leaves_the_caret_at_the_end_of_the_inserted_text_not_the_field() {
        // `range.start + text.length()`, so completing a word mid-line leaves
        // the caret mid-line.
        let mut e = field("say wo there");
        e.set_cursor_position(6);
        let mut cs = CommandSuggestions::for_chat();
        open(&mut cs, &mut e, suggestions(&["world"], 4, 6));
        cs.key_pressed(k(KEY_TAB), &mut e, metrics(), &w);
        assert_eq!(e.value(), "say world there");
        assert_eq!(e.cursor_position(), 9);
    }

    #[test]
    fn escape_hides_the_popup_and_clears_the_ghost() {
        let mut e = field("Ste");
        e.set_cursor_position(3);
        let mut cs = CommandSuggestions::for_chat();
        open(&mut cs, &mut e, suggestions(&["Steve"], 0, 3));
        assert!(e.suggestion().is_some());
        assert!(cs.key_pressed(k(KEY_ESCAPE), &mut e, metrics(), &w));
        assert!(!cs.is_visible());
        assert_eq!(e.suggestion(), None);
    }

    #[test]
    fn an_unrelated_key_falls_through_to_the_caller() {
        let mut e = field("");
        let mut cs = CommandSuggestions::for_chat();
        open(&mut cs, &mut e, suggestions(&["a"], 0, 0));
        assert!(!cs.key_pressed(k(70), &mut e, metrics(), &w));
    }

    #[test]
    fn tab_forces_the_popup_open_because_the_chat_screen_disables_hiding() {
        // `allowHiding && !isVisible` is the third disjunct of the early-out;
        // ChatScreen.init sets allowHiding false, so it never fires and Tab
        // reaches showSuggestions. Left at its `true` default, Tab would be a
        // no-op until the popup had appeared by itself.
        let mut e = field("Ste");
        e.set_cursor_position(3);
        let mut cs = CommandSuggestions::for_chat();
        cs.set_allow_suggestions(true);
        cs.set_pending(Some(suggestions(&["Steve"], 0, 3)));
        assert!(!cs.is_visible());
        assert!(cs.key_pressed(k(KEY_TAB), &mut e, metrics(), &w));
        assert!(cs.is_visible());

        let mut cs = CommandSuggestions::new(SuggestionsConfig::CHAT);
        cs.set_allow_suggestions(true);
        cs.set_pending(Some(suggestions(&["Steve"], 0, 3)));
        assert!(!cs.key_pressed(k(KEY_TAB), &mut e, metrics(), &w));
        assert!(!cs.is_visible());
    }

    // ── the mouse ────────────────────────────────────────────────────────

    #[test]
    fn a_click_inside_a_row_applies_it() {
        let mut e = field("");
        let mut cs = CommandSuggestions::for_chat();
        open(&mut cs, &mut e, suggestions(&["a", "b", "c"], 0, 0));
        let rect = cs.list().unwrap().rect;
        assert!(cs.mouse_clicked(rect.x + 2, rect.y + LINE_HEIGHT + 2, &mut e));
        assert_eq!(e.value(), "b");
    }

    #[test]
    fn a_click_outside_the_rect_is_not_consumed() {
        let mut e = field("");
        let mut cs = CommandSuggestions::for_chat();
        open(&mut cs, &mut e, suggestions(&["a"], 0, 0));
        let rect = cs.list().unwrap().rect;
        assert!(!cs.mouse_clicked(rect.x - 1, rect.y, &mut e));
    }

    #[test]
    fn a_click_one_pixel_below_the_popup_applies_an_entry_that_is_not_on_screen() {
        // `Rect2i.contains` is inclusive on the bottom, so y = rect.y + height
        // is inside; the row arithmetic then names `limit + offset`, which on
        // a list longer than the window is a real, invisible entry. Vanilla's,
        // and only guarded against the list's own bounds.
        let mut e = field("");
        let mut cs = CommandSuggestions::for_chat();
        let many: Vec<String> = (0..20).map(|i| format!("n{i}")).collect();
        let refs: Vec<&str> = many.iter().map(String::as_str).collect();
        open(&mut cs, &mut e, suggestions(&refs, 0, 0));
        let rect = cs.list().unwrap().rect;
        assert!(cs.mouse_clicked(rect.x + 2, rect.y + rect.h, &mut e));
        assert_eq!(e.value(), "n10");
    }

    #[test]
    fn the_wheel_scrolls_the_window_only_while_the_cursor_is_over_it() {
        let mut e = field("");
        let mut cs = CommandSuggestions::for_chat();
        let many: Vec<String> = (0..20).map(|i| format!("n{i}")).collect();
        let refs: Vec<&str> = many.iter().map(String::as_str).collect();
        open(&mut cs, &mut e, suggestions(&refs, 0, 0));
        let rect = cs.list().unwrap().rect;
        // A large delta still moves one row: the clamp is applied before the
        // list sees it.
        assert!(cs.mouse_scrolled(-9.0, (rect.x + 2, rect.y + 2)));
        assert_eq!(cs.list().unwrap().offset(), 1);
        assert!(!cs.mouse_scrolled(-1.0, (rect.x - 5, rect.y - 5)));
        assert_eq!(cs.list().unwrap().offset(), 1);
    }

    #[test]
    fn hovering_selects_a_row_only_when_the_mouse_actually_moved() {
        // `mouseMoved` guards the re-select, so resting the pointer over the
        // popup does not fight the arrow keys.
        let mut e = field("");
        let mut cs = CommandSuggestions::for_chat();
        open(&mut cs, &mut e, suggestions(&["a", "b", "c"], 0, 0));
        let rect = cs.list().unwrap().rect;
        let over_row_2 = (rect.x + 2, rect.y + 2 * LINE_HEIGHT + 2);
        cs.mouse_moved(over_row_2, &mut e);
        assert_eq!(cs.list().unwrap().current(), 2);
        // Move the keyboard selection, then report the SAME position again.
        cs.key_pressed(k(KEY_UP), &mut e, metrics(), &w);
        assert_eq!(cs.list().unwrap().current(), 1);
        cs.mouse_moved(over_row_2, &mut e);
        assert_eq!(cs.list().unwrap().current(), 1);
    }

    #[test]
    fn the_leftmost_column_is_clickable_and_never_highlights() {
        // `contains` uses `>=` on the left edge and the hover test uses `>`.
        let mut e = field("");
        let mut cs = CommandSuggestions::for_chat();
        open(&mut cs, &mut e, suggestions(&["a", "b"], 0, 0));
        let rect = cs.list().unwrap().rect;
        cs.mouse_moved((rect.x, rect.y + LINE_HEIGHT + 2), &mut e);
        assert_eq!(cs.list().unwrap().current(), 0, "hover excludes rect.x");
        assert!(cs.mouse_clicked(rect.x, rect.y + LINE_HEIGHT + 2, &mut e));
        assert_eq!(e.value(), "b", "the click includes it");
    }

    // ── updateCommandInfo ────────────────────────────────────────────────

    #[test]
    fn a_plain_word_is_completed_from_the_tab_word_list() {
        let mut e = field("hi ste");
        e.set_cursor_position(6);
        let mut cs = CommandSuggestions::for_chat();
        assert_eq!(
            cs.update_command_info(&mut e, ["Steve", "Alex", "Stephanie"]),
            CommandInfo::Message
        );
        let texts: Vec<&str> = cs
            .pending()
            .unwrap()
            .list
            .iter()
            .map(|s| s.text.as_str())
            .collect();
        assert_eq!(texts, ["Stephanie", "Steve"]);
    }

    #[test]
    fn a_blank_field_offers_nothing_even_when_it_holds_spaces() {
        // `!command.isBlank()`, not `!isEmpty()` — a naive emptiness test
        // would offer the whole player list for a field holding two spaces.
        let mut cs = CommandSuggestions::for_chat();
        for value in ["", "   "] {
            let mut e = field(value);
            assert_eq!(
                cs.update_command_info(&mut e, ["Steve"]),
                CommandInfo::Blank
            );
            assert!(cs.pending().is_none());
        }
    }

    #[test]
    fn a_slash_takes_the_command_branch_and_reports_the_text_up_to_the_cursor() {
        let mut e = field("/give @s stone");
        e.set_cursor_position(8);
        let mut cs = CommandSuggestions::for_chat();
        assert_eq!(
            cs.update_command_info(&mut e, ["Steve"]),
            CommandInfo::Command("/give @s".to_string())
        );
        assert!(cs.pending().is_none());
    }

    #[test]
    fn a_keystroke_drops_the_open_popup_unless_a_suggestion_is_being_applied() {
        let mut e = field("ste");
        e.set_cursor_position(3);
        let mut cs = CommandSuggestions::for_chat();
        open(&mut cs, &mut e, suggestions(&["Steve"], 0, 3));
        assert!(cs.is_visible());
        cs.update_command_info(&mut e, ["Steve"]);
        assert!(!cs.is_visible());
        assert_eq!(e.suggestion(), None);
    }

    #[test]
    fn turning_suggestions_off_also_drops_the_list() {
        let mut e = field("");
        let mut cs = CommandSuggestions::for_chat();
        open(&mut cs, &mut e, suggestions(&["a"], 0, 0));
        cs.set_allow_suggestions(false);
        assert!(!cs.is_visible());
    }

    #[test]
    fn auto_show_respects_both_the_option_and_the_allow_flag() {
        let mut e = field("ste");
        e.set_cursor_position(3);
        let mut cs = CommandSuggestions::for_chat();
        cs.set_pending(Some(suggestions(&["Steve"], 0, 3)));
        // allow_suggestions is false until the first edit.
        cs.auto_show(&mut e, metrics(), true, &w);
        assert!(!cs.is_visible());
        cs.set_allow_suggestions(true);
        cs.auto_show(&mut e, metrics(), false, &w);
        assert!(!cs.is_visible(), "the autoSuggestions option is off");
        cs.auto_show(&mut e, metrics(), true, &w);
        assert!(cs.is_visible());
    }
}
