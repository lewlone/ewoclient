"""M178's mutation battery - the advancements render half.

    python tools/m178_mutate.py [lo] [hi]

Checker per mutation (M158's rule): `gate` = `rewo advshot --check`, which
drives the production build/chrome/lines/icon_draws through the real screen
pass and grades 11 witnesses. The no-op control must SURVIVE; restore is
verified by BYTES; a timeout is a KILL.
"""

import io
import os
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
EXE = os.path.join(ROOT, "target", "debug", "rewo.exe")
VIEW = os.path.join(ROOT, "crates", "rewo-app", "src", "advancements_view.rs")

STRAYS = ("rewo.exe",)

MUTATIONS = [
    ("control: no change", VIEW,
     "pub fn background_index(path: &str) -> Option<u8> {",
     "pub fn background_index(path: &str) -> Option<u8> {",
     "MUST SURVIVE"),

    # ---- the model half ---------------------------------------------------
    ("open stops auto-selecting the first tab", VIEW,
     """        if !screen.tabs.is_empty() {
            screen.select(Some(0));
        }""",
     """        if false {
            screen.select(Some(0));
        }""",
     "init() selects the FIRST tab when nothing is remembered - without it the window renders empty"),

    # ---- the chrome -------------------------------------------------------
    ("the scissor rect leaks past the inside area", VIEW,
     "rect: (in_x, in_y, asm::INSIDE_W, asm::INSIDE_H),",
     "rect: (in_x, in_y - 20, asm::INSIDE_W, asm::INSIDE_H + 45),",
     "enableScissor bounds ARE the contents rect - leaking spills tiles past the window edge"),
    ("the connectivity passes run white-under-black", VIEW,
     "    for bg in [true, false] {",
     "    for bg in [false, true] {",
     "extractContents draws the BLACK underlay first, then the WHITE core over it"),
    ("the fade overlay loses its black tint", VIEW,
     "color: [0.0, 0.0, 0.0, tab.fade],",
     "color: [1.0, 1.0, 1.0, tab.fade],",
     "extractTooltips fills BLACK at the fade alpha (`fill(.., fade<<24)`), not white"),
    ("hidden widgets draw their frames anyway", VIEW,
     """        if !w.visible {
            continue;
        }
        batch.sprites.push(SpriteDraw {
            x: in_x + sx + w.x + 3,""",
     """        if false {
            continue;
        }
        batch.sprites.push(SpriteDraw {
            x: in_x + sx + w.x + 3,""",
     "a hidden advancement renders NOTHING until done"),

    # ---- text -------------------------------------------------------------
    ("the window title line is dropped", VIEW,
     """            push(
                &mut out,
                &tab.title,
                win_x + asm::TITLE_X,
                win_y + asm::TITLE_Y,
                0x40_4040,
                false,
            );""",
     """            let _ = (&mut out, &tab.title, win_x, win_y);""",
     "extractWindow draws the selected tab's title at (leftPos+8, topPos+6)"),

    # ---- icons ------------------------------------------------------------
    ("visible widgets stop owing icons", VIEW,
     """    for w in &tab.widgets {
        if !w.visible {
            continue;
        }
        out.push(IconDraw {""",
     """    for w in &tab.widgets {
        if !w.visible || true {
            continue;
        }
        out.push(IconDraw {""",
     "fakeItem draws every visible widget's icon"),
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
    code, out = run([EXE, "advshot", "--check"], 900)
    if code is None:
        return None, "TIMEOUT"
    if code != 0:
        return 1, out.strip().splitlines()[-1][:140] if out.strip() else "failed"
    if "14 / 14" not in out:
        return 1, "witness count short of 14/14"
    return 0, "ok"


def main():
    lo = int(sys.argv[1]) if len(sys.argv) > 1 else 1
    hi = int(sys.argv[2]) if len(sys.argv) > 2 else len(MUTATIONS)
    selected = [MUTATIONS[0]] + MUTATIONS[lo:hi]
    print(f"[m178] slice [{lo}, {hi}) - {len(selected) - 1} mutations + control")
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
    print(f"[m178] {killed} killed, control {'ok' if ctrl_ok else 'FAILED'}, "
          f"survivors: {survived}, problems: {bad}")


if __name__ == "__main__":
    main()
