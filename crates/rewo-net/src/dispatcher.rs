//! Brigadier's `CommandDispatcher`, client side (M116).
//!
//! M113 decoded the command tree and M114 built the popup over it, but with no
//! parser the popup could not answer a single thing locally — every `/` went
//! to the server. This is the parser: `StringReader`, `parseNodes`,
//! `findSuggestionContext` and `getCompletionSuggestions`, transcribed from
//! the same `brigadier-1.3.10.jar` [`rewo_world::suggestions`] documents.
//!
//! # `canUse` is ALWAYS true for suggestions, and `FLAG_RESTRICTED` is not it
//!
//! The natural wiring — a restricted node is one this player may not run, so
//! hide it — is wrong, and wrong in the direction that silently removes
//! commands from your autocomplete. `ClientPacketListener` builds **two**
//! providers:
//!
//! ```java
//! this.suggestionsProvider = new ClientSuggestionProvider(this, minecraft,
//!     playerPermissions.union(ALLOW_RESTRICTED_COMMANDS));
//! this.restrictedSuggestionsProvider = new ClientSuggestionProvider(this, minecraft,
//!     PermissionSet.NO_PERMISSIONS);
//! ```
//!
//! and `getSuggestionsProvider()` — the one `CommandSuggestions` uses —
//! returns the **first**, which is granted the restricted permission
//! explicitly. The second exists for one purpose: `checkCommand` parses twice
//! and, when a command parses *with* permissions and fails *without*, opens a
//! send-confirmation window. So the flag governs a prompt before sending, not
//! what the popup offers. (M113 guessed that `hasAllowedInput` would consult
//! it; that reads `ChatAbilities`, which is a different packet and a different
//! question.)
//!
//! Rewo has no permission model at all, so `can_use` is a constant `true` and
//! the flag stays decoded-and-unused until there is a confirmation window to
//! gate.
//!
//! # `getRelevantNodes` is a fast path that changes the ANSWER, not just the speed
//!
//! ```java
//! if (this.literals.size() > 0) {
//!    ... read one word ...
//!    LiteralCommandNode<S> literal = this.literals.get(text);
//!    return literal != null ? Collections.singleton(literal) : this.arguments.values();
//! }
//! ```
//!
//! When the next word matches a literal exactly, **only that literal is
//! tried** — the sibling arguments are not. So `/give` never attempts to parse
//! `give` as a player name, and a node with both a `stone` literal and a block
//! argument resolves the literal. Iterating all children instead would add
//! parses that succeed and change which `potentials` entry wins.
//!
//! # An unparseable argument is not a failure, it is where the parse stops
//!
//! Rewo transcribes the six brigadier primitives — the six whose properties
//! M113 already decodes off the wire — and every Minecraft argument type is
//! [`ArgKind::Unknown`], which refuses. `parseNodes` records that in `errors`
//! and returns the best partial parse, which is exactly what it does for a
//! genuinely malformed input. The parse therefore reaches as far as the
//! literals and primitives allow and stops at the first `minecraft:` argument.
//!
//! That is enough for the thing this was built for: **literal completion is
//! local**. `/g` offers `gamemode`/`give` with no packet at all, where M114
//! asked the server for every keystroke.
//!
//! # Where it still asks the server, and why that is correct rather than lazy
//!
//! `SuggestionProviders.getProvider(name)` is
//! `PROVIDERS_BY_NAME.getOrDefault(name, ASK_SERVER)` — **an unrecognised
//! provider id falls back to asking**, it is not an error. Three are
//! registered: `ask_server`, `available_sounds` and `summonable_entities`.
//! Rewo routes all three to the server: the last two suggest from data it does
//! have (M64's sound registry, `rewo_data`'s entity types) but through
//! `suggestResource`'s namespace-aware matcher, which is machinery this
//! milestone does not need — and the server returns the same list, so the
//! deviation costs a packet and not an answer.
//!
//! The important half is that [`Completion::ask_server`] is now *narrow*.
//! Vanilla merges the local and remote futures; Rewo prefers the server's
//! reply outright when any contributing child asks, because
//! `handleCustomCommandSuggestions` parses the whole input with the server's
//! own dispatcher and returns `getCompletionSuggestions` — literals included —
//! so its answer is a superset of the local one at that position.

use crate::commands::{ArgumentProps, CommandNode, CommandTree, NodeKind, StringType};
use rewo_world::suggestions::{StringRange, Suggestions, SuggestionsBuilder};

/// `com.mojang.brigadier.StringReader`.
///
/// Indexes **UTF-16 code units**, because every `StringRange` brigadier hands
/// out is a Java string index and the popup measures against them.
#[derive(Clone, Debug)]
pub struct StringReader {
    string: Vec<u16>,
    cursor: usize,
}

impl StringReader {
    pub fn new(string: &[u16]) -> Self {
        Self {
            string: string.to_vec(),
            cursor: 0,
        }
    }

    pub fn from_str(s: &str) -> Self {
        Self::new(&s.encode_utf16().collect::<Vec<_>>())
    }

