//! `EditBox` — the text-entry subsystem Rewo has never had (M93t).
//!
//! M93n shipped the anvil's *semantics* — the filter, the length rule, the
//! change gate — and recorded that nothing could type, because Rewo reads
//! `PhysicalKey`/`KeyCode` and never a character. This is the missing half:
//! `net.minecraft.client.gui.components.EditBox`'s editing core, transcribed.
//!
//! # The buffer is UTF-16, and that is not a detail
//!
//! Every index in vanilla's `EditBox` is a Java `String` index — a **UTF-16
//! code unit**. `value.length()`, `substring(start, end)`, `charAt`,
//! `Util.offsetByCodepoints`, and `maxLength` all count code units, and one of
//! the rules is only *expressible* in UTF-16:
//!
//! ```java
//! if (Character.isHighSurrogate(text.charAt(maxInsertionLength - 1))) {
//!    maxInsertionLength--;
//! }
//! ```
//!
//! That exists to stop a truncation splitting a surrogate pair. Modelling the
//! buffer as a Rust `String` would make it a different (if analogous)
//! operation and every index a conversion, so the value is a `Vec<u16>` and
//! the conversions happen only at the edges. M93n already counted the anvil's
//! 50 in code units for the same reason.
//!
//! # What is NOT here
//!
//! The **clipboard is supplied by the caller** as a plain `String` rather than
//! read from the OS: Rewo pulls in no clipboard crate, and `winit` exposes
//! none. Copy and cut write to it and paste reads it, so the semantics are
//! exact and the transport is a stub — an in-process clipboard, which is
//! useful on its own and swaps for a real one at one call site.
//!
//! IME pre-edit (`preeditUpdated`), the hint, the suggestion and the formatter
//! list are all omitted; the anvil sets none of them.

/// GLFW key codes, the namespace `KeyEvent.key()` speaks.
pub mod key {
    pub const BACKSPACE: i32 = 259;
    pub const DELETE: i32 = 261;
    pub const RIGHT: i32 = 262;
    pub const LEFT: i32 = 263;
    pub const HOME: i32 = 268;
    pub const END: i32 = 269;
    pub const A: i32 = 65;
    pub const C: i32 = 67;
    pub const V: i32 = 86;
    pub const X: i32 = 88;
}

/// GLFW modifier bits, as `InputWithModifiers` reads them.
pub mod modifier {
    pub const SHIFT: i32 = 1;
    pub const CONTROL: i32 = 2;
    pub const ALT: i32 = 4;
    /// `InputQuirks.EDIT_SHORTCUT_KEY_MODIFIER` — **`SUPER` (8) on macOS and
    /// `CONTROL` (2) everywhere else**. Rewo targets Windows and Linux, and
    /// macOS is out of scope for the whole project, so this is 2.
    pub const EDIT_SHORTCUT: i32 = CONTROL;
}

/// `InputWithModifiers`' predicates, over a `(key, modifiers)` pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Input {
    pub key: i32,
    pub modifiers: i32,
}

impl Input {
    pub fn new(key: i32, modifiers: i32) -> Self {
        Self { key, modifiers }
    }

    pub fn has_shift(self) -> bool {
        self.modifiers & modifier::SHIFT != 0
    }

    pub fn has_alt(self) -> bool {
        self.modifiers & modifier::ALT != 0
    }

    /// `hasControlDownWithQuirk`.
    pub fn has_control(self) -> bool {
        self.modifiers & modifier::EDIT_SHORTCUT != 0
    }

    /// The four editing shortcuts share one shape, and **all three modifier
    /// conditions matter**: the edit key down, and shift and alt *up*. So
    /// `Ctrl+Shift+C` is not a copy — it falls through and the box returns
    /// false, letting the screen have it.
    fn shortcut(self, key: i32) -> bool {
        self.key == key && self.has_control() && !self.has_shift() && !self.has_alt()
    }

    pub fn is_select_all(self) -> bool {
        self.shortcut(key::A)
    }
    pub fn is_copy(self) -> bool {
        self.shortcut(key::C)
    }
    pub fn is_paste(self) -> bool {
        self.shortcut(key::V)
    }
    pub fn is_cut(self) -> bool {
        self.shortcut(key::X)
    }
}

