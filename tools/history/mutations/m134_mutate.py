"""M134's mutation harness — the parse-error messages under the command field.

Verdicts come from the EXIT CODE of the check, never from a substring of its
output (M109's lesson). Every battery carries a NO-OP CONTROL expected to
SURVIVE and opens with a BASELINE run; a battery whose baseline is red or whose
control dies is measuring a broken instrument.

The original is restored in a `finally` AND its mtime is bumped, because cargo
keys its rebuild on mtime and a restore that preserved the older one made a
later run silently grade the mutated binary (M92's harness bug).

    python tools/m134_mutate.py <battery>
"""

import os
import subprocess
import sys
import time

ROOT = "."


def run(cmd, timeout):
    env = dict(os.environ)
    try:
        p = subprocess.run(cmd, shell=True, cwd=ROOT, capture_output=True,
                           timeout=timeout, env=env)
        return p.returncode, (p.stdout + p.stderr).decode("utf-8", "replace")
    except subprocess.TimeoutExpired:
        return 124, "<timed out>"


def write(path, data):
    with open(path, "wb") as f:
        f.write(data)
    # Bump the mtime past whatever cargo last saw.
    now = time.time() + 1
    os.utime(path, (now, now))


def battery(name, check, timeout, mutations):
    print(f"=== {name} ===")
    print(f"    check: {check}")
    code, _ = run(check, timeout)
    if code != 0:
        print(f"    ABORT: the check is already red (exit {code}). "
              f"Every verdict below would read KILLED.")
        return 1
    print("    baseline green")

    bad = 0
    for path, old, new, expect, why in mutations:
        full = os.path.join(ROOT, path)
        orig = open(full, "rb").read()
        n = orig.decode("utf-8").count(old)
        if n != 1:
            print(f"  ?? {why}\n     anchor matched {n} times, not 1 — SKIPPED")
            bad += 1
            continue
        try:
            write(full, orig.decode("utf-8").replace(old, new, 1).encode("utf-8"))
            code, _ = run(check, timeout)
        finally:
            write(full, orig)
        got = "KILLED" if code != 0 else "SURVIVED"
        ok = got == expect
        if not ok:
            bad += 1
        print(f"  {'ok ' if ok else 'BAD'} {got:9s} (want {expect:9s})  {why}")
    print(f"    {'ALL AS EXPECTED' if bad == 0 else str(bad) + ' UNEXPECTED'}")
    return bad


ERRS = "crates/rewo-net/src/command_errors.rs"
FMT = "crates/rewo-net/src/command_format.rs"
DISP = "crates/rewo-net/src/dispatcher.rs"
LIVE = "crates/rewo-app/src/live_cmd.rs"

NET = "cargo test -q -p rewo-net --lib"
APP = "cargo test -q -p rewo-app --bins"

