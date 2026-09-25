"""M114's mutation battery.

Each entry replaces one substring in one file, runs the check that claims to
cover it, and restores. A mutation that SURVIVES is a question about the
witness, not a verdict on the code -- prove the mutant equivalent or fix the
fixture.

Every batch carries a NO-OP CONTROL that must SURVIVE. Without one there is no
way to tell a kill from a broken instrument, and a battery run against an
already-failing command reads KILLED for every entry.

Usage:  python tools/mutate_m114.py [batch]
"""

import subprocess
import sys
import os

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

WORLD = "crates/rewo-world/src/command_suggestions.rs"
SUGG = "crates/rewo-world/src/suggestions.rs"
NET = "crates/rewo-net/src/suggestion_wire.rs"

TEST_WORLD = ["cargo", "test", "-q", "-p", "rewo-world", "--lib"]
TEST_NET = ["cargo", "test", "-q", "-p", "rewo-net", "--lib"]

# (batch, label, file, find, replace, command)
MUTATIONS = [
    # --- batch 1: the brigadier port -------------------------------------
    (1, "CONTROL no-op (must SURVIVE)", SUGG,
     "pub fn matches_sub_str(", "pub fn matches_sub_str(", TEST_WORLD),
    (1, "suggest: drop the exact-match early return", SUGG,
     "        if text == self.remaining {\n            return self;\n        }\n", "", TEST_WORLD),
    (1, "matchesSubStr: add ':' to the splitter set", SUGG,
     "b'.' | b'_' | b'/'", "b'.' | b'_' | b'/' | b':'", TEST_WORLD),
    (1, "create: sort case-SENSITIVELY", SUGG,
     "expanded.sort_by(|a, b| compare_ignore_case(&a.text, &b.text));",
     "expanded.sort_by(|a, b| a.text.cmp(&b.text));", TEST_WORLD),

    # --- batch 2: the brigadier port, continued --------------------------
    (2, "CONTROL no-op (must SURVIVE)", SUGG,
     "pub fn suggest_matching<", "pub fn suggest_matching<", TEST_WORLD),
    (2, "create: skip the dedupe", SUGG,
     "            if !expanded.contains(&e) {\n                expanded.push(e);\n            }",
     "            expanded.push(e);", TEST_WORLD),
    (2, "compareToIgnoreCase: fold to lower only", SUGG,
     "                let (ux, uy) = (fold(x, true), fold(y, true));",
     "                let (ux, uy) = (fold(x, false), fold(y, false));", TEST_WORLD),
    (2, "fold: take the first char of a multi-char expansion", SUGG,
     "        (Some(one), None) => {", "        (Some(one), _) => {", TEST_WORLD),
    (2, "merge: route a single source through create", SUGG,
     "        if input.len() == 1 {\n            return input.into_iter().next().unwrap();\n        }",
     "", TEST_WORLD),

    # --- batch 3: the popup's geometry -----------------------------------
    (3, "CONTROL no-op (must SURVIVE)", WORLD,
     "pub fn last_word_index(", "pub fn last_word_index(", TEST_WORLD),
    (3, "clamp: write it the other way round", WORLD,
     "    value.max(min).min(max)", "    value.min(max).max(min)", TEST_WORLD),
    (3, "Rect::contains: exclusive on the high edges", WORLD,
     "x >= self.x && x <= self.x + self.w && y >= self.y && y <= self.y + self.h",
     "x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h", TEST_WORLD),
    (3, "cycle: drop lineStartOffset from the downward branch", WORLD,
     "clamp_i32(current + cfg.line_start_offset - limit as i32, 0, max_offset)",
     "clamp_i32(current - limit as i32, 0, max_offset)", TEST_WORLD),
    (3, "list: forget the unbordered one-pixel shift", WORLD,
     "let list_x = x - if cfg.bordered { 0 } else { 1 };", "let list_x = x;", TEST_WORLD),

    # --- batch 4: the popup's behaviour ----------------------------------
    (4, "CONTROL no-op (must SURVIVE)", WORLD,
     "pub fn sort_suggestions(", "pub fn sort_suggestions(", TEST_WORLD),
    (4, "sortSuggestions: lower-case both sides", WORLD,
     "if s.text.starts_with(&last_word) || s.text.starts_with(&namespaced) {",
     "if s.text.to_lowercase().starts_with(&last_word) || s.text.to_lowercase().starts_with(&namespaced) {", TEST_WORLD),
    (4, "getLastWordIndex: stop at the first whitespace char", WORLD,
     "            result = j;\n            i = j;", "            result = i + 1;\n            i = j;", TEST_WORLD),
    (4, "tab: cycle on the first press too", WORLD,
     "                if self.tab_cycles {\n                    self.cycle(if input.has_shift() { -1 } else { 1 }, edit, cfg);\n                }",
     "                self.cycle(if input.has_shift() { -1 } else { 1 }, edit, cfg);", TEST_WORLD),
    (4, "useSuggestion: put the caret at the end of the field", WORLD,
     "        let end = suggestion.range.start + suggestion.text.encode_utf16().count();",
     "        let end = edit.len();", TEST_WORLD),

    # --- batch 5: the popup's gates --------------------------------------
    (5, "CONTROL no-op (must SURVIVE)", WORLD,
     "pub fn is_cycle_focus(", "pub fn is_cycle_focus(", TEST_WORLD),
    (5, "keyPressed: honour allowHiding as if it were true", WORLD,
     "        if !is_cycle_focus(input) || (self.allow_hiding && !visible) {",
     "        if !is_cycle_focus(input) || !visible {", TEST_WORLD),
    (5, "updateCommandInfo: treat a whitespace-only field as a message", WORLD,
     "        if value.trim().is_empty() {", "        if value.is_empty() {", TEST_WORLD),
    (5, "hover: re-select even when the mouse did not move", WORLD,
     "        if !moved {\n            return;\n        }", "", TEST_WORLD),
    (5, "showSuggestions: open on an empty set", WORLD,
     "        if pending.is_empty() {\n            return;\n        }", "", TEST_WORLD),

    # --- batch 6: the wire ------------------------------------------------
    (6, "CONTROL no-op (must SURVIVE)", NET,
     "pub fn read_custom_chat_completions(", "pub fn read_custom_chat_completions(", TEST_NET),
    (6, "toSuggestions: route through Suggestions::create", NET,
     "        Suggestions {\n            range,\n            list: self",
     "        return Suggestions::create(&[], self", TEST_NET),
    (6, "SET: add rather than replace", NET,
     "            CompletionAction::Set => {\n                self.custom.clear();",
     "            CompletionAction::Set => {\n                {}", TEST_NET),
    (6, "complete: accept any reply, not just the outstanding one", NET,
     "        if self.pending != Some(reply.id) {\n            return None;\n        }\n        self.pending = None;",
     "        self.pending = None;", TEST_NET),
    (6, "an out-of-range action ordinal defaults to ADD", NET,
     "        other => {\n            return Err(ProtoError::Frame(format!(\n                \"custom_chat_completions action ordinal {other} out of range\"\n            )))\n        }",
     "        _ => CompletionAction::Add,", TEST_NET),
]


def run(cmd):
    r = subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True, encoding="utf-8", errors="replace")
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
        control = label.startswith("CONTROL")
        if control:
            verdict = "OK (control survived)" if passed else "!! CONTROL DIED -- instrument broken"
            if not passed:
                broken += 1
        else:
            verdict = "SURVIVED  <-- investigate" if passed else "killed"
            if passed:
                surviving += 1
            else:
                killed += 1
        print(f"[batch {batch}] {verdict}: {label}")
    print(f"\nkilled {killed}, survived {surviving}, broken {broken}")
    return 1 if (surviving or broken) else 0


if __name__ == "__main__":
    sys.exit(main())