/// `StringUtil.isAllowedChatCharacter` — shared with [`crate::anvil`], which
/// documents why 167 is excluded.
fn allowed(ch: char) -> bool {
    crate::anvil::is_allowed_chat_character(ch)
}

/// `net.minecraft.client.gui.components.EditBox`'s editing core.
#[derive(Debug, Clone)]
pub struct EditBox {
    /// The value, as **UTF-16 code units** — see the module docs.
    value: Vec<u16>,
    cursor_pos: usize,
    highlight_pos: usize,
    display_pos: usize,
    max_length: usize,
    editable: bool,
    focused: bool,
    active: bool,
    /// Whether `onValueChange` has fired since the caller last looked.
    ///
    /// Vanilla's `setResponder` takes a `Consumer<String>` called from exactly
    /// three places — `insertText` (only when there was room), `deleteCharsToPos`
    /// (only when the range was non-empty) and `setValue` (**always**). The
    /// last is why this is a flag rather than a before/after comparison:
    /// `setValue("")` on an already-empty box fires the responder, and the
    /// anvil's `onNameChanged` depends on that firing at `subInit`.
    value_changed: bool,
}

impl Default for EditBox {
    fn default() -> Self {
        Self::new(32)
    }
}

impl EditBox {
    pub fn new(max_length: usize) -> Self {
        Self {
            value: Vec::new(),
            cursor_pos: 0,
            highlight_pos: 0,
            display_pos: 0,
            max_length,
            editable: true,
            focused: false,
            active: true,
            value_changed: false,
        }
    }

    pub fn value(&self) -> String {
        String::from_utf16_lossy(&self.value)
    }

    /// The value's length in the unit every index here uses.
    pub fn len(&self) -> usize {
        self.value.len()
    }

    pub fn is_empty(&self) -> bool {
        self.value.is_empty()
    }

    pub fn cursor_position(&self) -> usize {
        self.cursor_pos
    }

    pub fn highlight_position(&self) -> usize {
        self.highlight_pos
    }

    pub fn display_pos(&self) -> usize {
        self.display_pos
    }

    pub fn is_editable(&self) -> bool {
        self.editable
    }

    pub fn set_editable(&mut self, editable: bool) {
        self.editable = editable;
    }

