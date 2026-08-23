"""M172's mutation battery — the written-book reader's render.

    python tools/m172_mutate.py [lo] [hi]

Checkers per mutation (M158's rule): `gate` = `rewo bookshot --check` (the
pixel + model witnesses), `world` = the `book_view` unit tests, `net` = the
`book`-filtered rewo-net decode tests. The live r61 chain is not mutated here
(needs a server). `reap()` before every build (linker 1104); a timeout is a
KILL; restore byte-verified; the no-op control must SURVIVE.
"""

import io
import os
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
EXE = os.path.join(ROOT, "target", "debug", "rewo.exe")
LIVE = os.path.join(ROOT, "crates/rewo-app/src/live_cmd.rs")
MODEL = os.path.join(ROOT, "crates/rewo-world/src/book_view_screen.rs")
ITEM = os.path.join(ROOT, "crates/rewo-net/src/item_stack.rs")
SCREEN = os.path.join(ROOT, "crates/rewo-gpu/src/screen.rs")

STRAYS = ("rewo.exe", "rewo_world-*.exe", "rewo_net-*.exe", "rewo_gpu-*.exe")

MUTATIONS = [
    ("control: no change", MODEL,
     "pub const MENU_CONTROLS_TOP: i32 = BACKGROUND_TOP + IMAGE_H + 2; // 196",
     "pub const MENU_CONTROLS_TOP: i32 = BACKGROUND_TOP + IMAGE_H + 2; // 196",
     ["gate", "world", "net"], "MUST SURVIVE"),

    # ---- the resolve (fromItem) -----------------------------------------
    ("the written flag defers to non-empty pages", LIVE,
     "    if text.has_written_book {",
     "    if !text.book_pages.is_empty() {",
     ["gate"], "fromItem tries WRITTEN first — a zero-page written book opens EMPTY, not the draft"),
    ("the writable fallback is dropped", LIVE,
     "    } else if text.has_writable_book {",
     "    } else if false {",
     ["gate"], "a book-and-quill opens read-only through the same reader"),
    ("no component opens an empty book", LIVE,
     "        Some(\n            text.writable_pages\n                .iter()\n                .map(|page| wrap(&vec![base.clone().span(page.clone())]))\n                .collect(),\n        )\n    } else {\n        None\n    }",
     "        Some(\n            text.writable_pages\n                .iter()\n                .map(|page| wrap(&vec![base.clone().span(page.clone())]))\n                .collect(),\n        )\n    } else {\n        Some(Vec::new())\n    }",
     ["gate"], "handleOpenBook opens NOTHING when neither component is present"),
    ("the wrap is the chat wrapper", LIVE,
     "        rewo_world::string_splitter::split_lines_wrapped(spans, TEXT_WIDTH, &width_of)\n            .into_iter()\n            .map(|l| l.spans)\n            .collect::<Vec<_>>()",
     "        rewo_world::chat::wrap_components(spans, TEXT_WIDTH, &width_of)",
     ["gate"], "wrapComponents prepends the chat INDENT space to continuations"),

    # ---- the sprites -----------------------------------------------------
    ("the arrow directions swap", LIVE,
     "            sheet: Sheet::PageArrow(match (forward, highlighted) {\n                (true, false) => 0,\n                (true, true) => 1,\n                (false, false) => 2,\n                (false, true) => 3,\n            }),",
     "            sheet: Sheet::PageArrow(match (forward, highlighted) {\n                (true, false) => 2,\n                (true, true) => 3,\n                (false, false) => 0,\n                (false, true) => 1,\n            }),",
     ["gate"], "forward and backward are distinct 23x13 sprites"),
    ("hover never highlights", LIVE,
     "            sheet: Sheet::PageArrow(match (forward, highlighted) {\n                (true, false) => 0,\n                (true, true) => 1,\n                (false, false) => 2,\n                (false, true) => 3,\n            }),",
     "            sheet: Sheet::PageArrow(match (forward, highlighted) {\n                (true, _) => 0,\n                (false, _) => 2,\n            }),",
     ["gate"], "isHoveredOrFocused picks the _highlighted sprite"),

    # ---- the text --------------------------------------------------------
    ("the page text grows a shadow", LIVE,
     "                color_linear: srgb_bytes_to_linear_f(span.color),\n                alpha: 1.0,\n                shadow: false,\n                style: text_style_of(span),",
     "                color_linear: srgb_bytes_to_linear_f(span.color),\n                alpha: 1.0,\n                shadow: true,\n                style: text_style_of(span),",
     ["gate"], "PAGE_TEXT_STYLE is withoutShadow(); a shadow pushes the digit diff past the anchor"),
    ("the span colour is forced black", LIVE,
     "                color_linear: srgb_bytes_to_linear_f(span.color),\n                alpha: 1.0,\n                shadow: false,\n                style: text_style_of(span),",
     "                color_linear: [0.0, 0.0, 0.0],\n                alpha: 1.0,\n                shadow: false,\n                style: text_style_of(span),",
     ["gate"], "mergeStyles lets a page's own colour win — forcing black destroys styled pages"),
    ("the indicator anchors its LEFT edge", LIVE,
     "    let w = rewo_gpu::text::width_styled(&msg, advance, false);\n    out.push(rewo_gpu::world::OwnedTextLine {\n        x: (ax - w) as f32 * px,",
     "    let w = rewo_gpu::text::width_styled(&msg, advance, false);\n    let _ = w;\n    out.push(rewo_gpu::world::OwnedTextLine {\n        x: ax as f32 * px,",
     ["gate"], "TextAlignment.RIGHT is anchor - width; left-anchoring pushes into the margin"),
    ("the indicator substitutes sequentially", LIVE,
     "    let msg = match rewo_data::lang::decompose_template(template, args.len()) {\n        Some(parts) => parts\n            .into_iter()\n            .map(|p| match p {\n                rewo_data::lang::Part::Literal(t) => t.to_string(),\n                rewo_data::lang::Part::Arg(i) => args.get(i).cloned().unwrap_or_default(),\n            })\n            .collect::<String>(),",
     "    let msg = match Some(template) {\n        Some(t) => {\n            let mut out = String::new();\n            let mut rest = t;\n            for arg in &args {\n                match rest.split_once(\"%s\") {\n                    Some((h, tl)) => {\n                        out.push_str(h);\n                        out.push_str(arg);\n                        rest = tl;\n                    }\n                    None => break,\n                }\n            }\n            out.push_str(rest);\n            out\n        }",
     ["gate"], "book.pageIndicator is `Page %1$s of %2$s` — POSITIONAL; a plain-%s split renders the raw pattern"),

    # ---- the model / screen ---------------------------------------------
    ("the background centres vertically", MODEL,
     "pub const BACKGROUND_TOP: i32 = 2;",
     "pub const BACKGROUND_TOP: i32 = 24;",
     ["gate", "world"], "backgroundTop is the CONSTANT 2 — centring moves every button and line"),
    ("the Done button is dropped", MODEL,
     "        .with_widgets(vec![crate::screen::Widget::button(",
     "        .with_widgets(if false { vec![crate::screen::Widget::button(",
     None, "SKIP-SHAPED — replaced below"),
    ("the atlas collapses the arrows to one sheet", SCREEN,
     "        Sheet::PageArrow(i) => 26 + (i as usize).min(3),",
     "        Sheet::PageArrow(_i) => 26,",
     ["gate"], "four arrows, four placements — one index draws page_forward for everything"),

    # ---- the decode ------------------------------------------------------
    ("the writable filtered string is not skipped", ITEM,
     "            let page = r.string(32767).map_err(|_| ())?;\n            out.writable_pages.push(page);\n            if r.bool().map_err(|_| ())? {\n                r.string(32767).map_err(|_| ())?;\n            }",
     "            let page = r.string(32767).map_err(|_| ())?;\n            out.writable_pages.push(page);",
     ["net"], "each page is Filterable — skipping the optional desyncs the patch"),
    ("the writable presence flag is never set", ITEM,
     "        out.has_writable_book = true;\n        return Ok(true);\n    }",
     "        return Ok(true);\n    }",
     ["net", "gate"], "presence, not non-emptiness, is what fromItem's fallback tests"),
]