    pub fn string(&self) -> &[u16] {
        &self.string
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn set_cursor(&mut self, cursor: usize) {
        self.cursor = cursor;
    }

    pub fn total_length(&self) -> usize {
        self.string.len()
    }

    pub fn remaining_length(&self) -> usize {
        self.string.len().saturating_sub(self.cursor)
    }

    pub fn can_read_len(&self, length: usize) -> bool {
        self.cursor + length <= self.string.len()
    }

    pub fn can_read(&self) -> bool {
        self.can_read_len(1)
    }

    pub fn peek(&self) -> u16 {
        self.string.get(self.cursor).copied().unwrap_or(0)
    }

    pub fn skip(&mut self) {
        self.cursor += 1;
    }

    pub fn read(&mut self) -> u16 {
        let c = self.peek();
        self.cursor += 1;
        c
    }

    pub fn remaining(&self) -> &[u16] {
        &self.string[self.cursor.min(self.string.len())..]
    }

    /// `isAllowedNumber` — digits, `.` and `-`. **Not `+` and not `e`**, so
    /// `1e5` stops at the `e` and `+1` reads as empty; both then fail their
    /// argument's range check rather than parsing.
    pub fn is_allowed_number(c: u16) -> bool {
        (0x30..=0x39).contains(&c) || c == b'.' as u16 || c == b'-' as u16
    }

    /// `isAllowedInUnquotedString` — alphanumerics plus `_ - . +`.
    ///
    /// Note `+` **is** allowed here and is **not** allowed in a number, which
    /// is the one place the two sets disagree.
    pub fn is_allowed_in_unquoted_string(c: u16) -> bool {
        (0x30..=0x39).contains(&c)
            || (0x41..=0x5A).contains(&c)
            || (0x61..=0x7A).contains(&c)
            || c == b'_' as u16
            || c == b'-' as u16
            || c == b'.' as u16
            || c == b'+' as u16
    }

    pub fn is_quoted_string_start(c: u16) -> bool {
        c == b'"' as u16 || c == b'\'' as u16
    }

    fn read_number_text(&mut self) -> String {
        let start = self.cursor;
        while self.can_read() && Self::is_allowed_number(self.peek()) {
            self.skip();
        }
        String::from_utf16_lossy(&self.string[start..self.cursor])
    }

    /// `readInt`. **Java's `Integer.parseInt`**, so a value outside `i32`
    /// throws rather than saturating — and the cursor is put back before the
    /// error, which is what lets `parseNodes` retry the next sibling from the
    /// same place.
    pub fn read_i32(&mut self) -> Result<i32, ReaderError> {
        let start = self.cursor;
        let text = self.read_number_text();
        if text.is_empty() {
            return Err(ReaderError::ExpectedInt);
        }
        text.parse::<i32>().map_err(|_| {
            self.cursor = start;
            ReaderError::InvalidInt(text)
        })
    }

    pub fn read_i64(&mut self) -> Result<i64, ReaderError> {
        let start = self.cursor;
        let text = self.read_number_text();
        if text.is_empty() {
            return Err(ReaderError::ExpectedLong);
        }
        text.parse::<i64>().map_err(|_| {
            self.cursor = start;
            ReaderError::InvalidLong(text)
        })
    }

    pub fn read_f64(&mut self) -> Result<f64, ReaderError> {
        let start = self.cursor;
        let text = self.read_number_text();
        if text.is_empty() {
            return Err(ReaderError::ExpectedDouble);
        }
        text.parse::<f64>().map_err(|_| {
            self.cursor = start;
            ReaderError::InvalidDouble(text)
        })
    }

    pub fn read_f32(&mut self) -> Result<f32, ReaderError> {
        let start = self.cursor;
        let text = self.read_number_text();
        if text.is_empty() {
            return Err(ReaderError::ExpectedFloat);
        }
        text.parse::<f32>().map_err(|_| {
            self.cursor = start;
            ReaderError::InvalidFloat(text)
        })
    }

    pub fn read_unquoted_string(&mut self) -> String {
        let start = self.cursor;
        while self.can_read() && Self::is_allowed_in_unquoted_string(self.peek()) {
            self.skip();
        }
        String::from_utf16_lossy(&self.string[start..self.cursor])
    }

    /// `readStringUntil` — with backslash escaping, where **only the
    /// terminator and the backslash itself may be escaped**; anything else is
    /// an error rather than a literal.
    fn read_string_until(&mut self, terminator: u16) -> Result<String, ReaderError> {
        let mut out: Vec<u16> = Vec::new();
        let mut escaped = false;
        while self.can_read() {
            let c = self.read();
            if escaped {
                if c != terminator && c != b'\\' as u16 {
                    self.cursor -= 1;
                    // `readerInvalidEscape().createWithContext(this,
                    // String.valueOf(c))` — the offending character is the
                    // exception's argument, and its message quotes it.
                    return Err(ReaderError::InvalidEscape(
                        String::from_utf16_lossy(&[c]),
                    ));
                }
                out.push(c);
                escaped = false;
            } else if c == b'\\' as u16 {
                escaped = true;
            } else if c == terminator {
                return Ok(String::from_utf16_lossy(&out));
            } else {
                out.push(c);
            }
        }
        Err(ReaderError::ExpectedEndOfQuote)
    }

    /// `readString` — a quoted string when it starts with `"` or `'`, else an
    /// unquoted one. **An empty reader returns `""` rather than erroring**,
    /// which is what lets an argument at the very end of the input parse to
    /// nothing and the popup still offer something.
    pub fn read_string(&mut self) -> Result<String, ReaderError> {
        if !self.can_read() {
            return Ok(String::new());
        }
        let next = self.peek();
        if Self::is_quoted_string_start(next) {
            self.skip();
            self.read_string_until(next)
        } else {
            Ok(self.read_unquoted_string())
        }
    }

    /// `readBoolean`. Case-**sensitive**: `True` is an error, not `true`.
    pub fn read_bool(&mut self) -> Result<bool, ReaderError> {
        let start = self.cursor;
        let value = self.read_string()?;
        match value.as_str() {
            "" => Err(ReaderError::ExpectedBool),
            "true" => Ok(true),
            "false" => Ok(false),
            _ => {
                self.cursor = start;
                Err(ReaderError::InvalidBool(value))
            }
        }
    }
}

/// The `BUILT_IN_EXCEPTIONS` this module can raise.
///
/// Kept as a typed enum rather than a message because `updateUsageInfo`
/// distinguishes `literalIncorrect` from everything else — it counts those and
/// replaces them with a single `dispatcherUnknownArgument` line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReaderError {
    ExpectedInt,
    InvalidInt(String),
    ExpectedLong,
    InvalidLong(String),
    ExpectedDouble,
    InvalidDouble(String),
    ExpectedFloat,
    InvalidFloat(String),
    ExpectedBool,
    InvalidBool(String),
    /// The offending character, as `String.valueOf(c)`.
    InvalidEscape(String),
    ExpectedEndOfQuote,
    /// `IntegerArgumentType`'s own range checks and their siblings.
    ///
    /// Four types × two bounds is eight `Dynamic2CommandExceptionType`s in
    /// `BuiltInExceptions`, and they differ in the type NAME they print, so
    /// one variant carrying the name is the same table written once.
    ///
    /// **The lambda's arguments and the message's are transposed**:
    /// `createWithContext(reader, result, this.minimum)` feeds
    /// `(found, min) -> "Integer must not be less than " + min + ", found " +
    /// found`, so the value that arrives FIRST is printed LAST. Both numbers
    /// are already `String.valueOf`'d here, because only the parse site knows
    /// whether they are an `int`, a `long`, a `float` or a `double` — and
    /// `Float.toString(0.5f)` and `Double.toString(0.5)` are not the same
    /// function.
    OutOfRange {
        kind: NumKind,
        /// `integerTooHigh` rather than `integerTooLow`.
        too_high: bool,
        found: String,
        bound: String,
    },
    /// `literalIncorrect` — the one `updateUsageInfo` counts separately.
    ///
    /// **Unreachable from any brigadier parse**, and the argument is
    /// structural rather than empirical: `getRelevantNodes` hands
    /// `parseNodes` a `LiteralCommandNode` only when the next word equals its
    /// name exactly, and `LiteralCommandNode.parse` on that word always
    /// succeeds. Rewo raises it in one place vanilla has no equivalent for —
    /// a `Root` node found as somebody's child, which a well-formed tree does
    /// not contain. See [`crate::command_errors`] for what that makes dead in
    /// `updateUsageInfo`.
    LiteralIncorrect,
    /// `dispatcherExpectedArgumentSeparator`.
    ExpectedArgumentSeparator,
    /// Rewo's own: an argument type this client cannot parse. Vanilla has no
    /// equivalent because it has every type.
    ///
    /// It is also what a *failed* selector / block / item read reports, so it
    /// covers two different things: "this client has no reader for that type"
    /// and "the reader ran and refused". Neither has a `BuiltInExceptions`
    /// literal to print — see [`crate::command_errors::message`], which
    /// answers `None` for it rather than inventing text.
    UnknownArgumentType,
}

/// Which of the four numeric `BuiltInExceptions` families an
/// [`ReaderError::OutOfRange`] belongs to. The name is printed verbatim.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NumKind {
    Integer,
    Long,
    Float,
    Double,
}

impl NumKind {
    /// The word the message opens with — `"Integer must not be less than…"`.
    pub fn name(self) -> &'static str {
        match self {
            Self::Integer => "Integer",
            Self::Long => "Long",
            Self::Float => "Float",
            Self::Double => "Double",
        }
    }
}

/// The four numeric argument types' shared bound check.
///
/// ```java
/// if (result < this.minimum) { ...integerTooLow()... }
/// else if (result > this.maximum) { ...integerTooHigh()... }
/// ```
///
/// The LOW test runs first, so a range whose min exceeds its max reports the
/// low bound for every value — which is what an `else if` means and is not
/// the same as picking whichever bound is nearer.
fn range_error<T: PartialOrd>(
    kind: NumKind,
    found: T,
    min: T,
    max: T,
    show: impl Fn(&T) -> String,
) -> Option<ReaderError> {
    let (too_high, bound) = if found < min {
        (false, &min)
    } else if found > max {
        (true, &max)
    } else {
        return None;
    };
    Some(ReaderError::OutOfRange {
        kind,
        too_high,
        found: show(&found),
        bound: show(bound),
    })
}

/// A parseable argument type, resolved from the wire's props and type name.
#[derive(Clone, Debug, PartialEq)]
pub enum ArgKind {
    Bool,
    Integer { min: i64, max: i64 },
    Long { min: i64, max: i64 },
    Float { min: f64, max: f64 },
    Double { min: f64, max: f64 },
    Str(StringType),
    /// `minecraft:entity` and `minecraft:game_profile` (M118) — the `@e[…]`
    /// syntax. `playersOnly` differs between them and affects only which
    /// names are offered, which is the caller's list here.
    Entity,
    /// `minecraft:block_state` and `minecraft:block_predicate` (M119).
    BlockState,
    /// `minecraft:item_stack` and `minecraft:item_predicate` (M119).
    ItemStack,
    /// The coordinate family and the value-shaped types (M120), which share
    /// one resolver — see [`crate::arg_types`].
    Value(crate::arg_types::Value),
    /// Every other `minecraft:` type. Refuses to parse — see the module docs.
    Unknown,
}

impl ArgKind {
    /// The wire's `(type name, properties)` pair, as [`crate::commands`]
    /// decodes it.
    ///
    /// **Dispatch is on the NAME, not the props shape.** `brigadier:integer`
    /// and `brigadier:long` share `RangeI64`, and `float`/`double` share
    /// `RangeF64`, so the props alone cannot tell them apart — and the two
    /// differ in exactly the case that matters, a value outside `i32`.
    pub fn resolve(type_name: &str, props: &ArgumentProps) -> Self {
        match (type_name, props) {
            ("brigadier:bool", _) => Self::Bool,
            ("brigadier:integer", ArgumentProps::RangeI64 { min, max }) => Self::Integer {
                min: min.unwrap_or(i32::MIN as i64),
                max: max.unwrap_or(i32::MAX as i64),
            },
            ("brigadier:long", ArgumentProps::RangeI64 { min, max }) => Self::Long {
                min: min.unwrap_or(i64::MIN),
                max: max.unwrap_or(i64::MAX),
            },
            ("brigadier:float", ArgumentProps::RangeF64 { min, max }) => Self::Float {
                min: min.unwrap_or(f32::MIN as f64),
                max: max.unwrap_or(f32::MAX as f64),
            },
            ("brigadier:double", ArgumentProps::RangeF64 { min, max }) => Self::Double {
                min: min.unwrap_or(f64::MIN),
                max: max.unwrap_or(f64::MAX),
            },
            ("brigadier:string", ArgumentProps::String(t)) => Self::Str(*t),
            // The props carry `single`/`playersOnly`; neither changes how the
            // text parses, only which names are worth offering.
            ("minecraft:entity" | "minecraft:game_profile", _) => Self::Entity,
            // The `_predicate` pair take a `#tag` where their plain siblings
            // do not; both readers accept one, so the difference is a
            // validation Rewo does not perform rather than a syntax it cannot
            // read.
            ("minecraft:block_state" | "minecraft:block_predicate", _) => Self::BlockState,
            ("minecraft:item_stack" | "minecraft:item_predicate", _) => Self::ItemStack,
            // M120's family. Tried LAST of the `minecraft:` arms so a type
            // with its own module above wins — `block_predicate` is an id to
            // `arg_types`' eyes and a block state to `block_item`'s, and the
            // richer one must not be shadowed by the ordering.
            (name, _) if crate::arg_types::resolve(name).is_some() => {
                Self::Value(crate::arg_types::resolve(name).expect("just checked"))
            }
            _ => Self::Unknown,
        }
    }

