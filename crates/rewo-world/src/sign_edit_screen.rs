//! `AbstractSignEditScreen` / `SignEditScreen` / `HangingSignEditScreen` —
//! the sign editor (M174).
//!
//! Opened by `open_sign_editor`; closed by Done, Esc, or the tick's validity
//! check — and **every exit commits**: vanilla sends `sign_update` from
//! `removed()`, which every path reaches, with no dirty check and no cancel.
//!
//! In 26.2 the sign in this screen is a **flat GUI blit** — `textures/gui/
//! signs/<wood>.png` (24x26, scale 3.9) and `textures/gui/hanging_signs/
//! <wood>.png` (16x16, scale 4.5) — not a 3D model, not a render-to-texture
//! of `SignRenderer` (that was the ≤1.20 shape; porting it here is wrong).
//! A wall sign blits only the top 12 of the 26 texture rows (the board;
//! the other 14 are the post), at the same origin.
//!
//! The text machinery is `TextFieldHelper` over the CURRENT line, not
//! `EditBox` — the load-bearing difference is the validator: a whole-line
//! candidate is tested against `font.width(s) <= getMaxTextLineWidth()`
//! (90 sign / 60 hanging — a PIXEL width, not a character count) and a
//! failing insert, paste included, is **rejected in its entirety, never
//! truncated** (`TextFieldHelper.insertText`, TextFieldHelper.java:120-131).
//! `EditBox` truncates, which is why it is not reused here.
//!
//! Cursor indices: vanilla's are Java-String UTF-16 code-unit indices; this
//! module stores byte indices stepped by `char` boundaries. Both step by
//! codepoints (`Util.offsetByCodepoints`), so the observable split — which
//! characters sit left of the caret, what a selection covers — is identical.

use crate::edit_box::{key, Input};
use crate::screen::WidgetId;

/// GLFW codes the screen itself binds (`KeyEvent.isUp/isDown/isConfirmation`).
pub const KEY_UP: i32 = 265;
pub const KEY_DOWN: i32 = 264;
pub const KEY_ENTER: i32 = 257;
pub const KEY_KP_ENTER: i32 = 335;

/// `SignBlockEntity.getTextLineHeight()` / `getMaxTextLineWidth()`
/// (SignBlockEntity.java:85-91) and the `HangingSignBlockEntity` overrides
/// (HangingSignBlockEntity.java:16-24). The same numbers as
/// `rewo_data::sign_states::{SIGN_LINE_HEIGHT, ..}` — the world renderer's
/// copies; an agreement test in `rewo-app` pins the pair (`rewo-world`
/// cannot depend on `rewo-data`).
pub const SIGN_LINE_HEIGHT: i32 = 10;
pub const SIGN_MAX_WIDTH: i32 = 90;
pub const HANGING_LINE_HEIGHT: i32 = 9;
pub const HANGING_MAX_WIDTH: i32 = 60;

/// `SignEditScreen.TEXT_SCALE` — the literal is **0.9765628F**, not the
/// "nicer" 0.9765625 = 1/1.024 (SignEditScreen.java:13). The hanging screen's
/// is 1.0 (HangingSignEditScreen.java:56).
pub const SIGN_TEXT_SCALE: f32 = 0.976_562_8;

/// `getSignYOffset()` — 90 for signs (SignEditScreen.java:27-30), 125 for
/// hanging (HangingSignEditScreen.java:65-68). The board carries a further
/// inner translate: +27 for signs, -13 for hanging.
pub const SIGN_Y_OFFSET: f32 = 90.0;
pub const HANGING_Y_OFFSET: f32 = 125.0;

/// The Done button: `bounds(width / 2 - 100, height / 4 + 144, 200, 20)`
/// (AbstractSignEditScreen.java:57-59). Note the y is `height/4 + 144`, not
/// a `height - N` anchor.
pub const DONE: WidgetId = 0;
/// The title label's widget id (app-built — centring needs the font width).
pub const TITLE_LABEL: WidgetId = 1;
/// `centeredText(font, title, width / 2, 40, -1)` — y 40, white
/// (AbstractSignEditScreen.java:110).
pub const TITLE_Y: i32 = 40;

/// `TextCursorUtils.isCursorVisible` — wall-clock ms since `init()`
/// (`cursorBlinkStartTime = Util.getMillis()`), 300 on / 300 off, starting
/// visible. NOT tick- or frame-driven (TextCursorUtils.java:19-21).
pub fn cursor_visible(ms_since_open: u64) -> bool {
    (ms_since_open / 300) % 2 == 0
}

/// Which of the three sign shapes is being edited. The screen class dispatch
/// is on the block-ENTITY class (`HangingSignBlockEntity` → hanging screen,
/// LocalPlayer.java:612-617); wall-vs-standing changes only the blitted
/// height (`PlainSignBlock.getAttachmentPoint(state) == WALL ? 12 : 26`,
/// SignEditScreen.java:23-24) — the hanging screen has no such split (the
/// chains are baked into its 16x16 texture).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignKind {
    Standing,
    Wall,
    Hanging,
}

