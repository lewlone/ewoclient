"""M171's mutation battery — the written-book decode + screen model.

    python tools/m171_mutate.py [lo] [hi]

The decode (page capture + alignment) is graded by the `rewo-net` unit test; the
`BookViewScreen` model (navigation, button visibility, layout, the 14-line cap)
by the `rewo-world` unit tests. The render is deferred to M172, so nothing here
touches a GPU. `reap()` before every build; a timeout is a KILL; restore
verified by bytes; the no-op control must SURVIVE.
"""

import io
import os
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
ITEM = os.path.join(ROOT, "crates/rewo-net/src/item_stack.rs")
MODEL = os.path.join(ROOT, "crates/rewo-world/src/book_view_screen.rs")

STRAYS = ("rewo_net-*.exe", "rewo_world-*.exe")

MUTATIONS = [
    ("control: no change", MODEL,
     "pub const MAX_LINES: usize = 128 / 9; // 14", "pub const MAX_LINES: usize = 128 / 9; // 14",
     ["net", "world"], "MUST SURVIVE"),

    # ---- the model ------------------------------------------------------
    ("the page cap is off", MODEL,
     "pub const MAX_LINES: usize = 128 / 9; // 14", "pub const MAX_LINES: usize = 128; // wrong",
     ["world"], "a page shows Math.min(128 / 9, size) = 14 lines"),
    ("the forward button never hides", MODEL,
     "    pub fn forward_visible(&self) -> bool {\n        self.current_page + 1 < self.page_count()\n    }",
     "    pub fn forward_visible(&self) -> bool {\n        true\n    }",
     ["world"], "the forward button hides on the last page"),
    ("the back button is always visible", MODEL,
     "    pub fn back_visible(&self) -> bool {\n        self.current_page > 0\n    }",
     "    pub fn back_visible(&self) -> bool {\n        true\n    }",
     ["world"], "the back button hides on page 0"),
    ("page forward does not clamp", MODEL,
     "    pub fn page_forward(&mut self) {\n        if self.current_page + 1 < self.page_count() {\n            self.current_page += 1;\n        }\n    }",
     "    pub fn page_forward(&mut self) {\n        self.current_page += 1;\n    }",
     ["world"], "pageForward is capped at the last page"),
    ("the indicator floors at zero not one", MODEL,
     "        (self.current_page + 1, self.page_count().max(1))",
     "        (self.current_page + 1, self.page_count())",
     ["world"], "max(numPages, 1) — the indicator never shows 0"),
    ("arrow keys turn the page", MODEL,
     "            KEY_PAGE_UP => {\n                self.page_back();\n                true\n            }\n            KEY_PAGE_DOWN => {\n                self.page_forward();\n                true\n            }",
     "            KEY_PAGE_UP | 263 => {\n                self.page_back();\n                true\n            }\n            KEY_PAGE_DOWN | 262 => {\n                self.page_forward();\n                true\n            }",
     ["world"], "only PageUp/PageDown are bound, not the arrows"),
    ("a click ignores button visibility", MODEL,
     "        if self.forward_visible() && in_rect(mx, my, Self::forward_rect(width)) {",
     "        if in_rect(mx, my, Self::forward_rect(width)) {",
     ["world"], "a click on a hidden button does nothing"),
    ("the background is centred vertically", MODEL,
     "pub const BACKGROUND_TOP: i32 = 2;", "pub const BACKGROUND_TOP: i32 = 40;",
     ["world"], "backgroundTop is a fixed 2, not centred"),

    # ---- the decode -----------------------------------------------------
    ("the pages are not captured", ITEM,
     "            out.book_pages.push(tag);",
     "            let _ = tag;",
     ["net"], "the pages are the whole point of the capture"),
    ("the filtered page is not skipped", ITEM,
     "            out.book_pages.push(tag);\n            if r.bool().map_err(|_| ())? {\n                rewo_proto::nbt::Nbt::read_network(r).map_err(|_| ())?;\n            }",
     "            out.book_pages.push(tag);",
     ["net"], "each page is Filterable — the optional filtered NBT must be read or the walk desyncs"),
    ("the resolved bool is not read", ITEM,
     "        r.bool().map_err(|_| ())?; // resolved\n        return Ok(true);",
     "        return Ok(true);",
     ["net"], "the trailing bool must be read to stay aligned"),
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
    code, out = run(["cargo", "test", "-q", "-p", crate, "--lib", filt], 600)
    if code is None:
        return None, "TIMEOUT"
    if code != 0:
        return 1, "tests or build failed"
    if "test result:" not in out:
        return 1, "no test result line"
    return 0, "ok"


CHECKERS = {
    "net": lambda: unit("rewo-net", "written_book"),
    "world": lambda: unit("rewo-world", "book_view"),
}


def main():
    lo = int(sys.argv[1]) if len(sys.argv) > 1 else 1
    hi = int(sys.argv[2]) if len(sys.argv) > 2 else len(MUTATIONS)
    selected = [MUTATIONS[0]] + MUTATIONS[lo:hi]
    used = sorted({c for m in selected[1:] for c in m[4]}) or ["world"]
    print(f"[m171] slice [{lo}, {hi}) — {len(selected) - 1} mutations + control; checkers {used}")
    results = []
    for name, path, find, repl, checkers, why in selected:
        original = io.open(path, encoding="utf-8", newline="").read()
        n = original.count(find)
        if n != 1:
            print(f"SKIP      {name}: anchor matched {n} times")
            results.append((name, "SKIP", why))
            continue
        survived, reason = True, ""
        try:
            io.open(path, "w", encoding="utf-8", newline="").write(original.replace(find, repl, 1))
            run_checkers = used if name.startswith("control") else checkers
            for c in run_checkers:
                code, r = CHECKERS[c]()
                if code is None:
                    survived, reason = False, f"{c} TIMEOUT"
                    break
                if code != 0:
                    survived, reason = False, f"{c} exit {code}"
                    break
                reason = (reason + " " + f"{c} ok").strip()
        finally:
            io.open(path, "w", encoding="utf-8", newline="").write(original)
            assert io.open(path, "rb").read() == original.encode("utf-8"), f"RESTORE FAILED {path}"
        verdict = "SURVIVED" if survived else "KILLED"
        print(f"{verdict:9} {name}  ({reason})", flush=True)
        results.append((name, verdict, why))

    print()
    if not results or results[0][1] != "SURVIVED":
        print("BATTERY INVALID: the no-op control did not survive.")
        return 2
    killed = sum(1 for _, v, _ in results[1:] if v == "KILLED")
    total = len(results) - 1
    print(f"control SURVIVED (battery is valid) - {killed}/{total} killed")
    for name, verdict, why in results[1:]:
        if verdict != "KILLED":
            print(f"  {verdict}: {name}\n    would mean: {why}")
    return 0 if killed == total else 1


if __name__ == "__main__":
    sys.exit(main())
