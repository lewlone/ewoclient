//! `SnbtGrammar` — SNBT, validated rather than measured (M122).
//!
//! M121 shipped an **extent** walk for the six structured argument types and
//! said plainly what it was: a balanced-delimiter scan that finds where a value
//! ends and says nothing about whether it is well-formed. This is the grammar
//! it deferred, so `{a:}` is now an error rather than a value, and M117's red
//! unparsed tail appears where vanilla shows one.
//!
//! Vanilla's is a **packrat combinator** grammar (`SnbtGrammar.createParser`);
//! this is recursive descent over the same **language**. The combinators are
//! plumbing — `Term.sequence`, `Term.alternative`, `Term.cut` — and what has to
//! be faithful is which strings are accepted.
//!
//! # `0b` is zero-as-a-byte and `0b1` is binary one
//!
//! The integer rule reads `0`, cuts, and then tries hex, **binary**, a decimal
//! (which is the explicit leading-zero error), and finally an empty marker.
//! There is no cut after the `b`, so when the binary numeral fails the parse
//! **backtracks** to the empty marker and the `b` is then consumed by the
//! optional integer suffix. Two readings of the same two characters, resolved
//! by backtracking rather than by lookahead.
//!
//! # A leading zero is an ERROR, not a value
//!
//! `Term.sequence(rules.named(decimalNumeral), Term.cut(), Term.fail(
//! ERROR_LEADING_ZERO_NOT_ALLOWED))` — `01` does not parse as `1`, and does
//! not parse as `0` followed by junk either. It is rejected outright, which is
//! the opposite of what almost every other number parser does.
//!
//! # `_` is a digit separator, banned only at the ENDS
//!
//! `NumberRunParseRule` accepts `_` inside every numeral base and then rejects
//! the run only if its first or last character is one. So `1__2` is legal and
//! `_1` and `1_` are not — a rule that admits doubled separators is unusual
//! enough to be worth not "tidying".
//!
//! # A float needs a `.`, an exponent, or an `f`/`d`
//!
//! `literal` tries the float rule **before** the integer one, and every float
//! alternative requires one of those three. That is what stops `1` being read
//! as a float and left for the integer rule to fail on.
//!
//! # A trailing comma is legal
//!
//! Both `mapEntries` and `listEntries` are
//! `Term.repeatedWithTrailingSeparator`, so `{a:1,}` and `[1,]` parse — and so
//! do `{}` and `[]`, because the repetition may be empty.
//!
//! # Two things 26.x added that a pre-26 transcription would miss
//!
//! * **`\s` escapes to a SPACE**, not to "whitespace". The escape table is
//!   `b s t n f r \ ' "` plus `xHH`, `uHHHH`, `UHHHHHHHH` and `N{name}`, where
//!   the name matches `[-a-zA-Z0-9 ]+`.
//! * **An unquoted string may be CALLED**: `unquotedStringOrBuiltIn` is
//!   `unquoted ( '(' arguments ')' )?`, so `bool(1)` is a value. A grammar
//!   without it reads `bool` and then chokes on the parenthesis.
//!
//! # Whitespace
//!
//! Every terminal rule opens with `input.skipWhitespace()`, so `{ a : 1 }` is
//! as valid as `{a:1}`. Skipping only between *entries* — the natural
//! reading — rejects a space before a colon.
//!
//! # What this still does not do
//!
//! It validates **syntax**, not **range**: vanilla additionally parses each
//! numeral and reports `ERROR_NUMBER_PARSE_FAILURE` when it overflows its
//! suffix, and rejects a non-finite float with `ERROR_INFINITY_NOT_ALLOWED`.
//! Rewo accepts `999999999999b`. That is a narrower over-acceptance than M121's
//! and it is named here rather than left to be discovered.

use crate::dispatcher::{ReaderError, StringReader};

fn err() -> ReaderError {
    ReaderError::UnknownArgumentType
}

fn skip_whitespace(r: &mut StringReader) {
    while r.can_read() && matches!(r.peek(), 0x20 | 0x09 | 0x0A | 0x0B | 0x0C | 0x0D) {
        r.skip();
    }
}

fn at(r: &StringReader, c: u8) -> bool {
    r.can_read() && r.peek() == c as u16
}

