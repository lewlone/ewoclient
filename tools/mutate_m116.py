"""M116's mutation battery — the client-side Brigadier dispatcher.

Same contract as `mutate_m114.py`: one substring swap, run the check that
claims to cover it, restore. Every batch carries a NO-OP CONTROL that must
SURVIVE, because a battery run against an already-failing command reads
KILLED for every entry.

Usage:  python tools/mutate_m116.py [batch]
"""

import os
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
DISP = "crates/rewo-net/src/dispatcher.rs"
TEST = ["cargo", "test", "-q", "-p", "rewo-net", "--lib"]

MUTATIONS = [
    # --- batch 1: the tree walk -----------------------------------------
    (1, "CONTROL no-op (must SURVIVE)",
     "fn parse_nodes(", "fn parse_nodes("),
    (1, "getRelevantNodes: try every child, not just the matching literal",
     "    if literals.is_empty() {\n        return arguments;\n    }",
     "    if true {\n        let mut all = literals.clone();\n        all.extend(arguments.iter().copied());\n        return all;\n    }"),
    (1, "EQUIVALENT a literal need not end at a separator",
     "        if !reader.can_read() || reader.peek() == b' ' as u16 {\n            return true;\n        }\n        reader.set_cursor(start);",
     "        return true;"),
    (1, "the argument separator check is dropped",
     "            if reader.can_read() && reader.peek() != b' ' as u16 {\n                return Err(ReaderError::ExpectedArgumentSeparator);\n            }",
     ""),

    # --- batch 2: suggestion selection ----------------------------------
    (2, "CONTROL no-op (must SURVIVE)",
     "pub fn completion_suggestions(", "pub fn completion_suggestions("),
    (2, "literal suggest: test the prefix the other way round",
     "if lit.to_lowercase().starts_with(&builder.remaining().to_lowercase()) {",
     "if builder.remaining().to_lowercase().starts_with(&lit.to_lowercase()) {"),
    (2, "an ask_server child no longer sets the flag",
     "                if suggestions.is_some() {",
     "                if false {"),
    (2, "findSuggestionContext: drop the +1 past the separator",
     "                    start_pos: last.range.end + 1,",
     "                    start_pos: last.range.end,"),
    (2, "the builder is given the whole input rather than the truncation",
     "    let truncated = &full[..cursor.min(full.len())];",
     "    let truncated = full;"),

    # --- batch 3: the argument types ------------------------------------
    (3, "CONTROL no-op (must SURVIVE)",
     "impl ArgKind {", "impl ArgKind {"),
    (3, "ArgKind dispatches on the props shape rather than the name",
     '            ("brigadier:integer", ArgumentProps::RangeI64 { min, max }) => Self::Integer {',
     '            (_, ArgumentProps::RangeI64 { min, max }) => Self::Integer {'),
    (3, "an Unknown argument parses instead of refusing",
     "            Self::Unknown => Err(ReaderError::UnknownArgumentType),",
     "            Self::Unknown => Ok(()),"),
    (3, "a greedy string reads one word instead of the remainder",
     "            Self::Str(StringType::GreedyPhrase) => {\n                reader.set_cursor(reader.total_length());\n                Ok(())\n            }",
     "            Self::Str(StringType::GreedyPhrase) => {\n                reader.read_unquoted_string();\n                Ok(())\n            }"),
    (3, "the integer range check is dropped",
     "                if v < *min || v > *max {\n                    reader.set_cursor(start);\n                    return Err(ReaderError::OutOfRange);\n                }\n                Ok(())\n            }\n            Self::Long",
     "                let _ = (v, min, max);\n                Ok(())\n            }\n            Self::Long"),

    # --- batch 4: the reader --------------------------------------------
    (4, "CONTROL no-op (must SURVIVE)",
     "pub fn read_bool(", "pub fn read_bool("),
    (4, "isAllowedNumber admits '+'",
     "        (0x30..=0x39).contains(&c) || c == b'.' as u16 || c == b'-' as u16\n",
     "        (0x30..=0x39).contains(&c) || c == b'.' as u16 || c == b'-' as u16 || c == b'+' as u16\n"),
    (4, "readInt saturates instead of erroring",
     "        text.parse::<i32>().map_err(|_| {\n            self.cursor = start;\n            ReaderError::InvalidInt(text)\n        })",
     "        let _ = start;\n        Ok(text.parse::<i64>().unwrap_or(0).clamp(i32::MIN as i64, i32::MAX as i64) as i32)"),
    (4, "a quoted string accepts any escape",
     "                if c != terminator && c != b'\\\\' as u16 {\n                    self.cursor -= 1;\n                    return Err(ReaderError::InvalidEscape);\n                }",
     "                let _ = terminator;"),
    (4, "readBoolean becomes case-insensitive",
     "        match value.as_str() {",
     "        match value.to_lowercase().as_str() {"),
]


def run(cmd):
    r = subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True,
                       encoding="utf-8", errors="replace")
    return r.returncode == 0


def main():
    want = int(sys.argv[1]) if len(sys.argv) > 1 else None
    killed = surviving = broken = 0
    path = os.path.join(ROOT, DISP)
    for batch, label, find, repl in MUTATIONS:
        if want is not None and batch != want:
            continue
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
            # Proven equivalent rather than assumed: `getRelevantNodes` hands a
            # literal only text that already matched it exactly and stopped at
            # a space or the end, so the second half of `parse_literal` can
            # never fire. `a_word_matching_no_literal_reaches_no_node_and_
            # reports_no_error` shows a mis-typed word reaches no literal at
            # all. Dead in vanilla's shape too, and transcribed rather than
            # dropped.
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
