"""M180's mutation battery - the written-book page-text clicks.

    python tools/m180_mutate.py [lo] [hi]

Two instruments per mutation. `cargo test -p rewo-world --lib -- book_view`
reaches the layout/click/force_page units directly (an agreement witness
cannot see a mutation that moves BOTH sides - the pen-advance mutant leaves
m6 green while every span sits wrong). `rewo bookshot --check` drives the
production builders with the real advance table and pins the
renderer/walk agreement. Rebuilds inside the checker; control must SURVIVE;
restore verified by BYTES.
"""

import io
import os
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
EXE = os.path.join(ROOT, "target", "debug", "rewo.exe")
BOOK = os.path.join(ROOT, "crates", "rewo-world", "src", "book_view_screen.rs")
LIVE = os.path.join(ROOT, "crates", "rewo-app", "src", "live_cmd.rs")

STRAYS = ("rewo.exe",)

MUTATIONS = [
    ("control: no change", BOOK,
     "pub fn background_left(width: i32) -> i32 {",
     "pub fn background_left(width: i32) -> i32 {",
     "MUST SURVIVE"),

    # ---- the hit-test ------------------------------------------------------
    ("the click rect swallows its right edge", BOOK,
     ".find(|s| mx >= s.x && mx < s.x + s.w && my >= s.y && my < s.y + LINE_HEIGHT)",
     ".find(|s| mx >= s.x && mx <= s.x + s.w && my >= s.y && my < s.y + LINE_HEIGHT)",
     "isPointInRectangle is x < right (ActiveTextCollector.java) - adjacent spans would double-fire; m8"),
    ("the click rect drops its top edge", BOOK,
     ".find(|s| mx >= s.x && mx < s.x + s.w && my >= s.y && my < s.y + LINE_HEIGHT)",
     ".find(|s| mx >= s.x && mx < s.x + s.w && my > s.y && my < s.y + LINE_HEIGHT)",
     "y >= top - the line's own top row is inside; unit test"),
    ("the hit-test stops asking for events", BOOK,
     ".find(|s| mx >= s.x && mx < s.x + s.w && my >= s.y && my < s.y + LINE_HEIGHT)",
     ".filter(|s| s.span.click().is_some())\n        .find(|s| mx >= s.x && mx < s.x + s.w && my >= s.y && my < s.y + LINE_HEIGHT)",
     "PROVEN EQUIVALENT and removed - disjoint half-open rects plus and_then already decline plain spans; kept as the regression pin"),

    # ---- force_page --------------------------------------------------------
    ("force_page forgets the floor", BOOK,
     "        let clamped = page.max(0).min(self.page_count() as i32 - 1);",
     "        let clamped = page;",
     "Mth.clamp(page, 0, pageCount - 1) floors AND caps - a negative page must not wrap usize; unit test"),

    # ---- the layout walk ---------------------------------------------------
    ("the pen stops advancing between spans", BOOK,
     """            out.push(LaidSpan {
                span,
                x: pen,
                y: ty + i as i32 * LINE_HEIGHT,
                w,
            });
            pen += w;""",
     """            out.push(LaidSpan {
                span,
                x: pen,
                y: ty + i as i32 * LINE_HEIGHT,
                w,
            });""",
     "spans lay end to end - a stalled pen stacks them at one x; unit test"),

    # ---- force_page --------------------------------------------------------
    ("force_page forgets the upper clamp", BOOK,
     "        let clamped = page.max(0).min(self.page_count() as i32 - 1);",
     "        let clamped = page.max(0);",
     "Mth.clamp(page, 0, pageCount - 1) caps at the last page; unit test"),
    ("force_page always reports a change", BOOK,
     "        let changed = clamped >= 0 && clamped != self.current_page as i32;",
     "        let changed = clamped >= 0;",
     "setPage compares AFTER clamping - a same-page turn answers false; unit test"),

    # ---- the shared walk ---------------------------------------------------
    ("the renderer drifts off the click walk", LIVE,
     """        out.push(rewo_gpu::world::OwnedTextLine {
            x: ls.x as f32 * px,
            y: ls.y as f32 * px,""",
     """        out.push(rewo_gpu::world::OwnedTextLine {
            x: ls.x as f32 * px + 3.0,
            y: ls.y as f32 * px,""",
     "book_text_lines must read layout_spans verbatim - this is the drift m6 exists to catch"),
]