/// Consume `c` after whitespace, or fail without moving.
fn expect(r: &mut StringReader, c: u8) -> Result<(), ReaderError> {
    skip_whitespace(r);
    if at(r, c) {
        r.skip();
        Ok(())
    } else {
        Err(err())
    }
}

/// Consume either case of `c` after whitespace.
fn eat_either(r: &mut StringReader, lower: u8, upper: u8) -> bool {
    if r.can_read() && (r.peek() == lower as u16 || r.peek() == upper as u16) {
        r.skip();
        true
    } else {
        false
    }
}

/// `SnbtGrammar.canStartNumber`.
fn can_start_number(c: u16) -> bool {
    matches!(c, 0x2B | 0x2D | 0x2E) || (0x30..=0x39).contains(&c)
}

/// `NumberRunParseRule` for one base.
///
/// The underscore rule is the interesting half: `_` is accepted **inside** the
/// run and the run is rejected only when its first or last character is one.
fn number_run(r: &mut StringReader, accept: fn(u16) -> bool) -> Result<(), ReaderError> {
    skip_whitespace(r);
    let start = r.cursor();
    while r.can_read() && accept(r.peek()) {
        r.skip();
    }
    if r.cursor() == start {
        return Err(err());
    }
    let first = r.string()[start];
    let last = r.string()[r.cursor() - 1];
    if first == b'_' as u16 || last == b'_' as u16 {
        return Err(err());
    }
    Ok(())
}

fn is_decimal(c: u16) -> bool {
    (0x30..=0x39).contains(&c) || c == b'_' as u16
}
fn is_hex(c: u16) -> bool {
    is_decimal(c)
        || (0x41..=0x46).contains(&c)
        || (0x61..=0x66).contains(&c)
}
fn is_binary(c: u16) -> bool {
    c == 0x30 || c == 0x31 || c == b'_' as u16
}

fn decimal(r: &mut StringReader) -> Result<(), ReaderError> {
    number_run(r, is_decimal)
}

/// `sign?`
fn sign(r: &mut StringReader) {
    skip_whitespace(r);
    if at(r, b'+') || at(r, b'-') {
        r.skip();
    }
}

/// `integerSuffix?` — `[uU]` then one of `bBsSiIlL`, or a bare one of those.
fn integer_suffix(r: &mut StringReader) {
    let mark = r.cursor();
    if eat_either(r, b'u', b'U') {
        // The unsigned prefix REQUIRES a width; on its own it is not a suffix,
        // and the whole thing is optional, so the cursor goes back.
        if !(eat_either(r, b'b', b'B')
            || eat_either(r, b's', b'S')
            || eat_either(r, b'i', b'I')
            || eat_either(r, b'l', b'L'))
        {
            r.set_cursor(mark);
        }
        return;
    }
    let _ = eat_either(r, b'b', b'B')
        || eat_either(r, b's', b'S')
        || eat_either(r, b'i', b'I')
        || eat_either(r, b'l', b'L');
}

/// `integerLiteral`.
fn integer(r: &mut StringReader) -> Result<(), ReaderError> {
    sign(r);
    skip_whitespace(r);
    if at(r, b'0') {
        r.skip();
        if eat_either(r, b'x', b'X') {
            // Cut: after `0x` a hex numeral is required.
            number_run(r, is_hex)?;
        } else if at(r, b'b') || at(r, b'B') {
            // NO cut here, which is what makes `0b` fall back to zero-as-a-
            // byte while `0b1` is binary one.
            let mark = r.cursor();
            r.skip();
            if number_run(r, is_binary).is_err() {
                r.set_cursor(mark);
            }
        } else if r.can_read() && is_decimal(r.peek()) && r.peek() != b'_' as u16 {
            // `ERROR_LEADING_ZERO_NOT_ALLOWED` — an outright rejection, not a
            // re-read.
            return Err(err());
        }
    } else {
        decimal(r)?;
    }
    integer_suffix(r);
    Ok(())
}

/// `floatExponentPart`.
fn exponent(r: &mut StringReader) -> Result<(), ReaderError> {
    if !eat_either(r, b'e', b'E') {
        return Err(err());
    }
    sign(r);
    decimal(r)
}

fn float_suffix(r: &mut StringReader) -> bool {
    eat_either(r, b'f', b'F') || eat_either(r, b'd', b'D')
}

