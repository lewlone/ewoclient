//! The coordinate family and the value-shaped argument types (M120).
//!
//! M119 left 45 `minecraft:` types `Unknown`. This takes the ones whose
//! parsers are self-contained; what is left, and why, is at the bottom.
//!
//! # A coordinate is not a number with a `~` in front
//!
//! `WorldCoordinate.parseDouble` is four rules and only the first is obvious:
//!
//! ```java
//! if (reader.canRead() && reader.peek() == '^') throw ERROR_MIXED_TYPE;
//! boolean relative = isRelative(reader);              // consumes a leading '~'
//! double value = reader.canRead() && reader.peek() != ' ' ? reader.readDouble() : 0.0;
//! String number = reader.getString().substring(start, reader.getCursor());
//! if (relative && number.isEmpty()) return new WorldCoordinate(true, 0.0);
//! if (!number.contains(".") && !relative && center) value += 0.5;
//! ```
//!
//! * **A bare `~` is legal** and means "no offset" — the number is optional
//!   after the tilde, which is why `/tp ~ ~ ~` parses at all.
//! * **`^` is rejected here rather than handled**, because local coordinates
//!   are all-or-nothing: `Vec3Argument` reads `^^^` through a *different*
//!   class, and mixing the two is its own error.
//! * **The centring `+ 0.5` is suppressed by a decimal point**, not by the
//!   value: `5` becomes `5.5` and `5.0` stays `5.0`. So the two spellings of
//!   the same number mean different places, which is the rule that makes
//!   `/tp 5 64 5` land in the middle of a block.
//! * `parseInt` reads a **double** when the coordinate is relative and an
//!   **int** when it is not, so `~0.5` is legal in a block position and `0.5`
//!   is not.
//!
//! # `^` is all-or-nothing across the whole triple
//!
//! `Vec3Argument.parse` peeks for `^` once and then commits: either three
//! local coordinates or three world ones. A parser that decided per component
//! would accept `^1 ~2 3`, which vanilla rejects with `ERROR_MIXED_TYPE`.
//!
//! # The coordinate suggesters read the CROSSHAIR, and this one does not
//!
//! `BlockPosArgument.listSuggestions` offers `getRelevantCoordinates()` — the
//! block the player is looking at — and `Vec3Argument` offers
//! `getAbsoluteCoordinates()`, the exact hit position, each filtered through a
//! validator that re-parses every candidate. Rewo has the raycast (M73) and
//! not the validator, so it offers the **defaults** those collections fall
//! back to: `~ ~ ~` for a world coordinate and `^ ^ ^` for a local one, built
//! by the same progressive `x`, `x y`, `x y z` shape `suggestCoordinates`
//! uses. Stated rather than silently narrowed.
//!
//! # What is NOT here, and why
//!
//! **Six structured types** — `component`, `style`, `nbt_compound_tag`,
//! `nbt_tag`, `nbt_path`, `dialog` — each need a parser of their own (JSON
//! text, SNBT, a path expression). They stay `Unknown`.
//!
//! **Seven whose literals are a data extraction**, not a transcription:
//! `entity_anchor` and `operation` are here because their tables are two and
//! nine literals sitting in the argument class itself, but `heightmap`
//! (filtered by `keepAfterWorldgen`), `team_color` (`TeamColor.VALUES`),
//! `item_slot` / `item_slots` (`SlotRanges`, a registry of names),
//! `scoreboard_slot` (`DisplaySlot`, whose sixteen `sidebar.team.*` entries
//! are `ChatFormatting`'s colour names) and `swizzle` each read a table from
//! somewhere else. They parse as words here
//! and suggest nothing — which is wrong only in the suggestions, never in the
//! parse.
//!
//! **The registry-backed suggesters** are limited by what Rewo holds. The
//! `resource*` family carries its registry's name in the wire props (M113's
//! [`crate::commands::ArgumentProps::Registry`]), so the *right* registry is
//! always known; only `minecraft:block` and `minecraft:item` can be answered
//! from, and the rest parse and suggest nothing.

