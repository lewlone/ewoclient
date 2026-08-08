//! Brigadier's suggestion primitives, and the two `SharedSuggestionProvider`
//! helpers built on them (M114).
//!
//! # This is the first Rewo module whose ground truth is NOT the decompile
//!
//! `com.mojang.brigadier` is a **library**, not part of `net.minecraft`, so
//! `%APPDATA%/EwoClient/rewo/26.2/decompiled` does not contain it — the `com/`
//! tree there holds `blaze3d`, `math` and `realmsclient` and stops. Working
//! from memory is exactly what this project's rules forbid, so the jar Phase B
//! already downloaded
//! (`shared/libraries/com/mojang/brigadier/1.3.10/brigadier-1.3.10.jar`) was
//! decompiled with the same Vineflower that produced the client tree, into
//! `%APPDATA%/EwoClient/rewo/26.2/brigadier-decompiled/`. Every rule below
//! cites that source. **Regenerate it after a version bump** — brigadier's
//! version is pinned by the client jar, and `Suggestions.create`'s sort is the
//! sort of a user-visible list.
//!
//! # `suggest` drops a suggestion equal to what you have already typed
//!
//! ```java
//! public SuggestionsBuilder suggest(String text) {
//!    if (text.equals(this.remaining)) {
//!       return this;
//!    }
//!    ...
//! ```
//!
//! So finishing a name exactly makes it **leave** the popup, and the list a
//! player sees while typing `Stev` is not the list they see at `Steve`. A
//! reader who treats `suggest` as an unconditional push gets a popup with one
//! entry that can never be applied — pressing Tab would replace the word with
//! itself. The comparison is case-**sensitive** and against `remaining`, the
//! text from the builder's start to the end of its input, not against the
//! whole field.
//!
//! # `matchesSubStr`'s splitters are `.`, `_` and `/` — **not** `:`
//!
//! ```java
//! CharMatcher MATCH_SPLITTER = CharMatcher.anyOf("._/");
//! ```
//!
//! The pattern has to match at position 0 or immediately after one of those
//! three, which is why typing `bar` offers `Foo_Bar` — the case that matters
//! most in practice, since an underscore is the one punctuation character a
//! Minecraft name may contain. It is **not** a substring search, and it does
//! **not** split on `:`, so `stone` does not match `minecraft:stone` here.
//! (Namespaces are handled a layer up, by `sortSuggestions`' separate
//! `"minecraft:" + lastWord` test, and by the `suggestResource` family this
//! module does not need.) Guessing either the splitter set or "it's a
//! substring match" produces a list that looks plausible and is wrong for
//! every underscore-bearing name.
//!
//! # `Suggestions.create` dedupes, re-expands, and sorts case-insensitively
//!
//! Three steps, and the middle one is the surprise: every suggestion is
//! `expand`ed to the **widest** range in the set, padding its text with the
//! surrounding command characters so that all of them replace the same span.
//! With one builder every range is identical and `expand` is the identity, so
//! the step is invisible until suggestions from two sources are merged.
//!
//! **The dedupe is a `HashSet` and the sort is stable**, so vanilla's order for
//! two suggestions with equal text and different tooltips is *unspecified* —
//! it is whatever the hash set iterated. Rewo dedupes in insertion order
//! instead, which agrees with vanilla whenever the texts are distinct (the
//! ordinary case) and is deterministic when they are not. Anything
//! user-visible has to impose an order; `CommandTree::top_level` records the
//! same rule for the same reason.

/// `StringRange` — a half-open `[start, end)` span of the input.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StringRange {
    pub start: usize,
    pub end: usize,
}

impl StringRange {
    pub fn between(start: usize, end: usize) -> Self {
        Self { start, end }
    }

    /// `StringRange.at` — an empty range, which is what `Suggestions.EMPTY`
    /// carries.
    pub fn at(pos: usize) -> Self {
        Self {
            start: pos,
            end: pos,
        }
    }

    pub fn length(self) -> usize {
        self.end.saturating_sub(self.start)
    }

    pub fn is_empty(self) -> bool {
        self.start == self.end
    }
}

