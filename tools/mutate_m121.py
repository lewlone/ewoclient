"""M121's mutation battery — the six structured argument types.

Same contract as the earlier ones. Every batch carries a NO-OP CONTROL that
must SURVIVE.

Usage:  python tools/mutate_m121.py [batch]
"""

import os
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SN = "crates/rewo-net/src/snbt.rs"
AT = "crates/rewo-net/src/arg_types.rs"
TEST = ["cargo", "test", "-q", "-p", "rewo-net", "--lib"]

MUTATIONS = [
    # --- batch 1: the extent walk ----------------------------------------
    (1, "CONTROL no-op (must SURVIVE)", SN,
     "pub fn read_value_extent(", "pub fn read_value_extent("),
    (1, "the bracket walk stops ignoring quoted sections", SN,
     "        if is_quote(c) {\n            skip_quoted(reader)?;\n            continue;\n        }\n        reader.skip();",
     "        reader.skip();"),
    (1, "a backslash no longer escapes", SN,
     "        if c == b'\\\\' as u16 {\n            if !reader.can_read() {\n                return Err(ReaderError::ExpectedEndOfQuote);\n            }\n            reader.skip();\n        } else if c == terminator {",
     "        if c == terminator {"),
    (1, "nesting is not counted", SN,
     "        if c == open {\n            depth += 1;\n        } else if c == close {",
     "        if false {\n            depth += 1;\n        } else if c == close {"),
    (1, "a colon terminates a bare token again", SN,
     "        0x20 | 0x2C | 0x7B | 0x7D | 0x5B | 0x5D // space , { } [ ]",
     "        0x20 | 0x2C | 0x3A | 0x7B | 0x7D | 0x5B | 0x5D // space , : { } [ ]"),

    # --- batch 2: nbt_path -----------------------------------------------
    (2, "CONTROL no-op (must SURVIVE)", SN,
     "pub fn read_nbt_path(", "pub fn read_nbt_path("),
    (2, "a root filter is legal anywhere", SN,
     "        if !first {\n            return Err(ReaderError::UnknownArgumentType);\n        }",
     ""),
    (2, "nodes need no separator at all", SN,
     "            if next != b' ' as u16 && next != b'[' as u16 && next != b'{' as u16 {\n                if next != b'.' as u16 {\n                    return Err(ReaderError::UnknownArgumentType);\n                }",
     "            if false {\n                if next != b'.' as u16 {\n                    return Err(ReaderError::UnknownArgumentType);\n                }"),
    (2, "a trailing dot is accepted", SN,
     "                if !reader.can_read() || reader.peek() == b' ' as u16 {\n                    return Err(ReaderError::UnknownArgumentType);\n                }",
     ""),
    (2, "the name set becomes an identifier set", SN,
     "    !matches!(\n        c,\n        0x20 | 0x22 | 0x27 | 0x5B | 0x5D | 0x2E | 0x7B | 0x7D // space \" ' [ ] . { }\n    )",
     "    (0x61..=0x7A).contains(&c) || (0x30..=0x39).contains(&c) || c == 0x5F"),

    # --- batch 3: the wiring ---------------------------------------------
    (3, "CONTROL no-op (must SURVIVE)", AT,
     "pub fn resolve(", "pub fn resolve("),
    (3, "nbt_compound_tag accepts any value", AT,
     '        "minecraft:nbt_compound_tag" => Value::SnbtCompound,',
     '        "minecraft:nbt_compound_tag" => Value::Snbt,'),
    (3, "the six go back to Unknown", AT,
     '        "minecraft:nbt_tag" | "minecraft:component" | "minecraft:style" => Value::Snbt,',
     ""),
    (3, "dialog cannot take an inline value", AT,
     '        "minecraft:dialog" => Value::IdOrSnbt,',
     '        "minecraft:dialog" => Value::Id { tag: false },'),
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