use crate::dispatcher::{ReaderError, StringReader};
use rewo_world::suggestions::{suggest_matching, suggest_resource, SuggestionsBuilder};

/// `EntityAnchorArgument.Anchor` — two literals, in the class itself.
pub const ANCHORS: [&str; 2] = ["feet", "eyes"];
/// `OperationArgument.listSuggestions`' array, verbatim and in its order.
pub const OPERATIONS: [&str; 9] = ["=", "+=", "-=", "*=", "/=", "%=", "<", ">", "><"];
/// `GameType`'s serialized names.
pub const GAMEMODES: [&str; 4] = ["survival", "creative", "adventure", "spectator"];
/// `Mirror`'s.
pub const MIRRORS: [&str; 3] = ["none", "left_right", "front_back"];
/// `Rotation`'s — note **`180`**, not `clockwise_180`.
pub const ROTATIONS: [&str; 4] = ["none", "clockwise_90", "180", "counterclockwise_90"];

/// How many coordinates a type takes, and in what unit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Coords {
    /// `block_pos` — three, integral unless relative.
    BlockPos,
    /// `column_pos` — two, integral unless relative.
    ColumnPos,
    /// `vec3` — three doubles, centred.
    Vec3,
    /// `vec2` — two doubles, **not** centred.
    Vec2,
    /// `rotation` — two doubles, not centred; yaw then pitch.
    Rotation,
    /// `angle` — one, `~`-relative but never local.
    Angle,
}

impl Coords {
    fn count(self) -> usize {
        match self {
            Self::BlockPos | Self::Vec3 => 3,
            Self::ColumnPos | Self::Vec2 | Self::Rotation => 2,
            Self::Angle => 1,
        }
    }

    fn integral(self) -> bool {
        matches!(self, Self::BlockPos | Self::ColumnPos)
    }

    /// Whether `^` is accepted at all. `Vec2`, `Rotation` and `Angle` have no
    /// local form.
    fn allows_local(self) -> bool {
        matches!(self, Self::BlockPos | Self::ColumnPos | Self::Vec3)
    }
}

/// `WorldCoordinate.isRelative` — consumes a leading `~` and reports it.
fn is_relative(reader: &mut StringReader) -> bool {
    if reader.can_read() && reader.peek() == b'~' as u16 {
        reader.skip();
        true
    } else {
        false
    }
}

/// `WorldCoordinate.parseDouble` / `parseInt`.
fn read_world_coordinate(reader: &mut StringReader, integral: bool) -> Result<(), ReaderError> {
    if reader.can_read() && reader.peek() == b'^' as u16 {
        // `ERROR_MIXED_TYPE` — a local coordinate reached the world reader.
        //
        // **This check changes the ERROR, not the outcome.** `isAllowedNumber`
        // excludes `^`, so `readDouble` would read an empty token and raise
        // `ExpectedDouble` anyway; deleting the guard is a mutation that
        // survives every witness, and provably so. It is transcribed because
        // vanilla distinguishes the two messages and a future usage-line
        // milestone would want the right one.
        return Err(ReaderError::UnknownArgumentType);
    }
    if !reader.can_read() {
        return Err(ReaderError::ExpectedDouble);
    }
    let relative = is_relative(reader);
    // **The number is optional after a `~`**, and the guard is `peek() != ' '`
    // rather than "is there a digit" — so `~` alone is a complete coordinate
    // and `/tp ~ ~ ~` parses.
    if reader.can_read() && reader.peek() != b' ' as u16 {
        if integral && !relative {
            reader.read_i32()?;
        } else {
            reader.read_f64()?;
        }
    }
    Ok(())
}

