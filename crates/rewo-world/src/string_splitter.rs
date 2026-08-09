//! `StringSplitter.LineBreakFinder` and the **two** `splitLines` overloads
//! built on it — the second of which is not the first with a flag.
//!
//! M85 transcribed `splitLines(String, int, Style)` inside
//! [`crate::disconnect_screen`] for the disconnect reason, and that is still
//! where it lives and what its callers use. M108 needs the **other** overload,
//! `splitLines(FormattedText, int, Style, BiConsumer<FormattedText, Boolean>)`,
//! because that is the one `ComponentRenderUtils.wrapComponents` calls and
//! therefore the one every chat message is wrapped by.
//!
//! # The two overloads are different functions
//!
//! They share `LineBreakFinder`, so **every break lands in the same place** —
//! that part is genuinely one algorithm, and [`find_line_break`] (plain) and
//! [`find_styled_line_break`] (over a part list) are the two spellings of it.
//! What differs is what gets *emitted*:
//!
//! | | `splitLines(String, …)` | `splitLines(FormattedText, …)` |
//! |---|---|---|
//! | per-line flag | none | `isWrapped` |
//! | trailing `\n` | `"a\n"` → `["a"]` | `"a\n"` → `["a", ""]` |
//! | a line is | a `String` | a list of **styled parts** |
//!
//! **The flag is `isWrapped = !isNewLine`, not "this line has an index > 0".**
//!
//! ```java
//! forceNewLine = isNewLine;
//! FormattedText result = parts.splitAt(lineBreak, skipNextChar ? 1 : 0, lineBreakStyle);
//! output.accept(result, isWrapped);
//! isWrapped = !isNewLine;        // <-- assigned for the NEXT line
//! ```
//!
//! so a line carries the flag describing the break *before* it, and a break at
//! a `\n` clears it. That is what stops a component containing explicit
//! newlines from being indented on every line after the first — the obvious
//! index-based model gets exactly that wrong, and it is wrong only for
//! multi-line messages, which is the case nobody tries first.
//!
//! **The trailing empty line** is the `else` nobody reads:
//!
//! ```java
//! FormattedText lastLine = parts.getRemainder();
//! if (lastLine != null) {
//!    output.accept(lastLine, isWrapped);
//! } else if (forceNewLine) {
//!    output.accept(FormattedText.EMPTY, false);
//! }
//! ```
//!
//! `getRemainder` returns **null** (not empty) once `splitAt` has removed the
//! last part — `if (afterSplit.isEmpty()) it.remove()` — so a message ending
//! in `\n` reaches the `else` and emits one more, empty, line. A message
//! ending in a *space* that happened to break there does not, because
//! `forceNewLine` is only ever assigned `isNewLine`.
//!
//! # M126b — the parts are real now
//!
//! Until M126 Rewo flattened a component to plain text long before it reached
//! here, so `partList` had one element and `splitAt`'s multi-part walk
//! degenerated to a substring. The module docs said so. It no longer does:
//! [`split_lines_wrapped`] takes the [`ChatLine`] that
//! [`crate::chat_style::parse_component`] produces and emits lines that are
//! themselves span lists, so a break falling in the middle of a coloured run
//! carries the colour onto the next line.
//!
//! Two consequences of that which are easy to get wrong:
//!
//! * **The width provider takes a style.** `Font`'s is
//!   `getGlyph(cp).info().getAdvance(style.isBold())` and
//!   `GlyphInfo.getBoldOffset()` is `1.0F`, so a bold character is exactly one
//!   pixel wider — which moves where a line wraps, not just how it looks. A
//!   style-blind width measures a bold run short and lets it overhang the box.
//! * **The finder is created OUTSIDE the part loop.** `width`, `lastSpace` and
//!   `hadNonZeroWidthChar` accumulate across parts, and `addToOffset` makes
//!   the reported positions indices into the concatenation. A finder rebuilt
//!   per part would reset the running width at every colour change and never
//!   wrap a multi-span line at all.
//!
//! # What is deliberately not transcribed
//!
//! **UTF-16.** Vanilla indexes code units and advances `nextChar` by
//! `Character.charCount(codepoint)`; Rewo indexes `char` (Unicode scalars).
//! The two agree for everything in the BMP. The same list of omissions
//! [`crate::disconnect_screen::split_lines`] records (the `§`-code
//! re-emission, bidi) applies unchanged — with one now retired: `§` codes are
//! resolved into separate spans by `parse_component` before they reach here,
//! so the style genuinely survives a break instead of being dropped with the
//! prefix that carried it.

use crate::chat_style::{ChatLine, ChatSpan, ChatStyle};

