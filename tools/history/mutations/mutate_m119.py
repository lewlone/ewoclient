"""M119's mutation battery — the block-state and item argument types.

Same contract as the earlier ones. Every batch carries a NO-OP CONTROL that
must SURVIVE.

Usage:  python tools/mutate_m119.py [batch]
"""

import os
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BI = "crates/rewo-net/src/block_item.rs"
SUG = "crates/rewo-world/src/suggestions.rs"
DISP = "crates/rewo-net/src/dispatcher.rs"
TEST_NET = ["cargo", "test", "-q", "-p", "rewo-net", "--lib"]
TEST_WORLD = ["cargo", "test", "-q", "-p", "rewo-world", "--lib"]

MUTATIONS = [
    # --- batch 1: suggestResource ---------------------------------------
    (1, "CONTROL no-op (must SURVIVE)", SUG,
     "pub fn suggest_resource<", "pub fn suggest_resource<", TEST_NET),
    (1, "the whole id is matched even without a colon", SUG,
     "            let (namespace, path) = lower.split_once(':').unwrap_or((\"minecraft\", &lower));\n            matches_sub_str(&contents, namespace) || matches_sub_str(&contents, path)",
     "            matches_sub_str(&contents, &lower)", TEST_NET),
    (1, "a typed colon still matches the path alone", SUG,
     "        let matched = if has_namespace {\n            matches_sub_str(&contents, &lower)\n        } else {",
     "        let matched = if false {\n            matches_sub_str(&contents, &lower)\n        } else {", TEST_NET),

    # --- batch 2: the bracket gating -------------------------------------
    (2, "CONTROL no-op (must SURVIVE)", BI,
     "pub fn fill_suggestions(", "pub fn fill_suggestions(", TEST_NET),
    (2, "brackets are filtered by prefix rather than suppressed", BI,
     "        let empty = builder.remaining().is_empty();", "        let empty = true;", TEST_NET),
    (2, "the comma is offered even with every property set", BI,
     "                    if self.set.len() < total {", "                    if true {", TEST_NET),
    (2, "the open bracket ignores whether the block has properties", BI,
     "                if empty\n                    && self\n                        .id\n                        .as_deref()\n                        .and_then(|id| reg.blocks.and_then(|b| b.properties(id)))\n                        .is_some_and(|p| !p.is_empty())\n                {",
     "                if empty {", TEST_NET),

    # --- batch 3: the property loop --------------------------------------
    (3, "CONTROL no-op (must SURVIVE)", BI,
     "fn read_properties(", "fn read_properties(", TEST_NET),
    (3, "a set property is offered again", BI,
     "            if !self.set.iter().any(|k| k == name) && name.to_lowercase().starts_with(&prefix) {",
     "            if name.to_lowercase().starts_with(&prefix) {", TEST_NET),
    (3, "the value suggester is installed AFTER the value is read", BI,
     "            self.suggestions = Suggest::PropertyValue(key.clone());\n            let value_start = reader.cursor();\n            let value = reader.read_string()?;",
     "            let value_start = reader.cursor();\n            let value = reader.read_string()?;\n            self.suggestions = Suggest::PropertyValue(key.clone());", TEST_NET),
    (3, "suggestEquals is not set before the = test", BI,
     "            self.suggestions = Suggest::Equals;\n            if !reader.can_read() || reader.peek() != b'=' as u16 {",
     "            if !reader.can_read() || reader.peek() != b'=' as u16 {", TEST_NET),
    (3, "an unknown block does not rewind", BI,
     "            // `orElseThrow` rewinds before throwing, so the suggester still\n            // sees the whole typed id as its prefix.\n            reader.set_cursor(start);\n            return Err(ReaderError::UnknownArgumentType);",
     "            return Err(ReaderError::UnknownArgumentType);", TEST_NET),

    # --- batch 4: the identifier reader + wiring -------------------------
    (4, "CONTROL no-op (must SURVIVE)", BI,
     "fn read_identifier(", "fn read_identifier(", TEST_NET),
    (4, "the identifier reader stops at the colon", BI,
     "        || matches!(c, 0x5F | 0x3A | 0x2F | 0x2E | 0x2D)",
     "        || matches!(c, 0x5F | 0x2F | 0x2E | 0x2D)", TEST_NET),
    (4, "a bare id is not given the default namespace", BI,
     '        format!("minecraft:{id}")', "        id.to_string()", TEST_NET),
    (4, "block_state goes back to Unknown", DISP,
     '            ("minecraft:block_state" | "minecraft:block_predicate", _) => Self::BlockState,',
     "", TEST_NET),
    (4, "item_stack goes back to Unknown", DISP,
     '            ("minecraft:item_stack" | "minecraft:item_predicate", _) => Self::ItemStack,',
     "", TEST_NET),
]


def run(cmd):
    r = subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True,
                       encoding="utf-8", errors="replace")
    return r.returncode == 0


def main():
    want = int(sys.argv[1]) if len(sys.argv) > 1 else None
    killed = surviving = broken = 0
    for batch, label, rel, find, repl, cmd in MUTATIONS:
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
            passed = run(cmd)
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
