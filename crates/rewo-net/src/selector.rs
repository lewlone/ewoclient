//! `EntitySelectorParser` — the `@e[…]` syntax, client side (M118).
//!
//! M116 made every `minecraft:` argument type `Unknown`, which stopped the
//! parse at a command's second word and handed every argument completion to
//! the server. This is the first one transcribed, and it is the one worth
//! doing first: `minecraft:entity` is the second word of `/tp`, `/kill`,
//! `/give`, `/effect` and most of the rest.
//!
//! # The suggestion state is a FUNCTION POINTER the parse reassigns
//!
//! ```java
//! this.suggestions = this::suggestNameOrSelector;   // parse()
//! this.suggestions = this::suggestSelector;         // parseSelector()
//! this.suggestions = this::suggestOpenOptions;      // …after the type char
//! this.suggestions = this::suggestOptionsKeyOrClose;// …after '['
//! ```
//!
//! and `fillSuggestions` simply calls whatever it last held, against a builder
//! offset to the reader's cursor. **That is the whole mechanism**, and it is
//! why suggestions keep working when the parse throws: `EntityArgument
//! .listSuggestions` wraps `parser.parse()` in a `try` whose `catch` body is
//! **empty**, then calls `fillSuggestions` anyway. A selector half-typed is
//! always a selector that failed to parse.
//!
//! It also means an option handler can install a suggester and *then* fail.
//! `sort` does exactly that — `setSuggestions(...)` is called **before**
//! `readUnquotedString`'s result is matched — so `@e[sort=` offers the four
//! orders even though nothing valid has been typed yet. Transcribing the
//! handler in the other order gives an option that can never suggest.
//!
//! # `suggestEquals` is dead code in vanilla
//!
//! It is defined and **never assigned** — the same shape as
//! `SuggestionsList`'s `unselectedColor` and the recipe book's
//! `int border = 4`. `parseOptions` goes straight from the key to
//! `SUGGEST_NOTHING`, so a cursor sitting between a key and its `=` offers
//! nothing at all. Recorded rather than "fixed", because adding the state
//! would be inventing behaviour.
//!
//! # `@s` hides options rather than rejecting them
//!
//! `limit` and `sort` carry `!s.isCurrentEntity()` in their `canUse`, and
//! `suggestNames` filters on the same predicate — so `@s[` offers a **shorter
//! list** than `@e[`. Modelling `canUse` as always-true gives a popup that
//! offers `limit` on a selector that cannot take one.
//!
//! # What is transcribed, and what is not
//!
//! The **shape** is complete: the six selector types, the option registry in
//! its registration order, `canUse`, and every suggestion state. The option
//! **values** are transcribed where they are self-contained — an integer, a
//! word, a coordinate, a range, a name — and left unparsed for the four that
//! need a structured parser Rewo does not have (`nbt`, `scores`,
//! `advancements`, `predicate`). An unparsed value throws, which is what the
//! empty `catch` above is for: the option list still completes, and only the
//! text after that option is left uncoloured.

use crate::dispatcher::{ReaderError, StringReader};
use rewo_world::suggestions::SuggestionsBuilder;

/// The six selector types, in `fillSelectorSuggestions`' order — which is
/// **not** alphabetical and **not** the order `parseSelector`'s switch lists
/// them in. The popup shows them in this order after `Suggestions.create`
/// sorts, so the order here only decides ties; it is kept faithful anyway.
pub const SELECTORS: [(&str, &str); 6] = [
    ("@p", "argument.entity.selector.nearestPlayer"),
    ("@a", "argument.entity.selector.allPlayers"),
    ("@r", "argument.entity.selector.randomPlayer"),
    ("@s", "argument.entity.selector.self"),
    ("@e", "argument.entity.selector.allEntities"),
    ("@n", "argument.entity.selector.nearestEntity"),
];

/// How an option's `canUse` is gated.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gate {
    /// `ALWAYS_AVAILABLE` — repeatable, and offered forever.
    Always,
    /// Offered until it has been parsed once (`getDistance() == null` and its
    /// eight siblings, and the `canParse()` family).
    Once,
    /// `Once`, and additionally hidden on `@s`: `!s.isCurrentEntity() && …`.
    OnceNotCurrentEntity,
}