/// `floatLiteral` — and it needs a `.`, an exponent, or an `f`/`d`, which is
/// what stops it swallowing every integer.
fn float(r: &mut StringReader) -> Result<(), ReaderError> {
    let start = r.cursor();
    sign(r);
    skip_whitespace(r);
    if at(r, b'.') {
        r.skip();
        decimal(r)?;
        let _ = exponent(r);
        float_suffix(r);
        return Ok(());
    }
    if decimal(r).is_err() {
        r.set_cursor(start);
        return Err(err());
    }
    if at(r, b'.') {
        r.skip();
        // The fraction is optional: `1.` is a float.
        let _ = decimal(r);
        let _ = exponent(r);
        float_suffix(r);
        return Ok(());
    }
    if exponent(r).is_ok() {
        float_suffix(r);
        return Ok(());
    }
    if float_suffix(r) {
        return Ok(());
    }
    r.set_cursor(start);
    Err(err())
}

/// `\N{…}`'s name pattern, `[-a-zA-Z0-9 ]+`.
fn is_unicode_name(c: u16) -> bool {
    c == 0x2D
        || c == 0x20
        || (0x30..=0x39).contains(&c)
        || (0x41..=0x5A).contains(&c)
        || (0x61..=0x7A).contains(&c)
}

fn hex_digits(r: &mut StringReader, n: usize) -> Result<(), ReaderError> {
    for _ in 0..n {
        if !r.can_read() || !is_hex(r.peek()) || r.peek() == b'_' as u16 {
            return Err(err());
        }
        r.skip();
    }
    Ok(())
}

/// `stringEscapeSequence`. **`\s` is a space**, not "whitespace".
fn escape(r: &mut StringReader) -> Result<(), ReaderError> {
    if !r.can_read() {
        return Err(err());
    }
    let c = r.read();
    match c as u8 {
        b'b' | b's' | b't' | b'n' | b'f' | b'r' | b'\\' | b'\'' | b'"' => Ok(()),
        b'x' => hex_digits(r, 2),
        b'u' => hex_digits(r, 4),
        b'U' => hex_digits(r, 8),
        b'N' => {
            expect(r, b'{')?;
            let start = r.cursor();
            while r.can_read() && is_unicode_name(r.peek()) {
                r.skip();
            }
            if r.cursor() == start {
                return Err(err());
            }
            expect(r, b'}')
        }
        _ => Err(err()),
    }
}

/// `quotedStringLiteral`.
fn quoted(r: &mut StringReader) -> Result<(), ReaderError> {
    skip_whitespace(r);
    if !r.can_read() || !matches!(r.peek() as u8, b'"' | b'\'') {
        return Err(err());
    }
    let terminator = r.peek();
    r.skip();
    while r.can_read() {
        let c = r.read();
        if c == b'\\' as u16 {
            escape(r)?;
        } else if c == terminator {
            return Ok(());
        }
    }
    Err(ReaderError::ExpectedEndOfQuote)
}

/// brigadier's `readUnquotedString` charset, which is what
/// `UnquotedStringParseRule` delegates to.
fn is_unquoted(c: u16) -> bool {
    (0x30..=0x39).contains(&c)
        || (0x41..=0x5A).contains(&c)
        || (0x61..=0x7A).contains(&c)
        || matches!(c, 0x5F | 0x2D | 0x2E | 0x2B)
}

fn unquoted(r: &mut StringReader) -> Result<(), ReaderError> {
    skip_whitespace(r);
    let start = r.cursor();
    while r.can_read() && is_unquoted(r.peek()) {
        r.skip();
    }
    if r.cursor() == start {
        Err(err())
    } else {
        Ok(())
    }
}

/// `unquotedStringOrBuiltIn` — and the call form 26.x added.
fn unquoted_or_builtin(r: &mut StringReader, depth: usize) -> Result<(), ReaderError> {
    skip_whitespace(r);
    // `isAllowedToStartUnquotedString` is `!canStartNumber`, so a value that
    // begins with a digit or a sign is never an unquoted string.
    //
    // **Redundant here, and kept anyway.** `parse_value_at` only falls through
    // to this rule when the same test has already failed on the same
    // character, so deleting it is a mutation that survives — provably, not by
    // omission. Transcribed because vanilla carries it in this rule rather
    // than relying on its caller.
    if !r.can_read() || can_start_number(r.peek()) {
        return Err(err());
    }
    unquoted(r)?;
    skip_whitespace(r);
    if at(r, b'(') {
        r.skip();
        separated(r, depth, b')', parse_value_at)?;
        expect(r, b')')?;
    }
    Ok(())
}