/// `LocalCoordinates.parse` — three `^`-prefixed doubles, and nothing else.
fn read_local_coordinate(reader: &mut StringReader) -> Result<(), ReaderError> {
    if !reader.can_read() || reader.peek() != b'^' as u16 {
        return Err(ReaderError::UnknownArgumentType);
    }
    reader.skip();
    if reader.can_read() && reader.peek() != b' ' as u16 {
        reader.read_f64()?;
    }
    Ok(())
}

/// Parse a whole coordinate group.
///
/// **`^` is decided once for the group**, not per component: vanilla peeks at
/// the first character and commits, so `^1 ~2 3` is `ERROR_MIXED_TYPE` rather
/// than a mixed triple.
pub fn read_coords(reader: &mut StringReader, kind: Coords) -> Result<(), ReaderError> {
    let local = kind.allows_local() && reader.can_read() && reader.peek() == b'^' as u16;
    for i in 0..kind.count() {
        if i > 0 {
            if !reader.can_read() || reader.peek() != b' ' as u16 {
                return Err(ReaderError::UnknownArgumentType);
            }
            reader.skip();
        }
        if local {
            read_local_coordinate(reader)?;
        } else {
            read_world_coordinate(reader, kind.integral())?;
        }
    }
    Ok(())
}

/// The default coordinate suggestions — `~ ~ ~` or `^ ^ ^`, built the way
/// `suggestCoordinates` builds them.
///
/// `TextCoordinates.DEFAULT_GLOBAL` is `("~", "~", "~")` and `DEFAULT_LOCAL`
/// is `("^", "^", "^")`, and the suggester offers the **progressive** forms:
/// the first component, then the first two, then all three. Offering only the
/// complete one means Tab fills the whole triple when the player wanted one
/// axis.
pub fn suggest_coords(builder: &mut SuggestionsBuilder, kind: Coords) {
    let remaining = builder.remaining().to_string();
    // `remainder.charAt(0) == '^'` selects the local set.
    let unit = if remaining.starts_with('^') && kind.allows_local() {
        "^"
    } else {
        "~"
    };
    let mut acc = String::new();
    let mut out: Vec<String> = Vec::new();
    for _ in 0..kind.count() {
        if !acc.is_empty() {
            acc.push(' ');
        }
        acc.push_str(unit);
        out.push(acc.clone());
    }
    suggest_matching(out.iter().map(String::as_str), builder);
}

/// The value-shaped types, resolved from the wire's type name.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Coords(Coords),
    /// A fixed word list, suggested and required.
    Choice(&'static [&'static str]),
    /// `MinMaxBounds` — the same shape `@e[distance=…]` takes.
    Range,
    /// `readString`, with no suggestions: a scoreboard objective, a team name,
    /// a score holder, a UUID, a hex colour.
    Word,
    /// A greedy remainder — `message`, which vanilla also scans for selectors.
    Greedy,
    /// An identifier, optionally `#`-prefixed for the tag-accepting variants.
    /// The `Option` is the registry the wire named, where there was one.
    Id { tag: bool },
    /// An SNBT value's EXTENT (M121) — `nbt_tag`, and `component` / `style`,
    /// which are a `TagParser` value fed to a codec.
    Snbt,
    /// The same, restricted to a compound: `nbt_compound_tag`.
    SnbtCompound,
    /// `nbt_path`, whose own grammar is transcribed.
    NbtPath,
    /// `dialog` — a `ResourceOrIdArgument`: an id, or an inline value.
    IdOrSnbt,
}

