//! Test-only helpers for pinning this crate's wire layouts to the Java mod
//! sources that mirror them (`ingame-mod/src/main/java/dev/lewlone/ewohud/`).
//!
//! Both shared buffers (`EwoHudData`, `EwoModuleData`) are written by one
//! language and read by the other, byte for byte. A schema number alone cannot
//! catch a one-sided edit to an offset, so the tests read the Java *source* and
//! compare every constant. The sources are always present in a checkout, so a
//! missing file is a hard failure, never a skip.

use std::collections::BTreeMap;

/// Source text of a file under the Java mod's `dev/lewlone/ewohud/` package.
pub(crate) fn read_java(file: &str) -> String {
    let path = format!(
        "{}/../../ingame-mod/src/main/java/dev/lewlone/ewohud/{file}",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("cannot read {path}: {e}"))
}

/// Strip a trailing `//` comment and surrounding whitespace.
fn code_of(line: &str) -> &str {
    line.split("//").next().unwrap_or("").trim()
}

/// Evaluate the two constant forms the mirrors use: a decimal literal and
/// `a << b`. Anything else is `None` and the constant is left out of the map,
/// so a lookup of it panics instead of comparing a wrong number.
fn eval(expr: &str) -> Option<i64> {
    let expr = expr.trim();
    if let Some((a, b)) = expr.split_once("<<") {
        return Some(eval(a)? << eval(b)?);
    }
    expr.parse().ok()
}

/// Every `static final int NAME = <literal | a << b>;` in a Java source.
/// Doc-comment lines are skipped; trailing `//` comments are ignored.
pub(crate) fn int_constants(src: &str) -> BTreeMap<String, i64> {
    let mut out = BTreeMap::new();
    for line in src.lines() {
        let line = code_of(line);
        if line.starts_with('*') || line.starts_with("/*") {
            continue;
        }
        let Some((_, rest)) = line.split_once("static final int ") else {
            continue;
        };
        let Some((name, expr)) = rest.split_once('=') else {
            continue;
        };
        let Some(expr) = expr.trim().strip_suffix(';') else {
            continue;
        };
        if let Some(v) = eval(expr) {
            out.insert(name.trim().to_string(), v);
        }
    }
    out
}

/// The value of constant `name`, panicking with a pointed message if the Java
/// source no longer defines it in a form [`int_constants`] understands.
pub(crate) fn get(consts: &BTreeMap<String, i64>, name: &str) -> i64 {
    *consts
        .get(name)
        .unwrap_or_else(|| panic!("Java constant {name} is missing (or no longer a literal)"))
}

/// The text from the first occurrence of `from` up to (not including) the next
/// occurrence of `to` after it.
pub(crate) fn between<'a>(src: &'a str, from: &str, to: &str) -> &'a str {
    let start = src
        .find(from)
        .unwrap_or_else(|| panic!("source no longer contains {from:?}"));
    let rest = &src[start..];
    let end = rest[from.len()..]
        .find(to)
        .unwrap_or_else(|| panic!("no {to:?} after {from:?}"));
    &rest[..from.len() + end]
}

/// Field accesses in `region`: every call of one of `calls` (`(needle, kind)`
/// pairs) whose first argument — after `skip` leading characters of the call —
/// is `base` or `base + N`. Returns `(kind, N)` in source order. Used to
/// compare the per-record layouts, which the sources spell as bare `rec + 8`
/// style literals rather than named constants.
pub(crate) fn accesses(
    region: &str,
    base: &str,
    calls: &[(&str, &'static str)],
) -> Vec<(&'static str, i64)> {
    let mut found: Vec<(usize, &'static str, i64)> = Vec::new();
    for &(needle, kind) in calls {
        let mut from = 0;
        while let Some(pos) = region[from..].find(needle) {
            let at = from + pos + needle.len();
            from = at;
            let arg = region[at..].trim_start();
            let Some(after_base) = arg.strip_prefix(base) else {
                continue;
            };
            let after_base = after_base.trim_start();
            let offset = if let Some(n) = after_base.strip_prefix('+') {
                let digits: String = n
                    .trim_start()
                    .chars()
                    .take_while(|c| c.is_ascii_digit())
                    .collect();
                match digits.parse() {
                    Ok(v) => v,
                    Err(_) => continue,
                }
            } else if after_base.starts_with([',', ')']) {
                0
            } else {
                continue;
            };
            found.push((from, kind, offset));
        }
    }
    found.sort();
    found.into_iter().map(|(_, k, o)| (k, o)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_literals_and_shifts_and_skips_the_rest() {
        let src = "\
    public static final int SCHEMA_VERSION = 10;
    private static final int OFF_X = 24;        // trailing comment
    private static final int FLAG_OVERLAY = 1 << 4; // bit
    /** public static final int IN_DOC = 1; */
     * private static final int IN_DOC_STAR = 2;
    private static final int COMPUTED = OFF_X + 4;
    private static final float NOT_AN_INT = 1;
";
        let c = int_constants(src);
        assert_eq!(get(&c, "SCHEMA_VERSION"), 10);
        assert_eq!(get(&c, "OFF_X"), 24);
        assert_eq!(get(&c, "FLAG_OVERLAY"), 16);
        assert_eq!(c.len(), 3, "{c:?}");
    }

    #[test]
    fn accesses_read_base_plus_offset_in_source_order() {
        let region = "b.putInt(rec, a); b.putFloat(rec + 4, x); putString(b, rec + 12, CAP, s);
                      b.putInt(other + 8, y);";
        let got = accesses(
            region,
            "rec",
            &[
                ("putInt(", "i32"),
                ("putFloat(", "f32"),
                ("putString(b, ", "str"),
            ],
        );
        assert_eq!(got, vec![("i32", 0), ("f32", 4), ("str", 12)]);
    }
}
