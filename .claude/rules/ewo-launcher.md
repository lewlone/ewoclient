---
paths:
  - "crates/ewo-launcher/**"
  - "crates/ewo-render/src/gl_backend.rs"
  - "crates/ewo-render/src/app_window.rs"
  - "package.ps1"
  - "assets/icon.ico"
---

# EwoClient launcher (`ewo-launcher`)

The windowed Skia launcher: custom frame, auth, version downloads, JVM launch,
profiles, keybinds. Loader and bundle work has its own rule
(`ewo-loader-integration.md`), and so does the social layer (`ewo-social.md`).
Full history: `docs/history/ewoclient-v1.md` (Steps 1–16) and
`docs/history/ewoclient-v2-phases.md` (Phases A–H).

## Window model (current — supersedes the Step 1/2 notes)

- The window is a **transparent floating card**. The 22 px-rounded card fills
  the window edge to edge: `app_window::CARD_INSET = 0`, and the launcher's
  cursor mirror `CARD_INSET_LP` **must match**, or the cursor drifts from the
  widgets. There's no outer drop shadow, because there's no margin to draw it in.
  The edge is the inset rose hairline in `draw_chrome_inner`.
- **Windows presents through Skia D3D12 + DirectComposition**
  (`CreateSwapChainForComposition`, premultiplied BGRA, window
  `WS_EX_NOREDIRECTIONBITMAP` via `with_no_redirection_bitmap`). That's the
  only way to get transparent corners on Win11, because WGL alpha is always
  composited opaque. Linux/Hyprland uses Skia GL on glutin and must prefer an
  alpha config.
- `win32.rs` sets **`DWMWCP_DONOTROUND`**, since we paint our own corners.
  `configure()` does no DWM alpha hack. `WM_NCHITTEST` gives the resize edges
  (8 lp) and a 32 lp caption. Minimize/close rects are excluded from the caption
  and handled before modals.
- Consequence: **no uncapped frame rate on Windows** (DWM composites at refresh).
  The dev-overlay vsync toggle only uncaps on the GL path.
- `wayland.rs` is a no-op stub and has never run on real Hyprland. No X11.

## Lifecycle, loop and leaks

- **Freeze-on-exit:** `main()` calls `std::process::exit(0)` right after
  `run_app` returns, skipping the `App`/backend `Drop` that deadlocked. Settings
  and instances persist on change, not on exit. Don't "fix" this by restoring
  the drop.
- **Unfocused-swap memory leak (fixed 2026-05-26):** skip `RedrawRequested`
  entirely (no Skia work, no present) when the launcher isn't the OS foreground
  window (`window::is_foreground` → `GetForegroundWindow`). winit's
  `Focused`/`Occluded`/`is_minimized` are unreliable on Win11 under another
  app's fullscreen. **`!self.focused` is intentionally not a skip signal**: the
  user may be on a second monitor.
- **Idle throttle:** after `IDLE_THRESHOLD_SECS` (0.5 s) without input the
  focused loop drops to `IDLE_FPS` (120). Input snaps it back. It never
  throttles on the Launching screen. `ControlFlow` sets the cadence, not
  `request_redraw`.
- **Leak-hunt instrumentation was removed** (2026-09, commit `1d20a71`): the
  counting allocator, mem/alloc logs and `process_memory` are gone. The
  foreground/occlusion skip was the fix, not instrumentation, and stays. To
  hunt a leak again, measure RSS from outside (Task Manager / `Get-Process`)
  before adding probes back.

## Module map

- `auth/`: Microsoft OAuth + PKCE → XBL → XSTS → Minecraft Services
  (`chain.rs`), loopback redirect on `127.0.0.1` (`loopback.rs`), background
  runner (`service.rs`), `AccountStore { active, accounts[] }` in `auth.toml`
  (refresh/social tokens DPAPI-sealed on Windows by `auth/secret.rs`, legacy
  plaintext migrates on load; 0600 plaintext elsewhere). The Entra app
  `f901fc74-7e36-439d-80a8-c2e548f47fdc` is on Mojang's allowlist.
- **Session lifetime** (`auth/mod.rs`): the Minecraft token's expiry is kept
  in memory (serde-skipped, like the token). `launch_auth` decides each
  launch: Offline (no account), Ready, NeedsRefresh (no token, or within
  `TOKEN_REFRESH_MARGIN` = 10 min of expiry, with a refresh token), or
  Unavailable. A NeedsRefresh launch waits for the refresh on the auth worker
  and re-enters; a failed refresh, nothing to refresh with, or an account
  switch mid-wait fails the launch visibly. **A signed-in account never
  launches offline.** A refresh response without `refresh_token` keeps the old
  one, and a refresh never moves the active-account pointer.