/// One suggestion: the span it replaces, its text, and an optional tooltip.
///
/// The tooltip is a `Message` in brigadier and only ever reaches Rewo already
/// flattened to a string, from `command_suggestions`' optional `Component`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Suggestion {
    pub range: StringRange,
    pub text: String,
    pub tooltip: Option<String>,
}

impl Suggestion {
    pub fn new(range: StringRange, text: impl Into<String>) -> Self {
        Self {
            range,
            text: text.into(),
            tooltip: None,
        }
    }

    pub fn with_tooltip(mut self, tooltip: Option<String>) -> Self {
        self.tooltip = tooltip;
        self
    }

    /// `Suggestion.apply` — splice this suggestion's text over its range.
    ///
    /// The whole-string case is an explicit early return in brigadier rather
    /// than a special case of the splice; both produce the same string, so the
    /// branch is an optimisation and is kept only because it makes the
    /// transcription checkable line by line.
    ///
    /// **Indices are Java string indices**, so the caller must have built the
    /// range against the same UTF-16 view. `input` here is a `&[u16]` for that
    /// reason — the same choice [`crate::edit_box`] makes and for the same
    /// reason.
    pub fn apply(&self, input: &[u16]) -> Vec<u16> {
        let text: Vec<u16> = self.text.encode_utf16().collect();
        if self.range.start == 0 && self.range.end == input.len() {
            return text;
        }
        let mut out = Vec::new();
        if self.range.start > 0 {
            out.extend_from_slice(&input[..self.range.start.min(input.len())]);
        }
        out.extend_from_slice(&text);
        if self.range.end < input.len() {
            out.extend_from_slice(&input[self.range.end..]);
        }
        out
    }

    /// `Suggestion.expand` — widen this suggestion to `range` by padding its
    /// text with `command`'s own characters on whichever side grew.
    ///
    /// Returns `self` unchanged when the range already matches, which is the
    /// single-builder case and therefore almost always.
    fn expand(&self, command: &[u16], range: StringRange) -> Suggestion {
        if range == self.range {
            return self.clone();
        }
        let mut out: Vec<u16> = Vec::new();
        if range.start < self.range.start {
            out.extend_from_slice(&command[range.start..self.range.start.min(command.len())]);
        }
        out.extend(self.text.encode_utf16());
        if range.end > self.range.end {
            out.extend_from_slice(&command[self.range.end.min(command.len())..range.end.min(command.len())]);
        }
        Suggestion {
            range,
            text: String::from_utf16_lossy(&out),
            tooltip: self.tooltip.clone(),
        }
    }
}

/// A built suggestion list: the common range plus the sorted entries.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Suggestions {
    pub range: StringRange,
    pub list: Vec<Suggestion>,
}