impl SignKind {
    pub fn hanging(self) -> bool {
        self == SignKind::Hanging
    }
    pub fn line_height(self) -> i32 {
        if self.hanging() { HANGING_LINE_HEIGHT } else { SIGN_LINE_HEIGHT }
    }
    pub fn max_text_line_width(self) -> i32 {
        if self.hanging() { HANGING_MAX_WIDTH } else { SIGN_MAX_WIDTH }
    }
    pub fn text_scale(self) -> f32 {
        if self.hanging() { 1.0 } else { SIGN_TEXT_SCALE }
    }
    pub fn y_offset(self) -> f32 {
        if self.hanging() { HANGING_Y_OFFSET } else { SIGN_Y_OFFSET }
    }
    /// `signMidpoint = 4 * lineHeight / 2` (AbstractSignEditScreen.java:173).
    pub fn midpoint(self) -> i32 {
        4 * self.line_height() / 2
    }
}

/// `ChatFormatting.stripFormatting` — `(?i)§[0-9A-FK-OR]` replaced with
/// nothing (ChatFormatting.java:32,47-48). A trailing bare `§` with no code
/// after it SURVIVES the strip — vanilla's quirk, kept: a pasted lone `§`
/// lands on the sign even though one can never be typed
/// (`isAllowedChatCharacter` rejects 167 at the char path only).
pub fn strip_formatting(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '§' {
            if let Some(&next) = chars.peek() {
                if next.is_ascii_alphanumeric()
                    && matches!(next.to_ascii_lowercase(), '0'..='9' | 'a'..='f' | 'k'..='o' | 'r')
                {
                    chars.next();
                    continue;
                }
            }
        }
        out.push(c);
    }
    out
}

/// `StringUtil.isAllowedChatCharacter` — `ch != 167 && ch >= 32 && ch != 127`
/// (StringUtil.java:62-64). Identical to `anvil::is_allowed_chat_character`;
/// re-exported so the sign path reads as its own transcription.
pub fn is_allowed_chat_character(ch: char) -> bool {
    crate::anvil::is_allowed_chat_character(ch)
}

/// `StringSplitter.getWordPosition(text, dir, from, stripSpaces = true)` over
/// byte indices — a word boundary is a space OR `\n` (StringSplitter.java:
/// 113-152; `\n` cannot appear on a sign line but the rule is transcribed
/// whole). Forward: skip to the next separator, step past it, then skip any
/// run of separators; backward: skip a separator run, then skip to the word's
/// start.
pub fn word_position(text: &str, dir: i32, from: usize) -> usize {
    let b = text.as_bytes();
    let sep = |i: usize| b[i] == b' ' || b[i] == b'\n';
    let mut result = from.min(b.len());
    let reverse = dir < 0;
    for _ in 0..dir.abs() {
        if reverse {
            while result > 0 && sep(result - 1) {
                result -= 1;
            }
            while result > 0 && !sep(result - 1) {
                result -= 1;
            }
        } else {
            // indexOf(' ') / indexOf('\n') from `result`, min of the found.
            let next = (result..b.len()).find(|&i| sep(i));
            match next {
                None => result = b.len(),
                Some(i) => {
                    result = i;
                    while result < b.len() && sep(result) {
                        result += 1;
                    }
                }
            }
        }
    }
    result
}

/// `Util.offsetByCodepoints` over byte indices — step by whole `char`s,
/// clamped at the ends.
pub fn offset_by_codepoints(text: &str, from: usize, count: i32) -> usize {
    let mut idx = from.min(text.len());
    if count >= 0 {
        for _ in 0..count {
            match text[idx..].chars().next() {
                Some(c) => idx += c.len_utf8(),
                None => break,
            }
        }
    } else {
        for _ in 0..(-count) {
            match text[..idx].chars().next_back() {
                Some(c) => idx -= c.len_utf8(),
                None => break,
            }
        }
    }
    idx
}

/// The editor's state: the four flattened lines (styling is lost at
/// construction — `getMessage(i, filter)` via `Component::getString`,
/// AbstractSignEditScreen.java:50 — and stays lost on edit, since
/// `setMessage` rebuilds with `Component.literal`), the current line, and
/// `TextFieldHelper`'s cursor + selection over it.
#[derive(Clone, Debug)]
pub struct SignEditState {
    pub lines: [String; 4],
    /// `this.line` — the row the field edits. Up: `(line - 1) & 3` (wraps);
    /// Down OR Enter OR keypad-Enter: `(line + 1) & 3`; both set the cursor
    /// to the new line's END (AbstractSignEditScreen.java:81-93). **Enter
    /// never closes the screen.**
    pub line: usize,
    /// Byte index into `lines[line]`.
    pub cursor: usize,
    /// The selection anchor. `selection == cursor` means no selection.
    pub selection: usize,
    pub kind: SignKind,
}

