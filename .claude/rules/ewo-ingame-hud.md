---
paths:
  - "crates/ewo-jni/**"
  - "ingame-mod/**"
  - "docs/history/PHASE_E_PLAN.md"
---

# In-game HUD — `ewo-jni` cdylib + the `ewo-hud` Fabric mod

`ewo-render`'s Skia stack painting inside a running Minecraft (Phase E, E0–E7,
plus Phase F's dashboard). The modules and legit/pvp split have their own rule
(`ewo-modules.md`). Full history: `docs/history/ewoclient-v2-phases.md` and
`docs/history/PHASE_E_PLAN.md` (now a record).

## Locked architecture (do not regress)

1. **Dedicated GL context, never shared.** Skia and Minecraft share one window
   framebuffer but two GL state machines. Each frame does `wglMakeCurrent` to
   our context, draws, then hands the thread's context back untouched. A shared
   context flickered, then crashed `nvoglv64.dll`.
2. **The frame hook is `RenderSystem.flipFrame` HEAD** (`EwoHudMixin`), a
   universal end-of-frame point. Fabric's `ScreenEvents.afterExtract` fires too
   early in 26.x's deferred GUI.
3. **Mods run in Mojmap with no remapping.** The on-disk MC jar is Mojmap-named
   (26.x ships deobfuscated), EwoLoader logs `Mappings not present!`, and mods
   build with plain `javac` against the jar. The jar is Java-25 bytecode, so the
   build needs **JDK 25** (`%APPDATA%/EwoClient/jdks/temurin-25/`). The build
   compiles against **26.2**. `EwoMixinPlugin` picks 26.1 vs 26.2 mixin
   variants via `-Dewo.mc.version`, with `EwoCompat` for API differences.
4. **Clocks.** `paint` renders the HUD offscreen at the `HudPaintRate` cap,
   `composite` blits it every frame (never tears), and `refresh_frost` rebuilds
   the MODS/SETTINGS frosted backdrop about 10×/s into a quarter-res surface
   (two-step 2× downscale + small gaussian, cubic upscale per frame). The HUD
   editor view stays sharp.

## Data flow

- Java→Rust: **`EwoHudData`**, a shared direct `ByteBuffer` the mod fills each
  frame. Rust reads it through `GetDirectBufferAddress` (`jni-sys`), and
  `nativeRender()` takes no args. `SCHEMA_VERSION` guards the byte-for-byte
  mirror between `EwoHudData.java` and `hud/mod.rs`.
- Rust→Java: **`EwoModuleData`**, the mirror image, for module state
  (`modules.rs`). **Bump both sides together.** The 2026-05-26
  bump reached Java only, and the drift guard sat inert for three months. A test
  now pins the pairing.
- Overlay tab strip (`OverlayView::ALL`): HOME · HUD · CROSSHAIR · MODULES ·
  PVP · MODS · FRIENDS · SETTINGS. `hud::tab_layout` sizes from
  `ALL.len()`; it was once hardcoded to 6 and clipped a tab.
- Layout persists per client profile in `profiles/<name>/hud.toml`. The
  crosshair lives in `crosshair.toml`.

## Build, deploy, test

- **Always run `ingame-mod/build.ps1`** (add `-Pvp` for the assist build); never
  run `javac` by hand. It deploys the jar past the stale `file://` library cache
  (see `ewo-loader-integration.md`). If a change "doesn't take effect", compare
  sha1s first.
- `EwoHudMod` finds `ewo_jni.dll` in this order: `-Dewo.jni.path` /
  `-Dewo.hud.nativePath` (the launcher sets it when a dll ships beside its exe),
  beside the mod jar (where `build.ps1` deploys it), the game dir, then
  `Desktop/EwoClientV3/target/{release,debug}`. The log line
  `[ewo-hud] using ewo_jni.dll from …` says which one won.
- `ewo-*` crate tests run in CI's `cargo test --workspace`, but no gate
  exercises the HUD in a live game. Run `cargo test -p ewo-jni --lib` when you
  touch it. Its layout test once overflowed a 256-byte stack buffer and
  killed the process with `0xe06d7363`, unnoticed for months.
- **Opt-in profiler** (`ewo-jni/src/perf.rs` + `EwoPerf.java`): create
  `%TEMP%/ewo-perf.on` before launch to record per-section frame times;
  `ewo-perf.ab` also A/B-cycles the HUD composite (the HUD blinks about once a
  second) to measure its true cost. Without the sentinel it costs one boolean
  check per frame. Kept on purpose; it is not leftover leak-hunt code.
- In-game crashes in `nvoglv64.dll` under load come from **NVIDIA Threaded
  Optimization** fighting the second GL context. Ask the user to disable it in
  NVCP.

## Feature notes that invert if guessed

- **3D skin viewer (`skin.rs`)**: slim + wide models. `EwoSkinExport` must read
  the GameProfile `textures` property **reflectively**, because build-time
  authlib differs from runtime (record `properties()`/`value()` vs
  `getProperties()`/`getValue()`) and a direct call throws `NoSuchMethodError`.
  The viewer reloads on `ewo-skin.png` **mtime change**, or a stale file
  freezes the slim flag.
- **Custom crosshair**: when enabled, `GuiCrosshairMixin` asks
  `nativeIsCustomCrosshairEnabled` and cancels vanilla `Gui.extractCrosshair`.
- **Media controller (`media.rs`)** reads Windows SMTC on a background thread;
  transport clicks go back through `TryPlayAsync`/`TryPauseAsync`. `audio.rs`
  is a WASAPI process-loopback spectrum visualiser for the media widget.
- **FRIENDS tab** is a read-only **file bridge**: the launcher writes
  per-profile `ewo-friends.txt`, and `social::read_friends()` reads it each
  visible frame. The cdylib makes no HTTP calls. The launcher only rewrites the
  file while it is foreground, so the list is launch-time fresh during play.
- The HUD's `.iw-shell` uses `ewo-render::widgets::liquid_glass` (SkSL SDF
  refraction).
- Legit-client only in the default build. Native rendering only: no HTML
  injection, Velvet theme.