/// How an option's value is read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Value {
    Int,
    Double,
    /// `MinMaxBounds` — `a`, `a..b`, `a..`, `..b`.
    Range,
    /// `readString`, i.e. quoted or unquoted.
    Str,
    /// `readUnquotedString`, optionally prefixed with `!`.
    Word,
    /// One of a fixed set, with a suggester installed **before** the match.
    /// `sort`'s shape: `SharedSuggestionProvider.suggest`, so the match is
    /// `matchesSubStr` rather than a prefix test.
    Choice(&'static [&'static str]),
    /// `gamemode`'s shape: the same, plus a leading `!`. Its suggester
    /// strips the `!` from the prefix and offers `!name` **before** `name`,
    /// and offers both when nothing is typed — where `sort`'s offers one
    /// list through a different matcher entirely.
    InvertibleChoice(&'static [&'static str]),
    /// A structured parser Rewo does not have. Throws; see the module docs.
    Unsupported,
}

/// One entry of `EntitySelectorOptions.OPTIONS`, in registration order —
/// vanilla's is a `LinkedHashMap` and `suggestNames` iterates it, so this
/// order is the order the popup's ties resolve in.
pub struct Opt {
    pub name: &'static str,
    pub gate: Gate,
    pub value: Value,
    /// The translation key of the tooltip `suggestNames` attaches.
    pub description: &'static str,
}

const fn opt(name: &'static str, gate: Gate, value: Value) -> Opt {
    Opt {
        name,
        gate,
        value,
        description: "",
    }
}

/// The 21 options, in `EntitySelectorOptions.bootStrap`'s order.
pub static OPTIONS: &[Opt] = &[
    opt("name", Gate::Once, Value::Str),
    opt("distance", Gate::Once, Value::Range),
    opt("level", Gate::Once, Value::Range),
    opt("x", Gate::Once, Value::Double),
    opt("y", Gate::Once, Value::Double),
    opt("z", Gate::Once, Value::Double),
    opt("dx", Gate::Once, Value::Double),
    opt("dy", Gate::Once, Value::Double),
    opt("dz", Gate::Once, Value::Double),
    opt("x_rotation", Gate::Once, Value::Range),
    opt("y_rotation", Gate::Once, Value::Range),
    opt("limit", Gate::OnceNotCurrentEntity, Value::Int),
    opt(
        "sort",
        Gate::OnceNotCurrentEntity,
        Value::Choice(&["nearest", "furthest", "random", "arbitrary"]),
    ),
    opt(
        "gamemode",
        Gate::Once,
        Value::InvertibleChoice(&["survival", "creative", "adventure", "spectator"]),
    ),
    opt("team", Gate::Once, Value::Word),
    opt("type", Gate::Once, Value::Word),
    opt("tag", Gate::Always, Value::Word),
    opt("nbt", Gate::Always, Value::Unsupported),
    opt("scores", Gate::Once, Value::Unsupported),
    opt("advancements", Gate::Once, Value::Unsupported),
    opt("predicate", Gate::Always, Value::Unsupported),
];

/// What `fillSuggestions` should offer, i.e. `this.suggestions`'s current
/// value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Suggest {
    /// `suggestNameOrSelector` — the online names **and** the six selectors.
    NameOrSelector,
    /// `suggestSelector` — the six, from one character further back so that
    /// the `@` already typed is replaced too.
    Selector,
    /// `suggestName` — the names, from `startPosition`.
    Name,
    /// `suggestOpenOptions` — just `[`.
    OpenOptions,
    /// `suggestOptionsKeyOrClose` — `]` and every usable option name.
    ///
    /// **Unreachable in vanilla**, and transcribed anyway: `parseSelector`
    /// sets it and `parseOptions`' first statement replaces it. See
    /// `suggestEquals` in the module docs for the other one.
    OptionsKeyOrClose,
    /// `suggestOptionsKey` — every usable option name.
    OptionsKey,
    /// `suggestOptionsNextOrClose` — `,` and `]`.
    OptionsNextOrClose,
    /// `SUGGEST_NOTHING`, and an option's own suggester where it has one.
    Choice(&'static [&'static str]),
    InvertibleChoice(&'static [&'static str]),
    Nothing,
}