/// What a key press did — the caller needs `Close` (Esc reached `super`) to
/// run the commit path, and `Unhandled` so a fallen-through key can reach
/// the generic screen dispatch (vanilla's Delete does exactly that,
/// TextFieldHelper.java:89-91,113).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignKey {
    Handled,
    /// Esc — `onClose()` → `onDone()`; the packet still sends (from
    /// `removed()`), so this is a COMMIT, not a cancel.
    Close,
    Unhandled,
}

impl SignEditState {
    pub fn new(lines: [String; 4], kind: SignKind) -> Self {
        let cursor = lines[0].len();
        Self { lines, line: 0, cursor, selection: cursor, kind }
    }

    fn text(&self) -> &str {
        &self.lines[self.line]
    }

    /// `setCursorToEnd(selecting)`.
    pub fn set_cursor_to_end(&mut self, selecting: bool) {
        self.cursor = self.text().len();
        if !selecting {
            self.selection = self.cursor;
        }
    }

    /// `setCursorToStart(selecting)`.
    pub fn set_cursor_to_start(&mut self, selecting: bool) {
        self.cursor = 0;
        if !selecting {
            self.selection = self.cursor;
        }
    }

    /// The screen's `keyPressed` + the delegated `TextFieldHelper.keyPressed`.
    /// `width_fn` is the validator's font measure; `clipboard` the in-process
    /// clipboard (the `edit_box` convention).
    pub fn key_pressed(
        &mut self,
        input: Input,
        width_fn: &dyn Fn(&str) -> i32,
        clipboard: &mut String,
    ) -> SignKey {
        // The screen's own bindings run BEFORE the field's
        // (AbstractSignEditScreen.java:81-93).
        if input.key == KEY_UP {
            self.line = (self.line + 3) & 3;
            self.set_cursor_to_end(false);
            return SignKey::Handled;
        }
        if input.key == KEY_DOWN || input.key == KEY_ENTER || input.key == KEY_KP_ENTER {
            self.line = (self.line + 1) & 3;
            self.set_cursor_to_end(false);
            return SignKey::Handled;
        }
        if self.field_key(input, width_fn, clipboard) {
            return SignKey::Handled;
        }
        // `super.keyPressed` — Escape closes (Screen.java:119-124).
        if input.key == 256 {
            return SignKey::Close;
        }
        SignKey::Unhandled
    }

    /// `TextFieldHelper.keyPressed` (TextFieldHelper.java:62-114).
    fn field_key(
        &mut self,
        input: Input,
        width_fn: &dyn Fn(&str) -> i32,
        clipboard: &mut String,
    ) -> bool {
        if input.is_select_all() {
            self.selection = 0;
            self.cursor = self.text().len();
            return true;
        }
        if input.is_copy() {
            *clipboard = self.selected().to_string();
            return true;
        }
        if input.is_paste() {
            // The clipboard GETTER strips: `stripFormatting(clip.replaceAll(
            // "\\r", ""))` (TextFieldHelper.java:42-44); after the paste the
            // selection collapses (line 212-215).
            let text = strip_formatting(&clipboard.replace('\r', ""));
            self.insert_text(&text, width_fn);
            self.selection = self.cursor;
            return true;
        }
        if input.is_cut() {
            *clipboard = self.selected().to_string();
            let new = self.delete_selection();
            self.lines[self.line] = new;
            return true;
        }
        let word = input.has_control();
        match input.key {
            key::BACKSPACE => {
                self.remove_from_cursor(-1, word);
                true
            }
            // ⚠ Delete acts AND returns false — vanilla's `case 261` has no
            // `return true`, so the event also falls through to
            // `super.keyPressed` (harmless there; transcribed literally).
            key::DELETE => {
                self.remove_from_cursor(1, word);
                false
            }
            key::LEFT => {
                self.move_by(-1, input.has_shift(), word);
                true
            }
            key::RIGHT => {
                self.move_by(1, input.has_shift(), word);
                true
            }
            key::HOME => {
                self.set_cursor_to_start(input.has_shift());
                true
            }
            key::END => {
                self.set_cursor_to_end(input.has_shift());
                true
            }
            _ => false,
        }
    }

    /// The screen's `charTyped` — the field inserts only an allowed chat
    /// character, and the screen returns true EITHER WAY
    /// (AbstractSignEditScreen.java:96-99).
    pub fn char_typed(&mut self, ch: char, width_fn: &dyn Fn(&str) -> i32) -> bool {
        if is_allowed_chat_character(ch) {
            let mut buf = [0u8; 4];
            self.insert_text(ch.encode_utf8(&mut buf), width_fn);
        }
        true
    }

