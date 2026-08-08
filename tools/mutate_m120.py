"""M120's mutation battery — the coordinate family and the value-shaped types.

Same contract as the earlier ones. Every batch carries a NO-OP CONTROL that
must SURVIVE.

Usage:  python tools/mutate_m120.py [batch]
"""

import os
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
AT = "crates/rewo-net/src/arg_types.rs"
DISP = "crates/rewo-net/src/dispatcher.rs"
TEST = ["cargo", "test", "-q", "-p", "rewo-net", "--lib"]

MUTATIONS = [
    # --- batch 1: the coordinate reader ----------------------------------
    (1, "CONTROL no-op (must SURVIVE)", AT,
     "pub fn read_coords(", "pub fn read_coords("),
    (1, "the number after a tilde becomes mandatory", AT,
     "    if reader.can_read() && reader.peek() != b' ' as u16 {\n        if integral && !relative {\n            reader.read_i32()?;\n        } else {\n            reader.read_f64()?;\n        }\n    }\n    Ok(())",
     "    if integral && !relative {\n        reader.read_i32()?;\n    } else {\n        reader.read_f64()?;\n    }\n    Ok(())"),
    (1, "a block position reads a double even when absolute", AT,
     "        if integral && !relative {\n            reader.read_i32()?;", "        if false {\n            reader.read_i32()?;"),
    (1, "the caret is decided per component rather than per group", AT,
     "    let local = kind.allows_local() && reader.can_read() && reader.peek() == b'^' as u16;",
     "    let local = false;"),
    # `the world reader accepts a caret` is deliberately NOT here: it is
    # equivalent by construction, and the reasoning lives in the source
    # comment beside the guard. `isAllowedNumber` excludes `^`, so the
    # number reader raises ExpectedDouble on exactly the inputs the guard
    # rejects -- the guard changes which error vanilla reports, and Rewo
    # renders none of them.

    # --- batch 2: shapes and counts --------------------------------------
    (2, "CONTROL no-op (must SURVIVE)", AT,
     "pub fn resolve(", "pub fn resolve("),
    (2, "rotation gains a local form", AT,
     "        matches!(self, Self::BlockPos | Self::ColumnPos | Self::Vec3)",
     "        true"),
    (2, "vec2 takes three components", AT,
     "            Self::ColumnPos | Self::Vec2 | Self::Rotation => 2,",
     "            Self::ColumnPos | Self::Rotation => 2,\n            Self::Vec2 => 3,"),
    (2, "the components need no separator", AT,
     "            if !reader.can_read() || reader.peek() != b' ' as u16 {\n                return Err(ReaderError::UnknownArgumentType);\n            }\n            reader.skip();",
     "            if reader.can_read() && reader.peek() == b' ' as u16 {\n                reader.skip();\n            }"),

    # --- batch 3: the suggestions and the tables -------------------------
    (3, "CONTROL no-op (must SURVIVE)", AT,
     "pub fn suggest_coords(", "pub fn suggest_coords("),
    (3, "only the complete triple is offered", AT,
     "        out.push(acc.clone());", "        if acc.matches(' ').count() + 1 == kind.count() { out.push(acc.clone()); }"),
    (3, "a typed caret does not switch the default set", AT,
     '    let unit = if remaining.starts_with(\'^\') && kind.allows_local() {\n        "^"\n    } else {\n        "~"\n    };',
     '    let unit = "~";'),
    (3, "an operation is read as a word", AT,
     "    let punctuation = choices.iter().all(|c| !c.chars().next().is_some_and(char::is_alphanumeric));\n    if !punctuation {",
     "    let punctuation = false;\n    if !punctuation {"),
    (3, "a failed choice does not roll back", AT,
     "                    reader.set_cursor(start);\n                    Err(ReaderError::UnknownArgumentType)",
     "                    Err(ReaderError::UnknownArgumentType)"),

    # --- batch 4: the wiring ---------------------------------------------
    (4, "CONTROL no-op (must SURVIVE)", DISP,
     "pub fn resolve(", "pub fn resolve("),
    (4, "the value family shadows the modules above it", DISP,
     '            ("minecraft:entity" | "minecraft:game_profile", _) => Self::Entity,',
     ""),
    (4, "the value family is not resolved at all", DISP,
     "            (name, _) if crate::arg_types::resolve(name).is_some() => {",
     "            (name, _) if false && crate::arg_types::resolve(name).is_some() => {"),
    (4, "the registry name never reaches the suggester", DISP,
     "                    let registry = match props {\n                        ArgumentProps::Registry(r) => Some(r.as_str()),\n                        _ => None,\n                    };",
     "                    let registry: Option<&str> = None;"),
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
            # Proven, not assumed: `isAllowedNumber` excludes `^`, so the
            # number reader raises `ExpectedDouble` on exactly the inputs the
            # explicit guard rejects. The guard changes which error vanilla
            # reports, and Rewo renders none of them.
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
