//! `BlockStateParser` and `ItemParser` (M119).
//!
//! The two argument types after the selector that a player meets most: the
//! second word of `/setblock`, `/fill` and `/give`. Both are the same shape as
//! M118's selector — a **function pointer the parse reassigns**, applied by
//! `fillSuggestions` against a builder offset to the reader's cursor — so this
//! module is mostly about what each state offers.
//!
//! # `suggestEquals` is DEAD in the selector and LIVE here
//!
//! M118 recorded `EntitySelectorParser.suggestEquals` as defined and never
//! assigned. `BlockStateParser` has a method of the same name that **is**
//! assigned, immediately before the `=` test in `readProperties`:
//!
//! ```java
//! this.reader.skipWhitespace();
//! this.suggestions = this::suggestEquals;
//! if (!this.reader.canRead() || this.reader.peek() != '=') {
//! ```
//!
//! So `/setblock ~ ~ ~ oak_door[facing` offers `=` where
//! `@e[limit` offers nothing. Two classes, the same method name, and only one
//! of them wired — which is worth knowing before assuming either way.
//!
//! # Every bracket suggestion is gated on the remaining text being EMPTY
//!
//! ```java
//! if (builder.getRemaining().isEmpty()) {
//!    builder.suggest(String.valueOf('['));
//! }
//! ```
//!
//! — not filtered by prefix, *suppressed entirely* once anything is typed. So
//! `[` is offered after a complete block id and vanishes the moment the next
//! character arrives, rather than surviving as a non-matching entry. The same
//! guard governs `]`, `,` and `{`. Filtering by prefix instead leaves a
//! bracket in the list that no keystroke can ever select.
//!
//! `suggestNextPropertyOrEnd` carries a second condition on the comma —
//! `properties.size() < state.getProperties().size()` — so a block whose every
//! property is already set offers `]` and **not** `,`.
//!
//! # The namespace rule lives in `suggestResource`, not in the matcher
//!
//! Both id suggesters are `SharedSuggestionProvider.suggestResource`, whose
//! `filterResources` tests the typed text against the **namespace and the path
//! separately** when no colon has been typed. That is what lets `stone` find
//! `minecraft:stone` while [`rewo_world::suggestions::matches_sub_str`]
//! deliberately does not split on `:` — see M114a, which recorded the refusal
//! without yet knowing where the other half lived.
//!
//! # What is not here
//!
//! **The item's components** and **the block's NBT**. `readComponents` is the
//! text form of the `DataComponentPatch` M41 decodes on the wire — a parser of
//! its own — and `readNbt` is SNBT. Both leave the state at `SUGGEST_NOTHING`,
//! which is what vanilla shows *before* those parsers set their own, so an
//! item's `[` offers nothing here where vanilla would list component names.
//!
//! `suggestOpenNbt` is also gated on `hasBlockEntity()`, which Rewo can answer
//! (M25's registry) but through a crate this one does not take — so `{` is
//! never offered rather than being offered wrongly.

use crate::dispatcher::{ReaderError, StringReader};
use rewo_world::suggestions::{suggest_resource, SuggestionsBuilder};

/// What `fillSuggestions` should offer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Suggest {
    /// `suggestItem` — every id in the registry, through `suggestResource`.
    Ids,
    /// `suggestOpenPropertiesOrNbt` — `[` when the block has any properties.
    OpenProperties,
    /// `suggestStartComponents` — `[`, for an item.
    OpenComponents,
    /// `suggestPropertyNameOrEnd` — `]` and the unset property names.
    PropertyNameOrEnd,
    /// `suggestPropertyName` — the unset property names.
    PropertyName,
    /// `suggestEquals` — `=`. Live here and dead in the selector.
    Equals,
    /// A property's legal values.
    PropertyValue(String),
    /// `suggestNextPropertyOrEnd` — `]`, and `,` only while something is left
    /// to set.
    NextPropertyOrEnd,
    Nothing,
}

/// The parse's outcome, reduced to what the suggestion path reads.
pub struct ParsedRef {
    /// The id as typed, once it resolved.
    pub id: Option<String>,
    /// Property names already given a value.
    pub set: Vec<String>,
    pub suggestions: Suggest,
    pub cursor: usize,
    pub failed: bool,
}

