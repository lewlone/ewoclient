"""Does `tablistshot` see the tab list's hearts and faces at all?

Run: python tools/m158_gap.py

M155 shipped `tab_list_view::hearts` and `tab_list_view::faces`. Each has
exactly ONE production caller (`live_cmd.rs`), and `tablistshot` calls
neither — it builds its icon list from `icons()` alone. This asks the only
question that settles it: delete each emitter's body and see whether the gate
notices.

Discipline per AGENT_LOOP_BRIEF: a no-op control that must SURVIVE, exit codes
rather than substrings, and — because this battery is GATE-routed — a REBUILD
after every restore, or the next run grades the previous mutant's binary
against a clean tree.
"""

import io
import os
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
VIEW = os.path.join(ROOT, "crates/rewo-app/src/tab_list_view.rs")
EXE = os.path.join(ROOT, "target", "debug", "rewo.exe")

HEARTS_ANCHOR = """    gui_tick: i64,
) -> Vec<rewo_gpu::hud::HudBlit> {
    let mut out = Vec::new();"""

FACES_ANCHOR = """    loaded_of: &dyn Fn(u128) -> bool,
) -> Vec<rewo_gpu::hud::HudBlit> {
    let mut out = Vec::new();"""

MUTATIONS = [
    ("control: no change", HEARTS_ANCHOR, HEARTS_ANCHOR),
    (
        "hearts(): emit nothing at all",
        HEARTS_ANCHOR,
        HEARTS_ANCHOR.replace(
            "    let mut out = Vec::new();",
            "    if true { return Vec::new(); }\n    let mut out = Vec::new();",
        ),
    ),
    (
        "faces(): emit nothing at all",
        FACES_ANCHOR,
        FACES_ANCHOR.replace(
            "    let mut out = Vec::new();",
            "    if true { return Vec::new(); }\n    let mut out = Vec::new();",
        ),
    ),
]


def build():
    p = subprocess.run(
        ["cargo", "build", "-p", "rewo-app"],
        cwd=ROOT,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        timeout=900,
    )
    return p.returncode == 0


def gate():
    """True == the gate PASSED (so the mutation survived)."""
    try:
        p = subprocess.run(
            [EXE, "tablistshot", "--check"],
            cwd=ROOT,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=420,
        )
    except subprocess.TimeoutExpired:
        return False, "TIMEOUT (counted as killed)"
    # Require the summary line, not a substring of the body: M85's rule, so a
    # panic cannot read as a pass.
    if "[tablistshot] PASS" not in p.stdout:
        return False, f"no PASS line (exit {p.returncode})"
    return p.returncode == 0, f"exit {p.returncode}"


def main():
    results = []
    for name, find, repl in MUTATIONS:
        original = io.open(VIEW, encoding="utf-8", newline="").read()
        if original.count(find) != 1:
            print(f"SKIP      {name}: anchor matched {original.count(find)} times")
            results.append((name, "SKIP"))
            continue
        try:
            io.open(VIEW, "w", encoding="utf-8", newline="").write(
                original.replace(find, repl)
            )
            if not build():
                survived, reason = False, "build failed"
            else:
                survived, reason = gate()
        finally:
            io.open(VIEW, "w", encoding="utf-8", newline="").write(original)
            assert io.open(VIEW, "rb").read() == original.encode("utf-8"), (
                "RESTORE FAILED — mutation may be left on disk"
            )
            # GATE-routed: restoring the source does not rebuild the binary.
            build()
        verdict = "SURVIVED" if survived else "KILLED"
        print(f"{verdict:9} {name}  ({reason})")
        results.append((name, verdict))

    print()
    if results[0][1] != "SURVIVED":
        print("BATTERY INVALID: the no-op control did not survive.")
        return 2
    print("control SURVIVED (battery is valid)")
    for name, verdict in results[1:]:
        print(f"  {verdict}: {name}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