/// The parser's state, reduced to what the suggestion path reads.
pub struct SelectorParser {
    /// `startPosition`, which `suggestName` offsets its builder to.
    pub start_position: usize,
    /// `currentEntity` — set only by `@s`, and what hides `limit`/`sort`.
    pub current_entity: bool,
    /// Which options have been parsed, by index into [`OPTIONS`].
    parsed: Vec<usize>,
    /// `this.suggestions`.
    pub suggestions: Suggest,
    /// Where the builder offsets to — `fillSuggestions` uses
    /// `builder.createOffset(this.reader.getCursor())`.
    pub cursor: usize,
    /// Whether `parse()` threw. The suggestion path ignores it — that is the
    /// empty `catch` — but `EntityArgument.parse` does not.
    pub failed: bool,
}

impl SelectorParser {
    /// `EntitySelectorParser.parse`.
    ///
    /// `allow_selectors` is the client's `COMMANDS_ENTITY_SELECTORS`
    /// permission. It is **true** for the suggestion provider — the same
    /// `ALLOW_RESTRICTED_COMMANDS` union M116 records — so a client always
    /// offers `@e` even against a server that would refuse it.
    pub fn parse(reader: &mut StringReader, allow_selectors: bool) -> Self {
        let mut me = Self {
            start_position: reader.cursor(),
            current_entity: false,
            parsed: Vec::new(),
            suggestions: Suggest::NameOrSelector,
            cursor: reader.cursor(),
            failed: false,
        };
        me.failed = me.run(reader, allow_selectors).is_err();
        me.cursor = reader.cursor();
        me
    }

    /// The parse proper. Its `Err` is deliberately discarded by [`Self::parse`]
    /// — `EntityArgument.listSuggestions` catches and drops it, then calls
    /// `fillSuggestions` on whatever state was reached.
    fn run(&mut self, reader: &mut StringReader, allow_selectors: bool) -> Result<(), ReaderError> {
        if reader.can_read() && reader.peek() == b'@' as u16 {
            if !allow_selectors {
                return Err(ReaderError::UnknownArgumentType);
            }
            reader.skip();
            self.parse_selector(reader)
        } else {
            self.parse_name_or_uuid(reader)
        }
    }

    /// `parseNameOrUUID`.
    ///
    /// The `if (this.reader.canRead())` guard is load-bearing: with **nothing**
    /// typed the state stays `suggestNameOrSelector`, which offers the six
    /// selectors as well as the names. One character in, it becomes
    /// `suggestName` and the selectors disappear — because `@` can only be
    /// the first character.
    fn parse_name_or_uuid(&mut self, reader: &mut StringReader) -> Result<(), ReaderError> {
        if reader.can_read() {
            self.suggestions = Suggest::Name;
        }
        let start = reader.cursor();
        let name = reader.read_string()?;
        // A UUID is 36 characters, so the length test is what separates the
        // two; vanilla tries `UUID.fromString` first and falls through on the
        // exception. A name of 0 or more than 16 characters is neither.
        let is_uuid = name.len() == 36 && name.chars().filter(|c| *c == '-').count() == 4;
        if !is_uuid && (name.is_empty() || name.chars().count() > 16) {
            reader.set_cursor(start);
            return Err(ReaderError::UnknownArgumentType);
        }
        Ok(())
    }

    /// `parseSelector`.
    fn parse_selector(&mut self, reader: &mut StringReader) -> Result<(), ReaderError> {
        self.suggestions = Suggest::Selector;
        if !reader.can_read() {
            return Err(ReaderError::UnknownArgumentType);
        }
        let start = reader.cursor();
        let ty = reader.read();
        match ty as u8 as char {
            'a' | 'e' | 'n' | 'p' | 'r' => {}
            's' => self.current_entity = true,
            _ => {
                reader.set_cursor(start);
                return Err(ReaderError::UnknownArgumentType);
            }
        }
        self.suggestions = Suggest::OpenOptions;
        if reader.can_read() && reader.peek() == b'[' as u16 {
            reader.skip();
            self.suggestions = Suggest::OptionsKeyOrClose;
            return self.parse_options(reader);
        }
        Ok(())
    }