    /// `ArgumentType.parse`. Advances the reader on success.
    ///
    /// Every numeric type re-winds the cursor **before** raising its range
    /// error, which is what makes `parseNodes`' `reader.setCursor(cursor)`
    /// retry land in the right place.
    pub fn parse(&self, reader: &mut StringReader, ctx: CommandCtx<'_>) -> Result<(), ReaderError> {
        match self {
            Self::Bool => reader.read_bool().map(|_| ()),
            Self::Integer { min, max } => {
                let start = reader.cursor();
                let v = reader.read_i32()? as i64;
                if let Some(e) = range_error(NumKind::Integer, v, *min, *max, i64::to_string) {
                    reader.set_cursor(start);
                    return Err(e);
                }
                Ok(())
            }
            Self::Long { min, max } => {
                let start = reader.cursor();
                let v = reader.read_i64()?;
                if let Some(e) = range_error(NumKind::Long, v, *min, *max, i64::to_string) {
                    reader.set_cursor(start);
                    return Err(e);
                }
                Ok(())
            }
            Self::Float { min, max } => {
                let start = reader.cursor();
                let v = reader.read_f32()? as f64;
                // `Float.toString`, not `Double.toString`: the bound is a
                // `float` field and the value came off `readFloat`, so both
                // box to `Float` and print the shortest decimal that
                // round-trips **as an f32**.
                let f = |x: &f64| rewo_world::chat_translate::java_float_string(*x as f32);
                if let Some(e) = range_error(NumKind::Float, v, *min, *max, f) {
                    reader.set_cursor(start);
                    return Err(e);
                }
                Ok(())
            }
            Self::Double { min, max } => {
                let start = reader.cursor();
                let v = reader.read_f64()?;
                let f = |x: &f64| rewo_world::chat_translate::java_double_string(*x);
                if let Some(e) = range_error(NumKind::Double, v, *min, *max, f) {
                    reader.set_cursor(start);
                    return Err(e);
                }
                Ok(())
            }
            // `GREEDY_PHRASE` takes the WHOLE remainder and leaves the cursor
            // at the end, which is why a `/say`-shaped command never has a
            // node after its message.
            Self::Str(StringType::GreedyPhrase) => {
                reader.set_cursor(reader.total_length());
                Ok(())
            }
            Self::Str(StringType::SingleWord) => {
                reader.read_unquoted_string();
                Ok(())
            }
            Self::Str(StringType::QuotablePhrase) => reader.read_string().map(|_| ()),
            // `EntityArgument.parse` builds a parser and runs it; a failure
            // is a failure, and the reader is left wherever the selector's own
            // rollback put it.
            Self::Entity => {
                let mut probe = reader.clone();
                let p = crate::selector::SelectorParser::parse(&mut probe, true);
                if p.failed {
                    return Err(ReaderError::UnknownArgumentType);
                }
                reader.set_cursor(probe.cursor());
                Ok(())
            }
            Self::BlockState | Self::ItemStack => {
                let mut probe = reader.clone();
                let reg = ctx.registry();
                // With an empty context nothing resolves, so the parse fails
                // and the argument behaves exactly as it did before M119.
                let p = if matches!(self, Self::BlockState) {
                    crate::block_item::ParsedRef::parse_block(&mut probe, reg)
                } else {
                    crate::block_item::ParsedRef::parse_item(&mut probe, reg)
                };
                if p.failed {
                    return Err(ReaderError::UnknownArgumentType);
                }
                reader.set_cursor(probe.cursor());
                Ok(())
            }
            Self::Value(v) => v.parse(reader),
            Self::Unknown => Err(ReaderError::UnknownArgumentType),
        }
    }

    /// `ArgumentType.listSuggestions`.
    ///
    /// `BoolArgumentType` has one, and so does `EntityArgument` — whose is a
    /// whole parser, so it takes the online names the caller must supply.
    fn suggest_with(
        &self,
        builder: &mut SuggestionsBuilder,
        ctx: CommandCtx<'_>,
        registry: Option<&str>,
    ) {
        if let Self::Entity = self {
            // `EntityArgument.listSuggestions` re-reads from the builder's own
            // start rather than continuing an earlier parse, and swallows the
            // exception — see `crate::selector`.
            let input: Vec<u16> = builder.input_units();
            let mut reader = StringReader::new(&input);
            reader.set_cursor(builder.start());
            let p = crate::selector::SelectorParser::parse(&mut reader, true);
            let mut sub = SuggestionsBuilder::new(&input, p.cursor);
            p.fill_suggestions(&mut sub, ctx.names);
            for s in sub.build().list {
                builder.suggest(&s.text);
            }
            return;
        }
        if let Self::BlockState | Self::ItemStack = self {
            // Same shape as the selector's: re-read from the builder's own
            // start, swallow the exception, and apply whatever state the
            // parse reached.
            let input: Vec<u16> = builder.input_units();
            let mut reader = StringReader::new(&input);
            reader.set_cursor(builder.start());
            let reg = ctx.registry();
            let block = matches!(self, Self::BlockState);
            let p = if block {
                crate::block_item::ParsedRef::parse_block(&mut reader, reg)
            } else {
                crate::block_item::ParsedRef::parse_item(&mut reader, reg)
            };
            let mut sub = SuggestionsBuilder::new(&input, p.cursor);
            p.fill_suggestions(&mut sub, reg, block);
            for s in sub.build().list {
                builder.suggest(&s.text);
            }
            return;
        }
        if let Self::Value(v) = self {
            // The registry the wire named, where there was one — M113 keeps it
            // in the props and this is its first consumer.
            v.suggest(builder, registry, ctx.blocks, ctx.items);
            return;
        }
        if *self != Self::Bool {
            return;
        }
        // `"true".startsWith(getRemainingLowerCase())` — the LITERAL starts
        // with what you typed, not the other way round.
        let remaining = builder.remaining().to_lowercase();
        for word in ["true", "false"] {
            if word.starts_with(&remaining) {
                builder.suggest(word);
            }
        }
    }
}

/// Everything the argument types need that the tree does not carry: the
/// online names, and the block and item registries (M119).
///
/// Vanilla passes a `CommandSourceStack` for the same reason. Both parsing and
/// suggesting take it, because an argument that cannot resolve its id cannot
/// parse either — with an empty context every `minecraft:` type behaves as it
/// did before M118, which is what keeps a registry-less caller working.
#[derive(Clone, Copy, Default)]
pub struct CommandCtx<'a> {
    pub names: &'a [String],
    pub blocks: Option<&'a rewo_data::blocks::Blocks>,
    pub items: Option<&'a rewo_data::items::Items>,
}

impl<'a> CommandCtx<'a> {
    fn registry(&self) -> crate::block_item::Registry<'a> {
        crate::block_item::Registry {
            blocks: self.blocks,
            items: self.items,
        }
    }
}

/// One node the parse consumed, with the span it covered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedNode {
    pub node: i32,
    pub range: StringRange,
}

/// `CommandContextBuilder`, reduced to what the suggestion path reads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContextBuilder {
    pub root: i32,
    pub range: StringRange,
    pub nodes: Vec<ParsedNode>,
    /// `getArguments()` — the parsed ARGUMENTS, which is not the same list as
    /// `nodes`: literals are absent from it, and it is what
    /// `CommandSuggestions.formatText` colours.
    ///
    /// A `LinkedHashMap<String, ParsedArgument>` in vanilla, so a repeated
    /// name **replaces the value and keeps the first key's position**. Two
    /// arguments can share a name across a redirect, and a plain `Vec` would
    /// then colour both where vanilla colours one.
    pub arguments: Vec<(String, StringRange)>,
    pub child: Option<Box<ContextBuilder>>,
}

impl ContextBuilder {
    fn new(root: i32, start: usize) -> Self {
        Self {
            root,
            range: StringRange::at(start),
            nodes: Vec::new(),
            arguments: Vec::new(),
            child: None,
        }
    }

    fn with_node(&mut self, node: i32, range: StringRange) {
        self.range = StringRange::between(self.range.start.min(range.start), range.end);
        self.nodes.push(ParsedNode { node, range });
    }

