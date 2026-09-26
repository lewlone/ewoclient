---
paths:
  - "crates/rewo-*/**"
  - "tools/**"
  - "REWO_*.md"
---

# Rewo — the from-scratch native Minecraft client (cross-cutting rules)

A from-scratch Rust Minecraft: Java Edition client speaking the vanilla
protocol (pin: **26.2 / protocol 776**, read from the bundled jar's
`version.json`), rendered with **raw Vulkan via `ash`**. The priorities are
frame-time consistency (1%/0.1% lows) and input latency. It plugs into the
launcher as a `Native` instance and is **not** a JVM/mod project, so none of the
`ewo-jni`/mixin machinery applies. Keep `ewo-*` crates untouched in Rewo work.

## Where the truth lives (read in this order)

1. **`docs/REVIEW-2026-09.md`**: the independent review and what is still open.
2. **`REWO_PLAN.md` §0.0**: current measurements, gate list, the live-check
   recipe, load-bearing gotchas, known issues. **Its numbers are maintained and
   its forward-looking prose is not.** Check `git log --oneline` before trusting
   a paragraph. §15 is the per-milestone log.
3. `docs/history/AGENT_LOOP_BRIEF.md`: process rules (retired, still sound). `REWO_PACKET_COVERAGE.md`: which
   packets are handled (its table is **machine-checked by a unit test in
   `ids.rs`**, so update it in the same change as `ids.rs`).
4. Subsystem plans: `REWO_AUDIO_PLAN.md`, `REWO_VELVET_UI_PLAN.md` (a visual
   freeze is in effect for HUD widgets), `REWO_FEATURE_SURVEY.md`
   (prioritisation, not citation; ~22–25% error).
5. The reasoning behind any rule below: `docs/rewo/milestones-M0-M86.md`,
   `docs/rewo/milestones-M87-M142.md`, and
   `docs/history/claude-md-session-log.md` for M143–M180.

**Never restate a test or gate count in prose outside §0.0.** A number with a
test behind it stays true, and the sentence next to it doesn't.

Ground truth is the decompiled 26.2 jar (Vineflower) plus Mojang's datagen
reports under `%APPDATA%/EwoClient/rewo/26.2/` (git-ignored, derived from the
user's own download). Brigadier and DFU are libraries absent from the
decompile, so grade those against the real jars.

## Verification (user mandate: headless-first)

The user does not manually test what a machine can check. Every milestone ships
a self-check path.

- Serverless `*shot --check` gates (validation layers ON, 0 VUIDs, fail-closed),
  listed in §0.0. `mobshot --check` after any mob or UV change; demo PNG hash
  must stay byte-identical unless the change is meant to move it.
- **`rewo live --render-check`** (via `tools/render_check.py`) is the only
  check that drives the **windowed** client. Run it after any milestone that
  adds a render path. Validation is `cfg!(debug_assertions)`-gated for `live`,
  so run a **debug** build. M86 found nine features that had never rendered in
  the windowed client while every headless gate was green.
- Before starting a test server, probe the port: a server that failed to bind
  still lets injected witnesses pass. The vanilla server stops on stdin EOF, so
  don't background it with `nohup … &`.
- `CORRECTIONS 0` from `rewo play` proves only the paths the harness exercises
  (flat walk). Knockback, riding and explosions need their own witnesses.

## Traps that recur across the whole client

- **Witnesses are wrong more often than code.** A fixture that sits where two
  candidate readings agree, a control that changes with its subject, and a
  pixel detector that counts against a painted background have each passed
  while measuring nothing. Derive the expectation from an independent source
  (decompile literals, jar PNG texels), never from the constant under test.
- **A gate that reimplements a slice of the app's setup misses what the app
  adds later** (`install_shapes`, `init_entities`). **A gate that supplies an
  input production must derive leaves the derivation untested** (M92's
  `mob_effect` ids). Call the production helper.
- **`PlaySession` owns a socket and has no test module.** Put logic in free,
  tested functions behind a thin adapter (M71, M97). Same for `LiveApp`.
- **The local player is not in `EntityTable`.** Metadata and attributes
  addressed to it are dropped unless routed separately (M55, M73, M141e).
- **One accessor per concept.** When two call sites each compute "which menu",
  "which origin" or "which conversion", they drift (M89, M106b, M112, M151).
- **The M-numbers aren't a contiguous index** (M52, M61, M64, M66, M68 each
  name two pieces of work). Use commit subjects.
- **"Class-C blocker" claims are often wrong**: the data is frequently in the
  client jar rather than on the wire (furnace recipes, merchant quick-move,
  stonecutter list, smithing sets). Check the jar before believing one.
- Market-leader client mods (Sodium, EntityCulling, Xaero's, JourneyMap, Jade,
  WTHIT) are non-open-source. **Don't read their source as reference.**
- Public servers are out of scope (ban risk). Staging is the user's Frogsy network.
- **Never run `cargo fmt`.** Claim render-check witness ids (`rNN`) and atlas
  rows in §0.0's allocation table **before** writing code. Parallel branches
  once minted the same id, and git merges that silently.
- Write new files as LF bytes. PowerShell `Add-Content` appends CRLF.

## Mutation batteries (`tools/m*_mutate.py`)

- Grade by the `test result:` line or the gate's exit code. **Exit code alone
  can't tell a failing test from a failing build**, and a substring can
  mis-decode (Windows locale mangled an em dash into "every verdict KILLED").
- Every battery carries a **no-op control that must SURVIVE**. A battery run
  against an already-red command reads KILLED for everything.
- **After a battery, the exe is the last mutant.** Restore fixes sources, not
  binaries, so rebuild before any post-battery gate run.
- A killed or timed-out battery skips its `finally` and **leaves the mutation on
  disk**. Grep for the markers first. A hung test binary can hold the link
  output (linker error 1104).
- Never run git operations while a battery is modifying files.
- A survivor is a question: an equivalent mutant (prove it), a weak fixture, or
  logic in an untestable place.
