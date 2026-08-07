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
//! that part is genuinely one algorithm and is [`find_line_break`] here. What
//! differs is what gets *emitted*:
//!
//! | | `splitLines(String, …)` | `splitLines(FormattedText, …)` |
//! |---|---|---|
//! | per-line flag | none | `isWrapped` |
//! | trailing `\n` | `"a\n"` → `["a"]` | `"a\n"` → `["a", ""]` |
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
//! # What is deliberately not transcribed
//!
//! **Style runs.** Vanilla's `FormattedText` overload flattens a component
//! into `LineComponent` parts and reassembles them across the break with
//! `ComponentCollector`; Rewo flattens a component to plain text long before
//! it reaches here, so there is one part and `splitAt`'s multi-part walk
//! degenerates to a substring. Every break position is unaffected — the finder
//! runs over the concatenated `flatParts` either way. The same list of
//! omissions [`crate::disconnect_screen::split_lines`] records (the `§`-code
//! re-emission, surrogate pairs, bidi) applies unchanged.

/// `StringSplitter.LineBreakFinder` — one line's sweep.
///
/// Returns the position to break at, or `None` for `endOfText` (the sweep ran
/// off the end without wanting a break, so the remainder is one line).
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

/// One line out of the `FormattedText` overload: its text, and whether the
/// break *before* it was a width wrap rather than a `\n`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WrappedLine {
    pub text: String,
    /// `isWrapped` — true when the preceding break was a width wrap. The first
    /// line is always `false`.
    pub wrapped: bool,
}

/// `StringSplitter.splitLines(FormattedText, int, Style, BiConsumer)`.
///
/// See the module docs for how this differs from
/// [`crate::disconnect_screen::split_lines`], which transcribes the `String`
/// overload. An empty input yields **no** lines here; the one-empty-line rule
/// belongs to `wrapComponents`, not to the splitter — see
/// `crate::chat::wrap_components`.
pub fn split_lines_wrapped(
    text: &str,
    max_width: i32,
    width_of: &dyn Fn(&str) -> i32,
) -> Vec<WrappedLine> {
    let chars: Vec<char> = text.chars().collect();
    let mut out: Vec<WrappedLine> = Vec::new();
    let mut pos = 0usize;
    let mut is_wrapped = false;
    let mut force_new_line = false;
    while let Some(line_break) = find_line_break(&chars, pos, max_width, width_of) {
        let tail = chars[line_break];
        let is_new_line = tail == '\n';
        // `skipNextChar = isNewLine || firstTailChar == ' '` — the breaking
        // character is dropped only when it is one of those two.
        let skip = usize::from(is_new_line || tail == ' ');
        force_new_line = is_new_line;
        out.push(WrappedLine {
            text: chars[pos..line_break].iter().collect(),
            wrapped: is_wrapped,
        });
        is_wrapped = !is_new_line;
        pos = line_break + skip;
    }
    // `getRemainder()` is null exactly when `splitAt` removed the last part,
    // i.e. when the break consumed the string to its end.
    if pos < chars.len() {
        out.push(WrappedLine {
            text: chars[pos..].iter().collect(),
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
            text: String::new(),
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
    /// compared on identical inputs.
    fn w6(s: &str) -> i32 {
        s.chars().count() as i32 * 6
    }

    fn lines(text: &str, max_width: i32) -> Vec<(String, bool)> {
        split_lines_wrapped(text, max_width, &w6)
            .into_iter()
            .map(|l| (l.text, l.wrapped))
            .collect()
    }

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
        assert_eq!(lines("abc def", 18)[0].1, false);
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
            let b: Vec<String> = split_lines_wrapped(text, width, &w6)
                .into_iter()
                .map(|l| l.text)
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

    /// The same function with the trailing line's flag read from `is_wrapped`
    /// instead of vanilla's literal `false` — the surviving mutation, kept here
    /// so the claim of equivalence is measured rather than argued.
    fn split_lines_wrapped_variant(
        text: &str,
        max_width: i32,
        width_of: &dyn Fn(&str) -> i32,
    ) -> Vec<WrappedLine> {
        let chars: Vec<char> = text.chars().collect();
        let mut out: Vec<WrappedLine> = Vec::new();
        let mut pos = 0usize;
        let mut is_wrapped = false;
        let mut force_new_line = false;
        while let Some(line_break) = find_line_break(&chars, pos, max_width, width_of) {
            let tail = chars[line_break];
            let is_new_line = tail == '\n';
            let skip = usize::from(is_new_line || tail == ' ');
            force_new_line = is_new_line;
            out.push(WrappedLine {
                text: chars[pos..line_break].iter().collect(),
                wrapped: is_wrapped,
            });
            is_wrapped = !is_new_line;
            pos = line_break + skip;
        }
        if pos < chars.len() {
            out.push(WrappedLine {
                text: chars[pos..].iter().collect(),
                wrapped: is_wrapped,
            });
        } else if force_new_line {
            out.push(WrappedLine {
                text: String::new(),
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
                    split_lines_wrapped(text, width, &w6),
                    split_lines_wrapped_variant(text, width, &w6),
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
    }

    #[test]
    fn the_finder_returns_none_past_the_end() {
        // What `endOfText` looks like from the caller: no break wanted, so the
        // remainder is one line.
        assert_eq!(find_line_break(&['a'], 1, 600, &w6), None);
        assert_eq!(find_line_break(&[], 0, 600, &w6), None);
    }
}