impl Suggestions {
    /// `Suggestions.EMPTY` — range `(0, 0)`, **not** the builder's range.
    ///
    /// Nothing in vanilla reads an empty set's range (every consumer tests
    /// `isEmpty()` first), so the value is only observable to a reader who
    /// forgets that test.
    pub fn empty() -> Self {
        Self {
            range: StringRange::at(0),
            list: Vec::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    /// `Suggestions.create`.
    ///
    /// Widest range → expand every entry to it → dedupe → sort by
    /// `compareToIgnoreCase`. See the module docs for why the dedupe is by
    /// insertion order here and by hash order in vanilla.
    pub fn create(command: &[u16], suggestions: Vec<Suggestion>) -> Self {
        if suggestions.is_empty() {
            return Self::empty();
        }
        let start = suggestions.iter().map(|s| s.range.start).min().unwrap();
        let end = suggestions.iter().map(|s| s.range.end).max().unwrap();
        let range = StringRange::between(start, end);
        let mut expanded: Vec<Suggestion> = Vec::with_capacity(suggestions.len());
        for s in &suggestions {
            let e = s.expand(command, range);
            if !expanded.contains(&e) {
                expanded.push(e);
            }
        }
        // `List.sort` is TimSort, i.e. stable — which is why equal texts keep
        // whatever order the dedupe produced rather than being reordered.
        expanded.sort_by(|a, b| compare_ignore_case(&a.text, &b.text));
        Self {
            range,
            list: expanded,
        }
    }

    /// `Suggestions.merge` — used when more than one node contributes.
    ///
    /// The one-element case returns that element **unchanged**, so a single
    /// source is not re-expanded or re-sorted; only the multi-source path goes
    /// through `create`.
    pub fn merge(command: &[u16], input: Vec<Suggestions>) -> Self {
        if input.is_empty() {
            return Self::empty();
        }
        if input.len() == 1 {
            return input.into_iter().next().unwrap();
        }
        let mut all = Vec::new();
        for s in input {
            for e in s.list {
                if !all.contains(&e) {
                    all.push(e);
                }
            }
        }
        Self::create(command, all)
    }
}

/// `SuggestionsBuilder` — accumulates suggestions over `input[start..]`.
///
/// `input` is UTF-16 because every index brigadier hands out is a Java string
/// index.
pub struct SuggestionsBuilder {
    input: Vec<u16>,
    start: usize,
    remaining: String,
    result: Vec<Suggestion>,
}

impl SuggestionsBuilder {
    pub fn new(input: &[u16], start: usize) -> Self {
        let start = start.min(input.len());
        Self {
            input: input.to_vec(),
            start,
            remaining: String::from_utf16_lossy(&input[start..]),
            result: Vec::new(),
        }
    }

    /// Convenience for callers holding a `&str`.
    pub fn from_str(input: &str, start: usize) -> Self {
        let units: Vec<u16> = input.encode_utf16().collect();
        Self::new(&units, start)
    }

    pub fn remaining(&self) -> &str {
        &self.remaining
    }

    pub fn start(&self) -> usize {
        self.start
    }

    /// The builder's whole input, in the unit its indices mean.
    pub fn input_units(&self) -> Vec<u16> {
        self.input.clone()
    }

    /// `SuggestionsBuilder.suggest(String)` — including the early return that
    /// **drops** a suggestion equal to `remaining`. See the module docs.
    pub fn suggest(&mut self, text: &str) -> &mut Self {
        self.suggest_with_tooltip(text, None)
    }

    pub fn suggest_with_tooltip(&mut self, text: &str, tooltip: Option<String>) -> &mut Self {
        if text == self.remaining {
            return self;
        }
        self.result.push(
            Suggestion::new(StringRange::between(self.start, self.input.len()), text)
                .with_tooltip(tooltip),
        );
        self
    }

    /// `SuggestionsBuilder.build`.
    pub fn build(self) -> Suggestions {
        Suggestions::create(&self.input, self.result)
    }
}

/// `SharedSuggestionProvider.matchesSubStr`.
///
/// True when `pattern` occurs at position 0 of `input` or immediately after a
/// `.`, `_` or `/`. Both arguments are expected already lower-cased by the
/// caller — [`suggest_matching`] does that, and doing it here instead would
/// lower-case the pattern once per candidate.
pub fn matches_sub_str(pattern: &str, input: &str) -> bool {
    // Byte indices are safe here only because the three splitters are ASCII:
    // a `.`/`_`/`/` byte can never occur inside a multi-byte UTF-8 sequence,
    // and `str::starts_with` on a byte offset that is not a char boundary
    // would panic — so the offsets this walks are always boundaries.
    let bytes = input.as_bytes();
    let mut index = 0usize;
    loop {
        if index <= input.len() && input[index..].starts_with(pattern) {
            return true;
        }
        let Some(off) = bytes[index..]
            .iter()
            .position(|b| matches!(b, b'.' | b'_' | b'/'))
        else {
            return false;
        };
        index = index + off + 1;
    }
}

/// `SharedSuggestionProvider.suggest(Iterable<String>, SuggestionsBuilder)`.
///
/// Both sides are lower-cased for the match and the **original** casing is
/// what gets suggested, which is what makes `val` offer `Valtteri`.
pub fn suggest_matching<'a>(
    values: impl IntoIterator<Item = &'a str>,
    builder: &mut SuggestionsBuilder,
) {
    let lower_prefix = builder.remaining().to_lowercase();
    for name in values {
        if matches_sub_str(&lower_prefix, &name.to_lowercase()) {
            builder.suggest(name);
        }
    }
}

/// `String.compareToIgnoreCase`, transcribed rather than approximated.
///
/// Java compares **UTF-16 code units**, folding each through
/// `Character.toUpperCase` and then, only on a mismatch, `Character.toLowerCase`
/// — the double fold is what makes Georgian and Cherokee sort the way they do,
/// and it is not the same as comparing two lower-cased strings.
///
/// `Character.toUpperCase(char)` returns its argument unchanged when the
/// uppercase mapping is not a single code unit (German `ß` is the standard
/// example), which is why [`fold`] keeps the original in that case rather than
/// taking the first character of a multi-character expansion.
///
/// **The one stated divergence** is the Unicode version: Rust's `char::to_*case`
/// tables and the JDK's are independently versioned, so a code point whose
/// casing changed between them sorts differently. Every input Rewo actually
/// feeds this — player names, which are `[A-Za-z0-9_]` — is ASCII, where the
/// two agree by construction.
fn compare_ignore_case(a: &str, b: &str) -> std::cmp::Ordering {
    let mut ai = a.encode_utf16();
    let mut bi = b.encode_utf16();
    loop {
        match (ai.next(), bi.next()) {
            (None, None) => return std::cmp::Ordering::Equal,
            (None, Some(_)) => return std::cmp::Ordering::Less,
            (Some(_), None) => return std::cmp::Ordering::Greater,
            (Some(x), Some(y)) => {
                if x == y {
                    continue;
                }
                let (ux, uy) = (fold(x, true), fold(y, true));
                if ux == uy {
                    continue;
                }
                let (lx, ly) = (fold(ux, false), fold(uy, false));
                if lx != ly {
                    return lx.cmp(&ly);
                }
            }
        }
    }
}

/// `Character.toUpperCase(char)` / `toLowerCase(char)` — single-code-unit only.
///
/// A lone surrogate is not a scalar value, so `char::from_u32` rejects it and
/// it passes through unchanged. Java reaches the same answer by a different
/// route: a surrogate code unit has no case mapping.
fn fold(u: u16, upper: bool) -> u16 {
    let Some(c) = char::from_u32(u as u32) else {
        return u;
    };
    let mut it: Box<dyn Iterator<Item = char>> = if upper {
        Box::new(c.to_uppercase())
    } else {
        Box::new(c.to_lowercase())
    };
    match (it.next(), it.next()) {
        (Some(one), None) => {
            let v = one as u32;
            if v <= 0xFFFF {
                v as u16
            } else {
                u
            }
        }
        _ => u,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utf16(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    fn texts(s: &Suggestions) -> Vec<&str> {
        s.list.iter().map(|e| e.text.as_str()).collect()
    }

    // ── matchesSubStr ────────────────────────────────────────────────────

    #[test]
    fn a_pattern_matches_at_the_start_or_after_a_splitter() {
        assert!(matches_sub_str("val", "valtteri"));
        // The case that matters: an underscore is the only punctuation a
        // Minecraft name may carry, and `bar` finding `foo_bar` is why typing
        // the second half of a name completes it.
        assert!(matches_sub_str("bar", "foo_bar"));
        assert!(matches_sub_str("b", "a.b"));
        assert!(matches_sub_str("b", "a/b"));
        // Not a substring search: `oo` is inside `foo_bar` and does not sit at
        // the start or after a splitter.
        assert!(!matches_sub_str("oo", "foo_bar"));
    }

    #[test]
    fn a_colon_is_not_a_splitter() {
        // `CharMatcher.anyOf("._/")`. Namespaced ids are matched a layer up by
        // `sortSuggestions`' own `"minecraft:" + word` test, not here — so a
        // reader who adds `:` to the set makes this function answer a question
        // vanilla answers somewhere else, and double-counts it.
        assert!(!matches_sub_str("stone", "minecraft:stone"));
    }

    #[test]
    fn an_empty_pattern_matches_everything_including_an_empty_input() {
        // `input.startsWith("", 0)` is true, so the loop exits immediately —
        // which is what makes an empty field offer the whole list.
        assert!(matches_sub_str("", "anything"));
        assert!(matches_sub_str("", ""));
    }

    #[test]
    fn a_pattern_longer_than_its_input_finds_no_splitter_and_fails() {
        assert!(!matches_sub_str("valtteri", "val"));
        assert!(!matches_sub_str("x", ""));
    }

    #[test]
    fn a_trailing_splitter_does_not_run_off_the_end() {
        // index lands exactly on `input.len()`, where `startsWith` is false for
        // a non-empty pattern and the next search has nothing to find.
        assert!(!matches_sub_str("x", "a_"));
        assert!(matches_sub_str("", "a_"));
    }

    // ── the builder ──────────────────────────────────────────────────────

    #[test]
    fn a_suggestion_equal_to_the_remaining_text_is_dropped() {
        // The finding. Typing a name in full removes it from the popup, so the
        // list at `Steve` is shorter than the list at `Stev`.
        let mut b = SuggestionsBuilder::from_str("Steve", 0);
        b.suggest("Steve");
        b.suggest("Steven");
        assert_eq!(texts(&b.build()), ["Steven"]);
    }

    #[test]
    fn the_drop_is_case_sensitive_and_measured_against_remaining_not_the_field() {
        // `text.equals(remaining)` — `String.equals`, not `equalsIgnoreCase`.
        let mut b = SuggestionsBuilder::from_str("steve", 0);
        b.suggest("Steve");
        assert_eq!(texts(&b.build()), ["Steve"]);

        // With a non-zero start the comparison is against the tail only, so
        // the same text is dropped here and kept above.
        let mut b = SuggestionsBuilder::from_str("hi Steve", 3);
        b.suggest("Steve");
        assert!(b.build().is_empty());
    }

    #[test]
    fn every_suggestion_from_one_builder_spans_start_to_the_end_of_the_input() {
        let mut b = SuggestionsBuilder::from_str("hello wo", 6);
        b.suggest("world");
        let s = b.build();
        assert_eq!(s.range, StringRange::between(6, 8));
        assert_eq!(s.list[0].range, StringRange::between(6, 8));
    }

    // ── create ───────────────────────────────────────────────────────────

    #[test]
    fn an_empty_set_carries_the_zero_range_rather_than_the_builders() {
        let b = SuggestionsBuilder::from_str("hello wo", 6);
        let s = b.build();
        assert!(s.is_empty());
        assert_eq!(s.range, StringRange::at(0));
    }

    #[test]
    fn the_sort_is_case_insensitive_so_capitals_do_not_come_first() {
        // A case-SENSITIVE sort puts every capitalised name ahead of every
        // lower-case one, which for a player list is the whole list in the
        // wrong order.
        let mut b = SuggestionsBuilder::from_str("", 0);
        for n in ["zeta", "Alpha", "beta"] {
            b.suggest(n);
        }
        assert_eq!(texts(&b.build()), ["Alpha", "beta", "zeta"]);
    }

    #[test]
    fn duplicate_suggestions_collapse() {
        let mut b = SuggestionsBuilder::from_str("", 0);
        b.suggest("a");
        b.suggest("a");
        b.suggest("b");
        assert_eq!(texts(&b.build()), ["a", "b"]);
    }

    #[test]
    fn merging_two_sources_expands_both_to_the_wider_range() {
        // The step that is invisible with one builder: the narrower
        // suggestion is re-texted with the command's own characters so that
        // both replace the same span.
        let command = utf16("say hello");
        let wide = Suggestions {
            range: StringRange::between(0, 9),
            list: vec![Suggestion::new(StringRange::between(0, 9), "tell hello")],
        };
        let narrow = Suggestions {
            range: StringRange::between(4, 9),
            list: vec![Suggestion::new(StringRange::between(4, 9), "helium")],
        };
        let merged = Suggestions::merge(&command, vec![wide, narrow]);
        assert_eq!(merged.range, StringRange::between(0, 9));
        // "helium" grew a leading "say " so that it, too, replaces 0..9.
        assert_eq!(texts(&merged), ["say helium", "tell hello"]);
    }

    #[test]
    fn merging_one_source_returns_it_untouched() {
        // `input.size() == 1` short-circuits before `create`, so a single
        // source is neither re-expanded nor re-sorted — an out-of-order list
        // survives, which is only reachable by constructing one directly.
        let out_of_order = Suggestions {
            range: StringRange::between(0, 1),
            list: vec![
                Suggestion::new(StringRange::between(0, 1), "z"),
                Suggestion::new(StringRange::between(0, 1), "a"),
            ],
        };
        let merged = Suggestions::merge(&utf16("x"), vec![out_of_order.clone()]);
        assert_eq!(merged, out_of_order);
    }

    // ── apply ────────────────────────────────────────────────────────────

    #[test]
    fn applying_a_suggestion_splices_it_over_its_range() {
        let field = utf16("hello wo there");
        let s = Suggestion::new(StringRange::between(6, 8), "world");
        assert_eq!(
            String::from_utf16_lossy(&s.apply(&field)),
            "hello world there"
        );
    }

    #[test]
    fn a_whole_string_range_replaces_everything() {
        let field = utf16("wo");
        let s = Suggestion::new(StringRange::between(0, 2), "world");
        assert_eq!(String::from_utf16_lossy(&s.apply(&field)), "world");
    }

    #[test]
    fn apply_indexes_utf16_units_not_scalars() {
        // An astral character is two units to Java and one `char` to Rust, so
        // a scalar-indexed splice lands a unit early on every field that
        // contains one. The emoji here is U+1F600.
        let field = utf16("\u{1F600}ab");
        assert_eq!(field.len(), 4);
        let s = Suggestion::new(StringRange::between(2, 4), "XY");
        assert_eq!(String::from_utf16_lossy(&s.apply(&field)), "\u{1F600}XY");
    }

    // ── the comparator ───────────────────────────────────────────────────

    #[test]
    fn the_comparator_orders_ascii_case_insensitively_and_by_length_on_a_tie() {
        use std::cmp::Ordering::*;
        assert_eq!(compare_ignore_case("abc", "ABD"), Less);
        assert_eq!(compare_ignore_case("ABC", "abc"), Equal);
        assert_eq!(compare_ignore_case("abc", "abcd"), Less);
        assert_eq!(compare_ignore_case("", "a"), Less);
        // `_` is 0x5F: above every upper-case letter and below every
        // lower-case one, so the two folds disagree about it and the SECOND
        // one decides. This assertion was written backwards first — reasoning
        // "underscore is above the capitals" stops at the upper fold, where
        // `_` (0x5F) does beat `Z` (0x5A); the comparison does not stop there,
        // and against `z` (0x7A) it loses. The oracle settled it, not the
        // argument.
        assert_eq!(compare_ignore_case("A_b", "AZb"), Less);
    }

    /// The port graded against the real `brigadier-1.3.10.jar`, not against
    /// itself.
    ///
    /// Every expectation below is a line of `tools/suggestion_oracle`'s output
    /// under Temurin 25, pasted verbatim. A test that computes its expectation
    /// from this module's own constants is self-calibrating — M93q's finding,
    /// and M93r's sweep for it — and here the risk is sharper than usual,
    /// because the source these rules come from is a jar rather than a file
    /// anybody reviewing the diff can read.
    ///
    /// **Re-run the oracle after a version bump.** Brigadier's version is
    /// pinned by the client jar, and `Suggestions.create`'s sort is a
    /// user-visible order.
    #[test]
    fn the_port_agrees_with_the_brigadier_jar() {
        // `BUILD <label> <start> <end> <texts…>`
        let build = |input: &str, start: usize, offered: &[&str]| -> (StringRange, Vec<String>) {
            let mut b = SuggestionsBuilder::from_str(input, start);
            for t in offered {
                b.suggest(t);
            }
            let s = b.build();
            (s.range, s.list.iter().map(|e| e.text.clone()).collect())
        };
        let cases: &[(&str, usize, &[&str], (usize, usize), &[&str])] = &[
            ("Steve", 0, &["Steve", "Steven"], (0, 5), &["Steven"]),
            ("steve", 0, &["Steve"], (0, 5), &["Steve"]),
            // The drop leaves nothing, and an empty set reports (0, 0) rather
            // than the builder's (3, 8).
            ("hi Steve", 3, &["Steve"], (0, 0), &[]),
            ("", 0, &["zeta", "Alpha", "beta"], (0, 0), &["Alpha", "beta", "zeta"]),
            ("", 0, &["a", "a", "b"], (0, 0), &["a", "b"]),
            ("", 0, &["AZb", "A_b"], (0, 0), &["A_b", "AZb"]),
            ("hello wo", 6, &[], (0, 0), &[]),
        ];
        for (input, start, offered, (rs, re), want) in cases {
            let (range, got) = build(input, *start, offered);
            let got: Vec<&str> = got.iter().map(String::as_str).collect();
            assert_eq!(
                (range, got.as_slice()),
                (StringRange::between(*rs, *re), *want),
                "BUILD {input:?} start {start}"
            );
        }

        // `MERGE expand 0 9 say helium tell hello`
        let command: Vec<u16> = "say hello".encode_utf16().collect();
        let merged = Suggestions::merge(
            &command,
            vec![
                Suggestions {
                    range: StringRange::between(0, 9),
                    list: vec![Suggestion::new(StringRange::between(0, 9), "tell hello")],
                },
                Suggestions {
                    range: StringRange::between(4, 9),
                    list: vec![Suggestion::new(StringRange::between(4, 9), "helium")],
                },
            ],
        );
        assert_eq!(merged.range, StringRange::between(0, 9));
        assert_eq!(texts(&merged), ["say helium", "tell hello"]);

        // `APPLY splice hello world there` / `APPLY whole world`
        assert_eq!(
            String::from_utf16_lossy(
                &Suggestion::new(StringRange::between(6, 8), "world").apply(&utf16("hello wo there"))
            ),
            "hello world there"
        );
        assert_eq!(
            String::from_utf16_lossy(
                &Suggestion::new(StringRange::between(0, 2), "world").apply(&utf16("wo"))
            ),
            "world"
        );

        // `CMP <a> <b> <signum>` — Java's own `String.compareToIgnoreCase`.
        for (a, b, want) in [
            ("abc", "ABD", -1),
            ("ABC", "abc", 0),
            ("abc", "abcd", -1),
            ("", "a", -1),
            ("A_b", "AZb", -1),
            ("_", "a", -1),
            ("_", "A", -1),
            ("Alpha", "beta", -1),
            // The three pairs that separate Java's upper-THEN-lower fold from
            // a plain lower-only one, which agree on all of ASCII and so were
            // missing from the fixture until a surviving mutation asked for
            // them. `lower(upper('\u{131}'))` is `i` and `lower('\u{131}')` is
            // itself, so a lower-only comparator answers Greater here where
            // Java answers Equal.
            ("\u{131}", "i", 0),
            ("\u{131}", "I", 0),
            ("\u{df}", "\u{1e9e}", 0),
        ] {
            let got = match compare_ignore_case(a, b) {
                std::cmp::Ordering::Less => -1,
                std::cmp::Ordering::Equal => 0,
                std::cmp::Ordering::Greater => 1,
            };
            assert_eq!(got, want, "CMP {a:?} {b:?}");
        }
    }

    #[test]
    fn a_multi_unit_uppercase_mapping_leaves_its_character_alone() {
        // `Character.toUpperCase('ß')` is `ß`, because "SS" does not fit in a
        // char. Taking the first character of the expansion instead would fold
        // it to `S` and sort it among the S-words.
        assert_eq!(fold('ß' as u16, true), 'ß' as u16);
        assert_eq!(fold('a' as u16, true), 'A' as u16);
        assert_eq!(fold('A' as u16, false), 'a' as u16);
        // A lone surrogate is not a scalar value and has no case mapping.
        assert_eq!(fold(0xD800, true), 0xD800);
    }
}