/// Resolve a `minecraft:` type name to its value shape, or `None` when this
/// module does not handle it.
pub fn resolve(type_name: &str) -> Option<Value> {
    Some(match type_name {
        "minecraft:block_pos" => Value::Coords(Coords::BlockPos),
        "minecraft:column_pos" => Value::Coords(Coords::ColumnPos),
        "minecraft:vec3" => Value::Coords(Coords::Vec3),
        "minecraft:vec2" => Value::Coords(Coords::Vec2),
        "minecraft:rotation" => Value::Coords(Coords::Rotation),
        "minecraft:angle" => Value::Coords(Coords::Angle),

        "minecraft:entity_anchor" => Value::Choice(&ANCHORS),
        "minecraft:operation" => Value::Choice(&OPERATIONS),
        "minecraft:gamemode" => Value::Choice(&GAMEMODES),
        "minecraft:template_mirror" => Value::Choice(&MIRRORS),
        "minecraft:template_rotation" => Value::Choice(&ROTATIONS),

        "minecraft:int_range" | "minecraft:float_range" => Value::Range,

        // Words, and the ones whose literal tables live in another class —
        // see the module docs. They parse; they do not suggest.
        "minecraft:objective"
        | "minecraft:team"
        | "minecraft:score_holder"
        | "minecraft:uuid"
        | "minecraft:hex_color"
        | "minecraft:objective_criteria"
        | "minecraft:heightmap"
        | "minecraft:team_color"
        | "minecraft:swizzle"
        | "minecraft:scoreboard_slot"
        | "minecraft:item_slot"
        | "minecraft:item_slots"
        | "minecraft:time" => Value::Word,

        "minecraft:message" => Value::Greedy,

        "minecraft:resource_location"
        | "minecraft:resource"
        | "minecraft:resource_key"
        | "minecraft:resource_selector"
        | "minecraft:dimension"
        | "minecraft:function"
        | "minecraft:loot_table"
        | "minecraft:loot_predicate"
        | "minecraft:loot_modifier"
        | "minecraft:particle" => Value::Id { tag: false },
        // The `_or_tag` pair accept a leading `#`.
        "minecraft:resource_or_tag" | "minecraft:resource_or_tag_key" => Value::Id { tag: true },

        // M121 — the six structured ones. `component` and `style` are a
        // `TagParser` value handed to a codec, so the TEXT they parse is SNBT
        // and the codec only decides whether the decoded value is valid;
        // `crate::snbt` reads the extent and states plainly that it does not
        // validate.
        "minecraft:nbt_tag" | "minecraft:component" | "minecraft:style" => Value::Snbt,
        "minecraft:nbt_compound_tag" => Value::SnbtCompound,
        "minecraft:nbt_path" => Value::NbtPath,
        "minecraft:dialog" => Value::IdOrSnbt,

        _ => return None,
    })
}

impl Value {
    pub fn parse(&self, reader: &mut StringReader) -> Result<(), ReaderError> {
        match self {
            Self::Coords(k) => read_coords(reader, *k),
            Self::Choice(choices) => {
                let start = reader.cursor();
                let word = read_operation_or_word(reader, choices);
                if choices.contains(&word.as_str()) {
                    Ok(())
                } else {
                    // The rollback M118 records: the suggester's prefix has to
                    // survive the failure.
                    reader.set_cursor(start);
                    Err(ReaderError::UnknownArgumentType)
                }
            }
            Self::Range => crate::selector::read_range(reader),
            Self::Word => {
                reader.read_string()?;
                Ok(())
            }
            Self::Greedy => {
                reader.set_cursor(reader.total_length());
                Ok(())
            }
            Self::Snbt => crate::snbt::read_value_extent(reader),
            Self::SnbtCompound => crate::snbt::read_compound_extent(reader),
            Self::NbtPath => crate::snbt::read_nbt_path(reader),
            Self::IdOrSnbt => crate::snbt::read_id_or_value(reader),
            Self::Id { tag } => {
                if *tag && reader.can_read() && reader.peek() == b'#' as u16 {
                    reader.skip();
                }
                let start = reader.cursor();
                while reader.can_read() && is_identifier_char(reader.peek()) {
                    reader.skip();
                }
                if reader.cursor() == start {
                    return Err(ReaderError::UnknownArgumentType);
                }
                Ok(())
            }
        }
    }

