# EwoClient + Rewo

This workspace holds **two projects that share a repo and little else**:

- **EwoClient** (`crates/ewo-*`, `ingame-mod/`): a Rust + Skia port of the
  "Velvet & Pearl" launcher prototype, now a functional Minecraft launcher
  (Microsoft auth, downloads, JVM launch, bundled Fabric-fork loader) plus an
  in-game HUD painted by a JNI cdylib.
- **Rewo** (`crates/rewo-*`): a from-scratch native Minecraft: Java Edition
  client in Rust on raw Vulkan, speaking protocol 776 (MC 26.2). It launches as
  a `Native` instance from the launcher.

This file is the always-loaded index: orientation, non-negotiables and routing.
**Subsystem rules live in `.claude/rules/*.md`** and load automatically when
you touch matching paths. **History and reasoning live in `docs/`** and the
plan files; read them on demand. Keep this file short (see "Maintaining this
file").

## Confirm you're in the right directory

Three EwoClient directories exist on this Desktop. Two are dead, and both carry
tombstone notes redirecting here:

- `c:\Users\valtteri\Desktop\EwoClient`: dead (TypeScript + Tauri, 2026-04-20)
- `c:\Users\valtteri\Desktop\EwoClient-v2`: dead (earlier Rust, 2026-04-27)
- `c:\Users\valtteri\Desktop\EwoClientV3`: **live**. `git remote -v` shows
  `lewlone/ewoclient`.

Sibling repo **EwoLoader** (`C:\Users\valtteri\Desktop\EwoLoaderV1`, private
`lewlone/ewo-loader`) is a friendly fork of `fabric-loader` that keeps the
`net.fabricmc.loader.*` packages. Open it alongside when touching loader
integration. The two repos meet at the loader-manifest contract
(`manifest/0.1.0/{26.1,26.2}.json`).

Author identity for both repos: `lewlone <valtteri.e.saarinen@gmail.com>`.

## Start here

| You're working on | Read |
|---|---|
| Anything: what's broken or open | `docs/REVIEW-2026-09.md` (independent review, 2026-09-25, and fix status; anything listed there as open is open) |
| Rewo, anything | `REWO_PLAN.md` §0.0 (numbers, gates, gotchas). The old session handoffs (`HANDOFF.md`, `AGENT_LOOP_BRIEF.md`) are retired to `docs/history/` |
| Rewo packets | `REWO_PACKET_COVERAGE.md` (its table is machine-checked by a test in `ids.rs`) |
| Rewo audio / Velvet HUD / feature choice | `REWO_AUDIO_PLAN.md` / `REWO_VELVET_UI_PLAN.md` / `REWO_FEATURE_SURVEY.md` |
| Launcher visuals | `StyleSheet1`, `StyleSheet2`, the `.htm` prototype, `style/*.png` |
| Why a rule exists | `docs/README.md` → the verbatim history files |

Path-scoped rules (auto-loaded; read one directly if you need it early):

| Rule | Covers |
|---|---|
| `ewo-visual.md` | Velvet tokens, render graph, reference materials, glossary, Skia perf rules |
| `ewo-launcher.md` | window model, lifecycle and leak fixes, module map, disk layout |
| `ewo-loader-integration.md` | loader manifests, bundled mods, per-instance toggles, iteration loop, stale `file://` jar |
| `ewo-ingame-hud.md` | Phase E architecture, shared buffers + schema versions, build/deploy, HUD feature gotchas |
| `ewo-modules.md` | module catalog, legit / pvp split, mixin hooks, PvP Utils |
| `ewo-social.md` | friends/presence/join contract (Phase H) |
| `rewo.md` | Rewo orientation, verification, cross-cutting traps, mutation-battery hazards |
| `rewo-net.md` | protocol, registries, wire shapes, metadata, dispatch |
| `rewo-gpu.md` | Vulkan conventions, colour space, resource lifetime, CEM, HUD coordinates |
| `rewo-audio.md` | audio containment and traps |
| `rewo-screens.md` | containers, recipe book, text input, tooltips, chat |

### Where the old sections went

Code comments cite sections of this file by name. They now live here:

| Old section name | Now in |
|---|---|
| Non-negotiables, Architecture (locked decisions), Things NOT to do | this file |
| Velvet theme tokens, Render graph, Reference materials, Glossary, Memory + performance pass | `.claude/rules/ewo-visual.md` |
| Unfocused-swap memory leak, Leak-hunt instrumentation, Launcher window, Freeze-on-exit, Packaging, v2 Phases A–C, Useful runtime conventions | `.claude/rules/ewo-launcher.md` |
| Phase D, Bundle phase, Stale `file://` jar gotcha | `.claude/rules/ewo-loader-integration.md` |
| Phase E, Phase F dashboard + skin viewer, crosshair + media | `.claude/rules/ewo-ingame-hud.md` |
| Phase G, Post-ban refactor: legit / pvp split | `.claude/rules/ewo-modules.md` |
| Phase H — Social | `.claude/rules/ewo-social.md` |
| Build sequence, Step 1–16 implementation notes, v1 scope | `docs/history/ewoclient-v1.md` |
| Full v2 / Phase A–H text | `docs/history/ewoclient-v2-phases.md` |
| Rewo milestone record | `docs/rewo/milestones-M0-M86.md`, `docs/rewo/milestones-M87-M142.md` |
| Session "Update" footers (M143–M180 and earlier) | `docs/history/claude-md-session-log.md` |

## Commands

```bash
cargo build --workspace
cargo test --workspace --no-fail-fast       # unit tests (some skip without local assets)
REWO_REQUIRE_ASSETS=1 cargo test --workspace  # a test that would skip fails instead
python tools/gates.py                        # every `rewo *shot --check` gate + demo PNG hash
python tools/gates.py --only mobshot,itemshot
python tools/render_check.py                 # stages a fresh vanilla server, runs `rewo live --render-check`
cargo run -p ewo-launcher                    # launcher (add --dev for the tweaks overlay)
cargo run -p rewo-app -- live --host HOST:PORT
cargo build -p rewo-app --features gates     # gate subcommands, `live --render-check`, `play`, REWO_* test knobs
powershell ingame-mod/build.ps1 [-Pvp]       # builds ewo-jni + the mod jar AND deploys it
powershell package.ps1                       # release bundle into dist/EwoClient
python tools/regen_agents_mirror.py          # after editing this file: regenerate AGENTS.md
```

- **Local-only inputs.** Most gates and several tests need
  `%APPDATA%/EwoClient/rewo/26.2/` (datagen reports + a Vineflower decompile of
  the user's own client jar) and a Vulkan device with the SDK's validation
  layers. Hosted CI (`ci.yml`, required check `test` on `main`) runs only
  build + `cargo test` + clippy. `gates.yml` runs the tests with
  `REWO_REQUIRE_ASSETS=1`, `gates.py` and `render_check.py` on every push, on
  the owner's self-hosted runner (label `rewo-gpu`); it has no `pull_request`
  trigger, and outside contributors' workflow runs need approval.
- A test that needs local assets calls `rewo_data::skip_test!("reason")`, never
  returns silently: it prints `SKIP:`, and panics under `REWO_REQUIRE_ASSETS=1`.
- **Gate rules** (enforced by `tools/gates.py`): a gate passes only if it exits
  0 **and** its output has no Vulkan validation error. The demo PNG's expected
  hash lives in `tools/demo_hash.txt`; change it only in a commit that
  intentionally changes rendering, and say why. `render_check.py` insists on
  the debug binary (validation is only on in debug builds).

## Non-negotiables (design rules — never violate)

From the prototype author's intent. They're load-bearing:

1. **"Motion is load-bearing. Nothing here is static."** Every surface animates
   continuously. Static screens fail the parity test.
2. **Don't transform anything that contains text.** Scaling a text container
   re-rasterizes every glyph and softens dense text. Animate adjacent layers
   (tint layers) instead.
3. **Don't blur during entrance animations on text-bearing surfaces.** Same
   reason. The author removed `filter: blur` from the modal entrance.
4. **The `--silk` curve `cubic-bezier(0.22, 1, 0.36, 1)` is the signature
   easing.** Use it for nearly every transition. The exceptions are linear
   loops (rim slides, sheen sweeps) and the few durations the CSS names.
5. **Pearl dust is on every screen.** So are velvet folds, caustics and bokeh.
   That's identity, not decoration.
6. **Reduced motion** (Windows `SPI_GETCLIENTAREAANIMATION`) collapses all
   animations to under 1 ms. Honor it.
7. **`isolation: isolate` semantics matter.** The backdrop is its own
   compositing context, so screen-blend particles must not blend into panel
   content above.

Plus: **"OFFLINE FIRST. NOTHING PHONES HOME."** No telemetry, analytics,
auto-update or news. The only network calls are auth, the version manifest, the
asset CDN and, once the user links an account, the social API.

## Architecture (locked decisions)

- **Renderer (EwoClient): Skia** (`skia-safe`), not raw wgpu. Escape hatch is
  SkSL. Windows presents through **Skia D3D12 + DirectComposition** (the only
  way to get transparent rounded corners on Win11). Linux/Hyprland uses Skia GL
  on glutin. There's no uncapped frame rate on Windows.
- **Renderer (Rewo): raw Vulkan via `ash`**, GLSL compiled by `glslc` from the
  Vulkan SDK.
- **Layout:** hand-rolled (`taffy` is a workspace dependency but unused). **Window:** `winit` 0.30, custom frame, no native
  titlebar. Wayland only on Linux, no X11, no macOS.
- **Concurrency:** single-threaded plus `std::thread` + `mpsc`. **No
  tokio/smol.** Rayon only where profiling demands it (Rewo's mesh pool).
- **HTTP:** `ureq` (sync). **Settings:** TOML under `<config>/EwoClient/`
  (`%APPDATA%` / `$XDG_CONFIG_HOME`) via `dirs`.
- **Fonts:** bundled variable TTFs in `assets/fonts/`, never system fonts.
  English only.
- **Crates:** `ewo-core` (types, tokens, easing, module catalog; no graphics),
  `ewo-render`, `ewo-launcher` (binary), `ewo-jni` (in-game cdylib),
  and `rewo-{proto,data,world,net,mesh,gpu,audio,app}` (binary `rewo`). Add a
  crate only for a structural reason: boundaries follow what changes together.

## Scope boundaries (current)

- Only the **Velvet** theme exists. Pearl/Obsidian/Champagne are dropdown stubs
  with no compat shims. The launcher's audio sliders connect to nothing.
- Hyprland has never been run on real hardware, and no formal pixel-parity pass
  vs `style/*.png` has been logged. Both are checks for the user.
- EwoClient's in-game default build is **legit-client only** (zero packet
  synthesis). Assist modules live behind `--features pvp` / `build.ps1 -Pvp`.
- Rewo targets offline vanilla servers and the user's **Frogsy** network.
  **Public servers are out of scope** (anticheat ban risk).

## Verification policy (user mandate)

**Headless first.** The user does not manually test what a machine can check.
Every change ships with a self-check path: a PNG render harness, a
`*shot --check` gate, an auto-exit soak, or a test server you start yourself.

- Rewo gates and the live render check are described in `.claude/rules/rewo.md` and
  §0.0.
- **`ewo-*` crate tests aren't in any verification loop.** Run them yourself
  (`cargo test -p ewo-jni --lib`, etc.) when you touch those crates.
- In-game mod changes go through `ingame-mod/build.ps1`, never hand `javac`.
- Report honestly: a green gate proves what it grades and nothing more (audio
  gates can't hear; injected witnesses pass against a dead server).

## Cross-cutting working rules

- **Line endings: measure with a byte count, never a shell pattern containing
  a raw CR.** `core.autocrlf` is false and there's no `.gitattributes`. The tree
  is overwhelmingly LF; `rewo-gpu/src/cem.rs` is all-CRLF, and
  `rewo-app/src/mobshot_cmd.rs`, `rewo-gpu/src/vanilla_hier.rs`,
  `rewo-world/src/chunk.rs` and `rewo-world/src/light.rs` are mixed. Don't let
  an editor normalise them into a whole-file diff.
- **Don't keep the same number in two places.** Current Rewo counts live only in
  `REWO_PLAN.md` §0.0. Prose beside a machine-checked number rots, and the
  number doesn't.
- **Verify a claim before acting on it**, including claims in these docs.
  Several recorded "blockers" and doc statements have been wrong. Check
  `git log --oneline`, the code or the decompile.
- **Generated files** ("Do not edit", from `tools/gen_*.py` /
  `gen_anim_defs.ps1`) change only by re-running the generator.
- **The agent mirror is generated.** After editing this file, run
  `python tools/regen_agents_mirror.py`. Never hand-edit the mirror.

## Things NOT to do

- Don't pre-optimize for a wgpu rewrite of the launcher. Skia is the tool.
- Don't add async runtimes or a CSS parser (styling is Rust tokens and widget code).
- Don't move the prototype reference files (`EwoClient Prototype _fixed_.html`,
  the `.htm`, `StyleSheet1`, `StyleSheet2`, `style/`) without asking.
- Don't add tests that render the prototype HTML in a headless browser. Pixel
  parity is checked manually by the user.
- Don't widen `versions::manifest::is_supported` or break the Mojang-compatible
  `shared/` disk layout.
- Don't read the source of non-open-source client mods (Sodium,
  EntityCulling, Xaero's, JourneyMap, Jade, WTHIT) as a reference for Rewo.

## Maintaining this file

This file loads in full for every session, so it stays **under ~250 lines**.
It was 7,192 lines (about 131k tokens) until 2026-09-26; the verbatim original
is preserved across `docs/`.

- A milestone's narrative goes to `REWO_PLAN.md` §15 (Rewo) or a `docs/` file.
  Never append session "Update" footers here.
- A durable subsystem fact goes into the matching `.claude/rules/*.md` (add a
  `paths:` glob if you create a new rule). A cross-cutting rule goes here, in
  one line.
- When a rule turns out wrong, fix it in place and note the correction in the
  history doc, not as a new paragraph here.