/// `Term.repeatedWithTrailingSeparator` — zero or more, comma-separated, with
/// an optional trailing comma. The closing delimiter is what stops it.
fn separated(
    r: &mut StringReader,
    depth: usize,
    close: u8,
    item: fn(&mut StringReader, usize) -> Result<(), ReaderError>,
) -> Result<(), ReaderError> {
    loop {
        skip_whitespace(r);
        if at(r, close) {
            return Ok(());
        }
        item(r, depth)?;
        skip_whitespace(r);
        if at(r, b',') {
            r.skip();
            continue;
        }
        return Ok(());
    }
}

fn map_entry(r: &mut StringReader, depth: usize) -> Result<(), ReaderError> {
    skip_whitespace(r);
    // `mapKey` is a quoted OR unquoted string — and unlike a value, an
    // unquoted key may start with a digit, because the key rule does not
    // consult `canStartNumber`.
    if quoted(r).is_err() {
        unquoted(r)?;
    }
    expect(r, b':')?;
    parse_value_at(r, depth)
}

fn integer_item(r: &mut StringReader, _depth: usize) -> Result<(), ReaderError> {
    integer(r)
}

/// The recursion guard. Vanilla's packrat state has its own depth limit; this
/// one exists so a pathological `[[[[…` cannot blow the stack of a client
/// parsing whatever a server put in a chat suggestion.
const MAX_DEPTH: usize = 64;

fn parse_value_at(r: &mut StringReader, depth: usize) -> Result<(), ReaderError> {
    if depth > MAX_DEPTH {
        return Err(err());
    }
    skip_whitespace(r);
    if !r.can_read() {
        return Err(err());
    }
    let c = r.peek();
    // `literal`'s alternatives, in order, each guarded by a positive lookahead
    // and then CUT — so once the first character says which kind it is, there
    // is no falling back to another kind.
    if can_start_number(c) {
        // Float FIRST: it needs a `.`, an exponent or an f/d suffix, so an
        // ordinary integer falls through to the integer rule.
        let mark = r.cursor();
        if float(r).is_ok() {
            return Ok(());
        }
        r.set_cursor(mark);
        return integer(r);
    }
    if matches!(c as u8, b'"' | b'\'') {
        return quoted(r);
    }
    if c == b'{' as u16 {
        r.skip();
        separated(r, depth + 1, b'}', map_entry)?;
        return expect(r, b'}');
    }
    if c == b'[' as u16 {
        r.skip();
        // `arrayPrefix` is UPPERCASE only — `[b;1]` is not a byte array.
        let mark = r.cursor();
        skip_whitespace(r);
        let prefixed = matches!(r.peek() as u8, b'B' | b'L' | b'I') && {
            let after = r.cursor() + 1;
            r.string().get(after).copied() == Some(b';' as u16)
        };
        if prefixed {
            r.skip();
            r.skip();
            separated(r, depth + 1, b']', integer_item)?;
        } else {
            r.set_cursor(mark);
            separated(r, depth + 1, b']', parse_value_at)?;
        }
        return expect(r, b']');
    }
    unquoted_or_builtin(r, depth + 1)
}

/// Parse one SNBT value, validating it.
pub fn parse_value(r: &mut StringReader) -> Result<(), ReaderError> {
    parse_value_at(r, 0)
}