# The Done-button mutation needs a compilable form: drop the widget list.
MUTATIONS[12] = (
    "the Done button is dropped", MODEL,
    """        .with_widgets(vec![crate::screen::Widget::button(
            DONE,
            (gui_w - crate::screen::BUTTON_WIDTH) / 2,
            MENU_CONTROLS_TOP,
            crate::screen::BUTTON_WIDTH,
            crate::screen::BUTTON_HEIGHT,
            done_label,
        )])""",
    "        .with_widgets(Vec::new())",
    ["gate", "world"],
    "vanilla's 200-wide centred Done at menuControlsTop",
)


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
    "gate": lambda: (lambda c: (c[0], "gate"))(run([EXE, "bookshot", "--check"], 600)),
    "world": lambda: unit("rewo-world", "book_view"),
    "net": lambda: unit("rewo-net", "book"),
}


def main():
    lo = int(sys.argv[1]) if len(sys.argv) > 1 else 1
    hi = int(sys.argv[2]) if len(sys.argv) > 2 else len(MUTATIONS)
    selected = [MUTATIONS[0]] + MUTATIONS[lo:hi]
    used = sorted({c for m in selected[1:] for c in (m[4] or [])}) or ["gate"]
    print(f"[m172] slice [{lo}, {hi}) — {len(selected) - 1} mutations + control; checkers {used}")
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
            gate_needed = "gate" in run_checkers
            if gate_needed and not build():
                survived, reason = False, "build failed"
            else:
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
    build()

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
