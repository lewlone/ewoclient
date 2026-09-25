"""M117's mutation battery — syntax highlighting and the usage lines.

Same contract as `mutate_m114.py` / `mutate_m116.py`: one substring swap, run
the check that claims to cover it, restore. Every batch carries a NO-OP CONTROL
that must SURVIVE.

Usage:  python tools/mutate_m117.py [batch]
"""

import os
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
FMT = "crates/rewo-net/src/command_format.rs"
APP = "crates/rewo-app/src/live_cmd.rs"
TEST_NET = ["cargo", "test", "-q", "-p", "rewo-net", "--lib"]
TEST_APP = ["cargo", "test", "-q", "-p", "rewo-app"]

MUTATIONS = [
    # --- batch 1: formatText --------------------------------------------
    (1, "CONTROL no-op (must SURVIVE)", FMT,
     "pub fn format_text(", "pub fn format_text(", TEST_NET),
    (1, "the colour cycle starts at 0 and post-increments", FMT,
     "    let mut next_color: i32 = -1;", "    let mut next_color: i32 = 0;", TEST_NET),
    (1, "the gaps take the field's colour instead of grey", FMT,
     "        push(&mut parts, unformatted_start, start, GRAY);\n            push(&mut parts, start, end, ARGUMENT_COLORS[next_color as usize]);",
     "            push(&mut parts, start, end, ARGUMENT_COLORS[next_color as usize]);", TEST_NET),
    (1, "the unparsed tail is measured from the last argument, not the reader", FMT,
     "        let start = parse.reader.cursor().saturating_sub(offset);",
     "        let start = unformatted_start;", TEST_NET),
    (1, "the visible-text offset is ignored", FMT,
     "        let start = range.start.saturating_sub(offset);",
     "        let start = range.start;", TEST_NET),

    # --- batch 2: getSmartUsage ------------------------------------------
    (2, "CONTROL no-op (must SURVIVE)", FMT,
     "pub fn smart_usage(", "pub fn smart_usage(", TEST_NET),
    (2, "the pipe list reuses the deep usages instead of getUsageText", FMT,
     "            let joined = children\n                .iter()\n                .filter_map(|&c| tree.node(c).map(usage_text))\n                .collect::<Vec<_>>()\n                .join(\"|\");",
     "            let joined = child_usage.join(\"|\");", TEST_NET),
    (2, "the bracket choice follows the CHILD rather than this node", FMT,
     "    let child_optional = node.is_executable();\n    let (open, close) = if child_optional { (\"[\", \"]\") } else { (\"(\", \")\") };",
     "    let child_optional = node.is_executable();\n    let (open, close) = (\"(\", \")\");", TEST_NET),
    (2, "deep no longer stops the expansion", FMT,
     "    if deep {\n        // The whole expansion below is skipped, which is what stops a usage\n        // line growing past two levels.\n        return Some(self_text);\n    }",
     "", TEST_NET),
    (2, "literal children get a usage line too", FMT,
     "        .filter(|(c, _)| !matches!(tree.node(*c).map(|n| &n.kind), Some(NodeKind::Literal(_))))",
     "", TEST_NET),

    # --- batch 3: the usage gate + geometry ------------------------------
    (3, "CONTROL no-op (must SURVIVE)", FMT,
     "pub fn usage_lines(", "pub fn usage_lines(", TEST_NET),
    (3, "the error gate ignores whether the cursor is at the end", FMT,
     "    if at_end && suggestions_empty && !parse.errors.is_empty() {",
     "    if suggestions_empty && !parse.errors.is_empty() {", TEST_NET),
    (3, "the error gate ignores whether there are suggestions", FMT,
     "    if at_end && suggestions_empty && !parse.errors.is_empty() {",
     "    if at_end && !parse.errors.is_empty() {", TEST_NET),
    (3, "the usage position clamps to zero instead of its max", FMT,
     "    screen_x_of_start.max(0).min(max)", "    screen_x_of_start.min(max).max(0)", TEST_NET),

    # --- batch 4: the render ---------------------------------------------
    (4, "CONTROL no-op (must SURVIVE)", APP,
     "fn usage_box(", "fn usage_box(", TEST_APP),
    (4, "the usage lines lay out downward", APP,
     "        let line_y = gui_h - rewo_world::command_suggestions::USAGE_OFFSET_FROM_BOTTOM\n            - rewo_world::command_suggestions::LINE_HEIGHT * y as i32;",
     "        let line_y = gui_h - rewo_world::command_suggestions::USAGE_OFFSET_FROM_BOTTOM\n            + rewo_world::command_suggestions::LINE_HEIGHT * y as i32;", TEST_APP),
    (4, "the usage fill loses its one-pixel padding", APP,
     "            x: (position - 1) as f32 * px,\n            y: line_y as f32 * px,\n            w: (box_width + 2) as f32 * px,",
     "            x: position as f32 * px,\n            y: line_y as f32 * px,\n            w: box_width as f32 * px,", TEST_APP),
    (4, "the runs are laid out on a fixed six pixels", APP,
     "                x += width_of(&run.text) as f32 * px;",
     "                x += run.text.chars().count() as f32 * 6.0 * px;", TEST_APP),
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
