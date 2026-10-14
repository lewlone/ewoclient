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
2. **The frame hook is the end of the frame**: `RenderSystem.flipFrame` HEAD
   on 26.1 (`EwoHudMixin`), and `Minecraft.renderFrame` at its
   `GpuSurface.present()` call on 26.2 (`EwoHudMixin262`). Fabric's
   `ScreenEvents.afterExtract` fires too early in 26.x's deferred GUI. Both call
   `EwoFrameHook`, which catches any `Throwable` from the native call or the
   module tick and disables the native bridge for the session after 5
   consecutive failing frames (a stale dll has no ABI handshake). The exit
   watchdog keys off its own `killPathLoaded` flag, so disabling the bridge
   does not disarm it.
5. **Exit watchdog arm points**: `MinecraftShutdownMixin` on `destroy` (26.1,
   in `EwoMixinPlugin.LEGACY_ONLY`) and `MinecraftShutdownMixin262` on
   `exitWorldAndClose` (26.2 removed `destroy`). Both `require = 0`, so a
   rename is silent at runtime: `python tools/check_mixin_targets.py` resolves
   every mixin target, method and `INVOKE` against both jars and is the guard.
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
  bump reached Java only, and the drift guard sat inert for three months.
  The pairing is now pinned by tests that **read the Java source**
  (`hud/schema_pin.rs` for `EwoHudData.java` / `EwoIndicators.java`;
  `modules::module_layout_matches_the_java_mirror` for `EwoModuleData.java`;
  parser in `java_mirror.rs`): schema versions, every `OFF_*` offset by name in
  both directions, strides, capacities, flag bits and record layouts. A field
  added on one side only fails a test. Still bump both sides together.
- Overlay tab strip (`OverlayView::ALL`): HOME · HUD · CROSSHAIR · MODULES ·
  PVP · MODS · FRIENDS · SETTINGS. `hud::tab_layout` sizes from
  `ALL.len()`; it was once hardcoded to 6 and clipped a tab.
- Layout persists per client profile in `profiles/<name>/hud.toml`. The
  crosshair lives in `crosshair.toml`.
- **Every per-profile config path goes through `ewo_core::pvp::profile_file`
  / `ewo_config_file`** (`hud.toml`, `modules.toml`, `crosshair.toml`,
  `ewo-friends.txt`, `profiles.toml`, `pvp.toml`). They apply the launcher's
  name rule and fall back to the `dirs` config dir off Windows. Never build a
  path by hand: the active profile name comes from the user-editable
  `profiles.toml`, and a `..` once escaped the folder. `PvpConfig::save`
  writes atomically (`ewo_core::util::atomic_write`) and returns the error.
- **Closing the overlay ends held drags.** Esc never delivers the mouse
  release, so `EwoOverlayScreen.removed()` calls `nativeOverlayClosed`
  (inside a `Throwable` catch: a stale dll has no such symbol), which runs
  `Editor::on_overlay_closed` -> `end_drags()`. A new drag state must be
  cleared there. Quick-edit presses are routed before the popover and
  tab-strip handlers.

## Build, deploy, test

- **Always run `ingame-mod/build.ps1`** (add `-Pvp` for the assist build); never
  run `javac` by hand. A deploying build copies the jar and `ewo_jni.dll` into
  `shared/libraries/dev/lewlone/ewo-hud/0.1.0/` and the jar to the dev drop
  `%APPDATA%\EwoClient\dev\ewo-hud-0.1.0.jar` that the loader manifests point at
  (see `ewo-loader-integration.md`). **`-NoDeploy`** compiles and jars only
  (no cargo build, nothing copied): use it in CI, audits and worktrees, and
  while Minecraft is running. The legit build fails if its jar carries any
  assist entry or names the assist package; the pvp build fails without
  `EwoAssistEntry`.
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
  NVCP. The frame paths and `Hud::create` restore Minecraft's context through
  the `OursCurrent` RAII guard: init runs in `Hud::build` under `catch_unwind`,
  the guard is declared before the Skia context, and a failed init deletes our
  context (`HudState::Failed` is never retried). Keep anything fallible and
  CPU-only ahead of the GL scope. (Not yet exercised in a running Minecraft.)
- **`fixture.rs`** (synthetic HUD blocks for the harness) is compiled only for
  tests and `--features fixture`; the `hudshot` example needs it:
  `cargo run -p ewo-jni --features fixture --example hudshot -- --all`. CI's
  ewo-jni clippy steps pass `ewo-jni/fixture`.
- The log (`%TEMP%\ewo-jni.log`) is opened once and rate-limited (identical
  repeats within 5 s collapse); the frame clock and gate state are `f64`.

## Feature notes that invert if guessed

- **3D skin viewer (`skin.rs`)**: slim + wide models. `EwoSkinExport` must read
  the GameProfile `textures` property **reflectively**, because build-time
  authlib differs from runtime (record `properties()`/`value()` vs
  `getProperties()`/`getValue()`) and a direct call throws `NoSuchMethodError`.
- **Skin and cape downloads are server-supplied URLs.** `EwoSkinExport` and
  `EwoTargetSkin` accept only host `textures.minecraft.net` (authlib 9.0.75 on
  26.2 and 7.0.63 on 26.1 enforce exactly that host; the `*.minecraft.net` /
  `*.mojang.com` wildcard is 7.0.61's, 1.21.x), http upgraded to https, no
  userinfo, port or escapes, at most 512 bytes; then 5 s connect / 10 s request
  timeouts, a body-stall watchdog, no redirects, a 1 MiB cap and a PNG
  signature check before anything is written. Same rule as Rewo's
  `skins::allowed_texture_url`.
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