/// What the suggesters need to answer from: the registries, and for a block
/// its property table.
#[derive(Clone, Copy)]
pub struct Registry<'a> {
    pub blocks: Option<&'a rewo_data::blocks::Blocks>,
    pub items: Option<&'a rewo_data::items::Items>,
}

/// `Identifier.read` — `[a-z0-9_.-]` for the namespace and additionally `/`
/// for the path.
///
/// **`:` is read as part of the identifier**, which is what makes a namespaced
/// id one token; the block/item parsers then never see it. An id reader that
/// stopped at the colon would parse `minecraft` and leave `:stone` behind.
fn read_identifier(reader: &mut StringReader) -> String {
    let start = reader.cursor();
    while reader.can_read() && is_identifier_char(reader.peek()) {
        reader.skip();
    }
    String::from_utf16_lossy(&reader.string()[start..reader.cursor()])
}

fn is_identifier_char(c: u16) -> bool {
    (0x30..=0x39).contains(&c)
        || (0x61..=0x7A).contains(&c)
        || matches!(c, 0x5F | 0x3A | 0x2F | 0x2E | 0x2D)
}

/// Normalise a bare id to its namespaced form, as `Identifier.read` does with
/// `withDefaultNamespace`.
fn with_default_namespace(id: &str) -> String {
    if id.contains(':') {
        id.to_string()
    } else {
        format!("minecraft:{id}")
    }
}

impl ParsedRef {
    /// `BlockStateParser.parse` — id, then optional `[properties]`, then
    /// optional `{nbt}`.
    pub fn parse_block(reader: &mut StringReader, reg: Registry<'_>) -> Self {
        let mut me = Self {
            id: None,
            set: Vec::new(),
            suggestions: Suggest::Ids,
            cursor: reader.cursor(),
            failed: false,
        };
        me.failed = me.run_block(reader, reg).is_err();
        me.cursor = reader.cursor();
        me
    }

    fn run_block(&mut self, reader: &mut StringReader, reg: Registry<'_>) -> Result<(), ReaderError> {
        // `#` is a block TAG, whose properties are "vague" — matched loosely
        // against whatever the tag's members share. Rewo has the tag list but
        // not the per-tag property intersection, so a tag parses its id and
        // stops.
        if reader.can_read() && reader.peek() == b'#' as u16 {
            reader.skip();
            read_identifier(reader);
            self.suggestions = Suggest::Nothing;
            return Ok(());
        }
        let start = reader.cursor();
        let raw = read_identifier(reader);
        let id = with_default_namespace(&raw);
        let known = reg
            .blocks
            .is_some_and(|b| b.properties(&id).is_some());
        if !known {
            // `orElseThrow` rewinds before throwing, so the suggester still
            // sees the whole typed id as its prefix.
            reader.set_cursor(start);
            return Err(ReaderError::UnknownArgumentType);
        }
        self.id = Some(id);
        self.suggestions = Suggest::OpenProperties;
        if reader.can_read() && reader.peek() == b'[' as u16 {
            self.read_properties(reader, reg)?;
            // `suggestOpenNbt`, which Rewo never offers — see the module docs.
            self.suggestions = Suggest::Nothing;
        }
        if reader.can_read() && reader.peek() == b'{' as u16 {
            self.suggestions = Suggest::Nothing;
            return Err(ReaderError::UnknownArgumentType);
        }
        Ok(())
    }