/// `StringSplitter.LineBreakFinder` — one line's sweep over a plain string.
///
/// Returns the position to break at, or `None` for `endOfText` (the sweep ran
/// off the end without wanting a break, so the remainder is one line).
///
/// This is the unstyled spelling, kept for [`crate::disconnect_screen`], whose
/// vanilla counterpart really does take a `String`. [`find_styled_line_break`]
/// is the same sweep over a part list, and
/// `the_two_finders_agree_on_a_single_plain_part` pins that they are one
/// algorithm rather than two that happen to look alike.
///
/// Three details that are not the obvious greedy wrap, all load-bearing and
/// all pinned by [`crate::disconnect_screen`]'s tests:
///
/// * The overflow test is `width > maxWidth` measured **after** adding the
///   character, so a line whose width exactly equals `maxWidth` still fits.
/// * `hadNonZeroWidthChar` guarantees progress: the first visible character of
///   a line is always accepted even if it alone overflows, so a very narrow box
///   makes one-character lines rather than looping.
/// * `maxWidth` is floored at 1 (`Math.max(maxWidth, 1.0F)` in the
///   constructor), which is what stops a zero or negative width doing the same.
///
/// `case 32:` in vanilla's `switch` has **no `break`** — a space records
/// `lastSpace` *and* falls through to the width accumulation, so its own width
/// counts toward the overflow test.
pub fn find_line_break(
    chars: &[char],
    start: usize,
    max_width: i32,
    width_of: &dyn Fn(&str) -> i32,
) -> Option<usize> {
    let max_width = max_width.max(1);
    let mut width = 0i32;
    let mut had_visible = false;
    let mut last_space: Option<usize> = None;
    for i in start..chars.len() {
        let c = chars[i];
        if c == '\n' {
            return Some(i);
        }
        if c == ' ' {
            last_space = Some(i);
        }
        let cw = width_of(&c.to_string());
        width += cw;
        if !had_visible || width <= max_width {
            had_visible |= cw != 0;
        } else {
            return Some(last_space.unwrap_or(i));
        }
    }
    None
}

/// The same sweep across a list of styled parts, returning the break position
/// **as an index into the concatenation** together with the style in effect at
/// that character.
///
/// This is vanilla's `for (LineComponent part : parts.parts) { … finder
/// .addToOffset(part.contents.length()); }` loop folded into the finder,
/// because that is what it is: one `LineBreakFinder`, walked over every part in
/// turn, whose `offset` turns each part-local position into a flat one.
///
/// The returned style is `getSplitStyle()` — `lineBreakStyle` when the break is
/// a `\n` or a width overflow, `lastSpaceStyle` when it falls back to the last
/// space. Vanilla tracks the two separately for exactly that reason.
pub fn find_styled_line_break(
    parts: &[ChatSpan],
    max_width: i32,
    width_of: &dyn Fn(&str, ChatStyle) -> i32,
) -> Option<(usize, ChatStyle)> {
    let max_width = max_width.max(1);
    let mut width = 0i32;
    let mut had_visible = false;
    let mut last_space: Option<(usize, ChatStyle)> = None;
    let mut offset = 0usize;
    for part in parts {
        let style = part.style();
        let mut count = 0usize;
        for (i, c) in part.text.chars().enumerate() {
            count = i + 1;
            let pos = offset + i;
            if c == '\n' {
                return Some((pos, style));
            }
            if c == ' ' {
                last_space = Some((pos, style.clone()));
            }
            // `ChatStyle` stopped being `Copy` at M128 (it owns the run's
            // click / hover / insertion); the clone is a memcpy plus, for a
            // clickable run, one refcount bump.
            let cw = width_of(&c.to_string(), style.clone());
            width += cw;
            if !had_visible || width <= max_width {
                had_visible |= cw != 0;
            } else {
                return Some(last_space.unwrap_or((pos, style)));
            }
        }
        offset += count;
    }
    None
}

/// `StringSplitter.FlatComponents` — the mutable part list a split consumes
/// from, plus the concatenation it reports positions against.
///
/// Vanilla stores `flatParts` as a second field and keeps it in step by hand
/// (`this.flatParts = this.flatParts.substring(skipPosition + skipSize)`).
/// Here it is derived, because `splitAt` maintains exactly that invariant —
/// the remaining parts always concatenate to the old flat text minus the
/// `skipPosition + skipSize` characters it consumed — and a second stored copy
/// is a second thing that can drift.
#[derive(Debug)]
struct FlatComponents {
    parts: Vec<ChatSpan>,
}

impl FlatComponents {
    /// `input.visit((style, contents) -> { if (!contents.isEmpty()) partList
    /// .add(…); }, initialStyle)`.
    ///
    /// **Empty parts are dropped here**, not later. That matters: every index
    /// below is into the concatenation, and a zero-length part would sit at a
    /// position no character occupies, so `splitAt`'s `position > contentsSize`
    /// arithmetic would step over it without consuming anything.
    fn new(input: &[ChatSpan]) -> Self {
        Self {
            parts: input.iter().filter(|p| !p.text.is_empty()).cloned().collect(),
        }
    }

    /// `flatParts.charAt(position)`.
    fn char_at(&self, position: usize) -> Option<char> {
        let mut rest = position;
        for part in &self.parts {
            let n = part.text.chars().count();
            if rest < n {
                return part.text.chars().nth(rest);
            }
            rest -= n;
        }
        None
    }