    pub fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
    }

    pub fn is_focused(&self) -> bool {
        self.focused
    }

    /// `getHighlighted` — the selected run, either way round.
    pub fn highlighted(&self) -> String {
        let (a, b) = self.selection();
        String::from_utf16_lossy(&self.value[a..b])
    }

    fn selection(&self) -> (usize, usize) {
        (
            self.cursor_pos.min(self.highlight_pos),
            self.cursor_pos.max(self.highlight_pos),
        )
    }

    /// `setValue` — truncate to `maxLength`, then cursor to the END and the
    /// highlight with it.
    ///
    /// **The truncation is `substring(0, maxLength)` with no surrogate check**,
    /// unlike `insertText`'s — so a programmatic `setValue` can split a pair
    /// where a paste cannot. That asymmetry is vanilla's.
    pub fn set_value(&mut self, value: &str) {
        let units: Vec<u16> = value.encode_utf16().collect();
        self.value = if units.len() > self.max_length {
            units[..self.max_length].to_vec()
        } else {
            units
        };
        self.move_cursor_to_end(false);
        self.set_highlight_pos(self.cursor_pos);
        // `setValue` calls `onValueChange` unconditionally — even when the
        // value did not change.
        self.value_changed = true;
    }

    pub fn set_max_length(&mut self, max_length: usize) {
        self.max_length = max_length;
        if self.value.len() > max_length {
            self.value.truncate(max_length);
            self.value_changed = true;
        }
    }

    pub fn max_length(&self) -> usize {
        self.max_length
    }

    /// `insertText` — replace the selection, honouring the room left.
    ///
    /// ```java
    /// int maxInsertionLength = this.maxLength - this.value.length() - (start - end);
    /// ```
    ///
    /// A **double negative**: `start <= end`, so `-(start - end)` *adds back*
    /// the room the selection frees. Reading it as a subtraction makes a
    /// replacement of a long selection refuse a short insert.
    pub fn insert_text(&mut self, input: &str) {
        let (start, end) = self.selection();
        let room = (self.max_length + end).saturating_sub(self.value.len() + start);
        if room == 0 {
            return;
        }
        let mut text: Vec<u16> = input
            .chars()
            .filter(|c| allowed(*c))
            .flat_map(|c| {
                let mut b = [0u16; 2];
                c.encode_utf16(&mut b).to_vec()
            })
            .collect();
        if room < text.len() {
            let mut cut = room;
            // Never split a surrogate pair: if the last unit kept is a HIGH
            // surrogate its partner is about to be dropped, so drop it too.
            if cut > 0 && (0xD800..0xDC00).contains(&text[cut - 1]) {
                cut -= 1;
            }
            text.truncate(cut);
        }
        let inserted = text.len();
        self.value.splice(start..end, text);
        self.set_cursor_position(start + inserted);
        self.set_highlight_pos(self.cursor_pos);
        self.value_changed = true;
    }

    /// `deleteText` — `wholeWord` picks the word form.
    pub fn delete_text(&mut self, dir: i32, whole_word: bool) {
        if whole_word {
            self.delete_words(dir);
        } else {
            self.delete_chars(dir);
        }
    }

    /// `deleteWords` — but **a selection wins**: with anything highlighted,
    /// Ctrl+Backspace deletes the selection and not a word.
    pub fn delete_words(&mut self, dir: i32) {
        if self.value.is_empty() {
            return;
        }
        if self.highlight_pos != self.cursor_pos {
            self.insert_text("");
        } else {
            self.delete_chars_to_pos(self.word_position(dir));
        }
    }

    pub fn delete_chars(&mut self, dir: i32) {
        self.delete_chars_to_pos(self.cursor_pos_offset(dir));
    }

    /// `deleteCharsToPos`.
    pub fn delete_chars_to_pos(&mut self, pos: usize) {
        if self.value.is_empty() {
            return;
        }
        if self.highlight_pos != self.cursor_pos {
            self.insert_text("");
            return;
        }
        let (start, end) = (pos.min(self.cursor_pos), pos.max(self.cursor_pos));
        if start != end {
            self.value.drain(start..end);
            self.set_cursor_position(start);
            self.value_changed = true;
            self.move_cursor_to(start, false);
        }
    }

    /// `getWordPosition(dir, from, stripSpaces = true)`.
    ///
    /// Forward and backward are **not mirror images**. Forward finds the next
    /// space and then skips the run of spaces, landing at the start of the next
    /// word. Backward skips any spaces first and then walks back over
    /// non-spaces, landing at the start of the word it was in — so pressing
    /// Ctrl+Left then Ctrl+Right from mid-word does not return you to where you
    /// began.
    pub fn word_position(&self, dir: i32) -> usize {
        self.word_position_from(dir, self.cursor_pos)
    }

    fn word_position_from(&self, dir: i32, from: usize) -> usize {
        const SPACE: u16 = 32;
        let mut result = from;
        let reverse = dir < 0;
        for _ in 0..dir.unsigned_abs() {
            if !reverse {
                let length = self.value.len();
                match self.value[result.min(length)..].iter().position(|&c| c == SPACE) {
                    None => result = length,
                    Some(off) => {
                        result += off;
                        while result < length && self.value[result] == SPACE {
                            result += 1;
                        }
                    }
                }
            } else {
                while result > 0 && self.value[result - 1] == SPACE {
                    result -= 1;
                }
                while result > 0 && self.value[result - 1] != SPACE {
                    result -= 1;
                }
            }
        }
        result
    }

    /// `Util.offsetByCodepoints` — the cursor steps a whole **codepoint**, so
    /// one press crosses a surrogate pair rather than landing inside it.
    fn cursor_pos_offset(&self, dir: i32) -> usize {
        let mut pos = self.cursor_pos;
        for _ in 0..dir.unsigned_abs() {
            if dir > 0 {
                if pos >= self.value.len() {
                    break;
                }
                pos += if (0xD800..0xDC00).contains(&self.value[pos]) && pos + 1 < self.value.len() {
                    2
                } else {
                    1
                };
            } else {
                if pos == 0 {
                    break;
                }
                pos -= if pos >= 2 && (0xDC00..0xE000).contains(&self.value[pos - 1]) {
                    2
                } else {
                    1
                };
            }
        }
        pos.min(self.value.len())
    }

    pub fn move_cursor(&mut self, dir: i32, extend_selection: bool) {
        self.move_cursor_to(self.cursor_pos_offset(dir), extend_selection);
    }

    pub fn move_cursor_to(&mut self, pos: usize, extend_selection: bool) {
        self.set_cursor_position(pos);
        if !extend_selection {
            self.set_highlight_pos(self.cursor_pos);
        }
    }

    pub fn set_cursor_position(&mut self, pos: usize) {
        self.cursor_pos = pos.min(self.value.len());
    }

    pub fn set_highlight_pos(&mut self, pos: usize) {
        self.highlight_pos = pos.min(self.value.len());
    }

    pub fn move_cursor_to_start(&mut self, extend_selection: bool) {
        self.move_cursor_to(0, extend_selection);
    }

    pub fn move_cursor_to_end(&mut self, extend_selection: bool) {
        self.move_cursor_to(self.value.len(), extend_selection);
    }

    /// `canConsumeInput` — active **and** focused **and** editable.
    pub fn can_consume_input(&self) -> bool {
        self.active && self.focused && self.editable
    }

    /// `charTyped` — gated on `canConsumeInput`, then on the chat filter.
    ///
    /// Returns whether the character was consumed. A disallowed one is **not**,
    /// so `§` reaches the screen behind the box rather than vanishing.
    pub fn char_typed(&mut self, ch: char) -> bool {
        if !self.can_consume_input() {
            return false;
        }
        if !allowed(ch) {
            return false;
        }
        if self.editable {
            self.insert_text(&ch.to_string());
        }
        true
    }

    /// `keyPressed`, with the clipboard supplied.
    ///
    /// Two things read backwards. The whole switch is gated on `isActive() &&
    /// isFocused()` — **not** on `isEditable()` — and backspace and delete
    /// `return true` from *outside* their `if (this.isEditable)`, so an
    /// uneditable box still **swallows** them. And `case 260/264/265/266/267`
    /// (insert, the vertical arrows, page up/down) share the `default` label,
    /// so they are treated exactly as unrecognised keys: they fall through the
    /// clipboard checks and return false, letting the screen have them.
    pub fn key_pressed(&mut self, input: Input, clipboard: &mut String) -> bool {
        if !(self.active && self.focused) {
            return false;
        }
        match input.key {
            key::BACKSPACE => {
                if self.editable {
                    self.delete_text(-1, input.has_control());
                }
                true
            }
            key::DELETE => {
                if self.editable {
                    self.delete_text(1, input.has_control());
                }
                true
            }
            key::RIGHT => {
                if input.has_control() {
                    self.move_cursor_to(self.word_position(1), input.has_shift());
                } else {
                    self.move_cursor(1, input.has_shift());
                }
                true
            }
            key::LEFT => {
                if input.has_control() {
                    self.move_cursor_to(self.word_position(-1), input.has_shift());
                } else {
                    self.move_cursor(-1, input.has_shift());
                }
                true
            }
            key::HOME => {
                self.move_cursor_to_start(input.has_shift());
                true
            }
            key::END => {
                self.move_cursor_to_end(input.has_shift());
                true
            }
            _ => {
                if input.is_select_all() {
                    // Cursor to the END and the highlight to 0 — so a
                    // subsequent Left collapses to 0 and a Right to the end.
                    self.move_cursor_to_end(false);
                    self.set_highlight_pos(0);
                    true
                } else if input.is_copy() {
                    *clipboard = self.highlighted();
                    true
                } else if input.is_paste() {
                    if self.editable {
                        let text = clipboard.clone();
                        self.insert_text(&text);
                    }
                    true
                } else if input.is_cut() {
                    // The copy happens whether or not the box is editable;
                    // only the removal is gated.
                    *clipboard = self.highlighted();
                    if self.editable {
                        self.insert_text("");
                    }
                    true
                } else {
                    false
                }
            }
        }
    }

    /// `scrollTo` — keep `pos` inside the visible run.
    ///
    /// `width` measures a run of code units in pixels; `inner_width` is the
    /// box's. Vanilla takes both from its `Font`, which lives in `rewo-gpu`, so
    /// they are injected rather than depended on.
    pub fn scroll_to(&mut self, pos: usize, inner_width: i32, width: &dyn Fn(&[u16]) -> i32) {
        self.display_pos = self.display_pos.min(self.value.len());
        let displayed = plain_substr_by_width(&self.value[self.display_pos..], inner_width, width);
        let last_pos = displayed + self.display_pos;
        if pos == self.display_pos {
            // `plainSubstrByWidth(value, innerWidth, /* tail */ true)` — a run
            // measured from the END, so scrolling left jumps by a screenful
            // rather than a character.
            self.display_pos = self
                .display_pos
                .saturating_sub(plain_substr_by_width_tail(&self.value, inner_width, width));
        }
        if pos > last_pos {
            self.display_pos += pos - last_pos;
        } else if pos <= self.display_pos {
            self.display_pos -= self.display_pos - pos;
        }
        self.display_pos = self.display_pos.min(self.value.len());
    }

    /// The run that fits, from `display_pos`.
    pub fn displayed(&self, inner_width: i32, width: &dyn Fn(&[u16]) -> i32) -> &[u16] {
        let n = plain_substr_by_width(&self.value[self.display_pos..], inner_width, width);
        &self.value[self.display_pos..self.display_pos + n]
    }

    /// The responder — `Some(value)` when `onValueChange` fired since the last
    /// call, draining the flag.
    ///
    /// A flag rather than a `Consumer<String>` because the callback would have
    /// to borrow the caller's session mutably while the box is borrowed from
    /// it. The firing SITES are vanilla's exactly, which is the part that
    /// matters: a change that produces the same string still fires.
    pub fn take_value_changed(&mut self) -> Option<String> {
        std::mem::take(&mut self.value_changed).then(|| self.value())
    }

    /// The raw buffer, for a renderer that needs to measure sub-runs.
    pub fn units(&self) -> &[u16] {
        &self.value
    }
}

