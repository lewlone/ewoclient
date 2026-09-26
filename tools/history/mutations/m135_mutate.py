"""M135's mutation battery — do the new witnesses actually hold the fix down?

Usage:  python tools/m135_mutate.py a     (or: b)

Split into two batteries so each finishes inside the 10-minute tool cap. A
battery that is killed part-way leaves its mutation ON DISK, so this restores in
a `finally` AND asserts a clean `git diff` at the end; if it ever reports a dirty
tree, run `git checkout --` on the named file before anything else.

Every battery opens with a BASELINE (unmutated, must pass) and carries a NO-OP
CONTROL that must SURVIVE. Without the control, a battery run against an
already-broken tree reports KILLED for every entry and reads as a clean sweep —
which is how eight vacuous mutations passed for real during the chat arc.
"""
import io
import os
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
LIVE = os.path.join("crates", "rewo-app", "src", "live_cmd.rs")

# (name, file, old, new, expected_verdict)
BATTERIES = {
    "a": [
        (
            "CONTROL: a comment-only edit (must SURVIVE)",
            LIVE,
            "/// 960x720 gives `min(720/240, 960/320) = 3`, so these run at the GUI scale",
            "/// 960 x 720 gives `min(720/240, 960/320) = 3`, so these run at the GUI scale",
            "SURVIVED",
        ),
        (
            "hud_fills: reintroduce the double scale",
            LIVE,
            "    let chat_gui = opts.scale as f32;\n    chat.visible_lines(",
            "    let chat_gui = px * opts.scale as f32;\n    chat.visible_lines(",
            "KILLED",
        ),
        (
            "hud_fills: over-correct, dropping the chat scale too",
            LIVE,
            "    let chat_gui = opts.scale as f32;\n    chat.visible_lines(",
            "    let chat_gui = 1.0;\n    chat.visible_lines(",
            "KILLED",
        ),
        (
            "chat_scrollbar: reintroduce the double scale",
            LIVE,
            "    let chat_gui = opts.scale as f32;\n    chat.scrollbar(",
            "    let chat_gui = px * opts.scale as f32;\n    chat.scrollbar(",
            "KILLED",
        ),
    ],
    "b": [
        (
            "CONTROL: a comment-only edit (must SURVIVE)",
            LIVE,
            "/// 960x720 gives `min(720/240, 960/320) = 3`, so these run at the GUI scale",
            "/// 960 x 720 gives `min(720/240, 960/320) = 3`, so these run at the GUI scale",
            "SURVIVED",
        ),
        (
            "chat_input_backdrop: scale it again with a literal",
            LIVE,
            "        h: h as f32,\n        alpha: rewo_world::chat_screen::INPUT_BACKDROP_ALPHA,",
            "        h: h as f32 * 3.0,\n        alpha: rewo_world::chat_screen::INPUT_BACKDROP_ALPHA,",
            "KILLED",
        ),
        (
            "suggestion_popup_fills: scale it again with a literal",
            LIVE,
            "        h: h as f32,\n        alpha: ((argb >> 24) & 0xFF) as f32 / 255.0,",
            "        h: h as f32 * 3.0,\n        alpha: ((argb >> 24) & 0xFF) as f32 / 255.0,",
            "KILLED",
        ),
        (
            "gui_px: stop delegating and answer 1.0",
            LIVE,
            "    rewo_gpu::hud::gui_scale(w as f32, h as f32)\n}",
            "    let _ = (w, h);\n    1.0\n}",
            "KILLED",
        ),
        (
            "the witness's own model of the pass: drop the multiply",
            LIVE,
            "        (f.x * px, f.y * px, f.w * px, f.h * px)",
            "        let _ = px;\n        (f.x, f.y, f.w, f.h)",
            "KILLED",
        ),
    ],
}


def run_tests():
    """rewo-app is a BINARY crate, so --bins. Exit code is the verdict, never a
    substring of the output: a run that fails to COMPILE prints no
    'test result' line at all and a grep for 'ok' reads it as silence."""
    p = subprocess.run(
        ["cargo", "test", "-p", "rewo-app", "--bins"],
        cwd=ROOT,
        capture_output=True,
    )
    return p.returncode


def main():
    which = (sys.argv[1] if len(sys.argv) > 1 else "a").lower()
    muts = BATTERIES[which]

    snapshots = {rel: io.open(os.path.join(ROOT, rel), "rb").read()
                 for _, rel, _, _, _ in muts}

    print("BASELINE (unmutated) ...", end=" ", flush=True)
    if run_tests() != 0:
        sys.exit("BASELINE FAILS — every verdict below would be meaningless")
    print("pass")

    results = []
    for name, rel, old, new, want in muts:
        path = os.path.join(ROOT, rel)
        original = io.open(path, "rb").read()
        text = original.decode("utf-8")
        n = text.count(old)
        if n != 1:
            results.append((name, "ANCHOR x%d" % n, want, False))
            print("%-52s ANCHOR MATCHED %d TIMES" % (name[:52], n))
            continue
        try:
            io.open(path, "wb").write(text.replace(old, new).encode("utf-8"))
            verdict = "KILLED" if run_tests() != 0 else "SURVIVED"
        finally:
            io.open(path, "wb").write(original)
        ok = verdict == want
        results.append((name, verdict, want, ok))
        print("%-52s %-9s (want %-9s) %s" % (name[:52], verdict, want, "ok" if ok else "<<< UNEXPECTED"))

    # Bytes, not `git diff --quiet`: that cannot tell a leftover mutation from
    # uncommitted work, so it cries wolf on every dirty tree. Found by running
    # M138a's copy of this harness before its own work was committed.
    leftover = [rel for _, rel, _, _, _ in muts
                if io.open(os.path.join(ROOT, rel), "rb").read() != snapshots[rel]]
    dirty = 1 if leftover else 0
    print("-----")
    print("files restored: %s" % ("yes" if not leftover else "NO -- MUTATED: %s" % leftover))
    bad = [r for r in results if not r[3]]
    print("battery %s: %d/%d as expected" % (which, len(results) - len(bad), len(results)))
    sys.exit(1 if (bad or dirty != 0) else 0)


if __name__ == "__main__":
    main()
