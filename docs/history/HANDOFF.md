# Next-session prompt — Rewo continuation (headless work)

Copy everything below the line into a fresh session.

---

You are continuing **Rewo** — the from-scratch native Minecraft client in Rust
living in `crates/rewo-*` of the EwoClientV3 workspace (`C:\Users\valtteri\Desktop\EwoClientV3`).
The `ewo-*` crates are the separate Skia launcher + JNI HUD project — do not
touch them.

This session's queue is **headless-only work**: nothing below needs a running
server or the windowed client. Where an item *could* later take live
r-witnesses, that is named as deferred, not skipped silently.

**Read first, in this order:** `REWO_PLAN.md` §0.0 HANDOFF → §15's newest
entries (M180 book page-clicks, M179 advancement clicks) →
`AGENT_LOOP_BRIEF.md` for process rules. `MEMORY.md` is an older orientation
snapshot — read it, don't trust it over measurements.

## State (verified 2026-08-25 on the pushed tree)
```bash
git status --short        # expect empty
git rev-list --count origin/main..main   # expect 0 — everything is PUSHED
```

* **main == origin/main** (verify with the commands above — do not trust a
  hash written here). The M179/M180 arc and every doc/memory refresh through
  2026-08-25 are pushed.
* **3495 tests / 0 failures** across EIGHT rewo crates — world 1259,
  net 1257, gpu 320, data 235, app 236, mesh 52, proto 16, audio 120.
  `rewo-app` takes `--bins`; the others take `--lib`.
* **43 serverless gates green**, 0 validation errors. This session raised
  **`advshot` 14 → 20** and **`bookshot` 21 → 24**.
* `live --render-check` **64/64** exit 0 via `python tools/render_check.py`
  (debug build required). No new rNN since r64.
* `REWO_PACKET_COVERAGE.md` unchanged at **124 / 0 / 17** (both milestones
  consumed packets already decoded).
* Demo PNG `2cc56b4acbfb92cb` byte-identical.

## Work queue (headless-first, in priority order)

1. **Lectern menu-backed reader** — M87 recorded the shape: the lectern is a
   menu whose screen extends BookViewScreen, one slot, no player inventory,
   no container screen (`LecternMenu` never calls
   `addStandardInventorySlots`). Decode (`open_screen` already carries menu
   type lectern; verify what the lectern's `container_set_data`/slot packets
   carry) + model headless. M180's `closeContainerOnServer()` seam is the
   hook: only LecternScreen overrides it — find what its override SENDS.
   Verify against `LecternScreen.java` / `LecternMenu.java` FIRST.
2. **ETF random/emissive textures** — the user runs Fresh Animations;
   high personal value. `rewo_data/src/etf.rs` already parses weights/names/
   baby/sizes rules; textures land via the entity-atlas variant band.
   Verifiable headlessly with `rewo mobshot --pack <zip>` against the user's
   real FA pack.
3. **Happy-ghast quad-leash — CHECKED, scoped, not built.** The branch IS
   reachable now: `EntityRenderer.java:186-236` draws FOUR taut ropes
   (`leashCount = quadConnection ? 4 : 1`) when the holder is a happy ghast
   (`supportQuadLeashAsHolder`, HappyGhast.java:494, holder offsets
   `createQuadLeashOffsets(this, -0.03125, 0.4375, 0.46875, 0.03125)` at
   :499) and the LEASHEE supports quad leash (AbstractHorse.java:199,
   Llama.java:418, Sniffer.java:153, AbstractBoat.java:366). Each rope:
   `start = leashee pos + own attachment point yRot(-own bodyYaw)`,
   `end = holder pos + holder point yRot(-holder bodyYaw)`,
   **`slack = false`** (taut — no sag curve), lights shared across the four.
   M170/M176's `collect_leashes` renders ONE sagging rope to
   `getRopeHoldPosition` for every leash — wrong on all three counts for a
   ghast-held mob. The old docs' claim "nothing Rewo renders takes the quad
   branch" was true pre-M170 and is stale now. Gateable via `leashshot`
   extensions; live witness would need a staged ghast + horse + link.
