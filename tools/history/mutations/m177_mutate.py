"""M177's mutation battery - the advancement decode, tree state, screen model.

    python tools/m177_mutate.py [lo] [hi]

Checkers per mutation (M158's rule): `net` = the `advancements`-filtered
rewo-net tests (decode + ClientAdvancements), `world` = the
`advancements`-filtered rewo-world tests (the screen model). Restore verified
by BYTES; the no-op control must SURVIVE; a timeout is a KILL.
"""

import io
import os
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
NET = os.path.join(ROOT, "crates", "rewo-net", "src", "advancements.rs")
WORLD = os.path.join(ROOT, "crates", "rewo-world", "src", "advancements_screen.rs")

STRAYS = ("rewo.exe", "rewo_world-*.exe", "rewo_net-*.exe")

MUTATIONS = [
    ("control: no change", NET,
     "pub fn parse_update(body: &[u8]) -> Result<UpdateAdvancements, String> {",
     "pub fn parse_update(body: &[u8]) -> Result<UpdateAdvancements, String> {",
     ["net"], "MUST SURVIVE"),

    # ---- the wire decode --------------------------------------------------
    ("frame ordinal clamps to task instead of erroring", NET,
     'other => return Err(format!("advancement frame ordinal {other} out of range")),',
     'other => { let _ = other; return Ok(Frame::Task); }',
     ["net"], "readEnum out-of-range THROWS in vanilla; a silent default mislabels every challenge"),
    ("flags bit 1 is read as bit 0", NET,
     "let show_toast = flags & 2 != 0;",
     "let show_toast = flags & 1 != 0;",
     ["net"], "bit 0 is BACKGROUND-PRESENT - reading toast from it lights every rooted display"),
    ("the icon patch walk drops the removed count", NET,
     "crate::component_wire::read_item_template(r, 0)",
     "read_template_broken(r)",
     ["net"], "an untranscribed/short patch must abort the packet, not desync the reader"),

    # ---- the tree state ---------------------------------------------------
    ("insertion passes collapse to one pass", NET,
     """            remaining = deferred;
            if remaining.len() == before {""",
     """            remaining = deferred;
            if !remaining.is_empty() {
                log::error!("net: single-pass collapse");
                break;
            }
            if remaining.len() == before {""",
     ["net"], "a server may list a child BEFORE its root - one pass drops real advancements"),
    ("unknown-progress ids are stored anyway", NET,
     """                None => log::warn!(
                    "net: server informed client about progress for unknown advancement {id}"
                ),""",
     """                None => {
                    self.progress.insert(id, p);
                }""",
     ["net"], "ClientAdvancements.update warns and DROPS progress for unknown ids"),
    ("progress reshape stops pruning unnamed criteria", NET,
     "self.criteria.retain(|(name, _)| named(name));",
     "let _ = named;",
     ["net"], "update() prunes criteria the requirements do not name before storing"),
    ("done requires ANY group instead of ALL", NET,
     """        requirements.iter().all(|group| {
            group
                .iter()
                .any(|name| self.criteria.iter().any(|(n, t)| n == name && t.is_some()))
        })""",
     """        requirements.iter().any(|group| {
            group
                .iter()
                .any(|name| self.criteria.iter().any(|(n, t)| n == name && t.is_some()))
        })""",
     ["net"], "AdvancementProgress.isDone is an AND over groups - the battery's survivor taught where this rule must live (one body, on Progress, with a witness)"),
    ("empty requirements become vacuously done", NET,
     """    pub fn is_done(&self, requirements: &[Vec<String>]) -> bool {
        if requirements.is_empty() {
            return false;
        }""",
     """    pub fn is_done(&self, requirements: &[Vec<String>]) -> bool {
        if false {
            return false;
        }""",
     ["net"], "AdvancementRequirements.test returns FALSE for an empty list (:64-66), not vacuous truth"),
    ("single-group advances would show a progress counter", NET,
     "        if total <= 1 {\n            return None;\n        }",
     "        if total < 1 {\n            return None;\n        }",
     ["net"], "getProgressText suppresses the counter at total <= 1"),

    # ---- the state --------------------------------------------------------
    ("select_tab reports a change on every call", NET,
     "        if self.selected_tab.as_deref() != tab {",
     "        if true {\n            let _ = tab;\n",
     ["net"], "setSelectedTab fires the listener only when the selection CHANGED"),

    # ---- the screen model -------------------------------------------------
    ("widget bounds extend by the frame's 26 not 28x27", WORLD,
     "let (x1, y1) = (x + 28, y + 27);",
     "let (x1, y1) = (x + 26, y + 26);",
     ["world"], "addWidget's box IS 28x27 - centring and scrollability key off the larger box"),
    ("scroll uses Rust's clamp (panics when min>max)", WORLD,
     "self.scroll_x = java_clamp(self.scroll_x + dx, lo, 0.0);",
     "self.scroll_x = f64::clamp(self.scroll_x + dx, lo, 0.0);",
     ["world"], "-(maxX-234) can exceed 0 for small maxX; Java answers min, Rust panics"),
    ("hidden widgets become visible (and hoverable)", WORLD,
     "visible: !display.hidden || input.done,",
     "visible: true,",
     ["world"], "extractRenderState draws a hidden widget ONLY once done"),
    ("the connectivity core loses its vertical", WORLD,
     """                push_hline(&mut out, split_x, dep_x, dep_y);
                push_hline(&mut out, my_x, split_x, my_y);
                push_vline(&mut out, split_x, my_y, dep_y);""",
     """                push_hline(&mut out, split_x, dep_x, dep_y);
                push_hline(&mut out, my_x, split_x, my_y);""",
     ["world"], "the white core is three runs - dropping the elbow breaks every link"),
    ("hover boxes go exclusive like the tab strip's", WORLD,
     "mx >= x0 && mx <= x1 && my >= y0 && my <= y1",
     "mx > x0 && mx < x1 && my > y0 && my < y1",
     ["world"], "isMouseOver on the WIDGET is inclusive on all four edges"),
    ("the odd bar branch stops dimming the icon", WORLD,
     "        } else if raw > width - 2 {\n            (width / 2, true, true, false)\n",
     "        } else if raw > width - 2 {\n            (width / 2, true, false, false)\n",
     ["world"], "past width-2 vanilla keeps the BAR obtained but the ICON unobtained"),
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


def unit(crate, filt):
    reap()
    code, out = run(["cargo", "test", "-q", "-p", crate, "--lib", filt], 900)
    if code is None:
        return None, "TIMEOUT"
    if code != 0:
        return 1, "tests or build failed"
    if "test result:" not in out:
        return 1, "no test result line"
    return 0, "ok"


CHECKERS = {
    "net": lambda: unit("rewo-net", "advancements"),
    "world": lambda: unit("rewo-world", "advancements"),
}


def main():
    lo = int(sys.argv[1]) if len(sys.argv) > 1 else 1
    hi = int(sys.argv[2]) if len(sys.argv) > 2 else len(MUTATIONS)
    selected = [MUTATIONS[0]] + MUTATIONS[lo:hi]
    print(f"[m177] slice [{lo}, {hi}) - {len(selected) - 1} mutations + control")
    results = []
    for name, path, find, repl, checkers, why in selected:
        if find is None:
            print(f"SKIP      {name}")
            continue
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
            codes = []
            for c in checkers or []:
                code, r = CHECKERS[c]()
                codes.append((c, code, r))
            if any(code is None for _, code, _ in codes):
                verdict, reason = "TIMEOUT", "checker timed out"
            elif any(code != 0 for _, code, _ in codes):
                bad = next((c for c, code, _ in codes if code != 0), "?")
                verdict, reason = "KILLED", f"killed by {bad}"
            else:
                reason = "all checkers green"
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
        results.append((name, verdict, reason))
        print(f"{verdict:9} {name}")
        if verdict == "KILLED":
            print(f"          {why[:110]}")
    killed = sum(1 for _, v, _ in results if v == "KILLED")
    survived = [n for n, v, _ in results if v == "SURVIVED"]
    bad = [n for n, v, _ in results if v in ("FAILED", "TIMEOUT", "SKIP")]
    ctrl_ok = any(n.startswith("control") and v == "SURVIVED" for n, v, _ in results)
    print(f"[m177] {killed} killed, control {'ok' if ctrl_ok else 'FAILED'}, "
          f"survivors: {survived}, problems: {bad}")


if __name__ == "__main__":
    main()