    fn read_properties(
        &mut self,
        reader: &mut StringReader,
        reg: Registry<'_>,
    ) -> Result<(), ReaderError> {
        reader.skip();
        self.suggestions = Suggest::PropertyNameOrEnd;
        skip_whitespace(reader);
        while reader.can_read() && reader.peek() != b']' as u16 {
            skip_whitespace(reader);
            let key_start = reader.cursor();
            let key = reader.read_string()?;
            let legal = self.property_values(reg, &key);
            if legal.is_none() || self.set.iter().any(|k| *k == key) {
                // Unknown property AND duplicate both rewind — the second is
                // its own error in vanilla and matters here only because a
                // repeated key would otherwise be silently accepted.
                reader.set_cursor(key_start);
                return Err(ReaderError::UnknownArgumentType);
            }
            skip_whitespace(reader);
            self.suggestions = Suggest::Equals;
            if !reader.can_read() || reader.peek() != b'=' as u16 {
                return Err(ReaderError::UnknownArgumentType);
            }
            reader.skip();
            skip_whitespace(reader);
            // The value suggester goes in BEFORE the value is read, the same
            // rule M118 found in `sort`.
            self.suggestions = Suggest::PropertyValue(key.clone());
            let value_start = reader.cursor();
            let value = reader.read_string()?;
            if !legal.unwrap().iter().any(|v| *v == value) {
                reader.set_cursor(value_start);
                return Err(ReaderError::UnknownArgumentType);
            }
            self.set.push(key);
            self.suggestions = Suggest::NextPropertyOrEnd;
            skip_whitespace(reader);
            if reader.can_read() {
                if reader.peek() != b',' as u16 {
                    if reader.peek() != b']' as u16 {
                        return Err(ReaderError::UnknownArgumentType);
                    }
                    break;
                }
                reader.skip();
                self.suggestions = Suggest::PropertyName;
            }
        }
        if reader.can_read() {
            reader.skip();
            Ok(())
        } else {
            Err(ReaderError::UnknownArgumentType)
        }
    }

    /// `ItemParser.State.parse` — id, then optional `[components]`.
    pub fn parse_item(reader: &mut StringReader, reg: Registry<'_>) -> Self {
        let mut me = Self {
            id: None,
            set: Vec::new(),
            suggestions: Suggest::Ids,
            cursor: reader.cursor(),
            failed: false,
        };
        me.failed = me.run_item(reader, reg).is_err();
        me.cursor = reader.cursor();
        me
    }

    fn run_item(&mut self, reader: &mut StringReader, reg: Registry<'_>) -> Result<(), ReaderError> {
        let start = reader.cursor();
        let raw = read_identifier(reader);
        let id = with_default_namespace(&raw);
        if !reg.items.is_some_and(|i| i.id(&id).is_some()) {
            reader.set_cursor(start);
            return Err(ReaderError::UnknownArgumentType);
        }
        self.id = Some(id);
        self.suggestions = Suggest::OpenComponents;
        if reader.can_read() && reader.peek() == b'[' as u16 {
            // `visitSuggestions(SUGGEST_NOTHING)` happens BEFORE
            // `readComponents`, which then installs its own. Rewo has no
            // component parser, so the state stays here.
            self.suggestions = Suggest::Nothing;
            return Err(ReaderError::UnknownArgumentType);
        }
        Ok(())
    }

    fn property_values<'a>(&self, reg: Registry<'a>, key: &str) -> Option<&'a [String]> {
        let id = self.id.as_deref()?;
        reg.blocks?
            .properties(id)?
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_slice())
    }

    /// Apply the state the parse left.
    pub fn fill_suggestions(&self, builder: &mut SuggestionsBuilder, reg: Registry<'_>, block: bool) {
        // Every bracket suggestion is SUPPRESSED once anything is typed rather
        // than filtered — see the module docs.
        let empty = builder.remaining().is_empty();
        match &self.suggestions {
            Suggest::Ids => {
                if block {
                    if let Some(b) = reg.blocks {
                        suggest_resource(b.names().iter().map(String::as_str), builder);
                    }
                } else if let Some(i) = reg.items {
                    suggest_resource(i.names(), builder);
                }
            }
            Suggest::OpenProperties => {
                if empty
                    && self
                        .id
                        .as_deref()
                        .and_then(|id| reg.blocks.and_then(|b| b.properties(id)))
                        .is_some_and(|p| !p.is_empty())
                {
                    builder.suggest("[");
                }
            }
            Suggest::OpenComponents => {
                if empty {
                    builder.suggest("[");
                }
            }
            Suggest::PropertyNameOrEnd => {
                if empty {
                    builder.suggest("]");
                }
                self.suggest_property_names(builder, reg);
            }
            Suggest::PropertyName => self.suggest_property_names(builder, reg),
            Suggest::Equals => {
                if empty {
                    builder.suggest("=");
                }
            }
            Suggest::PropertyValue(key) => {
                if let Some(values) = self.property_values(reg, key) {
                    for v in values {
                        if v.to_lowercase().starts_with(&builder.remaining().to_lowercase()) {
                            builder.suggest(v);
                        }
                    }
                }
            }
            Suggest::NextPropertyOrEnd => {
                if empty {
                    builder.suggest("]");
                    // `properties.size() < state.getProperties().size()` — a
                    // block with everything set offers `]` and NOT `,`.
                    let total = self
                        .id
                        .as_deref()
                        .and_then(|id| reg.blocks.and_then(|b| b.properties(id)))
                        .map(|p| p.len())
                        .unwrap_or(0);
                    if self.set.len() < total {
                        builder.suggest(",");
                    }
                }
            }
            Suggest::Nothing => {}
        }
    }

    fn suggest_property_names(&self, builder: &mut SuggestionsBuilder, reg: Registry<'_>) {
        let Some(props) = self
            .id
            .as_deref()
            .and_then(|id| reg.blocks.and_then(|b| b.properties(id)))
        else {
            return;
        };
        let prefix = builder.remaining().to_lowercase();
        for (name, _) in props {
            // An already-set property is not offered again — `getProperties()`
            // minus `this.properties`.
            if !self.set.iter().any(|k| k == name) && name.to_lowercase().starts_with(&prefix) {
                builder.suggest(name);
            }
        }
    }
}

