# MEMORY — orientation snapshot (written 2026-08-23 by ox-alpha)

Personal read-only orientation notes after a full-repo deep dive. **Numbers below
carry their verification date; the authoritative current numbers live in
`REWO_PLAN.md` §0.0 and the AGENTS.md top block** — per project rule, don't
trust a number copied into a second document (including this one) without
re-measuring.

## What this repo actually is (two projects in one workspace)

1. **EwoClient launcher** (`ewo-*` crates + `ingame-mod/`): Skia (D3D12+DComp on
   Windows / GL on Linux) Velvet-&-Pearl Minecraft launcher + JNI in-game HUD
   cdylib loaded by a Fabric mod. Windows-developed; Linux written, never run.
2. **Rewo** (`rewo-*` crates → one binary `rewo.exe`): from-scratch native
   Minecraft client, vanilla protocol 26.2/pin 776, raw Vulkan via `ash`,
   ~300k of the workspace's ~340k LOC. This is where nearly all recent work is.

Sibling repos: `Desktop\EwoLoaderV1` (Fabric fork, loader manifests) and
`Desktop\FULLSTACK\...` (chickenbot API + ChickenLink plugin, Phase H social).

## Git state at snapshot (verified)

- `main == origin/main == d85e0a5`, tree clean. Last milestone: **M173**
  (volume sliders + options wiring, `optionshot` = 41st gate, r62).
- Worktree `.claude/worktrees/rewo-listening-pass-2f5215` on branch
  `claude/rewo-m174` holds **uncommitted M174 WIP: the sign-edit screen**
  (`open_sign_editor` decode, `sign_edit_screen.rs` model w/ TextFieldHelper
  pixel-width validation, flat GUI-blit sign rendering, ~443 insertions across
  11 files + 1 new file). Branch name ("listening-pass") does NOT match its
  content.
- Doc precedence when they disagree: REWO_PLAN §0.0 (per-milestone updated) >
  AGENTS.md top block > HANDOFF.md (2026-08-20 snapshot, M165-era) >
  NEXT_SESSION_PROMPT.md (M157-era, stale) > README.md (36-gate era).
  AGENTS.md is a GENERATED mirror of CLAUDE.md — never hand-edit.
- Coverage prose vs table: trust the machine-checked table
  (`ids.rs::the_coverage_table_matches_the_code`). Table said 121/0/20 at M173.

## Verified state claims at M173 (2026-08-23, from AGENTS.md top block; not re-run)

3424 tests/8 crates (app needs `--bins`, others `--lib`), 41 serverless gates,
`live --render-check` 62/62 via `python tools/render_check.py` (debug build!),
demo PNG sha256 prefix `2cc56b4acbfb92cb` byte-identical since M15,
mobshot 246/246, soundshot 37 default/57 audio.

## Crate map (LOC approx)

| crate | role |
|---|---|
| ewo-core (1.5k) | theme tokens, OkLch, SILK easing, Screen enum, module REGISTRY (12 legit slots 0..11; pvp feature adds 12..25), pvp.rs PvP-Utils config |
| ewo-render (15.6k) | Skia everything: gl_backend DComp/GL, backdrop+20Hz slow-clock cache, text engine, all widgets/screens (ewo-ui is a dead stub — real UI lives here) |
| ewo-launcher (12.5k) | binary; auth chain, versions/downloads, JVM spawn, EwoLoader merge, profiles/keybinds/modules, social/friends |
| ewo-jni (12.7k) | HUD cdylib: 13 JNI exports, EwoHudData v10 / EwoModuleData buffers, 8 overlay tabs, skin viewer, crosshair, SMTC media, WASAPI spectrum (audio.rs), cached frost |
| rewo-proto (1k) | wire primitives only |
| rewo-net (65k) | PlaySession (reader thread + caller-driven 20Hz tick, dispatch chain in play.rs), ids.rs name-resolved packet table, Brigadier dispatcher/SNBT/selectors, sound engine model + SilentDevice, menus/recipe book models, jump_riding, options.rs (M173) |
| rewo-world (54k) | decoded world read-models: light engine, timelines/celestial, EntityTable, chat stack, 14+ screen models, physics/raycast |
| rewo-data (27k) | asset bake + registries.json readers + 18 GENERATED tables (tools/gen_*.py, "Do not edit") |
| rewo-mesh (5k) | mesher + rayon pool + crumbling decals |
| rewo-gpu (45k) | Vulkan renderer; draw order in world.rs::draw; six vertex ranges solid|text|glint|trim|armor_glint|emissive; atlas 1024x1600 w/ demand pools; velvet glyph/text/chrome; 50 shaders |
| rewo-app (95k) | `rewo` binary: live/play/view/demo/bench/net + 41 *shot gates + modules.rs RenderModules port; render-check witnesses r1..r62 live IN live_cmd.rs |
| rewo-audio (7k) | mixer (caller-driven), quantise, buffers, symphonia decode workers, SPSC CommandRing, cpal_sink (never gate-tested); off by default |