- `versions/`: master manifest (6 h cache) and per-version manifests (sha1,
  permanent cache). `manifest::is_supported` is a **curated allowlist**. Don't
  widen it without a reason.
- `downloads/`: staged per-version → loader manifest → client → libraries →
  asset index → assets, all sha1-verified into Mojang's layout under
  `shared/`. **Don't break that layout**: vanilla-launcher interop is a feature.
- `launch/`: `plan.rs` (token substitution), `natives.rs`, `spawn.rs` (stdout
  and stderr piped to the log panel), `jre.rs` (`pick_for_major`, `detect_all`,
  `resolve_pin`), `reaper.rs`, `prepare.rs` (launch preparation on a worker;
  the JRE re-scan after a bundled JRE install runs on a worker too). `runtime/`: Adoptium
  JRE auto-fetch into `runtime/<major>/jre/`.
- **Reaper** (`launch/reaper.rs`): kills only a process this launcher spawned
  and recorded (PID **and** creation time, in memory and `last-launch.pid`),
  still running, exactly `java(w).exe` or `rewo.exe`, and owning no visible
  window (a zombie after shutdown never has one; a live game always does).
- **Java runtime per instance**: the Instances dropdown is "Auto" followed by
  the detected JREs. `Instance::java_runtime` is that index: `0` = auto
  (`pick_for_major`), `N >= 1` = `detect_all()[N-1]`. A pin that is gone or
  below the version's required major falls back to auto at launch with a
  warning (`launch::jre::resolve_pin`).
- `window/icon.rs` hands the largest PNG frame of `assets/icon.ico` to
  winit's `with_window_icon` (the borderless window's taskbar/alt-tab icon;
  the exe's embedded icon only serves Explorer and shortcuts).
- `profile.rs`: client profiles (global, not per-instance): `profiles.toml` +
  `profiles/<name>/{client.toml,hud.toml,modules.toml,…}`; `settings.toml` is
  global-only. `profile::is_valid_name` rejects path-unsafe names.
- `keybind.rs`: `KeyChord` = GLFW key code + modifier mask (Minecraft's own key
  namespace). `REGISTRY` is generated from `ewo_core::modules`. The active
  profile's binds resolve to `<instance>/ewo-keybinds.txt` before each launch.
- **Native · Rewo instances**: `InstanceLoader::Native` spawns `rewo.exe live`.
  `EWO_DEV_SERVER=host:port` is the dev-join knob. `package.ps1` stages
  `rewo.exe` into `dist/`.

## Runtime conventions

- Disk root `<config>/EwoClient/` = `%APPDATA%\EwoClient` (Windows) or
  `$XDG_CONFIG_HOME/EwoClient`. It holds `shared/{versions,libraries,assets}`,
  `instances/<id>/` (stable id folders; worlds, natives, logs
  `launch_<ts>.log`; deleting an instance leaves its folder, by design for
  now), `dev/` (the HUD jar `ingame-mod/build.ps1` drops),
  `runtime/<major>/jre/`, `auth.toml`, `profiles.toml`, `profiles/<name>/…`,
  `settings.toml`, `instances.toml`, `versions_cache.json`.
- Every long task runs on a `std::thread` and reports over `mpsc`, polled once
  per frame. **No tokio/smol.**
- All HTTP goes through `ureq` (sync) with User-Agent
  `EwoClient/0.1 (+https://github.com/lewlone/ewoclient)`.
- **Runtime knobs** (environment, none has a compiled-in default host):
  `EWO_BOT_API_BASE` (the social API, https only; unset = social off),
  `EWO_LOBBY_ADDR` (the network widget's join address; unset = no JOIN),
  `EWO_LOADER_BASE` (overrides the EwoLoader manifest base; see
  `ewo-loader-integration.md`), `EWO_DEV_SERVER=host:port` (dev join for
  Native instances). Child processes get the launcher env minus
  `*TOKEN*`/`*SECRET*` variables.
- Release builds use `windows_subsystem = "windows"` (no console). The icon
  `assets/icon.ico` is embedded by `build.rs` via `winresource`. `package.ps1`
  builds and stages a self-contained `dist/EwoClient/` and repoints the Desktop
  shortcut. Re-run it after code changes.

## What NOT to do

- Don't prefetch Microsoft credentials at startup. Fetch lazily on launch.
- No Discord presence, telemetry, news or auto-update. The network calls are
  auth, the version manifest and asset CDN, the Adoptium JRE API, the Maven /
  Modrinth / GitHub hosts a loader manifest names (sha1-pinned), and social
  only once `EWO_BOT_API_BASE` is set and the account is linked.
- Don't write your own auth library logic from memory. Follow the documented
  Microsoft scheme exactly.
- Theme variants (Pearl/Obsidian/Champagne) are dropdown stubs. Don't add
  compat shims for them. The audio sliders connect to nothing.