    /// `parseOptions`.
    fn parse_options(&mut self, reader: &mut StringReader) -> Result<(), ReaderError> {
        self.suggestions = Suggest::OptionsKey;
        skip_whitespace(reader);
        while reader.can_read() && reader.peek() != b']' as u16 {
            skip_whitespace(reader);
            let start = reader.cursor();
            let key = reader.read_string()?;
            let Some(index) = OPTIONS.iter().position(|o| o.name == key) else {
                reader.set_cursor(start);
                return Err(ReaderError::UnknownArgumentType);
            };
            if !self.can_use(index) {
                reader.set_cursor(start);
                return Err(ReaderError::UnknownArgumentType);
            }
            skip_whitespace(reader);
            if !reader.can_read() || reader.peek() != b'=' as u16 {
                reader.set_cursor(start);
                return Err(ReaderError::UnknownArgumentType);
            }
            reader.skip();
            skip_whitespace(reader);
            // `SUGGEST_NOTHING` FIRST, then the handler — which may install
            // its own suggester and only afterwards fail on the value.
            self.suggestions = Suggest::Nothing;
            self.parse_value(reader, index)?;
            self.parsed.push(index);
            skip_whitespace(reader);
            self.suggestions = Suggest::OptionsNextOrClose;
            if reader.can_read() {
                if reader.peek() != b',' as u16 {
                    if reader.peek() != b']' as u16 {
                        return Err(ReaderError::UnknownArgumentType);
                    }
                    break;
                }
                reader.skip();
                self.suggestions = Suggest::OptionsKey;
            }
        }
        if reader.can_read() {
            reader.skip();
            self.suggestions = Suggest::Nothing;
            Ok(())
        } else {
            // Falling off the end of `[…` without a `]` is an error, and the
            // state stays whatever the loop left — which is what keeps the
            // popup useful on a half-typed selector.
            Err(ReaderError::UnknownArgumentType)
        }
    }

    fn parse_value(&mut self, reader: &mut StringReader, index: usize) -> Result<(), ReaderError> {
        match OPTIONS[index].value {
            Value::Int => reader.read_i32().map(|_| ()),
            Value::Double => reader.read_f64().map(|_| ()),
            Value::Range => read_range(reader),
            Value::Str => reader.read_string().map(|_| ()),
            Value::Word => {
                // `shouldInvertValue` — a leading `!`.
                if reader.can_read() && reader.peek() == b'!' as u16 {
                    reader.skip();
                }
                reader.read_unquoted_string();
                Ok(())
            }
            Value::Choice(_) | Value::InvertibleChoice(_) => {
                let choices = match OPTIONS[index].value {
                    Value::Choice(c) => {
                        self.suggestions = Suggest::Choice(c);
                        c
                    }
                    Value::InvertibleChoice(c) => {
                        self.suggestions = Suggest::InvertibleChoice(c);
                        c
                    }
                    _ => unreachable!(),
                };
                // The suggester goes in BEFORE the value is matched, which is
                // the whole reason `@e[sort=` can offer anything.
                let start = reader.cursor();
                if reader.can_read() && reader.peek() == b'!' as u16 {
                    reader.skip();
                }
                let word = reader.read_unquoted_string();
                if choices.contains(&word.as_str()) {
                    Ok(())
                } else {
                    // `rollbackAndThrow` — the cursor goes BACK to the start
                    // of the value before the exception. That is what makes
                    // the suggester useful: `fillSuggestions` offsets its
                    // builder to the reader's cursor, so without the rollback
                    // the remaining text is empty, every choice matches, and
                    // choosing one appends rather than replaces — `@e[sort=n`
                    // would become `@e[sort=nnearest`.
                    reader.set_cursor(start);
                    Err(ReaderError::UnknownArgumentType)
                }
            }
            Value::Unsupported => Err(ReaderError::UnknownArgumentType),
        }
    }

    /// `EntitySelectorOptions.get`'s `canUse` test, and the same one
    /// `suggestNames` filters on.
    pub fn can_use(&self, index: usize) -> bool {
        match OPTIONS[index].gate {
            Gate::Always => true,
            Gate::Once => !self.parsed.contains(&index),
            Gate::OnceNotCurrentEntity => !self.current_entity && !self.parsed.contains(&index),
        }
    }

