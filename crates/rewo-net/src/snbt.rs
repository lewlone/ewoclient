//! The six structured argument types (M121) — and one deliberate,
//! prominently-stated approximation.
//!
//! `component`, `style`, `dialog`, `nbt_tag`, `nbt_compound_tag` and
//! `nbt_path` all reduce to the same thing: **where does this value end?**
//! Four of them are `TagParser` directly, `dialog` is an id *or* a `TagParser`
//! value, and `nbt_path` embeds compounds inside its own small grammar.
//!
//! # What this does NOT do, and why it is said first
//!
//! **It does not validate SNBT.** 26.x's SNBT is
//! `net/minecraft/nbt/SnbtGrammar.java`, a **916-line packrat grammar** —
//! signed and unsigned type suffixes (`1b`, `1ub`, `1s`, `1ui`), hex and
//! binary numerals, exponents, a full escape table, typed arrays. Transcribing
//! it faithfully is its own milestone, and an *approximate* SNBT parser would
//! be worse than none: it would silently accept text the server rejects,
//! inside the parse that drives the highlighting and the completion.
//!
//! So this module answers the narrower question the **command layer** actually
//! asks — the value's **extent** — with a balanced-delimiter walk that is
//! quote- and escape-aware. It is exact about where a value stops and says
//! nothing about whether the value is well-formed.
//!
//! **The consequence, stated rather than buried:** Rewo **over-accepts**. A
//! malformed compound like `{a:}` parses here and is rejected by the server,
//! so the red unparsed tail M117 draws will be absent where vanilla shows one.
//! The trade is deliberate and it is the better one at this layer: the
//! alternative — leaving these six `Unknown` — stops the parse at the NBT
//! word, which costs the highlighting *and* the completion of **every later
//! word** in commands like `/data merge entity @s {…} …`. Over-accepting costs
//! one missing error indicator; refusing costs the rest of the line.
//!
//! # The extent walk is exactly three things
//!
//! A quoted string (`"` or `'`) ends at its unescaped terminator; a bracket
//! (`{` or `[`) ends at its match, counting nesting and skipping quoted
//! sections; anything else is a bare token running to the first delimiter.
//! **The quote-awareness is the load-bearing part**: a naive brace counter
//! reads `{a:"}"}` as ending at the quoted brace, which is three characters
//! early and silently truncates the argument.
//!
//! # `nbt_path`'s own grammar IS transcribed
//!
//! It is small and self-contained, so there is no reason to approximate it:
//! a quoted name, `[{…}]` (a match filter), `[]` (all elements), `[n]` (an
//! index), a leading `{…}` (a root filter, **first node only**), or a bare
//! name. Two details that read backwards:
//!
//! * **`isAllowedInUnquotedName` is a NEGATIVE set** — everything except
//!   `` ` ' " [ ] . { } `` and a space. So a path name may contain `:`, `-`,
//!   digits and capitals, which an identifier-shaped reader would reject.
//! * **A `{` root filter is legal only as the FIRST node**, and vanilla says
//!   so with an explicit `firstNode` flag rather than by grammar position.

use crate::dispatcher::{ReaderError, StringReader};

fn is_quote(c: u16) -> bool {
    c == b'"' as u16 || c == b'\'' as u16
}

/// Skip a quoted string, starting at its opening quote.
///
/// The escape rule is the one thing here that has to be right: a `\` consumes
/// the next character whatever it is, so `"a\"b"` ends at the *fourth* quote
/// and not the second.
fn skip_quoted(reader: &mut StringReader) -> Result<(), ReaderError> {
    let terminator = reader.peek();
    reader.skip();
    while reader.can_read() {
        let c = reader.read();
        if c == b'\\' as u16 {
            if !reader.can_read() {
                return Err(ReaderError::ExpectedEndOfQuote);
            }
            reader.skip();
        } else if c == terminator {
            return Ok(());
        }
    }
    Err(ReaderError::ExpectedEndOfQuote)
}

