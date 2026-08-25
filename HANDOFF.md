# Next-session prompt — Rewo, Lane-A continuation (headless work)

Copy everything below the line into a fresh session.

---

You are continuing **Rewo** — the from-scratch native Minecraft client in Rust
living in `crates/rewo-*` of the EwoClientV3 workspace (`C:\Users\valtteri\Desktop\EwoClientV3`).
The `ewo-*` crates are the separate Skia launcher + JNI HUD project — do not
touch them.

This session's queue is **headless-only work**: nothing below needs a running
server or the windowed client. Where an item *could* later take live
r-witnesses, that is named as deferred, not skipped silently.

**Read first, in this order:** `REWO_PLAN.md` §0.0 HANDOFF → §15's last three
entries (M177 advancements decode, M178 advancements render, M176 leash light)
→ `AGENT_LOOP_BRIEF.md` for process rules. `MEMORY.md` is an older orientation
snapshot — read it, don't trust it over measurements.

## State (verified 2026-08-24 on the pushed tree)

```bash
git status --short        # expect empty
git rev-list --count origin/main..main   # expect 0 — everything is PUSHED
```

* **main == origin/main at `cf7a8bd`.** The M174/M175/M176 batch AND the
  M177+M178 advancements arc are all pushed.
* **3489 tests / 0 failures** across EIGHT rewo crates — world 1255,
  net 1257, gpu 320, data 235, app 231, mesh 52, proto 16, audio 120.
  `rewo-app` takes `--bins`; the others take `--lib`.
* **43 serverless gates green**, 0 validation errors. Newest is **`advshot`**
  (14 witnesses) — enumerate via `rewo --help`, don't trust lists.
* `live --render-check` **64/64** exit 0 via `python tools/render_check.py`
  (debug build required). No new rNN since r64.
* `REWO_PACKET_COVERAGE.md` at **124 / 0 / 17**, classes A+B empty; its table
  is machine-checked by ids.rs. Class C's remaining six: horse/mount screen
  (41), map pipeline (51), resource_pack_pop download half (80), transfer (129),
  dialog framework (139/140).
* Demo PNG `2cc56b4acbfb92cb` byte-identical since M15.

## Work queue (headless-first, in priority order)

1. **M179 — advancement clicks.** The model half exists:
   `AdvancementsView::tab_click(gui_w, gui_h, mx, my)` returns the hit tab;
   `asm::Tab::scroll(dx, dy)` + `tick()` exist; the session sends
   `send_seen_advancements_opened_tab/closed_screen`. Missing: mouse-press
   routing in `live_cmd` (the `(ScreenKind::Advancements, ...)` press arm has
   only Done), tab-click → select + openedTab send, wheel/drag scroll feeding
   `tab.scroll(SCROLL_SPEED * x, ...)`, and hover already ticks from the frame
   loop. Gateable entirely through `advshot` extensions + unit tests — the
   live r-witnesses for click paths can be DEFERRED and named.
2. **Page-text click events** (M172 leftover). Written-book page components
   can carry click events; M128's `active_text` machinery exists.
   **Verify BookViewScreen's actual click semantics against the decompile
   FIRST** (`BookViewScreen.mouseClicked` / `GuiGraphicsExtractor`'s click
   chain) — every milestone the last arcs found the plan's premise half-wrong.
3. **Lectern menu-backed reader** — M87 recorded the shape: the lectern is a
   menu whose screen extends BookViewScreen, one slot, no player inventory.
   Decode + model headless.
4. **ETF random/emissive textures** — the user runs Fresh Animations;
   high personal value. `rewo_data/src/etf.rs` already parses weights/names/
   baby/sizes rules; textures land via the entity-atlas variant band.
   Verifiable headlessly with `rewo mobshot --pack <zip>` against the user's
   real FA pack.
5. **Happy-ghast quad-leash** — check whether anything Rewo renders now takes
   `LeashState`'s quad branch before building it.

