"""M118's mutation battery — the entity selector parser.

Same contract as the earlier ones: one substring swap, run the check that
claims to cover it, restore. Every batch carries a NO-OP CONTROL that must
SURVIVE.

Usage:  python tools/mutate_m118.py [batch]
"""

import os
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SEL = "crates/rewo-net/src/selector.rs"
DISP = "crates/rewo-net/src/dispatcher.rs"
TEST = ["cargo", "test", "-q", "-p", "rewo-net", "--lib"]

MUTATIONS = [
    # --- batch 1: the shape ---------------------------------------------
    (1, "CONTROL no-op (must SURVIVE)", SEL,
     "pub fn parse(", "pub fn parse("),
    (1, "parseNameOrUUID sets suggestName unconditionally", SEL,
     "        if reader.can_read() {\n            self.suggestions = Suggest::Name;\n        }",
     "        self.suggestions = Suggest::Name;"),
    (1, "the option list is not gated on canUse", SEL,
     "            if self.can_use(i) && o.name.to_lowercase().starts_with(&lower_prefix) {",
     "            if o.name.to_lowercase().starts_with(&lower_prefix) {"),
    (1, "@s no longer counts as the current entity", SEL,
     "            's' => self.current_entity = true,", "            's' => {}"),
    (1, "an option is offered again after being parsed", SEL,
     "            Gate::Once => !self.parsed.contains(&index),",
     "            Gate::Once => true,"),

    # --- batch 2: the states ---------------------------------------------
    (2, "CONTROL no-op (must SURVIVE)", SEL,
     "pub fn fill_suggestions(", "pub fn fill_suggestions("),
    (2, "the option names are suggested without their equals", SEL,
     '                builder.suggest(&format!("{}=", o.name));',
     "                builder.suggest(o.name);"),
    (2, "SUGGEST_NOTHING is set AFTER the handler runs", SEL,
     "            self.suggestions = Suggest::Nothing;\n            self.parse_value(reader, index)?;",
     "            self.parse_value(reader, index)?;\n            self.suggestions = Suggest::Nothing;"),
    (2, "a closed selector still suggests", SEL,
     "            reader.skip();\n            self.suggestions = Suggest::Nothing;\n            Ok(())",
     "            reader.skip();\n            Ok(())"),
    (2, "the comma does not return to the key state", SEL,
     "                reader.skip();\n                self.suggestions = Suggest::OptionsKey;\n            }",
     "                reader.skip();\n            }"),

    # --- batch 3: the values ---------------------------------------------
    (3, "CONTROL no-op (must SURVIVE)", SEL,
     "fn read_range(", "fn read_range("),
    (3, "a failed choice does not roll the cursor back", SEL,
     "                    reader.set_cursor(start);\n                    Err(ReaderError::UnknownArgumentType)",
     "                    Err(ReaderError::UnknownArgumentType)"),
    (3, "the range reader swallows its own separator", SEL,
     "            if r.peek() == b'.' as u16\n                && r.string().get(r.cursor() + 1).copied() == Some(b'.' as u16)\n            {\n                break;\n            }",
     ""),
    (3, "a bare .. is accepted as a range", SEL,
     "    if has_low || has_high {", "    if true {"),
    (3, "the inverted gamemode forms are dropped", SEL,
     "                        if add_inverted {\n                            builder.suggest(&format!(\"!{c}\"));\n                        }",
     ""),

    # --- batch 4: the dispatcher wiring ----------------------------------
    (4, "CONTROL no-op (must SURVIVE)", DISP,
     "pub fn resolve(", "pub fn resolve("),
    (4, "minecraft:entity goes back to Unknown", DISP,
     '            ("minecraft:entity" | "minecraft:game_profile", _) => Self::Entity,', ""),
    (4, "a failed selector parse is accepted anyway", DISP,
     "                if p.failed {\n                    return Err(ReaderError::UnknownArgumentType);\n                }",
     ""),
    (4, "the entity suggester ignores the builder's start", DISP,
     "            reader.set_cursor(builder.start());", ""),
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