/// `parseCompoundAsArgument` — the value must be a compound.
pub fn parse_compound(r: &mut StringReader) -> Result<(), ReaderError> {
    skip_whitespace(r);
    if !at(r, b'{') {
        return Err(err());
    }
    parse_value(r)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(s: &str) -> bool {
        let units: Vec<u16> = s.encode_utf16().collect();
        let mut r = StringReader::new(&units);
        parse_value(&mut r).is_ok() && r.cursor() == units.len()
    }

    // ── numerals ─────────────────────────────────────────────────────────

    #[test]
    fn zero_b_is_a_byte_and_zero_b_one_is_binary() {
        // No cut after the `b`, so a failed binary numeral BACKTRACKS to the
        // empty-marker alternative and the `b` is taken by the integer suffix.
        // Two readings of the same two characters.
        assert!(ok("0b"));
        assert!(ok("0b1"));
        assert!(ok("0b1010"));
        // …and a `b` that is neither is still an error.
        assert!(!ok("0b2"));
    }

    #[test]
    fn a_leading_zero_is_rejected_outright() {
        // ERROR_LEADING_ZERO_NOT_ALLOWED. Not read as `1`, and not read as `0`
        // with junk after it either.
        assert!(!ok("01"));
        assert!(!ok("007"));
        assert!(ok("0"));
        assert!(ok("0.5"));
    }

    #[test]
    fn an_underscore_separates_digits_but_may_not_end_the_run() {
        assert!(ok("1_000"));
        // Doubled separators are legal, which is unusual enough to be worth
        // not tidying: the rule tests only the FIRST and LAST characters.
        assert!(ok("1__2"));
        // A TRAILING underscore fails the value outright, because a token
        // starting with a digit can only be a number.
        assert!(!ok("1_"));
        // A LEADING one does not — and that asymmetry is the interesting
        // part. The numeral rule rejects `_1` just the same, and the value
        // rule then falls through to the unquoted-string alternative, which
        // takes it. So `_1` is a STRING. This witness asserted it was invalid
        // and was conflating "not a number" with "not a value".
        assert!(ok("_1"));
        let units: Vec<u16> = "_1".encode_utf16().collect();
        let mut r = StringReader::new(&units);
        assert!(number_run(&mut r, is_decimal).is_err(), "not a numeral");
    }

    #[test]
    fn hex_and_binary_take_their_own_digits() {
        assert!(ok("0x1F"));
        assert!(ok("0xdead_beef"));
        assert!(!ok("0xG"));
        assert!(!ok("0x"));
    }

    #[test]
    fn an_integer_suffix_may_be_unsigned_but_the_u_needs_a_width() {
        assert!(ok("1b"));
        assert!(ok("1S"));
        assert!(ok("1ub"));
        assert!(ok("1UL"));
        // A bare `u` is not a suffix, so it is left for whatever follows —
        // here, nothing, which makes the whole value trail.
        assert!(!ok("1u"));
    }

    #[test]
    fn a_float_needs_a_point_an_exponent_or_a_suffix() {
        assert!(ok("1.0"));
        assert!(ok("1."));
        assert!(ok(".5"));
        assert!(ok("1e3"));
        assert!(ok("1E-3"));
        assert!(ok("1f"));
        assert!(ok("1.5d"));
        // …and a bare integer is an INTEGER, which is what the ordering buys.
        assert!(ok("1"));
    }

    // ── strings ──────────────────────────────────────────────────────────

    #[test]
    fn the_escape_table_is_the_26x_one_and_backslash_s_is_a_space() {
        for s in [r#""a\sb""#, r#""a\nb""#, r#""a\\b""#, r#""a\"b""#, r#"'a\'b'"#] {
            assert!(ok(s), "{s}");
        }
        assert!(ok(r#""\x41""#));
        assert!(ok(r#""A""#));
        assert!(ok(r#""\U0001F600""#));
        assert!(ok(r#""\N{LATIN SMALL LETTER A}""#));
        // An escape outside the table is an error, where M121's extent walk
        // accepted anything after a backslash.
        assert!(!ok(r#""a\qb""#));
        // …and a short hex escape is too — each length checked, because a
        // single fixture only covers the one width it uses.
        assert!(!ok(r#""\x4""#));
        assert!(!ok(r#""\u41""#));
        assert!(!ok(r#""\U0001""#));
    }

    #[test]
    fn an_unquoted_string_may_not_start_like_a_number() {
        // `isAllowedToStartUnquotedString` is `!canStartNumber`.
        assert!(ok("abc"));
        assert!(ok("minecraft.stone"));
        assert!(!ok("1abc"), "reads as an integer and then trails");
    }

    #[test]
    fn an_unquoted_string_may_be_CALLED() {
        // 26.x's builtin-operation form. A grammar without it reads `bool` and
        // then chokes on the parenthesis.
        assert!(ok("bool(1)"));
        assert!(ok("bool(1,2)"));
        assert!(!ok("bool(1"));
    }

    // ── structure ────────────────────────────────────────────────────────

    #[test]
    fn a_trailing_comma_is_legal_and_so_is_an_empty_group() {
        // `Term.repeatedWithTrailingSeparator`.
        assert!(ok("{a:1,}"));
        assert!(ok("[1,]"));
        assert!(ok("{}"));
        assert!(ok("[]"));
    }

    #[test]
    fn whitespace_is_skipped_before_every_token() {
        // Each terminal rule opens with `skipWhitespace`, so this is as valid
        // as the tight form. Skipping only between entries rejects a space
        // before a colon.
        assert!(ok("{ a : 1 , b : 2 }"));
        assert!(ok("[ 1 , 2 ]"));
    }

    #[test]
    fn an_array_prefix_is_uppercase_only() {
        assert!(ok("[B;1b,2b]"));
        assert!(ok("[I;1,2]"));
        assert!(ok("[L;1l]"));
        // `[b;1]` is not a byte array — and it is not a valid list either,
        // because `b;1` is not one value.
        assert!(!ok("[b;1]"));
    }

    #[test]
    fn a_map_key_may_start_with_a_digit_where_a_value_may_not() {
        // `mapKey` does not consult `canStartNumber`; the value rule does.
        assert!(ok("{1a:2}"));
        assert!(ok("{\"a b\":2}"));
    }

    // ── the validation M121 deferred ─────────────────────────────────────

    #[test]
    fn the_malformed_compounds_M121_accepted_are_now_errors() {
        // M121's extent walk measured these and moved on; its own test said
        // so, and this is the inversion. `{a:}` has no value and `{:1}` has no
        // key.
        assert!(!ok("{a:}"));
        assert!(!ok("{:1}"));
        assert!(!ok("{a}"));
        assert!(!ok("[1,,2]"));
    }

    #[test]
    fn a_brace_inside_a_string_still_does_not_close_the_compound() {
        // M121's load-bearing case, which the grammar gets for free because
        // the string rule consumes it.
        assert!(ok(r#"{a:"}"}"#));
        assert!(ok(r#"{a:"]["}"#));
    }

    #[test]
    fn a_value_stops_at_the_argument_separator() {
        // The command layer needs the extent as well as the validity, and the
        // grammar gives both: the parse ends where the value does.
        let units: Vec<u16> = "{a:1} rest".encode_utf16().collect();
        let mut r = StringReader::new(&units);
        assert!(parse_value(&mut r).is_ok());
        assert_eq!(r.cursor(), 5);
    }

    #[test]
    fn a_compound_is_required_where_only_a_compound_is_allowed() {
        let units: Vec<u16> = "[1]".encode_utf16().collect();
        assert!(parse_compound(&mut StringReader::new(&units)).is_err());
        let units: Vec<u16> = "{a:1}".encode_utf16().collect();
        assert!(parse_compound(&mut StringReader::new(&units)).is_ok());
    }

    #[test]
    fn nesting_is_bounded_rather_than_recursing_until_the_stack_gives_out() {
        // Not vanilla's limit — vanilla's packrat state has its own — but a
        // client parsing whatever a server put in a suggestion should not be
        // crashable by `[[[[…`.
        // A WELL-FORMED deep value is what makes the guard observable: an
        // unterminated `[[[[…` fails on the missing brackets whether or not
        // the guard is there, so a fixture of those cannot see it.
        let n = MAX_DEPTH + 5;
        let deep = "[".repeat(n) + &"]".repeat(n);
        assert!(!ok(&deep));
        // …and just inside the limit still parses.
        let shallow = "[".repeat(MAX_DEPTH - 1) + &"]".repeat(MAX_DEPTH - 1);
        assert!(ok(&shallow));
    }

    #[test]
    fn range_is_still_not_checked_and_that_is_stated() {
        // Vanilla additionally parses each numeral and reports
        // ERROR_NUMBER_PARSE_FAILURE on overflow. Recorded as a test so the
        // day someone adds it, this fails and asks to be updated.
        assert!(ok("999999999999b"));
    }
}