    pub fn suggest(
        &self,
        builder: &mut SuggestionsBuilder,
        registry: Option<&str>,
        blocks: Option<&rewo_data::blocks::Blocks>,
        items: Option<&rewo_data::items::Items>,
    ) {
        match self {
            Self::Coords(k) => suggest_coords(builder, *k),
            Self::Choice(choices) => suggest_matching(choices.iter().copied(), builder),
            Self::Id { .. } => {
                // The wire names the registry (M113's `ArgumentProps::Registry`),
                // so the RIGHT one is always known — only two of them are held.
                match registry {
                    Some("minecraft:block") => {
                        if let Some(b) = blocks {
                            suggest_resource(b.names().iter().map(String::as_str), builder);
                        }
                    }
                    Some("minecraft:item") => {
                        if let Some(i) = items {
                            suggest_resource(i.names(), builder);
                        }
                    }
                    _ => {}
                }
            }
            // None of the six suggests: vanilla's come from a codec's own
            // completion machinery, which needs the value decoded rather than
            // measured.
            Self::Range
            | Self::Word
            | Self::Greedy
            | Self::Snbt
            | Self::SnbtCompound
            | Self::NbtPath
            | Self::IdOrSnbt => {}
        }
    }
}

/// An operation is punctuation, so it cannot be read as a word.
fn read_operation_or_word(reader: &mut StringReader, choices: &[&str]) -> String {
    let punctuation = choices.iter().all(|c| !c.chars().next().is_some_and(char::is_alphanumeric));
    if !punctuation {
        return reader.read_unquoted_string();
    }
    let start = reader.cursor();
    while reader.can_read() && !matches!(reader.peek(), 0x20 | 0x09) {
        reader.skip();
    }
    String::from_utf16_lossy(&reader.string()[start..reader.cursor()])
}