def reap():
    for pat in STRAYS:
        subprocess.run(["taskkill", "/F", "/IM", pat], capture_output=True)


def run(cmd, timeout):
    try:
        p = subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True,
                           encoding="utf-8", errors="replace", timeout=timeout)
    except subprocess.TimeoutExpired:
        reap()
        return None, ""
    return p.returncode, p.stdout + p.stderr


def build():
    reap()
    code, _ = run(["cargo", "build", "-p", "rewo-app"], 900)
    return code == 0


def gate():
    reap()
    if not build():
        return 1, "BUILD FAILED"
    code, out = run(["cargo", "test", "-q", "-p", "rewo-world", "--lib", "--",
                     "book_view"], 600)
    if code is None:
        return None, "TIMEOUT"
    if code != 0:
        return 1, "unit tests red"
    code, out = run([EXE, "bookshot", "--check"], 900)
    if code is None:
        return None, "TIMEOUT"
    if code != 0:
        return 1, out.strip().splitlines()[-1][:140] if out.strip() else "failed"
    if "24 witnesses" not in out:
        return 1, "witness count short of 24"
    return 0, "ok"


def main():
    lo = int(sys.argv[1]) if len(sys.argv) > 1 else 1
    hi = int(sys.argv[2]) if len(sys.argv) > 2 else len(MUTATIONS)
    selected = [MUTATIONS[0]] + MUTATIONS[lo:hi]
    print(f"[m180] slice [{lo}, {hi}) - {len(selected) - 1} mutations + control")
    results = []
    for name, path, find, repl, why in selected:
        original = io.open(path, encoding="utf-8", newline="").read()
        n = original.count(find)
        if n != 1:
            print(f"SKIP      {name}: anchor matched {n} times")
            results.append((name, "SKIP", why))
            continue
        is_control = name.startswith("control")
        verdict, reason = None, ""
        try:
            io.open(path, "w", encoding="utf-8", newline="").write(original.replace(find, repl, 1))
            code, r = gate()
            if code is None:
                verdict, reason = "TIMEOUT", "gate timed out"
            elif code != 0:
                verdict = "KILLED"
                reason = f"killed by gate ({r})"
            else:
                reason = "gate green"
            if verdict is None:
                killed = reason.startswith("killed")
                verdict = ("SURVIVED" if not killed else "FAILED") if is_control else \
                          ("KILLED" if killed else "SURVIVED")
        finally:
            with io.open(path, "w", encoding="utf-8", newline="") as f:
                f.write(original)
            after = io.open(path, encoding="utf-8", newline="").read()
            if after != original:
                print(f"RESTORE FAILED for {path} - STOPPING THE BATTERY")
                sys.exit(2)
        results.append((name, verdict))
        print(f"{verdict:9} {name}")
        if verdict == "KILLED":
            print(f"          {why[:110]}")
    killed = sum(1 for _, v in results if v == "KILLED")
    survived = [n for n, v in results if v == "SURVIVED"]
    bad = [n for n, v in results if v in ("FAILED", "TIMEOUT", "SKIP")]
    # Leave the TREE's binary on disk, not the last mutant's (2026-08-25:
    # a post-battery gate sweep graded the final restore's stale exe and
    # only the drifted-against witness went red).
    if not build():
        print(f'[{tag}] FINAL REBUILD FAILED - the exe does not match the tree')
        sys.exit(2)
    ctrl_ok = any(n.startswith("control") and v == "SURVIVED" for n, v in results)
    print(f"[m180] {killed} killed, control {'ok' if ctrl_ok else 'FAILED'}, "
          f"survivors: {survived}, problems: {bad}")


if __name__ == "__main__":
    main()
