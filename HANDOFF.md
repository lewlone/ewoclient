# Next-session prompt — Rewo, Lane-A continuation

Copy everything below the line into a fresh session.

---

You are continuing **Rewo** — the from-scratch native Minecraft client in Rust
living in `crates/rewo-*` of the EwoClientV3 workspace (`C:\Users\valtteri\Desktop\EwoClientV3`).
The `ewo-*` crates are the separate Skia launcher + JNI HUD project — do not
touch them (their schema-mismatch fix and comment repairs are already landed;
leave them alone).

**Read first, in this order:** `REWO_PLAN.md` §0.0 HANDOFF → the last three
§15 entries (M174 sign editor, M175 baby sheets, M176 leash light — all
landed 2026-08-23/24) → `AGENT_LOOP_BRIEF.md` for process rules.
`MEMORY.md` at the repo root is the previous agent's orientation snapshot +
fresh-trap list; read it, don't trust it over measurements.

## State (verified 2026-08-24 on the merged tree)

```bash
git rev-list --left-right --count origin/main...main   # was 0 8 — LOCAL AHEAD, UNPUSHED
git status --short                                     # expect empty
git worktree list                                      # expect main only
```

The eight unpushed commits are the fix-pass + M174 + M175 + M176 merges plus
a memory-notes commit. **The user has not said "push" yet — ask, or push only
on instruction.** If they have pushed since, expect 0 0.

Verified numbers, read off the runner (2026-08-24):

* **3450 tests / 0 failures** across eight crates — world 1238, net 1238,
  gpu 320, data 235, app 231, mesh 52, proto 16, audio 120. `rewo-app` takes
  `--bins`; the other seven take `--lib`; a crate whose tests fail to compile
  prints no `test result` line and reads as silence.
* **42 serverless gates** green, 0 validation errors. Enumerate from
  `rewo --help`; newest is `signshot` (23 witnesses), mobtexshot grew to 17.
* **`live --render-check` 64/64, exit 0** via `python tools/render_check.py`
  (debug build; validation is `cfg!(debug_assertions)`-gated for `live`;
  r63/r64 arrived with M174, no new ids since).
* `REWO_PACKET_COVERAGE.md` **122 / 0 / 19**, classes A and B empty — its §2
  table is machine-checked by the unit test in `ids.rs`.
* Demo PNG `2cc56b4acbfb92cb` byte-identical since M15.

## The work queue (Lane-A, in priority order)

1. **The advancements screen — start here.** The last big GUI surface, and
   realistically a 2–3-milestone arc at house standard: decode
   (`update_advancements`, packets 85 and 130 in the coverage table) → the
   tabbed-tree model → render → clicks/serverbound queries. Measured scope
   already exists: **1562 of 1688 advancements have no `display`**, so the
   screen shows 126 — decode accordingly rather than modelling what never
   draws. Milestone-split suggestion: M177 decode + model, M178 render +
   gate, M179 clicks. Claim witness ids and any atlas rows in §0.0's
   shared-resource allocation table BEFORE writing code.
2. **Page-text click events** (M172's leftover): written-book page components
   can carry click events; M128's `active_text` machinery exists. **Verify
   BookViewScreen's actual click semantics against the decompile FIRST** —
   every milestone this arc found the plan's premise half-wrong.
3. **Lectern menu-backed reader** (the lectern is a menu whose screen extends
   BookViewScreen; M87 recorded the shape).
4. **ETF random/emissive textures** — the user runs Fresh Animations; high
   personal value. `rewo_data/src/etf.rs` already parses weights/names/baby/
   sizes rules; textures land via the variant band.
5. **Happy-ghast quad-leash** (M170/M176 leftover; nothing Rewo renders takes
   the branch yet — check whether anything now does before building it).

Farther (not queued): map pipeline, resource-pack fetch/apply,
server-transfer/reconnect, dialog framework — each a named subsystem in the
coverage doc.

## Traps that cost the LAST arc real time (new ones only — the briefs carry the standing set)

1. **PowerShell tooling vs this repo's bytes.** `Get-Content` reads UTF-8 as
   ANSI (mojibake) and `WriteAllLines` rewrites whole-file CRLF; `>` redirect
   writes UTF-16 patches git cannot read (`use git diff --output`);
   `"…\n…"` in double quotes is a LITERAL backslash-n — multiline replaces
   through `[IO.File]::ReadAllText/.Replace` must use backtick-n, and even
   then prefer the Edit tool for anything with em dashes. After ANY scripted
   doc edit: check CRLF count, U+FFFD/em-dash bytes, and `git diff --stat`.
2. **`mobs::kind_for_entity_name` wants NAMESPACED names**
   (`minecraft:zombie`) — bare short names all fall through to Capsule
   silently. This made every M175 swap inert until a pixel witness demanded
   an adult-impossible colour. It is now a battery mutation; keep that shape
   in mind for any name-keyed lookup.
3. **Never re-shoot a destroyed Stage/gate resource** — no Rust panic, just
   `vkResetCommandPool` on a dead handle and exit `0xC0000005`. Rebuild like
   the client does.
4. **GPU-gate flake watch-item:** three spontaneous `0xC0000005` exits this
   arc on heavy multi-stage gates (mobtexshot ×2, leashshot ×1), each clean
   on immediate re-run. Suspected teardown/driver under long-session memory
   pressure, not a regression. If it starts REPROING, investigate pass
   destroy-vs-frames-in-flight before touching code.
5. **A milestone's own battery should mutate its newest lookup tables** —
   M175's namespace mutation is the worked example; M167's witness-name pin
   and M93b's production-input rule are the older shapes.

## Process (unchanged, non-negotiable)

* Ground truth is the decompile at
  `%APPDATA%/EwoClient/rewo/26.2/decompiled` plus the datagen reports. Cite
  `file:line`; never a wiki.
* Every milestone: headless gate or unit tests → mutation battery with a
  no-op control that must SURVIVE → run `python tools/render_check.py` after
  any render-path change → update §15 + §0.0 (+ allocation-table claims) →
  CLAUDE.md, then **regenerate AGENTS.md** with the command in its header
  (never hand-edit the mirror).
* Branch off `main`, commit per logical step explaining the FINDING, merge
  `--no-ff`. End messages with `Co-Authored-By: ox-alpha <noreply@opencode.ai>`.
* Never `cargo fmt`. Never hand-edit generated `*_table.rs` files — re-run
  their `tools/gen_*.py`.
* Leave the tree clean; delete merged branches; check worktrees' dirty state
  before calling anything litter.

## Still yours, user

The audio listening pass (`cargo run -p rewo-audio --example listen`, then a
live `rewo live --audio` build) — no gate can grade sound; remember the
8-block-horn-is-quieter-than-0-blocks expectation (M153's pinned divergence).
And the push decision for the eight local commits above.

## Standing lesson

A green suite means the transcription agrees with itself. This arc: a
feature shipped with zero consumers four times over (tab list, M86's nine);
music silent for three milestones behind 36 green gates; M175's swaps inert
behind 23 green witnesses until n2 demanded an adult-impossible pixel. Open
the source beside the claim, prefer a measurement to a sentence, and when a
milestone's own premise survives first contact — write down WHY you believe
it, with a line number.