## Session findings (2026-08-23) — status after the fix pass

1. ~~**Latent bug:** `EwoModuleData.SCHEMA_VERSION` Java=3 vs Rust=2~~ **FIXED
   same day.** Rust now writes 3 (`modules.rs`), pairing pinned by a new unit
   test; geometry verified identical field-by-field. **Sharper find:** that
   module's layout test had been corrupting memory since MAX_SETTINGS grew
   (488 bytes written + reads to offset 452 into a `[0u8; 256]` array →
   abnormal exit `0xe06d7363`); fixture now CAPACITY-sized with a needed<=CAPACITY
   assert. Deployed via `ingame-mod/build.ps1` (jar + fresh debug dll). Lesson:
   **ewo-\* crate tests are in nobody's verification loop** (the rewo loop
   counts only the eight rewo crates) — run `cargo test -p ewo-jni --lib`
   after any ewo-jni change.
2. PvP-Utils layer — user confirmed intentional/separate; not a concern. Doc
   pointer added to CLAUDE.md so nobody "cleans up" EwoHitRange's legit-mixin call.
3. shaderpack/EwoVelvet — user confirmed barebones prototype; not our concern.
4. Stale comments fixed: skin.rs (slim ships), window/mod.rs + win32.rs
   (DWMWCP_DONOTROUND), ewo-jni/lib.rs JNI contract (13 exports), rewo-audio
   lib.rs crate header (deps + M138b→M144 scope).
5. CLAUDE.md updated (catalog count 17 + both manifests; dated update paragraph)
   and AGENTS.md regenerated from it via the header command (LF-only verified).
6. dist/EwoClient refreshed via package.ps1 — full bundle again (exe +
   rewo.exe + icon + 6 fonts); Desktop shortcut repointed.
7. LEAK_HUNT_INSTRUMENT diagnostics intentionally left in (strip at ship time).
8. M174 worktree WIP = sign-edit screen; NOT touched by this session.

## Process rules I must follow here (from AGENT_LOOP_BRIEF + HANDOFF)

- Ground truth = decompile at `%APPDATA%/EwoClient/rewo/26.2/decompiled` +
  datagen reports; cite file:line; never wikis.
- Headless-first verification; mutation batteries need a no-op control that must
  SURVIVE; read exit codes, never substrings; per-crate test invocation differs.
- Never run `cargo fmt`. Never hand-edit AGENTS.md (regenerate from CLAUDE.md).
- Claim rNN witness ids + atlas coords in §0.0's allocation table before work.
- Exactly five non-LF files under crates/ (cem.rs pure CRLF; mobshot_cmd.rs,
  vanilla_hier.rs, chunk.rs, light.rs mixed) — measure bytes, not grep.
- Commit messages explain the finding; merge --no-ff; update §15 + §0.0 +
  CLAUDE.md then regenerate mirror.
- The listening pass (audio) is explicitly THE USER'S — never claim audio works
  from a green suite.