    /// `withArgument` — `LinkedHashMap.put`, so a repeat overwrites in place.
    fn with_argument(&mut self, name: &str, range: StringRange) {
        if let Some(slot) = self.arguments.iter_mut().find(|(n, _)| n == name) {
            slot.1 = range;
            return;
        }
        self.arguments.push((name.to_string(), range));
    }

    /// `getLastChild` — the deepest context, which is where `getCommand()`
    /// and the argument list a caller wants actually live.
    pub fn last_child(&self) -> &ContextBuilder {
        match &self.child {
            Some(c) => c.last_child(),
            None => self,
        }
    }

    /// `findSuggestionContext` — which node's children may complete at
    /// `cursor`, and where their replacement span begins.
    ///
    /// Vanilla throws `IllegalStateException` on the two impossible branches;
    /// a client that could be made to throw by a malformed tree is worse than
    /// one that answers the root, so those return `None` here and the caller
    /// falls back.
    pub fn find_suggestion_context(&self, cursor: usize) -> Option<SuggestionContext> {
        if self.range.start > cursor {
            return None;
        }
        if self.range.end < cursor {
            if let Some(child) = &self.child {
                return child.find_suggestion_context(cursor);
            }
            return Some(match self.nodes.last() {
                // `+ 1` — past the separator the next word begins after.
                Some(last) => SuggestionContext {
                    parent: last.node,
                    start_pos: last.range.end + 1,
                },
                None => SuggestionContext {
                    parent: self.root,
                    start_pos: self.range.start,
                },
            });
        }
        let mut prev = self.root;
        for node in &self.nodes {
            if node.range.start <= cursor && cursor <= node.range.end {
                return Some(SuggestionContext {
                    parent: prev,
                    start_pos: node.range.start,
                });
            }
            prev = node.node;
        }
        Some(SuggestionContext {
            parent: prev,
            start_pos: self.range.start,
        })
    }
}

/// `SuggestionContext`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SuggestionContext {
    /// The node whose CHILDREN are the candidates.
    pub parent: i32,
    pub start_pos: usize,
}

/// One entry of `ParseResults.getExceptions()`.
///
/// Vanilla's value is a whole `CommandSyntaxException`, which is
/// `{type, message, input, cursor}`. [`ReaderError`] is the type plus the
/// message's arguments; `cursor` is the reader's position **at the throw**,
/// which is what `getContext()` excerpts around and what
/// `command.context.parse_error` prints. The `input` is not stored because
/// every exception a client parse records was created against the same
/// command string — `parseNodes` hands each child a *copy* of the reader —
/// so [`ParseResults::reader`]'s string is it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParseError {
    /// The child node that failed. Vanilla keys its `LinkedHashMap` by the
    /// node itself; each child is visited once per `parseNodes`, so there is
    /// no de-duplication to reproduce.
    pub node: i32,
    pub error: ReaderError,
    /// `reader.getCursor()` when `createWithContext` ran.
    pub cursor: usize,
}

/// `ParseResults`.
#[derive(Clone, Debug)]
pub struct ParseResults {
    pub context: ContextBuilder,
    pub reader: StringReader,
    /// `getExceptions()` in insertion order — vanilla's `LinkedHashMap`.
    pub errors: Vec<ParseError>,
}

impl ParseResults {
    /// `ClientPacketListener.isValidCommand` — nothing left to read, no
    /// errors, and the deepest context carries a command.
    ///
    /// The third term is what stops `/gamemode` alone reading as valid: the
    /// node exists and is not executable.
    pub fn is_valid(&self, tree: &CommandTree) -> bool {
        if self.reader.can_read() || !self.errors.is_empty() {
            return false;
        }
        let last = self.context.last_child();
        match last.nodes.last() {
            Some(p) => tree.node(p.node).is_some_and(CommandNode::is_executable),
            None => false,
        }
    }
}

/// What [`completion_suggestions`] decided.
#[derive(Clone, Debug)]
pub struct Completion {
    /// Everything the client could answer by itself.
    pub local: Suggestions,
    /// True when at least one candidate child's suggestion provider is one
    /// Rewo routes to the server — see the module docs on why the server's
    /// reply then replaces this rather than merging with it.
    pub ask_server: bool,
}

/// `CommandDispatcher.parse`, entered at the root.
///
/// `command` is the input **without** the leading slash stripped: vanilla's
/// `updateCommandInfo` skips it on the reader before calling, so the cursor
/// starts at 1 and every range is an index into the whole field. Passing the
/// slashless string instead shifts every suggestion by one.
pub fn parse(tree: &CommandTree, command: &[u16], start: usize, ctx: CommandCtx<'_>) -> ParseResults {
    let mut reader = StringReader::new(command);
    reader.set_cursor(start);
    let context = ContextBuilder::new(tree.root, start);
    parse_nodes(tree, tree.root, &reader, context, ctx)
}

/// `canUse` — see the module docs. Constant `true`, deliberately.
fn can_use(_node: &CommandNode) -> bool {
    true
}

/// `CommandNode.getRelevantNodes`.
fn relevant_nodes(tree: &CommandTree, node: &CommandNode, reader: &StringReader) -> Vec<i32> {
    let mut literals = Vec::new();
    let mut arguments = Vec::new();
    for &c in &node.children {
        match tree.node(c).map(|n| &n.kind) {
            Some(NodeKind::Literal(_)) => literals.push(c),
            Some(NodeKind::Argument { .. }) => arguments.push(c),
            _ => {}
        }
    }
    if literals.is_empty() {
        return arguments;
    }
    // One word, then put the cursor back — vanilla mutates the caller's reader
    // and restores it, which is why this takes a shared reference and walks a
    // copy instead.
    let mut scan = reader.clone();
    while scan.can_read() && scan.peek() != b' ' as u16 {
        scan.skip();
    }
    let text = String::from_utf16_lossy(&reader.string()[reader.cursor()..scan.cursor()]);
    for &c in &literals {
        if tree.node(c).and_then(CommandNode::name) == Some(text.as_str()) {
            return vec![c];
        }
    }
    arguments
}

/// `LiteralCommandNode.parse`'s private half — the literal must match AND be
/// followed by a space or the end.
///
/// Without the second test `/gamemodex` would match the `gamemode` literal and
/// then fail somewhere less obvious.
fn parse_literal(reader: &mut StringReader, literal: &str) -> bool {
    let lit: Vec<u16> = literal.encode_utf16().collect();
    let start = reader.cursor();
    if reader.can_read_len(lit.len()) && reader.string()[start..start + lit.len()] == lit[..] {
        reader.set_cursor(start + lit.len());
        if !reader.can_read() || reader.peek() == b' ' as u16 {
            return true;
        }
        reader.set_cursor(start);
    }
    false
}

fn parse_nodes(
    tree: &CommandTree,
    node_index: i32,
    original: &StringReader,
    context_so_far: ContextBuilder,
    ctx: CommandCtx<'_>,
) -> ParseResults {
    let Some(node) = tree.node(node_index) else {
        return ParseResults {
            context: context_so_far,
            reader: original.clone(),
            errors: Vec::new(),
        };
    };
    let cursor = original.cursor();
    let mut errors: Vec<ParseError> = Vec::new();
    let mut potentials: Vec<ParseResults> = Vec::new();

    for child_index in relevant_nodes(tree, node, original) {
        let Some(child) = tree.node(child_index) else {
            continue;
        };
        if !can_use(child) {
            continue;
        }
        let mut context = context_so_far.clone();
        let mut reader = original.clone();
        let outcome = (|| -> Result<(), ReaderError> {
            let start = reader.cursor();
            match &child.kind {
                NodeKind::Literal(lit) => {
                    if !parse_literal(&mut reader, lit) {
                        return Err(ReaderError::LiteralIncorrect);
                    }
                    context.with_node(child_index, StringRange::between(start, reader.cursor()));
                }
                NodeKind::Argument {
                    type_id: _,
                    props,
                    name,
                    ..
                } => {
                    let kind = kind_of(tree, child, props);
                    kind.parse(&mut reader, ctx)?;
                    let range = StringRange::between(start, reader.cursor());
                    // `withArgument` BEFORE `withNode`, as
                    // `ArgumentCommandNode.parse` does — both take the same
                    // range, so the order is not observable here, and it is
                    // kept so the transcription reads against the source.
                    context.with_argument(name, range);
                    context.with_node(child_index, range);
                }
                NodeKind::Root => return Err(ReaderError::LiteralIncorrect),
            }
            // `dispatcherExpectedArgumentSeparator` — an argument that stopped
            // mid-word did not really match.
            if reader.can_read() && reader.peek() != b' ' as u16 {
                return Err(ReaderError::ExpectedArgumentSeparator);
            }
            Ok(())
        })();
        if let Err(e) = outcome {
            // `errors.put(child, ex); reader.setCursor(cursor);` — in that
            // order, so the exception keeps the cursor the FAILING read left
            // and the retry rewinds afterwards. Reading the cursor after the
            // rewind (or from `original`) would excerpt the start of the
            // argument for every error, and `getContext`'s whole job is to
            // point at where the parse actually stopped.
            errors.push(ParseError {
                node: child_index,
                error: e,
                cursor: reader.cursor(),
            });
            continue;
        }

        // `reader.canRead(child.getRedirect() == null ? 2 : 1)` — TWO, because
        // after the separator there must be at least one more character for a
        // child to match. A redirect needs only the separator.
        let need = if child.has_redirect() { 1 } else { 2 };
        if reader.can_read_len(need) {
            reader.skip();
            if child.has_redirect() {
                let child_context = ContextBuilder::new(child.redirect, reader.cursor());
                let parse = parse_nodes(tree, child.redirect, &reader, child_context, ctx);
                context.child = Some(Box::new(parse.context));
                potentials.push(ParseResults {
                    context,
                    reader: parse.reader,
                    errors: parse.errors,
                });
            } else {
                potentials.push(parse_nodes(tree, child_index, &reader, context, ctx));
            }
        } else {
            potentials.push(ParseResults {
                context,
                reader,
                errors: Vec::new(),
            });
        }
    }

    if potentials.is_empty() {
        return ParseResults {
            context: context_so_far,
            reader: {
                let mut r = original.clone();
                r.set_cursor(cursor);
                r
            },
            errors,
        };
    }
    if potentials.len() > 1 {
        // Vanilla's comparator, in its own order: a parse that consumed
        // everything beats one that did not, and only then does an error-free
        // parse beat one with errors. `sort` is stable, so equal candidates
        // keep child order.
        potentials.sort_by(|a, b| {
            use std::cmp::Ordering::*;
            let (ar, br) = (a.reader.can_read(), b.reader.can_read());
            if !ar && br {
                return Less;
            }
            if ar && !br {
                return Greater;
            }
            match (a.errors.is_empty(), b.errors.is_empty()) {
                (true, false) => Less,
                (false, true) => Greater,
                _ => Equal,
            }
        });
    }
    potentials.remove(0)
}