fn is_identifier_char(c: u16) -> bool {
    (0x30..=0x39).contains(&c)
        || (0x61..=0x7A).contains(&c)
        || matches!(c, 0x5F | 0x3A | 0x2F | 0x2E | 0x2D)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(kind: Coords, s: &str) -> bool {
        let units: Vec<u16> = s.encode_utf16().collect();
        let mut r = StringReader::new(&units);
        read_coords(&mut r, kind).is_ok() && r.cursor() == units.len()
    }

    fn offer(kind: Coords, typed: &str) -> Vec<String> {
        let units: Vec<u16> = typed.encode_utf16().collect();
        let mut b = SuggestionsBuilder::new(&units, 0);
        suggest_coords(&mut b, kind);
        b.build().list.into_iter().map(|s| s.text).collect()
    }

    // ── coordinates ──────────────────────────────────────────────────────

    #[test]
    fn a_bare_tilde_is_a_complete_coordinate() {
        // The number is optional after `~`, guarded by `peek() != ' '` rather
        // than by "is there a digit" — which is the whole reason `/tp ~ ~ ~`
        // parses.
        assert!(ok(Coords::BlockPos, "~ ~ ~"));
        assert!(ok(Coords::BlockPos, "~1 ~-2 ~"));
        assert!(ok(Coords::Vec3, "~ ~ ~"));
    }

    #[test]
    fn a_block_position_takes_an_integer_absolutely_and_a_double_relatively() {
        // `parseInt` reads a DOUBLE when relative and an INT when not.
        assert!(ok(Coords::BlockPos, "1 2 3"));
        assert!(ok(Coords::BlockPos, "~0.5 ~ ~"));
        // `0.5` absolute stops at the `.`, leaving `.5` unread.
        assert!(!ok(Coords::BlockPos, "0.5 2 3"));
        // …where a vec3 takes it.
        assert!(ok(Coords::Vec3, "0.5 2 3"));
    }

    #[test]
    fn local_coordinates_are_all_or_nothing_across_the_triple() {
        assert!(ok(Coords::Vec3, "^ ^ ^"));
        assert!(ok(Coords::Vec3, "^1 ^2 ^-3"));
        // `ERROR_MIXED_TYPE` — vanilla peeks once and commits, so a mixed
        // triple is rejected rather than parsed per component.
        assert!(!ok(Coords::Vec3, "^1 ~2 3"));
        assert!(!ok(Coords::Vec3, "1 ^2 ^3"));
    }

    #[test]
    fn the_types_without_a_local_form_reject_the_caret() {
        // `Vec2`, `Rotation` and `Angle` have no `LocalCoordinates` path, so
        // `^` reaches the world reader and is its `ERROR_MIXED_TYPE`.
        assert!(!ok(Coords::Rotation, "^ ^"));
        assert!(!ok(Coords::Angle, "^"));
        assert!(ok(Coords::Rotation, "~ ~"));
        assert!(ok(Coords::Angle, "~90"));
    }

    #[test]
    fn each_type_takes_exactly_its_own_number_of_components() {
        assert!(ok(Coords::Angle, "0"));
        assert!(!ok(Coords::Angle, "0 0"), "one component, and the rest is not its own");
        assert!(ok(Coords::Vec2, "1 2"));
        assert!(!ok(Coords::Vec2, "1"));
        assert!(ok(Coords::ColumnPos, "1 2"));
        assert!(ok(Coords::BlockPos, "1 2 3"));
        assert!(!ok(Coords::BlockPos, "1 2"));
    }

    #[test]
    fn the_components_must_be_separated_by_exactly_one_space() {
        assert!(!ok(Coords::Vec2, "1,2"));
        assert!(!ok(Coords::Vec2, "12"));
        // The separator check is only OBSERVABLE where the next component
        // would parse without it. `1,2` and `12` fail through the number
        // reader either way; `1~2` does not, because `~` is where a
        // coordinate legitimately starts. A fixture of malformed numbers
        // cannot see the rule at all.
        assert!(!ok(Coords::Vec2, "1~2"));
        assert!(ok(Coords::Vec2, "1 ~2"));
    }

    // ── the coordinate suggestions ───────────────────────────────────────

    #[test]
    fn the_defaults_are_offered_progressively_rather_than_only_complete() {
        // `suggestCoordinates` builds `x`, `x y`, `x y z`, so Tab can fill one
        // axis. Offering only the full triple takes the choice away.
        assert_eq!(offer(Coords::BlockPos, ""), ["~", "~ ~", "~ ~ ~"]);
        assert_eq!(offer(Coords::Vec2, ""), ["~", "~ ~"]);
    }

    #[test]
    fn a_typed_caret_switches_the_default_set_to_the_local_one() {
        // `remainder.charAt(0) == '^'`. The single `^` is absent because it
        // EQUALS what is typed, and `SuggestionsBuilder.suggest` drops that
        // (M114a) — offering it would insert nothing. Second time that rule
        // has corrected a witness of mine.
        assert_eq!(offer(Coords::Vec3, "^"), ["^ ^", "^ ^ ^"]);
        // …and a type with no local form stays on the world set, which then
        // matches nothing.
        assert!(offer(Coords::Rotation, "^").is_empty());
    }

    // ── the value shapes ─────────────────────────────────────────────────

    #[test]
    fn an_operation_is_punctuation_and_cannot_be_read_as_a_word() {
        // `readUnquotedString` accepts none of `= += -= *= /= %= < > ><`, so
        // reading one as a word yields the empty string and every operation
        // fails to parse.
        let v = resolve("minecraft:operation").unwrap();
        for op in OPERATIONS {
            let units: Vec<u16> = op.encode_utf16().collect();
            let mut r = StringReader::new(&units);
            assert!(v.parse(&mut r).is_ok(), "{op}");
            assert_eq!(r.cursor(), units.len(), "{op}");
        }
        let units: Vec<u16> = "=?".encode_utf16().collect();
        assert!(v.parse(&mut StringReader::new(&units)).is_err());
    }

    #[test]
    fn a_failed_choice_rolls_back_so_its_suggester_keeps_the_prefix() {
        // M118's rule, one module over: `rollbackAndThrow` puts the cursor
        // back to the start of the value, so `fillSuggestions` offsets there
        // and the half-typed word is still the prefix. Without it the
        // remaining text is empty, every choice matches, and picking one
        // appends rather than replaces.
        let v = resolve("minecraft:entity_anchor").unwrap();
        let units: Vec<u16> = "ey".encode_utf16().collect();
        let mut r = StringReader::new(&units);
        assert!(v.parse(&mut r).is_err());
        assert_eq!(r.cursor(), 0, "the cursor went back");
        let mut b = SuggestionsBuilder::new(&units, r.cursor());
        v.suggest(&mut b, None, None, None);
        let texts: Vec<String> = b.build().list.into_iter().map(|s| s.text).collect();
        assert_eq!(texts, ["eyes"]);
    }

    #[test]
    fn a_rotation_is_spelled_180_rather_than_clockwise_180() {
        // `Rotation`'s serialized names, and the odd one out.
        assert!(ROTATIONS.contains(&"180"));
        assert!(!ROTATIONS.contains(&"clockwise_180"));
    }

    #[test]
    fn a_message_swallows_the_remainder() {
        let v = resolve("minecraft:message").unwrap();
        let units: Vec<u16> = "hello there world".encode_utf16().collect();
        let mut r = StringReader::new(&units);
        assert!(v.parse(&mut r).is_ok());
        assert_eq!(r.cursor(), units.len());
    }

    #[test]
    fn a_tag_accepting_id_takes_a_hash_and_a_plain_one_does_not() {
        let tagged = resolve("minecraft:resource_or_tag").unwrap();
        let plain = resolve("minecraft:resource").unwrap();
        let units: Vec<u16> = "#minecraft:logs".encode_utf16().collect();
        let mut r = StringReader::new(&units);
        assert!(tagged.parse(&mut r).is_ok());
        assert_eq!(r.cursor(), units.len());
        // The plain one stops before the `#`, having read nothing.
        let mut r = StringReader::new(&units);
        assert!(plain.parse(&mut r).is_err());
    }

    #[test]
    fn the_structured_types_are_claimed_as_extents() {
        // M120 asserted these were NOT claimed; M121 claims them, and this
        // test inverted with the code rather than being deleted — the shape
        // that would otherwise rot silently.
        for (t, want) in [
            ("minecraft:component", Value::Snbt),
            ("minecraft:style", Value::Snbt),
            ("minecraft:nbt_tag", Value::Snbt),
            ("minecraft:nbt_compound_tag", Value::SnbtCompound),
            ("minecraft:nbt_path", Value::NbtPath),
            ("minecraft:dialog", Value::IdOrSnbt),
        ] {
            assert_eq!(resolve(t), Some(want), "{t}");
        }
    }

    #[test]
    fn every_other_minecraft_type_is_claimed() {
        // The list this milestone is measured against. `entity`, the block and
        // item pair are handled by their own modules, so they are absent here
        // by design rather than by omission.
        let elsewhere = [
            "minecraft:entity",
            "minecraft:game_profile",
            "minecraft:block_state",
            "minecraft:block_predicate",
            "minecraft:item_stack",
            "minecraft:item_predicate",
        ];
        // M121 emptied this: every `minecraft:` type is now claimed either
        // here or by its own module.
        let structured: [&str; 0] = [];
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
        let mut unclaimed: Vec<&str> = Vec::new();
        for t in all {
            if elsewhere.contains(&t) || structured.contains(&t) {
                continue;
            }
            if resolve(t).is_none() {
                unclaimed.push(t);
            }
        }
        assert!(
            unclaimed.is_empty(),
            "unclaimed minecraft: argument types: {unclaimed:?}"
        );
    }
}
