"""M122's mutation battery — the SNBT grammar.

Same contract as the earlier ones. Every batch carries a NO-OP CONTROL that
must SURVIVE.

Usage:  python tools/mutate_m122.py [batch]
"""

import os
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
G = "crates/rewo-net/src/snbt_grammar.rs"
AT = "crates/rewo-net/src/arg_types.rs"
TEST = ["cargo", "test", "-q", "-p", "rewo-net", "--lib"]

MUTATIONS = [
    # --- batch 1: the numerals -------------------------------------------
    (1, "CONTROL no-op (must SURVIVE)", G, "fn integer(", "fn integer("),
    (1, "the leading-zero rejection becomes a re-read", G,
     "            return Err(err());\n        }\n    } else {\n        decimal(r)?;",
     "            decimal(r)?;\n        }\n    } else {\n        decimal(r)?;"),
    (1, "0b commits instead of backtracking", G,
     "            let mark = r.cursor();\n            r.skip();\n            if number_run(r, is_binary).is_err() {\n                r.set_cursor(mark);\n            }",
     "            r.skip();\n            number_run(r, is_binary)?;"),
    (1, "the underscore is allowed at the ends", G,
     "    if first == b'_' as u16 || last == b'_' as u16 {\n        return Err(err());\n    }",
     ""),
    (1, "the unsigned prefix does not need a width", G,
     "        if !(eat_either(r, b'b', b'B')\n            || eat_either(r, b's', b'S')\n            || eat_either(r, b'i', b'I')\n            || eat_either(r, b'l', b'L'))\n        {\n            r.set_cursor(mark);\n        }\n        return;",
     "        let _ = mark;\n        return;"),

    # --- batch 2: floats and strings --------------------------------------
    (2, "CONTROL no-op (must SURVIVE)", G, "fn float(", "fn float("),
    (2, "a bare integer is taken as a float", G,
     "    if float_suffix(r) {\n        return Ok(());\n    }\n    r.set_cursor(start);\n    Err(err())",
     "    Ok(())"),
    (2, "any character may follow a backslash", G,
     "        b'x' => hex_digits(r, 2),", "        _ => Ok(()),\n        #[allow(unreachable_patterns)]\n        b'x' => hex_digits(r, 2),"),
    (2, "a hex escape does not check its length", G,
     "        b'u' => hex_digits(r, 4),", "        b'u' => hex_digits(r, 0),"),
    (2, "EQUIVALENT an unquoted string may start like a number", G,
     "    if !r.can_read() || can_start_number(r.peek()) {\n        return Err(err());\n    }\n    unquoted(r)?;",
     "    unquoted(r)?;"),

    # --- batch 3: structure ------------------------------------------------
    (3, "CONTROL no-op (must SURVIVE)", G, "fn separated(", "fn separated("),
    (3, "a trailing comma requires another item", G,
     "        skip_whitespace(r);\n        if at(r, close) {\n            return Ok(());\n        }\n        item(r, depth)?;",
     "        item(r, depth)?;"),
    (3, "the builtin call form is dropped", G,
     "    if at(r, b'(') {\n        r.skip();\n        separated(r, depth, b')', parse_value_at)?;\n        expect(r, b')')?;\n    }",
     ""),
    (3, "an array prefix may be lower-case", G,
     "        let prefixed = matches!(r.peek() as u8, b'B' | b'L' | b'I') && {",
     "        let prefixed = matches!(r.peek() as u8, b'B' | b'L' | b'I' | b'b' | b'l' | b'i') && {"),
    (3, "whitespace is not skipped before a token", G,
     "fn skip_whitespace(r: &mut StringReader) {\n    while r.can_read() && matches!(r.peek(), 0x20 | 0x09 | 0x0A | 0x0B | 0x0C | 0x0D) {\n        r.skip();\n    }\n}",
     "fn skip_whitespace(_r: &mut StringReader) {}"),

    # --- batch 4: the wiring -----------------------------------------------
    (4, "CONTROL no-op (must SURVIVE)", AT, "pub fn resolve(", "pub fn resolve("),
    (4, "the arguments go back to the extent walk", AT,
     "            Self::Snbt => crate::snbt_grammar::parse_value(reader),\n            Self::SnbtCompound => crate::snbt_grammar::parse_compound(reader),",
     "            Self::Snbt => crate::snbt::read_value_extent(reader),\n            Self::SnbtCompound => crate::snbt::read_compound_extent(reader),"),
    (4, "the recursion guard is removed", G,
     "    if depth > MAX_DEPTH {\n        return Err(err());\n    }", ""),
]


def run(cmd):
    r = subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True,
                       encoding="utf-8", errors="replace")
    return r.returncode == 0


def main():
    want = int(sys.argv[1]) if len(sys.argv) > 1 else None
    killed = surviving = broken = 0
    for batch, label, rel, find, repl in MUTATIONS:
        if want is not None and batch != want:
            continue
        path = os.path.join(ROOT, rel)
        with open(path, "rb") as f:
            original = f.read()
        text = original.decode("utf-8")
        if text.count(find) != 1:
            print(f"[batch {batch}] ANCHOR x{text.count(find)}  {label}")
            broken += 1
            continue
        try:
            with open(path, "wb") as f:
                f.write(text.replace(find, repl, 1).encode("utf-8"))
            passed = run(TEST)
        finally:
            with open(path, "wb") as f:
                f.write(original)
        if label.startswith("EQUIVALENT"):
            # Proven, not assumed: parse_value_at only falls through to
            # unquoted_or_builtin when can_start_number has already failed on
            # the same character, so the rule's own copy of that test can
            # never fire. Transcribed because vanilla carries it there.
            verdict = "OK (equivalent, survives by construction)" if passed else "!! an EQUIVALENT mutant died -- the claim is wrong"
            if not passed:
                broken += 1
            print(f"[batch {batch}] {verdict}: {label}")
            continue
        if label.startswith("CONTROL"):
            if passed:
                print(f"[batch {batch}] OK (control survived): {label}")
            else:
                print(f"[batch {batch}] !! CONTROL DIED -- instrument broken: {label}")
                broken += 1
        elif passed:
            print(f"[batch {batch}] SURVIVED  <-- investigate: {label}")
            surviving += 1
        else:
            print(f"[batch {batch}] killed: {label}")
            killed += 1
    print(f"\nkilled {killed}, survived {surviving}, broken {broken}")
    return 1 if (surviving or broken) else 0


if __name__ == "__main__":
    sys.exit(main())