## Traps that cost the LAST arc real time (new ones only)

1. **A mutation battery must REBUILD between mutants.** m178's first run never
   built, graded every mutation against the stale exe, and read 8 SURVIVED —
   including mutations that were certainly lethal. The signature is "control
   survives AND everything else survives"; put an explicit build step inside
   the checker (m178_mutate.py now does) and treat all-survived as a broken
   instrument, not a result.
2. **Pixel probes on transparent texture texels pass vacuously against
   whatever is behind them.** `window.png` is paletted with per-index tRNS
   alphas (transparent interior); the first probe set "matched" by reading the
   backdrop on both sides. When a texel witness matters, assert `alpha == 255`
   beside the colour match, and measure the sheet directly (proper palette +
   tRNS decode) before trusting any probe placement.
3. **Presence-only pixel counts cannot see pass ORDER.** Flipping the
   connectivity passes (white-under-black) left enough white pixels from other
   sources to stay green. What killed it was asserting ZERO black-on-core
   pixels alongside the white count — order violations show up as wrong
   colours in expected places, not as missing pixels.
4. **A duplicate rule with no caller and no witness survives ANY mutation of
   it.** M177's battery flipped a second copy of the done rule to ANY with a
   fully green suite. The fix is structural: delete duplicates, keep one body,
   witness it where it lives.
5. **Regenerating AGENTS.md needs the header re-added** (its first ~15 lines,
   ending `-->`) and is safest through a temp `.py` file, not an inline
   `python -c` — the inline form through PowerShell cost one mojibake scare
   this arc. The diff after regen should be EXACTLY your CLAUDE.md paragraph.
6. Standing set still applies: PowerShell vs repo bytes (Get-Content reads
   UTF-8 as ANSI; `>` writes UTF-16 patches); `kind_for_entity_name` wants
   NAMESPACED names; never re-shoot a destroyed Stage (0xC0000005, not a
   panic); GPU-gate flake watch-item (three spontaneous 0xC0000005 exits last
   arc, each clean on re-run — if one starts reproing, investigate
   destroy-vs-frames-in-flight before touching code).

## Process (unchanged, non-negotiable)

* Ground truth is the decompile at `%APPDATA%/EwoClient/rewo/26.2/decompiled`
  plus the datagen reports. Cite `file:line`; never a wiki.
* Every milestone: headless gate or unit tests → mutation battery whose no-op
  control must SURVIVE (and which rebuilds!) → `python tools/render_check.py`
  after any render-path change → update §15 + §0.0 (+ allocation-table claims)
  → CLAUDE.md, then regenerate AGENTS.md with the command in its header.
* Branch off `main`, commit per logical step explaining the FINDING, merge
  `--no-ff`. End messages with `Co-Authored-By: ox-alpha <noreply@opencode.ai>`.
* Never `cargo fmt`. Never hand-edit generated `*_table.rs` files.
* Leave the tree clean; delete merged branches; claim witness ids / atlas rows
  in §0.0's allocation table BEFORE writing code (**next free rNN is r65**).
* Read each crate's EXIT CODE, never substrings of gate output.

## Still yours, user

The audio listening pass (`cargo run -p rewo-audio --example listen`, then
`rewo live --audio` on an audio build; the 8-block-horn-quieter-than-0-blocks
result is EXPECTED — M153's pinned divergence). Also un-eyeballed: the
advancements screen renders headlessly verified but nobody has pressed L in
the windowed client yet.

## Standing lesson

A green suite means the transcription agrees with itself. This arc: a
duplicate done rule survived every mutation behind a full green suite; eight
lethal mutants "survived" against a binary that was never rebuilt; three
tooltip probes passed through transparent texels against whatever sat behind
them. Prefer a measurement to a sentence, open the source beside every claim,
and when a milestone's premise survives first contact, write down WHY you
believe it — with a line number.