    /// `TextFieldHelper.insertText` — delete the selection, build the WHOLE
    /// candidate line, test it against the width validator, and commit only
    /// if it passes; a failing insert is a silent no-op (the cursor does not
    /// move) (TextFieldHelper.java:120-131). The validator is
    /// `font.width(s) <= getMaxTextLineWidth()` (AbstractSignEditScreen.java:
    /// 60-66).
    /// ⚠ `deleteSelection` is pure on the TEXT (it returns the updated
    /// string and collapses the cursor state; only `setMessageFn` commits) —
    /// so an insert-over-a-selection that FAILS the validator leaves the
    /// line's text intact, selected characters included, with only the
    /// selection collapsed. Odd, and vanilla's.
    pub fn insert_text(&mut self, text: &str, width_fn: &dyn Fn(&str) -> i32) {
        let mut message = self.text().to_string();
        if self.selection != self.cursor {
            message = self.delete_selection();
        }
        self.cursor = self.cursor.min(message.len());
        let candidate = format!("{}{}{}", &message[..self.cursor], text, &message[self.cursor..]);
        if width_fn(&candidate) <= self.kind.max_text_line_width() {
            self.cursor = (self.cursor + text.len()).min(candidate.len());
            self.selection = self.cursor;
            self.lines[self.line] = candidate;
        }
    }

    /// `moveBy(count, selecting, scope)`.
    pub fn move_by(&mut self, count: i32, selecting: bool, word: bool) {
        if word {
            self.cursor = word_position(self.text(), count, self.cursor);
        } else {
            self.cursor = offset_by_codepoints(self.text(), self.cursor, count);
        }
        if !selecting {
            self.selection = self.cursor;
        }
    }

    /// `removeFromCursor(count, scope)` → `removeCharsFromCursor` /
    /// `removeWordsFromCursor` (TextFieldHelper.java:187-216): with a
    /// selection, the selection is deleted whatever `count` says; otherwise
    /// the span from the cursor to the offset is deleted, and only a
    /// BACKWARD delete (`count < 0`) moves the cursor.
    ///
    /// ⚠ `removeWordsFromCursor` re-expresses the word span as
    /// `wordPosition - cursorPos` — a **UTF-16 code-unit delta** — and hands
    /// it to `removeCharsFromCursor`, which steps that many **codepoints**.
    /// On a word containing astral characters the two disagree and vanilla
    /// deletes PAST the word boundary; transcribed, not tidied.
    pub fn remove_from_cursor(&mut self, count: i32, word: bool) {
        if word {
            let target = word_position(self.text(), count, self.cursor);
            let (a, b) = (self.cursor.min(target), self.cursor.max(target));
            let units = self.text()[a..b].encode_utf16().count() as i32;
            let delta = if target < self.cursor { -units } else { units };
            self.remove_chars_from_cursor(delta);
        } else {
            self.remove_chars_from_cursor(count);
        }
    }

    /// `removeCharsFromCursor` (TextFieldHelper.java:198-216).
    fn remove_chars_from_cursor(&mut self, count: i32) {
        let message = self.text().to_string();
        if message.is_empty() {
            return;
        }
        if self.selection != self.cursor {
            let new = self.delete_selection();
            self.lines[self.line] = new;
        } else {
            let other = offset_by_codepoints(&message, self.cursor, count);
            let start = other.min(self.cursor);
            let end = other.max(self.cursor);
            let new = format!("{}{}", &message[..start], &message[end..]);
            if count < 0 {
                self.selection = start;
                self.cursor = start;
            }
            self.lines[self.line] = new;
        }
    }

    /// `getSelected`.
    pub fn selected(&self) -> &str {
        let a = self.cursor.min(self.selection).min(self.text().len());
        let b = self.cursor.max(self.selection).min(self.text().len());
        &self.text()[a..b]
    }

    /// `deleteSelection` — returns the new line text and collapses the
    /// cursor to the span's start.
    fn delete_selection(&mut self) -> String {
        let message = self.text().to_string();
        if self.selection == self.cursor {
            return message;
        }
        let start = self.cursor.min(self.selection);
        let end = self.cursor.max(self.selection);
        let updated = format!("{}{}", &message[..start], &message[end..]);
        self.selection = start;
        self.cursor = start;
        updated
    }
}

// ---------------------------------------------------------------------------
// Geometry — all in GUI pixels (f32; the pose scales are fractional).
// ---------------------------------------------------------------------------

/// The board blit's GUI rect. `extractSign` translates to `(width/2,
/// yOffset)`; the sign background then translates `(0, 27)`, scales 3.9 and
/// blits `(-12, -13, 24, wall ? 12 : 26)` (SignEditScreen.java:33-37); the
/// hanging one translates `(0, -13)`, scales 4.5 and blits `(-8, -8, 16,
/// 16)` (HangingSignEditScreen.java:71-75).
pub fn board_rect(kind: SignKind, gui_w: i32) -> (f32, f32, f32, f32) {
    let cx = gui_w as f32 / 2.0;
    match kind {
        SignKind::Standing | SignKind::Wall => {
            let rows = if kind == SignKind::Wall { 12.0 } else { 26.0 };
            (cx - 12.0 * 3.9, SIGN_Y_OFFSET + 27.0 - 13.0 * 3.9, 24.0 * 3.9, rows * 3.9)
        }
        SignKind::Hanging => (cx - 8.0 * 4.5, HANGING_Y_OFFSET - 13.0 - 8.0 * 4.5, 16.0 * 4.5, 16.0 * 4.5),
    }
}