/// The argument kind for a node, resolved through the tree's own type table.
///
/// The wire carries a numeric `type_id`; [`crate::commands`] keeps it raw so a
/// caller can name it. The name is what dispatch keys on, so a tree whose ids
/// could not be named yields [`ArgKind::Unknown`] — which refuses, rather than
/// silently parsing one type as another.
fn kind_of(tree: &CommandTree, node: &CommandNode, props: &ArgumentProps) -> ArgKind {
    let _ = tree;
    match &node.kind {
        NodeKind::Argument { type_name, .. } => ArgKind::resolve(type_name, props),
        _ => ArgKind::Unknown,
    }
}

/// `CommandDispatcher.getCompletionSuggestions`.
///
/// `start = min(nodeBeforeCursor.startPos, cursor)`, and the builder is given
/// the input **truncated at the cursor** — so a suggestion replaces from the
/// word's start to the cursor and whatever follows is preserved.
pub fn completion_suggestions(
    tree: &CommandTree,
    parse_results: &ParseResults,
    cursor: usize,
    cmd: CommandCtx<'_>,
) -> Completion {
    let Some(ctx) = parse_results.context.find_suggestion_context(cursor) else {
        return Completion {
            local: Suggestions::empty(),
            ask_server: false,
        };
    };
    let Some(parent) = tree.node(ctx.parent) else {
        return Completion {
            local: Suggestions::empty(),
            ask_server: false,
        };
    };
    let start = ctx.start_pos.min(cursor);
    let full = parse_results.reader.string();
    let truncated = &full[..cursor.min(full.len())];
    let mut parts: Vec<Suggestions> = Vec::new();
    let mut ask_server = false;
    for &child_index in &parent.children {
        let Some(child) = tree.node(child_index) else {
            continue;
        };
        let mut builder = SuggestionsBuilder::new(truncated, start);
        match &child.kind {
            NodeKind::Literal(lit) => {
                // `literalLowerCase.startsWith(getRemainingLowerCase())` — the
                // LITERAL starts with the typed text, which is the opposite
                // direction from `matchesSubStr`'s "does the pattern occur in
                // the candidate".
                if lit.to_lowercase().starts_with(&builder.remaining().to_lowercase()) {
                    builder.suggest(lit);
                }
            }
            NodeKind::Argument {
                props, suggestions, ..
            } => {
                if suggestions.is_some() {
                    // Any registered provider, and an unknown id too:
                    // `getProvider` defaults to ASK_SERVER.
                    ask_server = true;
                } else {
                    let registry = match props {
                        ArgumentProps::Registry(r) => Some(r.as_str()),
                        _ => None,
                    };
                    kind_of(tree, child, props).suggest_with(&mut builder, cmd, registry);
                }
            }
            NodeKind::Root => {}
        }
        parts.push(builder.build());
    }
    Completion {
        local: Suggestions::merge(full, parts),
        ask_server,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn u(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    /// A tree of `/give <int>`, `/gamemode <bool>` and `/say <greedy>`, plus a
    /// `/tp <unknown>` whose argument type this client cannot parse.
    fn tree() -> CommandTree {
        use crate::commands::NodeKind::*;
        let arg = |name: &str, type_name: &str, props: ArgumentProps, sugg: Option<&str>| Argument {
            name: name.into(),
            type_id: 0,
            type_name: type_name.into(),
            props,
            suggestions: sugg.map(str::to_string),
        };
        let n = |flags: u8, children: Vec<i32>, kind| CommandNode {
            flags,
            children,
            redirect: 0,
            kind,
        };
        CommandTree {
            root: 0,
            nodes: vec![
                n(0, vec![1, 3, 5, 7], Root),
                n(1, vec![2], Literal("give".into())),
                n(
                    2 | 4,
                    vec![],
                    arg(
                        "count",
                        "brigadier:integer",
                        ArgumentProps::RangeI64 {
                            min: Some(1),
                            max: Some(64),
                        },
                        None,
                    ),
                ),
                n(1, vec![4], Literal("gamemode".into())),
                n(2 | 4, vec![], arg("flag", "brigadier:bool", ArgumentProps::None, None)),
                n(1, vec![6], Literal("say".into())),
                n(
                    2 | 4,
                    vec![],
                    arg(
                        "message",
                        "brigadier:string",
                        ArgumentProps::String(StringType::GreedyPhrase),
                        None,
                    ),
                ),
                n(1, vec![8], Literal("tp".into())),
                n(
                    2 | 4,
                    vec![],
                    // An IMPOSSIBLE type name, not merely an unimplemented
                    // one. `minecraft:entity` stood here until M118
                    // transcribed it, and both tests below silently stopped
                    // testing their claim — the rot M41 found in `swingshot`
                    // and M43 in two `item_stack` fixtures, for the third
                    // time. A name no registry can contain cannot rot again.
                    arg(
                        "target",
                        "minecraft:__no_such_argument_type",
                        ArgumentProps::None,
                        Some("minecraft:ask_server"),
                    ),
                ),
            ],
        }
    }

    fn complete(input: &str) -> Completion {
        let t = tree();
        let units = u(input);
        let p = parse(&t, &units, 1, CommandCtx::default());
        completion_suggestions(&t, &p, units.len(), CommandCtx::default())
    }

    fn texts(c: &Completion) -> Vec<&str> {
        c.local.list.iter().map(|s| s.text.as_str()).collect()
    }

    // ── the reader ───────────────────────────────────────────────────────

    #[test]
    fn a_number_may_not_contain_a_plus_and_an_unquoted_string_may() {
        // The one place `isAllowedNumber` and `isAllowedInUnquotedString`
        // disagree.
        assert!(!StringReader::is_allowed_number(b'+' as u16));
        assert!(StringReader::is_allowed_in_unquoted_string(b'+' as u16));
        // And neither admits `e`, so `1e5` is not a float here.
        let mut r = StringReader::from_str("1e5");
        assert_eq!(r.read_i32().unwrap(), 1);
        assert_eq!(r.cursor(), 1);
    }

    #[test]
    fn an_out_of_range_integer_errors_rather_than_saturating() {
        // `Integer.parseInt` throws; a saturating parse would accept
        // 99999999999 as i32::MAX and then pass a range check it should fail.
        let mut r = StringReader::from_str("99999999999");
        assert_eq!(
            r.read_i32(),
            Err(ReaderError::InvalidInt("99999999999".into()))
        );
        assert_eq!(r.cursor(), 0, "the cursor is put back");
    }

    #[test]
    fn a_quoted_string_escapes_only_its_terminator_and_the_backslash() {
        let mut r = StringReader::from_str(r#""a\"b""#);
        assert_eq!(r.read_string().unwrap(), r#"a"b"#);
        let mut r = StringReader::from_str(r#""a\nb""#);
        // The exception carries the offending CHARACTER, which is what its
        // message quotes — `n`, not the backslash and not the whole escape.
        assert_eq!(
            r.read_string(),
            Err(ReaderError::InvalidEscape("n".to_string()))
        );
    }

    #[test]
    fn an_empty_reader_reads_an_empty_string_rather_than_erroring() {
        // What lets an argument at the very end of the input parse to nothing
        // so the popup can still offer something.
        let mut r = StringReader::from_str("");
        assert_eq!(r.read_string().unwrap(), "");
        assert_eq!(r.read_bool(), Err(ReaderError::ExpectedBool));
    }

    #[test]
    fn a_boolean_is_case_sensitive() {
        assert!(StringReader::from_str("true").read_bool().unwrap());
        assert!(!StringReader::from_str("false").read_bool().unwrap());
        assert_eq!(
            StringReader::from_str("True").read_bool(),
            Err(ReaderError::InvalidBool("True".into()))
        );
    }

    #[test]
    fn integer_and_long_share_a_props_shape_and_differ_where_it_matters() {
        // `brigadier:integer` and `brigadier:long` both decode to `RangeI64`,
        // so the props alone cannot tell them apart — and the difference is
        // exactly a value outside `i32`, which `Integer.parseInt` rejects and
        // `Long.parseLong` accepts. Dispatching on the shape rather than the
        // NAME therefore turns every large `long` argument into a parse error,
        // and a fixture with no `long` node in it cannot see that.
        let unbounded = ArgumentProps::RangeI64 {
            min: None,
            max: None,
        };
        let int = ArgKind::resolve("brigadier:integer", &unbounded);
        let long = ArgKind::resolve("brigadier:long", &unbounded);
        assert_ne!(int, long);
        let big = "3000000000";
        assert!(int.parse(&mut StringReader::from_str(big), CommandCtx::default()).is_err());
        assert!(long.parse(&mut StringReader::from_str(big), CommandCtx::default()).is_ok());
        // Float and double are the same pair one shape over.
        let f = ArgumentProps::RangeF64 {
            min: None,
            max: None,
        };
        assert_ne!(
            ArgKind::resolve("brigadier:float", &f),
            ArgKind::resolve("brigadier:double", &f)
        );
        // And every Minecraft type is Unknown whatever its props look like.
        assert_eq!(
            ArgKind::resolve("minecraft:__no_such_argument_type", &unbounded),
            ArgKind::Unknown
        );
        // …and the one type that IS transcribed resolves to itself whatever
        // its props say, because `single`/`playersOnly` do not change how the
        // text parses.
        assert_eq!(ArgKind::resolve("minecraft:entity", &unbounded), ArgKind::Entity);
    }

    // ── literal completion, which is the point ───────────────────────────

    #[test]
    fn a_prefix_offers_every_literal_it_starts_and_asks_nobody() {
        let c = complete("/g");
        assert_eq!(texts(&c), ["gamemode", "give"]);
        assert!(!c.ask_server, "no packet for a literal");
    }

    #[test]
    fn an_empty_command_offers_every_top_level_literal() {
        let c = complete("/");
        assert_eq!(texts(&c), ["gamemode", "give", "say", "tp"]);
    }

    #[test]
    fn the_literal_match_is_case_insensitive_but_the_suggestion_keeps_its_case() {
        // `literalLowerCase.startsWith(remainingLowerCase)`.
        let c = complete("/GA");
        assert_eq!(texts(&c), ["gamemode"]);
    }

    #[test]
    fn a_complete_literal_offers_its_own_children_rather_than_itself() {
        // The cursor is past the literal, so `findSuggestionContext` walks to
        // it and its children are the candidates. `give`'s child is an integer,
        // which suggests nothing — the popup is empty and no packet goes out.
        let c = complete("/give ");
        assert!(c.local.is_empty());
        assert!(!c.ask_server);
    }

    #[test]
    fn a_boolean_argument_suggests_true_and_false() {
        assert_eq!(texts(&complete("/gamemode ")), ["false", "true"]);
        assert_eq!(texts(&complete("/gamemode t")), ["true"]);
    }

    #[test]
    fn completing_mid_line_measures_only_the_text_before_the_cursor() {
        // `getCompletionSuggestions` builds against `fullInput.substring(0,
        // cursor)`. Every other test here puts the cursor at the end, where
        // the truncation is a no-op — which is why the mutation removing it
        // survived until this existed. With the cursor inside the line the
        // remaining text would otherwise be "g extra", which completes
        // nothing.
        let t = tree();
        let units = u("/g extra");
        let p = parse(&t, &units, 1, CommandCtx::default());
        let c = completion_suggestions(&t, &p, 2, CommandCtx::default());
        assert_eq!(texts(&c), ["gamemode", "give"]);
        // …and the span they replace stops at the cursor, so `extra` survives
        // being completed over.
        assert_eq!(c.local.range, StringRange::between(1, 2));
    }

    // ── where it still asks ──────────────────────────────────────────────

    #[test]
    fn an_ask_server_argument_sets_the_flag_and_offers_nothing_locally() {
        let c = complete("/tp ");
        assert!(c.ask_server);
        assert!(c.local.is_empty());
    }

    #[test]
    fn a_literal_being_typed_does_not_ask_even_when_a_sibling_would() {
        // The candidates at the top level are all literals, so `tp`'s
        // ask-server child is not among them.
        assert!(!complete("/t").ask_server);
    }

    #[test]
    fn an_entity_argument_parses_and_completes_without_the_server() {
        // M118 — the first `minecraft:` type transcribed, and the reason the
        // parse can now reach a command's THIRD word.
        let mut t = tree();
        t.nodes[0].children.push(9);
        t.nodes.push(CommandNode {
            flags: 1,
            children: vec![10],
            redirect: 0,
            kind: NodeKind::Literal("kill".into()),
        });
        t.nodes.push(CommandNode {
            flags: 2 | 4,
            children: vec![],
            redirect: 0,
            kind: NodeKind::Argument {
                name: "targets".into(),
                type_id: 0,
                type_name: "minecraft:entity".into(),
                props: ArgumentProps::None,
                suggestions: None,
            },
        });
        // It parses, so the command is valid and nothing is left over.
        let p = parse(&t, &u("/kill @e"), 1, CommandCtx::default());
        assert!(p.errors.is_empty());
        assert!(p.is_valid(&t));
        // A selector that fails AT THE END OF THE INPUT is the case that
        // needs the explicit failure check: `@z` leaves the cursor on the `z`
        // and the argument-separator test catches it anyway, but a bare `@`
        // and an unclosed `@e[` leave it at the end, where nothing downstream
        // can notice. Without the check both would read as valid commands.
        for bad in ["/kill @", "/kill @e["] {
            let p = parse(&t, &u(bad), 1, CommandCtx::default());
            assert!(!p.is_valid(&t), "{bad} must not be a valid command");
        }
        // And it completes locally: the six selectors plus the names the
        // caller supplied — with NO ask_server, because the node carries no
        // suggestion provider of its own.
        let units = u("/kill @");
        let p = parse(&t, &units, 1, CommandCtx::default());
        let names = vec!["Steve".to_string()];
        let c = completion_suggestions(&t, &p, units.len(), CommandCtx { names: &names, ..Default::default() });
        assert!(!c.ask_server);
        let texts: Vec<&str> = c.local.list.iter().map(|s| s.text.as_str()).collect();
        assert!(texts.contains(&"@e"));
        assert_eq!(texts.iter().filter(|s| s.starts_with('@')).count(), 6);
    }

    #[test]
    fn the_block_and_item_types_resolve_and_complete_through_the_dispatcher() {
        // M119, and the M92/M93b shape: `block_item`'s own tests drive
        // `ParsedRef` directly, so the wiring that turns a registry NAME into
        // that parser was untested until this existed — both mutations
        // reverting it to `Unknown` survived.
        let none = ArgumentProps::None;
        assert_eq!(
            ArgKind::resolve("minecraft:block_state", &none),
            ArgKind::BlockState
        );
        assert_eq!(
            ArgKind::resolve("minecraft:item_stack", &none),
            ArgKind::ItemStack
        );
        assert_eq!(
            ArgKind::resolve("minecraft:block_predicate", &none),
            ArgKind::BlockState
        );

        // …and it completes end to end, through the production suggester.
        let blocks = rewo_data::blocks::Blocks::for_tests(
            vec!["minecraft:air".into()],
            vec![
                ("minecraft:air".to_string(), vec![]),
                ("minecraft:stone".to_string(), vec![]),
            ],
        );
        let mut t = tree();
        t.nodes[0].children.push(9);
        t.nodes.push(CommandNode {
            flags: 1,
            children: vec![10],
            redirect: 0,
            kind: NodeKind::Literal("setblock".into()),
        });
        t.nodes.push(CommandNode {
            flags: 2 | 4,
            children: vec![],
            redirect: 0,
            kind: NodeKind::Argument {
                name: "block".into(),
                type_id: 0,
                type_name: "minecraft:block_state".into(),
                props: ArgumentProps::None,
                suggestions: None,
            },
        });
        let cmd = CommandCtx {
            blocks: Some(&blocks),
            ..Default::default()
        };
        let units = u("/setblock sto");
        let p = parse(&t, &units, 1, cmd);
        let c = completion_suggestions(&t, &p, units.len(), cmd);
        assert!(!c.ask_server);
        let texts: Vec<&str> = c.local.list.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(texts, ["minecraft:stone"]);
        // With a registry the argument also PARSES, so the command is valid.
        let done = parse(&t, &u("/setblock stone"), 1, cmd);
        assert!(done.is_valid(&t));
        // …and with an empty context it does not, which is what keeps a
        // registry-less caller behaving as it did before M119.
        assert!(!parse(&t, &u("/setblock stone"), 1, CommandCtx::default()).is_valid(&t));
    }

    #[test]
    fn the_coordinate_family_parses_and_completes_through_the_dispatcher() {
        // M120, and the M92/M93b guard again: `arg_types`' own tests never
        // touch the wiring that turns a registry NAME into that resolver.
        assert_eq!(
            ArgKind::resolve("minecraft:block_pos", &ArgumentProps::None),
            ArgKind::Value(crate::arg_types::Value::Coords(
                crate::arg_types::Coords::BlockPos
            ))
        );
        let mut t = tree();
        t.nodes[0].children.push(9);
        t.nodes.push(CommandNode {
            flags: 1,
            children: vec![10],
            redirect: 0,
            // NOT `tp` — the fixture already has one, whose child is the
            // impossible type M118 put there, and `getRelevantNodes` returns
            // the first literal that matches exactly. A duplicate name in a
            // test tree is silently shadowed rather than rejected.
            kind: NodeKind::Literal("teleport".into()),
        });
        t.nodes.push(CommandNode {
            flags: 2 | 4,
            children: vec![],
            redirect: 0,
            kind: NodeKind::Argument {
                name: "location".into(),
                type_id: 0,
                type_name: "minecraft:vec3".into(),
                props: ArgumentProps::None,
                suggestions: None,
            },
        });
        let ctx = CommandCtx::default();
        // It parses, so the command is valid — three components and the two
        // separators between them.
        assert!(parse(&t, &u("/teleport ~ ~ ~"), 1, ctx).is_valid(&t));
        assert!(parse(&t, &u("/teleport 1 2 3"), 1, ctx).is_valid(&t));
        assert!(!parse(&t, &u("/teleport 1 2"), 1, ctx).is_valid(&t));
        // …and it completes with the defaults, progressively.
        let units = u("/teleport ");
        let p = parse(&t, &units, 1, ctx);
        let c = completion_suggestions(&t, &p, units.len(), ctx);
        assert!(!c.ask_server);
        let texts: Vec<&str> = c.local.list.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(texts, ["~", "~ ~", "~ ~ ~"]);
    }

    #[test]
    fn a_resource_argument_suggests_from_the_registry_the_WIRE_named() {
        // M113 keeps the registry id in the props and this is its first
        // consumer, so the path from that field to the suggester was untested
        // until now — the mutation blanking it survived everything else.
        let items = rewo_data::items::Items::for_tests(&["minecraft:stone", "minecraft:stick"]);
        let mut t = tree();
        t.nodes[0].children.push(9);
        t.nodes.push(CommandNode {
            flags: 1,
            children: vec![10],
            redirect: 0,
            kind: NodeKind::Literal("clear".into()),
        });
        t.nodes.push(CommandNode {
            flags: 2 | 4,
            children: vec![],
            redirect: 0,
            kind: NodeKind::Argument {
                name: "item".into(),
                type_id: 0,
                type_name: "minecraft:resource".into(),
                props: ArgumentProps::Registry("minecraft:item".into()),
                suggestions: None,
            },
        });
        let ctx = CommandCtx {
            items: Some(&items),
            ..Default::default()
        };
        let units = u("/clear sti");
        let p = parse(&t, &units, 1, ctx);
        let c = completion_suggestions(&t, &p, units.len(), ctx);
        let texts: Vec<&str> = c.local.list.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(texts, ["minecraft:stick"]);
        // Name the WRONG registry and the same argument offers nothing, which
        // is what shows the field is read rather than a fixed registry used.
        let mut t2 = t.clone();
        if let NodeKind::Argument { props, .. } = &mut t2.nodes[10].kind {
            *props = ArgumentProps::Registry("minecraft:block".into());
        }
        let p2 = parse(&t2, &units, 1, ctx);
        assert!(completion_suggestions(&t2, &p2, units.len(), ctx).local.is_empty());
    }

    #[test]
    fn a_type_with_its_own_module_is_not_shadowed_by_the_value_family() {
        // `block_predicate` is an id to `arg_types` and a block state to
        // `block_item`; the arm order is what keeps the richer one. Putting
        // the value family first silently downgrades both predicates.
        assert_eq!(
            ArgKind::resolve("minecraft:block_predicate", &ArgumentProps::None),
            ArgKind::BlockState
        );
        assert_eq!(
            ArgKind::resolve("minecraft:item_predicate", &ArgumentProps::None),
            ArgKind::ItemStack
        );
    }

    #[test]
    fn every_minecraft_argument_type_now_parses() {
        // M121's headline, and the guard against it quietly regressing: the
        // 57-entry registry has no `Unknown` left. `ArgKind::Unknown` still
        // exists — it is what an unrecognised NAME resolves to, which is a
        // version mismatch rather than a gap — but no type in this list
        // reaches it.
        let none = ArgumentProps::None;
        let all = [
            "minecraft:angle", "minecraft:block_pos", "minecraft:block_predicate",
            "minecraft:block_state", "minecraft:column_pos", "minecraft:component",
            "minecraft:dialog", "minecraft:dimension", "minecraft:entity",
            "minecraft:entity_anchor", "minecraft:float_range", "minecraft:function",
            "minecraft:game_profile", "minecraft:gamemode", "minecraft:heightmap",
            "minecraft:hex_color", "minecraft:int_range", "minecraft:item_predicate",
            "minecraft:item_slot", "minecraft:item_slots", "minecraft:item_stack",
            "minecraft:loot_modifier", "minecraft:loot_predicate", "minecraft:loot_table",
            "minecraft:message", "minecraft:nbt_compound_tag", "minecraft:nbt_path",
            "minecraft:nbt_tag", "minecraft:objective", "minecraft:objective_criteria",
            "minecraft:operation", "minecraft:particle", "minecraft:resource",
            "minecraft:resource_key", "minecraft:resource_location",
            "minecraft:resource_or_tag", "minecraft:resource_or_tag_key",
            "minecraft:resource_selector", "minecraft:rotation", "minecraft:score_holder",
            "minecraft:scoreboard_slot", "minecraft:style", "minecraft:swizzle",
            "minecraft:team", "minecraft:team_color", "minecraft:template_mirror",
            "minecraft:template_rotation", "minecraft:time", "minecraft:uuid",
            "minecraft:vec2", "minecraft:vec3",
        ];
        let unknown: Vec<&str> = all
            .into_iter()
            .filter(|t| ArgKind::resolve(t, &none) == ArgKind::Unknown)
            .collect();
        assert!(unknown.is_empty(), "still Unknown: {unknown:?}");
        // …and a name no registry can contain still is.
        assert_eq!(
            ArgKind::resolve("minecraft:__no_such_argument_type", &none),
            ArgKind::Unknown
        );
    }

    #[test]
    fn an_nbt_argument_lets_the_parse_reach_the_words_after_it() {
        // The whole point of M121: leaving these Unknown stopped the parse at
        // the NBT word, which cost the highlighting AND the completion of
        // every later word.
        let mut t = tree();
        t.nodes[0].children.push(9);
        t.nodes.push(CommandNode {
            flags: 1,
            children: vec![10],
            redirect: 0,
            kind: NodeKind::Literal("data".into()),
        });
        t.nodes.push(CommandNode {
            flags: 2,
            children: vec![11],
            redirect: 0,
            kind: NodeKind::Argument {
                name: "nbt".into(),
                type_id: 0,
                type_name: "minecraft:nbt_compound_tag".into(),
                props: ArgumentProps::None,
                suggestions: None,
            },
        });
        t.nodes.push(CommandNode {
            flags: 1 | 4,
            children: vec![],
            redirect: 0,
            kind: NodeKind::Literal("force".into()),
        });
        let ctx = CommandCtx::default();
        assert!(parse(&t, &u("/data {a:1} force"), 1, ctx).is_valid(&t));
        // …including when a brace hides inside a string, which is where a
        // naive counter truncates.
        assert!(parse(&t, &u("/data {a:\"}\"} force"), 1, ctx).is_valid(&t));
        // M122 — and a MALFORMED compound is now invalid, where M121's extent
        // walk measured it and let the rest of the command parse. This is the
        // milestone's whole claim at the dispatcher level.
        assert!(!parse(&t, &u("/data {a:} force"), 1, ctx).is_valid(&t));
        // The leading zero has to be in the VALUE position: `{01:1}` is
        // valid, because a map KEY is a string and never goes through the
        // numeral rule. This witness used the key first — the same
        // distinction `snbt_grammar`'s own tests record, two modules apart.
        assert!(!parse(&t, &u("/data {a:01} force"), 1, ctx).is_valid(&t));
        assert!(parse(&t, &u("/data {01:1} force"), 1, ctx).is_valid(&t));
        // And the word after it completes locally.
        let units = u("/data {a:1} fo");
        let p = parse(&t, &units, 1, ctx);
        let c = completion_suggestions(&t, &p, units.len(), ctx);
        let texts: Vec<&str> = c.local.list.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(texts, ["force"]);
    }

    // ── the parse ────────────────────────────────────────────────────────

    #[test]
    fn a_valid_command_parses_to_the_end_with_no_errors() {
        let t = tree();
        let p = parse(&t, &u("/give 5"), 1, CommandCtx::default());
        assert!(p.errors.is_empty());
        assert!(!p.reader.can_read());
        assert!(p.is_valid(&t));
    }

    #[test]
    fn a_literal_alone_is_not_a_valid_command_when_it_is_not_executable() {
        // The third term of `isValidCommand`: the node exists and carries no
        // command.
        let t = tree();
        assert!(!parse(&t, &u("/give"), 1, CommandCtx::default()).is_valid(&t));
    }

    #[test]
    fn an_argument_outside_its_range_is_an_error_and_leaves_the_reader_put() {
        let t = tree();
        let p = parse(&t, &u("/give 99"), 1, CommandCtx::default());
        assert!(!p.errors.is_empty());
        assert!(!p.is_valid(&t));
    }

    #[test]
    fn an_unparseable_argument_stops_the_parse_rather_than_failing_the_command() {
        // Every `minecraft:` type is Unknown, so the parse reaches the literal
        // and stops. That is the same shape as a malformed input, which is
        // what makes it safe: `parseNodes` already had to handle it.
        let t = tree();
        let p = parse(&t, &u("/tp Steve"), 1, CommandCtx::default());
        assert_eq!(p.errors.len(), 1);
        assert_eq!(p.errors[0].error, ReaderError::UnknownArgumentType);
        // …and the literal it did consume is still in the context, which is
        // what lets `findSuggestionContext` reach `tp`'s children.
        assert_eq!(p.context.nodes.len(), 1);
    }

    #[test]
    fn a_greedy_string_swallows_the_remainder() {
        let t = tree();
        let p = parse(&t, &u("/say hello there world"), 1, CommandCtx::default());
        assert!(p.errors.is_empty());
        assert!(!p.reader.can_read());
        assert_eq!(p.context.nodes.len(), 2);
    }

    #[test]
    fn an_exact_literal_hides_its_sibling_arguments_from_the_parse() {
        // `getRelevantNodes` returns the single matching literal, so nothing
        // tries to parse `give` as an argument. Adding an argument beside the
        // literals and checking it is NOT attempted is what pins it.
        let mut t = tree();
        // A greedy string as a fifth root child: it would match anything.
        t.nodes[0].children.push(9);
        t.nodes.push(CommandNode {
            flags: 2 | 4,
            children: vec![],
            redirect: 0,
            kind: NodeKind::Argument {
                name: "anything".into(),
                type_id: 0,
                type_name: "brigadier:string".into(),
                props: ArgumentProps::String(StringType::GreedyPhrase),
                suggestions: None,
            },
        });
        let p = parse(&t, &u("/give 5"), 1, CommandCtx::default());
        // Two nodes — the literal and its integer — not one greedy catch-all.
        assert_eq!(p.context.nodes.len(), 2);
    }

    #[test]
    fn a_word_that_matches_no_literal_falls_through_to_the_arguments() {
        let mut t = tree();
        t.nodes[0].children.push(9);
        t.nodes.push(CommandNode {
            flags: 2 | 4,
            children: vec![],
            redirect: 0,
            kind: NodeKind::Argument {
                name: "anything".into(),
                type_id: 0,
                type_name: "brigadier:string".into(),
                props: ArgumentProps::String(StringType::GreedyPhrase),
                suggestions: None,
            },
        });
        let p = parse(&t, &u("/notacommand"), 1, CommandCtx::default());
        assert!(p.errors.is_empty());
        assert_eq!(p.context.nodes.len(), 1);
    }

    /// A tree with a greedy catch-all beside the literals, for the three
    /// witnesses below. `getRelevantNodes`' fast path is what stops it winning.
    fn tree_with_catch_all() -> CommandTree {
        let mut t = tree();
        t.nodes[0].children.push(9);
        t.nodes.push(CommandNode {
            flags: 2 | 4,
            children: vec![],
            redirect: 0,
            kind: NodeKind::Argument {
                name: "anything".into(),
                type_id: 0,
                type_name: "brigadier:string".into(),
                props: ArgumentProps::String(StringType::GreedyPhrase),
                suggestions: None,
            },
        });
        t
    }

    #[test]
    fn a_matched_literal_wins_even_when_its_own_child_then_fails() {
        // The sharp form of `getRelevantNodes`. `/give 99` is out of range, so
        // the literal path finishes WITH an error — and a dispatcher that also
        // offered the sibling catch-all would find a parse with NO error,
        // which the comparator prefers, and would report an invalid command as
        // valid. Asserting only "two nodes were consumed" misses this,
        // because on a well-formed input the literal wins the tie anyway.
        let t = tree_with_catch_all();
        let p = parse(&t, &u("/give 99"), 1, CommandCtx::default());
        assert!(!p.errors.is_empty(), "the range error survives");
        assert!(!p.is_valid(&t), "and the command is not valid");
    }

    #[test]
    fn a_word_matching_no_literal_reaches_no_node_and_reports_no_error() {
        // Written first as "it fails as literalIncorrect", which is wrong, and
        // the correction is the more interesting fact: `getRelevantNodes`
        // returns a literal ONLY on an exact match, so a mis-typed word is
        // never handed to one. With no argument children beside the literals
        // there is nothing to try, `potentials` stays empty, and the parse
        // comes back with an empty context and — the surprising half — an
        // EMPTY error list.
        //
        // It also makes the second half of `parse_literal` ("and it must end
        // at a separator") unreachable from here: the pre-filter only ever
        // hands it text that already satisfies it. That is why the mutation
        // deleting it survives, and it survives in vanilla's shape too.
        let t = tree();
        let p = parse(&t, &u("/gamemodex"), 1, CommandCtx::default());
        assert!(p.errors.is_empty());
        assert!(p.context.nodes.is_empty());
        assert!(!p.is_valid(&t));
        // The candidates still come from the ROOT's children — the parse
        // consumed nothing, so there is nowhere else to look — and none of
        // them completes this word. The test asserted `["gamemode"]` first,
        // which is the direction error `LiteralCommandNode.listSuggestions`
        // exists to avoid: the LITERAL must start with the typed text, and
        // `gamemode` does not start with `gamemodex`.
        assert!(completion_suggestions(&t, &p, 10, CommandCtx::default()).local.is_empty());
        // A PREFIX of it does — but not the whole word, because
        // `SuggestionsBuilder.suggest` drops a suggestion equal to what is
        // typed (M114a). This assertion used the complete literal first and
        // measured an empty list for that reason, which is the two findings
        // meeting: the literal matches, and then the builder discards it.
        let shorter = parse(&t, &u("/gamemod"), 1, CommandCtx::default());
        assert_eq!(
            completion_suggestions(&t, &shorter, 8, CommandCtx::default())
                .local
                .list
                .iter()
                .map(|s| s.text.as_str())
                .collect::<Vec<_>>(),
            ["gamemode"]
        );
    }

    #[test]
    fn an_argument_that_stops_mid_word_is_an_error_not_leftover_input() {
        // `dispatcherExpectedArgumentSeparator`. Without it the integer reads
        // `5`, the node is accepted, and `5x` merely leaves the reader mid
        // string — invalid either way, but with no error to report and the
        // reader parked somewhere the usage line would point at wrongly.
        let t = tree();
        let p = parse(&t, &u("/give 5x"), 1, CommandCtx::default());
        assert_eq!(
            p.errors.iter().map(|e| e.error.clone()).collect::<Vec<_>>(),
            [ReaderError::ExpectedArgumentSeparator]
        );
        // The cursor the exception kept is where the argument stopped — after
        // `5`, i.e. on the `x` — not the start of the word and not the end of
        // the input. `getContext` excerpts up to it.
        assert_eq!(p.errors[0].cursor, 7);
        assert!(!p.is_valid(&t));
    }

    // ── findSuggestionContext ────────────────────────────────────────────

    #[test]
    fn the_cursor_inside_a_word_completes_that_word_from_its_start() {
        let t = tree();
        let units = u("/give 5");
        let p = parse(&t, &units, 1, CommandCtx::default());
        // Cursor at 3, inside "give".
        let ctx = p.context.find_suggestion_context(3).unwrap();
        assert_eq!(ctx.parent, t.root);
        assert_eq!(ctx.start_pos, 1);
    }

    #[test]
    fn the_cursor_past_the_last_word_starts_the_next_one_after_the_separator() {
        let t = tree();
        let units = u("/give ");
        let p = parse(&t, &units, 1, CommandCtx::default());
        let ctx = p.context.find_suggestion_context(6).unwrap();
        // The `give` node, and a start one past its end — the `+ 1` is the
        // space.
        assert_eq!(ctx.start_pos, 6);
    }
}