/// `Font.plainSubstrByWidth` — how many leading code units fit in `max_width`.
pub fn plain_substr_by_width(s: &[u16], max_width: i32, width: &dyn Fn(&[u16]) -> i32) -> usize {
    let mut n = 0;
    while n < s.len() && width(&s[..n + 1]) <= max_width {
        n += 1;
    }
    n
}

/// The same measured from the **end** — `plainSubstrByWidth(s, w, true)`.
pub fn plain_substr_by_width_tail(s: &[u16], max_width: i32, width: &dyn Fn(&[u16]) -> i32) -> usize {
    let mut n = 0;
    while n < s.len() && width(&s[s.len() - n - 1..]) <= max_width {
        n += 1;
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    fn boxed(v: &str) -> EditBox {
        let mut b = EditBox::new(50);
        b.set_focused(true);
        b.set_value(v);
        b
    }

    /// A monospace 6-px font, so the scroll maths is arithmetic rather than a
    /// table lookup.
    fn mono(s: &[u16]) -> i32 {
        s.len() as i32 * 6
    }

    #[test]
    fn set_value_puts_the_cursor_at_the_end_and_collapses_the_selection() {
        let b = boxed("hello");
        assert_eq!(b.value(), "hello");
        assert_eq!(b.cursor_position(), 5);
        assert_eq!(b.highlight_position(), 5);
        assert_eq!(b.highlighted(), "");
    }

    #[test]
    fn insert_text_adds_back_the_room_a_selection_frees() {
        // `maxLength - length - (start - end)` is a DOUBLE NEGATIVE: the
        // selection's width is added back. A full box with three characters
        // selected has room for three.
        let mut b = EditBox::new(5);
        b.set_focused(true);
        b.set_value("abcde");
        b.set_cursor_position(1);
        b.set_highlight_pos(4);
        b.insert_text("XYZ");
        assert_eq!(b.value(), "aXYZe");
        // …and reading it as a subtraction would have refused this outright.
        let mut c = EditBox::new(5);
        c.set_value("abcde");
        c.insert_text("Z");
        assert_eq!(c.value(), "abcde", "a full box with no selection takes nothing");
    }

    #[test]
    fn insert_text_never_splits_a_surrogate_pair() {
        // Four units of room, and a three-emoji paste is six units. Truncating
        // at four would keep a lone HIGH surrogate; vanilla drops it.
        let mut b = EditBox::new(4);
        b.set_focused(true);
        b.insert_text("😀😀😀");
        assert_eq!(b.value(), "😀😀", "two whole pairs, not two and a half");
        assert_eq!(b.len(), 4);

        let mut c = EditBox::new(3);
        c.set_focused(true);
        c.insert_text("😀😀");
        assert_eq!(c.value(), "😀", "3 units of room takes one pair, not one-and-a-half");
        assert_eq!(c.len(), 2);
    }

    #[test]
    fn set_value_truncates_WITHOUT_the_surrogate_check_that_insert_has() {
        // The asymmetry is vanilla's: `setValue` is `substring(0, maxLength)`
        // flat. A programmatic set can split a pair where a paste cannot.
        let mut b = EditBox::new(3);
        b.set_value("😀😀");
        assert_eq!(b.len(), 3, "a lone surrogate survives here");
        assert_ne!(b.len(), 2, "which is NOT what insert_text would have done");
    }

    #[test]
    fn the_filter_runs_on_insert() {
        let mut b = boxed("");
        b.insert_text("a\u{a7}b\u{7f}c\u{1}");
        assert_eq!(b.value(), "abc", "§, DEL and a control char are dropped");
    }

    #[test]
    fn a_disallowed_character_is_not_consumed() {
        // So it reaches the screen behind the box rather than vanishing.
        let mut b = boxed("");
        assert!(b.char_typed('a'));
        assert!(!b.char_typed('\u{a7}'));
        assert_eq!(b.value(), "a");
    }

    #[test]
    fn char_typed_needs_all_three_of_active_focused_editable() {
        let mut b = boxed("");
        b.set_focused(false);
        assert!(!b.char_typed('a'));
        b.set_focused(true);
        b.set_editable(false);
        assert!(!b.char_typed('a'));
        b.set_editable(true);
        assert!(b.char_typed('a'));
    }

    #[test]
    fn an_uneditable_box_still_SWALLOWS_backspace() {
        // `return true` sits outside `if (this.isEditable)`, and the switch is
        // gated on focus rather than editability — so the key is consumed and
        // the screen never sees it, even though nothing changed.
        let mut b = boxed("abc");
        b.set_editable(false);
        let mut clip = String::new();
        assert!(b.key_pressed(Input::new(key::BACKSPACE, 0), &mut clip));
        assert_eq!(b.value(), "abc");
        assert!(b.key_pressed(Input::new(key::DELETE, 0), &mut clip));
        assert_eq!(b.value(), "abc");
    }

    #[test]
    fn the_vertical_arrows_are_treated_as_unrecognised_keys() {
        // 260/264/265/266/267 share the `default` label, so they fall through
        // to the clipboard checks and return FALSE — the screen gets them.
        let mut b = boxed("abc");
        let mut clip = String::new();
        for key in [260, 264, 265, 266, 267, 999] {
            assert!(!b.key_pressed(Input::new(key, 0), &mut clip), "key {key}");
        }
        // …while the horizontal ones are consumed.
        assert!(b.key_pressed(Input::new(key::LEFT, 0), &mut clip));
        assert!(b.key_pressed(Input::new(key::RIGHT, 0), &mut clip));
    }

    #[test]
    fn word_motion_is_not_symmetric() {
        // Forward lands at the START of the next word; backward lands at the
        // start of the word it was in. So out-and-back does not return.
        let mut b = boxed("one two three");
        b.set_cursor_position(5); // the 't' of "two"
        assert_eq!(b.word_position(1), 8, "past 'two' and its space");
        assert_eq!(b.word_position(-1), 4, "the start of 'two'");
        b.set_cursor_position(6);
        assert_eq!(b.word_position(-1), 4);
        assert_eq!(b.word_position(1), 8);
    }

    #[test]
    fn backward_word_motion_skips_trailing_spaces_first() {
        let mut b = boxed("one   ");
        b.set_cursor_position(6);
        assert_eq!(b.word_position(-1), 0, "over the spaces, then over 'one'");
    }

    #[test]
    fn a_selection_beats_a_word_delete() {
        // `deleteWords` checks the highlight FIRST, so Ctrl+Backspace with
        // something selected deletes the selection and not a word.
        let mut b = boxed("one two three");
        b.set_cursor_position(13);
        b.set_highlight_pos(8);
        let mut clip = String::new();
        b.key_pressed(Input::new(key::BACKSPACE, modifier::CONTROL), &mut clip);
        assert_eq!(b.value(), "one two ");
    }

    #[test]
    fn the_cursor_steps_a_whole_codepoint() {
        let mut b = boxed("a😀b");
        assert_eq!(b.len(), 4);
        b.set_cursor_position(0);
        b.move_cursor(1, false);
        assert_eq!(b.cursor_position(), 1);
        b.move_cursor(1, false);
        assert_eq!(b.cursor_position(), 3, "the pair is one step, not two");
        b.move_cursor(-1, false);
        assert_eq!(b.cursor_position(), 1, "and back the same way");
    }

    #[test]
    fn select_all_puts_the_cursor_at_the_end_and_the_highlight_at_zero() {
        let mut b = boxed("hello");
        b.set_cursor_position(2);
        b.set_highlight_pos(2);
        let mut clip = String::new();
        assert!(b.key_pressed(Input::new(key::A, modifier::CONTROL), &mut clip));
        assert_eq!((b.cursor_position(), b.highlight_position()), (5, 0));
        assert_eq!(b.highlighted(), "hello");
    }

    #[test]
    fn a_shortcut_needs_control_AND_no_shift_AND_no_alt() {
        let mut b = boxed("hello");
        let mut clip = String::new();
        // Ctrl+Shift+C is not a copy — it falls through and is not consumed.
        assert!(!b.key_pressed(
            Input::new(key::C, modifier::CONTROL | modifier::SHIFT),
            &mut clip
        ));
        assert!(!b.key_pressed(
            Input::new(key::C, modifier::CONTROL | modifier::ALT),
            &mut clip
        ));
        assert!(!b.key_pressed(Input::new(key::C, 0), &mut clip), "and bare C");
        assert_eq!(clip, "");
    }

    #[test]
    fn cut_copies_even_when_uneditable_and_removes_only_when_not() {
        let mut b = boxed("hello");
        b.set_cursor_position(0);
        b.set_highlight_pos(5);
        b.set_editable(false);
        let mut clip = String::new();
        assert!(b.key_pressed(Input::new(key::X, modifier::CONTROL), &mut clip));
        assert_eq!(clip, "hello", "the copy is ungated");
        assert_eq!(b.value(), "hello", "the removal is not");
        b.set_editable(true);
        b.key_pressed(Input::new(key::X, modifier::CONTROL), &mut clip);
        assert_eq!(b.value(), "");
    }

    #[test]
    fn paste_goes_through_the_same_filter_and_length_rule() {
        let mut b = EditBox::new(4);
        b.set_focused(true);
        let mut clip = String::from("a\u{a7}bcdef");
        assert!(b.key_pressed(Input::new(key::V, modifier::CONTROL), &mut clip));
        assert_eq!(b.value(), "abcd", "§ dropped, then cut to 4");
    }

    #[test]
    fn the_view_scrolls_to_keep_the_cursor_visible() {
        // 60 px of a 6-px font is ten units.
        let mut b = EditBox::new(50);
        b.set_focused(true);
        b.set_value("abcdefghijklmnop");
        assert_eq!(b.display_pos(), 0);
        b.scroll_to(16, 60, &mono);
        assert_eq!(b.display_pos(), 6, "the last ten units are shown");
        assert_eq!(String::from_utf16_lossy(b.displayed(60, &mono)), "ghijklmnop");
        // Back to the start.
        b.scroll_to(0, 60, &mono);
        assert_eq!(b.display_pos(), 0);
        assert_eq!(String::from_utf16_lossy(b.displayed(60, &mono)), "abcdefghij");
    }

    #[test]
    fn plain_substr_by_width_counts_from_each_end() {
        let s: Vec<u16> = "abcdefghij".encode_utf16().collect();
        assert_eq!(plain_substr_by_width(&s, 30, &mono), 5);
        assert_eq!(plain_substr_by_width_tail(&s, 30, &mono), 5);
        assert_eq!(plain_substr_by_width(&s, 0, &mono), 0);
        assert_eq!(plain_substr_by_width(&s, 999, &mono), 10);
    }
}