    /// `FlatComponents.splitAt` — take the first `skip_position` characters as
    /// a line, drop the next `skip_size`, and leave the rest.
    ///
    /// `split_style` is the style vanilla stamps on the tail of the part the
    /// break landed in. It only ever reaches a part whose own style is already
    /// that style (see `the_split_style_is_the_tail_parts_own_style`), because
    /// Rewo resolves `§` codes into separate spans up front where vanilla
    /// leaves them inside a part for the decomposer — transcribed anyway rather
    /// than dropped, so the day a part carries an internal style change this
    /// stays right.
    ///
    /// The cursor is the **head of the list throughout**: vanilla's
    /// `ListIterator` never advances past an element it does not remove (the
    /// `!inSkip` else-branch falls straight into the `inSkip` block on the same
    /// element), which is a consequence of the function consuming everything
    /// before the split rather than an accident.
    fn split_at(
        &mut self,
        skip_position: usize,
        skip_size: usize,
        split_style: ChatStyle,
    ) -> ChatLine {
        let mut result: ChatLine = Vec::new();
        let mut position = skip_position;
        let mut in_skip = false;
        while !self.parts.is_empty() {
            let contents: Vec<char> = self.parts[0].text.chars().collect();
            let size = contents.len();
            if !in_skip {
                // **Strictly** greater: a break exactly at the end of this part
                // takes the else-branch, so the whole part goes to the line and
                // any skipped character comes out of the NEXT one.
                if position > size {
                    result.push(self.parts.remove(0));
                    position -= size;
                    continue;
                }
                let before: String = contents[..position].iter().collect();
                if !before.is_empty() {
                    result.push(ChatSpan {
                        text: before,
                        ..self.parts[0].clone()
                    });
                }
                position += skip_size;
                in_skip = true;
            }
            if position <= size {
                let after: String = contents[position..].iter().collect();
                if after.is_empty() {
                    self.parts.remove(0);
                } else {
                    self.parts[0] = split_style.span(after);
                }
                break;
            }
            self.parts.remove(0);
            position -= size;
        }
        result
    }

    /// `getRemainder()` — **null**, not empty, when nothing is left.
    ///
    /// That distinction is the whole of the trailing-empty-line rule: a message
    /// ending in `\n` has had its last part removed by `splitAt`, so this
    /// answers `None` and the caller's `else if (forceNewLine)` fires.
    fn remainder(&mut self) -> Option<ChatLine> {
        if self.parts.is_empty() {
            None
        } else {
            Some(std::mem::take(&mut self.parts))
        }
    }
}

/// One line out of the `FormattedText` overload: its spans, and whether the
/// break *before* it was a width wrap rather than a `\n`.
#[derive(Clone, Debug, PartialEq)]
pub struct WrappedLine {
    pub spans: ChatLine,
    /// `isWrapped` — true when the preceding break was a width wrap. The first
    /// line is always `false`.
    pub wrapped: bool,
}

impl WrappedLine {
    /// The line's characters, spans concatenated — for a caller that only
    /// wants the text, and for tests.
    pub fn plain(&self) -> String {
        crate::chat_style::plain_text(&self.spans)
    }
}