    /// `fillSuggestions` — apply the state the parse left.
    ///
    /// `names` supplies the online player names, which only the two name
    /// states use. The builder is offset to the reader's cursor by the caller,
    /// exactly as `builder.createOffset(this.reader.getCursor())` does.
    pub fn fill_suggestions(&self, builder: &mut SuggestionsBuilder, names: &[String]) {
        match &self.suggestions {
            Suggest::NameOrSelector => {
                for n in names {
                    builder.suggest(n);
                }
                for (s, _) in SELECTORS {
                    builder.suggest(s);
                }
            }
            // `suggestName` offsets to `startPosition`; with the whole
            // selector being one word that is where the builder already is
            // for our caller, so the names go in directly.
            Suggest::Name => {
                for n in names {
                    builder.suggest(n);
                }
            }
            // `suggestSelector` builds at `getStart() - 1` so the `@` already
            // typed is part of what gets replaced — otherwise choosing `@e`
            // after typing `@` gives `@@e`.
            Suggest::Selector => {
                for (s, _) in SELECTORS {
                    builder.suggest(s);
                }
            }
            Suggest::OpenOptions => {
                builder.suggest("[");
            }
            Suggest::OptionsKeyOrClose => {
                builder.suggest("]");
                self.suggest_option_names(builder);
            }
            Suggest::OptionsKey => self.suggest_option_names(builder),
            Suggest::OptionsNextOrClose => {
                builder.suggest(",");
                builder.suggest("]");
            }
            // `sort`'s: `SharedSuggestionProvider.suggest`, which matches
            // through `matchesSubStr` rather than a prefix test. For these
            // four words the two agree — none contains a `.`, `_` or `/` —
            // so the choice is invisible here and faithful anyway.
            Suggest::Choice(choices) => {
                rewo_world::suggestions::suggest_matching(choices.iter().copied(), builder);
            }
            // `gamemode`'s: strip a leading `!` from the prefix, and offer the
            // INVERTED form first. Reusing the plain matcher above leaves
            // `@e[gamemode=!` offering nothing, because no mode starts with
            // `!`.
            Suggest::InvertibleChoice(choices) => {
                let mut prefix = builder.remaining().to_lowercase();
                let (mut add_normal, mut add_inverted) = (true, true);
                if !prefix.is_empty() {
                    if prefix.starts_with('!') {
                        add_normal = false;
                        prefix.remove(0);
                    } else {
                        add_inverted = false;
                    }
                }
                for c in *choices {
                    if c.starts_with(&prefix) {
                        if add_inverted {
                            builder.suggest(&format!("!{c}"));
                        }
                        if add_normal {
                            builder.suggest(c);
                        }
                    }
                }
            }
            Suggest::Nothing => {}
        }
    }

    /// `EntitySelectorOptions.suggestNames` — **`key + "="`**, not the bare
    /// key, so choosing one leaves the caret ready for a value.
    fn suggest_option_names(&self, builder: &mut SuggestionsBuilder) {
        let lower_prefix = builder.remaining().to_lowercase();
        for (i, o) in OPTIONS.iter().enumerate() {
            if self.can_use(i) && o.name.to_lowercase().starts_with(&lower_prefix) {
                builder.suggest(&format!("{}=", o.name));
            }
        }
    }
}

/// `StringReader.skipWhitespace` — `Character.isWhitespace`, which unlike
/// `\s` in a regex *is* Unicode-aware. Only the ASCII cases are reachable from
/// a chat field, and only those are transcribed.
fn skip_whitespace(reader: &mut StringReader) {
    while reader.can_read() && matches!(reader.peek(), 0x20 | 0x09 | 0x0A | 0x0B | 0x0C | 0x0D) {
        reader.skip();
    }
}