/// Skip a `{…}` or `[…]` group, starting at its opening bracket.
///
/// Counts nesting and **skips quoted sections**, which is what stops
/// `{a:"}"}` being read as ending three characters early.
fn skip_bracketed(reader: &mut StringReader) -> Result<(), ReaderError> {
    let open = reader.peek();
    let close = if open == b'{' as u16 {
        b'}' as u16
    } else {
        b']' as u16
    };
    let mut depth = 0usize;
    while reader.can_read() {
        let c = reader.peek();
        if is_quote(c) {
            skip_quoted(reader)?;
            continue;
        }
        reader.skip();
        if c == open {
            depth += 1;
        } else if c == close {
            depth -= 1;
            if depth == 0 {
                return Ok(());
            }
        }
    }
    Err(ReaderError::ExpectedEndOfQuote)
}

/// Where a bare SNBT token stops.
///
/// Not `isAllowedInUnquotedString`, and **`:` is deliberately not a
/// terminator**. Inside a group the bracket walk handles the separators, so
/// this branch is only ever reached at the top level — where a colon is part
/// of a namespaced value like `minecraft:server_links` rather than a
/// key/value separator. Including it truncates every such id at the colon,
/// which is how `dialog` and a bare resource value both broke on the first
/// run of this module's tests.
fn is_bare_terminator(c: u16) -> bool {
    matches!(
        c,
        0x20 | 0x2C | 0x7B | 0x7D | 0x5B | 0x5D // space , { } [ ]
    )
}

/// The extent of one SNBT value. See the module docs for what this claims.
pub fn read_value_extent(reader: &mut StringReader) -> Result<(), ReaderError> {
    if !reader.can_read() {
        return Err(ReaderError::ExpectedDouble);
    }
    let c = reader.peek();
    if is_quote(c) {
        return skip_quoted(reader);
    }
    if c == b'{' as u16 || c == b'[' as u16 {
        return skip_bracketed(reader);
    }
    let start = reader.cursor();
    while reader.can_read() && !is_bare_terminator(reader.peek()) {
        reader.skip();
    }
    if reader.cursor() == start {
        Err(ReaderError::ExpectedDouble)
    } else {
        Ok(())
    }
}

/// `parseCompoundAsArgument` — the same walk, but the value must be a
/// compound.
///
/// `castToCompoundOrThrow` is a *post*-parse check in vanilla, so a
/// non-compound is read and then rejected. Here the two collapse, because
/// nothing downstream reads the value.
pub fn read_compound_extent(reader: &mut StringReader) -> Result<(), ReaderError> {
    if !reader.can_read() || reader.peek() != b'{' as u16 {
        return Err(ReaderError::UnknownArgumentType);
    }
    skip_bracketed(reader)
}

/// `NbtPathArgument.isAllowedInUnquotedName` — a **negative** set.
fn is_allowed_in_unquoted_name(c: u16) -> bool {
    !matches!(
        c,
        0x20 | 0x22 | 0x27 | 0x5B | 0x5D | 0x2E | 0x7B | 0x7D // space " ' [ ] . { }
    )
}

/// `NbtPathArgument.parse` — the node loop and its separators.
///
/// The loop runs while the reader can read and is not on a space, so the path
/// ends at the argument separator like any other word. Between nodes vanilla
/// demands a `.` **unless** the next character opens a `[` or `{`, which is
/// why `a[0]b` is invalid and `a[0].b` and `a[0]` are not.
pub fn read_nbt_path(reader: &mut StringReader) -> Result<(), ReaderError> {
    let mut first = true;
    if !reader.can_read() || reader.peek() == b' ' as u16 {
        return Err(ReaderError::UnknownArgumentType);
    }
    while reader.can_read() && reader.peek() != b' ' as u16 {
        read_nbt_path_node(reader, first)?;
        first = false;
        if reader.can_read() {
            let next = reader.peek();
            if next != b' ' as u16 && next != b'[' as u16 && next != b'{' as u16 {
                if next != b'.' as u16 {
                    return Err(ReaderError::UnknownArgumentType);
                }
                reader.skip();
                // A trailing `.` is an error rather than an empty last node.
                if !reader.can_read() || reader.peek() == b' ' as u16 {
                    return Err(ReaderError::UnknownArgumentType);
                }
            }
        }
    }
    Ok(())
}