/// `StringSplitter.splitLines(FormattedText, int, Style, BiConsumer)`.
///
/// See the module docs for how this differs from
/// [`crate::disconnect_screen::split_lines`], which transcribes the `String`
/// overload. An empty input yields **no** lines here; the one-empty-line rule
/// belongs to `wrapComponents`, not to the splitter — see
/// `crate::chat::wrap_components`.
pub fn split_lines_wrapped(
    input: &[ChatSpan],
    max_width: i32,
    width_of: &dyn Fn(&str, ChatStyle) -> i32,
) -> Vec<WrappedLine> {
    let mut parts = FlatComponents::new(input);
    let mut out: Vec<WrappedLine> = Vec::new();
    let mut is_wrapped = false;
    let mut force_new_line = false;
    // vanilla's `while (shouldRestart)`: after each emitted line the whole part
    // walk restarts with a FRESH finder against the shortened list, which is
    // what re-calling the finder here is.
    while let Some((line_break, split_style)) = find_styled_line_break(&parts.parts, max_width, width_of)
    {
        let Some(tail) = parts.char_at(line_break) else {
            // Unreachable: `iterateFormatted` only returns false after
            // `finishIteration`, so a reported break is always a real index.
            break;
        };
        let is_new_line = tail == '\n';
        // `skipNextChar = isNewLine || firstTailChar == ' '` — the breaking
        // character is dropped only when it is one of those two.
        let skip = usize::from(is_new_line || tail == ' ');
        if line_break + skip == 0 {
            // Also unreachable, and load-bearing that it is: `hadNonZeroWidthChar`
            // accepts the first character of a line unconditionally, so a width
            // break is never at position 0, and a break AT position 0 is a `\n`
            // or a space and therefore skips one. Guarded rather than asserted
            // because the input is a network component and a consumed-nothing
            // split would spin forever.
            debug_assert!(false, "split consumed nothing at {line_break}");
            break;
        }
        force_new_line = is_new_line;
        let spans = parts.split_at(line_break, skip, split_style);
        out.push(WrappedLine {
            spans,
            wrapped: is_wrapped,
        });
        is_wrapped = !is_new_line;
    }
    if let Some(spans) = parts.remainder() {
        out.push(WrappedLine {
            spans,
            wrapped: is_wrapped,
        });
    } else if force_new_line {
        // `output.accept(FormattedText.EMPTY, false)` — the literal `false`,
        // kept as vanilla writes it and **provably redundant**, in vanilla as
        // much as here: `force_new_line` and `is_wrapped` are assigned from the
        // same `is_new_line` in opposite senses at the same point, so reaching
        // this branch already implies `is_wrapped == false`. Pinned by
        // `the_literal_false_on_the_trailing_line_is_inert`, which is an
        // exhaustive proof rather than an example — a mutation to `is_wrapped`
        // is equivalent today and would stop being so if anything ever assigns
        // one of the two without the other.
        out.push(WrappedLine {
            spans: Vec::new(),
            wrapped: false,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 6-px-per-character font, matching the fixture
    /// [`crate::disconnect_screen`]'s tests use so the two overloads can be
    /// compared on identical inputs. Bold costs the vanilla
    /// `GlyphInfo.getBoldOffset()` of one pixel.
    fn w6s(s: &str, style: ChatStyle) -> i32 {
        let per = 6 + i32::from(style.bold);
        s.chars().count() as i32 * per
    }

    fn w6(s: &str) -> i32 {
        s.chars().count() as i32 * 6
    }

    fn plain(text: &str) -> ChatSpan {
        ChatStyle::WHITE.span(text)
    }

    fn red() -> ChatStyle {
        ChatStyle::plain([1.0, 0.0, 0.0])
    }

    fn blue() -> ChatStyle {
        ChatStyle::plain([0.0, 0.0, 1.0])
    }

    /// `(text, wrapped)` per line, for the cases that only care about breaks.
    fn lines(text: &str, max_width: i32) -> Vec<(String, bool)> {
        split_lines_wrapped(&[plain(text)], max_width, &w6s)
            .into_iter()
            .map(|l| (l.plain(), l.wrapped))
            .collect()
    }

    /// `(text, colour)` per span per line, for the cases that care about style.
    fn styled(input: &[ChatSpan], max_width: i32) -> Vec<Vec<(String, [f32; 3])>> {
        split_lines_wrapped(input, max_width, &w6s)
            .into_iter()
            .map(|l| {
                l.spans
                    .into_iter()
                    .map(|s| (s.text, s.color))
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    // -- the rules the single-part version already pinned ------------------

    #[test]
    fn a_width_wrap_flags_the_continuation_and_a_newline_does_not() {
        // The whole point of the flag. Both inputs produce two lines; only the
        // wrapped one flags its second.
        assert_eq!(
            lines("abc def", 18),
            vec![("abc".into(), false), ("def".into(), true)],
        );
        assert_eq!(
            lines("abc\ndef", 600),
            vec![("abc".into(), false), ("def".into(), false)],
        );
    }

    #[test]
    fn the_first_line_is_never_flagged() {
        assert_eq!(lines("abc", 600), vec![("abc".into(), false)]);
        // Even when the break that follows it is a wrap.
        assert!(!lines("abc def", 18)[0].1);
    }

    #[test]
    fn the_flag_clears_at_a_newline_after_a_wrap() {
        // `isWrapped = !isNewLine` assigns, it does not accumulate: the line
        // after the `\n` is unflagged even though a wrap happened before it.
        assert_eq!(
            lines("abc def\nghi", 18),
            vec![
                ("abc".into(), false),
                ("def".into(), true),
                ("ghi".into(), false),
            ],
        );
    }

    #[test]
    fn a_trailing_newline_emits_one_more_empty_line() {
        // The `else if (forceNewLine)` branch. `split_lines` answers `["a"]`
        // for this input; the two overloads genuinely disagree.
        assert_eq!(
            lines("a\n", 600),
            vec![("a".into(), false), (String::new(), false)],
        );
        assert_eq!(
            crate::disconnect_screen::split_lines("a\n", 600, &w6),
            vec!["a"],
        );
    }

    #[test]
    fn a_trailing_space_that_breaks_does_not() {
        // `forceNewLine` is only ever assigned `isNewLine`, so a break at a
        // space leaves it false and no extra line is emitted — even though the
        // space is skipped and the remainder is just as empty.
        assert_eq!(lines("abc ", 18), vec![("abc".into(), false)]);
    }

    #[test]
    fn a_leading_newline_emits_an_empty_first_line() {
        // `splitAt` returns `getResultOrEmpty()`, never null, so a break at
        // position 0 is a real (empty) line rather than nothing.
        assert_eq!(
            lines("\nb", 600),
            vec![(String::new(), false), ("b".into(), false)],
        );
    }

    #[test]
    fn an_empty_input_yields_no_lines_here() {
        // The one-empty-line rule lives in `wrapComponents`, not the splitter.
        assert_eq!(lines("", 600), Vec::<(String, bool)>::new());
        assert!(split_lines_wrapped(&[], 600, &w6s).is_empty());
    }

    #[test]
    fn break_positions_agree_with_the_string_overload() {
        // The shared `LineBreakFinder` is the claim: only the flag and the
        // trailing-newline rule differ, never where a line ends. Every case
        // here is one `disconnect_screen`'s own tests pin.
        for (text, width) in [
            ("abc def", 18),
            ("abcdef", 18),
            ("abc", 18),
            ("aa bb cc", 18),
            ("a\nb", 600),
            ("abc", 1),
        ] {
            let a = crate::disconnect_screen::split_lines(text, width, &w6);
            let b: Vec<String> = split_lines_wrapped(&[plain(text)], width, &w6s)
                .into_iter()
                .map(|l| l.plain())
                .collect();
            assert_eq!(a, b, "{text:?} at {width}");
        }
    }

    #[test]
    fn a_hard_break_keeps_the_character_that_did_not_fit() {
        // `adjustedBreak` advances past the break character only for a space
        // or a newline, so a mid-word break must not eat a letter.
        assert_eq!(
            lines("abcdef", 18),
            vec![("abc".into(), false), ("def".into(), true)],
        );
    }

    // -- M126b: the part list ---------------------------------------------

    #[test]
    fn the_two_finders_agree_on_a_single_plain_part() {
        // They are one algorithm in vanilla — the same `LineBreakFinder`, fed
        // either one string or a walk over parts. Asserted rather than argued,
        // because `find_styled_line_break` folds in the `addToOffset` loop and
        // a fold is exactly where an off-by-one lives.
        for (text, width) in [
            ("abc def", 18),
            ("abcdef", 18),
            ("aa bb cc", 18),
            ("a\nb", 600),
            ("abc", 1),
            ("", 18),
            (" leading", 18),
            ("trailing ", 18),
        ] {
            let chars: Vec<char> = text.chars().collect();
            let a = find_line_break(&chars, 0, width, &w6);
            let b = find_styled_line_break(&[plain(text)], width, &w6s).map(|(p, _)| p);
            assert_eq!(a, b, "{text:?} at {width}");
        }
    }

    #[test]
    fn a_break_inside_a_run_carries_the_colour_onto_the_next_line() {
        // The whole point of the milestone. One red span of six characters, a
        // box three characters wide: both lines are red.
        assert_eq!(
            styled(&[red().span("abcdef")], 18),
            vec![
                vec![("abc".into(), [1.0, 0.0, 0.0])],
                vec![("def".into(), [1.0, 0.0, 0.0])],
            ],
        );
    }

    #[test]
    fn a_line_that_spans_two_runs_keeps_both() {
        // `splitAt` appends the whole of every part before the split and a
        // prefix of the one it lands in, so a line crossing a colour change
        // comes out as two spans rather than one.
        assert_eq!(
            styled(&[red().span("abc"), blue().span("def")], 600),
            vec![vec![
                ("abc".into(), [1.0, 0.0, 0.0]),
                ("def".into(), [0.0, 0.0, 1.0]),
            ]],
        );
    }

    #[test]
    fn a_break_at_a_part_boundary_leaves_the_next_part_whole() {
        // `position > contentsSize` is **strictly** greater, so a break exactly
        // at the end of a part takes the else-branch: the part is consumed
        // whole and the following one is untouched. Reading it as `>=` would
        // append the part twice.
        assert_eq!(
            styled(&[red().span("abc"), blue().span("def")], 18),
            vec![
                vec![("abc".into(), [1.0, 0.0, 0.0])],
                vec![("def".into(), [0.0, 0.0, 1.0])],
            ],
        );
    }

    #[test]
    fn the_skipped_space_can_come_out_of_the_following_part() {
        // The break is the space at flat index 3, which is the first character
        // of the *second* part. `splitAt` consumes part one whole, then walks
        // into part two with `position = 1` and drops that one character.
        assert_eq!(
            styled(&[red().span("abc"), blue().span(" def")], 18),
            vec![
                vec![("abc".into(), [1.0, 0.0, 0.0])],
                vec![("def".into(), [0.0, 0.0, 1.0])],
            ],
        );
    }

    #[test]
    fn an_empty_part_is_dropped_before_any_index_is_taken() {
        // `if (!contents.isEmpty())` in the visit. An empty span left in the
        // list would occupy no flat position while still being walked by
        // `splitAt`'s `position > contentsSize` arithmetic.
        assert_eq!(
            styled(&[red().span(""), blue().span("abc"), red().span("")], 600),
            vec![vec![("abc".into(), [0.0, 0.0, 1.0])]],
        );
    }

    #[test]
    fn the_running_width_accumulates_across_parts() {
        // The finder is built OUTSIDE the part loop. Three two-character parts
        // in an 18-px box wrap after the third character, exactly as one
        // six-character part does — a finder rebuilt per part would never
        // reach the limit and would emit one long line.
        let split: Vec<String> = split_lines_wrapped(
            &[plain("ab"), plain("cd"), plain("ef")],
            18,
            &w6s,
        )
        .into_iter()
        .map(|l| l.plain())
        .collect();
        assert_eq!(split, vec!["abc".to_string(), "def".to_string()]);
    }

    #[test]
    fn the_last_space_survives_a_part_boundary() {
        // `lastSpace` is finder state, so a space in part one is still the
        // break candidate when the overflow happens in part two.
        let split: Vec<String> = split_lines_wrapped(&[plain("ab "), plain("cdef")], 18, &w6s)
            .into_iter()
            .map(|l| l.plain())
            .collect();
        assert_eq!(split, vec!["ab".to_string(), "cde".to_string(), "f".to_string()]);
    }

    #[test]
    fn bold_is_one_pixel_wider_and_that_moves_the_break() {
        // `GlyphInfo.getBoldOffset()` is 1.0F. At 18 px a plain run fits three
        // 6-px characters; the same run in bold is 7 px each and fits two.
        // A style-blind width would wrap both the same way and let the bold
        // line overhang the box by three pixels.
        assert_eq!(
            split_lines_wrapped(&[plain("abcdef")], 18, &w6s)[0].plain(),
            "abc",
        );
        let bold = ChatStyle {
            bold: true,
            ..ChatStyle::WHITE
        };
        assert_eq!(
            split_lines_wrapped(&[bold.span("abcdef")], 18, &w6s)[0].plain(),
            "ab",
        );
    }

    #[test]
    fn the_split_style_is_the_tail_parts_own_style() {
        // Rewo resolves `§` codes into separate spans before the splitter sees
        // them, so every part has a uniform style and `splitStyle` can only
        // ever be stamped on a part that already had it. Proved over the
        // shapes that can reach `it.set` rather than argued: a break inside a
        // part, at its last character, at a part boundary, and at a space.
        for input in [
            vec![red().span("abcdef")],
            vec![red().span("abc"), blue().span("def")],
            vec![red().span("ab"), blue().span("cdef")],
            vec![red().span("ab cdef")],
            vec![red().span("ab "), blue().span("cdef")],
        ] {
            for width in [1, 6, 12, 18, 600] {
                for line in split_lines_wrapped(&input, width, &w6s) {
                    for span in &line.spans {
                        // Every emitted span's colour is one the input carried.
                        assert!(
                            input.iter().any(|p| p.color == span.color),
                            "{span:?} from {input:?} at {width}",
                        );
                    }
                }
            }
        }
    }


    /// `FlatComponents::split_at` with vanilla's `position > contentsSize` read
    /// as `>=` — the mutation that survived M126's battery, kept so the claim
    /// of equivalence is measured rather than argued.
    fn split_at_ge(parts: &mut Vec<ChatSpan>, skip_position: usize, skip_size: usize, split_style: ChatStyle) -> ChatLine {
        let mut result: ChatLine = Vec::new();
        let mut position = skip_position;
        let mut in_skip = false;
        while !parts.is_empty() {
            let contents: Vec<char> = parts[0].text.chars().collect();
            let size = contents.len();
            if !in_skip {
                // The mutation.
                if position >= size {
                    result.push(parts.remove(0));
                    position -= size;
                    continue;
                }
                let before: String = contents[..position].iter().collect();
                if !before.is_empty() {
                    result.push(ChatSpan { text: before, ..parts[0].clone() });
                }
                position += skip_size;
                in_skip = true;
            }
            if position <= size {
                let after: String = contents[position..].iter().collect();
                if after.is_empty() {
                    parts.remove(0);
                } else {
                    parts[0] = split_style.span(after);
                }
                break;
            }
            parts.remove(0);
            position -= size;
        }
        result
    }

    /// The style at a flat position — what `getSplitStyle()` reports, and
    /// therefore what production always passes as `split_style`.
    fn style_at(parts: &[ChatSpan], pos: usize) -> ChatStyle {
        let mut rest = pos;
        for p in parts {
            let n = p.text.chars().count();
            if rest < n {
                return p.style();
            }
            rest -= n;
        }
        ChatStyle::WHITE
    }

    #[test]
    fn the_strictly_greater_test_is_inert_only_because_of_a_coupling() {
        // The mutation `position > size` -> `>=` survives the entire suite,
        // and this says exactly why — and how narrowly.
        //
        // The two readings DIVERGE: where `>` takes the else-branch on the
        // part the break ends, `>=` appends that part whole and walks into the
        // NEXT one, where `it.set` stamps `split_style` on its tail. `>` never
        // reaches that `set`. So the guard is doing real work.
        //
        // It is invisible today because of a coupling between two arguments:
        // production always passes `getSplitStyle()`, the style at the break
        // CHARACTER, and in the diverging case that character is the first of
        // the very part `>=` restyles — so it is restyled to what it already
        // was. Decouple them and `>` is load-bearing immediately, which is the
        // second half of this test.
        let shapes: [&[&str]; 8] = [
            &["abcdef"],
            &["abc", "def"],
            &["ab", "cdef"],
            &["a", "b", "c", "d"],
            &["ab ", "cdef"],
            &["abc", " def"],
            &["a b", "c d"],
            &["abc", "d", "ef", "gh"],
        ];
        let styles = [red(), blue()];
        let (mut agreed, mut disagreed) = (0usize, 0usize);
        for shape in shapes {
            let input: Vec<ChatSpan> = shape
                .iter()
                .enumerate()
                .map(|(i, t)| styles[i % 2].span(*t))
                .collect();
            let total: usize = shape.iter().map(|t| t.chars().count()).sum();
            for skip_position in 0..total {
                for skip_size in [0usize, 1] {
                    if skip_position + skip_size > total {
                        continue;
                    }
                    // (a) the production pairing — always agrees.
                    let prod = style_at(&input, skip_position);
                    let mut a = FlatComponents::new(&input);
                    let line_a = a.split_at(skip_position, skip_size, prod.clone());
                    let mut b: Vec<ChatSpan> =
                        input.iter().filter(|p| !p.text.is_empty()).cloned().collect();
                    let line_b = split_at_ge(&mut b, skip_position, skip_size, prod);
                    assert_eq!(line_a, line_b, "line at {skip_position}/{skip_size} of {shape:?}");
                    assert_eq!(a.parts, b, "parts at {skip_position}/{skip_size} of {shape:?}");
                    agreed += 1;

                    // (b) an unrelated split style — where they come apart.
                    let odd = ChatStyle::plain([0.0, 1.0, 0.0]);
                    let mut c = FlatComponents::new(&input);
                    let lc = c.split_at(skip_position, skip_size, odd.clone());
                    let mut d: Vec<ChatSpan> =
                        input.iter().filter(|p| !p.text.is_empty()).cloned().collect();
                    let ld = split_at_ge(&mut d, skip_position, skip_size, odd);
                    if lc != ld || c.parts != d {
                        disagreed += 1;
                    }
                }
            }
        }
        // A floor whose only job is "the loop ran" — deliberately well below
        // the actual 100, because a threshold set AT the count recalibrates
        // itself whenever the corpus changes (M104's self-calibrating witness).
        assert!(agreed > 50, "only {agreed} comparisons");
        // If this ever reached zero the mutation really would be equivalent,
        // and the `>` could go. It does not: a break landing exactly on a part
        // boundary is the shape that separates them.
        assert!(disagreed > 0, "the two readings never diverged — recheck the claim");
    }

    #[test]
    fn a_split_position_is_always_a_real_character() {
        // The precondition the proof above rests on, asserted rather than
        // assumed: every break the finder reports indexes a character, so
        // `split_at` never sees `skip_position == total`.
        for text in ["abcdef", "abc def", "a\nb", "  a", "a  b"] {
            for width in [1, 6, 12, 18, 600] {
                let parts = [plain(text)];
                if let Some((pos, _)) = find_styled_line_break(&parts, width, &w6s) {
                    assert!(
                        pos < text.chars().count(),
                        "{text:?} at {width} broke at {pos} of {}",
                        text.chars().count(),
                    );
                }
            }
        }
    }

    /// The same function with the trailing line's flag read from `is_wrapped`
    /// instead of vanilla's literal `false` — the surviving mutation, kept here
    /// so the claim of equivalence is measured rather than argued.
    fn split_lines_wrapped_variant(
        input: &[ChatSpan],
        max_width: i32,
        width_of: &dyn Fn(&str, ChatStyle) -> i32,
    ) -> Vec<WrappedLine> {
        let mut parts = FlatComponents::new(input);
        let mut out: Vec<WrappedLine> = Vec::new();
        let mut is_wrapped = false;
        let mut force_new_line = false;
        while let Some((line_break, split_style)) =
            find_styled_line_break(&parts.parts, max_width, width_of)
        {
            let Some(tail) = parts.char_at(line_break) else {
                break;
            };
            let is_new_line = tail == '\n';
            let skip = usize::from(is_new_line || tail == ' ');
            if line_break + skip == 0 {
                break;
            }
            force_new_line = is_new_line;
            let spans = parts.split_at(line_break, skip, split_style);
            out.push(WrappedLine {
                spans,
                wrapped: is_wrapped,
            });
            is_wrapped = !is_new_line;
        }
        if let Some(spans) = parts.remainder() {
            out.push(WrappedLine {
                spans,
                wrapped: is_wrapped,
            });
        } else if force_new_line {
            out.push(WrappedLine {
                spans: Vec::new(),
                // The mutation.
                wrapped: is_wrapped,
            });
        }
        out
    }

    #[test]
    fn the_literal_false_on_the_trailing_line_is_inert() {
        // `force_new_line` and `is_wrapped` are written from the same
        // `is_new_line` in opposite senses, so the guard already implies the
        // value. This is a proof over every shape that can reach the branch —
        // trailing newline, trailing newline after a wrap, after another
        // newline, doubled, and the near-misses that must NOT reach it — not a
        // single example, because a single example cannot distinguish
        // "equivalent" from "the one case I happened to pick".
        let corpus = [
            "a\n",
            "abc def\n",
            "a\n\n",
            "abc\ndef\n",
            "abc def\nghi\n",
            "\n",
            "\n\n",
            "abc ",
            "abc def",
            "abc\ndef",
            "",
            "a",
        ];
        for text in corpus {
            for width in [1, 6, 18, 600] {
                assert_eq!(
                    split_lines_wrapped(&[plain(text)], width, &w6s),
                    split_lines_wrapped_variant(&[plain(text)], width, &w6s),
                    "{text:?} at {width}",
                );
            }
        }
    }

    #[test]
    fn the_finder_floors_max_width_at_one() {
        // A zero width would otherwise never accept a character and loop.
        assert_eq!(find_line_break(&['a', 'b'], 0, 0, &w6), Some(1));
        assert_eq!(find_line_break(&['a', 'b'], 0, -5, &w6), Some(1));
        assert_eq!(
            find_styled_line_break(&[plain("ab")], 0, &w6s).map(|(p, _)| p),
            Some(1),
        );
    }

    #[test]
    fn the_finder_returns_none_past_the_end() {
        // What `endOfText` looks like from the caller: no break wanted, so the
        // remainder is one line.
        assert_eq!(find_line_break(&['a'], 1, 600, &w6), None);
        assert_eq!(find_line_break(&[], 0, 600, &w6), None);
        assert_eq!(find_styled_line_break(&[], 600, &w6s), None);
    }

    #[test]
    fn a_break_is_never_at_position_zero_without_a_skip() {
        // The termination argument, measured. `hadNonZeroWidthChar` accepts the
        // first character of every line unconditionally, so a width overflow
        // cannot be reported at 0; a break AT 0 is therefore a `\n` or a space
        // and skips one character. If this ever failed, `split_lines_wrapped`
        // would consume nothing and spin.
        for text in ["\na", " a", "abc", "a", "\n", " ", "\u{200b}abc"] {
            for width in [1, 2, 6, 18] {
                if let Some((pos, _)) = find_styled_line_break(&[plain(text)], width, &w6s) {
                    let tail = text.chars().nth(pos).unwrap();
                    assert!(
                        pos + usize::from(tail == '\n' || tail == ' ') > 0,
                        "{text:?} at {width} broke at 0 with no skip",
                    );
                }
            }
        }
    }

    // ---- M128: the events survive the wrap -------------------------------

    fn clickable(text: &str, cmd: &str) -> ChatSpan {
        use crate::chat_events::{ChatEvents, ClickEvent};
        ChatStyle {
            events: Some(std::sync::Arc::new(ChatEvents {
                click: Some(ClickEvent::RunCommand(cmd.into())),
                ..Default::default()
            })),
            ..ChatStyle::WHITE
        }
        .span(text)
    }

    /// **`splitAt` rebuilds the tail from the STYLE, not from the span** —
    /// `self.parts[0] = split_style.span(after)` — so anything a span carries
    /// and a style does not is dropped on the continuation of a width wrap and
    /// kept on its first line. That is why M128 put the events on
    /// [`ChatStyle`]. The failure it prevents is exactly the one that matters
    /// most: a link long enough to wrap, live on line one and dead on line two.
    #[test]
    fn a_click_event_survives_a_width_wrap_onto_the_continuation() {
        use crate::chat_events::ClickEvent;
        let out = split_lines_wrapped(&[clickable("aaa bbb", "/x")], 24, &w6s);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].plain(), "aaa");
        assert_eq!(out[1].plain(), "bbb");
        for l in &out {
            for span in &l.spans {
                assert_eq!(span.click(), Some(&ClickEvent::RunCommand("/x".into())));
            }
        }
    }

    /// And across an explicit newline, which takes the other branch of
    /// `splitAt`'s `position <= size` test.
    #[test]
    fn a_click_event_survives_a_newline_break() {
        use crate::chat_events::ClickEvent;
        let out = split_lines_wrapped(&[clickable("aa\nbb", "/x")], 600, &w6s);
        assert_eq!(out.len(), 2);
        for l in &out {
            for span in &l.spans {
                assert_eq!(span.click(), Some(&ClickEvent::RunCommand("/x".into())));
            }
        }
    }

    /// A break between two differently-linked parts keeps each side's own —
    /// `getSplitStyle` hands over the style at the break, and the head of the
    /// line comes through `ChatSpan { text, ..parts[0].clone() }`.
    #[test]
    fn two_links_on_one_line_keep_their_own_targets() {
        use crate::chat_events::ClickEvent;
        let out = split_lines_wrapped(
            &[clickable("aaa ", "/a"), clickable("bbb", "/b")],
            24,
            &w6s,
        );
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].spans[0].click(), Some(&ClickEvent::RunCommand("/a".into())));
        assert_eq!(out[1].spans[0].click(), Some(&ClickEvent::RunCommand("/b".into())));
    }
}
