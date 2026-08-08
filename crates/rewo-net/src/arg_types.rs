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
//! The **six whose tables live outside their argument class** are done as of
//! M124, and the sentence they used to carry here — that they were wrong only
//! in the suggestions, never in the parse — was wrong about three of them.
//! `heightmap` is `Heightmap.Types` **filtered by `keepAfterWorldgen`**, which
//! drops two of six; `team_color` is `TeamColor.VALUES` and `scoreboard_slot`
//! is `DisplaySlot`, both closed sets a bare word does not check against;
//! `swizzle` has a real parse (each axis at most once) and, uniquely here, no
//! `listSuggestions` at all; and `item_slot` / `item_slots` read to the next
//! SPACE rather than as an unquoted string, look their name up in
//! [`crate::slot_ranges`], and differ from each other in the parse as well as
//! the suggestions.
//!
//! Two more went with them that the plan's list had missed, both in the same
//! category and both reading as bare words: `time` is a float followed by a
//! unit from a **four**-entry map whose fourth key is the EMPTY string (so a
//! bare number is a duration in ticks), and its suggester **re-anchors past the
//! number** so the unit completes as a suffix; and `hex_color` is three or six
//! hex digits, suggesting its own two `EXAMPLES`.
//!
//! What is still a bare word is what has no literal table. Three of those —
//! `objective`, `team`, `objective_criteria` — do have a vanilla suggester, but
//! each reads **live state** (the scoreboard, the stat registries) rather than
//! a list, which is a different job.
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
/// `TimeArgument.UNITS`' keys — the empty one is real and means "ticks", so a
/// bare number is a legal duration and any other suffix is not.
pub const TIME_UNITS: [&str; 4] = ["d", "s", "t", ""];
/// `HexColorArgument.EXAMPLES`, which is also what it SUGGESTS — vanilla
/// offers its own two examples rather than any kind of colour list.
pub const HEX_COLOR_EXAMPLES: [&str; 2] = ["F00", "FF0000"];
/// `Heightmap.Types`, **filtered by `keepAfterWorldgen`** and lowercased.
///
/// The filter is the whole point: the enum has six members and this list has
/// four, because `WORLD_SURFACE_WG` and `OCEAN_FLOOR_WG` are `Usage.WORLDGEN`.
/// A transcription of "the Heightmap.Types enum" offers two names the server
/// rejects.
pub const HEIGHTMAPS: [&str; 4] = [
    "world_surface",
    "ocean_floor",
    "motion_blocking",
    "motion_blocking_no_leaves",
];
/// `TeamColor.VALUES`' serialized names — `ChatFormatting`'s sixteen, in its
/// order.
pub const TEAM_COLORS: [&str; 16] = [
    "black", "dark_blue", "dark_green", "dark_aqua", "dark_red", "dark_purple", "gold", "gray",
    "dark_gray", "blue", "green", "aqua", "red", "light_purple", "yellow", "white",
];
/// `DisplaySlot`'s — three plain slots, then the sixteen `sidebar.team.*` ones
/// in the same colour order as [`TEAM_COLORS`].
///
/// Their dots are load-bearing for the suggester, not decoration: `.` is one of
/// `matchesSubStr`'s splitters, so typing `team` or `black` both match
/// `sidebar.team.black`.
pub const SCOREBOARD_SLOTS: [&str; 19] = [
    "list",
    "sidebar",
    "below_name",
    "sidebar.team.black",
    "sidebar.team.dark_blue",
    "sidebar.team.dark_green",
    "sidebar.team.dark_aqua",
    "sidebar.team.dark_red",
    "sidebar.team.dark_purple",
    "sidebar.team.gold",
    "sidebar.team.gray",
    "sidebar.team.dark_gray",
    "sidebar.team.blue",
    "sidebar.team.green",
    "sidebar.team.aqua",
    "sidebar.team.red",
    "sidebar.team.light_purple",
    "sidebar.team.yellow",
    "sidebar.team.white",
];

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
    /// `swizzle` — some of `x`, `y`, `z`, each at most once. **Suggests
    /// nothing**, because `SwizzleArgument` has no `listSuggestions` override
    /// and so inherits brigadier's empty default.
    Swizzle,
    /// `item_slot` (`single`) and `item_slots`, over [`crate::slot_ranges`].
    Slot { single: bool },
    /// `time` — a float and then a unit from [`TIME_UNITS`].
    Time,
    /// `hex_color` — three or six hex digits.
    HexColor,
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
        "minecraft:heightmap" => Value::Choice(&HEIGHTMAPS),
        "minecraft:team_color" => Value::Choice(&TEAM_COLORS),
        "minecraft:scoreboard_slot" => Value::Choice(&SCOREBOARD_SLOTS),

        "minecraft:swizzle" => Value::Swizzle,
        "minecraft:item_slot" => Value::Slot { single: true },
        "minecraft:item_slots" => Value::Slot { single: false },

        "minecraft:int_range" | "minecraft:float_range" => Value::Range,

        // Words, and the ones whose literal tables live in another class —
        // see the module docs. They parse; they do not suggest.
        "minecraft:objective"
        | "minecraft:team"
        | "minecraft:score_holder"
        | "minecraft:uuid"
        | "minecraft:objective_criteria" => Value::Word,

        "minecraft:time" => Value::Time,
        "minecraft:hex_color" => Value::HexColor,

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
            // `TimeArgument.parse`: a float, then an unquoted string looked up
            // in UNITS. The empty key is IN that map, so a bare number is a
            // duration in ticks — the error test is `factor == 0`, not "was
            // there a unit".
            Self::Time => {
                reader.read_f32()?;
                let unit = reader.read_unquoted_string();
                if TIME_UNITS.contains(&unit.as_str()) {
                    Ok(())
                } else {
                    Err(ReaderError::UnknownArgumentType)
                }
            }
            // `HexColorArgument.parse`: an unquoted string of exactly three or
            // six hex digits. Any other length is `argument.hexcolor.invalid`,
            // so `#` is not part of it and four digits is not a colour.
            Self::HexColor => {
                let start = reader.cursor();
                let text = reader.read_unquoted_string();
                let hex = matches!(text.len(), 3 | 6)
                    && text.chars().all(|c| c.is_ascii_hexdigit());
                if hex {
                    Ok(())
                } else {
                    reader.set_cursor(start);
                    Err(ReaderError::UnknownArgumentType)
                }
            }
            // `SwizzleArgument.parse`: read to the next space, each character
            // an axis, none of them twice. The loop may run ZERO times, so an
            // empty swizzle is a legal (empty) set — vanilla's, not a slip.
            Self::Swizzle => {
                let mut seen = [false; 3];
                while reader.can_read() && reader.peek() != b' ' as u16 {
                    let axis = match reader.read() as u8 {
                        b'x' => 0,
                        b'y' => 1,
                        b'z' => 2,
                        _ => return Err(ReaderError::UnknownArgumentType),
                    };
                    if std::mem::replace(&mut seen[axis], true) {
                        return Err(ReaderError::UnknownArgumentType);
                    }
                }
                Ok(())
            }
            // `SlotArgument` / `SlotsArgument`: `readWhile(c != ' ')` — NOT an
            // unquoted string, because `container.*` has to survive — then the
            // name must exist, and for `item_slot` cover exactly one slot.
            Self::Slot { single } => {
                let start = reader.cursor();
                while reader.can_read() && reader.peek() != b' ' as u16 {
                    reader.skip();
                }
                let name: String = String::from_utf16_lossy(&reader.string()[start..reader.cursor()]);
                match crate::slot_ranges::lookup(&name) {
                    Some(1) => Ok(()),
                    Some(_) if !*single => Ok(()),
                    // `slot.only_single_allowed` and `slot.unknown` are two
                    // different errors in vanilla and one here; what matters is
                    // that neither is a value.
                    _ => {
                        reader.set_cursor(start);
                        Err(ReaderError::UnknownArgumentType)
                    }
                }
            }
            Self::Word => {
                reader.read_string()?;
                Ok(())
            }
            Self::Greedy => {
                reader.set_cursor(reader.total_length());
                Ok(())
            }
            // M122 — the grammar, where M121 measured an extent. `{a:}` is
            // now an error rather than a value.
            Self::Snbt => crate::snbt_grammar::parse_value(reader),
            Self::SnbtCompound => crate::snbt_grammar::parse_compound(reader),
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
            // The two lists the two types differ by — and the same split the
            // parse enforces, so they cannot drift.
            Self::Slot { single: true } => {
                suggest_matching(crate::slot_ranges::single_slot_names(), builder)
            }
            Self::Slot { single: false } => {
                suggest_matching(crate::slot_ranges::all_names(), builder)
            }
            // `TimeArgument.listSuggestions` re-anchors the builder PAST the
            // number, so the unit completes as a suffix rather than replacing
            // the whole argument — and it offers nothing at all when what has
            // been typed is not a float yet.
            Self::Time => {
                let units = builder.input_units();
                let mut r = StringReader::new(&units);
                r.set_cursor(builder.start());
                if r.read_f32().is_err() {
                    return;
                }
                let after_number = r.cursor();
                builder.rebase(after_number);
                suggest_matching(TIME_UNITS.iter().copied(), builder);
            }
            Self::HexColor => suggest_matching(HEX_COLOR_EXAMPLES.iter().copied(), builder),
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
            | Self::IdOrSnbt
            // Not an omission: `SwizzleArgument` declares no `listSuggestions`.
            | Self::Swizzle => {}
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

    // ── M124: the seven literal tables ───────────────────────────────────

    fn parses(type_name: &str, s: &str) -> bool {
        let units: Vec<u16> = s.encode_utf16().collect();
        let mut r = StringReader::new(&units);
        resolve(type_name).is_some_and(|v| v.parse(&mut r).is_ok()) && r.cursor() == units.len()
    }

    fn offers(type_name: &str, typed: &str) -> Vec<String> {
        let units: Vec<u16> = typed.encode_utf16().collect();
        let mut b = SuggestionsBuilder::new(&units, 0);
        resolve(type_name).unwrap().suggest(&mut b, None, None, None);
        b.build().list.into_iter().map(|s| s.text).collect()
    }

    #[test]
    fn a_heightmap_offers_four_names_because_two_are_filtered_out() {
        // `keepAfterWorldgen` is `usage != WORLDGEN`, which drops
        // WORLD_SURFACE_WG and OCEAN_FLOOR_WG from an enum of six.
        assert_eq!(offers("minecraft:heightmap", "").len(), 4);
        assert!(!offers("minecraft:heightmap", "")
            .iter()
            .any(|s| s.ends_with("_wg")));
        assert!(parses("minecraft:heightmap", "world_surface"));
        assert!(!parses("minecraft:heightmap", "world_surface_wg"));
        // Lowercased by `convertId`; the enum constants are upper.
        assert!(!parses("minecraft:heightmap", "WORLD_SURFACE"));
    }

    #[test]
    fn a_team_colour_is_chat_formattings_sixteen_and_not_a_dye_list() {
        assert_eq!(offers("minecraft:team_color", "").len(), 16);
        assert!(parses("minecraft:team_color", "dark_aqua"));
        assert!(parses("minecraft:team_color", "light_purple"));
        // The four dye names that are NOT chat colours.
        for absent in ["orange", "magenta", "lime", "pink"] {
            assert!(!parses("minecraft:team_color", absent), "{absent}");
        }
        // `reset` is a ChatFormatting and not a TeamColor.
        assert!(!parses("minecraft:team_color", "reset"));
    }

    #[test]
    fn a_scoreboard_slots_dots_are_splitters_for_the_suggester() {
        assert_eq!(offers("minecraft:scoreboard_slot", "").len(), 19);
        assert!(parses("minecraft:scoreboard_slot", "below_name"));
        assert!(parses("minecraft:scoreboard_slot", "sidebar.team.dark_gray"));
        // `matchesSubStr` splits on `.`, so a fragment from the middle or the
        // end of a dotted name matches it — this is why typing the colour
        // alone finds its team slot.
        assert!(offers("minecraft:scoreboard_slot", "team")
            .iter()
            .any(|s| s == "sidebar.team.black"));
        assert!(offers("minecraft:scoreboard_slot", "black")
            .iter()
            .any(|s| s == "sidebar.team.black"));
        // …and `sidebar` alone matches SIXTEEN, not seventeen, because
        // `SuggestionsBuilder.suggest` drops a candidate equal to the text
        // already typed — there is nothing left to complete. One character
        // short and the plain `sidebar` is back.
        assert_eq!(offers("minecraft:scoreboard_slot", "sidebar").len(), 16);
        assert!(!offers("minecraft:scoreboard_slot", "sidebar")
            .iter()
            .any(|s| s == "sidebar"));
        assert!(offers("minecraft:scoreboard_slot", "sideba")
            .iter()
            .any(|s| s == "sidebar"));
    }

    #[test]
    fn a_swizzle_is_each_axis_at_most_once_and_suggests_nothing() {
        assert!(parses("minecraft:swizzle", "xyz"));
        assert!(parses("minecraft:swizzle", "x"));
        assert!(parses("minecraft:swizzle", "zx"));
        assert!(!parses("minecraft:swizzle", "xx"));
        // A SINGLE non-axis character is what separates "not an axis" from
        // "already seen": `xw` and `XYZ` both die on the duplicate rule even
        // when every unknown character silently maps to `x`, so neither can
        // witness the axis test on its own.
        assert!(!parses("minecraft:swizzle", "w"));
        assert!(!parses("minecraft:swizzle", "X"));
        assert!(!parses("minecraft:swizzle", "xw"));
        assert!(!parses("minecraft:swizzle", "XYZ"));
        // `SwizzleArgument` declares no `listSuggestions`, so it inherits
        // brigadier's empty default. Nothing is missing here.
        assert!(offers("minecraft:swizzle", "").is_empty());
        assert!(offers("minecraft:swizzle", "x").is_empty());
    }

    #[test]
    fn an_empty_swizzle_parses_because_the_loop_may_run_zero_times() {
        // `while (reader.canRead() && reader.peek() != ' ')` with nothing to
        // read yields an empty EnumSet rather than an error. Recorded because
        // it looks like a bug and is what vanilla does.
        assert!(parses("minecraft:swizzle", ""));
    }

    #[test]
    fn item_slot_and_item_slots_differ_in_the_parse_not_only_the_suggestions() {
        // `SlotArgument` rejects `size() != 1`; `SlotsArgument` does not.
        assert!(parses("minecraft:item_slot", "weapon.mainhand"));
        assert!(parses("minecraft:item_slots", "weapon.mainhand"));
        assert!(!parses("minecraft:item_slot", "container.*"));
        assert!(parses("minecraft:item_slots", "container.*"));
        // An unknown name is an error for both.
        assert!(!parses("minecraft:item_slot", "container.54"));
        assert!(!parses("minecraft:item_slots", "container.54"));
        // The suggestion lists are exactly that same split.
        assert_eq!(offers("minecraft:item_slots", "").len(), 165);
        assert_eq!(offers("minecraft:item_slot", "").len(), 156);
    }

    #[test]
    fn a_slot_name_is_read_to_the_space_not_as_an_unquoted_string() {
        // `*` is not allowed in an unquoted string, so reading one truncates
        // `container.*` to `container.` and then fails the lookup — which
        // would make every star form unusable while looking like a table gap.
        assert!(parses("minecraft:item_slots", "armor.*"));
        assert!(parses("minecraft:item_slots", "player.crafting.*"));
        // …and the read stops at a space, so the next word survives for the
        // dispatcher.
        let units: Vec<u16> = "weapon rest".encode_utf16().collect();
        let mut r = StringReader::new(&units);
        assert!(resolve("minecraft:item_slot").unwrap().parse(&mut r).is_ok());
        assert_eq!(r.cursor(), 6);
    }

    #[test]
    fn a_duration_is_a_float_and_then_a_unit_from_a_four_entry_map() {
        // The EMPTY key is in `UNITS`, so a bare number is a duration in
        // ticks. The error test is `factor == 0`, not "was there a unit".
        assert!(parses("minecraft:time", "0"));
        assert!(parses("minecraft:time", "1d"));
        assert!(parses("minecraft:time", "2.5s"));
        assert!(parses("minecraft:time", "20t"));
        assert!(!parses("minecraft:time", "1h"));
        assert!(!parses("minecraft:time", "d"));
        assert!(!parses("minecraft:time", "abc"));
    }

    #[test]
    fn a_durations_unit_completes_after_the_number_not_over_it() {
        // `listSuggestions` re-anchors with `createOffset(start + cursor)`, so
        // the offered range starts past the float. Suggesting from the
        // argument's own start would replace `10` along with the unit.
        let units: Vec<u16> = "10".encode_utf16().collect();
        let mut b = SuggestionsBuilder::new(&units, 0);
        resolve("minecraft:time").unwrap().suggest(&mut b, None, None, None);
        let built = b.build();
        assert_eq!(built.range.start, 2, "the unit replaces nothing typed");
        let texts: Vec<String> = built.list.into_iter().map(|s| s.text).collect();
        // Three, not four: the empty unit is dropped by the same exact-match
        // rule that hides `sidebar` from its own offers.
        assert_eq!(texts.len(), 3);
        assert!(texts.contains(&"d".to_string()));
        // …and nothing at all until what has been typed is a float.
        assert!(offers("minecraft:time", "x").is_empty());
    }

    #[test]
    fn a_hex_colour_is_three_or_six_digits_and_suggests_its_own_examples() {
        assert!(parses("minecraft:hex_color", "F00"));
        assert!(parses("minecraft:hex_color", "FF0000"));
        assert!(parses("minecraft:hex_color", "abc"));
        // Four and five are not colours, and neither is a `#` prefix.
        assert!(!parses("minecraft:hex_color", "FF00"));
        assert!(!parses("minecraft:hex_color", "FF000"));
        assert!(!parses("minecraft:hex_color", "#F00"));
        assert!(!parses("minecraft:hex_color", "GGG"));
        // Vanilla suggests EXAMPLES itself — not a palette, not a colour list.
        assert_eq!(offers("minecraft:hex_color", ""), vec!["F00", "FF0000"]);
    }

    #[test]
    fn the_eight_no_longer_fall_through_to_word() {
        // They parsed as bare words before M124, which accepted anything
        // word-shaped and offered nothing. `time` and `hex_color` are here
        // because the plan's list of "seven" had missed them: both have a
        // real table and a real parse, and both were reading as words.
        for t in [
            "minecraft:heightmap",
            "minecraft:team_color",
            "minecraft:scoreboard_slot",
            "minecraft:swizzle",
            "minecraft:item_slot",
            "minecraft:item_slots",
            "minecraft:time",
            "minecraft:hex_color",
        ] {
            assert!(
                !matches!(resolve(t), Some(Value::Word)),
                "{t} is still a bare word"
            );
        }
        // What remains a word is what has no table to check against. Three of
        // these DO have a vanilla suggester, and each reads live state rather
        // than a literal list: `objective` and `team` come from the scoreboard
        // and `objective_criteria` from the stat registries. That is the next
        // category, not this one.
        for t in [
            "minecraft:objective",
            "minecraft:team",
            "minecraft:score_holder",
            "minecraft:uuid",
            "minecraft:objective_criteria",
        ] {
            assert!(matches!(resolve(t), Some(Value::Word)), "{t}");
        }
    }

}