/// [`board_rect`] rounded to the integer GUI pixels `SpriteDraw` carries —
/// nearest-integer on each edge, so the 93.6-wide sign board draws 94 GUI px
/// (a stated ≤0.6-px deviation; vanilla rasterises the fractional rect).
/// The hanging board is exactly integral (72x72 at -36/-72), so it rounds to
/// itself.
pub fn board_sprite(kind: SignKind, gui_w: i32) -> (i32, i32, i32, i32) {
    let (x, y, w, h) = board_rect(kind, gui_w);
    (
        x.round() as i32,
        y.round() as i32,
        w.round() as i32,
        h.round() as i32,
    )
}

/// One line's GUI-px anchor: vanilla draws line `i` at `x = -width(line)/2`,
/// `y = i*lineHeight - signMidpoint` inside the text-scaled pose centred on
/// `(width/2, yOffset)` (AbstractSignEditScreen.java:173-184). Returns the
/// line's left edge and top in GUI px.
pub fn line_origin(kind: SignKind, gui_w: i32, i: usize, line_width: i32) -> (f32, f32) {
    let ts = kind.text_scale();
    let x = gui_w as f32 / 2.0 + ts * (-(line_width as f32) / 2.0);
    let y = kind.y_offset() + ts * ((i as i32 * kind.line_height() - kind.midpoint()) as f32);
    (x, y)
}

/// Where the caret sits and which shape it takes: at or past the line's end
/// it is the `"_"` GLYPH; inside the line it is a 1-px-wide FILL from `y-1`
/// to `y + lineHeight` in text space, forced opaque (TextCursorUtils.java:
/// 11-17; AbstractSignEditScreen.java:186-207). Both at
/// `x = width(prefix) - width(line)/2` in text space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CaretDraw {
    /// GUI px of the underscore glyph's left/top (drawn at the line's y).
    Underscore { x: f32, y: f32 },
    /// GUI rect of the insert bar.
    Bar { x: f32, y: f32, w: f32, h: f32 },
}

pub fn caret_draw(
    state: &SignEditState,
    gui_w: i32,
    width_fn: &dyn Fn(&str) -> i32,
) -> CaretDraw {
    let kind = state.kind;
    let ts = kind.text_scale();
    let line = state.text();
    let cursor = state.cursor.min(line.len());
    let prefix_w = width_fn(&line[..cursor]) as f32;
    let line_w = width_fn(line) as f32;
    let cx = gui_w as f32 / 2.0 + ts * (prefix_w - line_w / 2.0);
    let y_text = (state.line as i32 * kind.line_height() - kind.midpoint()) as f32;
    if cursor >= line.len() {
        CaretDraw::Underscore { x: cx, y: kind.y_offset() + ts * y_text }
    } else {
        CaretDraw::Bar {
            x: cx,
            y: kind.y_offset() + ts * (y_text - 1.0),
            w: ts,
            h: ts * (kind.line_height() as f32 + 1.0),
        }
    }
}

/// The selection highlight's GUI rect, when a selection exists — endpoints
/// are the two substring widths minus `width/2`, min/maxed, over
/// `y .. y + lineHeight` (AbstractSignEditScreen.java:212-220). Vanilla
/// draws it OVER the text as a white GUI_INVERT fill then an opaque blue
/// `0xFF0000FF`; Rewo draws the blue UNDER the text (the screen pass has no
/// invert pipeline), a stated approximation — a blue box with the line's own
/// dark text instead of inverted white.
pub fn selection_rect(
    state: &SignEditState,
    gui_w: i32,
    width_fn: &dyn Fn(&str) -> i32,
) -> Option<(f32, f32, f32, f32)> {
    if state.selection == state.cursor {
        return None;
    }
    let kind = state.kind;
    let ts = kind.text_scale();
    let line = state.text();
    let a = state.cursor.min(state.selection).min(line.len());
    let b = state.cursor.max(state.selection).min(line.len());
    let line_w = width_fn(line) as f32;
    let x0 = width_fn(&line[..a]) as f32 - line_w / 2.0;
    let x1 = width_fn(&line[..b]) as f32 - line_w / 2.0;
    let y = (state.line as i32 * kind.line_height() - kind.midpoint()) as f32;
    let gx = gui_w as f32 / 2.0;
    Some((
        gx + ts * x0,
        kind.y_offset() + ts * y,
        ts * (x1 - x0),
        ts * kind.line_height() as f32,
    ))
}

/// `SELECTION_COLOR` — the opaque blue `-16776961` = `0xFF0000FF`
/// (GuiGraphicsExtractor.java:231-237).
pub const SELECTION_BLUE: [f32; 4] = [0.0, 0.0, 1.0, 1.0];