fn skip_whitespace(reader: &mut StringReader) {
    while reader.can_read() && matches!(reader.peek(), 0x20 | 0x09 | 0x0A | 0x0B | 0x0C | 0x0D) {
        reader.skip();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A two-block registry with the properties `oak_door` really has.
    fn blocks() -> rewo_data::blocks::Blocks {
        rewo_data::blocks::Blocks::for_tests(
            vec!["minecraft:air".into(), "minecraft:stone".into()],
            vec![
                ("minecraft:air".to_string(), vec![]),
                ("minecraft:stone".to_string(), vec![]),
                (
                    "minecraft:oak_door".to_string(),
                    vec![
                        (
                            "facing".to_string(),
                            vec!["north".into(), "south".into(), "west".into(), "east".into()],
                        ),
                        ("half".to_string(), vec!["upper".into(), "lower".into()]),
                        ("open".to_string(), vec!["true".into(), "false".into()]),
                    ],
                ),
            ],
        )
    }

    fn reg(b: &rewo_data::blocks::Blocks) -> Registry<'_> {
        Registry {
            blocks: Some(b),
            items: None,
        }
    }

    fn offer_block(input: &str) -> Vec<String> {
        let b = blocks();
        let units: Vec<u16> = input.encode_utf16().collect();
        let mut reader = StringReader::new(&units);
        let p = ParsedRef::parse_block(&mut reader, reg(&b));
        let mut builder = SuggestionsBuilder::new(&units, p.cursor);
        p.fill_suggestions(&mut builder, reg(&b), true);
        builder.build().list.into_iter().map(|s| s.text).collect()
    }

    #[test]
    fn a_bare_name_finds_the_namespaced_id() {
        // `filterResources`' no-colon branch tests the pattern against the
        // namespace and the path SEPARATELY, which is the only reason `sto`
        // reaches `minecraft:stone` — `matchesSubStr` does not split on `:`.
        let o = offer_block("sto");
        assert_eq!(o, ["minecraft:stone"]);
    }

    #[test]
    fn a_typed_namespace_matches_the_whole_id() {
        assert_eq!(offer_block("minecraft:o"), ["minecraft:oak_door"]);
        // …and a wrong namespace finds nothing, where the path-only branch
        // would still have matched.
        assert!(offer_block("other:stone").is_empty());
    }

    #[test]
    fn the_colon_is_part_of_the_identifier() {
        // `Identifier.read` accepts `:`, so a namespaced id is ONE token. A
        // reader that stopped there would parse `minecraft` and leave
        // `:oak_door` behind — and that is invisible through the suggester,
        // because the failure rewinds and the whole typed text then matches as
        // a prefix. The parse RESULT is what separates them.
        let b = blocks();
        let units: Vec<u16> = "minecraft:oak_door".encode_utf16().collect();
        let mut reader = StringReader::new(&units);
        let p = ParsedRef::parse_block(&mut reader, reg(&b));
        assert!(!p.failed);
        assert_eq!(p.id.as_deref(), Some("minecraft:oak_door"));
        assert_eq!(p.cursor, units.len());
    }

    #[test]
    fn a_complete_id_offers_the_bracket_only_when_it_has_properties() {
        assert_eq!(offer_block("oak_door"), ["["]);
        assert!(offer_block("stone").is_empty(), "no properties, no bracket");
    }

    #[test]
    fn a_bracket_offers_the_close_and_every_property_name() {
        let mut o = offer_block("oak_door[");
        o.sort();
        assert_eq!(o, ["]", "facing", "half", "open"]);
    }

    #[test]
    fn a_property_name_offers_the_equals() {
        // `suggestEquals` — DEAD in `EntitySelectorParser` and live here.
        assert_eq!(offer_block("oak_door[facing"), ["="]);
    }

    #[test]
    fn a_property_offers_its_legal_values_and_nothing_else() {
        let mut o = offer_block("oak_door[facing=");
        o.sort();
        assert_eq!(o, ["east", "north", "south", "west"]);
        assert_eq!(offer_block("oak_door[facing=n"), ["north"]);
    }

    #[test]
    fn a_set_property_is_not_offered_again() {
        let mut o = offer_block("oak_door[facing=north,");
        o.sort();
        assert_eq!(o, ["half", "open"]);
    }

    #[test]
    fn a_value_offers_the_comma_and_the_close_until_everything_is_set() {
        let mut o = offer_block("oak_door[facing=north");
        o.sort();
        assert_eq!(o, [",", "]"]);
        // With every property set the comma goes — `properties.size() <
        // state.getProperties().size()`.
        let o = offer_block("oak_door[facing=north,half=upper,open=true");
        assert_eq!(o, ["]"]);
    }

    #[test]
    fn a_bracket_suggestion_is_suppressed_by_any_typed_text_rather_than_filtered() {
        // `if (builder.getRemaining().isEmpty())`. Filtering by prefix instead
        // leaves a `]` in the list that no keystroke can select.
        //
        // Reaching that needs the builder's remaining text to be NON-empty,
        // which only happens after a rewind — the builder always starts at the
        // reader's cursor. Here the junk after the value stops the loop with
        // the cursor ON it, so `NextPropertyOrEnd` sees "x" and offers
        // nothing. The first version of this witness used
        // `oak_door[facing=north,x`, which ends in the property-NAME state and
        // so could not see the gate at all.
        assert!(offer_block("oak_door[facing=north x").is_empty());
        // …and with nothing typed the same state does offer them.
        let mut o = offer_block("oak_door[facing=north");
        o.sort();
        assert_eq!(o, [",", "]"]);
    }

    #[test]
    fn the_value_suggester_survives_a_value_that_fails_to_READ() {
        // Installing it after `readString` looks equivalent — an illegal value
        // still reads, and the rewind puts the cursor back either way. The
        // case that separates them is a value that cannot be read at all: an
        // unclosed quote propagates before the assignment, leaving the state
        // at `Equals` and offering `=` where vanilla offers the four values.
        let mut o = offer_block("oak_door[facing=\"");
        o.sort();
        assert_eq!(o, ["east", "north", "south", "west"]);
    }

    #[test]
    fn a_closed_state_offers_nothing() {
        assert!(offer_block("oak_door[facing=north]").is_empty());
    }

    #[test]
    fn an_unknown_block_rewinds_so_the_whole_id_is_still_the_prefix() {
        // `orElseThrow` puts the cursor back before throwing, which is what
        // keeps the id suggester's prefix intact.
        let b = blocks();
        let units: Vec<u16> = "zzz".encode_utf16().collect();
        let mut reader = StringReader::new(&units);
        let p = ParsedRef::parse_block(&mut reader, reg(&b));
        assert!(p.failed);
        assert_eq!(p.cursor, 0);
    }

    #[test]
    fn a_block_tag_parses_its_id_and_stops() {
        let b = blocks();
        let units: Vec<u16> = "#minecraft:doors".encode_utf16().collect();
        let mut reader = StringReader::new(&units);
        let p = ParsedRef::parse_block(&mut reader, reg(&b));
        assert!(!p.failed);
        assert_eq!(p.suggestions, Suggest::Nothing);
    }
}