fn read_nbt_path_node(reader: &mut StringReader, first: bool) -> Result<(), ReaderError> {
    let c = reader.peek();
    if is_quote(c) {
        return skip_quoted(reader);
    }
    if c == b'[' as u16 {
        reader.skip();
        if !reader.can_read() {
            return Err(ReaderError::UnknownArgumentType);
        }
        let next = reader.peek();
        if next == b'{' as u16 {
            read_compound_extent(reader)?;
        } else if next == b']' as u16 {
            reader.skip();
            return Ok(());
        } else {
            reader.read_i32()?;
        }
        if !reader.can_read() || reader.peek() != b']' as u16 {
            return Err(ReaderError::UnknownArgumentType);
        }
        reader.skip();
        return Ok(());
    }
    if c == b'{' as u16 {
        // **First node only**, and vanilla enforces it with a flag rather than
        // by grammar position.
        if !first {
            return Err(ReaderError::UnknownArgumentType);
        }
        return read_compound_extent(reader);
    }
    let start = reader.cursor();
    while reader.can_read() && is_allowed_in_unquoted_name(reader.peek()) {
        reader.skip();
    }
    if reader.cursor() == start {
        Err(ReaderError::UnknownArgumentType)
    } else {
        Ok(())
    }
}

/// `ResourceOrIdArgument` — an id, or an inline value.
///
/// `dialog` is the only one of the six that takes either, and the grammar
/// decides by the first character: a `{` opens an inline value and anything
/// else is read as an identifier.
pub fn read_id_or_value(reader: &mut StringReader) -> Result<(), ReaderError> {
    if reader.can_read() && reader.peek() == b'{' as u16 {
        return read_compound_extent(reader);
    }
    let start = reader.cursor();
    while reader.can_read() && !is_bare_terminator(reader.peek()) {
        reader.skip();
    }
    if reader.cursor() == start {
        Err(ReaderError::UnknownArgumentType)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn extent(s: &str) -> Option<usize> {
        let units: Vec<u16> = s.encode_utf16().collect();
        let mut r = StringReader::new(&units);
        read_value_extent(&mut r).ok().map(|_| r.cursor())
    }

    fn path_ok(s: &str) -> bool {
        let units: Vec<u16> = s.encode_utf16().collect();
        let mut r = StringReader::new(&units);
        read_nbt_path(&mut r).is_ok() && r.cursor() == units.len()
    }

    // ── the extent walk ──────────────────────────────────────────────────

    #[test]
    fn a_compound_ends_at_its_matching_brace() {
        assert_eq!(extent("{a:1}"), Some(5));
        assert_eq!(extent("{a:{b:1},c:2} rest"), Some(13));
    }

    #[test]
    fn a_brace_inside_a_string_does_not_close_the_compound() {
        // The load-bearing half. A naive counter reads `{a:"}"}` as ending at
        // the quoted brace — three characters early — and silently truncates
        // the argument, which then makes the REST of the command unparseable
        // in a way that looks like a different bug entirely.
        assert_eq!(extent(r#"{a:"}"}"#), Some(7));
        assert_eq!(extent(r#"{a:"]["}"#), Some(8));
    }

    #[test]
    fn an_escaped_quote_does_not_end_its_string() {
        assert_eq!(extent(r#""a\"b""#), Some(6));
        assert_eq!(extent(r#"{a:"x\"}"}"#), Some(10));
    }

    #[test]
    fn a_list_and_a_typed_array_both_end_at_their_bracket() {
        assert_eq!(extent("[1,2,3]"), Some(7));
        assert_eq!(extent("[I;1,2,3]"), Some(9));
        assert_eq!(extent("[[1],[2]]"), Some(9));
    }

    #[test]
    fn a_bare_token_ends_at_a_structural_character() {
        // Not `isAllowedInUnquotedString`: `minecraft:stone` is ONE token even
        // though it contains a colon, because the colon only separates a key
        // from a value at the compound level.
        assert_eq!(extent("1b rest"), Some(2));
        assert_eq!(extent("true,x"), Some(4));
        assert_eq!(extent("minecraft:stone}"), Some(15));
    }

    #[test]
    fn an_unterminated_group_or_string_is_an_error() {
        assert_eq!(extent("{a:1"), None);
        assert_eq!(extent("[1,2"), None);
        assert_eq!(extent(r#""abc"#), None);
        assert_eq!(extent(""), None);
    }

    #[test]
    fn a_compound_is_required_where_only_a_compound_is_allowed() {
        let units: Vec<u16> = "[1]".encode_utf16().collect();
        assert!(read_compound_extent(&mut StringReader::new(&units)).is_err());
        let units: Vec<u16> = "{a:1}".encode_utf16().collect();
        assert!(read_compound_extent(&mut StringReader::new(&units)).is_ok());
    }

    #[test]
    fn malformed_snbt_is_ACCEPTED_and_that_is_the_stated_deviation() {
        // Recorded as a test rather than only as prose, so the day someone
        // transcribes `SnbtGrammar` this fails and asks to be updated. Vanilla
        // rejects both of these; Rewo reads their extent and moves on.
        assert_eq!(extent("{a:}"), Some(4));
        assert_eq!(extent("{:1}"), Some(4));
    }

    // ── nbt_path ─────────────────────────────────────────────────────────

    #[test]
    fn a_path_takes_names_indices_filters_and_the_all_elements_form() {
        assert!(path_ok("Inventory"));
        assert!(path_ok("Inventory[0]"));
        assert!(path_ok("Inventory[]"));
        assert!(path_ok("Inventory[{id:\"minecraft:stone\"}]"));
        assert!(path_ok("Inventory[0].tag.display.Name"));
        assert!(path_ok("{foo:1}.bar"));
    }

    #[test]
    fn a_root_filter_is_legal_only_as_the_first_node() {
        // vanilla's `firstNode` flag, not a grammar position.
        assert!(path_ok("{a:1}"));
        assert!(!path_ok("x.{a:1}"));
    }

    #[test]
    fn a_name_may_contain_what_an_identifier_may_not() {
        // `isAllowedInUnquotedName` is a NEGATIVE set — everything but
        // ` " ' [ ] . { }` — so capitals, colons and dashes are all fine.
        assert!(path_ok("SelectedItem"));
        assert!(path_ok("a:b-c_1"));
        // …and the excluded ones really are excluded.
        assert!(!path_ok("a b"));
    }

    #[test]
    fn nodes_are_separated_by_a_dot_unless_the_next_one_opens_a_bracket() {
        assert!(path_ok("a.b"));
        assert!(path_ok("a[0]"));
        assert!(path_ok("a[0][1]"));
        // No separator and no bracket.
        assert!(!path_ok("a[0]b"));
        // A trailing dot is an error rather than an empty last node.
        assert!(!path_ok("a."));
    }

    #[test]
    fn a_quoted_name_is_a_node() {
        assert!(path_ok("\"a b\""));
        assert!(path_ok("\"a b\".c"));
    }

    // ── dialog ───────────────────────────────────────────────────────────

    #[test]
    fn a_dialog_takes_an_id_or_an_inline_value() {
        for s in ["minecraft:server_links", "{type:\"minecraft:notice\"}"] {
            let units: Vec<u16> = s.encode_utf16().collect();
            let mut r = StringReader::new(&units);
            assert!(read_id_or_value(&mut r).is_ok(), "{s}");
            assert_eq!(r.cursor(), units.len(), "{s}");
        }
    }
}
