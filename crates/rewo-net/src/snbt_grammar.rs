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

/// Consume one character, with `TerminalCharacters`' whitespace rule.
fn eat_char(r: &mut StringReader, c: u8) -> bool {
    eat_either(r, c, c)
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

/// The next non-whitespace unit, without moving.
fn peek_nonspace(r: &StringReader) -> Option<u16> {
    let mut i = r.cursor();
    let s = r.string();
    while i < s.len() && matches!(s[i], 0x20 | 0x09 | 0x0A | 0x0B | 0x0C | 0x0D) {
        i += 1;
    }
    s.get(i).copied()
}

/// `StringReaderTerms.TerminalCharacters` — which opens with
/// `input.skipWhitespace()` and, when it does not match, is unwound by whatever
/// `Term.optional` / `Term.alternative` wraps it. So the whitespace is consumed
/// on a match and given back on a miss: `1 u b` is an unsigned byte, and
/// `{a:1 }` still closes.
fn eat_either(r: &mut StringReader, lower: u8, upper: u8) -> bool {
    let mark = r.cursor();
    skip_whitespace(r);
    if r.can_read() && (r.peek() == lower as u16 || r.peek() == upper as u16) {
        r.skip();
        true
    } else {
        r.set_cursor(mark);
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
/// Returns the run with its separators removed — `cleanAndAppend`'s half of the
/// job, done here so the range check downstream has digits it can convert.
fn number_run(r: &mut StringReader, accept: fn(u16) -> bool) -> Result<String, ReaderError> {
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
    Ok(r.string()[start..r.cursor()]
        .iter()
        .filter(|&&c| c != b'_' as u16)
        .map(|&c| c as u8 as char)
        .collect())
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

fn decimal(r: &mut StringReader) -> Result<String, ReaderError> {
    number_run(r, is_decimal)
}

/// `sign?` — returns whether it was MINUS. `Sign.PLUS.append` writes nothing,
/// so a leading `+` reaches the conversion as no sign at all.
fn sign(r: &mut StringReader) -> bool {
    skip_whitespace(r);
    if at(r, b'+') {
        r.skip();
        false
    } else if at(r, b'-') {
        r.skip();
        true
    } else {
        false
    }
}

/// `SnbtGrammar.Base`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Base {
    Binary,
    Decimal,
    Hex,
}

/// `SnbtGrammar.TypeSuffix`, restricted to the four the integer rule can name.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Width {
    Byte,
    Short,
    Int,
    Long,
}

/// `SnbtGrammar.SignedPrefix`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Signedness {
    Signed,
    Unsigned,
}

/// `SnbtGrammar.IntegerLiteral` — kept whole, because none of its four fields
/// alone decides the range.
#[derive(Clone, Debug)]
struct IntegerLiteral {
    negative: bool,
    base: Base,
    /// Separators already removed.
    digits: String,
    declared: Option<Signedness>,
    width: Option<Width>,
}

impl IntegerLiteral {
    /// `signedOrDefault` — and this is the rule that makes `0xFF` a legal byte
    /// while `255b` is not: an explicit `u`/`s` wins, and otherwise **binary
    /// and hex default to UNSIGNED where decimal defaults to SIGNED**.
    fn signedness(&self) -> Signedness {
        self.declared.unwrap_or(match self.base {
            Base::Binary | Base::Hex => Signedness::Unsigned,
            Base::Decimal => Signedness::Signed,
        })
    }

    /// `IntegerLiteral.create` — the range check, against a width the caller
    /// supplies (an array prefix overrides the literal's own).
    fn check(&self, width: Width) -> Result<(), ReaderError> {
        let signed = self.signedness() == Signedness::Signed;
        if !signed && self.negative {
            // `ERROR_EXPECTED_NON_NEGATIVE_NUMBER`. `-0xF` is an error, because
            // hex is unsigned by default — the sign and the base disagree and
            // the base wins.
            return Err(err());
        }
        let radix = match self.base {
            Base::Binary => 2,
            Base::Decimal => 10,
            Base::Hex => 16,
        };
        let mut magnitude: u128 = 0;
        for c in self.digits.chars() {
            let d = c.to_digit(radix).ok_or_else(err)? as u128;
            // A numeral long enough to overflow a u128 is far outside every
            // width, so saturating here cannot turn a reject into an accept.
            magnitude = magnitude.saturating_mul(radix as u128).saturating_add(d);
            if magnitude > u64::MAX as u128 {
                magnitude = u64::MAX as u128 + 1;
            }
        }
        let ok = if signed {
            let (min, max) = match width {
                Width::Byte => (i8::MIN as i128, i8::MAX as i128),
                Width::Short => (i16::MIN as i128, i16::MAX as i128),
                Width::Int => (i32::MIN as i128, i32::MAX as i128),
                Width::Long => (i64::MIN as i128, i64::MAX as i128),
            };
            let value = if self.negative {
                -(magnitude as i128)
            } else {
                magnitude as i128
            };
            value >= min && value <= max
        } else {
            let max = match width {
                Width::Byte => u8::MAX as u128,
                Width::Short => u16::MAX as u128,
                Width::Int => u32::MAX as u128,
                Width::Long => u64::MAX as u128,
            };
            magnitude <= max
        };
        if ok {
            Ok(())
        } else {
            Err(err())
        }
    }
}

/// `integerSuffix?`.
///
/// **`s` is both the SIGNED prefix and the SHORT width**, and the prefix
/// alternative is tried first — so `1s` is a short (the prefix branch needs a
/// width after it, finds none, and backtracks) while `1sb` is a *signed byte*.
fn integer_suffix(r: &mut StringReader) -> (Option<Signedness>, Option<Width>) {
    for (letter, upper, signedness) in [
        (b'u', b'U', Signedness::Unsigned),
        (b's', b'S', Signedness::Signed),
    ] {
        let mark = r.cursor();
        if eat_either(r, letter, upper) {
            if let Some(w) = eat_width(r) {
                return (Some(signedness), Some(w));
            }
            // A prefix on its own is not a suffix, and the whole thing is
            // optional, so the cursor goes back and the bare alternatives run.
            r.set_cursor(mark);
        }
    }
    (None, eat_width(r))
}

fn eat_width(r: &mut StringReader) -> Option<Width> {
    if eat_either(r, b'b', b'B') {
        Some(Width::Byte)
    } else if eat_either(r, b's', b'S') {
        Some(Width::Short)
    } else if eat_either(r, b'i', b'I') {
        Some(Width::Int)
    } else if eat_either(r, b'l', b'L') {
        Some(Width::Long)
    } else {
        None
    }
}

/// `integerLiteral`.
fn integer_literal(r: &mut StringReader) -> Result<IntegerLiteral, ReaderError> {
    let negative = sign(r);
    skip_whitespace(r);
    let (base, digits) = if at(r, b'0') {
        r.skip();
        let after_zero = r.cursor();
        if eat_either(r, b'x', b'X') {
            // Cut: after `0x` a hex numeral is required.
            (Base::Hex, number_run(r, is_hex)?)
        } else {
            // NO cut after the `b`, which is what makes `0b` fall back to
            // zero-as-a-byte while `0b1` is binary one.
            let mut binary = None;
            if eat_either(r, b'b', b'B') {
                match number_run(r, is_binary) {
                    Ok(d) => binary = Some(d),
                    Err(_) => r.set_cursor(after_zero),
                }
            }
            match binary {
                Some(d) => (Base::Binary, d),
                None if peek_nonspace(r).is_some_and(|c| is_decimal(c) && c != b'_' as u16) => {
                    // `ERROR_LEADING_ZERO_NOT_ALLOWED` — an outright
                    // rejection, not a re-read.
                    return Err(err());
                }
                None => (Base::Decimal, "0".to_string()),
            }
        }
    } else {
        (Base::Decimal, decimal(r)?)
    };
    let (declared, width) = integer_suffix(r);
    Ok(IntegerLiteral {
        negative,
        base,
        digits,
        declared,
        width,
    })
}

/// A standalone integer value. The default width is INT
/// (`requireNonNullElse(suffix.type, TypeSuffix.INT)`).
fn integer(r: &mut StringReader) -> Result<(), ReaderError> {
    let literal = integer_literal(r)?;
    let width = literal.width.unwrap_or(Width::Int);
    literal.check(width)
}

/// `floatExponentPart`, appended to `out` in the form `createFloat` builds.
fn exponent(r: &mut StringReader, out: &mut String) -> Result<(), ReaderError> {
    if !eat_either(r, b'e', b'E') {
        return Err(err());
    }
    let negative = sign(r);
    let digits = decimal(r)?;
    out.push('e');
    if negative {
        out.push('-');
    }
    out.push_str(&digits);
    Ok(())
}

/// `floatTypeSuffix?` — `f` is FLOAT, `d` is DOUBLE, absent is DOUBLE.
fn float_suffix(r: &mut StringReader) -> Option<bool> {
    if eat_either(r, b'f', b'F') {
        Some(true)
    } else if eat_either(r, b'd', b'D') {
        Some(false)
    } else {
        None
    }
}

/// `convertFloat` / `convertDouble` — the whole of which is a finiteness test.
/// `1e400` is not a parse failure in Java either; it is `Infinity`, and it is
/// `ERROR_INFINITY_NOT_ALLOWED` that rejects it.
fn finite(text: &str, single: bool) -> Result<(), ReaderError> {
    let ok = if single {
        text.parse::<f32>().map(|v| v.is_finite())
    } else {
        text.parse::<f64>().map(|v| v.is_finite())
    };
    match ok {
        Ok(true) => Ok(()),
        _ => Err(err()),
    }
}

/// `floatLiteral` — and it needs a `.`, an exponent, or an `f`/`d`, which is
/// what stops it swallowing every integer.
fn float(r: &mut StringReader) -> Result<(), ReaderError> {
    let start = r.cursor();
    let mut text = String::new();
    if sign(r) {
        text.push('-');
    }
    skip_whitespace(r);
    let mut finish = |r: &mut StringReader, text: &mut String| -> Result<(), ReaderError> {
        let single = float_suffix(r).unwrap_or(false);
        finite(text, single)
    };
    if eat_char(r, b'.') {
        text.push('.');
        text.push_str(&decimal(r)?);
        let _ = exponent(r, &mut text);
        return finish(r, &mut text);
    }
    let whole = match decimal(r) {
        Ok(d) => d,
        Err(_) => {
            r.set_cursor(start);
            return Err(err());
        }
    };
    text.push_str(&whole);
    if eat_char(r, b'.') {
        // The fraction is optional: `1.` is a float.
        if let Ok(f) = decimal(r) {
            text.push('.');
            text.push_str(&f);
        }
        let _ = exponent(r, &mut text);
        return finish(r, &mut text);
    }
    if exponent(r, &mut text).is_ok() {
        return finish(r, &mut text);
    }
    match float_suffix(r) {
        Some(single) => finite(&text, single),
        None => {
            r.set_cursor(start);
            Err(err())
        }
    }
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
        separated(r, depth, b')', &mut parse_value_at)?;
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
    item: &mut dyn FnMut(&mut StringReader, usize) -> Result<(), ReaderError>,
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

/// `ArrayPrefix.buildNumber` — the element's width, checked against the
/// array's.
///
/// `isAllowed` is a **narrowing** rule, not a widening one: `[B; …]` admits
/// only BYTE, `[I; …]` admits INT/BYTE/SHORT, and `[L; …]` admits
/// LONG/BYTE/SHORT/INT. So `[I; 1b]` is fine and `[B; 1i]` is
/// `ERROR_INVALID_ARRAY_ELEMENT_TYPE`. An element that declares no width takes
/// the array's own.
fn array_item(r: &mut StringReader, prefix: Width) -> Result<(), ReaderError> {
    let literal = integer_literal(r)?;
    let width = match literal.width {
        None => prefix,
        Some(w) if array_allows(prefix, w) => w,
        Some(_) => return Err(err()),
    };
    literal.check(width)
}

fn array_allows(prefix: Width, width: Width) -> bool {
    match prefix {
        Width::Byte => width == Width::Byte,
        Width::Int => matches!(width, Width::Int | Width::Byte | Width::Short),
        Width::Long => matches!(width, Width::Long | Width::Byte | Width::Short | Width::Int),
        // There is no short-array prefix.
        Width::Short => false,
    }
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
        separated(r, depth + 1, b'}', &mut map_entry)?;
        return expect(r, b'}');
    }
    if c == b'[' as u16 {
        r.skip();
        // `arrayPrefix` is UPPERCASE only — `[b;1]` is not a byte array.
        let mark = r.cursor();
        skip_whitespace(r);
        let prefix = match r.peek() as u8 {
            b'B' => Some(Width::Byte),
            b'I' => Some(Width::Int),
            b'L' => Some(Width::Long),
            _ => None,
        }
        .filter(|_| {
            let after = r.cursor() + 1;
            r.string().get(after).copied() == Some(b';' as u16)
        });
        if let Some(prefix) = prefix {
            r.skip();
            r.skip();
            separated(r, depth + 1, b']', &mut |r, _| array_item(r, prefix))?;
        } else {
            r.set_cursor(mark);
            separated(r, depth + 1, b']', &mut parse_value_at)?;
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
    fn range_is_checked_now_which_is_what_m122_asked_for() {
        // M122 recorded this acceptance as a test so that the day someone
        // added the check, it would fail and ask to be updated. This is that
        // update: the same literal is now ERROR_NUMBER_PARSE_FAILURE.
        assert!(!ok("999999999999b"));
    }

    #[test]
    fn the_base_decides_the_signedness_and_therefore_the_range() {
        // `signedOrDefault`: an explicit u/s wins, and otherwise BINARY and HEX
        // default to UNSIGNED where DECIMAL defaults to SIGNED. The default
        // width is INT either way, so the same value is in range in one base
        // and out of it in another.
        assert!(ok("0xFFFFFFFF"));
        assert!(ok("0b11111111111111111111111111111111"));
        assert!(!ok("4294967295"));
        assert!(ok("2147483647"));
        assert!(!ok("2147483648"));
        assert!(ok("2147483648L"));
    }

    #[test]
    fn an_unsigned_literal_may_not_be_negative() {
        // ERROR_EXPECTED_NON_NEGATIVE_NUMBER. The sign and the base disagree
        // and the base wins, so `-0xF` is an error while `-15` is fine.
        assert!(!ok("-0xF"));
        assert!(!ok("-0b1"));
        assert!(ok("-15"));
        // An explicit `s` rescues the hex one.
        assert!(ok("-0xFsi"));
    }

    #[test]
    fn s_is_both_the_signed_prefix_and_the_short_width() {
        // The prefix alternative is tried first, needs a width after it, and
        // backtracks when there is none — so a bare `s` reaches the SHORT
        // alternative while `sb` is a signed byte.
        assert!(ok("1s"));
        assert!(ok("1sb"));
        assert!(ok("1ss"));
        assert!(!ok("200sb"));
        assert!(ok("200ub"));
        assert!(!ok("256ub"));
        // A prefix with no width is not a suffix at all, so the `u` is left
        // over and the value does not consume its input.
        assert!(!ok("1u"));
    }

    #[test]
    fn every_width_gets_its_own_two_sided_range() {
        assert!(ok("127b") && !ok("128b"));
        assert!(ok("-128b") && !ok("-129b"));
        assert!(ok("255ub") && !ok("256ub"));
        assert!(ok("32767s") && !ok("32768s"));
        assert!(ok("-32768s") && !ok("-32769s"));
        assert!(ok("65535us") && !ok("65536us"));
        assert!(ok("9223372036854775807L"));
        assert!(!ok("9223372036854775808L"));
        assert!(ok("-9223372036854775808L"));
        assert!(ok("18446744073709551615uL"));
        assert!(!ok("18446744073709551616uL"));
        // A numeral far past every width must still be a clean reject rather
        // than an overflow in the checker.
        assert!(!ok(&"9".repeat(400)));
    }

    #[test]
    fn the_separators_come_out_before_the_conversion() {
        // `cleanupDigits` removes `_` and only then parses, so a separator
        // cannot push a value out of range.
        assert!(ok("1_2_7b"));
        assert!(!ok("1_2_8b"));
    }

    #[test]
    fn a_float_is_rejected_for_being_infinite_rather_than_unparseable() {
        // ERROR_INFINITY_NOT_ALLOWED. Java's parseDouble("1e400") does not
        // throw; it returns Infinity, and it is the finiteness test that
        // rejects it.
        assert!(!ok("1e400"));
        assert!(!ok("1e400f"));
        // The same digits are finite as a double and infinite as a float,
        // which is the only thing the f/d suffix decides here.
        assert!(ok("1e40"));
        assert!(ok("1e40d"));
        assert!(!ok("1e40f"));
        assert!(ok("3.4e38f"));
        assert!(!ok("3.5e38f"));
        assert!(ok("-1.5e-3f"));
    }

    #[test]
    fn an_array_element_may_narrow_its_width_but_not_widen_it() {
        // `ArrayPrefix.isAllowed`: B admits only BYTE, I admits INT/BYTE/SHORT,
        // L admits those plus LONG.
        assert!(ok("[I;1b]"));
        assert!(ok("[I;1s]"));
        assert!(!ok("[I;1L]"));
        assert!(ok("[L;1i]"));
        assert!(!ok("[B;1i]"));
        assert!(ok("[B;1b]"));
    }

    #[test]
    fn an_undeclared_array_element_takes_the_arrays_width_and_its_own_base() {
        // The sharpest pair in the rule: same number, different base. `255` is
        // decimal and therefore signed, so it does not fit a byte; `0xFF` is
        // hex and therefore unsigned, so it does.
        assert!(ok("[B;1]"));
        assert!(!ok("[B;255]"));
        assert!(ok("[B;0xFF]"));
        assert!(!ok("[B;0x1FF]"));
        assert!(ok("[I;2147483647]"));
        assert!(!ok("[I;2147483648]"));
    }

    #[test]
    fn a_terminal_skips_whitespace_before_it_and_gives_it_back_on_a_miss() {
        // `StringReaderTerms.TerminalCharacters` opens with skipWhitespace, so
        // the pieces of a numeral may be spaced apart — absurd, and what the
        // terminals do.
        assert!(ok("{a:1 u b}"));
        assert!(ok("{a:0 x FF}"));
        assert!(ok("{a:1 e 5}"));
        // …and a miss must not eat the whitespace. This is observable at the
        // top level, and it matters: the dispatcher splits the rest of the
        // command on the space after an argument, so a value that swallowed
        // its own trailing space would take the next word's separator with it.
        let stop = |s: &str| {
            let units: Vec<u16> = s.encode_utf16().collect();
            let mut r = StringReader::new(&units);
            parse_value(&mut r).map(|()| r.cursor())
        };
        assert_eq!(stop("1 "), Ok(1));
        assert_eq!(stop("1 b"), Ok(3));
        assert!(ok("{a:1 }"));
        assert!(ok("[1 , 2 ]"));
        // The leading-zero rule looks past whitespace too.
        assert!(!ok("{a:0 5}"));
    }

    #[test]
    fn a_plus_sign_is_not_a_minus_sign() {
        // `Sign.PLUS.append` writes nothing, so `+` reaches the conversion as
        // no sign at all — and cannot rescue an out-of-range magnitude.
        assert!(ok("+5b"));
        assert!(!ok("+300b"));
    }
}