/// `Player.isWithinBlockInteractionRange(pos, 4.0)` inverted —
/// `AABB(pos).distanceToSqr(eye) < (blockInteractionRange() + buffer)^2` is
/// "within" (Player.java:2014-2017), and the editor's tick closes when NOT
/// within (`playerIsTooFarAwayToEdit`, SignBlockEntity.java:260-263).
/// ⚠ **The 4.0 is a BUFFER on the block-interaction-range ATTRIBUTE** (4.5
/// default → the editor survives to ~8.5 blocks), not a flat 4-block leash.
pub fn too_far_to_edit(
    eye: (f64, f64, f64),
    pos: (i32, i32, i32),
    block_interaction_range: f64,
) -> bool {
    let clamp_axis = |e: f64, lo: f64| -> f64 {
        if e < lo {
            lo - e
        } else if e > lo + 1.0 {
            e - (lo + 1.0)
        } else {
            0.0
        }
    };
    let dx = clamp_axis(eye.0, pos.0 as f64);
    let dy = clamp_axis(eye.1, pos.1 as f64);
    let dz = clamp_axis(eye.2, pos.2 as f64);
    let max_range = block_interaction_range + 4.0;
    !(dx * dx + dy * dy + dz * dz < max_range * max_range)
}

/// Build the framework [`crate::screen::Screen`]: the transparent-gradient
/// backdrop (`isInGameUi()` is true — the `0xC0101010 -> 0xD0101010`
/// gradient only, Screen.java:374-380,408-410) and the one standard widget,
/// Done at `(width/2 - 100, height/4 + 144, 200, 20)`. The title label is
/// added by the app (centring needs the font width — the options-title
/// pattern).
pub fn build_screen(gui_w: i32, gui_h: i32, done_label: &str) -> crate::screen::Screen {
    crate::screen::Screen::new(crate::screen::ScreenKind::SignEdit, gui_w, gui_h)
        .with_backdrop(crate::screen::Backdrop::TRANSPARENT)
        .with_widgets(vec![crate::screen::Widget::button(
            DONE,
            gui_w / 2 - 100,
            gui_h / 4 + 144,
            200,
            20,
            done_label,
        )])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A width function shaped like the fixture needs: 10 px per char, so 9
    /// chars fill a 90-px sign line exactly.
    fn w10(s: &str) -> i32 {
        s.chars().count() as i32 * 10
    }

    fn state() -> SignEditState {
        SignEditState::new(Default::default(), SignKind::Standing)
    }

    fn press(s: &mut SignEditState, key: i32) -> SignKey {
        let mut clip = String::new();
        s.key_pressed(Input::new(key, 0), &w10, &mut clip)
    }

    #[test]
    fn the_line_switch_wraps_both_ways_and_parks_the_cursor_at_the_end() {
        let mut s = state();
        s.lines[3] = "abc".into();
        // Up from line 0 wraps to 3 — `(line - 1) & 3`.
        assert_eq!(press(&mut s, KEY_UP), SignKey::Handled);
        assert_eq!(s.line, 3);
        assert_eq!(s.cursor, 3, "setCursorToEnd on the NEW line");
        assert_eq!(s.selection, 3);
        // Down from 3 wraps to 0.
        assert_eq!(press(&mut s, KEY_DOWN), SignKey::Handled);
        assert_eq!(s.line, 0);
    }

    #[test]
    fn enter_moves_down_a_line_and_never_closes() {
        let mut s = state();
        assert_eq!(press(&mut s, KEY_ENTER), SignKey::Handled);
        assert_eq!(s.line, 1);
        assert_eq!(press(&mut s, KEY_KP_ENTER), SignKey::Handled);
        assert_eq!(s.line, 2);
    }

    #[test]
    fn escape_reports_close_which_is_a_commit_not_a_cancel() {
        let mut s = state();
        assert_eq!(press(&mut s, 256), SignKey::Close);
    }

    #[test]
    fn the_validator_is_a_pixel_width_and_a_failing_insert_is_rejected_whole() {
        let mut s = state();
        s.char_typed('a', &w10);
        assert_eq!(s.lines[0], "a");
        // 8 more fill the 90-px line.
        for _ in 0..8 {
            s.char_typed('b', &w10);
        }
        assert_eq!(s.lines[0].len(), 9);
        // The 10th is rejected — silently, cursor unmoved.
        let before = s.cursor;
        s.char_typed('c', &w10);
        assert_eq!(s.lines[0].len(), 9);
        assert_eq!(s.cursor, before);
        // A paste that would fit partially is rejected IN ITS ENTIRETY —
        // never truncated (TextFieldHelper.insertText).
        let mut half = state();
        half.insert_text("12345", &w10);
        half.insert_text("67890", &w10); // 100 px — rejected whole
        assert_eq!(half.lines[0], "12345");
    }

    #[test]
    fn hanging_signs_validate_against_sixty_not_ninety() {
        let mut s = SignEditState::new(Default::default(), SignKind::Hanging);
        for _ in 0..7 {
            s.char_typed('a', &w10);
        }
        assert_eq!(s.lines[0].len(), 6, "60 px / 10 = 6 chars; the 7th is rejected");
    }

    #[test]
    fn the_char_filter_rejects_section_signs_and_control_characters() {
        let mut s = state();
        assert!(s.char_typed('§', &w10), "the screen returns true EITHER WAY");
        assert!(s.char_typed('\u{7f}', &w10));
        assert!(s.char_typed('\n', &w10));
        assert_eq!(s.lines[0], "");
        s.char_typed(' ', &w10);
        assert_eq!(s.lines[0], " ", "space (32) is the boundary and allowed");
    }

    #[test]
    fn backspace_returns_handled_and_delete_falls_through_after_acting() {
        let mut s = state();
        s.insert_text("abc", &w10);
        let mut clip = String::new();
        // Backspace: acts, handled.
        assert_eq!(
            s.key_pressed(Input::new(key::BACKSPACE, 0), &w10, &mut clip),
            SignKey::Handled
        );
        assert_eq!(s.lines[0], "ab");
        // Delete at the start: acts AND reports Unhandled — vanilla's
        // `case 261` has no `return true`.
        s.set_cursor_to_start(false);
        assert_eq!(
            s.key_pressed(Input::new(key::DELETE, 0), &w10, &mut clip),
            SignKey::Unhandled
        );
        assert_eq!(s.lines[0], "b");
    }

    #[test]
    fn a_backward_delete_moves_the_cursor_and_a_forward_one_does_not() {
        let mut s = state();
        s.insert_text("abcd", &w10);
        s.cursor = 2;
        s.selection = 2;
        s.remove_from_cursor(1, false); // forward: deletes 'c'
        assert_eq!(s.lines[0], "abd");
        assert_eq!(s.cursor, 2, "a forward delete leaves the cursor");
        s.remove_from_cursor(-1, false); // backward: deletes 'b'
        assert_eq!(s.lines[0], "ad");
        assert_eq!(s.cursor, 1, "a backward delete moves to the span start");
    }

    #[test]
    fn ctrl_word_ops_use_space_boundaries() {
        let mut s = state();
        s.insert_text("ab cd ef", &w10);
        assert_eq!(s.cursor, 8);
        s.move_by(-1, false, true);
        assert_eq!(s.cursor, 6, "back one word lands at 'ef'");
        s.move_by(-1, false, true);
        assert_eq!(s.cursor, 3);
        s.move_by(2, false, true);
        assert_eq!(s.cursor, 8, "forward two words: past 'cd ' then to the end");
        // Ctrl+Backspace from the end removes 'ef'.
        s.remove_from_cursor(-1, true);
        assert_eq!(s.lines[0], "ab cd ");
    }

    #[test]
    fn selection_copy_cut_paste_round_trip_with_the_paste_strip() {
        let mut s = state();
        s.insert_text("hello", &w10);
        let mut clip = String::new();
        // Select all, copy.
        s.key_pressed(Input::new(key::A, 2), &w10, &mut clip);
        assert_eq!(s.selected(), "hello");
        s.key_pressed(Input::new(key::C, 2), &w10, &mut clip);
        assert_eq!(clip, "hello");
        // Cut clears the line.
        s.key_pressed(Input::new(key::X, 2), &w10, &mut clip);
        assert_eq!(s.lines[0], "");
        // Paste strips §-codes and \r — and a LONE trailing § survives.
        clip = "§ca\r§".into();
        s.key_pressed(Input::new(key::V, 2), &w10, &mut clip);
        assert_eq!(s.lines[0], "a§", "§c stripped, \\r stripped, bare § kept");
        assert_eq!(s.selection, s.cursor, "paste collapses the selection");
    }

    #[test]
    fn typing_over_a_selection_replaces_it_and_a_failing_insert_commits_nothing() {
        let mut s = state();
        s.insert_text("abcdefgh", &w10);
        s.cursor = 2;
        s.selection = 5; // "cde" selected
        s.char_typed('X', &w10);
        assert_eq!(s.lines[0], "abXfgh");
        // An insert over a selection that fails the width even after the
        // deletion: `deleteSelection` is pure on the text and only
        // `setMessageFn` commits — so the LINE keeps every character,
        // selected ones included, and only the selection collapses.
        let mut t = state();
        t.insert_text("123456789", &w10); // full at 90
        t.cursor = 0;
        t.selection = 1;
        t.insert_text("ab", &w10); // candidate "ab23456789" = 100 px, fails
        assert_eq!(t.lines[0], "123456789", "the text is untouched");
        assert_eq!((t.cursor, t.selection), (0, 0), "only the collapse stands");
    }

    #[test]
    fn the_board_rect_is_the_pose_math_and_the_wall_board_is_twelve_rows() {
        let (x, y, w, h) = board_rect(SignKind::Standing, 320);
        assert!((x - (160.0 - 46.8)).abs() < 1e-4);
        assert!((y - (117.0 - 50.7)).abs() < 1e-4);
        assert!((w - 93.6).abs() < 1e-4);
        assert!((h - 101.4).abs() < 1e-4);
        let (_, _, _, hw) = board_rect(SignKind::Wall, 320);
        assert!((hw - 46.8).abs() < 1e-4, "wall = 12 rows x 3.9");
        // Hanging: 72x72 centred at (w/2, 112) — integral.
        assert_eq!(board_sprite(SignKind::Hanging, 320), (124, 76, 72, 72));
    }

    #[test]
    fn line_origins_are_centred_in_the_text_scaled_pose() {
        // Line 0 of a standing sign: y = 90 + ts * (0*10 - 20).
        let (x, y) = line_origin(SignKind::Standing, 320, 0, 40);
        assert!((y - (90.0 - SIGN_TEXT_SCALE * 20.0)).abs() < 1e-4);
        assert!((x - (160.0 - SIGN_TEXT_SCALE * 20.0)).abs() < 1e-4);
        // Hanging line 3: y = 125 + 1.0 * (27 - 18) = 134.
        let (_, hy) = line_origin(SignKind::Hanging, 320, 3, 0);
        assert!((hy - 134.0).abs() < 1e-4);
    }

    #[test]
    fn the_caret_is_an_underscore_at_the_end_and_a_bar_inside() {
        let mut s = state();
        s.insert_text("ab", &w10);
        match caret_draw(&s, 320, &w10) {
            CaretDraw::Underscore { x, .. } => {
                // prefix 20, line 20 → x = 160 + ts*(20 - 10).
                assert!((x - (160.0 + SIGN_TEXT_SCALE * 10.0)).abs() < 1e-4);
            }
            other => panic!("expected underscore, got {other:?}"),
        }
        s.cursor = 1;
        s.selection = 1;
        match caret_draw(&s, 320, &w10) {
            CaretDraw::Bar { y, h, .. } => {
                // y extends one text-px above the line: 90 + ts*(-20 - 1).
                assert!((y - (90.0 + SIGN_TEXT_SCALE * -21.0)).abs() < 1e-4);
                assert!((h - SIGN_TEXT_SCALE * 11.0).abs() < 1e-4);
            }
            other => panic!("expected bar, got {other:?}"),
        }
    }

    #[test]
    fn the_selection_rect_spans_the_substring_widths() {
        let mut s = state();
        s.insert_text("abcd", &w10);
        s.cursor = 1;
        s.selection = 3;
        let (x, _, w, h) = selection_rect(&s, 320, &w10).unwrap();
        // x0 = 10 - 20 = -10, x1 = 30 - 20 = 10 → x = 160 - ts*10, w = ts*20.
        assert!((x - (160.0 - SIGN_TEXT_SCALE * 10.0)).abs() < 1e-4);
        assert!((w - SIGN_TEXT_SCALE * 20.0).abs() < 1e-4);
        assert!((h - SIGN_TEXT_SCALE * 10.0).abs() < 1e-4);
        s.selection = 1;
        assert!(selection_rect(&s, 320, &w10).is_none());
    }

    #[test]
    fn the_blink_is_wall_clock_three_hundred_on_three_hundred_off() {
        assert!(cursor_visible(0));
        assert!(cursor_visible(299));
        assert!(!cursor_visible(300));
        assert!(!cursor_visible(599));
        assert!(cursor_visible(600));
    }

    #[test]
    fn too_far_is_the_attribute_plus_a_four_block_buffer_against_the_block_aabb() {
        // The box spans pos..pos+1, so an eye at y=9.4 is 8.4 above the TOP
        // face — within 4.5 + 4.0 = 8.5. A flat-4.0 reading calls this too
        // far, which is the mutation this fixture kills.
        assert!(!too_far_to_edit((0.5, 9.4, 0.5), (0, 0, 0), 4.5));
        // 8.6 above the top: past the 8.5 boundary (`<` is "within", so the
        // exact boundary itself is too far).
        assert!(too_far_to_edit((0.5, 9.6, 0.5), (0, 0, 0), 4.5));
        // Inside the block: distance 0.
        assert!(!too_far_to_edit((0.5, 0.5, 0.5), (0, 0, 0), 4.5));
    }

    #[test]
    fn build_screen_places_done_at_the_quarter_height_anchor() {
        let s = build_screen(320, 240, "Done");
        assert_eq!(s.kind, crate::screen::ScreenKind::SignEdit);
        let done = s.widgets.iter().find(|w| w.id == DONE).unwrap();
        assert_eq!((done.x, done.y), (320 / 2 - 100, 240 / 4 + 144));
        assert_eq!((done.width, done.height), (200, 20));
    }

    #[test]
    fn word_position_transcribes_get_word_position() {
        // Forward from 0 in "ab cd": first separator at 2, skip it → 3.
        assert_eq!(word_position("ab cd", 1, 0), 3);
        // Forward again: no more separators → length.
        assert_eq!(word_position("ab cd", 1, 3), 5);
        // Backward from 5: skip "cd" → 3.
        assert_eq!(word_position("ab cd", -1, 5), 3);
        // Backward from 3: strip the space run, then "ab" → 0.
        assert_eq!(word_position("ab cd", -1, 3), 0);
        // A separator RUN is skipped whole.
        assert_eq!(word_position("a   b", 1, 0), 4);
    }
}
