"""M179's mutation battery - the advancement-click half.

    python tools/m179_mutate.py [lo] [hi]

Two instruments per mutation (M111's rule: run a mutation against the check
that covers it). `gate` rebuilds (M178's stale-binary trap), then runs
`cargo test -p rewo-app --bins advancements_view` (the drag machine lives
ONLY there - advshot never drives AdvDrag) AND `rewo advshot --check`
(clicks, wheel and drag scaling). The no-op control must SURVIVE; restore is
verified by BYTES; a timeout is a KILL.
"""

import io
import os
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
EXE = os.path.join(ROOT, "target", "debug", "rewo.exe")
VIEW = os.path.join(ROOT, "crates", "rewo-app", "src", "advancements_view.rs")
LIVE = os.path.join(ROOT, "crates", "rewo-app", "src", "live_cmd.rs")
MODEL = os.path.join(ROOT, "crates", "rewo-world", "src", "advancements_screen.rs")

STRAYS = ("rewo.exe",)

MUTATIONS = [
    ("control: no change", VIEW,
     "pub fn background_index(path: &str) -> Option<u8> {",
     "pub fn background_index(path: &str) -> Option<u8> {",
     "MUST SURVIVE"),

    # ---- clicks ------------------------------------------------------------
    ("tab_click refuses single-tab screens again", VIEW,
     """    pub fn tab_click(&self, gui_w: i32, gui_h: i32, mx: f64, my: f64) -> Option<usize> {
        let (xo, yo) = asm::window_origin(gui_w, gui_h);""",
     """    pub fn tab_click(&self, gui_w: i32, gui_h: i32, mx: f64, my: f64) -> Option<usize> {
        if self.screen.tabs.len() <= 1 {
            return None;
        }
        let (xo, yo) = asm::window_origin(gui_w, gui_h);""",
     "mouseClicked has NO size guard (AdvancementsScreen.java:113-127) - only the DRAW does (:206); m8"),
    ("tab cells answer inclusive edges", MODEL,
     "        mx > (xo + x) as f64",
     "        mx >= (xo + x) as f64",
     "isMouseOver is strict on every edge (AdvancementTabType.java:157); m7"),
    ("a click reports nothing (and so sends nothing)", VIEW,
     """    pub fn tab_click_report(&mut self, gui_w: i32, gui_h: i32, mx: f64, my: f64) -> Option<String> {
        let i = self.tab_click(gui_w, gui_h, mx, my)?;""",
     """    pub fn tab_click_report(&mut self, gui_w: i32, gui_h: i32, mx: f64, my: f64) -> Option<String> {
        return None;
        #[allow(unreachable_code)]
        let i = self.tab_click(gui_w, gui_h, mx, my)?;""",
     "the handler's whole decision lives in tab_click_report - killing it kills select AND send; m9"),

    # ---- wheel -------------------------------------------------------------
    ("the wheel loses its x16", VIEW,
     "        tab.scroll(dx * asm::SCROLL_SPEED, dy * asm::SCROLL_SPEED);",
     "        tab.scroll(dx, dy);",
     "mouseScrolled scales by SCROLL_SPEED = 16 (AdvancementsScreen.java:185); m11"),
    ("the scroll clamp bounds flip sign", MODEL,
     """            let lo = -((self.max_x - INSIDE_W) as f64);
            self.scroll_x = java_clamp(self.scroll_x + dx, lo, 0.0);""",
     """            let lo = (self.max_x - INSIDE_W) as f64;
            self.scroll_x = java_clamp(self.scroll_x + dx, lo, 0.0);""",
     "the lower bound is -(maxX - 234); flipped, java_clamp answers min>max with +46; m11"),

    # ---- drag --------------------------------------------------------------
    ("drag_scroll picks up the wheel's x16", VIEW,
     """        if let Some(tab) = self.screen.tabs.get_mut(sel) {
            tab.scroll(dx, dy);
        }
    }

    /// The scroll a tab change reports""",
     """        if let Some(tab) = self.screen.tabs.get_mut(sel) {
            tab.scroll(dx * asm::SCROLL_SPEED, dy * asm::SCROLL_SPEED);
        }
    }

    /// The scroll a tab change reports""",
     "mouseDragged passes RAW deltas - SCROLL_SPEED belongs to mouseScrolled alone (:170 vs :185); m12"),
    ("the dead first drag event delivers anyway", VIEW,
     """        if !self.latched {
            // The dead first event.
            self.latched = true;
            return None;
        }
        Some((dx, dy))""",
     """        self.latched = true;
        Some((dx, dy))""",
     "AdvancementsScreen.java:167-171: the first drag event ONLY flips isScrolling; unit test"),
    ("a non-left held button stops cancelling the latch", VIEW,
     """        if button != 0 {
            // mouseDragged's early-out: `isScrolling = false; return false`.
            self.latched = false;
            return None;
        }""",
     """        if button != 0 {
            return None;
        }""",
     "mouseDragged's non-zero arm CLEARS isScrolling (:162-165); unit test"),
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
    code, out = run(["cargo", "test", "-q", "-p", "rewo-app", "--bins", "--",
                     "advancements_view"], 600)
    if code is None:
        return None, "TIMEOUT"
    if code != 0:
        return 1, "unit tests red"
    code, out = run([EXE, "advshot", "--check"], 900)
    if code is None:
        return None, "TIMEOUT"
    if code != 0:
        return 1, out.strip().splitlines()[-1][:140] if out.strip() else "failed"
    if "20 / 20" not in out:
        return 1, "witness count short of 20/20"
    return 0, "ok"


def main():
    lo = int(sys.argv[1]) if len(sys.argv) > 1 else 1
    hi = int(sys.argv[2]) if len(sys.argv) > 2 else len(MUTATIONS)
    selected = [MUTATIONS[0]] + MUTATIONS[lo:hi]
    print(f"[m179] slice [{lo}, {hi}) - {len(selected) - 1} mutations + control")
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
    ctrl_ok = any(n.startswith("control") and v == "SURVIVED" for n, v in results)
    print(f"[m179] {killed} killed, control {'ok' if ctrl_ok else 'FAILED'}, "
          f"survivors: {survived}, problems: {bad}")


if __name__ == "__main__":
    main()
