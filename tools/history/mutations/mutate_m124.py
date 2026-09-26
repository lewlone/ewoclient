"""M124's mutation battery — the seven literal tables.

Same contract as the earlier ones. Every batch carries a NO-OP CONTROL that
must SURVIVE.

Usage:  python tools/mutate_m124.py [batch]
"""

import os
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
A = "crates/rewo-net/src/arg_types.rs"
S = "crates/rewo-net/src/slot_ranges.rs"
TEST = ["cargo", "test", "-q", "-p", "rewo-net", "--lib"]

MUTATIONS = [
    # --- batch 1: the three word tables -----------------------------------
    (1, "CONTROL no-op (must SURVIVE)", A, "pub const HEIGHTMAPS", "pub const HEIGHTMAPS"),
    (1, "the worldgen heightmaps come back", A,
     'pub const HEIGHTMAPS: [&str; 4] = [\n    "world_surface",',
     'pub const HEIGHTMAPS: [&str; 6] = [\n    "world_surface_wg",\n    "ocean_floor_wg",\n    "world_surface",'),
    (1, "a team colour list gains the dye names", A,
     '    "dark_gray", "blue", "green", "aqua", "red", "light_purple", "yellow", "white",\n];',
     '    "dark_gray", "blue", "green", "aqua", "red", "light_purple", "yellow", "white", "orange",\n];'),
    (1, "the scoreboard slots lose their sidebar prefix", A,
     '    "sidebar.team.black",', '    "team.black",'),
    (1, "heightmap goes back to a bare word", A,
     '        "minecraft:heightmap" => Value::Choice(&HEIGHTMAPS),', ''),

    # --- batch 2: swizzle --------------------------------------------------
    (2, "CONTROL no-op (must SURVIVE)", A, "let mut seen = [false; 3];", "let mut seen = [false; 3];"),
    (2, "an axis may repeat", A,
     "                    if std::mem::replace(&mut seen[axis], true) {\n                        return Err(ReaderError::UnknownArgumentType);\n                    }",
     "                    seen[axis] = true;"),
    (2, "any character is an axis", A,
     "                        _ => return Err(ReaderError::UnknownArgumentType),\n                    };",
     "                        _ => 0,\n                    };"),
    (2, "swizzle starts suggesting its axes", A,
     "            // Not an omission: `SwizzleArgument` declares no `listSuggestions`.\n            | Self::Swizzle => {}",
     "            => {}\n            Self::Swizzle => suggest_matching([\"x\", \"y\", \"z\"], builder),"),

    # --- batch 3: the slot table -------------------------------------------
    (3, "CONTROL no-op (must SURVIVE)", S, "fn build()", "fn build()"),
    (3, "a range numbers from one", S,
     "        for i in 0..size {\n            out.push((format!(\"{prefix}{i}\"), 1));\n        }",
     "        for i in 1..=size {\n            out.push((format!(\"{prefix}{i}\"), 1));\n        }"),
    (3, "the star is counted as a single slot", S,
     "        out.push((format!(\"{prefix}*\"), size));",
     "        out.push((format!(\"{prefix}*\"), 1));"),
    (3, "the container range is 27 deep", S,
     'range(&mut out, "container.", 54);', 'range(&mut out, "container.", 27);'),
    (3, "horse.chest is dropped as a duplicate of the horse range", S,
     '    single(&mut out, "horse.chest");', ''),

    # --- batch 4: the two slot arguments -----------------------------------
    (4, "CONTROL no-op (must SURVIVE)", A, "Self::Slot { single } => {", "Self::Slot { single } => {"),
    (4, "item_slot stops rejecting a multi-slot name", A,
     "                    Some(1) => Ok(()),\n                    Some(_) if !*single => Ok(()),",
     "                    Some(_) => Ok(()),"),
    (4, "an unknown slot name is accepted", A,
     "                match crate::slot_ranges::lookup(&name) {",
     "                if crate::slot_ranges::lookup(&name).is_none() {\n                    return Ok(());\n                }\n                match crate::slot_ranges::lookup(&name) {"),
    (4, "a slot name is read as an unquoted string", A,
     "                let start = reader.cursor();\n                while reader.can_read() && reader.peek() != b' ' as u16 {\n                    reader.skip();\n                }\n                let name: String =",
     "                let start = reader.cursor();\n                let _ = reader.read_unquoted_string();\n                let name: String ="),
    # --- batch 5: time and hex_color ---------------------------------------
    (5, "CONTROL no-op (must SURVIVE)", A, "pub const TIME_UNITS", "pub const TIME_UNITS"),
    (5, "the empty time unit is dropped", A,
     'pub const TIME_UNITS: [&str; 4] = ["d", "s", "t", ""];',
     'pub const TIME_UNITS: [&str; 3] = ["d", "s", "t"];'),
    (5, "a duration takes any unit", A,
     "                if TIME_UNITS.contains(&unit.as_str()) {",
     "                if true || TIME_UNITS.contains(&unit.as_str()) {"),
    (5, "the unit completes over the whole argument", A,
     "                builder.rebase(after_number);", "                let _ = after_number;"),
    (5, "the unit suggester stops requiring a float", A,
     "                if r.read_f32().is_err() {", "                if false {"),
    (5, "a hex colour accepts any length", A,
     "                let hex = matches!(text.len(), 3 | 6)",
     "                let hex = !text.is_empty()"),
    (5, "a hex colour accepts any character", A,
     "                    && text.chars().all(|c| c.is_ascii_hexdigit());",
     "                    ;"),

    (4, "the two types share one suggestion list", A,
     "            Self::Slot { single: true } => {\n                suggest_matching(crate::slot_ranges::single_slot_names(), builder)\n            }",
     "            Self::Slot { single: true } => {\n                suggest_matching(crate::slot_ranges::all_names(), builder)\n            }"),
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