/// `MinMaxBounds` — `a`, `a..b`, `a..`, `..b`.
///
/// The bare `..` is legal and means "unbounded both ways", which is why the
/// emptiness test is on **both** halves rather than on the first.
pub fn read_range(reader: &mut StringReader) -> Result<(), ReaderError> {
    let number = |r: &mut StringReader| {
        let start = r.cursor();
        while r.can_read() && StringReader::is_allowed_number(r.peek()) {
            // `..` is two dots, and `isAllowedNumber` admits a dot — so a
            // naive scan swallows the separator. Stop at the first of a pair.
            if r.peek() == b'.' as u16
                && r.string().get(r.cursor() + 1).copied() == Some(b'.' as u16)
            {
                break;
            }
            r.skip();
        }
        r.cursor() > start
    };
    let has_low = number(reader);
    let mut has_high = false;
    if reader.can_read_len(2)
        && reader.peek() == b'.' as u16
        && reader.string()[reader.cursor() + 1] == b'.' as u16
    {
        reader.skip();
        reader.skip();
        has_high = number(reader);
    }
    if has_low || has_high {
        Ok(())
    } else {
        Err(ReaderError::ExpectedInt)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names() -> Vec<String> {
        ["Steve", "Alex"].iter().map(|s| s.to_string()).collect()
    }

    /// Parse `input` and return what the popup would offer at its end.
    fn offer(input: &str) -> Vec<String> {
        let units: Vec<u16> = input.encode_utf16().collect();
        let mut reader = StringReader::new(&units);
        let p = SelectorParser::parse(&mut reader, true);
        let mut builder = SuggestionsBuilder::new(&units, p.cursor);
        p.fill_suggestions(&mut builder, &names());
        builder
            .build()
            .list
            .into_iter()
            .map(|s| s.text)
            .collect()
    }

    #[test]
    fn an_empty_argument_offers_the_names_and_the_six_selectors() {
        let o = offer("");
        assert!(o.contains(&"Steve".to_string()));
        assert!(o.contains(&"@e".to_string()));
        assert_eq!(o.iter().filter(|s| s.starts_with('@')).count(), 6);
    }

    #[test]
    fn one_character_in_the_selectors_are_gone() {
        // `parseNameOrUUID`'s `if (canRead())` guard: `@` can only be the
        // first character, so once anything is typed the state moves to
        // `suggestName` and the selectors stop being offered.
        let o = offer("St");
        assert!(o.contains(&"Steve".to_string()));
        assert!(!o.iter().any(|s| s.starts_with('@')));
    }

    #[test]
    fn an_at_sign_offers_the_six_selectors_and_no_names() {
        let o = offer("@");
        assert_eq!(o.len(), 6);
        assert!(o.contains(&"@e".to_string()));
        assert!(!o.contains(&"Steve".to_string()));
    }

    #[test]
    fn a_selector_type_offers_the_opening_bracket() {
        assert_eq!(offer("@e"), ["["]);
    }

    #[test]
    fn an_open_bracket_offers_every_option_name_with_an_equals_and_NOT_the_close() {
        // Written the other way round first, and the correction is a finding:
        // `parseSelector` sets `suggestOptionsKeyOrClose` and then calls
        // `parseOptions`, whose FIRST statement is
        // `this.suggestions = this::suggestOptionsKey`. So the "or close"
        // state is overwritten before anything can observe it — dead, like
        // `suggestEquals`. Two of the seven states in this class are
        // unreachable.
        let o = offer("@e[");
        assert!(!o.contains(&"]".to_string()));
        // `suggestNames` suggests `key + "="`, so choosing one leaves the
        // caret ready for a value. The bare key would need a second keystroke
        // and would not match what vanilla inserts.
        assert!(o.contains(&"limit=".to_string()));
        assert!(!o.contains(&"limit".to_string()));
        assert_eq!(o.len(), OPTIONS.len());
    }

    #[test]
    fn at_s_hides_the_options_it_cannot_take() {
        // `!s.isCurrentEntity()` on `limit` and `sort`, and `suggestNames`
        // filters on the same predicate. Always-true `canUse` offers `limit`
        // on a selector that cannot have one.
        let all = offer("@e[");
        let s = offer("@s[");
        assert!(all.contains(&"limit=".to_string()));
        assert!(!s.contains(&"limit=".to_string()));
        assert!(!s.contains(&"sort=".to_string()));
        assert_eq!(s.len(), all.len() - 2);
    }

    #[test]
    fn a_parsed_option_stops_being_offered_and_a_repeatable_one_does_not() {
        let o = offer("@e[limit=1,");
        assert!(!o.contains(&"limit=".to_string()), "Gate::Once");
        assert!(o.contains(&"tag=".to_string()), "Gate::Always");
        assert!(o.contains(&"sort=".to_string()));
    }

    #[test]
    fn a_complete_option_offers_the_comma_and_the_close() {
        let mut o = offer("@e[limit=1");
        o.sort();
        assert_eq!(o, [",", "]"]);
    }

    #[test]
    fn an_option_with_a_suggester_offers_its_values_before_one_is_typed() {
        // `setSuggestions` is called BEFORE the value is matched, so the four
        // orders are offered on an empty value. Installing it afterwards gives
        // an option that can never suggest.
        let mut o = offer("@e[sort=");
        o.sort();
        assert_eq!(o, ["arbitrary", "furthest", "nearest", "random"]);
        assert_eq!(offer("@e[sort=n"), ["nearest"]);
    }

    #[test]
    fn an_option_without_a_suggester_offers_nothing() {
        // `SUGGEST_NOTHING`, which is the correct answer for most options and
        // is why the popup vanishes while you type a number.
        assert!(offer("@e[limit=").is_empty());
    }

    #[test]
    fn a_closed_selector_offers_nothing() {
        assert!(offer("@e[limit=1]").is_empty());
    }

    #[test]
    fn a_gamemode_value_offers_both_forms_until_the_bang_settles_it() {
        // With nothing typed, both — and the INVERTED one first, which is
        // `b.suggest("!" + name)` sitting above `b.suggest(name)`.
        let o = offer("@e[gamemode=");
        assert!(o.contains(&"survival".to_string()));
        assert!(o.contains(&"!survival".to_string()));
        // A `!` clears `addNormal`, so only the inverted forms remain — this
        // witness asserted `survival` first, which is the branch's whole
        // purpose inverted.
        let o = offer("@e[gamemode=!");
        assert!(o.contains(&"!survival".to_string()));
        assert!(!o.contains(&"survival".to_string()));
        // …and anything else clears `addInverted`.
        let o = offer("@e[gamemode=s");
        assert!(o.contains(&"survival".to_string()));
        assert!(o.contains(&"spectator".to_string()));
        assert!(!o.iter().any(|s| s.starts_with('!')));
    }

    #[test]
    fn an_unsupported_option_value_still_leaves_the_option_list_usable() {
        // `nbt` needs a structured parser Rewo does not have, so its value
        // throws — and the empty `catch` in `EntityArgument.listSuggestions`
        // is what keeps that from costing anything but the text after it.
        assert!(offer("@e[nbt={a:1}]").is_empty());
        // The option itself is still offered.
        assert!(offer("@e[").contains(&"nbt=".to_string()));
    }

    // ── the range reader ─────────────────────────────────────────────────

    #[test]
    fn a_range_accepts_all_four_of_its_shapes() {
        for s in ["5", "1..9", "1..", "..9", ".."] {
            let units: Vec<u16> = s.encode_utf16().collect();
            let mut r = StringReader::new(&units);
            let out = read_range(&mut r);
            if s == ".." {
                // Both halves empty: vanilla's `MinMaxBounds` rejects it, and
                // so does this.
                assert!(out.is_err(), "{s}");
            } else {
                assert!(out.is_ok(), "{s}");
                assert_eq!(r.cursor(), units.len(), "{s} consumed exactly");
            }
        }
    }

    #[test]
    fn a_range_does_not_swallow_its_own_separator() {
        // `isAllowedNumber` admits `.`, so a naive scan reads `1..9` as one
        // malformed number and the `..` is never seen.
        let units: Vec<u16> = "1..9".encode_utf16().collect();
        let mut r = StringReader::new(&units);
        assert!(read_range(&mut r).is_ok());
        assert_eq!(r.cursor(), 4);
    }

    #[test]
    fn a_distance_range_parses_and_the_option_is_then_spent() {
        let o = offer("@e[distance=1..9,");
        assert!(!o.contains(&"distance=".to_string()));
        assert!(o.contains(&"limit=".to_string()));
    }
}
