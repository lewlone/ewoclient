"""M123's mutation battery — the numerals' types and ranges.

Same contract as the earlier ones. Every batch carries a NO-OP CONTROL that
must SURVIVE.

Usage:  python tools/mutate_m123.py [batch]
"""

import os
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
G = "crates/rewo-net/src/snbt_grammar.rs"
TEST = ["cargo", "test", "-q", "-p", "rewo-net", "--lib"]

MUTATIONS = [
    # --- batch 1: which range applies -------------------------------------
    (1, "CONTROL no-op (must SURVIVE)", G, "fn signedness(", "fn signedness("),
    (1, "the base-driven default is inverted", G,
     "            Base::Binary | Base::Hex => Signedness::Unsigned,\n            Base::Decimal => Signedness::Signed,",
     "            Base::Binary | Base::Hex => Signedness::Signed,\n            Base::Decimal => Signedness::Unsigned,"),
    (1, "an explicit u/s no longer wins over the base", G,
     "        self.declared.unwrap_or(match self.base {",
     "        let _ = self.declared;\n        (match self.base {"),
    (1, "the non-negative check is dropped", G,
     "        if !signed && self.negative {\n            // `ERROR_EXPECTED_NON_NEGATIVE_NUMBER`. `-0xF` is an error, because\n            // hex is unsigned by default — the sign and the base disagree and\n            // the base wins.\n            return Err(err());\n        }",
     ""),
    (1, "the standalone default width becomes LONG", G,
     "    let width = literal.width.unwrap_or(Width::Int);",
     "    let width = literal.width.unwrap_or(Width::Long);"),

    # --- batch 2: the bounds themselves ------------------------------------
    (2, "CONTROL no-op (must SURVIVE)", G, "fn check(", "fn check("),
    (2, "a signed byte gets the unsigned range", G,
     "                Width::Byte => (i8::MIN as i128, i8::MAX as i128),",
     "                Width::Byte => (0, u8::MAX as i128),"),
    (2, "an unsigned width gets the signed maximum", G,
     "                Width::Int => u32::MAX as u128,",
     "                Width::Int => i32::MAX as u128,"),
    (2, "the range check is only one-sided", G,
     "            value >= min && value <= max",
     "            let _ = min;\n            value <= max"),
    (2, "the separators are left in the digits", G,
     "        .filter(|&&c| c != b'_' as u16)", "        .filter(|&&c| c != 0xFFFF)"),

    # --- batch 3: the suffix and the float ---------------------------------
    (3, "CONTROL no-op (must SURVIVE)", G, "fn integer_suffix(", "fn integer_suffix("),
    (3, "the s signed prefix is gone again", G,
     "        (b'u', b'U', Signedness::Unsigned),\n        (b's', b'S', Signedness::Signed),",
     "        (b'u', b'U', Signedness::Unsigned),"),
    (3, "a prefix with no width is kept anyway", G,
     "            // A prefix on its own is not a suffix, and the whole thing is\n            // optional, so the cursor goes back and the bare alternatives run.\n            r.set_cursor(mark);",
     "            return (Some(signedness), None);"),
    (3, "the finiteness test is dropped", G,
     "    match ok {\n        Ok(true) => Ok(()),\n        _ => Err(err()),\n    }",
     "    let _ = ok;\n    Ok(())"),
    (3, "f and d are swapped", G,
     "    if eat_either(r, b'f', b'F') {\n        Some(true)\n    } else if eat_either(r, b'd', b'D') {\n        Some(false)",
     "    if eat_either(r, b'f', b'F') {\n        Some(false)\n    } else if eat_either(r, b'd', b'D') {\n        Some(true)"),

    # --- batch 4: arrays, and the terminal's whitespace --------------------
    (4, "CONTROL no-op (must SURVIVE)", G, "fn array_allows(", "fn array_allows("),
    (4, "an array element may widen", G,
     "        Width::Byte => width == Width::Byte,",
     "        Width::Byte => true,"),
    (4, "the long array stops admitting int", G,
     "        Width::Long => matches!(width, Width::Long | Width::Byte | Width::Short | Width::Int),",
     "        Width::Long => width == Width::Long,"),
    (4, "an undeclared element defaults to INT, not the array's width", G,
     "        None => prefix,\n        Some(w) if array_allows(prefix, w) => w,",
     "        None => Width::Int,\n        Some(w) if array_allows(prefix, w) => w,"),
    (4, "the terminal keeps the whitespace it skipped on a miss", G,
     "        r.set_cursor(mark);\n        false\n    }\n}\n\n/// `SnbtGrammar.canStartNumber`.",
     "        false\n    }\n}\n\n/// `SnbtGrammar.canStartNumber`."),
    (4, "the terminal stops skipping whitespace", G,
     "fn eat_either(r: &mut StringReader, lower: u8, upper: u8) -> bool {\n    let mark = r.cursor();\n    skip_whitespace(r);",
     "fn eat_either(r: &mut StringReader, lower: u8, upper: u8) -> bool {\n    let mark = r.cursor();"),
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