4. **Advancement clicks' live r-witnesses** — r65 is CLAIMED in §0.0's
   allocation table and DEFERRED: tab clicks re-sending `opened_tab`
   (unconditionally, even when unchanged) and CLOSED_SCREEN on every close.
   Needs a servered session alongside whatever else runs live.

## Traps that cost THIS session real time (new ones only)

1. **After a mutation battery, the exe IS THE LAST MUTANT.** Restore fixes
   sources, not binaries. The post-battery 43-gate sweep graded a +3px drift
   mutant and only the drifted-against witness went red — which read exactly
   like the GPU flake until the re-run reproduced deterministically. All
   three battery harnesses (`m178/m179/m180_mutate.py`) now rebuild after
   the final restore; keep that property in any new harness.
2. **A gate witness driving a COPY of production wiring proves nothing**
   (M93b's shape, again, newest form): m179's m9 hand-rolled
   `screen.select` beside production's handler, so deleting production's
   select survived both instruments. Fix pattern: extract the decision into
   a pure function (`tab_click_report`) that BOTH the handler and the gate
   drive.
3. **A state-machine rule can be dead code by construction of your own test**
   — m179's drag latch was cleared at press, which vanilla never does, making
   the non-left cancel arm unreachable and its test vacuous until the test
   switched buttons MID-drag. When a mutant survives, ask whether the RULE
   is reachable at all under your fixture sequences.
4. **26.x component click events are snake_case on the wire** (`click_event`,
   field `page` not `value`); the wiki's camelCase never arrives. And one
   styled component inherits its event across every wrapped piece of its OWN
   text — a plain-span control needs SIBLING components.
5. Standing set still applies: PowerShell vs repo bytes (this session:
   `Add-Content` appended one trailing CRLF into an LF file — normalize new
   files as BYTES before committing); `kind_for_entity_name` wants NAMESPACED
   names; never re-shoot a destroyed Stage; GPU-gate flake watch-item — but
   FIRST rule out a stale/mutant binary (trap 1) before believing a flake,
   because a flake does not reproduce while a stale binary does.

## Process (unchanged, non-negotiable)

* Ground truth is the decompile at `%APPDATA%/EwoClient/rewo/26.2/decompiled`
  plus the datagen reports. Cite `file:line`; never a wiki.
* Every milestone: headless gate or unit tests → mutation battery whose
  no-op control must SURVIVE (and which rebuilds — including AFTER the final
  restore!) → `python tools/render_check.py` after any render-path change →
  update §15 + §0.0 (+ allocation-table claims) → CLAUDE.md, then regenerate
  AGENTS.md via `python tools/regen_agents_mirror.py` (preserves the header;
  diff must be EXACTLY your CLAUDE.md paragraphs).
* Branch off `main`, commit per logical step explaining the FINDING, merge
  `--no-ff`. End messages with `Co-Authored-By: ox-alpha <noreply@opencode.ai>`.
* Never `cargo fmt`. Never hand-edit generated `*_table.rs` files.
* Leave the tree clean; delete merged branches; claim witness ids / atlas rows
  in §0.0's allocation table BEFORE writing code (**next free rNN is r65** —
  claimed by M179's deferred live witnesses; take r66 if you need one).
* Read each crate's EXIT CODE, never substrings of gate output.

## Still yours, user

The audio listening pass (`cargo run -p rewo-audio --example listen`, then
`rewo live --audio` on an audio build; the 8-block-horn-quieter-than-0-blocks
result is EXPECTED — M153's pinned divergence). Also un-eyeballed: the
advancements screen (press L — clicks now work too) and the book's page-text
clicks, both rendered headlessly verified but never seen live.

## Standing lesson

A green suite means the transcription agrees with itself. This session: two
milestones whose batteries caught their own gates' copies of production logic
on the FIRST run, a latch rule made unreachable by its own press handler, and
a full-gate sweep that graded a leftover mutant. Prefer a measurement to a
sentence, open the source beside every claim, and when a premise survives
first contact write down WHY you believe it — with a line number.