BATTERIES = {
    # ── the literals and getContext ──────────────────────────────────────
    "a": (
        "BuiltInExceptions' literals + getContext",
        NET,
        900,
        [
            (ERRS, "pub const CONTEXT_AMOUNT: usize = 10;",
                   "pub const CONTEXT_AMOUNT: usize = 10; // no-op control",
                   "SURVIVED", "NO-OP CONTROL: a comment"),
            (ERRS, "pub const CONTEXT_AMOUNT: usize = 10;",
                   "pub const CONTEXT_AMOUNT: usize = 12;",
                   "KILLED", "the excerpt window is ten units, not twelve"),
            (ERRS, 'pub const HERE: &str = "<--[HERE]";',
                   'pub const HERE: &str = "<-[HERE]";',
                   "KILLED", "the marker is brigadier's literal, verbatim"),
            (ERRS, "    if cursor > CONTEXT_AMOUNT {",
                   "    if cursor >= CONTEXT_AMOUNT {",
                   "KILLED", "`cursor > 10` is strict: exactly ten is not elided"),
            (ERRS, "    let cursor = cursor.min(input.len());",
                   "    let cursor = cursor;",
                   "KILLED", "a cursor past the end is clamped, not trusted"),
            (ERRS, 'let cmp = if *too_high { "more" } else { "less" };',
                   'let cmp = if *too_high { "less" } else { "more" };',
                   "KILLED", "too-high says `more than`, too-low `less than`"),
            (ERRS, 'format!("{} must not be {cmp} than {bound}, found {found}", kind.name())',
                   'format!("{} must not be {cmp} than {found}, found {bound}", kind.name())',
                   "KILLED", "the BOUND is printed first and the value second"),
            (ERRS, 'ReaderError::InvalidBool(v) => {\n            format!("Invalid bool, expected true or false but found \'{v}\'")\n        }',
                   'ReaderError::InvalidBool(v) => {\n            format!("Invalid boolean, expected true or false but found \'{v}\'")\n        }',
                   "KILLED", "the literals are the decompile's text verbatim"),
            (ERRS, "        ReaderError::LiteralIncorrect | ReaderError::UnknownArgumentType => return None,",
                   '        ReaderError::LiteralIncorrect | ReaderError::UnknownArgumentType => "Incorrect argument for command".to_string(),',
                   "KILLED", "a type with no vanilla literal answers None, not a guess"),
        ],
    ),
    # ── the cursor each exception keeps ──────────────────────────────────
    "b": (
        "the cursor createWithContext captured",
        NET,
        900,
        [
            (DISP, "            errors.push(ParseError {",
                   "            // no-op control\n            errors.push(ParseError {",
                   "SURVIVED", "NO-OP CONTROL: a comment"),
            (DISP, """            errors.push(ParseError {
                node: child_index,
                error: e,
                cursor: reader.cursor(),
            });""",
                   """            errors.push(ParseError {
                node: child_index,
                error: e,
                cursor,
            });""",
                   "KILLED", "the cursor is the FAILING read's, not the retry rewind's"),
            (DISP, """                if let Some(e) = range_error(NumKind::Integer, v, *min, *max, i64::to_string) {
                    reader.set_cursor(start);
                    return Err(e);
                }""",
                   """                if let Some(e) = range_error(NumKind::Integer, v, *min, *max, i64::to_string) {
                    return Err(e);
                }""",
                   "KILLED", "a range error rewinds BEFORE it throws"),
            (DISP, """    let (too_high, bound) = if found < min {
        (false, &min)
    } else if found > max {
        (true, &max)
    } else {
        return None;
    };""",
                   """    let (too_high, bound) = if found > max {
        (true, &max)
    } else if found < min {
        (false, &min)
    } else {
        return None;
    };""",
                   "KILLED", "the low test is FIRST: a min>max range reports the LOW bound"),
            (DISP, """                    return Err(ReaderError::InvalidEscape(
                        String::from_utf16_lossy(&[c]),
                    ));""",
                   """                    return Err(ReaderError::InvalidEscape(
                        String::from_utf16_lossy(&[terminator]),
                    ));""",
                   "KILLED", "the escape error names the offending char, not the quote"),
        ],
    ),
    # ── updateUsageInfo's three branches ─────────────────────────────────
    "c": (
        "updateUsageInfo's branches and colours",
        NET,
        900,
        [
            (FMT, "    let mut trailing_characters = false;",
                  "    let mut trailing_characters = false; // no-op control",
                  "SURVIVED", "NO-OP CONTROL: a comment"),
            (FMT, "    if cursor == parse.reader.total_length() {",
                  "    if cursor <= parse.reader.total_length() {",
                  "KILLED", "the branches need the cursor AT THE END of the field"),
            (FMT, "        if suggestions_empty && !parse.errors.is_empty() {",
                  "        if !parse.errors.is_empty() {",
                  "KILLED", "a popup to show suppresses the exception branch"),
            (FMT, "        } else if parse.reader.can_read() {\n            trailing_characters = true;",
                  "        } else if !parse.reader.can_read() {\n            trailing_characters = true;",
                  "KILLED", "trailing characters means the reader CAN still read"),
            (FMT, "    if usage.is_empty() && !exception_branch {",
                  "    if usage.is_empty() {",
                  "KILLED", "an exception branch that printed nothing still suppresses usage"),
            (FMT, "        if entries.is_empty() && trailing_characters {",
                  "        if trailing_characters {",
                  "KILLED", "a usage entry beats the third branch"),
            (FMT, "        usage.extend(entries.into_iter().map(|text| UsageLine {\n            text,\n            color: USAGE_COLOR,\n        }));",
                  "        usage.extend(entries.into_iter().map(|text| UsageLine {\n            text,\n            color: ERROR_COLOR,\n        }));",
                  "KILLED", "usage entries are USAGE_FORMAT's grey, not the default white"),
            (FMT, "pub const ERROR_COLOR: u32 = 0xFF_FFFF;",
                  "pub const ERROR_COLOR: u32 = 0xAA_AAAA;",
                  "KILLED", "an unstyled message keeps extractUsage's `-1`"),
            (FMT, "                if e.error == ReaderError::LiteralIncorrect {\n                    literals += 1;\n                } else if let Some(line) = error_line(BuiltIn::Reader(&e.error), e.cursor) {",
                  "                if false {\n                    literals += 1;\n                } else if let Some(line) = error_line(BuiltIn::Reader(&e.error), e.cursor) {",
                  "SURVIVED", "literalIncorrect is unreachable — the counter is dead in vanilla too"),
        ],
    ),
    # ── Commands.getParseException ───────────────────────────────────────
    "d": (
        "Commands.getParseException",
        NET,
        900,
        [
            (ERRS, "    if !parse.reader.can_read() {\n        return None;\n    }",
                   "    if !parse.reader.can_read() {\n        return None;\n    }\n    // no-op control",
                   "SURVIVED", "NO-OP CONTROL: a comment"),
            (ERRS, "    if !parse.reader.can_read() {\n        return None;\n    }",
                   "    if false {\n        return None;\n    }",
                   "KILLED", "a fully consumed input has no parse exception"),
            (ERRS, "    if parse.errors.len() == 1 {",
                   "    if parse.errors.len() >= 1 {",
                   "KILLED", "exactly ONE recorded error is reported as itself"),
            (ERRS, "    let which = if parse.context.range.is_empty() {",
                   "    let which = if !parse.context.range.is_empty() {",
                   "KILLED", "an EMPTY root range is Unknown command"),
            (ERRS, "        return Some((BuiltIn::Reader(&e.error), e.cursor));",
                   "        return Some((BuiltIn::Reader(&e.error), parse.reader.cursor()));",
                   "KILLED", "the single error keeps its OWN cursor"),
        ],
    ),
    # ── the renderer ─────────────────────────────────────────────────────
    "e": (
        "the usage box's per-line colour",
        APP,
        900,
        [
            (LIVE, "            color: srgb_bytes_to_linear(line.color),",
                   "            color: srgb_bytes_to_linear(line.color), // no-op control",
                   "SURVIVED", "NO-OP CONTROL: a comment"),
            (LIVE, "            color: srgb_bytes_to_linear(line.color),",
                   "            color: [1.0, 1.0, 1.0],",
                   "KILLED", "the box is not one colour — M117's bug"),
            (LIVE, "            color: srgb_bytes_to_linear(line.color),",
                   "            color: srgb_bytes_to_linear_f([\n                ((line.color >> 16) & 0xFF) as f32 / 255.0,\n                ((line.color >> 8) & 0xFF) as f32 / 255.0,\n                (line.color & 0xFF) as f32 / 255.0,\n            ]),",
                   "SURVIVED", "the two sRGB helpers are the same function"),
        ],
    ),
}


def main():
    which = sys.argv[1] if len(sys.argv) > 1 else None
    total = 0
    for key, (name, check, timeout, muts) in BATTERIES.items():
        if which and key != which:
            continue
        total += battery(f"{key}: {name}", check, timeout, muts)
    print("ALL BATTERIES AS EXPECTED" if total == 0 else f"{total} UNEXPECTED")
    return 1 if total else 0


if __name__ == "__main__":
    sys.exit(main())
