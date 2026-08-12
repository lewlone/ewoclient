# EwoClient — Velvet & Pearl Launcher

A pixel-perfect native port of the CSS/HTML "EwoClient" Minecraft launcher prototype to Rust + Skia, plus an in-game GUI for Minecraft Java Edition 26.x.

This file is the durable source of truth for the project. **Read it first when context is thin.** Every section has a purpose; none of it is decorative.

---

## What this project is

The user has a CSS/HTML prototype of a "Velvet & Pearl" themed Minecraft launcher (`EwoClient Prototype _fixed_.html` and `EwoClient · Velvet & Pearl prototype.htm` in the repo root). The goal is to port that prototype to a Rust native application that:

1. **Looks identical** to the prototype, side-by-side. If a user can tell which is which, we have failed.
2. **Runs at high frame rates** (target: 500fps on a 500Hz OLED, vsynced).
3. **Eventually renders inside Minecraft** as an in-game GUI via a custom Fabric-fork mod loader.

The user is targeting Windows 11 + CachyOS/Hyprland. macOS is not in scope.

## ⚠️ Two dead sibling directories — confirm you're in V3

There are **three** EwoClient-named directories on this Desktop:

- `c:\Users\valtteri\Desktop\EwoClient` — DEAD (TypeScript+Tauri prototype, abandoned 2026-04-20)
- `c:\Users\valtteri\Desktop\EwoClient-v2` — DEAD (earlier Rust workspace, abandoned 2026-04-27)
- `c:\Users\valtteri\Desktop\EwoClientV3` ← **live, this one**

The two dead directories have tombstone `CLAUDE.md` files redirecting here. If you're a fresh session and you opened the wrong one by accident, this file is the one to read. The live repo's `git remote -v` shows `lewlone/ewoclient`.

## Sibling project: EwoLoader

EwoClient depends on a sibling project **EwoLoader** — a friendly fork of `fabric-loader` living at `C:\Users\valtteri\Desktop\EwoLoaderV1` (private repo `lewlone/ewo-loader`). It's where the actual mod-loading happens at JVM time. Eight strip passes have shipped (see `STRIP_PLAN.md` in that repo) removing ~17k LoC of upstream cruft we don't need. The current bundle ships 16 user-toggleable mods + 5 infrastructure libs (Fabric API, fabric-language-kotlin, YACL, placeholder-api, Cloth Config) via the loader manifest at `EwoLoaderV1/manifest/0.1.0/26.1.json`.

**If you're working on launcher code that touches loader integration** (`crates/ewo-launcher/src/loaders/`, `downloads/job.rs::ensure_libraries`, `launch::merge`, instance loader handling), open EwoLoader in a second window. The two repos are tightly coupled at the loader-manifest contract.

**Author identity for both repos:** `lewlone <valtteri.e.saarinen@gmail.com>`. Set per-repo `git config user.name`/`user.email` if cloning fresh.

---

## Reference materials (read these when designing anything visual)

All in the repo root:

- **`StyleSheet1`** — first half of the extracted production CSS. Font @font-face declarations, `:root` theme tokens, app-window shell, backdrop layers (velvet folds, caustics, bokeh, pearl dust, petals), glass-panel primitive, breathing text, button, progress bar, slider, dropdown, status line, launcher screen layouts, state/layout/error pickers, meta pills, accessibility motion-reduction.
- **`StyleSheet2`** — second half. Main menu (asym layout), screen head, instances screen, launching screen with log, settings (4 tabs: graphics, audio, paths, advanced), instance rename, new-instance modal, settings toggles/path-fields/danger buttons.
- **`EwoClient · Velvet & Pearl prototype.htm`** — saved page. Contains the JSX-via-babel React components. Most importantly:
  - Line ~6438: `VelvetFolds` component — SVG `feTurbulence` filter (`fractalNoise`, baseFrequency `0.007 0.011`, numOctaves 2, seed 4) + `feDisplacementMap scale=28`, plus 3 oklch radial-gradient layers.
  - Line ~6482: `PearlDust` component — full canvas particle system (90 airborne + 60 settled at density=1, with `disturb` mechanic on backdrop click).
  - Line ~6604: `BokehOrb` — 4 tint variants (berry, champagne, lavender, opal).
  - Line ~6628: `Petals` component — 4 baseline → 140 burst on celebrate, teardrop bezier shape.
- **`style/*.png`** — 10 reference screenshots covering every screen and dropdown state. Use these for pixel parity validation.
- **`Firefox 2026-04-28 21.18 profile.json`** — performance recording (90MB, optional).

The user's screenshots are also where the truth lives for color values and proportions when the CSS leaves room for interpretation.

---

## Non-negotiables (design rules — never violate)

These come from the author's CSS comments and design intent. They are load-bearing:

1. **"Motion is load-bearing. Nothing here is static."** Every surface in the prototype animates continuously. Static screens fail the parity test.
2. **Don't transform anything that contains text.** Scaling text containers forces sub-pixel re-rasterization of every glyph and visibly softens dense text. Animate adjacent layers (e.g. tint layers) instead. (See `.glass-panel.breathing > .glass-tint` in StyleSheet1.)
3. **Don't blur during entrance animations on text-bearing surfaces.** Same reason. The author explicitly removed `filter: blur` from modal entrance for this. (See `@keyframes modal-card-in` comment in StyleSheet2.)
4. **The `--silk` curve `cubic-bezier(0.22, 1, 0.36, 1)` is the project's signature easing.** Use it for nearly every transition. Exceptions: linear loops (rim slides, sheen sweeps) and the few specific-duration animations called out in CSS.
5. **Pearl dust is on every screen.** It's not decoration; it's identity. The same applies to velvet folds, caustics, and bokeh.
6. **`prefers-reduced-motion` (Windows: `SystemParametersInfo SPI_GETCLIENTAREAANIMATION`) collapses all animations to <1ms.** Honor it.
7. **`isolation: isolate` semantics matter for screen-blend layers.** Backdrop is a separate stacking context; this prevents `mix-blend-mode: screen` particles from blending into panel content above. Reproduce equivalent compositor isolation in our render graph.

---

## Architecture (locked decisions)

- **Renderer:** Skia (`skia-safe` crate), with future migration to Vulkan when Minecraft 26.2's Vulkan renderer stabilizes. **Not raw wgpu.** Skia gives us 1-2-line equivalents for every gradient, blur, shadow, font shaping, and color-space conversion in the prototype. wgpu would require ~5000 lines of custom shaders to match. For escape-hatch custom shading, use SkSL (Skia's shader language).
  - **Presentation backend is per-platform** — both live in `crates/ewo-render/src/gl_backend.rs` behind one `GlBackend` API (`new`/`resize`/`render`/`set_vsync`), cfg-selected:
    - **Windows → Skia D3D12 + DirectComposition.** Skia renders into a composition swapchain (`IDXGIFactory4::CreateSwapChainForComposition`, `DXGI_FORMAT_B8G8R8A8_UNORM`, `DXGI_ALPHA_MODE_PREMULTIPLIED`, flip-sequential, 2 buffers) presented through a DComp visual on a `WS_EX_NOREDIRECTIONBITMAP` window. **This is the ONLY way to get the transparent rounded corners on Win11.** A WGL/GL swapchain's alpha is composited *opaque* by DWM no matter what — `with_transparent(true)`, an alpha-capable GL config, `DwmEnableBlurBehindWindow` (empty region), and `DwmExtendFrameIntoClientArea` (sheet-of-glass) were all tried and all produced black corners. Requires `skia-safe` feature `d3d` (prebuilts exist — no LLVM/source build) + `windows` features `Win32_Graphics_{Direct3D,Direct3D12,Dxgi,Dxgi_Common,DirectComposition}`. Surface origin is `TopLeft` (D3D convention) and color type `BGRA8888` (matches the swapchain — no channel swizzle).
    - **Linux/Hyprland → Skia GL** on a glutin window surface (the original step-2 path). Wayland compositors honor the GL surface's alpha directly, so no DComp equivalent is needed. The GL config selection *must* prefer an alpha-bearing config (glutin's `with_alpha_size(8)` is only a hint).
  - **Consequence — no uncapped frame rate on Windows.** A composition swapchain is always composited by DWM at the display refresh; there is no tearing/uncapped present path. The 500fps-OLED target is met *vsynced* (present every vblank, up to 500Hz); the dev-overlay vsync toggle can't push past the monitor refresh on Windows. GL/Linux still uncaps via `SwapInterval::DontWait`.
- **Layout:** `taffy` crate (CSS flex/grid-compatible). Don't hand-roll.
- **Text shaping:** Skia's bundled HarfBuzz via `skia-safe::Shaper`. Variable font axes (Fraunces SOFT/WONK, Newsreader opsz/wght) are passed through `font-variation-settings`.
- **Window:** `winit` 0.30, with custom-frame (no native titlebar). Per-platform implementations live in `crates/ewo-launcher/src/window/`.
  - `win32.rs` — `WM_NCHITTEST` for hit zones, `WM_NCCALCSIZE` for stripping non-client area, `DwmSetWindowAttribute(DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND)` so DWM doesn't round over our own painted 22px corners. The window is created `WS_EX_NOREDIRECTIONBITMAP` (via winit's `with_no_redirection_bitmap`) so the DirectComposition backend's alpha isn't composited against an opaque redirection surface. **`configure()` does NOT do any DWM alpha hack** — per-pixel transparency comes entirely from the DComp presentation backend (see Renderer above); the old `DwmEnableBlurBehindWindow` code was removed as a dead end.
  - `wayland.rs` — Hyprland (wlroots) via `xdg-decoration` mode `client-side`, `xdg_toplevel.move()` / `resize()` for drag/resize. We paint our own shadow, rim, and rounded corners via the alpha buffer.
  - **No X11 implementation.** User targets Wayland-only on Linux.
- **Concurrency:** Single-threaded by default. No tokio/smol. Particle update may use `rayon` if profiling demands it.
- **Settings persistence:** TOML at `%APPDATA%/EwoClient/settings.toml` on Windows / `$XDG_CONFIG_HOME/EwoClient/settings.toml` on Linux, via the `dirs` crate.
- **Fonts:** Bundled in `assets/fonts/` as variable `.ttf` files. Loaded directly via FreeType inside Skia. **No reliance on system fonts.** English-only (Latin subset is sufficient for v1; full subsets ship for free since variable TTFs are small).
- **Offline-first.** "OFFLINE FIRST. NOTHING PHONES HOME." — quoted from the prototype's main menu. No telemetry, no analytics, no auto-update.

---

## v1 scope (locked — implementation complete)

- **Visual replica only.** Not a functional launcher. The "Launch" button plays the launching animation (synthetic progress curve + canned log lines) but does not actually launch Minecraft. Real Microsoft auth, JVM launching, version downloading, and instance management are out of scope for v1 — they belong in v2 (see "v2 plan" section near the end of this file).
- **Velvet theme only.** Pearl, Obsidian, and Champagne theme variants are stubbed in the dropdown but not implemented. Theme switcher is wired so adding them later is one struct.
- **Audio: UI only.** The Audio settings page renders sliders for Master/Music/Effects/Ambient hum but they don't connect to a real audio engine. Real ambient audio is post-v1.
- **Custom frame** on both Windows and Linux. No native window decorations.
- **Dev controls behind `--dev` flag.** The state-picker, layout-picker, error-picker, and tweaks-panel from the prototype are debug overlays that ship behind a runtime flag. The tweaks-panel exposes the `--motion-speed`/`--breath-amp`/`--density`/`--warmth`/`--accent-hue-shift` tokens for live tuning.
- **English only.** No localization framework.
- **Standalone host first, Minecraft host later.** Sequential, not parallel. Don't start the JNI/Fabric-fork work until the standalone is rendering at parity.

---

## Crate layout

```
ewolauncher/                 # workspace root
├── Cargo.toml               # workspace manifest, pinned deps, shared profiles
├── rust-toolchain.toml      # stable
├── .cargo/config.toml       # build optimizations
├── CLAUDE.md                # this file
├── reference/               # (optional, currently in repo root)
├── assets/
│   ├── fonts/               # Fraunces, Newsreader, JetBrains Mono variable .ttf
│   └── shaders/             # SkSL shaders + custom GLSL when needed
└── crates/
    ├── ewo-core/            # types, theme tokens, easing, animation engine, app state. NO graphics deps.
    ├── ewo-render/          # Skia wrapper, text shaping, particles, fx, primitives, frame graph
    ├── ewo-ui/              # taffy layout, widgets (vbtn/vslider/vdrop/glass-panel/...), screens
    └── ewo-launcher/        # binary: winit + custom frame + dev overlay + main loop
```

Each crate has a `lib.rs` (or `main.rs` for the launcher) with module docs. Don't add a fifth crate without a structural reason — boundaries should follow what changes together.

---

## Build sequence (numbered, sequential — don't skip ahead)

This was the path to v1. Each step was a working program. **As of 2026-05-03, every step is structurally complete on Windows.** v1 visual replica is done modulo the two validation items called out below the table — these are checks you do, not code you write.

| # | Step | Notes | Status |
|---|---|---|---|
| 1 | **Bare custom-frame Windows window.** Black, 1180×720, draggable by the top, resizable by edges, rounded corners. winit + windows-sys, no rendering yet. | Verify on Win11 first, then verify on Hyprland. | **DONE (Win11)** — Hyprland verification pending |
| 2 | **Skia GL backend hooked up.** Same window, drawing one rounded rect with the app-window 3-layer box-shadow stack. | First Skia compile took ~few min (prebuilts used; LLVM not needed). | **DONE** |
| 3 | **Backdrop layer.** Wine radial + caustics + bokeh + vignette. No turbulence/particles yet. | Should look like screenshots minus the dust. | **DONE** |
| 4 | **Pearl dust particles.** Port `PearlDust` to instanced GPU draws. 90 airborne + 60 settled, including `disturb()`. | Airborne halos now use 3-stop radial-gradient shaders (matches the prototype's JS exactly; the mask-blur halos were a placeholder). | **DONE** |
| 5 | **Velvet folds.** 3 oklch radial layers, 40px blur, screen-blend 85%, fold-drift transforms. | Turbulence + displacement (`feTurbulence` + `feDisplacementMap` scale=28) shipped. | **DONE** |
| 6 | **Petals.** Same particle pipeline as pearl dust, different shape/motion. 4 baseline → 140 burst. | | **DONE** |
| 7 | **Glass panel primitive.** The canonical composite (refract blur 32px + tint gradient + 4 animated rim lights + breath). One static panel rendered. | `widgets/glass_panel.rs` — backdrop blur via `SaveLayerRec::backdrop`, 135° linear + top radial tint with 8s breath, 4 edge gradients on a 12s linear cycle. | **DONE** |
| 8 | **Text engine.** Variable axes + per-glyph control. Render "EwoClient" in Fraunces with WONK=1, SOFT=50. | Variable axes (`fraunces_axes`) + per-glyph rendering via tracked-em path. Full Skia `Shaper` (proper kerning + ligatures) intentionally skipped — visible benefit is negligible at the sizes this app uses. | **DONE for v1** |
| 9 | **Breathing text widget.** Per-glyph positions (8s letter-spacing 0em ↔ 0.02em). Hover-glow stagger at 30/60/90/120ms via `:nth-child` analog. | `text::draw_breathing` / `draw_breathing_glow` + `HoverGlowState`. Hover-glow stagger wired to the main-menu heading (`heading_bounds` hover-tracked from `App`). | **DONE** |
| 10 | **vbtn.** Tint, animated rim, sheen sweep, hover lift, press scale, ripple expansion, cursor specks. Click handlers. | All four chrome layers + ripple (4-slot fixed pool, 6→320px silk over 620ms) + cursor specks (8-slot pool, 40ms throttle, rose↔champagne hue jitter, drift up + fade). | **DONE** |
| 11 | **Main menu screen.** | Full composition with breathing heading + hover-glow stagger, hover affordances on items, footer, click-to-disturb hint. | **DONE** |
| 12 | All widgets. | vbtn, vslider, vdrop (portaled with flip-up + scroll + per-row stagger), vstatus, pbar (Normal + Complete + 3 error variants Rose/Recede/Shimmer), vtoggle, vghost_btn (Pearl + Danger), vpathfield, meta-pill, scrollbar. | **DONE** |
| 13 | All screens. | Main menu, instances (with full detail, mods list, rename, sort, scrolling), settings (4 tabs all wired), launching (with synthetic log + error simulation). | **DONE** |
| 14 | Modal system. | New-instance modal (with form validation + submit) + About modal. Both with shroud + glass card chrome + entrance silk + Esc/outside-click/button dismiss. | **DONE** |
| 15 | Dev overlay behind `--dev`. | Tweaks panel (5 token sliders) + FPS HUD + worst-frame counter + Vsync toggle + Reset + Sim-error cycle. State/layout pickers were intentionally skipped (prototype-debug-specific, low parity-tuning value; the tab bar + Launch flow already reach those states). | **DONE for v1** |
| 16 | Polish + perf pass. | All originally-deferred items shipped: pbar error variants, vbtn ripple+specks, mods + dropdown row stagger, meta-pill, hover-glow stagger, frame-stat HUD, vsync toggle, pearl-dust gradient halos. | **DONE** |

**Two validation items remain** before declaring v1 fully validated. Both are checks the user does, not code Claude writes:

1. **Hyprland verification.** Step 1's `wayland.rs` is a no-op stub (winit's `with_decorations(false)` should be enough on wlroots). Nobody has run the launcher on actual Hyprland. Custom-frame edge cases (drag, resize, shadow rendering) need a smoke test there.
2. **Side-by-side pixel-parity pass.** No formal screen-by-screen comparison vs `style/*.png` has been logged. Visual verification has happened iteratively during development but not as a structured pass. If parity gaps surface, they're step-16 polish items by definition.

## What's intentionally NOT in v1 (and never was)

- ~~Real Microsoft OAuth, JVM launching, instance directories on disk, asset/library downloads.~~ All shipped in v2 (see "v2 status" section below).
- Pearl / Obsidian / Champagne theme variants. Stubs in the dropdown only.
- Real text editing in the new-instance Name field (the "Rename" affordance on the Instances detail panel does have working text input — the new-instance modal does not yet).
- Browse buttons opening real OS file dialogs.
- Localization. English-only.
- Audio. The Audio settings sliders don't connect to anything.
- macOS, X11.

---

## Velvet theme tokens (canonical — keep in sync with `ewo-core::theme::Theme::VELVET`)

```
--bg-core         #000000
--bg-wine-a       #0A0006
--bg-wine-b       #120010
--panel-glass     rgba(180, 130, 160, 0.06)
--text-pearl      #F4E8EA
--text-mauve      #9A8087
--accent-berry    #B47491
--accent-rose     #E5B8C5
--accent-lav      #C9A5D4
--accent-champ    #E8D4A8
--accent-ember    #C96A7A   (error)

--silk            cubic-bezier(0.22, 1, 0.36, 1)

--motion-speed    1.0   (user-tunable)
--breath-amp      1.0   (user-tunable)
--density         1.0   (user-tunable)
--warmth          0.6   (user-tunable, shifts velvet hues berry↔champagne)
--accent-hue-shift 0deg (user-tunable)
```

Non-tokenized hues that appear inline in CSS — add to palette as needed:
- `#FFF6F0` warm-white (focus highlights)
- `#FFF0F4` pearl highlight (slider cores)
- `#C4AFB5` mid-pearl (back buttons, metadata)
- `#6B555C` deep mauve (tertiary text, log timestamps)
- `#D4889A` error text
- `#A35A6C`, `#8A6E7E` error gradient stops

Fonts (bundled as variable TTF in `assets/fonts/`):
- **Fraunces** — display headings. Variable axes: `SOFT` 0-100, `WONK` 0/1, `opsz` 9-144, `wght` 100-900. Both italic + roman.
- **Newsreader** — body, italic taglines, button labels. Variable: `opsz`, `wght` 200-800. Both italic + roman.
- **JetBrains Mono** — monospace eyebrows, log lines, tracked labels. Static `wght` 400 + 500. Both italic + roman.

The launcher title uses `font-variation-settings: "SOFT" 50, "WONK" 1` — this is non-default for Fraunces and matters for visual identity.

---

## Render graph (one frame, top to bottom)

```
1. Clear to #000
2. Backdrop render target:
   2a. Wine radial-gradient (50% 55%, #06000A → #000 70%)
   2b. Velvet folds: 3 oklch radial layers with fold-drift transform; sample turbulence texture in fragment shader (displacement scale 28); blur 40px; screen-blend
   2c. Caustics: 2 layers × 3-5 oklch radial gradients; blur 30px; screen-blend; periods 38s + 52s reversed
   2d. Bokeh orb: 50vmin radial; blur 40px; screen-blend; 60s linear cross
   2e. Vignette
   2f. Pearl dust canvas (screen-blend)
   2g. Petals canvas (regular alpha)
3. Screen-stage render target:
   3a. For each glass-panel: backdrop blur of layer 2 → tint gradient → 4 animated rim lights → inner content
   3b. Widgets layered onto their parent panels
   3c. Text passes last; never inside scaled containers
4. Dev overlay (if --dev): state-picker, layout-picker, error-picker, tweaks-panel
5. Window chrome: app-window 3-layer box-shadow + inset hairline rim
6. Present
```

This implies at minimum **2 offscreen render targets** plus ping-pong targets for separable Gaussian blur. Skia handles offscreen surfaces via `Surface::new_render_target`.

---

## Things NOT to do

- **Don't pre-optimize for a wgpu rewrite.** Skia is the chosen tool, not a stepping stone. The prototype's effects are all 2D — Skia is more capable here than wgpu, not less.
- **Don't add async runtimes.** No tokio, no smol. Single-threaded with optional rayon for parallel particle updates if profiling demands it.
- **Don't add backwards-compat shims for theme variants we haven't implemented.** Velvet is the only theme in v1. Pearl/Obsidian/Champagne are stubs in the dropdown only.
- **Don't connect the audio sliders to anything in v1.** UI-only.
- **Don't try to make the Launch button actually launch Minecraft in v1.** Synthetic progress + canned log only.
- **Don't pull in a CSS parser.** The styling is reimplemented in Rust as theme tokens + widget code, not loaded from `.css` files at runtime.
- **Don't move the prototype reference files** (`EwoClient Prototype _fixed_.html`, `StyleSheet1`, `StyleSheet2`, `style/`) without asking the user. They're the source of truth for parity work.
- **Don't add tests that depend on the prototype HTML rendering correctly in a headless browser.** Pixel parity is validated by side-by-side screenshots, manually, by the user.

---

## When context gets muddy: where to look

1. **This file (CLAUDE.md)** — locked decisions, scope, non-negotiables, build sequence.
2. **`StyleSheet1` and `StyleSheet2`** — every CSS rule, with the original author's perf-decision comments embedded inline. They explain *why* certain values were chosen (e.g. blur 40→32, modal entrance blur removed, etc.).
3. **`EwoClient · Velvet & Pearl prototype.htm`** — JSX implementation of the JS-driven pieces (particle systems, SVG turbulence filter).
4. **`style/*.png`** — visual ground truth.
5. **`ewo-core::theme::Theme::VELVET`** — canonical Rust palette, must match the CSS `:root` values byte-for-byte.

If the user asks "is X already done", check the build-sequence table above for the latest step status, then `git log` for what's actually committed. The table above is the durable plan; commits are the durable record.

---

## Glossary (project-specific terms)

- **Velvet** — the default theme name and aesthetic identity. Dark aubergine + pearl/rose accents.
- **Pearl** — a lighter theme variant, not implemented in v1.
- **Boudoir aesthetic** — the author's term for the overall design language. Soft, intimate, warm-dark, high-detail, high-motion.
- **Glass panel** — the canonical surface primitive. Refract blur + tint gradient + 4 rim lights + breath.
- **Pearl dust** — the canvas-driven particle system of soft pink/cream motes. 150 particles total at density=1.
- **Petals** — the canvas-driven flower-petal particle system. 4 baseline, 140 during celebrate.
- **Velvet folds** — the SVG turbulence + displacement effect that gives the backdrop its silk-fabric character.
- **Caustics** — slow oklch radial-gradient drift, screen-blended. Reads as iridescent light on silk.
- **Bokeh orb** — single huge soft-pearl blob that crosses the backdrop every 60s. 4 tint variants.
- **Breathing text** — Fraunces titles split per-glyph, with 8s letter-spacing oscillation 0em ↔ 0.02em.
- **Silk easing** — `cubic-bezier(0.22, 1, 0.36, 1)`. The project's signature curve.
- **Celebrate state** — success animation. Backdrop brightens, petals burst, progress bars get extra glow.
- **bt-g** — per-glyph span class in the prototype CSS, the unit of breathing-text animation.
- **Disturb** — the click-the-backdrop affordance. Settled pearl dust is pushed around briefly, decaying at `*= 0.93/frame`.

---

## Step 1 implementation notes (current state)

- Window: borderless 1180×720, min size 800×520 (`with_decorations(false)` + DPI-aware logical sizes).
- Drag/resize: `main.rs::hit_test()` maps cursor position to a `Zone` (8 logical-px resize border, 32 logical-px caption strip). Left-mouse-down routes to `Window::drag_window()` or `Window::drag_resize_window(dir)` — winit handles the OS plumbing.
- Cursor icons swap on the resize edges via `update_cursor_icon()` so the user gets visual feedback before clicking.
- Win11 rounded corners: `DwmSetWindowAttribute(DWMWA_WINDOW_CORNER_PREFERENCE = DWMWCP_ROUND)` applied in `window/win32.rs::configure()`. DWM-provided shadow comes for free until step 2 paints its own.
- Painting: `softbuffer` 0.4 fills the whole client area with `0x000000` on `RedrawRequested`. **This is intentionally throwaway** — softbuffer gets removed in step 2 when the Skia GL surface takes over.
- Hyprland (`window/wayland.rs`) is a no-op stub — winit's `with_decorations(false)` triggers `xdg-decoration` mode `client-side` automatically. Verify on actual Hyprland before committing more.

**Known limitation (deferred to step 11+):** the entire top 32 logical-px strip is currently click-to-drag. When the top tab bar (Main Menu / Instances / Settings / Launcher) lands as real widgets, those zones need an exclusion-list API on `hit_test` so widget clicks aren't swallowed by the drag handler. Pattern: widgets register hit-rects with the platform layer; `hit_test` returns `Zone::Caption` only if no widget rect contains the cursor.

## Step 2 implementation notes

- `ewo-render::GlBackend` (`crates/ewo-render/src/gl_backend.rs`) — owns the glutin display/config/context/surface and a `skia_safe::gpu::DirectContext`. Constructor: `GlBackend::new(event_loop, Arc<Window>)`. Public API: `resize(w, h)` and `render(|canvas, w, h| { ... })`. The render closure runs once per frame and swaps buffers + flushes Skia internally.
- GL 3.3 core profile, alpha=8, stencil=8, MSAA picks the highest available sample count from glutin's config list. VSync wait=1 by default — change to `SwapInterval::DontWait` (uncapped) when measuring 500fps target.
- Skia setup uses the **non-deprecated** `gpu::direct_contexts::make_gl(interface, None)` — `DirectContext::new_gl` is deprecated as of skia-safe 0.78.
- `ewo-render::app_window::draw(canvas, w, h)` — paints the prototype's `.app-window` chrome:
  - Clear to `#000` (matches `--bg-core`)
  - Outer berry glow shadow (CSS `0 40 120 -20 rgba(180,100,140,0.35)`)
  - Outer black drop shadow (`0 12 40 -8 rgba(0,0,0,0.8)`)
  - Card body — solid black rrect at 28px inset, radius 22
  - Inset hairline rim (1px stroke, `rgba(229,184,197,0.08)`)
  - Inner berry glow approximation (`::after` inset glow), clipped to card
- CSS-blur-radius → Skia-blur-sigma conversion: `σ = blur_radius / 2` (industry-standard convention).
- Spread is implemented manually by inflating/deflating the shadow's source rrect (Skia's `MaskFilter::blur` doesn't have a spread param).

**Skia prebuilt binaries** worked on this machine — no LLVM install needed. The build downloaded prebuilt skia-bindings for `skia-safe = "0.78"` features `["gl", "textlayout"]` on `x86_64-pc-windows-msvc` and linked them. If those prebuilts ever stop being available (version bump, target change), `winget install LLVM.LLVM` is the fallback.

**Visual verification still pending** — code compiles and runs without crashing, but the user should eyeball the card vs `style/mainMenu.png` to confirm the shadow stack reads correctly. If the inner berry glow looks too sharp or too diffuse, the `inner_glow` stroke width / mask sigma in `app_window.rs` are the knobs.

---

## Step 3 implementation notes

- `ewo-render::frame::Clock` — `tick()` updates `elapsed` (wall-time seconds since startup) and `dt` (frame delta). Called in `RedrawRequested`. App schedules continuous redraws via `ControlFlow::Poll` + `request_redraw()` so animations actually run.
- `ewo-core::color::OkLch::to_linear_srgb()` and `to_srgba()` — full CSS Color Module Level 4 §11.3 oklch → oklab → LMS → linear-sRGB pipeline. Caustics use the linear-sRGB path; gamma-encoded path exists for callers that need u8.
- `ewo-core::easing::CubicBezier::eval()` — Newton-Raphson solver, 8 iterations, converges within 1e-6 of the target x. `SILK` constant exposes the project's signature easing.
- `ewo-render::backdrop` module:
  - `wine.rs` — radial-gradient(ellipse at 35% 40%, wine-b 0%, wine-a 45%, #000 80%). Local matrix scales the radial circle into the farthest-corner ellipse.
  - `caustics.rs` — 2 layers (3 + 2 oklch radial blobs each), `mix-blend-mode: screen`, `filter: blur(30px)` via `image_filters::blur` inside a `save_layer`. 38s linear period for layer A; 52s reversed for layer B. The `caustic-drift` keyframe (`translate(3%, -4%) rotate(6deg)`) is interpolated linearly. `inset: -20%` over-extension is preserved so the animated transform doesn't expose edges.
  - `bokeh.rs` — single 50vmin pearl-pink orb. Crosses every 60s with the prototype's 5-stop opacity envelope (0% → 15% → 50% → 85% → 100%) and 3-keyframe translate/scale. Berry tint variant only for v1.
  - `vignette.rs` — `radial-gradient(ellipse at 50% 60%, transparent 30%, rgba(0,0,0,0.65) 85%)`. Drawn last in the backdrop stack so it dims everything beneath.
- `app_window.rs` refactored: `draw_chrome_outer` (clear + outer shadows + black card body), `draw_chrome_inner` (inset rim + inner berry glow), and `draw_frame` orchestrating outer → backdrop (clipped to card rrect, translated to card-local coords) → inner.
- Continuous-redraw loop runs at the GL surface's swap interval (currently VSync). For uncapped 500fps testing later, change `SwapInterval::Wait(1)` to `SwapInterval::DontWait` in `gl_backend.rs`.

**Visual verification still pending** — the binary runs without crashing, but the user should eyeball the result against `style/mainMenu.png` (mainly to confirm the wine tint, caustic shimmer, and bokeh orb are all readable through the vignette).

**Known approximations** (carry forward to step 16 polish pass):
- Caustic blob radius is approximated as a circle scaled by local matrix; CSS `radial-gradient(40% 30% at ...)` is an exact ellipse. With 30px blur the difference is essentially invisible, but it'll show up under heavy zoom.
- Caustic drift uses linear phase (matches `animation-timing-function: linear`).
- Bokeh opacity envelope uses linear lerp between keyframes; the prototype doesn't specify a per-keyframe easing, so default `ease` would be more correct. Linear is close enough at this opacity range.

## Steps 4 + 5 implementation notes

Done together because step 5 (velvet folds) closes the bigger visible gap (the dominant purple wash) and step 4 (pearl dust) layers cleanly on top.

### Backdrop refactor

`ewo-render::backdrop::Backdrop` is now a struct that owns the stateful subsystems (currently `PearlDust`; future: `Petals`). Stateless layers stay as free `draw()` functions. Public API:

```
Backdrop::new(w, h, &Settings)       // construct + spawn particles
Backdrop::resize(w, h, &Settings)    // regenerate particles
Backdrop::update(dt: f32)            // tick particle state
Backdrop::disturb()                  // bump pearl-dust disturb to 1.0
Backdrop::draw(canvas, w, h, time, theme, settings)
```

`app_window::draw_frame` now takes `&Backdrop` and `&Settings` so chrome + backdrop draw together. The standalone `app_window::draw_chrome_outer` / `draw_chrome_inner` helpers remain available for callers who manage their own backdrop layering. The deprecated `app_window::draw` fallback was removed.

Click-on-backdrop in `main.rs` calls `Backdrop::disturb()`. The "click backdrop to disturb rose-dust" prototype affordance is wired.

### Velvet folds (`backdrop::velvet_folds`)

- 3 oklch radial gradient layers, parameterized by `Settings::warmth` (default 0.6 → hues 333° / 302° / 41°).
- Lightness 0.22-0.28, alpha 0.55-0.90 — these are mid-low values that, when screen-blended 85% over the wine, paint the strong purple ground.
- 40px CSS blur (sigma 20) applied at the layer level via `image_filters::blur` inside a `save_layer`.
- `fold-drift` keyframes interpolated as a triangle wave (CSS `infinite alternate`) with smoothstep easing (cubic-bezier ease-in-out approximation). Periods 22s/28s/34s; the third layer is delayed -14s and dimmed 0.6.
- `inset: -20%` over-extension preserved so the animated transform doesn't expose layer edges.
- **Deferred to step 16:** SVG `feTurbulence` (fractalNoise, baseFreq 0.007 0.011, octaves 2, seed 4) + `feDisplacementMap` scale=28. Skia has both (`shaders::fractal_noise` + `image_filters::displacement_map`). Without it the layers read smooth; with it they read as silk-fabric folds. Visual difference is the "fabric" texture overlaid on the color.

### Pearl dust (`backdrop::pearl_dust::PearlDust`)

- 90 airborne motes + 60 settled motes at `density=1`. Counts scale with `Settings::density`.
- Per-particle state matches the JS reference exactly. Airborne motes have `(x, y, vx, vy, r, life, maxLife, shinePhase, shineFreq)`. Settled motes have `(x, y, r, twinkle, jitter)` — `jitter` is a per-particle baked-in deterministic horizontal direction for `disturb` push (the JS uses `Math.random()` per-frame for this; baking is faster and the visual result is indistinguishable).
- `update(dt)` is dt-aware (the JS reference is implicit-60Hz; we scale by `dt * 60`, clamped to ≤4 to avoid huge jumps after stalls).
- `disturb()` sets `disturb_amount = 1.0`; decays at `*= 0.93^step` per tick; pushes settled motes by `disturb * 18 * jitter` horizontally and `-disturb * 12` vertically (briefly lifting them off the bottom).
- Render: per-particle blurred-disk halo (4× radius, BlurStyle::Normal mask filter) + a bright pearl core. Faster than per-particle radial-gradient shaders; visual difference is negligible at this size. **Step 16 polish option:** switch halos to true 3-stop radial gradients to match the JS exactly.
- Layer-level `BlendMode::Screen` so all motes brighten the layers beneath.
- Uses `rand::thread_rng()` inline at construction/respawn; no stored RNG.

### Layer order in `Backdrop::draw`

```
wine → velvet_folds → caustics → bokeh → pearl_dust → vignette
```

Differs slightly from the prototype's DOM structure (where vignette is inside vf-root and only dims velvet folds): we apply vignette globally for simpler composition. Visual difference is minor — caustics/bokeh/dust naturally fade at the edges via their own gradients.

## Step 6 + 7+ implementation notes

Steps 6 (petals), 7 (glass panel deferred), 8 (text-engine partial), and 11 (main-menu static parity) all landed together because they fed each other — petals + velvet-folds turbulence + text were the visible-impact priorities, and the main menu's static composition was achievable without glass panels (which the screen doesn't use).

### Petals (`ewo-render::backdrop::petals`)

- 4 baseline petals at `density=1`, surges to 140 during `celebrate` state.
- Per-petal state: `{ x, y, vx, vy, rot, vrot, size, hue, alpha, flutter }`.
- Motion: `x += vx + sin(flutter)*0.4`; `y += vy`; `rot += vrot`; `flutter += 0.03` per ~60Hz step.
- Shape: teardrop bezier (`moveTo(0, -size)` + 2 cubic curves back to start).
- Color: 3-stop linear gradient across petal width in oklch (`0.75 0.09 hue / α` → `0.82 0.10 hue+10 / α` → `0.68 0.08 hue-10 / α*0.6`).
- Alpha-blend (NOT screen-blend) — petals are foreground motes, not light contributors.
- Burst on celebrate: spawn from upper-left every ~30ms; off-screen petals are *deleted* (not recycled) so the population drains back to baseline when celebrate ends.
- API: `Backdrop::celebrate(bool)` exposed for when launch-success integration lands; not yet wired to a trigger.

### Velvet-folds turbulence (pulled from step 16)

- `feTurbulence` (fractalNoise, baseFreq 0.007 0.011, octaves 2, seed 4) + `feDisplacementMap` scale=28 chained via Skia primitives (`shaders::fractal_noise` + `image_filters::shader` + `image_filters::displacement_map`). Static turbulence (animating it was the original perf hog per author's CSS comments); silky motion comes from the `fold-drift` gradient transforms.
- Filter chain: `blur(40px) → displacement(28, source=fractalNoise)`. Set on the layer's `Paint::set_image_filter`.
- **Inner-layer blend mode is `Screen`, not default SrcOver**. The CSS `.vf-layer { mix-blend-mode: screen }` screen-blends each layer separately onto its parent. Without this our 3 folds were alpha-blending together (averaging) before the outer screen-composite — visibly dimmer than the browser. Switching to inner Screen matches the spec and brightens the whole purple wash.

### Text engine (`ewo-render::text`)

- `FontStore` searches `assets/fonts/` for `Fraunces*`, `Newsreader*`, `Newsreader-Italic*`, `JetBrainsMono*` — case-insensitive prefix, italic vs upright filtered by filename. Falls back to system default with a `log::warn` if missing.
- Variable axes wired via `Typeface::clone_with_arguments` + `FontArguments::set_variation_design_position` + `VariationPosition::Coordinate { axis: FourByteTag, value }`. `FontStore::fraunces_axes(size, soft, wonk, weight, opsz)` is the public entry point. `opsz=None` tracks the size (matches CSS `font-optical-sizing: auto`).
- Per-glyph control (Skia `Shaper`, glyph positions, kerning) is **deferred to step 9** — not needed for static screens; lands when breathing text demands it. CSS `letter-spacing` approximation is currently a string-iter helper (`draw_tracked` in `screens/main_menu.rs`); will be replaced by glyph-positioned spacing when step 9 lands.

### Main menu static composition (`ewo-render::screens::main_menu`)

- Renders the full `.screen-asym` layout: top tab bar (centered pill, MAIN MENU active), top-right "SETTINGS" link, "EwoClient" heading + V0.1 subtitle + italic Newsreader tagline, four-item right column (Instances/Settings/About/Quit-to-desktop with JetBrains Mono submenus), footer ("OFFLINE FIRST..."), and the "click backdrop to disturb rose-dust" hint.
- Coordinate space: card-local. Caller (`app_window::draw_frame`) clips and translates to card-local before invoking.
- All static — no hover lift, no caret animation, no breathing yet. Those land with step 9 (breathing) and step 10 (vbtn).

### Lighting tuning notes

A few divergences from CSS spec values, kept and documented because they read better on OLED:
- **Bokeh peak opacity** raised from CSS keyframes (0.35/0.45/0.30) to (0.45/0.55/0.40). Skia's straight composite reads dimmer than the browser's gamma-aware path at the same numbers; visual parity beats numeric parity.
- **Vignette darkening** reduced from CSS `rgba(0,0,0,0.65)` outer to `rgba(0,0,0,0.50)`. Stays dark, but doesn't crush the layers underneath.
- **Inner berry glow** alpha bumped from CSS `0.15` to `0.22` for a touch more rim radiance.
- **Heading and menu-label blooms** were tried (resting-state text-shadow approximation) and **rejected** — they made the type read soft. Crisp text wins on OLED. Hover-state glow lands later as a transient affordance, not a resting one.

---

## Steps 7 + 9 + 10 + 13 implementation notes

Shipped together because the glass-panel primitive (step 7) blocked the visible chrome on the Instances detail panel and the Settings screen, so it landed alongside the screens that demanded it. Steps 9 (breathing text) and 10 (vbtn) had already shipped earlier; they are documented here for completeness because the per-step detail was missing.

### Glass panel (`ewo-render::widgets::glass_panel`)

- API: `draw_glass_panel(canvas, bounds, breathing, time, settings, |canvas| { … })`. The closure draws inner content with the panel rrect already clipped — caller works in card-local coords, anywhere inside `bounds`.
- Layer order: refract → tint → inner content → 4 rims → 1px hairline. Matches CSS z-stack (refract z=0, tint z=1, rims z=3, inner z=4, hairline drawn last for a crisp corner).
- **Refract (backdrop blur)**: `SaveLayerRec::default().bounds(&panel).backdrop(&blur)` captures the current canvas through a 16-sigma blur (CSS `blur(32px)` ÷ 2 industry convention), then a `rgba(28,10,20,0.52)` fill is composited on top inside the layer. Saturate 1.45 omitted — the dark fill dominates and the saturation lift is too subtle to justify a second filter pass.
- **Tint**: 135° linear `rose 0.18 → berry 0.12 → lav 0.16` plus a top-edge radial fade (warm-white → transparent) using a local matrix to stretch the circle into the CSS 120%×80% ellipse. Both layers share a single `breath_alpha` driven by an 8s smoothstepped triangle wave (1.0 ↔ 1.0 - 0.10·breath_amp).
- **Rims**: 4 edges (top/right/bottom/left), each a 1.5px-wide rect with a 6-stop linear gradient panned by `phase * pattern_w` over a 12s cycle. CSS animation-delays modeled as +0/+3/+6/+9 sec phase offsets. `TileMode::Repeat` makes the pattern wrap. Edge gradient palettes per CSS: top {rose, lav, champ, rose}, right {champ, rose, lav, champ}, bottom {lav, champ, rose, lav}, left {rose, champ, lav, rose}.
- **Hairline rim**: 1px stroke at `rgba(229,184,197,0.18)`, inset by 0.5 so it bites both sides of the pixel.

CSS spec calls for a `mount-in` keyframe (900ms silk: opacity 0→1, scale 0.98→1, blur 8px→0) but that's omitted on the static draw — non-negotiable #2/#3 forbids scaling/blurring text-bearing surfaces. When entrance animation lands later it'll target the chrome layers (refract+tint+rims) only, leaving the inner content un-transformed.

### Instances detail panel (`screens::instances::draw_detail`)

- Right column wrapped in a glass panel (`breathing: true`).
- Panel bounds: `(LIST_WIDTH + DETAIL_PAD, HEADER_BOTTOM + 28)` to `(card_w - DETAIL_PAD, card_h - 28)`. Inner content offset by 40/36 padding.
- Launch button is now drawn *inside* the panel's inner closure so its sheen and rim animate beneath the rim cycle. `launch_button_bounds(card_w)` returns card-local coords; `main.rs` hit-testing is unchanged because Skia clipping doesn't affect input.

### Settings screen (`screens::settings`)

- Two-column layout per CSS `.settings-body { grid-template-columns: 240px 1fr; gap: 36 }`. Left: title + 4 tab rows. Right: glass panel containing the active tab's content.
- `SettingsTab` enum (Graphics / Audio / Paths / Advanced) lives in this module; main.rs holds the active state and routes sidebar clicks to switch it.
- `sidebar_tab_bounds(fonts)` returns the per-tab hit-rects so main.rs can route clicks. Cursor swaps to Pointer when hovering them.
- Active-tab visual: 4×18 px gradient mark on the left edge (CSS `linear-gradient(180deg, #E5B8C5, #C9A5D4)` simplified to a solid rose with a soft glow rect underneath), plus pearl text vs mauve for inactive.
- Each tab renders a section head ("Graphics" / "Audio" / "Paths" / "Advanced" + a one-line italic subhead) followed by 3-4 label/hint placeholder rows with "(widget pending)" right-aligned. The actual toggles, sliders, dropdowns, and path fields land with step 12 widgets.

---

## Step 12 implementation notes (vtoggle + vslider)

### vtoggle (`widgets::vtoggle`)

- 44×22 pill, fixed dimensions (`TOGGLE_W`, `TOGGLE_H` constants exposed for layout code).
- State: `{ on, hover, anim }`. `anim` is a 0..1 silk-eased tracker of `on` over the CSS 320ms transition; drives both color crossfades (bg 0.08↔0.22, border 0.18↔0.45) and pearl x-position simultaneously so pearl + colors lock-step.
- Pearl: 16×16 circle with the CSS radial gradient `circle at 30% 30%` (warm-white → rose → mid-mauve). Drop shadow always; outer halo (`0 0 10 rgba(255,246,240,0.7)`) ramps up by `anim`. White inset highlight ring at 0.5px stroke for surface gloss.
- API: `state.handle(mouse, bounds, mouse_just_pressed) -> bool` flips on the press edge inside bounds and returns whether it changed; `state.tick(dt)` advances the anim tracker.

### vslider (`widgets::vslider`)

- 2px-tall pill track between `bounds.left + 6` and `bounds.right - 6` (CSS `.vslider-row` 6px horizontal padding). Track gradient `rose 0.10 → lav 0.15 → rose 0.10`; fill gradient `#D4A8B8 → #E5B8C5 → #C9A5D4`.
- 14×14 handle: rose halo on a 3s firefly pulse (opacity 0.65↔1, scale 1↔1.15) + 8×8 pearl core (`#FFF0F4 → #E5B8C5 → #B47491` radial) + outer rose box-shadow approximation + 0.5px white highlight ring.
- Drag model: caller calls `drive(mouse, bounds, mouse_down)` *every* CursorMoved and MouseInput. Internal `dragging` flag flips on the rising edge inside bounds and clears on the falling edge; while dragging, value tracks `mouse.x` even outside bounds. Returns `true` on any value change. Optional `with_step(s)` snaps to multiples (used for Max framerate's 10-fps step).
- Continuous-fraction math: `((x - track_left) / track_w).clamp(0, 1)` then `min + (max - min) * frac` then optional snap.
- Trail particles + floating value pill from CSS deferred to step 16 polish — value display lives next to the slider (right-aligned tracked label) for now.

### Settings panel widget plumbing

- New `screens::settings::Slot` enum identifies wired-up rows (Vsync / MaxFps / Master / Music / Effects / AmbientHum / AutoBackup / Telemetry). `widget_bounds(tab, fonts, card_w, card_h) -> Vec<(Slot, Rect)>` returns the actual hit-rect of each visible widget — toggle pill rect for toggles, full slider track rect for sliders.
- New `screens::settings::Prefs` struct holds all wired-up widget states; `App::prefs` owns it. `Prefs::tick(dt)` advances toggle anims; sliders are stateless re: time so they don't need ticking.
- Input routing: `main.rs::drive_settings_sliders` runs every CursorMoved + MouseInput so sliders get smooth drag updates. Toggles flip in `MouseInput { Pressed }` via slot-matched dispatch. Cursor swaps to `Pointer` when hovering any widget.
- Rows whose widget primitive doesn't exist yet (Window mode, Theme, path fields, Log level, Reset preferences) keep the "(widget pending)" placeholder, so unimplemented controls remain visibly distinct.

---

### vdrop (`widgets::vdrop`)

- Two pieces with separate render calls: `draw_vdrop_head` (drawn inline inside the screen layout, alongside the row's label) and `draw_vdrop_menu` (drawn after the parent glass panel returns so the menu can portal-spill outside the panel's clip).
- Head reuses `vbtn::draw_tint_for_dropdown` / `draw_rim_for_dropdown` / `draw_sheen_for_dropdown` (re-exported as `pub(crate)` so vdrop doesn't have to duplicate the velvet chrome). Adds: value text on the left + an SVG-style chevron caret on the right that rotates 45° → -135° tracking the open animation.
- Menu chrome (`.vdrop-panel-solid`): vertical wine gradient body + warm radial fade `::before` + 3-stop drop shadow stack (24-sigma + 8-sigma + 16-sigma rose bloom) + inset hairline + 1px top inner highlight. Rows render with a 4×4 pearl "selected" dot, hover-row highlight, and a faint hairline divider above each non-first row.
- Portal-in animation: 220ms silk-eased opacity ramp + scale 0.98→1 + translateY(-4 → 0). Flip-up variant inverts the translate origin (CSS uses `transform-origin: bottom center` — Skia has no transform-origin so we translate around the appropriate anchor).
- Flip-up detection: `menu_layout(head, rows, card_h)` returns `(menu_rect, flip_up: bool)`. Flips when the down-position would extend past `card_h - 12`.
- Animation uses CSS row-stagger (`animation-delay: calc(var(--row-i) * 50ms)`) deferred — for now the whole menu fades as a single layer. Per-row stagger is a step-16 polish item.

### Settings dropdown wiring

- Three new `Slot` variants — `WindowMode`, `Theme`, `LogLevel` — wired into the Graphics + Advanced tabs.
- New `Prefs` fields (`window_mode`, `theme`, `log_level`) hold each `VdropState`. `Prefs::open_dropdown()` finds whichever (if any) is currently open, used by the render path to know what menu to portal-draw and by `main.rs` to route menu clicks.
- `screens::settings::dropdown_options(slot)` exposes the canonical option lists (Windowed/Borderless/Fullscreen, Velvet/Pearl/Obsidian/Champagne, Trace/Debug/Info/Warn/Error). Defaults match the React prototype's `useState` initial values.
- `screens::settings::dropdown_head_for_slot` finds a slot's head bounds across any tab — used by the portal-draw logic in `draw_panel` to compute the menu's anchor.

### Settings input routing

The press-handling branch on the Settings screen now lives in [main.rs::handle_settings_press](crates/ewo-launcher/src/main.rs). Resolution order:

1. **Open menu first** — if any dropdown is open, check if the press is inside its menu rect. If so, commit the row and return `handled=true`.
2. **Click-outside dismiss** — if the press is outside both the open dropdown's head AND its menu, close the menu but let the press fall through to the rest of the dispatch (so the user can click another widget while dismissing).
3. **Normal dispatch** — iterate the active tab's widgets. Dropdown heads toggle (with `close_other_dropdowns` enforcing single-open-at-a-time); toggles flip; slider clicks were already consumed by `drive_settings_sliders`.

`drive_settings_sliders` runs on every CursorMoved + MouseInput; in addition to driving slider drags it also calls `state.update_menu_hover(...)` for any open dropdown so the row under the cursor lights up.

Tab switch (`Step 2.5`) and screen switch (`Step 1`) call `prefs.close_dropdowns()` so an orphan menu can't survive a screen change.

---

### Instances detail wiring

- New `screens::instances::Slot` (Ram / RenderDist / JavaRuntime) + `InstancePrefs` struct (parallels `screens::settings::Prefs`). Defaults match the React prototype: 8 GB RAM, 16 chunks, "Adoptium 21.0.4 (bundled)".
- Three `inst-config-row`s render inside the existing glass panel beneath the head divider. Each row is `label_top → italic Newsreader 14 label + right-aligned mono value pill (e.g. "8 GB", "16 CHUNKS")`, then the widget below at `+10px` per CSS `.inst-config-row { gap: 10 }`.
- Java-runtime row uses a left-aligned dropdown head (320px wide rather than the 220px Settings dropdowns get, since runtime strings like "Adoptium 21.0.4 (bundled)" need horizontal room). The portaled menu draws after the glass panel returns so it can spill outside the panel's clip — same technique as Settings.
- Hit-testing: `widget_bounds(card_w, card_h, fonts)` returns the per-widget rects; `dropdown_head_for_slot` provides the menu anchor. Layout is computed identically by both the renderer and the hit-tester via a shared `head_divider_y` helper.

### Instances input routing

[main.rs](crates/ewo-launcher/src/main.rs) gains a parallel pair to the Settings handlers:

- `drive_instance_widgets` (CursorMoved + MouseInput) drives slider drags + dropdown-menu hover.
- `handle_instances_press` (MouseInput pressed) handles the same three-stage resolution: open menu first → click-outside dismiss → normal widget dispatch (dropdown head toggle).

Tab-bar nav and main-menu-driven screen switches both call `instance_prefs.close_dropdowns()` so the Java-runtime menu can't survive a screen change.

---

### vghost_btn (`widgets::vghost_btn`)

- A "ghost" button: transparent fill + 1px border that brightens on hover, plus a soft outer glow. Two variants behind a `GhostKind` enum:
  - `Pearl` — rose hairline, italic 13, 8px corner radius. Used by the Paths tab's Browse buttons.
  - `Danger` — ember hairline, italic 14, 999px pill radius. Used for "Reset preferences" in Advanced.
- Doesn't share enough chrome with vbtn to justify reuse (no tint, no rim cycle, no sheen). Hover anim drives both border-color and label-color crossfades simultaneously.

### vpathfield (`widgets::vpathfield`)

- Renders the prototype's `.settings-pathfield` row: a faux text input (`.settings-pathinput` chrome — 8px rrect, faint rose fill, 1px border that lifts on hover, 3px focus glow on hover-anim) plus a `vghost_btn(Pearl)` "Browse…" button on the right with a 12px gap.
- **Read-only** — text input is post-v1. The path renders as a JetBrains Mono 12 label; long paths are ellipsized from the *left* (e.g. `…/path/to/file.json`) so the meaningful tail stays readable. A real text-edit affordance (caret, IME, key handling) lands when v1's "no real launching, no real text input" scope expires.
- `split_bounds(row) -> (input_rect, browse_rect)` keeps the layout split in one place; `browse_bounds(row)` is the public accessor for `main.rs` hit-testing.

### Settings — Paths + Reset wiring

- Three new `Slot` variants: `GameDir`, `Downloads`, `ResetPrefs`.
- `Prefs` gains `game_dir`, `downloads`, `reset_prefs` fields with sensible defaults (XDG-ish path strings; reset button starts un-hovered).
- **Stacked rows**: CSS uses `.settings-row.settings-row-stack { grid-template-columns: 1fr; gap: 12px }` for path fields — label on its own row above the field. New `RowDef::stacked` constructor + `stack: bool` field on `RowDef`. `row_extents` reserves an extra `STACK_LABEL_TO_FIELD_GAP + ROW_HEIGHT` of vertical space; `control_rect` returns a full-width-below-label rect for stacked rows instead of the right-column rect.
- Static row arrays (`GRAPHICS_ROWS`, `AUDIO_ROWS`, `PATHS_ROWS`, `ADVANCED_ROWS`) replaced the inline `&[…]` literals so `RowDef::row(…)`/`RowDef::stacked(…)` const-fn constructors can return `&'static [RowDef]` cleanly. (Inline `&[fn_call(…), …]` doesn't promote to static lifetime; module-level `const` does.)
- `path_browse_bounds(slot, fonts, w, h)` exposes the Browse button's hit-rect for `main.rs` so the click-handling stays narrow (input rect is inert; only the Browse press triggers any state change).
- `drive_settings_sliders` now also calls `prefs.game_dir.drive_hover` / `prefs.downloads.drive_hover` / `prefs.reset_prefs.handle(…, false)` on every cursor move so the focus-style border lift + danger-button glow track the cursor.

---

### pbar (`widgets::pbar`)

- 2px-tall pill track + a gradient fill that grows with `fraction`. CSS spec has Normal (pearl→rose→pearl shimmer fill) + Complete (rose→pearl→champagne with extra outer glow) + three error variants. v1 ships Normal + Complete; error variants and the expanding ring (`.pbar-ring`, 1.4s 4px→320px halo) are step-16 polish.
- Three sub-layers: `draw_track` (linear gradient + 0.5px inset rim), `draw_fill` (5-stop or 4-stop linear gradient depending on state, clipped to the value rect), `draw_flow` (40%-wide pearl shimmer translating -40% → 140% over 3s linear, screen-blended), `draw_bloom` (radial pearl glow at the leading edge, sigma-3 blur, screen-blended). Complete state adds `draw_complete_glow` — a sigma-6 stroke around the fill rrect at rose 0.45 alpha.

### Launching screen (`screens::launching`)

- `LaunchingState` owns `start_time: Option<f32>`, `instance_name`, `instance_meta`, `shown_logs: usize`, `done: bool`. Wall-clock-based: `enter(time, name, meta)` snapshots the start, `tick(time)` drains the log script + flips `done` once `DURATION_SECONDS` (8.0) has elapsed, `should_handoff(time)` returns true after a 1.5s hold so main.rs can auto-return to Instances.
- Script timeline matches the React prototype verbatim — `LOG_SCRIPT` (25 entries from `t=0` to `t=2400`) + `PROGRESS_KEYFRAMES` (8 stops). Both use opaque "script units"; the runtime maps `elapsed / DURATION_SECONDS * SCRIPT_DURATION` per CSS-author note that says "8 seconds — long enough to read the logs streaming."
- Rendering: head row (`LAUNCHING` eyebrow + Fraunces 56 instance name + JetBrains Mono 11 meta on the left, Fraunces 80 percent display + smaller `%` in mauve on the right), divider, italic stage label + pbar in the progress region, glass-panel log below with `LOG · STDOUT` head + entry count + scrolling lines.
- Log lines use the CSS 56/88/1fr column grid: time (mauve-deep right-aligned, `0.00s` formatted), source (per-level color: rose for info-launcher/mods, lavender for ok, champagne for warn), message (mid-pearl for info, champagne for warn, italic Newsreader 12 mauve-deep for `dim` entries — matches CSS `.log-dim` font-family swap). Most-recent line dims slightly to suggest the streaming-in animation (per-entry surface time isn't tracked yet — proper fade-in is a polish item).
- Footer crossfades from "Cancel" (newsreader 14 mauve) to "the curtain rises." (Fraunces 22 pearl) on completion.

### Launch flow + nav

- Clicking the Launch button on the Instances detail logs `vbtn: Launch clicked → entering Launching screen`, calls `launching.enter(time, …)`, navigates to `Screen::Launching`, and triggers the existing `backdrop.celebrate(true)` for the petals burst.
- Clicking the LAUNCHING tab directly while `start_time.is_none()` auto-starts a fresh launch — keeps the screen from ever rendering empty.
- Each frame, when on Launching, `App` calls `launching.tick(time)` and checks `should_handoff(time)` → exits the launching state and auto-routes back to `Screen::Instances`.

---

### vstatus (`widgets::vstatus`)

- Single-line text with a 500ms silk-eased crossfade. State holds the `current` string + an optional `fading_out` snapshot + a 0..1 anim. `set(text)` snapshots the previous and resets anim; `tick(dt)` advances + clears `fading_out` on completion.
- Rendering wraps each half in `Canvas::save_layer` with a layer paint that combines `set_alpha_f` (opacity) and `set_image_filter(image_filters::blur(...))` (blur sigma 3 → 0). The layer is then translated for the slide-in/out: incoming `translateY(8 → 0)`, outgoing `translateY(0 → -10)` per CSS `@keyframes status-in` / `status-out`. Note: blurring text *during a transient transition* is allowed; CLAUDE.md non-negotiable #3 forbids blur on entrance to text-bearing surfaces (modals/panels), not on transient text crossfades.
- Wired into `screens::launching::LaunchingState::stage`. The launching tick calls `stage.set(progress_at(scaled).1)` each frame; identical strings no-op.

### Mods list (`screens::instances::draw_mod_section`)

- Renders below the inst-config bottom hairline (with the CSS 28px margin-bottom) when the selected instance has any mods. Active instance ships with 7 mods (Sodium / Iris / Distant Horizons / Continuity / Lithium / Mod Menu / Carpet — last is off by default).
- Layout matches the CSS 4-column grid (28px / 1fr / auto / auto, 14px gap): toggle circle / mod name / category eyebrow / version. Categories are uppercase Mono 9 with 0.18em tracking; off rows render at 0.4 alpha so they read as inactive without disappearing.
- Toggle circle is a 22px hairline-rose ring; when on, an 8×8 pearl-gradient (white→rose→lavender) sits in the center with a 4-sigma rose halo blurred underneath.
- New `Slot::ModToggle(usize)` carries the row index. Hit-testing returns one entry per mod; clicking flips `InstancePrefs::mods_on[i]` and the head's "X of Y enabled" count updates immediately.

---

### New-instance modal (`screens::new_instance_modal`)

- **Two-layer composition.** Layer 1 = `.modal-shroud` — full card-content rect with a radial dim gradient (rose-tinted black 0.55 → 0.85) + 4px backdrop blur. Layer 2 = `.modal-card` — 560px-wide rounded card centered, with three sub-passes: drop-shadow stack (40-sigma + 14-sigma + rose-bloom), backdrop blur 40 + dark-wine fill, 135° tint + warm-white top radial fade, hairline rim, then inner content clipped to the rrect.
- **Entrance animation.** 240ms silk crossfade — opacity 0→1, translateY(10→0), scale(0.97→1). The shroud fades in slightly faster (linearly ramped to peak by `anim ≥ 0.67`). No blur during entrance per CLAUDE.md non-negotiable #3 — text-bearing surfaces never re-rasterize.
- **Inner layout.** Head: `NEW INSTANCE` mono eyebrow + Fraunces 36 "Begin a new world" + italic Newsreader 14 subhead. Body: name field (faux input rrect with placeholder text — actual editing is post-v1), 2-column Version + Loader dropdowns, and a RAM slider row with a Fraunces 22 value + Mono 12 "GB" suffix and a dynamic hint ("small and quick" / "comfortable" / "roomy" / "palatial"). Footer: ghost Cancel + primary "Create world" vbtn.
- **Hit-testing.** `widget_bounds(card_w, card_h)` returns slot rects for Version / Loader / Ram / Cancel / Create. `card_rect()` and `shroud_consumes(mouse, w, h)` let `main.rs` distinguish "click on card → consume + dispatch" from "click on shroud → close".
- **Open dropdown menus** are portal-drawn after the card so they sit on top of the form (same `menu_layout` + flip-up logic as the Settings dropdowns).

### Modal input routing

- New "+" button in the Instances list head (Worlds title row); clicking it logs `instances: + clicked → opening new-instance modal` and calls `modal.open()`.
- When `modal.open` is true, **all** mouse input is absorbed by the modal: a Step 0 in `MouseInput` runs `handle_modal_press` first, and CursorMoved drives the modal widgets exclusively. Underlying-screen widgets don't receive any input until the modal closes.
- Close paths: Cancel button click, Create button click (logs the form values then closes — no actual instance creation in v1), Esc key, click on shroud (anywhere outside the card), tab-bar nav.

---

### Dev overlay (`screens::dev_overlay`)

- Visible only when the launcher is started with `--dev`. `App` constructs `Some(DevOverlayState::default())` from the current `Settings`, otherwise `None`. Render path conditionally invokes `screens::draw_dev_overlay` after the modal so the panel sits above all other UI.
- 280px-wide dark-glass panel anchored to the top-right of the card (16/60 inset). Chrome: drop shadow + 20-sigma backdrop blur + wine fill (`rgba(10,0,6,0.72)`) + warm radial top-fade + hairline rose rim. Inside: `TWEAKS · DEV` mono eyebrow, hairline divider, then 5 rows (italic Newsreader 13 label + right-aligned mono 11 value pill, then a full-width vslider beneath each), then a "Reset to defaults" pearl ghost button.
- Per-token slider configs match the Settings ranges: motion_speed [0.1, 3.0] step 0.1, breath_amp [0, 2] step 0.1, density [0, 2] step 0.1, warmth [0, 1] step 0.05, accent_hue_shift [-180, 180] step 5.
- **Live wiring**: each frame `overlay.apply_to_settings(&mut self.settings)` runs before rendering. Returns `true` when `density` changed, in which case `App` calls `Backdrop::resize(...)` to re-spawn the particle pools at the new count (slight visual snap is acceptable for dev tuning). Everything else takes effect immediately because `Settings` is read every frame.
- **Input absorbtion**: when the cursor is over the panel rect, all input routes to the overlay first and is consumed (Step -1 in `MouseInput`). The dev overlay cursor doesn't interfere with the modal — sliders/reset work even with the new-instance modal open.

The state-picker / layout-picker / error-picker the prototype's `dat.gui`-style overlay also exposes are deferred. They were prototype-debug-specific (forcing the launcher to render mid-progress, swapping launcher-stage layouts, simulating launch errors) and the tab bar + Launch flow already reach those states by clicking; the parity-tuning value lives entirely in the tweaks-panel.

---

### Frame-stat instrumentation (step 16)

- `Clock` gains a 60-entry rolling buffer of frame deltas. `tick()` writes the current dt; `avg_dt`, `avg_fps`, and `worst_dt` read it. The buffer ignores deltas outside `(0, 1)` to skip the first-tick artifact and any pause/resume hitches that would skew the rolling average.
- `App::draw_frame` packs the three numbers into a `screens::FrameStats` struct that flows to `screens::draw_dev_overlay`. The overlay header right-aligns `"60 FPS · 16.6 MS"` colored by frame budget — lavender when ≤16.7ms, champagne 16.7-33.4ms, ember above. A second small line below shows `worst N.N ms` over the rolling window, dim-mauve.

### pbar complete-ring (step 16)

- `widgets::pbar::draw_pbar` gains a `complete_age_seconds: Option<f32>` parameter. When provided and in `[0, 1.4]`, it draws the CSS `.pbar-ring` expanding halo: a silk-eased ring at the leading edge of the fill, growing 4px → 320px diameter, fading 0.9 → 0 alpha, with stroke width 2px → 1px.
- `LaunchingState` now records `done_at: Option<f32>` (the wall-clock second the launch crossed `DURATION_SECONDS`); the launching render path computes `complete_age = time - done_at` and threads it into `draw_pbar`. Result: when the synthetic launch hits 100%, a single rose ring expands outward from the right end of the bar, fading as it grows — the prototype's celebratory "ping" before handoff.

---

### Vsync toggle (step 16)

- `GlBackend::set_vsync(bool)` swaps the GL surface's `SwapInterval` between `Wait(1)` (cap to monitor refresh, no tearing) and `DontWait` (uncapped). Silently no-ops when the requested interval is unsupported by the platform.
- Dev overlay's bottom row splits 50/50 between a `VSync · on/off` ghost button (left) and a compact `Reset` button (right). Clicking the vsync toggle flips `DevOverlayState::vsync` and `main.rs` propagates the new state to `GlBackend::set_vsync`.
- Combined with the existing FPS HUD this validates the 500fps OLED target — turn vsync off, watch the worst-frame readout for hitches, and tune blur radii / particle counts using the live token sliders if they cluster above 2ms (the budget at 500fps).

---

## v2 status — A–E all shipped

**v2 turns the Launch button into an actually-functional Minecraft launcher.** Phases A–C are shipped and verified end-to-end (Minecraft 26.1 launches, plays, and exits cleanly from a cold start). Phase D blocks on the user's custom Fabric fork existing as a separate project. Phase E is the original v2 ambition (in-game GUI) and lives further out.

### Phase A — Microsoft authentication ✅ shipped + approved

Code is complete in [`crates/ewo-launcher/src/auth/`](crates/ewo-launcher/src/auth/):

- `chain.rs` — the four-step token exchange (Microsoft OAuth + PKCE → Xbox Live → XSTS → Minecraft Services) + profile fetch
- `loopback.rs` — `tiny_http` listener bound to a random `127.0.0.1:PORT` that catches the OAuth redirect at the bare `http://localhost` registered URI
- `pkce.rs` — code verifier/challenge generation
- `persistence.rs` — refresh-token persistence at `<config>/EwoClient/auth.toml` (plaintext for now; DPAPI/keychain encryption is a TODO before wide distribution)
- `service.rs` — background-thread auth runner + mpsc UI events

UI lives on the **Account** tab in Settings (first tab; uses a custom layout, not the row-grid system). Sign-in / sign-out / try-again button changes label by `AccountView` state.

**The Entra app `f901fc74-7e36-439d-80a8-c2e548f47fdc` is on Mojang's allowlist** (Launcher Program approval came through ~1 week after submission). Sign-in completes the full chain end-to-end: Microsoft OAuth → Xbox Live → XSTS → `login_with_xbox` → profile fetch → token persisted to `auth.toml`. Verified live: signed in as **Vwyla**, joined Hypixel, sent chat, played a Bedwars game end-to-end.

`App::try_real_launch` reads `AuthState::SignedIn(account)` and slots the live `MinecraftAccount.minecraft_token` into the `LaunchProfile` — online multiplayer works without further code changes. Falls back to `LaunchProfile::offline(name)` (synthetic UUID + placeholder `"0"` token) when no signed-in account is available; offline mode is still useful for LAN or singleplayer-only sessions.

### Phase B — Version manifest + downloads ✅ shipped

- [`crates/ewo-launcher/src/versions/`](crates/ewo-launcher/src/versions/) — master `version_manifest_v2.json` fetch + 6h disk cache; per-version manifest fetch + sha1 verify + permanent cache (manifests are immutable per Mojang)
  - `manifest.rs::is_supported` is the **curated allowlist**: 1.21.x + 26.x release line + 1.8.9 + `26.2-snapshot-5`. Add other snapshots via `SNAPSHOT_ALLOWLIST`. The launcher intentionally doesn't surface every Mojang version.
- [`crates/ewo-launcher/src/downloads/`](crates/ewo-launcher/src/downloads/) — orchestrates the per-stage download (`PerVersion → Client → Libraries → AssetIndex → Assets`), sha1-verifies every file, drops into Mojang's official disk layout under `<config>/EwoClient/shared/{versions,libraries,assets}/`. One worker thread per active job; mpsc events to the UI.

UI surfaces:
- New-instance modal Version dropdown is live (filtered from `version_manifest_v2.json`)
- Instance row shows real percentage badge: `DOWNLOADING · 47%`
- On completion the instance flips from `Pending` → `Ready` and persists

### Phase C — JVM spawn + game launch ✅ shipped

[`crates/ewo-launcher/src/launch/`](crates/ewo-launcher/src/launch/):

- `plan.rs` — `LaunchPlan` builder. Substitutes every documented Mojang token (`${auth_player_name}`, `${classpath}`, `${natives_directory}`, etc.) into both modern `arguments` blocks and legacy `minecraftArguments` strings. `LaunchProfile::offline(name)` synthesizes an offline-mode profile.
- `natives.rs` — extracts native-classifier JAR contents into per-instance `natives/` dir (deletes + recreates each launch). META-INF skipping, basic path-traversal guard.
- `spawn.rs` — `std::process::Command` child + two reader threads piping stdout/stderr line-by-line back to the UI through mpsc.
- `jre.rs` — JRE detector. Scans Oracle javapath, Adoptium, Microsoft, Zulu, Liberica, JAVA_HOME, PATH, plus the bundled-runtime dir (see Runtime below). Probes each via `java -version`. Cached + manually invalidatable. `pick_for_major(required)` returns exact match else lowest installed major ≥ requirement.

[`crates/ewo-launcher/src/runtime/`](crates/ewo-launcher/src/runtime/):

- **Bundled-JRE auto-fetch.** When `pick_for_major` returns `None`, `try_real_launch` triggers an Adoptium download for the missing major. Archive lands at `<config>/EwoClient/runtime/<major>/`; extracted into `<runtime_dir>/jre/`. JRE detector picks it up automatically (cache invalidated by `jre::invalidate_cache` after extraction completes). Currently Windows-only — `.zip` extraction; macOS/Linux `.tar.gz` is the next polish item.

**Real launches** are spawned with the right JRE, full classpath, extracted natives, working dir = per-instance dir. JVM stdout/stderr stream into the launching screen's log panel (replaces the synthetic `LOG_SCRIPT`). On clean exit the screen handoffs back to Instances; on non-zero exit the pbar flips to `ErrorRose`, the screen sticks, and a Retry / Back button pair appears in the footer.

**Per-launch logs** are dumped to `<config>/EwoClient/instances/<name>/logs/launch_<unix_ts>.log` on JVM exit, tagged `[OUT]` / `[ERR]` per source.

**Verified end-to-end on a fresh box:** create instance → Phase B downloads (~3 min for 26.1 full asset set) → click Launch → JRE auto-fetch via Adoptium (~2s for Java 25) → JVM spawn → Minecraft renders → clean exit → handoff. Logs in `tasks/b8ycimnb5.output` (smoke-test session 2026-05-03).

### Phase D — Custom Fabric-fork loader (EwoLoader) ✅ shipped + verified live

**EwoLoader** is a friendly fork of fabric-loader living in a sibling repo at `C:\Users\valtteri\Desktop\EwoLoaderV1` (`lewlone/ewo-loader`, private). Friendly-fork meaning package names stay `net.fabricmc.loader.*` for binary compat with the existing Fabric mod ecosystem — only artifact identity (`dev.lewlone:ewo-loader`) + Maven group + the `STRIP_PLAN.md`-driven leanness differ. Built from upstream `fabric-loader` 0.19.2; eight strip passes shipped (see `STRIP_PLAN.md` in that repo) removing ~17k LoC: ProGuard pipeline, LaunchWrapper/Applet/FML125, legacyJava source set, dev-mode helpers, `mods/` folder discovery, Swing crash GUI, all test infrastructure, V0 metadata schema. Build time: 6s clean on JDK 21.

**Launcher integration is fully wired:**

- [`crates/ewo-launcher/src/loaders/`](crates/ewo-launcher/src/loaders/) — `manifest.rs` (`LoaderManifest` shape), `merge.rs` (`merge(vanilla_pv, loader_manifest) -> PerVersion`), `fetch.rs` (`get_or_fetch` with HTTP + `file://` support).
- `merge::merge` prepends loader libraries ahead of vanilla's on the classpath, overrides `mainClass` when present, concatenates JVM/game args. Critical detail: keeps `id: vanilla.id.clone()` so client.jar path resolution still works (`versions/<vanilla_id>/<vanilla_id>.jar`).
- `try_real_launch` ([main.rs:254](crates/ewo-launcher/src/main.rs#L254)) reads `Instance::loader`, dispatches: `Vanilla` → vanilla PerVersion as-is; `Ewo { manifest_url }` → fetch loader manifest, merge, hand merged PerVersion downstream. Loader-fetch failures fall back to vanilla launch with a warn (non-fatal — won't block a flaky local manifest).
- Loader manifest URL is currently the in-development `file:///C:/Users/valtteri/Desktop/EwoLoaderV1/manifest/0.1.0/26.1.json` (per `DEV_EWO_LOADER_URL` const). Becomes a config knob (or public HTTPS URL) once the loader publishes a meta endpoint.

**The new-instance modal's Loader dropdown** is now `["Vanilla", "Ewo (development)"]` only — the prototype's `["Fabric", "Forge", "NeoForge", "Quilt"]` entries got removed during Phase D wiring since they were never going to be wired (we ship one loader, ours).

### Bundle phase — 16 user-toggleable mods + 5 infrastructure libs ✅ shipped

The bundle ships the curated set originally planned (Sodium, Lithium, Iris) plus 13 more — Simple Voice Chat, Distant Horizons (default-off, heavy), plus the optimization + QoL set the user picked (ImmediatelyFast, FerriteCore, EntityCulling, More Culling, Mod Menu, Reese's Sodium Options, BetterF3, AppleSkin, Zoomify, LambDynamicLights, Continuity), each toggleable per-instance from the Instances UI's mod list. Infrastructure (Fabric API + 4 transitive lib mods: fabric-language-kotlin for Zoomify, YACL for Zoomify, placeholder-api for Mod Menu, Cloth Config for BetterF3 + More Culling) is bundled but hidden from the toggle UI.

**How bundling works:**

1. The launcher reads `manifest/0.1.0/26.1.json` (in the EwoLoader repo, served via `file://` for active dev — see "GitHub Release snapshots" below) which lists every artifact the launch needs in its `libraries[]` array — Mojang's standard library schema (`name`, `downloads.artifact.{path, sha1, size, url}`). Currently 28 entries: 1 EwoLoader fat jar + 5 ASM jars + sponge-mixin + 5 infrastructure mods (Fabric API + fabric-language-kotlin + YACL + placeholder-api + Cloth Config) + 16 user-toggleable mods.
2. Phase D's `merge` prepends every loader library onto vanilla's, so the final `PerVersion` going to Phase B/C contains them all.
3. **Phase B is loader-aware as of `64c6fe1`** ([downloads/job.rs](crates/ewo-launcher/src/downloads/job.rs)). `DownloadService::start(entry, Some(LoaderSpec { id, url }))` fetches the loader manifest in a new `Stage::LoaderManifest` between PerVersion and Client, merges in-memory, and counts + downloads the merged library set through the same progress bar. **`downloads::ensure_libraries`** still runs from `try_real_launch` after the merge as a safety net for the iteration loop where the user edits the loader manifest between instance-setup and launch — Phase B's snapshot then misses the new entries and ensure_libraries picks them up. In the steady state it's a fast no-op.
4. JVM spawns with `mainClass = net.fabricmc.loader.impl.launch.knot.KnotClient` (from the manifest's mainClass override) and `-cp` containing all the loader+mod jars ahead of vanilla's libs + the client.jar.
5. EwoLoader's `ClasspathModCandidateFinder` scans `fabric.mod.json` resources across the classpath → finds every bundled mod that wasn't stripped → registers them with the mod resolver. Then `BundledMods.BUNDLED_MODS` verification fires: every expected modId not already in the user-disabled set must appear in the discovered set, else throw `ModResolutionException`. The expected list lives at `BundledMods.java`; the disabled-mods subtraction reads `fabric.debug.disableModIds` (upstream system property the launcher repurposes for per-instance toggles — see below).

**Per-instance mod toggles** are wired end-to-end as of `bcc3ea6` (launcher) + `99a38df` (loader):

- Launcher's `bundled::CATALOG` ([crates/ewo-launcher/src/bundled.rs](crates/ewo-launcher/src/bundled.rs)) is the source of truth: each row has the display name + category + version + `fabric.mod.json` id + loader-manifest library name + `default_on` + `toggleable`. Infrastructure rows (FAPI, language-kotlin, YACL, placeholder-api) are `toggleable: false` so they don't appear in the UI.
- New Ewo instances get their `Instance.mods` seeded from `bundled::seed_instance_mods()`. Existing instances are migrated on launcher startup via `bundled::sync_mods_with_catalog` (called from `persistence::load_instances`), which adds missing catalog entries with their default-on and preserves user-flipped state.
- At launch time, `try_real_launch` runs `bundled::disabled_mod_ids(&inst.mods)` → strips the matching libraries from the merged `PerVersion.libraries` (so the classpath excludes them) → appends `-Dfabric.debug.disableModIds=<csv>` to `plan.jvm_args`.
- The loader's `ModDiscoverer.findDisabledModIds` is upstream's already-wired filter at discovery time. `FabricLoaderImpl.setup()` was extended (`parseDisabledModIds` + verification subtraction) so `BundledMods.BUNDLED_MODS` checks don't fire on intentionally-absent mods.
- Disabled mods stay on disk after `ensure_libraries` — re-enabling a mod in the UI doesn't trigger a re-download.
- If the user disables a mod whose required deps are still enabled, the resolver fails loud at launch with the usual upstream "X requires Y" error. We don't pre-detect cascades.

**Bundled-mod sourcing:**
- Mod jars come from Modrinth's Maven (`https://api.modrinth.com/maven/...` → 307 redirects to `cdn.modrinth.com`). Coordinate form: `maven.modrinth:<slug>:<version_number>`. Modrinth uses the human-readable version string as the path segment.
- ASM + sponge-mixin come from `maven.fabricmc.net`. EwoLoader's Gradle `installer` configuration declares them but doesn't bundle them — they're expected to come from the installer's manifest, which in our case IS our loader manifest.
- The EwoLoader fat jar itself comes from `file:///C:/Users/valtteri/Desktop/EwoLoaderV1/build/libs/ewo-loader-0.19.2-fat.jar` for now. **Hosting it publicly is a follow-up** (see Known gaps).

**Live `BundledMods` declaration:** [`src/main/java/net/fabricmc/loader/impl/discovery/BundledMods.java`](https://github.com/lewlone/ewo-loader/blob/main/src/main/java/net/fabricmc/loader/impl/discovery/BundledMods.java) in the EwoLoader repo. Adding a new bundled mod = three places: (a) the loader manifest's `libraries[]`, (b) `BundledMods.BUNDLED_MODS`, (c) one `./gradlew fatJar` to bake the new BUNDLED_MODS into the fat jar. Manifest sha1 doesn't need updating for the EwoLoader fat jar (`file://` skips sha1 verification).

**Iteration loop (concrete commands):**
```
1. cd C:\Users\valtteri\Desktop\EwoLoaderV1
2. # edit src/main/java/.../BundledMods.java + manifest/0.1.0/26.1.json
3. GITHUB_ACTIONS=true ./gradlew fatJar       # 10s; clean version, no +local suffix
4. # click Launch in EwoClient
```
The launcher re-reads the loader manifest on every launch (no TTL cache in `loaders::fetch`). On a fresh instance Phase B already downloaded the merged library set through its progress bar; on the iteration loop where the user edits the manifest between setup and launch, the safety-net `ensure_libraries` hot-downloads new entries to `<config>/EwoClient/shared/libraries/...` before JVM spawn.

**Adding a new bundled mod:** three-place change (the BundledMods verification fails loud if any two drift):
1. `crates/ewo-launcher/src/bundled.rs::CATALOG` — adds the UI row + library-name → mod-id mapping the launcher uses for classpath stripping.
2. `EwoLoaderV1/manifest/0.1.0/<version>.json::libraries[]` — adds the download
   artifact entry. **There are TWO manifests now**, `26.1.json` and `26.2.json`,
   and the launcher picks one per Minecraft version line, so a mod added to only
   one of them is missing on the other with no error until `BundledMods`
   verification fires at launch.
3. `EwoLoaderV1/src/main/java/.../BundledMods.java::BUNDLED_MODS` — adds the post-discovery verification entry.

**GitHub Release snapshots (off-box backup of the fat jar):** versioned snapshots of the fat jar live on `lewlone/ewo-loader` GitHub Releases (private). `v0.19.2-bundle.1` was the first one. The loader manifest's fat-jar URL stays `file://` for active dev (per the iteration loop above — fast rebuild + no upload step), so the launcher reads the local jar on every launch. The Release exists as a versioned backup + a stepping-stone if the loader ever needs to ship to a second machine. To cut a new snapshot:
```
./gradlew fatJar
gh release upload v0.19.2-bundle.<N> build/libs/ewo-loader-0.19.2-fat.jar --repo lewlone/ewo-loader --clobber
```
The launcher already has the auth wiring for private-repo asset URLs: when an HTTP URL matches `api.github.com/repos/<owner>/<repo>/releases/assets/<id>`, [`downloads::job::github_auth_headers_for`](crates/ewo-launcher/src/downloads/job.rs) attaches `Authorization: Bearer <EWO_LOADER_TOKEN>` + `Accept: application/octet-stream`. The token comes from the env var (fine-grained PAT, Contents: read on `lewlone/ewo-loader`). When the env var is unset, no headers are sent and the request goes through unauthenticated — which is correct behavior for the file:// path (the helper is never called for file URLs at all). To flip to the hosted jar, just point the manifest's `dev.lewlone:ewo-loader:0.19.2` library URL at the GitHub asset API URL and re-launch.

### Phase E — In-game HUD ✅ E0–E7 shipped — Phase E complete (full HUD + editor + dashboard + cached glass refract)

**The spike is done (2026-05-20): `ewo-render`'s Skia pipeline paints over a running Minecraft, verified live on the 26.1 title screen** — a rotating glass panel composites over the game, stable (no flicker, no driver crash). Phase E's core question — can the launcher's Skia stack render over Minecraft at all — is answered: yes.

Three pieces, all in the EwoClientV3 repo:

- [`crates/ewo-jni/`](crates/ewo-jni/) — a `cdylib` loaded into the Minecraft JVM. Creates a **dedicated GL context** on Minecraft's window, builds a Skia `DirectContext` against it, and paints `ewo-render`'s glass-panel widget onto the window framebuffer.
- [`ingame-mod/`](ingame-mod/) — Fabric mod `ewo-hud`. Loads the cdylib; `EwoHudMixin` injects `RenderSystem.flipFrame` HEAD and calls into Rust once per frame. Plain `javac` + `jar` build ([`ingame-mod/build.ps1`](ingame-mod/build.ps1)) — no Loom, no Gradle.
- `EwoLoaderV1/manifest/0.1.0/26.1.json` — a `file://` `libraries[]` entry (`dev.lewlone:ewo-hud:0.1.0`) so EwoLoader puts the mod on the classpath.

Decisions + findings from the spike (load-bearing for the real HUD):

1. **Dedicated GL context, never shared.** Skia and Minecraft both drive the OpenGL state machine and corrupt each other if they share a context — observed live as flickering UI, then a fatal `EXCEPTION_ACCESS_VIOLATION` in `nvoglv64.dll`. The fix: a second `wglCreateContext` on MC's window; each frame `wglMakeCurrent`s to it, draws, and hands the thread's context back to Minecraft untouched. Two GL state machines, one shared window framebuffer. This is the HUD's permanent isolation model — do not regress to a shared context.
2. **Frame hook is `RenderSystem.flipFrame` HEAD** — a universal end-of-frame point (title screen, menus, in-game). Fabric API's `ScreenEvents.afterExtract` fires mid-pipeline in MC 26.x's deferred GUI rendering, too early to composite over.
3. **The toolchain runs mods in Minecraft's Mojmap namespace — no intermediary, no Loom remapping.** Fabric mods here build with plain `javac` against the on-disk jars. `shared/versions/26.1/26.1.jar` ships **Mojmap-named** (Mojang distributes 26.x deobfuscated — confirmed E2: `net.minecraft.client.Minecraft` + ~10.7k readable classes), and EwoLoader logs `Mappings not present!` — it does no remapping, so the on-disk jar *is* the runtime namespace. The jar is Java-25 bytecode, so building against it needs a **JDK 25** (installed E2 at `%APPDATA%/EwoClient/jdks/temurin-25/`). The spike's string-target mixin + `TracyFrameCapture` compile-only stub were a JDK-21 workaround; E2 removed them for plain class-literal mixins.
4. **Draw-direct in the spike; two-clock as of E1.** The spike painted `fbo 0` directly every frame. **E1 (done 2026-05-20)** replaced that with the decoupled model: `paint` renders the HUD to an offscreen GPU surface (rate-gated by `HudPaintRate`), `composite` blits it onto `fbo 0` every frame so it never tears. Verified live with the rate forced to 30 — chunky panel, game still 400–500 fps. The known tradeoff — offscreen painting drops the glass panels' live-game backdrop blur — was resolved in E7 with a cached frosted backdrop for the overlay views (see below). Details in `PHASE_E_PLAN.md`.

E2 shipped the first real widget (a live FPS readout). E3 finished the read-only widget set: **FPS, Coords, Ping, Keystrokes, Armor, PotionHUD, TargetHUD** — all Velvet re-skins of `hud.jsx` elements, in `crates/ewo-jni/src/hud.rs`. The data pipeline is a shared direct `ByteBuffer`: the mod allocates it once (`EwoHudData`), fills it each frame, and Rust reads it through its address (`GetDirectBufferAddress` via the `jni-sys` crate) — `nativeRender()` takes no args, zero per-frame JNI marshaling. A `SCHEMA_VERSION` guards the byte-for-byte layout mirror between `EwoHudData.java` and `hud.rs`. E4 added overlay input: Right Shift opens a custom `Screen` (`EwoOverlayScreen`) that frees the cursor and forwards mouse input to Rust. E5 made the HUD editable — drag widgets, snap-to-align, toggle them, anchor them via a side panel; the layout persists to `<config>/EwoClient/hud.toml`. E6 turned the overlay into a 3-tab dashboard — **HUD · MODS · SETTINGS** — with in-game bundled-mod toggles (write-back via `crates/ewo-launcher/src/overlay_mods.rs` — a per-instance `overlay-mods.toml`/`overlay-mod-overrides.toml` pair) and the `HudPaintRate` cap as a real setting.

E7 closed Phase E with the **glass-refract decision**: the MODS/SETTINGS overlay views frost the live game behind them; the HUD editor view leaves it sharp. The frost is a *genuine* blur but **cached on a third clock** — `refresh_frost` recomputes it ~10×/sec into a quarter-resolution surface via a clean two-step 2× downscale + a small gaussian, and `composite` upscales that cache every frame with a cubic resampler (cheap) plus a faint Velvet wine wash. A first cut that blurred `fbo 0` directly every composite looked chunky and was wasteful; the cached downscale→blur→cubic-upscale chain is smooth and nearly free per frame. Verified live at ≈500 fps with the overlay open.

**Phase E is complete (E0–E7, all 2026-05-20).** [`PHASE_E_PLAN.md`](PHASE_E_PLAN.md) holds the per-step detail and the locked architecture decisions — it is now a record, not a forward plan. The in-game HUD ships: full read-only widget set, draggable editor, 3-tab dashboard, in-game mod toggles, cached glass refract.

### Known gaps + small follow-ups

- **EwoLoader fat jar — local dev uses `file://`, snapshots live on GitHub Releases.** Active iteration reads the local build output via `file:///C:/.../ewo-loader-0.19.2-fat.jar` so the rebuild loop stays one step (per the "GitHub Release snapshots" section above). `v0.19.2-bundle.1` is the first off-box backup; cut new snapshots with `gh release upload --clobber` when shipping is meaningful. The launcher's `Authorization: Bearer $EWO_LOADER_TOKEN` wiring is in place + dormant — flip the manifest URL to the GitHub asset API URL and set the PAT to use the hosted jar.
- **Indium — upstream-blocked.** Latest release `1.0.36+mc1.20.1` published 2025-02-25, no MC 26.x build. Continuity 3.x ships without it for 26.1 (the original "Continuity needs Indium" assumption was wrong); Indium would unblock other render-API-extension mods if/when it returns to active maintenance.
- **macOS/Linux JRE bundling** — `.tar.gz` extraction shipped; not exercised on a real Linux/macOS box yet.
- **Hyprland verification** — still not run on actual Linux.
- **Pixel-parity pass** — never formally walked through every screen vs `style/*.png` with side-by-side screenshots.
- **Refresh-token at-rest encryption** — `auth.toml` is plaintext. Fine for single-developer dev box; would want DPAPI / keychain / libsecret before binary distribution.
- **Settings → Java runtime dropdown** — decorative; `pick_for_major` does the right thing automatically. Wiring is low-priority.
- **Duplicate `file_url_to_path`** in `loaders/fetch.rs` + `downloads/job.rs`. Both work; factoring into a shared module is a clean-up follow-up.

### Useful runtime conventions

- Disk: `<config>/EwoClient/` is `%APPDATA%/EwoClient` on Windows, `$XDG_CONFIG_HOME/EwoClient` (or `~/.config/EwoClient`) on Linux. Layout under it:
  ```
  shared/{versions,libraries,assets}/  ← Mojang-compatible; vanilla launchers can read these
  instances/<name>/                    ← per-instance: worlds, screenshots, mods, natives, logs
  runtime/<major>/jre/                 ← bundled JREs auto-fetched from Adoptium
  auth.toml                            ← AccountStore { active, accounts[] } (plaintext) — Phase F0
  profiles.toml                        ← client-profile registry { active, profiles[] } — Phase F2
  profiles/<name>/client.toml          ← profile-scoped config (tweak tokens, theme, audio, keybinds)
  profiles/<name>/hud.toml             ← in-game HUD layout, per profile — Phase F5a
  profiles/<name>/modules.toml         ← per-module config (enabled + settings) — Phase G
  versions_cache.json                  ← master manifest cache (6h TTL)
  settings.toml                        ← GLOBAL-only config (paths, window mode, log level) — Phase F2
  instances.toml                       ← persisted instance list
  ```
- Threading: every long-running task spawns a `std::thread` and reports back via `mpsc`. Polled by `App` once per frame in `RedrawRequested`. No tokio/smol — see CLAUDE.md non-negotiables.
- All HTTP via `ureq` (sync). User-Agent: `EwoClient/0.1 (+https://github.com/lewlone/ewoclient)`.

### What NOT to do in v2

- Don't prefetch Microsoft credentials at app startup if a user might never sign in. Lazy-fetch on launch.
- Don't add a real-time launcher protocol (Discord rich presence, telemetry, news). The "OFFLINE FIRST. NOTHING PHONES HOME." invariant from v1 stays — auth + version manifest + asset CDN are the only network calls.
- Don't write your own auth lib; the Microsoft-auth chain is well-documented but the failure modes are subtle. Reference https://wiki.vg/Microsoft_Authentication_Scheme religiously.
- Don't expand the curated version allowlist (`is_supported`) without a reason. The launcher targets specific versions deliberately.
- Don't break the disk layout — vanilla-launcher interop is a real feature.

---

## Phase F — Profiles, Dashboard & Keybinds ✅ shipped (F0–F6, 2026-05-21 → 2026-05-22)

The first feature phase past v1 + v2. `PHASE_F_PLAN.md` (repo root) holds the
per-step detail and is now a record, not a forward plan. Three pillars:
multi-account, client profiles, and an in-game dashboard.

### Accounts (F0–F1)

`auth.toml` went from a single account to an `AccountStore { active, accounts[] }`
(transparent migration on first F-build launch). `AuthService` owns the store
plus an `AuthOp` (Idle / Working / Failed) — the single source of truth. The
**Settings → Account tab** is a list: add / remove / set-active, with monogram
avatars (Velvet-tinted disc + initial; real skin-head avatars deferred — the
in-game 3D viewer covers the skin-display need).

### Client profiles (F2–F3, rename in F6)

A *client profile* is a named, hot-swappable bundle of cosmetic + perf config —
**global, not per-instance**. Orthogonal to accounts (any account × any profile).
Disk: `profiles.toml` (registry), `profiles/<name>/client.toml` (profile-scoped:
the 5 tweak tokens + theme / vsync / max-fps / audio / **keybinds**), and
`settings.toml` is now **global-only** (paths, window mode, auto-backup, log
level, telemetry). The **Settings → Profiles tab** manages them — switch / new /
duplicate / delete / **rename** (an inline text field with a blinking caret;
`profile::rename` moves the `profiles/<name>/` directory + updates `profiles.toml`,
`profile::is_valid_name` rejects path-unsafe names). Switching re-applies the
config live via `App::apply_loaded_config`.

The launcher-side `profile` module (`crates/ewo-launcher/src/profile.rs`) owns
all of this — `load` / `save` (split/merge the unified `SettingsConfig` ↔ the
on-disk pair), `list` / `active_name` / `switch` / `create` / `duplicate` /
`delete` / `rename`, and `load_keybinds` / `save_keybinds`.

### In-game dashboard (F4–F5)

**"The dashboard" is an in-game overlay tab, NOT a launcher home screen** — the
launcher main menu is unchanged (this was a scope correction mid-phase). The
overlay tab strip is **HOME · HUD · MODS · SETTINGS**. HOME (`draw_home` in
`crates/ewo-jni/src/hud.rs`) is the overview: session stat cards (FPS / ping /
playtime / coords / server), account + active-profile line, per-HUD-widget
quick-toggles, and a drag-rotatable **3D skin viewer**. The data pipeline is
`SCHEMA_VERSION` 3 (`EwoHudData.java` ↔ `hud.rs`). `hud.toml` moved under
`profiles/<name>/` (F5a) so HUD layout is per-profile; the overlay SETTINGS tab
has an in-game profile switcher (F5b) that hot-swaps the layout live.

### 3D skin viewer

`crates/ewo-jni/src/skin.rs` — a Skia software renderer for the Minecraft
player model: 12 textured-quad cuboids + cape, box-UV unwrap, back-face cull,
painter's-sort, per-face shade, drag-to-rotate. **Slim + wide models.** The mod's
`EwoSkinExport` downloads the skin/cape PNGs from the player's GameProfile
`textures` property and writes an `ewo-skin-slim` marker.
- **Gotcha:** the mod must read the `textures` property **reflectively** — the
  build-classpath authlib skews from the runtime one (record `properties()` /
  `value()` vs. class `getProperties()` / `getValue()`); a direct call compiles
  but throws `NoSuchMethodError` at runtime.
- **Gotcha:** the viewer reloads the skin on `ewo-skin.png` **mtime change**,
  not just once — else a stale png from an earlier launch freezes the slim flag.

### Keybinds (F5c)

A **module-extensible keybind registry** — launcher `keybind` module
(`crates/ewo-launcher/src/keybind.rs`): `KeyChord` (a GLFW key code + modifier
bitmask — GLFW is Minecraft's own key namespace, so the in-game side compares
the integer with no translation), `KeybindAction`, the static `REGISTRY` (F
ships one: `overlay.open` → Right Shift), a winit→GLFW key table, label
formatting. Keybinds are **per client profile** (`client.toml` `[keybinds]`
table). The **Settings → Keybinds tab** is a remap row per action — click the
chord button to arm a rebind, the next key press is captured. The active
profile's keybinds resolve to `<instance>/ewo-keybinds.txt` before each launch;
the mod's `EwoKeybinds` reads it so the overlay-open key (`KeyboardHandlerMixin`
+ `EwoOverlayScreen`) is rebindable. **The registry is the seam future
EwoClient *modules* plug their bindable actions into** — modules (EwoClient's
own legit client features) are out of scope for F; F built only the seam.

### Phase F — verification still pending (the user does these)

- Launcher-side: the Account / Profiles / Keybinds Settings tabs + profile
  rename — built + committed, not yet eyeballed.
- In-game: the F4 HOME tab, F5 profile hot-swap, the 3D skin viewer — build
  clean; in-game testing is crash-prone until the user disables NVIDIA
  Threaded Optimization in NVCP (it fights the HUD's 2nd GL context — a
  `nvoglv64.dll` access violation under heavy GPU load).

---

## Phase G — EwoClient Modules ✅ shipped (G0–G8, 2026-05-22)

The second feature phase past v1 + v2. `PHASE_G_PLAN.md` (repo root) holds the
per-step detail and is now a record, not a forward plan. Phase G builds the
**modules** the Phase F keybind registry was the seam for: in-game,
legit-client quality-of-life features with an on/off state, optional settings,
and an optional keybind. Seven shipped — see the table below.

**Constraint (Phase E #4) holds: legit-client features only.** No hacked-client
modules. The MODULES UIs are clean Velvet feature lists, not ClickGUI grids.

### The architecture

The pre-G in-game data flow was one-way — Java→Rust (`EwoHudData`: game state
in, Rust paints). Modules must *change the running game*, which only the Java
mod can do, so Phase G adds the missing direction — a **live Rust→Java
channel**:

- **`ewo_core::modules`** — the module catalog (`REGISTRY`): pure `&'static`
  data, the single source of truth shared by the launcher and `ewo-jni`. The
  launcher's `keybind::REGISTRY` is now *generated* from it — each module
  contributes a `KeybindAction`.
- **`modules.toml`** — per client profile (`profiles/<name>/modules.toml`,
  sibling of `hud.toml`): each module's `enabled` + settings. Both the launcher
  and `ewo-jni` read/write it; modules apply *live*, so there is no
  overrides-dance (unlike bundled mods).
- **`EwoModuleData`** — a second shared `ByteBuffer`, the mirror image of
  `EwoHudData`: Rust writes every module's state each frame, the mod reads it
  to drive the effect mixins. `crates/ewo-jni/src/modules.rs` owns the Rust
  side; `EwoModuleData.java` mirrors the layout (its own `SCHEMA_VERSION`).
- Two new JNI methods: `nativeInitModules` (register the buffer) and
  `nativeModuleToggle` (a keybind press round-trips a toggle through Rust,
  which owns module state).

### The module set

All seven are **non-destructive** — each overrides a *computed* value via a
mixin; nothing writes Minecraft's `options.txt`, so toggling a module off
restores vanilla behavior exactly.

| Module | Effect | Hook (26.1.1 Mojmap) |
|---|---|---|
| Full Bright | World renders fully lit | `@Inject` cranks `LightmapRenderState.brightness` after `LightmapRenderStateExtractor.extract` |
| FOV Control | FOV past the 110° cap | `@Redirect` the `options.fov()` read in `Camera.calculateFov` |
| Toggle Sprint | Sprint held for you | force the `keySprint` `KeyMapping` from the frame hook |
| Toggle Sneak | Sneak held for you | force the `keyShift` `KeyMapping` from the frame hook |
| No Damage Tilt | No hit camera-lurch | `@Inject` cancel on `GameRenderer.bobHurt` |
| No View Bob | No walk view-bob | `@Inject` cancel on `GameRenderer.bobView` |
| FreeLook | Spectator-style flying freecam — hold to detach; WASD flies, mouse looks, body frozen, player model visible, snaps back on release | `@Redirect` `LocalPlayer.turn` in `MouseHandler.turnPlayer` (mouse → freecam) + `@ModifyVariable` on `Camera.setRotation` (rotation) + `@Inject` at `Camera.alignWithEntity` RETURN with `@Shadow`'d `setPosition` (position) + `@Redirect` `CameraType.isFirstPerson()` in `alignWithEntity` (force detached so the body renders). Body is frozen by forcing the six movement `KeyMapping`s `setDown(false)` while active; flight uses raw `glfwGetKey` on default WASD/Space/Shift/Ctrl. No-clip in multiplayer = line-of-sight advantage; a leash or block collision would be the legit-client compromise. |

**26.x rendering moved** — load-bearing for future mixin work: `LightTexture` →
`net.minecraft.client.renderer.Lightmap`; the lightmap is GPU-driven (a
`LightmapRenderState` UBO, `Lightmap.getBrightness` has no callers); FOV is no
longer `GameRenderer.getFov` — it's `Camera.calculateFov`, and the projection
matrix is built in `Camera.extractRenderState`.

### The UIs

- **In-game** — a 5th overlay tab: `HOME · HUD · MODULES · MODS · SETTINGS`.
  `draw_modules` in `crates/ewo-jni/src/hud.rs` is a Velvet feature list — a
  toggle per module + a slider for FOV. A toggle writes `modules.toml` and
  flows live through the buffer.
- **Launcher** — an 8th `SettingsTab::Modules`, modelled on the Keybinds tab:
  a toggle per module + FOV's slider, editing the active profile's
  `modules.toml` (`profile::load_modules` / `save_modules`).
- **Keybinds** — each module contributes a `KeybindAction` (default unbound),
  so module hotkeys appear in the launcher Keybinds tab and resolve through
  `ewo-keybinds.txt` for free. In-game, `KeyboardHandlerMixin` toggles a module
  on its key; FreeLook's key is a hold, polled by `EwoFreeLook`.

### Phase G — verified live (2026-05-23)

All seven modules built, deployed and smoke-tested in-game. FreeLook was then
reworked into the flying spectator-style freecam described in the table row
above — the original "look around with body frozen" implementation worked but
was nearly invisible in first person, so the user asked for a real spectator
view.

Two non-blocking notes carry forward:
- Full Bright uses `brightness = 15.0` — tune in
  `LightmapRenderStateExtractorMixin` if it reads too dim or washed.
- In-game testing stays crash-prone until NVIDIA Threaded Optimization is off
  in NVCP (the HUD's 2nd GL context — see Phase E).

### Stale `file://` jar gotcha (debugging trap)

The launcher caches `file://` libraries under `shared/libraries/<path>` and
does **not** refresh them when the source jar changes. A multi-hour FreeLook
debugging session in 2026-05-22 was caused by exactly this: every "fresh
build + launch" was secretly running a several-hours-old jar; diagnostics
looked impossible until `certutil -hashfile` on the build output vs the
`shared/libraries` copy showed different sha1s.

**Fix in `ingame-mod/build.ps1`**: after `jar.exe --create`, the script copies
the freshly-built jar into
`%APPDATA%\EwoClient\shared\libraries\dev\lewlone\ewo-hud\0.1.0\ewo-hud-0.1.0.jar`,
bypassing the cache. **Always run `build.ps1` for mod changes; never `javac`
by hand** or you skip the deploy. If a change ever "doesn't take effect",
compare sha1s before anything else. The same caching almost certainly applies
to the EwoLoader fat jar (also a `file://` library entry); it has not bitten
in practice because the loader is rebuilt infrequently.

---

## Unfocused-swap memory leak (fixed 2026-05-26)

The launcher used to leak ≈6 KB / frame (≈3 MB/s at ~500 fps) of
C++-side memory whenever the window wasn't the foreground — by the time
you'd been in YouTube fullscreen for half an hour, the launcher could be
sitting on 6 GB. The hunt is logged in [[leak_hunt]] memory; the short
version: `wglSwapBuffers` on a fully-obscured window queues presentations
in the NVIDIA GL driver indefinitely because the compositor never
consumes them. Skia's tracked caches were all bounded the whole time;
nothing in Rust grew. winit's `WindowEvent::Focused(false)` /
`Occluded(true)` / `is_minimized()` are all **unreliable on Win11 when
another app takes fullscreen** — they kept reporting the launcher as
visible, focused, and not minimised even when YouTube fullscreen sat on
top of it.

**Fix:** skip `RedrawRequested` entirely (no Skia work, no
`swap_buffers`) when the launcher isn't the OS foreground window. The
foreground check polls `GetForegroundWindow()` via Win32 (lives in
`crates/ewo-launcher/src/window/win32.rs::is_foreground`, wrapped by
`window::is_foreground`). On non-Windows it returns `true` always, so
the cross-platform path falls back to winit's `Occluded` +
`is_minimized` signals — those work fine on Hyprland/wlroots because
Wayland's frame-callback model makes occlusion explicit. The render
loop also still skips on `WindowEvent::Occluded(true)` and
`window.is_minimized() == Some(true)` for the platforms / scenarios
where those do fire.

**`!self.focused` is intentionally NOT a skip signal** — the user might
have a chat or browser focused on a second monitor while the launcher
animates on their primary. Foreground covers the actually-leak-causing
case (no compositor presentations possible) without sabotaging that.

### Leak-hunt instrumentation — strip before release

Various leak-hunt diagnostics + Skia cache caps were left in place
after the fix landed. They're harmless but add noise + a tiny per-alloc
atomic overhead, and should be removed when the project is ready to
ship binaries. **Every diagnostic site is tagged with the comment
marker `LEAK_HUNT_INSTRUMENT`** — `git grep LEAK_HUNT_INSTRUMENT`
turns them all up. Removing the lot:

- `crates/ewo-launcher/src/main.rs` — the `CountingAllocator` block +
  `alloc_stats()` + `#[global_allocator]`, the periodic `mem:` / `alloc:`
  log block in `RedrawRequested`, and the `cap_skia_global_caches()`
  call in `main()`.
- `crates/ewo-render/src/gl_backend.rs` — `cap_skia_global_caches`,
  `log_skia_global_cache_state`, `format_bytes`, the
  `set_resource_cache_limit(192 MB)` call in `GlBackend::new`, the
  `frames` field, the periodic `perform_deferred_cleanup` +
  `skia gpu:` log block at the bottom of `render()`.
- `crates/ewo-launcher/src/window/win32.rs::process_memory` and the
  `window::process_memory` wrapper in `window/mod.rs`.
- `Cargo.toml` — `Win32_System_Threading` + `Win32_System_ProcessStatus`
  features on the `windows` workspace dep (only used by
  `process_memory`).

The actual fix (`is_foreground`, the `WindowEvent::Occluded` handler,
the `self.occluded` field, the render-skip check using all three
visibility signals) is NOT tagged — it stays forever.

---

## Post-ban refactor: legit / pvp split (2026-05-26)

The "legit-client only" rule that Phase G stated drifted hard during the
post-G iteration sprints — what was originally CLAUDE.md non-negotiable
#4 (Phase E) got walked across a series of incremental PvP / macro
modules (Auto Tool, Auto Totem, Hand Restock, Auto Eat, Sprint Tap, Auto
Mace Swap, Auto Jump Reset, Auto Pearl, Riptide Boost, Auto Hit Timing,
Mace Combo, Wind Charge MLG, Triggerbot, …) that shipped without docs
updates. An anticheat ban landed on CatPvP **with the macros switched
off**, which is the giveaway that class-name fingerprinting (not
behavior detection) was the surface. **This refactor splits the catalog
into a legit set that ships by default and an assist set behind a build
flag.**

### What's where after the refactor

- **Legit module catalog** (always ships, slots 0..11 of REGISTRY):
  Full Bright, FOV Control, Toggle Sprint, Toggle Sneak, No Damage Tilt,
  No View Bob, FreeLook, No Fire Overlay, Crosshair on Reach, No Pumpkin
  Overlay, Hit Color, Hit Indicator. Pure rendering / read-only / universal
  QoL — zero packet synthesis.
- **Assist module catalog** (slots 12..25 of REGISTRY, only present under
  `--features pvp` / `build.ps1 -Pvp`): Auto Tool, Auto Totem, Legit
  Elytra Swap, Hand Restock, Auto Eat, Auto Jump Reset, Sprint Tap,
  Knockback Maximizer, Auto Mace Swap, Auto Pearl, Riptide Boost, Reach
  Lock, Auto Hit Timing, **Swing Cadence**. All synthesize input packets
  (inventory clicks, hotbar swaps, sprint state, attack packets) — that's
  the user's "touching packets isn't fine" red line, and that's why these
  classes must not exist in the legit jar at all.
- **Deleted outright** (gone from both legit + pvp catalogs): `auto_crit`
  (was already a no-op since the bunny-hop tell was too obvious),
  `mace_combo` (tick-perfect kill chain — beyond the line even for
  semi-anarchy), `wind_charge_mlg` (snap-pitch mode was literal aim
  assist).
- **Renamed** with humanization: `triggerbot` → `swing_cadence`. Same
  core behaviour (auto-fires the next swing when attack-strength is
  ready and the crosshair sits on a living target), plus three
  humanization knobs on top — minimum inter-fire interval (default
  200 ms ≈ 5 hits/sec cap), ±ms jitter (default 30), and a
  target-acquired reaction delay (default 80 ms). The class identity
  changed (`EwoTriggerbot` → `EwoSwingCadence`) specifically to drop the
  obvious class-name fingerprint a class-scan AC would flag on.

### Build mechanics

- Rust: each crate (`ewo-core`, `ewo-jni`, `ewo-launcher`) has a `pvp`
  feature. `ewo-core` is the one that gates the registry; `ewo-jni` and
  `ewo-launcher` just propagate it. `cargo build` is the legit build;
  `cargo build --features pvp` (or `-p ewo-jni --features pvp`) is the
  pvp build.
- `crates/ewo-core/src/modules.rs` uses `#[cfg(feature = "pvp")]` on
  individual REGISTRY entries — the legit-build registry is a 12-entry
  prefix of the 26-entry pvp registry. Slot indices for legit modules
  are stable across builds, so Java's legit slot constants
  (`EwoModuleData.FULLBRIGHT = 0`, etc.) never change.
- Java: gated classes live in `dev.lewlone.ewohud.assist.*`
  (modules + `EwoActionMotor` + `EwoSwingCadence`) and their mixin in
  `dev.lewlone.ewohud.assist.mixin.PlayerAttackAssistMixin`. Assist slot
  constants live in `assist.AssistSlots` (slots 12..25). The legit
  `EwoModuleData.java` carries only legit slot constants — assist names
  don't enter the legit jar's constant pool at all.
- The legit driver `EwoModules.java` resolves
  `dev.lewlone.ewohud.assist.EwoAssist` via reflection at class-load:
  present → `tick()` + `handleKeyPress()` delegate to it; absent (legit
  build) → both are no-ops with zero per-frame cost.
- The pvp-only `PlayerAttackAssistMixin` injects HEAD on
  `Player.attack(Entity)` alongside the legit `PlayerAttackMixin`
  (multiple HEAD injects coexist). The legit `PlayerAttackMixin` was
  trimmed to only the legit handoffs (`EwoHitRange.onAttack` +
  `EwoComboTracker.onAttack`); the assist mixin adds Knockback Max +
  `EwoSprintTap.onAttack`.
- Two mixin configs: `ewohud.mixins.json` (legit, always shipped) and
  `ewohud-pvp.mixins.json` (additive, pvp jar only). Two `fabric.mod.json`
  variants: legit references only the legit config; `fabric-pvp.mod.json`
  references both (`build.ps1 -Pvp` renames it to `fabric.mod.json`
  inside the jar).
- `EwoModuleData.SCHEMA_VERSION` bumped 2 → 3. The new layout: legit
  slots 0..11 in stable order; assist slots 12..25 only written when
  Rust is built with `--features pvp`. `MODULE_COUNT` is now dynamic —
  read from buffer offset 4, where Rust writes the live registry length
  per build. `enabled(slot)` returns false for any slot at or past that
  value, so legit-build code asking about an assist slot is safe.
- `ingame-mod/build.ps1` gains a `-Pvp` switch. Without it: cargo build
  is plain, assist Java sources are filtered out of `javac`,
  `ewohud-pvp.mixins.json` is excluded from the jar, legit
  `fabric.mod.json` ships. With it: cargo build adds `--features pvp`,
  assist sources are compiled, both mixin configs ship, and
  `fabric-pvp.mod.json` ships as the in-jar `fabric.mod.json`.

### Verification (2026-05-26)

- `cargo test -p ewo-core --lib` passes 8 tests (legit). Same with
  `--features pvp` (one extra test covers the assist slot ordering).
  Two regression-guard tests block re-introduction: one fails if any
  deleted id reappears in REGISTRY; one fails if the legit-build
  REGISTRY ever exceeds 12 entries.
- `build.ps1` (legit) produces a jar whose `jar --list` shows zero
  classes under `dev/lewlone/ewohud/assist/` and exactly one mixin
  config (`ewohud.mixins.json`). `build.ps1 -Pvp` produces a jar with
  all 14 assist classes + the pvp mixin + both mixin configs.
- **In-game verification of both builds still pending** — the user does
  this. Likely smoke test: launch a legit-build session, open the
  overlay's MODULES tab, confirm only the 12 legit modules render;
  launch a pvp-build session, confirm all 26 show + Swing Cadence
  toggles + fires with the humanized cadence.

### Known follow-ups

- The launcher's profile-level `modules.toml` is id-keyed so settings
  for the deleted modules linger as inert sections. Cosmetic; the
  modules are no longer in REGISTRY so the launcher Settings → Modules
  tab won't surface them. A future cleanup pass on `profile::load_modules`
  could drop unknown ids on save.
- Keybinds for `triggerbot` (now `swing_cadence`) need a rebind because
  the id changed — the launcher Keybinds tab will show `swing_cadence`
  unbound on profiles where `triggerbot` was bound.

---

## Phase H — Social: Friends, Presence, Launcher-Link (built; deploy + live test pending)

The third feature phase past v1 + v2. [`PHASE_H_PLAN.md`](PHASE_H_PLAN.md)
holds the original forward plan (now partly stale — see below). Phase H
plugs the launcher into the user's existing **chickenedin** Minecraft
network (**renamed Frogsy in mid-2026** — this section predates the rename;
domains/API bases below still say chickenedin.com, verify what's live before
relying on them) so the launcher knows your friends, their presence, and
(future H6) lets you Roblox-style join them. **Offline-first holds**: signed-out
launcher makes zero network calls; signed-in-but-unlinked makes MS-auth
calls only; social calls happen only once the launcher has a per-user
`social_token`.

### Three repos, all built — the contract is verified-aligned

Phase H spans three repos. As of 2026-05-30 **all three are implemented
and the wire contract matches across them** (verified handler-by-handler
against [`crates/ewo-launcher/src/social/mod.rs`](crates/ewo-launcher/src/social/mod.rs)):

1. **chickenbot (Python)** — `C:/Users/valtteri/Desktop/FULLSTACK/chickenbot/`,
   branch `chickenbot-mod-tools`. **Committed + clean.** `database.py`
   declares the Phase H tables (`launcher_link_codes`, `social_tokens`,
   `presence`, `friendships`, `mc_name_cache`) and helpers
   (`mint_social_token`, `validate_social_token`, `list_friendships_for`,
   `upsert_friendship_request`, `respond_to_friendship`,
   `remove_friendship`, `upsert_presence`, `consume_launcher_link_code`,
   `lookup_uuid_by_mc_name`). `api.py` registers + implements every
   endpoint below. Auth: `check_auth` (system `API_SECRET`) for
   plugin/website calls; `check_user_token` (per-row `social_tokens`
   bearer) for launcher calls.
2. **ChickenLink (Paper plugin)** — `FULLSTACK/NETWORK/ChickenLink/`.
   The `/launcher-link` command (`LauncherLinkCommand.java`) mints a
   6-digit code via `POST /api/launcher-link-code` and shows it to the
   player. **Uncommitted as of 2026-05-30** (command + `APIClient` +
   `ChickenLink.java` registration + `plugin.yml`).
3. **EwoClient launcher (this repo)** — `social/mod.rs` (the HTTP +
   state machine), `screens/friends.rs` (`Screen::Friends`),
   `screens/launcher_link_modal.rs` (6-digit redeem modal). Wired into
   `App` as `SocialState`. **Uncommitted** (part of the big checkpoint).

### The wire contract (launcher ↔ bot)

Base URL = `https://chickenedin.com/bot` (override with env
`EWO_BOT_API_BASE`); endpoints hang under `{base}/api/...`, reverse-proxied
to the bot's `:8080`. **The bot itself listens on `/api/...` directly** —
the `/bot` prefix is the nginx route.

```
GET    /api/links/by-uuid?minecraft_uuid=<dashed>   PUBLIC  → {linked: bool}
POST   /api/launcher-link-code                      system  body {minecraft_uuid} → {code, expires_at} | 404 not-linked
POST   /api/launcher/link                           PUBLIC  body {code} → {social_token, discord_id} | 404 code-invalid-or-expired
POST   /api/presence/heartbeat                       user    body {minecraft_uuid, location, screen?, server_addr?, visibility?} → {ok}
GET    /api/friends                                  user    → {friends[], incoming[], outgoing[]} (discord_id as STRING, presence nested)
POST   /api/friends/request                          user    body {target_mc_name} → {status} | 404 {status: not-found|not-linked}
POST   /api/friends/respond                          user    body {request_from_discord_id:int, action} → {status}
DELETE /api/friends/{discord_id}                     user    → {status: removed}
```

Contract gotchas (already correct on both sides — recorded so they don't
regress): `discord_id` is a **string** in `/api/friends` entries (the
launcher parses it with `.as_str()`; a numeric value would silently drop
every friend) but a **number** in `/api/friends/respond`'s body. UUIDs go
to the bot **dashed** (`social::uuid_with_dashes`). Two cosmetic-only
mismatches remain unfixed: a self-request returns HTTP 400 so the launcher
shows "http 400" instead of "you can't friend yourself", and an
`already_*`/`reciprocal` status renders as generic text. Neither breaks
anything.

### What's done vs. open

- **Launcher H1–H5 built**: link probe on MS sign-in (Account tab shows
  linked status), `/launcher-link` redeem modal, 30s presence heartbeat,
  friends list + request/respond/remove, `Screen::Friends`. Some loose
  ends (the `friend_action` toast, `refresh_friends_now`, and the
  `InGame` presence variant are coded but not all call-sites wired —
  these surface as dead-code warnings).
- **H6 (live server-status widget + Roblox-style join) BUILT (2026-05-30)**:
  - Launch-into-server plumbing — `App::active_server` + a shared
    `start_launch(idx, server, time)` helper (the Launch button and both
    join paths funnel through it). When set, `try_real_launch` appends
    `--quickPlayMultiplayer <addr>` (the 1.20+ replacement for the removed
    `--server`/`--port` pair) and the presence heartbeat reports
    `in_game · <addr>` while the JVM is alive (gated on `launch_rx.is_some()`).
  - `social::ServerStatus` poller (`maybe_refresh_server_status`, 15s,
    main-menu only) against the now-public `GET /api/server-status`.
  - Main-menu network widget (`main_menu::draw_server_widget`, lower-left
    Velvet card) — "X / Y online · TPS Z", click joins
    `play.chickenedin.com`. Render-side `ServerWidgetView` mirrors the
    `FriendRowView` cross-crate pattern.
  - Friend "Join" button — `server_addr` added to `FriendRowView`; the
    button draws only for in-game friends and joins their `server_addr`.
  - **Bot change**: `GET /api/server-status` made public (POST stays
    system-authed) — needs a redeploy. **Visual placement of the
    main-menu widget is unverified** (lower-left; may want tuning vs the
    right-column menu items on narrow windows) — eyeball pass pending.
- **H7 (WebSocket push) not done** — polling-only, by design until lag
  justifies it.
- **In-game FRIENDS overlay tab BUILT (2026-05-31, read-only)** — an 8th
  overlay tab; the strip is now HOME · HUD · CROSSHAIR · MODULES · PVP ·
  MODS · FRIENDS · SETTINGS. **File-bridge, no HTTP in the cdylib**: the
  launcher writes a per-profile `ewo-friends.txt` snapshot
  (`<online>\t<name>\t<presence>\t<server_addr>` per accepted friend) on
  each friends-list change via `profile::active_dir()`; the cdylib's
  `social::read_friends()` reads it fresh each frame the tab is visible and
  `hud::draw_friends_view` renders the Velvet list. View-only — mutations +
  join stay launcher-side. **Freshness caveat**: the launcher only rewrites
  the snapshot while it's foreground (its leak-fix skips per-frame work when
  backgrounded), so during active play the list reflects the last
  launcher-foreground refresh (≈launch time); live-during-play needs an
  in-game poller or a launcher background tick — deferred. Also fixed a
  latent bug: `hud::tab_layout` was hardcoded to 6 slots while
  `OverlayView::ALL` had 7 (PVP), silently clipping the SETTINGS tab off
  the strip — it now sizes to `ALL.len()`.
- **THE real remaining blocker is ops, not code**: the bot must be
  *deployed/running* on the VPS with nginx routing `/bot/api/*` → bot
  `:8080`, and a live end-to-end test (sign in → `/launcher-link`
  in-game → paste code → see a friend's presence) has not been run. H0's
  SSH key (`ssh ewo-vps`) was generated 2026-05-27; the public key may
  still need deploying to the VPS.

## Two undocumented in-game features (shipped, in the big checkpoint)

Built during the post-G / Phase H sprints and absent from every plan doc
until now. Both live in `crates/ewo-jni` (the in-game HUD cdylib) and the
overlay editor:

- **Custom crosshair** — [`crates/ewo-jni/src/crosshair.rs`](crates/ewo-jni/src/crosshair.rs)
  + `ingame-mod/.../mixin/GuiCrosshairMixin.java`. A per-profile
  `crosshair.toml` (sibling of `hud.toml`), an in-overlay **CROSSHAIR**
  editor tab (sliders + toggles + colour swatches + live preview), and an
  in-world crosshair drawn at screen center. When enabled, the Java mixin
  reads `nativeIsCustomCrosshairEnabled` and cancels the vanilla
  `Gui.extractCrosshair` so only ours shows. `MouseHandlerInputMixin` was
  added alongside.
- **Media controller** — [`crates/ewo-jni/src/media.rs`](crates/ewo-jni/src/media.rs).
  Reads Windows **SMTC** (System Media Transport Controls — the global
  feed Spotify/browsers/Apple Music write to) on a background thread;
  paints a HUD "now playing" widget (title/artist/scrub/transport/thumb)
  plus a large card on the HOME overlay tab. Transport clicks
  (play/pause/skip) flow back through `MediaService::act` →
  `TryPlayAsync`/`TryPauseAsync`. `EwoHudData` schema is at version 10.

---

## Launcher window: transparent floating card (2026-05-31 — supersedes the Step 1/2 "app-window chrome" notes)

The launcher window changed from an **opaque card painted inside a 28px
margin with a 3-layer berry box-shadow** to a **real per-pixel-alpha window
where the rounded card IS the window**. The old Step 1/Step 2 notes
(`CARD_INSET = 28`, `draw_chrome_outer` berry-glow + black drop shadows,
`DWMWCP_ROUND`) are historical — here's the current state.

- **`with_transparent(true)`** on the winit window (the GL config already
  carries `alpha_size 8`). `draw_chrome_outer` clears the frame to
  `Color::TRANSPARENT`; the desktop shows through anywhere the card isn't.
  Transparent GL windows can be driver-finicky on Windows — it composited
  fine here (NVIDIA), but that's the thing to re-check if it ever renders
  black instead of the desktop.
- **`CARD_INSET = 0`** (both `app_window::CARD_INSET` *and* the launcher's
  mirror `CARD_INSET_LP` used for cursor→card mapping — they MUST match or
  the cursor drifts from the widgets). The 22px-rounded card fills the
  window edge-to-edge, so **the window's edges are the card's edges** and
  the OS resize/drag hit zones (which key off the window rect) line up. This
  fixed a long-standing UX bug where the invisible 28px margin still counted
  as the window, putting the resize edge out in empty space.
- **No outer drop shadow** — there's no margin to render one into.
  `draw_outer_shadow` is kept (`#[allow(dead_code)]`) for a future
  shadow-with-margin variant. Edge definition is the inset rose hairline rim
  in `draw_chrome_inner`. (An interim transparent-window build *did* keep a
  margin + shadow; the berry glow bled a coloured halo onto apps behind the
  window, so it was first neutralised, then dropped with the margin.)
- **`win32.rs`: `DWMWCP_DONOTROUND`** (was `DWMWCP_ROUND`) — we paint our own
  22px corners, so DWM must not also round the window rect (its ~8px would
  clip our corners + cast a competing rectangular shadow).
- **Minimize + close buttons** (top-right) — `app_window::window_button_bounds`
  + `draw_window_buttons`: vector — / × icons, hover fills a soft rounded bg
  (rose / ember) + brightens. `hit_test` excludes the button rects from the
  drag caption so clicks land; the launcher handles them before modals so
  they always work. Close → `event_loop.exit()`; minimize →
  `Window::set_minimized(true)`.

### Freeze-on-exit — fixed (was a bug since the start)

On quit (Quit-to-desktop / taskbar close / the close button) the GL-context +
window teardown could **deadlock the main thread**, leaving the window "Not
Responding" until killed via Task Manager. Fix: `main()` calls
**`std::process::exit(0)` right after `event_loop.run_app` returns**, skipping
the hanging `App`/`GlBackend` `Drop`. Nothing in `Drop` needs to run —
settings / instances persist on change, not on exit — and the OS reclaims the
window, GL context, and detached threads.

### Packaging — shortcut + icon + portability

- **Release builds are GUI apps** — `#![cfg_attr(not(debug_assertions),
  windows_subsystem = "windows")]` in `main.rs`, so no console window pops up
  from a shortcut. Debug keeps the console for `cargo run`.
- **App icon**: `assets/icon.ico` (multi-res 16/32/48/256, 32-bit RGBA) is
  embedded into the exe by `crates/ewo-launcher/build.rs` via the
  `winresource` build-dep (no-op until the `.ico` exists, so the build never
  blocks on art). Shows in taskbar / Explorer / shortcut. The borderless
  window's own taskbar/alt-tab icon may still need a runtime
  `with_window_icon` if it shows a default — not yet wired.
- **Portable assets**: `ewo-render` `text.rs::workspace_assets_dir` resolves
  `assets/fonts` **next to the executable first**, falling back to the
  compile-time workspace path for `cargo run`. Fonts are the only
  runtime-loaded asset (shaders are built-in SkSL).
- **`package.ps1`** (repo root): release-builds + stages a self-contained
  `dist/EwoClient/` (EwoClient.exe + assets/fonts + icon), then points the
  Desktop "EwoClient" shortcut at it. `dist/` is gitignored. Run it after code
  changes to refresh the bundle the shortcut launches; `cargo run` is the dev
  path (uses the in-repo assets).

### Launcher visual polish (same session)

- **Hover glow** — `text::draw_glow_str` / `draw_tracked_glow` (the hero
  title's two-halo treatment, gated by an intensity) now lights up: main-menu
  items, the top tab bar (hovered tab), the Settings sidebar tabs, instance
  list rows, and the "‹ Main menu" back-link — all on hover, with a pearl /
  warm-white brighten.
- **Tofu fixes** — the menu caret, server-widget "JOIN", Instances back/sort,
  and Settings back-link used arrow glyphs the serif fonts lack (rendered as
  boxes). Replaced with vector chevrons (`draw_chevron_right`) / en-dashes.
- **Dropdown** — flip-up is now bias-toward-down (only flips when < ~1.5 rows
  fit below); the "square corner" artifact was finally diagnosed by rendering
  the dropdown to a PNG (`crates/ewo-render/examples/dropdown_shot.rs`) — it
  was the drop shadow pooling below the rounded corners, fixed with a faint
  symmetric halo + opaque body. **That render harness is the tool for
  verifying visual changes without a full launcher run.**
- **Sliders** glow/grow the handle on hover; the **scrollbar** moved into the
  panel's right gutter (was overlapping widgets); the **main menu** dropped
  the SETTINGS link + footer + disturb-hint texts and gained a fade+slide
  entrance; **Friends** heading/subtitle overlap fixed; the back-link
  navigates to MainMenu.

---

## Memory + performance pass (2026-05-31)

A dedicated pass to kill the focused-RSS leak and cut per-frame GPU cost.
Verified live: RSS now **stable at ~108 MB** (was climbing ~2.6 GB/hour while
focused + idle), and the app runs nicely at the 500fps target.

### The focused-idle leak — root-caused + fixed

The H5-session leak (RSS → ~20.9 GB over 8h *while focused*; foreign
C++/FreeType memory not in any Skia cache) was two per-frame foreign
allocations:

1. **`backdrop/velvet_folds.rs` rebuilt a Perlin-noise filter chain every
   frame** — `shaders::fractal_noise` + `image_filters::shader` +
   `displacement_map` + `blur`, all *logically static* (fixed seed/freq/octaves;
   the module comment already said so), constructed fresh ~500×/sec. Each
   `SkPerlinNoiseShader` carries precomputed noise tables in plain C++ heap,
   invisible to Skia's resource/font caches — the exact "foreign, untracked,
   leaks while idle" profile. Runs on **every** screen (the backdrop draws
   unconditionally), which is why it leaked at idle. **Fix:** build the chain
   once into a `thread_local`, clone the refcounted handle per frame. This was
   the dominant leak.
2. **`screens/main_menu.rs::newsreader_italic_axes` called
   `clone_with_arguments` per frame** (main menu only) — the same
   variable-font-clone hazard `text.rs`'s `fraunces_cache` already documents
   (it measured ≈4 MB/s @ 500fps). **Fix:** added
   `FontStore::newsreader_italic_axes` with a quantized typeface cache mirroring
   `fraunces_cache`.

Both fixes are **zero pixel change**. The load-bearing principle: *never
construct a Skia shader / image-filter / variable-font `Typeface` inside a
per-frame draw — they allocate foreign C++/FreeType state that Skia's tracked
caches don't bound. Build once, clone the handle.* The constant-sigma blur
filters in `caustics.rs` / `bokeh.rs` were hoisted the same way.

### The backdrop render graph now has a "slow clock"

`backdrop::Backdrop` caches the four **slow** layers — wine → velvet folds →
caustics → bokeh, i.e. the three full-screen Gaussian blurs (σ 20/15/20) + the
fractal-noise displacement, by far the heaviest GPU work — into an offscreen
surface refreshed at **`CACHE_REFRESH_HZ` (20 Hz)**, blitted 1:1 every frame.
The **fast** layers (pearl dust, petals) + the cheap vignette draw live on top
each frame. Heavy blur work now runs ~20×/sec instead of ~500×/sec (~25× less
per-frame backdrop cost) with no perceptible change — those layers drift on
8–60 s periods. Mechanism: `canvas.new_surface(...)` → `image_snapshot()` →
`draw_image`; the cache is invalidated on resize. Same "cache the slow clock"
trick the in-game HUD frost uses (`ewo-jni::refresh_frost`).

Layering is exact: wine fills the offscreen opaquely, folds/caustics/bokeh
screen-blend on top inside it, the opaque result blits over the black card body
under the same rrect clip → identical pixels, rounded corners and all. The
offscreen has no MSAA (soft gradient content doesn't need it).

### Other wins

- **Pearl-dust halos batched** (`backdrop/pearl_dust.rs`) — the 110 airborne
  motes each allocated a fresh 3-stop radial-gradient shader per frame (which
  also churned Skia's gradient-LUT cache). Now the halo is **baked once into a
  64px GPU sprite** at reference alpha 1.0 and stamped per mote with a reused
  `set_alpha_f` paint — mathematically identical pixels, zero per-frame shader
  allocation. Cores + settled motes stay as solid `draw_circle` (no alloc).
  (Counts are 110 airborne + 80 settled, above the docs' 90/60.)
- **Inner berry glow baked** (`app_window.rs`) — was a σ40 mask-blur of an
  unchanging stroke every frame; now baked once per window size into a cached
  image (`GLOW_CACHE` thread_local) and blitted.
- **Idle frame throttle** (`main.rs::about_to_wait`) — after
  `IDLE_THRESHOLD_SECS` (0.5 s) of no input the focused loop drops to `IDLE_FPS`
  (120) **even with vsync on**; any cursor/key/scroll resets `last_activity` and
  snaps back to full rate; never throttles during the Launching screen. Cuts
  idle GPU/heat/power without touching the interactive 500fps target. (This
  softens the "always 500fps" line, but only when the launcher is untouched.
  `request_redraw` only marks the window dirty; `ControlFlow` decides the wake
  cadence, so the WaitUntil caps the rate even with a redraw pending — the
  existing `max_fps` cap relied on this too.)

### Still open

- **`LEAK_HUNT_INSTRUMENT` diagnostics are still in** (the counting global
  allocator + periodic mem/cache logs + Skia cache caps) — deliberately kept
  through the verification of this fix. Now that RSS is confirmed flat they can
  be stripped per the markers (`git grep LEAK_HUNT_INSTRUMENT`).

---

*Last meaningful structural change to this file (2026-05-26 session):
**Post-ban refactor — the legit/pvp split.** An anticheat ban on CatPvP
landed with the macros switched off, pointing at class-name
fingerprinting as the surface. The catalog now ships in two halves:
12 legit modules in the default build (zero packet synthesis), and 14
assist modules behind `--features pvp` / `build.ps1 -Pvp` (the
packet-touching helpers — Auto Tool, Auto Totem, …, Swing Cadence).
Java assist sources live in `dev.lewlone.ewohud.assist.*` and are
filtered out of the legit jar entirely so their class names never enter
the runtime. Deleted outright: `auto_crit`, `mace_combo`,
`wind_charge_mlg`. Renamed + humanized: `triggerbot` →
`swing_cadence` with min-interval-cap + jitter + reaction-delay knobs.
The previous Phase G "legit-client features only" rule is now
explicitly the rule for the **default build**; the assist set is the
opt-in for semi-anarchy use. Build + Rust tests pass in both
configurations; in-game smoke-test of both builds is the open
verification step.*

*Update (2026-05-30 session): two parts. **(1) Reconciliation** — documented
Phase H (Social) + the two undocumented in-game features (custom crosshair,
SMTC media controller) after finding ~8.4k lines of working-but-uncommitted
launcher work plus a fully-built-and-committed bot side; checkpointed it
(commit `61855ed`) and committed the ChickenLink `/launcher-link` plugin
(`14326ba`). The bot is **live + wire-verified** (`/bot/api/links/by-uuid`,
`/launcher/link`, `/friends`, auth middleware all respond to contract).
**(2) H6 built** — Roblox-style join: `--quickPlayMultiplayer` launch
plumbing via a shared `start_launch` helper, `in_game` presence, a 15s
server-status poller, the main-menu network widget, and a friend "Join"
button; the bot's `GET /api/server-status` was made public (needs redeploy).
Both legit and `--features pvp` compile and `ewo-core` tests pass. Open:
visual eyeball of the main-menu widget, the in-game FRIENDS tab, H7
WebSocket, and the live in-game `/launcher-link` → presence → join test.*

*Update (2026-05-31 session): in-game **FRIENDS overlay tab** (read-only,
file-bridge) + a long **launcher polish + window** pass — see the new
"**Launcher window: transparent floating card**" section above (it supersedes
the Step 1/2 app-window chrome notes). Headlines: the window is now a
transparent per-pixel-alpha **floating 22px card that fills the window**
(`CARD_INSET = 0`, no bevel, no outer shadow, `DWMWCP_DONOTROUND`); the
**freeze-on-exit bug is fixed** (`process::exit(0)` after the loop);
**minimize/close buttons** added top-right; **release builds are GUI apps**
with an **embedded app icon** (`assets/icon.ico` via `build.rs`/`winresource`)
and a **portable `package.ps1` → `dist/EwoClient/` bundle** (fonts resolve
next to the exe). Plus broad visual polish (hover glow everywhere, tofu-arrow
→ vector-chevron fixes, dropdown corner fix diagnosed via a new
PNG-render harness `examples/dropdown_shot.rs`, slider/scrollbar/main-menu
tweaks). All committed; verified live except the open Phase H items above.*

*Update (2026-05-31 session, perf): **Memory + performance pass** — see the new
"**Memory + performance pass**" section above. Root-caused + fixed the
focused-idle RSS leak (per-frame `fractal_noise` filter chain in `velvet_folds`
+ an uncached `newsreader_italic` variable-font clone on the main menu — both
foreign C++/FreeType churn, both zero-pixel fixes), gave the backdrop a 20 Hz
"slow clock" offscreen cache for the three full-screen blurs, baked the
pearl-dust halos to a sprite + the inner glow to a cached image, and added an
idle frame throttle (120fps after 0.5s untouched, full rate on interaction).
Verified live: **RSS stable ~108 MB** (was climbing ~2.6 GB/hr), renders
identically. `LEAK_HUNT_INSTRUMENT` diagnostics intentionally left in pending a
final strip.*

*Update (2026-07-22 session, all Rewo — one long session, 13 commits): **M7
online-mode** (login encryption + signed chat, verified on the user's real
account against an enforce-secure-profile server), **M7c real player skins**
(slim + wide, live-fetched into a runtime atlas pool), **metadata mob detail**
(slime/magma size + baby, both at SynchedEntityData index 16, polymorphic by
serializer type), and the whole **M9 native CEM stack** (the EMF/ETF-
equivalent): Rewo now loads an OptiFine resource pack and renders both the
custom mob **models** and their **animations** — verified end-to-end on the
user's real Fresh Animations pack, all body plans walking, no mod loader. The
two load-bearing CEM conventions (top-level translate = pivot; invertAxis:"xy"
= a 180° Z-rotation → negate the animation's X/Y) are documented in the Rewo
section below and REWO_PLAN §15. Everything is in the `rewo-*` crates and the
launcher `Native` arm; none of it touches the `ewo-jni`/mixin/launcher-GUI
machinery.*

*Update (2026-07-23 session, Rewo): **M9d — CEM polish, the Fresh Animations
detail rig.** Closed the M9c "polish left" list (foot-submodel pivots,
per-face `uvNorth`, scale channels), which all shared one root cause: the
parser only made top-level parts bones, so a FA rig's nested detail (head,
eyes, feet) was flattened onto its parent and its animation channels skipped.
Fix = a bone per `.jem` node. Three verified pieces (headless via
`mobshot --pack` vs the real FA creeper/pig/zombie; no-pack gate stays
243/243; 26 rewo-gpu tests): **per-face UVs** (`cube_f_faceuv` — the FA
eyes/snout, previously box-UV garbage), **scale channels** (+ bone-channel
reads + file-order via a new `indexmap` dep — serde_json's sorted `Map` broke
FA's mirror expressions), and **submodels-as-bones** (head-look, eye blink,
foot articulation now animate; box rest positions unchanged). Two more
load-bearing asymmetries surfaced, both empirical-from-FA + vanilla-verified:
a submodel's pivot is its accumulated *position* (`to_model(boxOff)`, e.g.
creeper head2 → the neck), not −translate; and OptiFine translation
**replaces** a bone's translate (subtract a per-bone rest baseline, else the
pig head flings ~12u off the body — baseline is `invertAxis(pivot)` for
top-level, own translate for submodel). Open: ETF random/emissive textures
(M9b). Detail in REWO_PLAN §15 (M9d entry).*


*Update (2026-07-27 session, all Rewo): **M32b, M33 and M33b — the portal pixel
oracle, then weather and clouds.** `portalshot` closed M32's recorded read-back
gap (uniform textures collapse the fifteen layer matrices, so the frame is a
number the CPU can compute; one layer then isolates one sample and makes the
column-major reading observable). Then rain, snow and a cloud deck, wired into
`rewo live`. Three separate corrections came out of it, each caught by something
different: a **gate witness** caught a cloud front-face convention that looked
right from below alone; the **first live frame** caught that vanilla's weather
and cloud geometry is camera-relative while Rewo's `view_proj` already carries
the camera (the gate had rendered at the origin, where the two coincide — it now
renders 2,500 blocks out); and **eyeballing the sky** caught that the rainy sky
greys through `WeatherAttributes`, an environment-attribute layer system, not
through the `applyWeatherDarken` formula M33 had transcribed — which also
corrected two earlier claims of mine, that rain does not darken the lightmap and
that stars merely dim. The rain fog ramp needed a second, *environmental* fog
band in the world pass, pinned by four mutation-verified pixel witnesses in
`lightmapshot`. **561 tests**, fourteen gates green, demo PNG byte-identical to
M15 onward. All of M10–M33b is now **pushed** to
`codex/rewo-m19-combat-swings`; **merged to `origin/main` on 2026-07-27** as a
clean fast-forward, closing the long-standing unmerged-branch risk.*

*Update (2026-08-02 session, docs): **caught this file up from M73 to M86** —
twenty milestones (M54–M60, M74–M86) that shipped without a CLAUDE.md pass, now
one grouped section at the end of the Rewo part rather than twenty essays. The
session opened by verifying rather than reading: build clean, **1623 tests / 0
failures**, `mobshot` 246/246, `HEAD == origin/main == 0ddbc66`, no branch
holding an unmerged commit, and the fourteen dirty agent worktrees confirmed to
be litter (every file already on `main`). **The numbers in the docs were exact;
two of the plans were stale** — `REWO_PLAN.md` §0.0 still said "M0–M57 at
`aadd8e9`" and still offered the health-bar render half and the bundle grid's
chrome as the cheapest pickups, both shipped (M59, M58). §0.0's "Where it is"
and "What to do next" are rewritten, and its second, 2026-07-27 "What to do
next" is now explicitly marked HISTORICAL, because it still recommends worn
armour (shipped M46–M50) and a fresh session reading top-down could act on it.
**The headline state change: `REWO_PACKET_COVERAGE.md` is at 107 / 0 / 34 with
classes A and B both empty** — every packet Rewo can render is rendered, so the
next unit of work is a **subsystem**, not a packet.*

*Update (2026-08-07 session, Rewo): **M105–M107 — closing the recipe book.**
The four items the M104 handoff listed, plus two bugs found on the way. **M105**
the page counter (`gui.recipebook.page` is `%s/%s`, no spaces; only the FIRST
argument is converted to 1-based; the five-argument `graphics.text` delegates
with `dropShadow = true`; a one-page book shows no counter at all). **M106a**
the recipe cell's tooltip, whose extra line is `"Right Click for More"` and
carries **no count** — the handoff called it "the +N more recipes line", which
it is not — and which **loses to the menu's own tooltip even though vanilla
calls it afterwards**, because `setTooltipForNextFrameInternal` is
`if (deferredTooltip == null || replaceExisting)` with `replaceExisting` false
on every path: the FIRST tooltip of a frame wins. **M106b** the menu
displacement in the last two consumers that never learnt about it — the hover
highlight and the item tooltip both converted the cursor against a panel 77 GUI
px from the one they were drawn against, M89's "a per-call-site choice is how
they come to disagree" failing a third time in one file; the fix is one
`book_open` binding that five consumers read, and the highlight's derivation
turned out to have had **no witness of any kind** because every `set_container`
call in the app passes `hovered: None`. **M106c** the ghost's tooltip, the one
place first-wins is observable (a ghost sits ON a menu slot, so a filled slot
asks both producers at once and the real item wins). **M107**
`tryPlaceRecipe`'s guard — both halves load-bearing, since "not twice" breaks
bulk crafting and "not uncraftable" breaks the click that fills the ghost —
plus `useMaxItems`, and the finding that
`FurnaceRecipeBookComponent.isCraftingSlot` switches on the slot's **container**
index and never asks which container, so **three of the player's hotbar slots
are "crafting slots" of a furnace**. Four witnesses were wrong before any code
was; three composition roots (`apply_screen`, the winit handler) still have no
test seam and their mutations are named rather than hidden. `live
--render-check` gained **r25** and therefore a **second caller requirement** —
the staged invocation now needs `recipe give @s *`, verified by meeting a fresh
player after a reused server directory made the first attempt look green.*

*Update (2026-08-07 session, Rewo): **M104 — the which-of-these overlay**, which
finishes the recipe book bar four small items, plus a **doc-staleness pass**.
The milestone's own entry is in the Rewo section below; the docs half is the
part worth flagging here, because it is the second time in six days that the
prose around a machine-checked number was the thing that lied.
`REWO_PACKET_COVERAGE.md`'s §2 table is verified by a unit test in `ids.rs` and
was exact at **114 / 0 / 27**, while its §0 handoff prose still said 107 / 34,
its class table's caption said "the 32 gaps" over rows summing to 27, and its
class-C definition still listed "a recipe book" as a subsystem Rewo lacks —
three milestones after M93y decoded it. All corrected. `REWO_PLAN.md` had two
"What to do next" blocks in flat contradiction (the older one telling the
reader to prefer it) and a section headed "The current numbers" containing only
historical ones; both are now labelled by precedence rather than by date.
CLAUDE.md's own top-level Rewo status block was three milestones behind at M93 /
1860 tests / `containershot` 49-of-49. **The rule this keeps re-teaching: a
number with a test behind it stays true and the sentence next to it does not.**
Repo hygiene: the twenty stale `claude/rewo-m93*` branches and the two leftover
agent worktrees were verified fully merged and clean, then pruned.*
*Update (2026-08-10 session, Rewo): **the audio plan, and the first two items
of it.** [`REWO_AUDIO_PLAN.md`](REWO_AUDIO_PLAN.md) is the decision §0.0 had been
asking for: cpal + symphonia in a new `rewo-audio` crate, a pure caller-driven
`Mixer::render` that never names cpal, four shippable steps M138a–d, then a
loopback oracle and breadth. It records why kira and rodio lose — **kira
interpolates attenuation in DECIBELS where Minecraft's curve is
amplitude-linear**, a 24 dB error at half the radius, and rodio's specified voice
cannot play a stereo source while all the music is stereo — and it carries three
corrections to its own winning design. Its most important section says what the
gate does NOT assert: no gate opens a device, so a client that mixes perfectly
into a stream nobody opened passes every witness, which is why the milestone
requires an owned human listening pass. **M138a shipped in full.** `Entity.DATA_SILENT` is metadata index 4,
parsed and discarded since M1 while `entity_silent` answered a hardcoded
`false` — and the witness that matters is the one asserting the sound world
READS it, since every other test passes against a decode stored where nothing
looks. `build_sounds` now fails closed under `--render-check` rather than turning
a missing `sounds.json` into an empty index, which is indistinguishable from
totally broken resolution and green because it asserts nothing. The battery also
found that **both mutation harnesses proved "nothing left on disk" with `git diff
--quiet`, which cannot tell a leftover mutation from uncommitted work**; they
compare file bytes now. The listener seam then closed the gap the survey called
structural — nothing in Rewo carried a listener, so every sound was panned
against ears at the origin facing -Z. **`listener_basis`'s forward vector turns
out to be exactly `Entity.calculateViewVector`**, reached by a different route,
which is what makes the transcription checkable instead of self-consistent; and
**`ListenerTransform::INITIAL` is not a camera at yaw 0** — `setRotation` opens
with a half turn, so yaw 0 faces +Z while the record's default faces -Z, and a
test asserting they agree looks obviously right and puts the ears backwards.
**r45 asserts `pushes == frames` rather than `> 0`**, because a per-tick client
would still be non-zero; verified by deleting the call site, which drops it to
0 of 5783 while every unit test stays green. **M138b then started the audio crate**
— `rewo-audio`, with no dependencies, holding the two pieces that have an exact
vanilla answer: the `32767.5 / -0.5 / truncate-toward-zero` quantisation (whose
every failure mode is inaudible rather than obvious — a floor puts a DC offset on
every silent sample of every sound) and `SoundBufferLibrary`'s caching, where
**statics are cached permanently, a FAILURE is cached with them**, streams are
never cached, and **the loop flag rides with the stream rather than the channel**,
because `SoundEngine.play` tells a streamed source explicitly not to loop.
Then the decoder: symphonia, ogg and vorbis only. **Rewo cannot grade a Vorbis
decode bit-for-bit** — the format defines the bitstream, not the exact float — so
the witnesses pin measured vectors from three real assets while the audio stays in
the user's install, and two documented claims became measurements: `goat_horn/call3`
is stereo where `call0` is mono, and the store is mixed-rate. **The battery caught a
witness blind to its own subject** — dropping the `-0.5` bias moves a real sound's
PEAK by one and its SUM by 812, so the assertion moved to the sum. The other
survivor was proven genuinely equivalent: symphonia's probe rejects every truncated
ogg before the `channels == 0` guard can be reached, so it is recorded as an
expected survivor rather than left looking untested. **M138c then shipped the
mixer** — caller-driven like `alcRenderSamplesSOFT`, with the module doc splitting
transcription (the OpenAL attenuation curve, exact) from **stated approximation**
(the pan law and the resampler, which live in a DLL Rewo cannot read) and HRTF
absent entirely. Its finding is what the listener's up vector is FOR: `right =
forward x up` is `(-cos yaw, 0, -sin yaw)` at every pitch, so almost every way of
breaking the basis is invisible — except pinning up to `(0,1,0)`, which at pitch 90
makes `forward x up` the **zero vector** and collapses the stereo image to centre.
The battery also caught two weak fixtures: a rate witness whose sources outlasted
its render window measured the window, and witnesses rendering into freshly-zeroed
buffers cannot see a missing clear. **M138d then shipped its testable half** — the SPSC command ring, whose discipline
reads backwards on purpose: a full ring drops the NEWEST command and never blocks,
because a full ring means the callback has stopped, and blocking the render thread
on a dead device trades a silent sound for a frozen client. **The cpal binding is
deliberately NOT shipped** — it is the first thing in Rewo no gate can check, and
it ends with the listening pass a machine cannot perform. Its battery also fired
both harness hazards this file records at once: a mutant that hangs took the
battery down so its `finally` never ran and **left the mutation on disk**, and the
hung test binary then held the link output so the next build failed with linker
error 1104 and looked like a broken tree. Both are fixed in the harness — a
per-run timeout makes a hang a KILL rather than an outage, and a reaper clears
strays. **M138 then completed**: cpal, the mixer's `ChannelCall` interpreter, and
`examples/listen.rs` — the first code in Rewo that can make a noise, and the first
that **no gate checks or can check**, since an absent, muted, exclusive-mode or
unplugged device all look identical from inside the process. Two containments hold:
**cpal is not wired into `rewo-app`**, so the binary and all 34 gates do not link
an audio stack, and **no test opens a device**, so `cargo test` stays silent —
`cargo run -p rewo-audio --example listen` is the only path to a sound, and the
listening pass is the user's. A voice is created SILENT and waits for its `Play`
(vanilla's order is properties, attach, play, and `alSourcePlay` before an attach
is a no-op), and `AttachStaticBuffer` is counted-and-ignored at the callback
because resolving an asset key is a syscall plus a large allocation. **One stated
deviation**: `retire_finished` runs in the callback, so a finished voice's last
`Arc<Pcm>` deallocates there, which can drop out as a sound ends. **M140 then gave
`level_event_sounds` its first production caller** — the 83-id table has been
complete and partition-tested since M66 and nothing called it, so a dispenser, an
anvil and a wither breaking a block were silent regardless of the device. The
position is the block **CENTRE** (`Level.java:475` delegates to `pos.getX() + 0.5`
on all three axes), volume is carried and spans 200x, and a `data` gate outside its
branches is silence rather than a fall-through. A structural fact found while
writing the witness: **all three global rows are camera-placed**, so that path
never fires for a global packet — pinned over the whole table. And one of my own
assertions was a tautology (`assert!(… || true)`) that also contradicted a test two
functions down. **M140b then took `MusicManager`'s gain ramp**, whose finding is
that `calculateVolume`'s third factor `gainBySource` has **exactly one writer in
the entire client** — the music crossfade, not the options sliders, which arrive
one factor earlier. Its two branches have different shapes: fading up **the step
IS the current gain** clamped to `[0.0005, 0.005]` (so a rise from silence
accelerates, and a constant step agrees only above the clamp), while fading down
is an exponential blend; and the floor **stops** the track rather than clamping,
because a silent track would hold one of only two to eight streaming channels. A
surviving mutation was **proven equivalent** — the down branch's second disjunct
cannot fire, since the blend never crosses its target — so it is dead code in
vanilla too, kept and recorded.*

*Update (2026-08-12 session, Rewo): **M143 — `rewo live --audio`, and the one
method that turns every sound into a click.** M138 built an audio stack and left
it unreachable on purpose; this wires it in, behind a `rewo-app` feature that is
**off by default** so a default build still links neither cpal nor symphonia
(verified with `cargo tree`, not asserted) and the 34 gates are unchanged.
**`LiveSounds.device`'s own doc invited a device milestone to swap the field,
and that is wrong twice over.** The channel pools and the listener record are
`Library`'s bookkeeping — identical behind any device — so `SilentDevice` keeps
them, every witness reading it keeps working (r45 among them), and a backend
implements a three-method `ChannelSink` instead. And `SilentDevice::stopped`
answers `true` unconditionally, which is right for something that makes no noise
and catastrophic for something that does: `schedule_tick` turns a `true`
**straight** into `device.release(channel)` on the next tick —
`MIN_SOURCE_LIFETIME` gates the *instance* reclaim, not the release — and
vanilla's `release` destroys the source, so inheriting it makes **every sound a
50 ms click**, with correct-looking code and a green suite. `stopped()` is
therefore modelled from the buffer's own length on the producer side rather than
asked of the mixer: the truthful alternative is a flag the callback publishes
back, which would move the one method that decides whether the client plays
sounds or clicks into the region **no gate can reach**. Four cases sit before
the arithmetic and each inverts if guessed — a looping source never stops,
acquired-but-unplayed and played-with-nothing-attached are both `AL_INITIAL`,
and a failed attach is the module's one judgement (vanilla leaks that channel
forever, which on a partial asset store means the 26th missing sound exhausts
the pool and the client goes **permanently** silent). **r46 was deliberately not
added** — it needs a device, so it can only self-skip on the machine where it
matters, which is the trap `REWO_AUDIO_PLAN` §5 names; what shipped instead is
r46's claim with the device removed, a test driving a decoded packet through the
real engine, tee, sink, ring and mixer to non-zero samples, with exact silence
asserted first. **Both mutation survivors were weak fixtures and both were
hidden by the ordinary call sequence**: the attach's `AL_INITIAL` reset is
overwritten a moment later by the `Play` that always follows it, and the
declined-stream witness never asked `stopped()` — so a declined stream held its
channel for the session, and the streaming pool is **five**. Streams are
declined and counted, so **music needs the streaming path as well as its
selection logic**, which M140's open half did not say — **and it is not only
music**: measured against the real `sounds.json`, 344 of 8,024 variants are
streamed and **six of them are ambient loops** (the five Nether beds and
`ambient.underwater.loop`), which M142's handlers resolve and M143 drops. A
full decode is not the escape either — `music.end` is 806 s, i.e. **142 MB in
one PCM buffer**. 3088 tests, 34 gates
green, demo PNG byte-identical. **Nobody has listened yet, and no number above
is that claim.***

*Update (2026-08-10 session, Rewo): **M136 and M137 — two fixes recovered from
worktrees that a handoff called litter.** The claim rested on their branches being
0 commits off `main`, which was true; **`git branch --merged` says nothing about a
dirty working tree**, and two of the five still held uncommitted work. **M136**: a
spectator's tab-list name is `-1862270977` = `0x90FFFFFF`, **white at alpha 144**,
where M52f wrote a grey `0x9099_9999` — and its doc comment restated the same
wrong value, so code and prose agreed with each other and neither agreed with
vanilla. Nothing consumed the constant (the tab list is still model-only), which
is exactly why it survived — the same shape as M135 the same day. **M137**: a
mutation rendering a styled run style-blind survived `deathshot`'s m20, and it was
a weak fixture, not an equivalent mutant — **nothing follows a last span**, so a
styled span placed last has an advance that moves nothing, and bold is charged per
character. The styled span goes first now, and the identical hole turned out to
exist at a different call site in `titleshot`, found by asking where else the shape
could occur rather than by a mutation. Plus a unit test for the one label a pixel
gate can never grade: every label on the pause/disconnect/dialog screens is white,
and white is 1.0 in both colour spaces, so only an INACTIVE button (`0xA0A0A0`)
can show a colour-space error — and none of those screens builds one. 2853 tests,
34 gates, demo PNG byte-identical.*

*Update (2026-08-10 session, Rewo): **M135 — the chat fills were drawn off the
bottom of every screen**, a real shipping bug rather than a feature. `HudFill` is
in GUI pixels and the pass multiplies by the GUI scale; four producers multiplied
by it first, so the chat rows' backdrops, the input bar, the scrollbar and the
suggestion popup have been **absent rather than misplaced** since M109 — no
artefact to report, which is how they lasted eight milestones. **The contract was
documented correctly in two places and it did not help**: the producers were
written against the function beside each of them, and `OwnedTextLine` takes
SCREEN pixels. Two passes, two conventions, one file. **The fix is not a deleted
multiply** — a chat pixel really is `opts.scale` GUI pixels, and dropping both
factors is a third wrong answer that no `scale == 1` fixture can tell from the
right one, which is exactly why every existing fixture was blind (all of them
pass `px = 1.0`). The generalisable finding: an agreement witness already
compared `hud_fills` against `chat_lines` and **passed at every scale with the
bug in place**, because both sides were in the same wrong space — **an agreement
witness has to model whatever sits between the producer and the screen.** One of
the new witnesses was wrong before the code was (the fourth such instance):
vanilla's own chat backdrop is `maxWidth + 12` wide against a screen-independent
320, so on a 320-GUI-px window it really does run past the right edge. Hardening:
`rewo_gpu::hud::gui_scale` already existed, its doc already warned about
recomputing it, and **two of the three sites did not call it**; they do now, and
the two producers whose inputs were already GUI pixels lost their `px` parameter
outright. Also shipped `tools/render_check.py`, so the one gate needing a server
is one command with every recorded trap turned into an assertion. 2851 tests, 34
gates, `--render-check` 44/44 twice, demo PNG byte-identical, 9/9 mutations.*

*Update (2026-08-09 session, Rewo): **the M127–M134 integration** — eight
milestones that had been built in parallel on six branches off the M126 merge,
none of them merged, integrated in one pass. M127 the chat decoration
(`boundChatType.decorate`, so a message renders as its chat type formats it),
M128 clickable chat, M129 the disconnect reason, M130 the linear-colour
correction (**the text pass wants LINEAR and nine of twenty-two callers were
handing it the sRGB byte**) plus the title's and death screen's style flags,
M131 the sound-instance model and a device seam, M132 the scoreboard sidebar,
M133 the recipe book's widget tooltips, M134 the command line's exception
messages. Merge order was chosen so the trunk landed first and M130's
`TextLine::color` → `color_linear` rename hit a settled tree once.
**The integration's own findings are the part worth keeping.** A witness-number
collision — three branches each minting an r42 — that git merges silently,
because `--render-check` ends `pass == rows.len()` with no declared count and
no uniqueness check. Two breaks invisible to a textual merge: `ChatStyle` lost
`Copy` (M128 put an `Arc` on it) and broke five by-value uses written against
the `Copy` version, and the `color_linear` rename E0560'd every literal on the
other branches — both fail loud, which is the good outcome. The dangerous one
was the `usage_box` conflict, where **one side compiled and silently reverted
M134b**. And a real regression no branch's own gate could see: r42's click was
being eaten by the suggestion popup, because M128 branched before M127c added
its decoration witnesses to the same injection block and the clickable row —
the newest message — ended up drawn under the popup. **A branch being green is
not evidence about the merged tree.** 2846 tests, 34 gates,
`--render-check` 44/44, demo PNG byte-identical.*

*Update (2026-08-08 session, Rewo): **M126 — the styled chat pipeline**, which
§0.0 recommended taking before the chat decoration precisely so the decoration
could ship complete. `GuiMessage::content` was a `String`, so the chat store
flattened whatever it was handed; it is a span list now, `StringSplitter` walks
the **part list** vanilla always had (`FlatComponents` / `splitAt` /
`ComponentCollector`, all of which degenerated to a substring while there was
only ever one part), and `TextPass` draws all five `Style` flags. The types had
to move down a crate first — `rewo_world::chat` must name `ChatSpan` and the
dependency runs net → world — which is proved pure by a **conservation** rather
than a reading: world +58, net −58, app unchanged. Findings: **the width
provider takes a style**, because `getBoldOffset()` is 1.0 charged PER
CHARACTER, so a style-blind measure wraps a bold line late rather than merely
drawing it differently; **`position` restarts at 0 for every part**
(`fromList` chains `accept` without renumbering), so an underline's one-pixel
lead-in belongs to each span and a multi-colour underlined line overlaps by a
pixel; and the **deleted marker is GRAY + ITALIC**, drawn plain white until now
because the store could not hold a style. Obfuscation transcribes the
same-width bucket, the unstyled advance and the never-a-space rule, and
diverges deliberately on the source — vanilla's is nanotime-seeded and
reproduces nothing, so Rewo uses a frame-seeded SplitMix64 whose `run_seed`
reads the **unoffset** origin, or the drop shadow would shadow different
characters. **The mutation battery caught a witness lying** (r38 counted
colours across the whole chat box, which a flattening client satisfies; it
counts within one row now) and left one survivor that is neither equivalent nor
a weak fixture: `splitAt`'s `position > contentsSize` read as `>=` genuinely
diverges, and is invisible only because production always pairs it with
`getSplitStyle()` — the test proves agreement under that pairing and divergence
under any other. 2615 tests, 33 gates, `--render-check` 39/39, demo PNG
byte-identical.*

*Update (2026-08-08 session, Rewo): **M125 — translatable components resolve**,
plus a full docs pass. §0.0 offered the chat decoration and said to verify its
blocker first; both halves were reachable, and the survey found what the
decoration sits on and what is far more visible than it — **every `translate`
component Rewo received rendered as its raw KEY with its arguments dropped**, so
a real server's join messages read `multiplayer.player.joined`, every death
message `death.attack.player`, and every command's feedback
`commands.give.success.single`. Both walkers said so in their own doc comments;
nobody had read them. Its finding is not about chat at all: a live trace showed
`/give` rendering as "Gave  [Diamond Sword]" with the count gone, because **NBT
lists are homogeneous**, so a mixed one is written as compounds with every
non-compound element boxed as `{"": value}` and unwrapped on read — and
**Rewo's reader had never unwrapped, from M1 to M125**. It does not fail; it
yields a plausible wrong tree, which is why 124 milestones missed it. Two
witnesses were also wrong before any code was, which is the fifth and sixth
documented instance: r37's premise (that a server announces a joining player to
that player) is disproved by `PlayerList.placeNewPlayer`, and a surviving
mutation turned out to be a weak fixture whose argument overrode the one field
the test observed. The docs pass fixed a rotting file count in §0.0's gotcha 9
(replaced with the stable claim — **five** files under `crates/` are not pure
LF, named), a broken sentence and a stale end-of-line list in
`AGENT_LOOP_BRIEF.md`, and the M-range in `REWO_FEATURE_SURVEY.md`'s staleness
note.*

---

## Rewo — from-scratch native Minecraft client (online play, native CEM, exact light/colour, dimensions, the combat + block-entity arcs, weather, particles, the first-person hand, the Velvet type stack, the container arc, the recipe book, chat, translated text, styled spans, the chat decoration, clickable text, and the scoreboard sidebar)

**[REWO_PLAN.md](REWO_PLAN.md) is the plan of record — a fresh session must
read its §0.0 HANDOFF first** (it consolidates current state, what to do next,
the headless verification toolkit, the load-bearing gotchas, and a categorized
list of every known issue/gap/deviation, explicitly framed for critique).
**[REWO_AUDIO_PLAN.md](REWO_AUDIO_PLAN.md) is the detail behind its audio
item** — M138a–d, `level_event`'s sounds, the music fade, **M141's ten tickable
ramps** (ten, not the "~8" that plan says), M142's three ambient handlers and
**M143's wire into the client** have all shipped, so `rewo live --audio` on a
build with `--features audio` opens a device and plays what the engine
resolves. **The listening pass is the outstanding work and it is the user's**,
because no gate in this project opens an audio device — an absent, muted,
exclusive-mode or unplugged one all look identical from inside the process, so
everything a machine can check passes and that is *not* the same claim. The
feature is **off by default**, so a default build links no audio stack and the
34 gates are unchanged.
**Everything is shipped, gated and merged to `main`** as of 2026-08-12
(M143) — **3088 tests / 0 failures** (world 1166, net 1098, gpu 275, data 228,
app 199, mesh 45, proto 16, **audio 61** — EIGHT crates now, read off the runner
per crate; a loop written against the old seven drops the new one silently),
`mobshot` 246/246,
`containershot` **107/107**, `inventoryshot` **158/158**, `itemshot` 75/75,
`handshot` 34/34, `swingshot` 97/97, all **34** serverless gates green with 0
validation errors, `live --render-check` **45/45** with validation ON and 0
validation errors, demo PNG `2cc56b4acbfb92cb`.
**The recipe book is closed** (M105–M107) and **M108–M111 shipped chat** —
`ChatComponent`, the wrap under it, the `MessageSignatureCache` without which
`delete_chat` cannot be read, the text, the backdrop fills (which took a colour
channel on the HUD vertex), the **`ChatScreen`** you type into, and its
scrollbar. What is left of chat all needs a subsystem Rewo lacks. **M112** then
closed `isHovering`'s narrow-window override and found the recipe book's 77 px
displacement missing from four more consumers — the click, the double-click, the
drag and the item-hover highlight. **M113** decoded the Brigadier command tree
(2,017 nodes off a real server, consumed exactly), and **M114** built
`CommandSuggestions` on it — the two remaining chat packets, brigadier's own
suggestion primitives (graded against the **real jar**, because brigadier is a
library and absent from the decompile), the popup's model and geometry, and its
place at the head of `ChatScreen`'s key order. Coverage is **118 / 0 / 23**,
class C **12**. **M115** then drew it — the rows, the truncation bars, the
scroll dashes and the greyed ghost suffix — and its `--render-check` witness
r29 is built to measure the **production chain** rather than a hand-built
`Suggestions`: the gate injects a `custom_chat_completions` packet through the
real router and types one character, so a break anywhere from the decode to the
render drops it to zero. Mutation-verified live, twice. **M116** then built
the client-side **Brigadier dispatcher**, so `/g` completes to
`gamemode`/`give` with **no packet at all** where M114 asked the server for
every keystroke — and found that **`canUse` is always true for suggestions**:
`getSuggestionsProvider()` returns the provider granted
`ALLOW_RESTRICTED_COMMANDS` explicitly, so `FLAG_RESTRICTED` governs a
send-confirmation prompt and not what the popup offers (M113's guess that
`hasAllowedInput` reads it is wrong — that reads `ChatAbilities`). **M117**
then built the two things that read the parse: the **syntax highlighting**
(where `LITERAL_STYLE` is GRAY, so a parsed command *dims* and only its
arguments stay bright) and the **usage box**, which grows **upward** from the
bottom and is **mutually exclusive** with the suggestion popup. Its sharpest
finding is that `getSmartUsage` **decides with one string and prints another** —
the `LinkedHashSet` of deep usages only settles whether the alternatives
differ, and the pipe list is then built from `getUsageText`. **M118** added the
**entity selector parser** — `@e[…]` parses and completes locally — whose
mechanism is a **function pointer the parse reassigns**, which is why
suggestions survive a throw: `EntityArgument.listSuggestions` catches the
exception with an **empty body** and calls `fillSuggestions` anyway. Two of its
seven suggestion states are **dead in vanilla**. **M119** added
`block_state` and `item_stack`, and found where the **namespace rule** lives:
`suggestResource`'s `filterResources` tests the typed text against an
identifier's namespace and path **separately** when no colon has been typed,
which is the other half of M114a's refusal to split `matchesSubStr` on `:`.
**M120** then claimed **39 of the remaining 45** argument types — the
coordinate family, the fixed word lists, the ranges, the word-shaped scalars
and the identifier family — leaving **six structured ones** (`component`,
`style`, `nbt_*`, `dialog`) named rather than half-done, with a test asserting
every other type IS claimed so the list cannot rot. Its finding: **a bare `~`
is a complete coordinate** (the number after it is optional), and **`^` is
all-or-nothing across a triple** — `^1 ~2 3` is `ERROR_MIXED_TYPE`, not a
mixed one. **M121** closed the set: **every `minecraft:` argument type now
parses**, with the six structured ones handled as **extents rather than as a
grammar**. That is a stated, test-asserted approximation — 26.x's SNBT is a
916-line packrat grammar, and an approximate one would silently accept text the
server rejects, so Rewo measures where the value *ends* and does not validate
it. It over-accepted `{a:}` — and **M122** closed that by transcribing the
grammar itself, so SNBT is now validated rather than measured and M117's red
unparsed tail appears where vanilla shows one. Its findings are the kind a
plausible parser gets silently wrong: **`0b` is zero-as-a-byte and `0b1` is
binary one** (resolved by backtracking, not lookahead), **a leading zero is an
error rather than a value**, and **`_` is a digit separator banned only at the
ends**. **M123** then closed the range gap M122 had recorded as a
failing-on-purpose test, and it was not a bounds check bolted on: **the BASE
decides the signedness** (binary and hex default to UNSIGNED where decimal
defaults to SIGNED), so `0xFFFFFFFF` is a valid int where `4294967295` is not
and `-0xF` is an error; **`s` is both the signed prefix and the SHORT width**;
an array element may **narrow** its width but not widen it, keeping its own
base, so **`[B;255]` is an error and `[B;0xFF]` is fine**; and a float is
rejected for being **infinite**, not unparseable. **M124** then closed the
literal tables — **eight** argument types, not the seven the plan claimed, and
**not** merely a suggestion gap: `heightmap` is the enum **filtered by
`keepAfterWorldgen`** (four names, not six), `swizzle` has a real parse and
deliberately **no suggester at all**, the two slot types read **to the next
space** so `container.*` survives and differ from each other **in the parse**,
and `time`'s suggester **re-anchors past the number** so its unit completes as
a suffix.

**M125** then took the chat decoration §0.0 offered and found a prerequisite
under it, much more visible than the decoration: **every `translate` component
Rewo received rendered as its raw key with its arguments dropped**, so a real
server's join messages read `multiplayer.player.joined`, every death message
`death.attack.player`, and every command's feedback
`commands.give.success.single`. Both walkers said so in their own doc comments.
It ships `decomposeTemplate` as **parts** (so one `FORMAT_PATTERN` serves the
plain and the styled paths — M100's lesson), the resolution itself, and the
wiring, and its composition rule is the thing worth pinning: `Component.visit`
opens with `getStyle().applyTo(parentStyle)`, so a template's literals take the
translatable's style while a component argument applies its own **on top** — a
resolution that substituted plain strings would paint the whole line one colour
and read perfectly correctly. `getArgument`'s `arg.toString()` needed two
artefacts rather than the decompile: `JavaOps` (read out of the shipped
`datafixerupper-10.0.21.jar`) proves the numeric **width survives**, so an
`IntTag(3)` renders `3` and not `3.0`, and `Double.toString` is graded against
a real JDK 25 by `tools/java_tostring_oracle/` — the M114 precedent — where the
plain-versus-scientific band turns out **inclusive at 1e-3 and exclusive at
1e7**. **Its real finding is not about chat at all**: a live trace showed
`/give` rendering as "Gave  [Diamond Sword]" with the count gone, because **NBT
lists are homogeneous**, so a mixed one is written as a list of compounds with
every non-compound element boxed as `{"": value}` (`ListTag.wrapIfNeeded`) and
unwrapped on read (`addAndUnwrap`) — and **Rewo's reader had never unwrapped,
from M1 to M125**. Nothing caught it because skipping the unwrap does not fail;
it yields a plausible wrong tree, and the first thing in 124 milestones to look
at such an element was a translatable's `with`. Its gate witness r37 was also
**wrong before the code was**, the fifth documented instance: it drove the join
message on the premise that a server announces a joining player to that player,
which `PlayerList.placeNewPlayer` disproves (broadcast at line 202,
`players.add` at line **210**).

No branch or worktree holds a commit
off `main`. The long-unmerged-branch risk closed on 2026-07-27 and has
stayed closed; branch new work from `main` and keep it that way.

> **⚠ §0.0's prose goes stale faster than its numbers.** The 2026-08-02 pass
> found the handoff still claiming M57 at `aadd8e9` and still offering two
> "cheapest things to pick up" that had both shipped (M58, M59), while every
> *measurement* in the same file was exact. `REWO_PACKET_COVERAGE.md` does not
> have this problem because its table is **machine-checked** against `ids.rs`
> by a unit test. Treat §15's log and the coverage table as current; treat any
> forward-looking paragraph as suspect until checked against `git log`.

> **⚠ The M-numbers are not a contiguous index — use commit subjects.**
> Several sessions have run concurrently with parallel agents, so numbers were
> assigned independently and reconciled on merge: `M52` appears on more than
> one piece of work (as does `M61` — the wavy cape and the bundle decoder),
> `M68` also names two (the motion packets and the sheep's undercoat), `M53` is a *specification* rather than code, and the
> ladder jumps to M58/M59. **`REWO_PLAN.md` §0.0 carries the authoritative
> numbering note** — read it rather than inferring order from the numbers.
> When you need to know what actually shipped, read `git log --oneline`
> subjects. Rewo (from
"rewolution", as Ewo came from "ewolution") is a from-scratch Rust Minecraft
client speaking the vanilla protocol (pin: **26.2 / protocol 776**, read from
the bundled jar's version.json), rendered with **raw Vulkan via ash** —
frame-time consistency (1%/0.1% lows) and input latency first. It plugs into
this launcher as a `Native` instance kind reusing auth + spawn + reaper; it
is NOT a JVM/mod project — `ewo-jni`/mixin machinery does not apply.

**[REWO_FEATURE_SURVEY.md](REWO_FEATURE_SURVEY.md) is the feature roadmap** —
what to build *after* the M-series milestones, derived from a survey of all
9,291 open-source client-side Fabric mods on Modrinth (2026-07-26,
regenerate with `python tools/survey_modrinth.py`). Read it when picking the
next feature rather than the next milestone. Three things from it are
load-bearing anywhere in the repo: (1) **the market leader in every big solved
category is non-open-source** — Sodium (Polyform Shield), EntityCulling
(bespoke protective), Xaero's Minimap + JourneyMap (All-Rights-Reserved), Jade
+ WTHIT (CC-BY-NC-SA, NonCommercial); ~509M downloads whose source must not be
read as reference for Rewo, though bundling the jars in EwoLoader is a
separate question; (2) **53.4% of all client-mod download mass** exists only because the
game is a JVM client with a mod loader (modding infrastructure 25.4% +
JVM performance 17.5% + OptiFine-pack parity 10.5%), which is the strongest
external validation Rewo has; (3) the 9,291 mods collapse to **75 distinct
features** — 50 QoL to build, 11 that are one "port the modules + HUD set into
Rewo" milestone, **5 already at vanilla parity** (M40 tooltips, M41 durability
bars, M51 screenshots, the crosshair, the selection outline — audit against the
crates before scheduling anything from this doc), 3 blocked on audio. Counts
carry a measured **~22-25% error**, the list is a **floor**, and the error is
**not uniform** — it tracks keyword distinctiveness, so vague-keyword clusters
(`Reach / hit indicators` 38% wrong) rank too high while distinctive ones
(`Tooltip overhaul` 0%) are clean. Use it as a prioritisation, never as a
citation.

**[REWO_VELVET_UI_PLAN.md](REWO_VELVET_UI_PLAN.md) is the Velvet UI spec** —
the type stack Rewo needs for tooltips, chat and F3, and the record of a
deliberate **visual freeze**. Read its §8/§9 before touching HUD visuals. The
short version: the glyph/text/chrome machinery landed and is keeper work; the
widget transcription **stopped at one widget on purpose**, because EwoClient's
HUD is getting a visual overhaul and anything transcribed now would be redone.
The chrome palette is de-baked into a `ShellStyle` table so a redesign is a
data edit, not a shader edit. §3's colour-space note is the one thing that
survives the overhaul unchanged, because it is a property of the renderer:
**the Velvet passes must be built with `world::unorm_of(target_format)` and
drawn inside `WorldRenderer::with_gamma_space`**, or the pipeline format
mismatches the attachment.

- **M0–M6 all shipped + headlessly verified + pushed (2026-07-21).** It's a
  playable windowed client (`rewo live`) on offline vanilla 26.2 servers:
  connect, walk/dig/place/chat with 0 physics-corrections, full block-model
  rendering, GPU-driven (compute cull + one indirect draw), with a
  deterministic `rewo bench` regression gate. Subcommands: `net` (M1
  protocol), `view` (M2 snapshot), `play` (M3 headless bot), `live` (windowed
  client), `demo` (M4 model showcase), `bench` (M6 render benchmark).
- **Three load-bearing gotchas** (full list in REWO_PLAN §0.0): (1) mesher
  emits WORLD-space vertices — the shader must NOT add a column origin (the
  double-add was the real M4 "far-field holes" bug). (2) Collision uses
  `baked.solid`, NOT `matches!(Cube)` — grass_block renders as a Model, so
  keying off the render fast-path makes the player fall through the ground.
  (3) 26.x model textures can be `{sprite}` objects, not just strings.
- **Biggest open gaps to critique** (REWO_PLAN §0.0, which is authoritative —
  the rest of this bullet is the *historical* record of how the M0–M6-era gaps
  closed, kept because the corrections are instructive). **Current gaps:** no
  **inventory model** (the container packets have zero references in
  `rewo-net`, so the hotbar draws empty slots and the local player holds
  nothing — the named blocker for first-person hand and GUI); no particles; no
  sound; entity collision ignored; the HUD is crosshair/hotbar/hearts/hunger
  only. Everything below this sentence is closed. All three §4 deviations are
  now closed (rayon mesh pool, Native `live` arm, async upload ring). Fixed
  2026-07-21 (several passes): meshing moved OFF the
  main thread (rayon `MeshPool` + `Arc<Column>` CoW snapshots — mesher
  unchanged, demo PNG byte-identical, bench gate green); the launcher
  Native arm now spawns `rewo live` (+ `EWO_DEV_SERVER=host:port` dev-join
  knob, `package.ps1` stages rewo.exe; UI eyeball still pending);
  **entity rendering shipped** — full movement/player-info decode, vanilla
  3-tick lerp, capsule pass + bitmap-font nametags, verified live
  (position cross-check exact, 129-entity soak at ~1,170 fps, "RewoCap2"
  legible in a headless PNG); **fluids + translucent pass shipped** —
  water (corner-height surfaces, texture-alpha blend, CPU-sorted
  back-to-front per column) + opaque fullbright lava, demo-PNG verified,
  view-replay byte-identical + bench green; **texture animation** (water
  ripples/lava churns via .mcmeta-driven 20 Hz layer re-uploads, PNG-diff
  verified); **the player model** — Steve (12 cuboids incl. overlays,
  box-UV from skin.rs, jar default skin, yaw + head pitch + **walk-cycle
  limb swing** derived client-side from motion) replaces the player
  capsule, headless-PNG verified; the **async upload ring** (4-slot,
  no per-frame fence wait — closes the last §4 deviation); and **gradient
  sky + distance fog** (view-ray sky, terrain fades to the horizon color
  so the chunk-boundary edge dissolves — PNG-verified); and the **in-game
  HUD** (crosshair, hotbar + selection, health hearts, hunger drumsticks
  from the jar's gui sprites; live-only so demo/view/bench are unchanged);
  the **slime mob model** (first real mob — the entity pass now has a
  model registry `EntityModelKind {Player, Slime, Capsule}`; slime is the
  vanilla 8³ cube, face/size deferred); and **block targeting** (voxel
  raycast → vanilla-style selection outline + left-click dig / right-click
  place, so `rewo live` can mine and build); and the **zombie mob** (reuses
  the player model geometry with the zombie skin + arms-forward pose; the
  `chat_command` packet + a `REWO_SUMMON` op'd-summon verification knob came
  with it); and the **cow mob** (the quadruped body plan — rotated body box,
  `box_uv_faces` extracted for the per-vertex transform); and **screen-space
  text** (a `TextPass` rendering the vanilla font with drop shadows — a
  coordinates/facing line + a chat overlay, the client's first on-screen
  text); and **entity metadata decode** (`set_entity_data` → a serializer
  skip table → custom nametags on mobs; slime size/baby deferred as
  entity-specific indices); and **quadruped leg animation** (the cow walks
  in vanilla's diagonal gait — `emit_model`'s rotation generalized to a
  `(pivot_y, pivot_z)` pivot for the front/back legs); and **mob head-look**
  (humanoids turn their heads toward nearby players via the `rotate_head`
  packet — `LimbPart::Head` yaws to its own absolute angle; the same change
  caught+fixed a variable-shadowing regression where the head/body-yaw
  binding shadowed `emit_model`'s model-scale `s`, silently scaling every
  mob by `sin(yaw)`); and an **F3 debug overlay** (vanilla-style block — XYZ
  / block+in-chunk / chunk / facing+axis / loaded-chunks+entities, with
  `rem_euclid`/`div_euclid` chunk math; F3 toggles it windowed, always on
  headless); the **pig** (fourth mob model — grew the entity atlas to
  256×256, generalized `quadruped_model_quads` to `(off, leg, snout)` so cow
  legSize=12 and pig legSize=6+snout share it; cow renders unchanged); and
  the **sheep** (fifth mob — own body dims + an inflated white wool overlay
  from `SheepFurModel`; extracted `build_quad_parts` as the shared quadruped
  builder, and replaced `EntityPass::new`'s 8 positional texture params with
  an `EntityTextures` struct). The mob registry spans humanoid/cube/quadruped.
  Also un-broke `rewo view` (stale M2 bake-sanity check). The mob textures
  those passes shipped were UV-scrambled (verified by silhouette+colour only
  — the "verify the property, not a proxy" lesson); **fixed 2026-07-22 by
  the mob redo**: `crates/rewo-gpu/src/mobs.rs` is a verbatim port of
  vanilla `ModelPart.Cube`/`Polygon` + vanilla's exact entity transform
  (the old path was also X-mirrored), all mob meshes re-transcribed from the
  26.2 decompile (the 26.2 cow is its own mesh, not the generic quadruped),
  and the set grew (two more same-day passes) to **88 mobs — every living
  vanilla mob**: full zombie/skeleton/illager/piglin families, witch,
  guardian+elder, shulker, blaze, ghast 4.5× + happy ghast 4.0×,
  silverfish/endermite, phantom, vex, hoglin/zoglin, strider, magma cube,
  all farm + overworld passives (cat/fox/goat/bee/frog/armadillo/axolotl/
  dolphin/turtle/fish×4/panda/polar bear/camel/llama/parrot/horse family/
  bat), snow/iron/copper golem, allay, warden, sniffer (192²), breeze
  (two-texture: body + wind funnel), creaking, ravager, wither, nautilus
  pair, and the **ender dragon** (256², full mesh). Capsules remain only
  for object entities. Atlas 1024² + shelf packer (16²..256²). Verified
  by the serverless **`rewo mobshot --check` facelabel gate**
  (face-colored debug textures vs a perspective ray-cast of the same
  geometry — occlusion-exact; **246/246 mob-views green**, with 6 mobs
  auto-detected as color-check-N/A where vanilla reuses texels across
  faces — run it after any mob/UV change) + `rewo mobshot --out` contact
  sheet + `--only` closeups + live summon shots; demo PNG stayed
  byte-identical, bench flat, 0 VUIDs. **Animations: every procedural
  vanilla `setupAnim` is formula-exact** (spider leg waves, golem
  triangle-wave limbs, blaze rod orbits, ghast/squid tentacles,
  phantom/allay/vex/bee wing flaps, fish tails, wolf tail wag, silverfish
  wiggle, wither side heads) — parts have base rotations + a parent
  hierarchy + pivot-motion anims; `set_entities` takes a time param
  (`ageInTicks` = s·20). **The keyframe rigs run too**: vanilla
  `AnimationDefinition`s machine-extracted by `tools/gen_anim_defs.ps1`
  into generated `anim_defs.rs` (re-run after a version bump) + a
  vanilla-exact evaluator (next-frame interpolation mode, catmullrom,
  additive apply, per-mob `applyWalk` params) — frog/camel/sniffer/
  armadillo/creaking/copper-golem walks, bat flight, breeze idle,
  nautilus swim, rabbit hop. **Gesture rigs run too**: Pose metadata
  (index 6) + sniffer/armadillo state enums (index 17) decoded; a
  `GestureTracker` times rigs from the observed state change;
  `KfGate::{During, Unless, NotShell}` + `KfDriver::GestureAge` + part
  `Show` visibility rules play warden roar/sniff/emerge/dig, frog
  croak/tongue (+throat pouch), breeze shoot/slide/inhale/jump, sniffer
  dig/sniff/happy/rise + the SEARCHING walk-swap, armadillo
  roll/scared/unroll with the shell-ball swap (verify with
  `rewo mobshot --gesture name[,age] [--shell]` or
  `REWO_FORCE_GESTURE`). **M17 fires the exact model-visible entity
  events** — Warden attack/sonic-boom + Armadillo re-peek from the
  `entity_event` packet (see the M17 bullet below). **M18 shipped the Allay
  dance** — `DATA_DANCING` metadata (index 16, BOOLEAN serializer 8), not an
  event (event 18 is heart particles only): exact `Allay.tick()` counters +
  `AllayModel` root/head formulas, gated by `rewo danceshot --check` 24/24
  (see the M18 bullet below). Still open: the Warden tendril (event 61),
  generic `ClientboundAnimate` arm swings, and dragon flight (bespoke
  procedural code, posed).
  [`REWO_MOB_REDO_HANDOFF.md`](REWO_MOB_REDO_HANDOFF.md) is now a completion
  record; details in REWO_PLAN §15 "2026-07-22 — the mob redo shipped".

- **M0 shipped 2026-07-21** (`crates/rewo-gpu` + `crates/rewo-app`, binary
  `rewo`): ash 1.3 device + MAILBOX swapchain + frame-time strip-chart
  overlay + GPU timestamps + tracy. Verified headlessly on the RTX 5080:
  ~4.3k fps clear+overlay, cpu p99 0.87 ms, validation-clean.
- **M1 shipped 2026-07-21** (`crates/rewo-proto` + `rewo-data` + `rewo-world`
  + `rewo-net`): the full vanilla protocol — Handshake→Login(offline)→
  Configuration→Play with zlib compression + the liveness contract, chunk/
  light/entity decode, packet record/replay. Ground truth = the decompiled
  26.2 jar (Vineflower) + Mojang datagen reports under
  `%APPDATA%/EwoClient/rewo/26.2/` (git-ignored, derived from the user's own
  download). **Verified against a live vanilla 26.2 offline flat-world server
  Claude set up + ran headlessly**: 329 chunks decoded with zero failures,
  block queries hit the exact flat-world layers (bedrock/dirt/grass/air), and
  replay reproduced the live world digest bit-for-bit. `rewo net soak` /
  `rewo net replay` are the M1 verification tools. Key wire gotchas captured:
  the paletted long array is **fixed-size, not length-prefixed**; each 16³
  section starts with **two shorts (non-empty + fluid count)**; packet ids
  are resolved **by name** from the datagen report so a version bump fails
  loud instead of misfiring.
- **M2 shipped 2026-07-21** (`crates/rewo-mesh` + `rewo-data::assets` +
  `rewo-gpu::world` + `rewo view`): first pixels — client-jar asset bake
  (cube-family models only; indexed-PNG expand was the gotcha), face-culled
  mesher with per-face shade × server light, texture array + CPU mips +
  depth + frustum cull, snapshot viewer (`rewo view --replay|--host …
  [--out png]`). Verified headlessly: recognizable flat world PNGs from
  both the M1 recording and a live server; windowed fly-cam ~1k fps, p99
  2.16 ms. **Launcher `Native` arm landed too**: `InstanceLoader::Native`,
  "Native · Rewo" in the new-instance modal, `try_real_launch` spawns
  `rewo.exe` (REWO_* env contract; `view --host` args when a server join is
  active), reaper covers rewo.exe. UI eyeball of the modal + Launch flow
  still pending; `package.ps1` doesn't copy rewo.exe into dist yet.
- **M3 shipped 2026-07-21** (`rewo-world::physics` + `rewo-net::play` +
  `rewo-data::items` + `rewo play`): be a player. Faithful vanilla 20 Hz
  physics port from the decompile (walk/sprint/jump speeds unit-locked to
  vanilla), live play session (split socket: reader thread + 20 Hz tick
  loop) with the exact `LocalPlayer.sendPosition` cadence, dig/place/attack/
  chat/hotbar. Verified headlessly vs the live 26.2 server: **0 server
  corrections over 3,000 ticks** of continuous movement, place→dirt &
  dig→air confirmed by world query + block_update echo, chat round-trip.
  `rewo play` is the DoD bot harness. Gotcha fixed: `Column::block_state_at`
  wasn't consulting the `overrides` map that `set_block` writes (block
  edits looked ignored though the server applied them). Not exercised:
  attack-a-mob (no mobs on flat creative). **The live windowed client
  shipped too** (`rewo live`): the protocol+physics session feeds the M2
  renderer in one loop (20 Hz tick accumulator + per-frame dirty-column
  remesh budget + eye camera + WASD/mouse). Headless `--out PNG` renders the
  first-person eye view; windowed soak ~988 fps, 0 corrections. Rewo is now
  a playable windowed client.
- **M4 shipped 2026-07-21** (`rewo-data::assets` model parser + `rewo-mesh`
  AO/model path + `rewo demo`): real meshing. Full block-model resolution
  (variants w/ x/y rotation, multipart w/ when-conditions, elements/faces/
  cullface/element-rotation) → cube fast-path or baked quad list; 2,320
  cubes + 26,555 models. Mesher adds 26-neighbor AO + the model-quad path;
  tint baked into texture layers (grass/foliage colormap). `rewo demo`
  renders a synthetic showcase (stairs/slab/fence/glass/torch/plants/log)
  headless — verified all model families correct. Also switched the world
  pass to **reversed-Z depth** (`world::perspective_reverse_z`, GREATER, 0.0
  clear) to fix distant-terrain z-fighting holes — helps M2/M3 too. 26.x
  gotcha: model textures can be `{sprite, force_translucent}` objects, not
  just string refs (glass baked invisible until handled). Known cosmetic
  follow-up: grazing-angle far-field slivers on flat ground (needs MSAA /
  back-face cull). Deferred: greedy meshing, fluids, per-biome tint,
  animation, packed vertices.
- **M5 shipped 2026-07-21** (`rewo-gpu::world` full rewrite + `cull.comp`):
  GPU-driven rendering. Mega-buffer arena (device-local vert+index buffers,
  free-list suballocation, one-shot staging uploads) + per-column metadata
  SSBO + compute cull (frustum test → indirect commands + atomic count) +
  single `vkCmdDrawIndexedIndirectCount`. Enabled Vulkan 1.2
  draw_indirect_count + multiDrawIndirect. Verified: renders identically to
  M4, validation-clean, GPU cull drew 113/329 (216 culled on GPU via
  readback), windowed ~974 fps, removed the live-remesh per-frame wait_idle.
  **Two bugs fixed while verifying** (both predated M5): (1) world-space
  vertex double-add — mesher emits world-space but the shader also added the
  column origin, THE real cause of the M4 far-field holes (not depth);
  dropped the origin add. (2) grass_block collided as non-solid — it renders
  as a Model (cube + overlay element), and the collision table was
  matches!(Cube), so the bot fell through grass every tick (258 corrections);
  added a proper `solid` flag to the bake (Cube OR full-16³-element Model).
  Deferred M5 follow-ons: dedicated async transfer queue, visibility-graph
  cull, mega-buffer resize (over-cap columns dropped w/ log).
- **M6 shipped 2026-07-21** (latency/measurement pass): `rewo bench` — the
  deterministic render benchmark (replay world + orbit camera + GPU
  timestamps → avg/p50/p99/p99.9/1%-low/0.1%-low/max + histogram), the
  merge-gate metric. `stats.rs` gains 1%/0.1% lows (mean of slowest N%
  frames) + histogram. Frames-in-flight knob (`--fif`, `Renderer::
  with_frames_in_flight`). Measured on the 5080: GPU render 0.198 ms avg /
  0.367 ms 0.1%-low (rock-solid); windowed frame-consistency avg ~1 ms /
  ~5 ms 0.1%-low; fif=1 measurably tighter lows than fif=2 at same fps
  (latency-first, default stays 2). `VK_NV_low_latency2` deferred
  measure-first (GPU render ~0.2 ms is far below the frame budget — not the
  bottleneck until high RD/complexity). Subcommands: net/view/play/live/
  demo/bench.
- **M7 shipped 2026-07-22** (online-mode): the offline-only restriction is
  gone. `rewo-net/src/crypt.rs` — the login-encryption handshake
  (RSA-PKCS1v15 key exchange → Mojang session join with the BigInteger
  server hash → AES-128-CFB8 both directions, all KAT-tested), wired as a
  `NetStream` that ciphers at the Read/Write seam and splits per-direction.
  `rewo-net/src/chat_sign.rs` — signed chat: fetch the player certificate
  (`api.minecraftservices.com/player/certificates`), announce
  `chat_session_update`, SHA256withRSA-sign each message over the verbatim
  `PlayerChatMessage.updateSignature` layout with a chain index. Account
  handoff via `crypt::OnlineAuth::from_env` (REWO_ACCESS_TOKEN/UUID/
  USERNAME); `ewolauncher --mint-rewo-env` refreshes + prints them
  headlessly. **Verified with the user's real account** on an
  `enforce-secure-profile` server: session join + AES + a signed chat
  message that logged with no `[Not Secure]` prefix (0 corrections). Cert
  gotcha: Mojang's private key is PKCS#8 DER under a PKCS#1 label wrapped
  at 76 chars — strip the armor + parse DER directly (the rsa crate's
  strict RFC-7468 reader rejects it). Milestone detail in REWO_PLAN §15.
- **M7c shipped 2026-07-22** (real player skins): online play shows each
  player's actual skin. `rewo-net/src/skins.rs` decodes the Player Info
  `textures` property → URL + slim/wide; `rewo-app/src/skin_fetch.rs`
  fetches the PNG → 64×64 RGBA (username→profile resolution too); the
  entity atlas reserves a 32-slot 64² skin pool and `EntityPass::
  upload_skin` region-copies a fetched skin into it, returning a UV offset
  that relocates the default-Steve player quads onto that slot. Slim
  (`EntityModelKind::PlayerSlim`, vanilla 3-px arms) + wide, chosen from
  the profile model; overlays ride along. Live wiring is a `SkinLoader`
  worker thread + per-UUID registry in live_cmd. Verified headlessly with
  `mobshot --skin <username|url>` — lewlone's slim skin + Notch's wide
  skin, both distinct from default, overlays + arm width correct; facelabel
  243/243, demo byte-identical, bench flat.
- **Metadata mob detail shipped 2026-07-22**: slime/magma **size** + **baby**
  scaling. Both live at SynchedEntityData **index 16** (polymorphic — INT
  there is `AbstractCubeMob.ID_SIZE`, BOOLEAN there is `AgeableMob`/`Zombie`
  `DATA_BABY_ID`; the serializer type disambiguates in one decode). Index
  pinned by counting `defineId` up the hierarchy (Entity 0–7, LivingEntity
  8–14, Mob 15, subclass 16; cross-checked by the working `DATA_POSE=6`).
  `EntityDraw.scale_mul` scales the model (slime = size/2; baby ×0.5, a
  documented uniform approximation — vanilla keeps the head bigger).
- **M9 native CEM shipped 2026-07-22** (the EMF/ETF-equivalent — the answer
  to "can we do it without mods": yes): Rewo loads an OptiFine CEM resource
  pack and renders both the **custom mob models** and their **animations**,
  no mod loader. Stack: `rewo-data/src/cem.rs` (pack-zip loader → raw
  `.jem`/`.jpm` strings), `rewo-gpu/src/cem.rs` (JEM→`Model` with named
  bones + the `_animations.jpm` → program parse), `rewo-gpu/src/cem_anim.rs`
  (the OptiFine expression-language interpreter — lexer+Pratt parser+eval,
  parses all 284 real FA expressions). `EntityPass::new_with_cem` overrides
  a kind's built-in model; `part_transforms` applies the per-frame bone
  deltas. `mobshot --pack <zip> [--walk sw,amt --time t]` + `rewo live
  --pack` (also `REWO_PACK`). **Two load-bearing CEM conventions** (learned
  the hard way, don't re-derive): (1) a top-level `.jem` `part.translate` is
  the *rotation pivot*, NOT static position → boxes sit at raw coords, only
  *submodel* translates accumulate, and `pivot = to_model(−translate)`
  (verified exact vs vanilla); (2) the model bakes through a 180° Z-rotation
  (that's what `invertAxis:"xy"` is), so the animation's X/Y rotation angles
  + translations are **negated**, Z passes through — this is what turns
  flung-apart limbs into a cohesive walk. Verified on the user's real Fresh
  Animations pack: all body plans render + animate (zombie strides, pig/cow
  walk), no-pack facelabel gate stays 243/243 (additive). Polish left:
  foot-submodel leg pivots ~1px off (flat humanoid rigs exact), per-face
  `uvNorth`, scale channels, ETF textures (M9b). Detail in REWO_PLAN §15.
- **M10 client light engine shipped 2026-07-23** — a placed torch now lights,
  a dug tunnel now brightens. Vanilla clients recompute light for their own
  edits (the server only sends authoritative light at chunk load), so
  `rewo-world/src/light.rs` is the two-phase flood fill (decrease → increase)
  over block + sky, bounded by loaded columns; wired into `PlaySession` so
  `rewo live` relights and remeshes exactly the affected columns. Every rule
  is transcribed from the decompile: `dampening = isSolidRender ? 15 :
  (propagatesSkylightDown ? 0 : 1)`, step cost `max(1, dampening)`, **a face
  passes no light when the two occlusion shapes cover it** (this, not a graded
  cost, is why a `dampening 0` stair still shadows), and the sky column
  descends only while the edge is unoccluded. Data comes from a new machine
  extractor **`tools/gen_block_light.py`** (re-run after a version bump): it
  maps block → implementation class → the `propagatesSkylightDown` /
  `getLightDampening` **overrides** up the `extends` chain (glass returns
  true — sky passes it at full strength; leaves pin 1), expands the
  `ColorCollection` **and `WeatheringCopperCollection`** families by reading
  the id tables in `references/BlockItemIds.java` (the copper naming is
  irregular — `copper_block` but `exposed_copper` — and each name carries its
  weather-state index so a copper bulb's per-state 15/12/8/4 resolves), and
  **validates every generated name against blocks.json** (0 unresolved). Two traps worth remembering: `RenderKind::Cube` is **not** an
  opacity proxy (glass/leaves/ice all bake as `Cube`), and glass/leaves/water
  dampen by **1** — neither 0 nor 15. The gate **`rewo play --light-check`**
  recomputes loaded columns and diffs against the server's own light engine —
  the lighting equivalent of `CORRECTIONS`; **884,736 cells, 0 mismatches**
  on flat terrain, a village, an enclosed shaft, and a sealed torch-lit room.
  It immediately caught two long-standing bugs beyond lighting: the chunk
  payload's `empty_sky` mask was read and **discarded**, so every section above
  the terrain silently read sky-0; and **`section_blocks_update` was entirely
  unhandled**, so any multi-block edit to an already-loaded chunk (a `/fill`,
  an explosion, a piston, another player building) never appeared at all —
  hidden by the harness, because a structure built right after a `tp` is
  already there when the chunks stream in. M10 also added property-driven
  emission (candles `lit ? 3 × candles : 0`, glow berries, sea pickles, light
  blocks, vaults, trial spawners — each rule keyed by a source signature so a
  version bump fails loud) and shape occlusion for the rest of vanilla's
  `useShapeForLightOcclusion` set. **Gate caveat**: `--light-check` diffs
  against the *stored* light, which incremental relighting writes — pass
  `--no-relight`, or build in one run and grade from a fresh join, or the
  engine grades itself. Detail + open items in REWO_PLAN §15.
- **M11 vanilla lightmap + day/night shipped 2026-07-23** — M10 made the light
  *values* right; they were still rendered through an invented formula.
  `shaders/lightmap.glsl` now transcribes vanilla's: block and sky stay
  **separate** to the fragment shader (packed into the spare bits of the
  per-vertex layer word, `layer | block<<16 | sky<<20`, so the vertex doesn't
  grow), each goes through the curve `l/(4-3l)`, and they are **added** — not
  `max`ed — with block tinted warm. There is **no floor**, so an unlit cave is
  finally black (the old `0.25 + 0.75*l` could never go below 25%).
  `rewo-world/src/daylight.rs` transcribes 26.x's keyframed
  `Timelines.OVERWORLD_DAY` (the hard-coded `getSkyDarken` is gone): sky light
  1.0→0.24 and white→blue at night, sky gradient and fog darkening with it.
  Because the factor is a **uniform**, a sunrise costs one push constant, not
  a remesh — and a torch stays as bright at midnight as at noon. Wire gotchas:
  `set_time` is now `gameTime` + a **map of clock states**, a server sends
  **two** clocks (overworld *and* the_end — match by registry id, don't take
  the first), `ByteBufCodecs.holderRegistry` writes the id **raw** (the `id+1`
  scheme is a different codec), and clock states are only sent when they
  change. It also fixed a real long-standing bug: `emit_model` sampled the
  block's **own** cell (inside a solid block = always dark), so since
  grass_block renders as a Model the entire ground plane of every overworld
  was lighting at zero — hidden by the old floor. Detail in REWO_PLAN §15.
- **M12 sun/moon/stars/sunrise shipped 2026-07-23** — the sky was a bare
  gradient; M12 draws the clear-weather Overworld celestials in a Vulkan pass
  between the gradient sky and terrain. `rewo-world/src/celestial.rs` ports the
  exact 26.2 `Timelines.OVERWORLD_DAY`: the sun/moon/star **angle** tracks carry
  `symmetricCubicBezier(0.362, 0.241)` over a two-keyframes-at-tick-6000
  wrap-around pair (a naive lerp would freeze the sun), so `EasingType.CubicBezier`
  is ported verbatim (Newton-Raphson `solve_t` + bisection fallback); star
  brightness + the sunrise `ARGB_COLOR` track use linear ease, the latter
  interpolated by `srgbLerp` (componentwise `Mth.lerpInt`, **alpha included** —
  that settles RGB-vs-ARGB). `rewo-gpu/src/celestial.rs` + four shaders draw
  sunrise→sun→moon→stars in `addSkyPass` order, rotation-only sky space
  (`view_proj·T(eye)`), no depth, with the decompiled transform chains (base
  `Y(−90°)`, per-body `X(angle)`, sun `T(0,100,0)·scale(30,1,30)`, moon
  `scale(20,1,20)`, fan `X(90°)·Z(angle+90°)·scale(z=alpha)`), OVERLAY/TRANSLUCENT
  blends, reversed moon UV winding, and an 8-cell atlas. Textures come from the
  user's own client jar — sun + **eight separate moon-phase files**
  (`environment/celestial/moon/<phase>.png`, `MoonPhase.index()` order); no jar,
  no celestials. **Stars are generated bit-for-bit** vs a JOML 1.10.8 Java oracle
  (seed-10842 `BitRandomSource` LCG, reject sq-length `≤0.010000001`/`≥1.0` →
  **780 accepted / 4680 indices**, fingerprint `fef182656c6fe202`): the catch is
  JOML's `Math.fma` is **non-fused** by default, so `lengthSquared` is the
  right-associative `x*x+(y*y+z*z)` (2 ULP off a true FMA), and `libm::sin` (new
  dep, fdlibm) matches Java's `(float)Math.sin` where Windows' libm drifts. The
  sunrise fan samples the **`Mth` 65,536-entry sine table**, not platform trig —
  load-bearing at the half-turn, where `Mth.sin(π)` is a tiny *positive* table
  entry so the fan stays on side 0° while platform `sin(π_f32)` is negative and
  would flip (fan fingerprint `75280003503b2a33`). M12 also fixed the M11
  `SKY_COLOR` bug (only the horizon was tinted → blue midnight zenith; the zenith
  now scales by the sky tint too) and a **frozen-clock** bug: the server
  broadcasts `SetTime(gameTime, empty map)` every 20 ticks (only join/`/time`
  carry a clock state), so `day_ticks` froze while game time advanced. The fix
  ports 26.2's `ClientClockManager` — a `WorldClock` advanced from both
  `apply_set_time` (advance-then-overwrite, so an empty sync still moves the
  cycle) and a per-tick `ClientLevel.tickTime` local `+1`; running both isn't
  double-counting because each `advance` re-bases on `last_game_time`. Java
  primitive semantics are exact (`Mth.floor` returns an **`int`** → narrow to
  `i32` before widening `long fullTicks`; `partial` truncates back to `f32`;
  wrapping `long` arithmetic). Verified by a new permanent serverless gate
  **`rewo skyshot --check`** (validation layers on) that reconstructs each
  transform independently in f64 and asserts read-back pixel properties (zenith
  tint ratios, phase/alpha/discard/UV-winding, projected sun/moon envelopes,
  analytic sunrise-fan footprint, the 780/4680 star count) — not a "looks right"
  proxy. Gates: **142** unit tests green (world 44, net 41, gpu 33, data 5, mesh
  8, proto 11), skyshot green, mobshot 243/243, demo PNG byte-identical, bench
  GPU 0.228 ms avg, light gate EXACT, world clock advanced +278/280 ticks.
  In-game visual parity is **not** claimed (no eyeball pass); the properties the
  gate checks are what M12 verifies. Detail in REWO_PLAN §15.
- **M13 complete 26.2 lightmap shipped 2026-07-23** — ports the remaining
  `LightmapRenderStateExtractor`/`lightmap.fsh` terms: the exact four-draw
  LegacyRandom block flicker, gamma (default 0.5), night vision, darkness and
  its 22-tick blend state. The extractor partial is fixed **1.0**, not render
  interpolation. The full shader order is preserved, and the actual block tint
  is **0xFFFFD88C (255/216/140)**; M11's 0xFFD86C blue was wrong. Configuration
  captures the two effect registry raw IDs; update/remove packets affect only
  the local player, with exact duration/replacement semantics. One resolved RGB
  lightmap state now drives terrain, water and entities. Permanent gate:
  **`rewo lightmapshot --check`**, a validation-required production Vulkan
  readback matrix that independently proves tint, block factor, gamma ramp,
  night vision, black NaN store, darkness, water parity and entity RGB. It
  caught an adjacent asset-bake bug in the uncompromised M10 oracle: the fluid
  branch skipped light assignment, so water dampening was 0 (must be 1) and
  lava emission 0 (must be 15). Fixed from generated tables, not by editing
  generated code. Final gates: **180/180** six-crate tests + 10 app tests,
  lightmapshot/skyshot validation ON, mobshot 243/243, byte-identical demo,
  physics corrections 0, light **884,736 cells / 0 mismatches**. Replay median
  remained ~0.23 ms but later tail samples were system-noisy; exact numbers and
  the honest red-to-green water history are in REWO_PLAN §15.
- **M14 per-biome color shipped 2026-07-24** — grass/foliage/water tint +
  biome-driven camera sky/fog. The Configuration registry decodes in raw wire
  order (**66 biomes, 4 dimension types**); section biomes are retained 4×4×4
  (index `((y<<2)|z)<<2|x`, strategy bits 0 single / 1–3 indirect / >3 direct at
  registry `ceilLog2` = 7 for 66), from both the level-chunk payload and the
  `chunks_biomes` replacement (changes/load dirty 3×3). Dynamic tint is the exact
  **radius-2 5×5 integer mean** over the fiddled `BiomeManager.getBiome` for
  grass/foliage/dry-foliage/water, with `dark_forest`+`swamp` grass modifiers,
  fixed spruce/birch constants, tall-grass UPPER sampling below; tinted faces use
  the **raw atlas layer** + `MeshVertex.color` (no ABI growth), and a no-biome
  world keeps the legacy pre-tinted layers so the demo stays byte-identical. A
  per-`mesh_column` **`TintCache`** (canonical key = sampled pos+resolver,
  GrassBelow→Grass@y-1, constants bypass) mirrors vanilla's `BlockTintCache`
  without a global lock/invalidation. Camera sky/fog is a **separate** path — the
  raw-quart 6³ Gaussian (kernel `[0,1,4,6,4,1,0]`, integer `ARGB.srgbLerp`,
  dimension base then biome override) feeding **per-frame GPU base uniforms** (no
  remesh); Rewo's existing gradient/timeline sky still renders it, so this is
  *not* a formula-exact whole-sky claim. **Load-bearing protocol fix**: the play
  login dimension holder is `holderRegistry`/idMapper **raw 0-based**, NOT
  `ByteBufCodecs.holder`'s inline/`id+1` — correcting that adjacent bug was
  required to select the dimension sky/fog base. Permanent gate:
  **`rewo tintshot --check`** (serverless, validation-required Vulkan readback of
  the production jar-bake + synthetic single/indirect/direct biome containers +
  `mesh_column`), pinning Temurin-25-verified vectors — boundary
  **[91,163,163]**, dark_forest **[147,26,5]**, swamp light/dark
  **[106,112,57]**/**[76,118,60]**, spruce/birch **[97,153,97]**/**[128,167,85]**,
  camera fog boundary **0xffac2d6d** (A inherits / B overrides), fully-fogged
  terrain green→blue under a red sky, **0 VUIDs**; it rejects constant-plains,
  axis transpose, wrong fiddle/radius/mean, spruce/birch-as-foliage, wrong
  modifiers, raw/legacy mixup, block-fiddle camera sampling, and dropped GPU
  plumbing. Final gates: **215/215** six-crate tests (world 81, net 67, gpu 37,
  data 9, mesh 10, proto 11) + **10/10** app; tintshot/lightmapshot/skyshot
  validation ON exit 0 0 VUIDs, mobshot 243/243, byte-identical demo, physics
  **CORRECTIONS 0** over 600 ticks (PLACE+DIG both verified), light **884,736
  cells / block 0 sky 0 EXACT**. Replay (no biome context — guards neutral
  rendering, does not measure the tint cache) GPU avg ~0.238–0.241 ms.
  **Scoped exclusions**: biome blend radius fixed at vanilla default 2;
  modifier-form custom-datapack sky/fog attrs not applied (26.2 uses bare
  overrides); probe per-tick history omitted (sampled per frame); respawn /
  dimension-transition / Nether-End base selection untested; redstone/stem/lily
  `BlockColors` explicitly out of M14. Honest history + exact numbers in
  REWO_PLAN §15 (incl. the first oracle rejected as insufficient and strengthened).
- **M15 exact packed ABI + conservative greedy cubes shipped 2026-07-24.**
  `MeshVertex` is **28 bytes**: position f32×3, exact UV f32×2, packed
  `layer16|block4|sky4|shade3|AO2`, and packed tint RGB. A 24-byte f16-UV
  candidate was rejected after changing 6 canonical-demo pixels (max Δ25).
  The final shader reconstructs the legacy shade×AO×tint formula exactly; the
  shader build parses optimized SPIR-V and requires float 255, `OpFDiv`, and
  `NoContraction`. Full cube faces greedily merge only across identical block
  state, packed light and tint with uniform AO. Models/fluids remain
  byte-identical. **Never merge +Y/top faces** without a new proof: enabling
  them changed 11 demo pixels (10 UV interpolation/nearest-sampling, one
  coverage); the other five directions are byte-identical. Permanent gate
  `rewo meshshot --check` expands rectangles to reference unit faces and pins
  direction/block/layer/light/AO/tint seams plus exact model/water/lava controls.
  Oracle fixture: 854→265 quads (−69.0%). Replay: 149.13→109.39 MiB
  (**−26.65%**), 3,723,192→3,373,772 vertices (−9.38%), arena 93.080→84.344%;
  final GPU avg 0.232 ms, but noisy tails mean no latency improvement is
  claimed. Gates: **237/237** six-crate + **19/19** app, all property gates
  green, demo exact, physics corrections 0, light 884,736/0. Full failure
  history and measurements: REWO_PLAN §15.
- **M16 dimensions shipped + verified 2026-07-24.** It is committed locally on
  branch `codex/rewo-m16-dimensions` and not pushed. The vanilla test server was
  stopped and port 25599 verified free after the final gates. The
  `minecraft:dimension_type` registry is now parsed once
  (`rewo-net/src/dimension_parse.rs`), kept in **raw wire order** — the vector
  index *is* the holder id, nothing selects by name — and actually consumed:
  per-dimension vertical shape (the Nether is 0..256, not −64..384; the stale
  Overworld shape mis-decoded every Nether chunk), `has_skylight`, `skybox`,
  ambient light, Nether cardinal face shade, sky/fog/ambient/sky-light colours
  and factor, `has_fixed_time`, and `has_day_timeline`. Plus the End sky pass
  (`rewo-gpu/src/end_sky.rs`), spawn info, and a transition that discards the
  old world and refences the mesh pool by generation.
  **Three load-bearing facts.** (1) `has_day_timeline` is **independent of
  `has_fixed_time`** — separate `DimensionType` members; deriving one from the
  other happens to be right for all four vanilla dimensions and is still wrong.
  It comes from the `timelines` holder set, expanded through
  `data/minecraft/tags/timeline/*.json`. (2) The Nether sets **no**
  `sky_color`/`fog_color`; absence must stay `None`, because the attribute's
  literal `0` default would read as opaque black to the biome colour stack.
  (3) A malformed entry is a connection error — never a substituted Overworld.
  Gates: **`rewo dimensioncheck --check`** (serverless) grades four independent
  inputs — a captured Configuration `registry_data` packet, the bundled
  transcription, the **real decompiled datagen JSON** read by
  `rewo-app/src/dimension_json.rs` (a `serde_json` reader sharing no code with
  the NBT parser), and a hand-written `EXPECT` table that grades all three and
  is itself graded by the JSON — then the world/mesh binding and the generation
  fence; it fails closed on a missing recording or decompile. **`rewo play
  --dimension-check`** is the live gate: 4/4 checkpoints, 3/3 transitions, 329
  columns discarded/requeued each, 0 decode failures, 0 settled corrections.
  Measured: **344 unit tests** (proto 11, world 93, data 9, net 102, mesh 38,
  gpu 44, app 47), `mobshot` 243/243, all Vulkan oracles green with validation
  ON / 0 VUIDs, demo SHA-256 byte-identical to M15, physics 600 ticks
  CORRECTIONS 0, light 884,736 cells / 0 mismatches, release build green.
  Replay GPU avg 0.240 ms with a system-noisy tail — **no** latency improvement
  claimed. Full detail: REWO_PLAN §15.
- **M16.1 — play gate build actions now fail closed 2026-07-24 (`f4b54d1`,
  local; not pushed).** M16 left one honest red deferred: `rewo play`
  (build-enabled) printed "PLACE verify … still air ✗" ~1 run in 4 yet exited 0,
  and `place:true`/`give:true` meant only "packet sent". **Not a protocol bug —
  packets were byte-exact vs the decompile; a pre-M16 (M3-era) harness + gate
  defect.** Root cause (decompile): the harness placed dirt at `(fx+1, fy)`, the
  cell beside the bot's feet; 26.2 `BlockItem.canPlace` gates on
  `isUnobstructed(state, clickedPos, placementContext(player))` and the player's
  0.6-wide AABB reaches east to `fx+1.3`, so its own body occupied the cell
  whenever fractional x ≥ 0.7 → server rejects → air (intermittent as resting x
  varies; dig never hits this). Fix: place two east (`fx+2`, past `fx+1.3`). The
  observation was always right — `handleUseItemOn` sends the acting player a
  `block_update` for BOTH `pos` and `pos.relative(direction)` on every
  use-item-on, accepted or not. The gate (`build_acceptance` +
  `evaluate_build_actions`) now reads the server's world at the recorded targets
  and proves the EXACT state (placed == `minecraft:dirt` default from the block
  table, dug == air), prints `ACCEPT …`, and returns exit 1 if unproven or
  never-run; `--no-build`/`--dimension-check` are exempt. Gates: 350 unit (app
  47→53), 4× live 30 s CORRECTIONS 0 + place=dirt + dig=air exit 0, fail-closed
  proven live (16 s run exits 1).
- **M17 exact model-visible entity events shipped + verified 2026-07-25.**
  Committed locally as `55388c8` on `codex/rewo-m17-entity-events` (base
  `f4b54d1`; not pushed). Before it, `ClientboundEntityEventPacket` fell off the dispatch chain
  as an unknown id — the Warden's ribcages never animated and a balled Armadillo
  never re-peeked. The packet is a **signed fixed BE-i32 entity id + signed byte
  event id** (not var-ints); the report resolves clientbound-play `entity_event`
  to **id 34** (looked up by name, so a renumber fails loud). Type ids resolve
  through production `EntityTypes::id_of`: **Warden 143, Armadillo 4**. Three
  mappings in `apply_entity_event`/`route_entity_event`: Warden 4 → durably stop
  the metadata roar `AnimationState` for that same ROARING episode, then
  unconditionally restart exact `WARDEN_ATTACK`; Warden 62 → restart exact
  `WARDEN_SONIC_BOOM`; Armadillo 64 → re-clock the shared metadata SCARED/PEEK
  `AnimationState` from age 0 (the final balled hold remains after it runs).
  Repeats restart the clock; missing/wrong-kind/unknown/excluded events are
  inert; state clears on entity removal and id reuse.
  **Load-bearing:** the two Warden rigs are exact generated defs from decompiled
  `WardenAnimation.java` via `tools/gen_anim_defs.ps1` (never hand-edited); the
  Warden ribcages were promoted from static folded cubes to **named body
  children** so `WARDEN_SONIC_BOOM` can swing them (neutral geometry unchanged →
  mobshot untouched); the generator now emits **deterministic LF** so
  `git diff --check` is clean and a re-run reproduces the file byte-for-byte (an
  EOL-ignoring semantic diff is exactly the two new defs, 222 lines). The
  renderer feeds a **production event-age input distinct from the metadata
  gesture ages**, sharing the session tick/partial epoch, through the same
  CEM/vanilla part pipeline.
  **Corrections/exclusions (recorded so they aren't re-derived):** Allay
  `handleEntityEvent(18)` is **heart particles only** — the dance is
  `DATA_DANCING` (metadata index 16, BOOLEAN serializer id 8) with client
  dancing/spinning counters + root/head formulas; that's separate future
  *metadata-animation* work, not an entity-event claim (the generic
  `(16,BOOLEAN)→baby` decode Rewo has is latent/inert for Allay only because
  `is_baby` isn't rendered for it). Warden tendril (event 61, needs tendril
  procedural/emissive modelling), generic `ClientboundAnimatePacket` arm swings
  (need handedness/equipment/CEM closure — a future combat-animation milestone),
  hurt/damage overlays, particle/sound-only statuses, and AI simulation are all
  excluded. **No live AI-triggered encounter was staged or claimed** — M17 is
  authoritative through exact raw-packet injection into the production dispatcher
  plus independent decompile literals; these client-receipt semantics don't
  depend on vanilla's server-authoritative (nondeterministic) AI timing.
  **Gate: `rewo eventshot --check`** — permanent serverless CPU-only,
  fail-closed **28/28 witnesses**, driving the whole production path (raw
  fixed-body packet → `route_entity_event` → `EntityTable::start_event` →
  `resolve_mob_anim` → `oracle_part_deltas`). Loads real
  `packets.json`/`registries.json`, proves id 34 + Warden 143/Armadillo 4; the
  targets are **independent decompiled literals** (it does NOT read `anim_defs`
  as its expectation; catmull-rom recomputed from four frame literals), ~1e-4
  tolerances, each with a mutation/sensitivity partner (wrong packet id,
  missing/wrong entity, event 61/unknown, repeat, remove/reuse, neutral parts).
  Two consecutive release runs: identical PASS 28/28. Measured: **360 unit
  tests** (world 95, net 110, gpu 44, data 9, mesh 38, proto 11 = 307 lib; app
  53 — M16.1 was 350; M17 adds world +2, net +8), release build green
  (pre-existing warnings only), mobshot 243/243, lightmapshot/skyshot/tintshot/
  meshshot/dimensioncheck green with Vulkan validation ON / 0 VUIDs, demo SHA-256
  byte-identical to M15, bench replay GPU avg 0.231 ms (no latency change claimed
  — M17 doesn't touch the replay entity path), physics 600 ticks CORRECTIONS 0,
  light `--no-relight` 884,736 cells / 0 mismatches, live dimension 4/4
  checkpoints + 3/3 transitions. Full detail: REWO_PLAN §15.
- **M18 exact Allay dance (DATA_DANCING metadata animation) shipped + verified
  2026-07-25.** Committed locally as `bb8be20` on `codex/rewo-m18-allay-dance`
  (base `6096bbd`, the M17 handoff; not pushed). The Allay dance is *metadata*,
  not an entity event (M17 proved event 18 is heart particles) — the first
  metadata-driven rig. `DATA_DANCING` is **SynchedEntityData index 16, BOOLEAN
  serializer 8** (`Allay` extends `PathfinderMob`, not `AgeableMob`, so slot 16
  BOOLEAN is dancing, whereas `AgeableMob`/`Zombie` put `DATA_BABY_ID` there — the
  byte parser can't disambiguate, only the **kind** can, at the routing layer).
  Resolved wire facts: `set_entity_data` id **99**, Allay type **2**, Zombie
  control **151**. Shipped, all decompile-exact: the `Allay.tick()` client
  counters in `rewo-world` `EntityTable` (dance-tick increments then reads the
  `%55<15` spin window; `spinning`/`spinning0` ramp ±1 clamped 0..15; false resets
  on the *next* tick; repeated true does **not** restart; cleared on remove +
  re-add), the `AllayModel` root/head formulas (`Anim::AllayRoot`/`AllayHead`:
  `danceSpeed = ageInTicks·8° + walkAnimationSpeed`; `root.yRot = 4π·spin` only
  while `isSpinning` else 0; `root.zRot = cos·16°·(1−spin)`; `head.yRot/zRot =
  cos·30°/14°·(1−spin)`; dancing suppresses the head-look; wings stay
  unconditional), the Allay model **restructured into the real
  `root→{head, body→{arms,wings}}` hierarchy** (rest geometry neutral, mobshot
  243/243 unchanged), and **vanilla missing-entity inertness** (`handleSetEntityData`
  drops metadata for `getEntity==null` — no state mutated). Production chain: raw
  report-resolved `set_entity_data` → `route_set_entity_data`/`apply_set_entity_data`
  (kind-aware routing) → `EntityTable` counters → `live_cmd::resolve_allay_dance`
  (shared by the collector and the gate) → GPU pose; `play_cmd` + `live_cmd` both
  resolve the Allay type id. **Senior review corrected**: missing-id baby fallback
  → decompile-exact inert; extracted the shared live resolver so the gate can't
  bypass the app mapping; `play_cmd` now resolves the Allay id; explicit
  wrong-index/wrong-serializer witnesses. **Gate: `rewo danceshot --check`** —
  permanent serverless CPU-only fail-closed **24/24**, two identical runs;
  independent counter sim + `AllayModel`/`AllayWing` transcriptions (nothing reads
  the production formulas as expectation), real `packets.json`/`registries.json`.
  Measured: **368 unit tests** (world 98, net 114, gpu 44, data 9, mesh 38, proto
  11 = 314 lib; app 54), release build green, plain `git diff --check` clean,
  eventshot 28/28, mobshot 243/243, lightmapshot/skyshot/tintshot/meshshot/
  dimensioncheck green (Vulkan validation ON), demo SHA-256 byte-identical to
  M15/M16/M17, replay GPU avg 0.220 ms (no latency change claimed), live physics
  600 ticks CORRECTIONS 0, light 884,736 cells / 0 mismatches, dimension 4/4 + 3/3.
  Exclusions: no live jukebox/AI encounter (raw-packet injection is the
  deterministic proof); the Allay's unconditional body flying-tilt / root
  idle-bob / arm idle-bob remain unimplemented (not the dance); no claim of
  exhaustive index-16 ownership. Full detail: REWO_PLAN §15.
- **M19 exact combat swings + the ArmPose hold baseline shipped + verified
  2026-07-25.** `ClientboundAnimatePacket` (id **2**, VarInt id + unsigned byte
  action) was falling off the dispatch chain, so nothing ever swung. M19 ships
  the exact `LivingEntity` swing state machine (accept/restart rule, `swingTime
  = -1` park, increment-then-end, the `getAttackAnim` `+1` wrap), item-driven
  duration (`tools/gen_swing_animations.py`: **7 non-default over 1,537 items** —
  the spears, STAB 13–23; everything else WHACK/6) with exact DIG_SPEED /
  MINING_FATIGUE adjustment, a machine-extracted living/swing-ticking split
  (`tools/gen_entity_classes.py`: **93 living / 36 swing-ticking of 158**), and
  `HumanoidModel.setupAttackAnimation` — **layered on the `ArmPose` hold
  baseline** `pose{Right,Left}Arm` writes first (`EMPTY` / `ITEM` / `SPEAR`).
  Two load-bearing facts: **`ITEM` is the fall-through for any ordinary held
  item**, so omitting the hold stage posed every armed player from an unarmed
  baseline (18° too high, walk swing unhalved) — and it is `AvatarRenderer`, not
  `HumanoidMobRenderer`, that produces it; and `SPEAR`'s `affectsOffhandPose`
  means a spear in the **off** hand leaves the main arm entirely unposed.
  Unknowable items suppress the pose and CEM `swing_progress` rather than guess.
  `ItemTags.SPEARS` is read as a *tag* from the client jar, not inferred from the
  swing component. Gate: **`rewo swingshot --check` 61/61**, serverless,
  fail-closed, with independent `ease`/`Mth`/pose transcriptions (the `Mth`
  witness: 0 bit mismatches over 60,003 samples vs 39,917 platform-sine
  differences). **404 tests**; demo PNG byte-identical to M15–M18; live
  `--swing-check` decodes server-sent equipment with CORRECTIONS 0. **Open:**
  `animateZombieArms` (the undead families have their own attack rig, so a
  swinging zombie shows no arm motion yet) and the eight use-driven arm poses.
- **M20 exact mob combat rigs shipped + verified 2026-07-25.** M19 gave the
  *player* an exact swing; M20 gives it to the mobs that attack you — four
  vanilla rigs that all run **after** `HumanoidModel.setupAnim` and overwrite
  it: `AnimationUtils.animateZombieArms` (zombie/husk/drowned/zombie-villager/
  zombified-piglin), `SkeletonModel`'s own override, and `IllagerModel`'s
  arm-pose switch with both its attack branches. **The sizing discovery:** the
  undead arms were a baked `Fold::rot(−π/2)` on `STATIC_PART` — frozen at −90°
  where vanilla rests at **−π/2.25 (−80°)** and deepens to **−π/1.5 (−120°)**
  when aggressive, so the pose was ~10° wrong *and* structurally unable to move.
  They are real animated parts now. Three vanilla quirks reproduced, each
  witnessed: a **STAB item skips the strike** (the humanoid pose survives) and
  then takes a **second bob** (`bobArms` sits outside the guard — observable as
  `zRot +0.2`); `animateAttackArms` **assigns rotations only**, so
  `setupAttackAnimation`'s pivot movement survives underneath; and **only a baby
  holding an item** drops its arms. One new wire input:
  `Mob.DATA_MOB_FLAGS_ID` — **index 15, BYTE**, bit 2 `isLeftHanded`, bit 4
  `isAggressive` — the same slot M19 reads as the player's main arm, separated
  by serializer and additionally gated on the type being a `Mob` (an
  `ArmorStand`'s client flags share the slot). It fixes mob handedness for free
  (`Mob.getMainArm()` *is* `isLeftHanded()`). Three more polymorphic slots are
  resolved by machine-extracted ancestry (`gen_entity_classes.py` `ANCESTRY_SETS`,
  fail-loud if empty): `MOB` 90, `RAIDER` 6, `SPELLCASTER_ILLAGER` 2, `ILLAGER`
  4 — note **`ravager` and `witch` are Raiders but not Illagers**, so their
  index-16 BOOLEAN is `IS_CELEBRATING`, previously misread as baby. Illagers
  assign their **own walk over both arms** (wiping the hold pose, attack and
  bob), then switch: empty-handed `ATTACKING` runs `animateZombieArms` with a
  **literal `true`**, armed runs `swingWeaponDown`, and `CROSSED` is a
  *visibility* switch (one model, both arm sets). Gate: **`swingshot --check`
  61 → 77 witnesses**, independent transcriptions throughout, metadata driven
  through the real `route_set_entity_data`. **410 tests**; demo PNG
  byte-identical to M15–M19; `mobshot` 243/243 even though undead neutral
  geometry moved. **Open:** held items are not rendered (mobs swing
  empty-handed); illager `CROSSBOW_HOLD`/`CROSSBOW_CHARGE` are derived but not
  posed (they need `ticksUsingItem`, unsynced for remote entities).
- **M20.1 + M21 shipped + verified 2026-07-25.** **M20.1** fixes the live build
  gate M20 recorded as flaky: it clicked the top face of `(fx+2, fy-1)`, which
  assumed the bot stood on *undisturbed* ground — an earlier run's own hole put
  the target on the grass surface and the server correctly rejected it. It now
  scans east for the first air-over-solid column and **fails closed** if there
  is none (5/5 green in the world that used to fail 1-in-4). **M21 consumes
  `ClientboundDamageEventPacket`**: the exact `hurtTime`/`hurtDuration` clock
  (10, one decrement per tick, re-armed not extended by a repeat), the
  `walkAnimation.setSpeed(1.5F)` limb kick with vanilla's render-side clamp to
  1.0, and the red damage flash. The packet's damage-type holder is
  `holderRegistry` — a **raw 0-based id**, not `holder`'s `id+1` — and the whole
  body is walked so a short read cannot desync the buffer; receipt is gated on
  the entity being tracked *and* living (`handleDamageEvent` is a `LivingEntity`
  override). **The flash forced a vertex-ABI split**: vanilla's `entity.fsh`
  mixes the overlay into `texture × vertexColor` and multiplies the lightmap
  *after*, so the CPU can no longer fold light into the vertex colour — a new
  `light_hurt` attribute carries light in `rgb` and the hurt flag in `a`. The
  overlay is `OverlayTexture`'s red row, **0xB3FF0000** (rgb (1,0,0), a =
  179/255), and the mix is done **in sRGB space**, not linear. Gate:
  **`rewo hurtshot --check` 18/18**, validation ON, 0 VUIDs, verifying the flash
  *by predicting the hurt pixel from the unhurt one* with sensitivity partners
  for linear-space mixing and post-lightmap application. **415 tests**; demo PNG
  byte-identical to M15–M20. The ABI change also exposed a latent bug — the
  upload path hard-coded `total * 36` beside `VERTEX_STRIDE`, so at stride 52
  only 36 of every 52 bytes reached the GPU (`mobshot` 223/243 until fixed).
  **Open:** held items are still not rendered (mobs swing and flash
  empty-handed); `deathTime` — the other half of `hasRedOverlay` — is the death
  animation and its own feature.
- **M22 held items shipped + verified 2026-07-25 — both geometry paths.**
  M19-M21 built the swing, the mob rigs and the damage flash; every one of them
  was swinging empty-handed. 26.x splits an item into a *definition*
  (`assets/minecraft/items/<item>.json`, a tree that chooses a model from stack
  state) and the parent-chained model. Surveyed on the real jar: **1390 of 1537
  are plain `minecraft:model`**, of which **750 point at `block/…`** and the
  rest walk `item/<n>` → `handheld` → `generated` → `builtin/generated`. The
  seam that unified them is **`append_model_quads`**, which takes a model *name*
  and emits quads carrying a texture-array layer index — so a block item reuses
  the block bake rather than needing a parallel resolver; the entity pass cannot
  sample that layer, so its pixels are copied out. Both paths converge on
  **quads in 0..16 model units with UVs in 0..1 of their own texture**, and the
  renderer never learns which source an item came from. The sprite path is
  `ItemModelGenerator`'s extrusion (two faces across the 7.5..8.5 slab + one
  thin quad per alpha edge, UVs inset 0.1); a diamond sword bakes to **82
  quads**. **Two invertible details** transcribed deliberately:
  `SideDirection::Left` is `Direction.EAST` (the names describe the sprite edge,
  not the world axis) and `isTransparent` is **true out of bounds**, which is
  the only reason a sprite border extrudes. **The trap:**
  `ItemTransform.Deserializer` multiplies translation by **0.0625** and clamps
  (±5, ±4) *before* `apply` runs — storing raw JSON puts every item **16× too
  far from the hand**. Shading uses the **rotated** normal, not the baked `dir`,
  because an item is turned on its side in the hand. 1233 textures do not fit an
  atlas band, so items got the demand-filled pool player skins already have; the
  atlas grew 1024→1280 while the shelf packer still stops at 896, leaving **mob
  packing byte-for-byte unchanged** (mobshot 243/243). Gate:
  **`rewo itemshot --check` 18/18**, validation ON, 0 VUIDs, verifying placement
  *against the hand* — sprite centroid (90,151) and block centroid (87,156) land
  together, proving one transform chain serves both sources, and a suppressed
  item differs from an empty hand by **0 pixels**. **435 tests**; demo PNG
  byte-identical to M15-M21. **Open:** the 147 state-dependent items
  (select/special/composite/condition/range_dispatch) suppress rather than
  guess; first-person/GUI/ground contexts, the spear attack-item animation,
  enchantment glint and per-layer tint are all out.
- **M23–M25 + the block-entity arc shipped 2026-07-25/26.** M23 item-use state
  (retiring the blocker three earlier milestones blamed — `useItemRemainingTicks`
  is *derived* by the client, not synchronised) and the eight use-driven
  `ArmPose`s; M24 the death animation and item entities; M25 block-entity decode
  plus a fail-closed type registry and a *measured* statement of the gap (96
  blocks bake to no geometry, 86 of them real block entities). Then the
  rendering half: chests, chest lids driven by `block_event`, double chests,
  17 shulker boxes, and **world-space text** so signs are legible — which
  turned out to be a small addition rather than a new pass, because a nametag
  is already world-space glyph quads and sign text is the same emitter with the
  basis taken from the surface instead of the camera.
- **M26 shipped + verified 2026-07-26 — `block_event` reaches the right block
  entity, and a shulker box opens.** `b0 == 1` is **not one opcode**: it is a
  chest's viewer count, a shulker box's open/close pair, and a bell's
  `Direction.from3DDataValue`, selected by the block entity's type exactly as
  vanilla's virtual `triggerEvent` call is. Reading it as "a chest lid" — which
  this client did — meant a bell rung from any side but below opened a phantom
  lid at the bell. Also: the shulker's rule is `b1 == 0` / `b1 == 1` with **no
  else** (a second viewer changes nothing), not the chest's `b1 > 0`; the
  animated part group became a matrix so one emitter expresses both a hinge and
  a slide-plus-spin; the classification caught up with the four types that had
  quietly started rendering (**seven** still invisible, not eight); and
  `BlockEntityRegistry` runs in the client rather than only in the gate. Two
  process lessons recorded in REWO_PLAN §0.0: a witness that asserts a *moment*
  ("nothing is Rendered yet") is not a guard, and several source files carry
  mixed CRLF/LF endings that an editor will silently normalise into a
  3,400-line diff. Gate `rewo blockentityshot --check` **88/88**; **479 tests**
  (424 lib + 55 app); demo PNG byte-identical to M15 onward.
- **M27/M28 shipped + verified 2026-07-26 — sign text, and the invisible block
  entities.** Five commits took `blockentityshot` from 70 to **125** witnesses
  and the still-invisible block-entity set from **eleven types to two**.
  - **M27** dyed and glowing sign text plus the line break. *Glowing text is
    not "the same colour, brighter"*: unglowing is the dye at 40%, glowing is
    the dye at FULL strength lit fullbright, with the 40% version demoted to
    its eight-copy outline. A sign does not wrap — `getRenderMessages` keeps
    fragment 0, so a long line is truncated at a word boundary.
  - **M28** skulls (7 types, 14 blocks) + the conduit shell. Skulls are
    **entity** models, authored y-down, so both transforms end in
    `scale(-1,-1,1)` — a chest has no such flip. Forced four generalisations of
    the box machinery: rest rotation, `CubeDeformation` grow, mirror, and a
    **per-model texture size** (a mob head's sheet is 64×32, not 64×64).
  - **M28b** the decorated pot — the first block entity that is not one model
    (base + four sherd-textured sides). Needed a second form of `visibleFaces`:
    `EnumSet.of(NORTH)` builds only one face where `allOfEnumExcept` omits one.
  - **M28c** banners (32 blocks) — the first whose texture carries no colour. A
    pattern sprite is a greyscale **mask**, so `BlockEntityDraw` grew a `tint`
    rather than baking 16 dyes × 43 patterns. The banner dye table is
    `getTextureDiffuseColor`, **not** the sign's `getTextColor` (red 0xB02E26
    vs 0xFF0000), and a wall banner's yaw is the facing's *own* toYRot where a
    wall skull's is its opposite.
  - **M28d** the spawner's `block_event` — the third meaning of `b0 == 1`.
    Resetting `spawnDelay` is the whole client effect and shows only through
    the spin: `1000 / (spawnDelay + 200)` makes a spawner **accelerate** toward
    its next spawn.
  - **Three gate witnesses caught real bugs pre-ship**: a pot side baking six
    quads instead of one (coincident, z-fighting), a banner base texture path
    (`entity/banner/banner_base.png`, not `entity/banner_base`) that baked no
    pole while every pattern still loaded, and an existing witness that had
    quietly started measuring skulls as shulker boxes.
  - **M28e/M28f** the copper golem statue and the two end portals — the last
    two. The statue's four poses are **separate** nested layers where a child's
    offset rides through its parent's *rotation*; they are machine-extracted by
    `tools/gen_copper_golem_poses.py` because 38 rotated boxes fail **silently**
    when hand-copied, and `k25` proves the hierarchy by comparing each box
    against a naive offset-sum (must agree in STANDING, must differ in
    RUNNING). The end portals were half-misdescribed as "a shader, not a
    model": the geometry is an ordinary cube (portal = horizontal faces only, a
    slab from y 0.375 to 0.75), and only the render *type* is a shader,
    approximated by one static layer of `end_portal.png`.
  - **M25's Invisible list is now EMPTY — eleven types measured, eleven
    rendering.** `blockentityshot` 21 → **133** witnesses across the arc.
  - **M29 the block-entity animation clock** — banners sway, pots wobble,
    piglin ears move, dragon jaws open. Not ONE clock: what each animates
    *from* differs (position+gametime / an event+start tick / an accumulating
    counter), and grouping by that is what made it tractable. A pot's wobble is
    a **fourth** meaning of `b0 == 1` (`b1` = a WobbleStyle ordinal, and the
    arrival tick is the start). **It exposed two rest poses that were already
    wrong**: `SkullModelBase.setupAnim` ALWAYS runs, so a piglin's ears belong
    at ∓0.7 rad (not the mesh's ∓30°, ~10° off on every head) and a dragon's
    jaw rests 0.2 rad OPEN (Rewo drew it shut) — a wrong *rest* pose is
    invisible precisely because nothing moves to contradict it.
  - **M30 the active conduit** — that world scan. A conduit decides its own
    activation from the blocks around it (the server sends nothing), so
    `updateShape` was the whole prerequisite. **The shell is 42 positions, not
    48** — the three axis rings share their axis ends — **and 42 is also the
    hunting threshold**, so a conduit opens its eye exactly when its frame is
    COMPLETE. `isWaterAt` counts waterlogged blocks, so the bake grew a
    per-state `water` table. Ships the cage (tumbling about the tilted axis
    `(0.5,1,0.5)`, not plain Y), the wind shroud twice (the second at 0.875,
    counter-rotated), and a camera-facing eye — the one input in this path
    that's a property of the VIEW not the block. The deg→rad round trip in the
    renderer is an exact no-op; don't "fix" half of it.
  - **M31 the spawner's caged mob.** M29 called this "an entity model composed
    into a block-entity draw" — one word off: the mob belongs in the **ENTITY**
    path, just positioned differently. Every other entity STANDS (`pos` = its
    feet); this one is *mounted*, so `EntityDraw` gained an optional `mount`
    affine applied to the feet-relative position. Same models, rigs and
    animations as every other mob. Display entity is `SpawnData→entity→id` (two
    levels down); empty/absent/unregistered → NO mob, never a default. Scale is
    `0.53125 / max(bbW,bbH)` **only if > 1.0**; render spin is the stored one
    **×10**; `scale_mul` stays 1 (the fit scale is in the mount — applying both
    shrinks it squared).
  - **A witness disproved my own comment** (3rd time this arc): I claimed the
    inner `translate(0,-0.2,0)` makes the mob orbit — it lies **along the spin
    axis**, so it commutes and the translates could be swapped with no effect.
    The `-30°` X tilt is the load-bearing part. **The claims that survive
    unchallenged are the ones nothing in the render moves against.**
  - **M32 the end-portal shader** — the last item. It samples in **SCREEN
    space** (`texProj0 = projection_from_position(gl_Position)`, vertex format
    POSITION-only), which is why the mesh UVs were never used and why it needed
    its own pipeline; the two portals leave the block-entity resolver entirely,
    or they'd draw twice. `PORTAL_LAYERS` 15 (portal) / 16 (gateway) — a shader
    *define* in vanilla, a push constant here. Sampler0 is end_sky, Sampler1 is
    end_portal (the opposite of the name). `GameTime` is a **daily fraction**,
    not a tick count. **Trap:** vanilla's `mat4(...)` literals are
    **column-major** GLSL — the translate lives at `m[0][3]`, and it works
    because the sampling is `texProj0 * matrix`, a ROW-vector multiply. Copy
    them verbatim; "tidying" them into the slots they look like they belong in
    silently breaks every layer.
  - **The block-entity arc is complete**: 11 invisible types measured, 11
    rendering, `blockentityshot` 21 → **172** witnesses. Its real lesson is
    the **five** times a witness corrected something already written as fact
    (see REWO_PLAN §15 "The block-entity arc, in one place"). **The claims that
    survive unchallenged are the ones nothing in the render moves against.**
  - **M32b closed the portal pass's read-back gap**: `rewo portalshot --check`,
    serverless, validation-required, **12/12**, 0 VUIDs. Two properties make an
    exact prediction possible without reproducing a single matrix — **uniform
    textures collapse them** (the frame is then
    `sky*COLORS[0] + portal*sum(COLORS[0..layers])`, computed on the CPU), and
    **one layer isolates one sample**, at which point the sampled `u` is an
    affine function of the screen UV alone and the column-major reading is
    directly observable. Mutating the shipped shader to the transposed multiply
    drops that witness 21/21 → 9/21 while every uniform-texture witness still
    passes.
  - **The portal's sample is welded to the SCREEN, not the model.** Sliding the
    quad through the world or rolling the camera leaves a screen-covering
    portal's pixels identical (measured: ≤175 of 65,536 bytes, all at delta 1).
    The first version of that witness asserted the opposite and failed.
  - **M33 weather and clouds** — rain, snow and a cloud deck, gated by
    `rewo weathershot --check` **27/27** (validation ON, 0 VUIDs). Three facts
    that read backwards: **`START_RAINING` sets the rain level to 0 and
    `STOP_RAINING` to 1** (the names describe the server's transition; the
    client sets the value its `RAIN_LEVEL_CHANGE` ramp starts *from*); the
    client **never interpolates** the level (`setRainLevel` writes both slots,
    so the smoothing is entirely server-side); and clouds are absent **by
    attribute, not by dimension check** — `CLOUD_COLOR` defaults to a
    transparent 0 and the pass is skipped on zero alpha, which is exactly how
    the Nether and End have none. A cloud carries no texture: `clouds.png` is a
    map, one texel per 12×12×4 cell, and the mesh is three bytes per quad the
    vertex shader expands from a fixed table. Weather forced `MOTION_BLOCKING`
    to stop being decoded-and-discarded. **Two witnesses caught real
    bugs**: a front-face convention that looked right from below alone (hence
    grading the deck from both sides), and — from the first live frame, not the
    gate — that vanilla's weather and cloud geometry is **camera-relative**
    while Rewo's `view_proj` already carries the camera, so the relative form
    draws every storm around the world origin. The gate had rendered at
    `[0,0,0]` where the two coincide; it now renders 2,500 blocks out. Wired
    into `rewo live` (both paths) with `REWO_FORCE_WEATHER=<rain>[,<thunder>]`
    as the headless knob.
  - **M33b — the rainy sky greys through `WeatherAttributes`, not
    `applyWeatherDarken`.** M33 shipped the latter and the sky stayed blue.
    26.2 puts weather's visuals in the **environment attribute system**
    (`world/attribute/WeatherAttributes.java`): RAIN/THUNDER layers rewrite
    SKY_COLOR (`BLEND_TO_GRAY`), FOG_COLOR (`MULTIPLY_RGB`), CLOUD_COLOR,
    SKY_LIGHT_LEVEL/COLOR/FACTOR, STAR_BRIGHTNESS (`set 0` — stars are removed,
    not dimmed) and SUNRISE_SUNSET_COLOR before any renderer reads them.
    `applyWeatherDarken` is a secondary touch-up on the SKY colour only; M33 had
    also applied it to the fog, double-darkening it. **The lightmap does darken
    in rain** — a `client/`-only grep for `getRainLevel` misses it because it
    arrives through the attribute system. The levels **partition**
    (`rain - thunder`), and THUNDER applies to RAIN's output. The **rain fog
    ramp** shipped with it — stateful (eases at `deltaTicks * 0.2`), gated by
    sky light (a cave is clear in a storm), half-strength in a dry biome. It
    needed a second, **environmental** fog band in the world pass: Rewo's
    existing band is a render-distance fade, vanilla's `total_fog_value` is the
    `max` of that and an environmental term, and only the latter is what rain
    thickens — applying the offsets to Rewo's tight band half-fogged the air ten
    blocks out. The new band lives in the `LightmapExtra` UBO (the push block is
    exactly at its 128-byte budget) and defaults to disabled. The `max` of the two
    bands is pinned by four pixel witnesses in **`lightmapshot`** (its camera is
    a known 16 blocks from the quad, so the fog fraction is exact) —
    mutation-verified against `min` and a sum.
  - **561 tests** (500 lib + 61 app); demo PNG byte-identical to M15 onward.
    Fourteen serverless gates, all green with Vulkan validation ON and 0 VUIDs:
    `mobshot` 243/243, `blockentityshot` 172/172, `swingshot` 97/97, `hurtshot`
    38/38, `weathershot` 35/35, `eventshot` 28/28, `itemshot` 28/28, `danceshot`
    24/24, `portalshot` 12/12, plus `skyshot`, `lightmapshot`, `tintshot`,
    `meshshot`, `dimensioncheck`. Live: `play --light-check` 884,736 cells / 0
    mismatches, `play --dimension-check` 4/4 + 3/3, physics CORRECTIONS 0.
- **M34 the inventory, and icons in the hotbar (2026-07-27)** — the client now
  knows what it is carrying and draws it. **Two coordinate systems** meet here
  and never line up: the wire's 46 **menu slots** (hotbar from 36, offhand 45)
  against the game's **inventory indices** (hotbar 0..8), and the three packets
  are split across them — `container_set_*` speaks the first, `set_held_slot`
  the second. Three non-obvious decode rules: an out-of-range held slot is
  **ignored, not clamped**; `container_set_slot` carries its index as a
  **signed short** among var-ints; any container id but 0 is an open screen
  this client hasn't got, so it's dropped whole. Icons needed `display.gui` —
  **absent for a sprite, which is correct** (identity maps 0..16 model units
  onto exactly the 16 px slot), `scale 0.625` + `rotation [30, 225, 0]` for a
  block (reaches 8.37 px against the slot's 8). GUI lighting is a **third**
  model, neither the world's `Direction` shade nor the hand's. Building the
  gate found two bugs first — `init_gui_items` leaked an image/sampler/pipeline
  per hotbar change, and the atlas was repacked every frame. Then the gate's
  own first measurement counted "non-black" pixels **against a painted sky**
  and measured exactly zero while the PNG showed both icons rendering
  perfectly; and one witness had its reasoning backwards ("a sprite covers more
  of its slot than a block" — a sword is mostly transparent), replaced by a
  mutation rendering the same block with an identity transform. Gate
  `rewo inventoryshot --check` **16/16**; **578 tests** (517 lib + 61 app);
  demo PNG byte-identical to M15 onward. **Open:** no inventory *screen* (the
  other 37 slots are held, never shown), no stack counts or durability bars.
- **M35 the inventory screen (2026-07-27)** — the panel, all 46 slots, the
  hover highlight, the stack on the cursor, and clicking. **The click is a
  prediction the server grades**: the packet carries the client's belief about
  every changed slot as a `HashedStack`, and the *only* resync trigger is
  `packet.stateId() != menu.getStateId()`. The first live click was rejected for
  exactly that — a harness bug, not a code one: it clicked while `/give` was
  still advancing the id. `tools/gen_item_props.py` extracts the two per-item
  facts the arithmetic needs, neither on the wire — `max_stack_size` (295 of
  1537 differ from 64) and `equippable`'s slot (83 items). Layout facts that are
  not guessable: `isHovering` is an **18x18** box (`left - 1 .. left + w + 1`),
  so slots tile without a dead column; the hotbar row is a named `top + 58`, not
  3x18; highlights are drawn at `slot - 4` at 24x24, **bracketing** the icon;
  the panel is centred by integer division. The backdrop is a **gradient**
  (0xC0101010 → 0xD0101010), not a fill. The one honest approximation:
  `isSameItemSameComponents` — Rewo knows *whether* a stack carried components,
  never what, so a patched stack swaps rather than merging (one-directional by
  construction; a wrong merge would fuse two tools, a missed one is corrected).
  **Measured, not squinted at**: the panel looked washed out with a black hole —
  six of seven probes are byte-identical to `inventory.png` (the seventh is the
  F3 overlay) and the black is the texture's own window, which vanilla covers
  with the 3D player. Gate `rewo inventoryshot --check` 16 → **39/39**, plus a
  live `REWO_CLICK` knob that counts container resyncs (the container
  `CORRECTIONS`); **586 tests**; demo PNG byte-identical to M15 onward.
  **Open:** the player preview is not drawn (the most visible gap); no
  shift-click, drag, number-key swap, Q-drop, tooltips, recipe book or
  durability bars; armour icons stay blank (their `select` trim definitions are
  among M22's 147 suppressed).
- **M36 the player preview (2026-07-27)** — the black rectangle M35 left in the
  inventory is `inventory.png`'s **own** window (vanilla paints it so the model
  has something to stand against), and this fills it. Transform =
  `PictureInPictureRenderer.prepare` then `GuiEntityRenderer.renderToTexture`:
  `T(w/2,h/2) . S(s,s,-s) . T(0,bbH/2+0.0625) . Rz(pi) . Rx(yAngle)`, with
  `s = guiScale * 30`. **The step that is easy to miss is on the CAMERA** —
  `orientation.rotateY(PI)` — and it is load-bearing: `bodyRot = 180 + xAngle`
  already points the model away from an unturned camera, so the first build
  rendered Steve's **back**. Rewo's entity pass takes no camera state, so the
  half turn goes on the model instead. The preview owns a **second
  `EntityPass`** (two `set_draws` into one vertex ring would cross the draws),
  built on first open, with its own atlas — hence its own skin upload, since a
  UV from the world's atlas would land on some mob's texture. It **clears depth**
  over its window (`vkCmdClearAttachments`, to **0.0** — reversed-Z; vanilla's
  `Projection.getMatrix` swaps `near`/`far` for the same reason) or the model
  comes out sliced by the terrain behind the panel. **Measuring beat squinting
  again**: the render looked too large and mispositioned; the measured feet
  (191.6 px down a 210 px window) and head (29.6) matched the decompile exactly
  — the size was right and the eye was wrong, and what *was* wrong was the
  facing. Gate `inventoryshot --check` 39 -> **44/44**; headless knobs
  `REWO_PREVIEW_SKIN=<username|url>` and `REWO_MOUSE=x,y`. **Open:** the model
  stands still (no local-player animation state); lighting is Rewo's entity
  shading, not vanilla's `ENTITY_IN_UI` rig; armour is not shown on it.
- **M37 particles (2026-07-27)** — the milestone REWO_PLAN §16 refused to
  propose, because every gate here is geometry-based and particles looked
  stochastic. They are not: **`Particle.tick()` contains no randomness at all**,
  every generator is `java.util.Random`'s 48-bit LCG, and a fixed seed turns
  spawn offset, velocity, lifetime, colour, quad size and sprite index into
  assertable numbers. Two anchors stop that being circular, and they retire
  different failure modes — the JDK's own `Random` is genuinely independent
  ground truth for the generator (MC's `BitRandomSource` reimplements its
  formulas), and a Java harness of **verbatim decompile source** grades the
  physics, which is the only thing that can catch a *misreading* rather than a
  mistranslation. It caught `+ 0.1` where vanilla writes `+ 0.1F` on its first
  run — a ~1.5e-9 error, invisible in any screenshot, that shifted every
  subsequent tick. `nextGaussian` is the one primitive graded to a **ULP bound**
  instead of to the bit, because `Math.log` is a JIT intrinsic spec'd only to
  1 ULP, so vanilla's own spawn scatter is not bit-reproducible between two
  JVMs and a zero-tolerance gate there would assert more than vanilla
  guarantees. Six kinds (block, smoke, flame, splash, crit, poof); a block-break
  shard samples the **block** texture and a flame the particle strip, unified
  into one `sampler2DArray` so both share a pipeline; `BakedAssets::
  particle_layer` resolves each state's model `#particle` slot, which is why a
  broken grass_block throws *dirt*-coloured shards. Gate **`rewo particleshot
  --check` 34/34**, mutation-tested against five breakages. Verified live: the
  shard colour **tracks the block state** (redstone red / lapis blue / gold
  yellow), and a real `/setblock … air destroy` spawns exactly **64** shards —
  the 4×4×4 grid the gate asserts from the other direction. **610 tests**; demo
  PNG byte-identical to M15 onward.
- **A frame-diff witness must hold everything but the subject constant, and a
  world-mutating trigger cannot** (M37, not particle-specific). Measuring the
  block-break shards' colour by frame-diff gave 0.04 chromaticity agreement with
  an explicit `block{grass_block}` particle on one run and 0.16 on the next, so
  the first figure was **retracted rather than defended**. `/setblock … air
  destroy` changes the world: the removed block covers thousands of pixels, the
  shards spawn inside the volume it vacated, and the two frames differ in
  lighting *history* (one relit incrementally from a client edit, the other given
  the server's light at chunk load). A same-world control removes the largest
  term but not all of them. **The diagnostic tell, both times, was that
  restricting to strongly-changed pixels made the discrepancy WORSE** — that is
  the signature of a contaminated control, where edge-blending would have
  improved. Measure such a path by a property that does not need a clean frame
  diff (here: the spawn *count*, and the texture resolution the non-mutating
  `/particle block{…}` rows already exercise).
- **M38 the first-person hand (2026-07-28)** — the blocker §0.0 named was M34's
  inventory model, and with it gone the hand went in: the held item through
  both geometry paths, the swing, the equip dip, the view sway. **Two bake
  rules are invertible** — an absent `firstperson_lefthand` falls back to the
  **right** entry and *only* in first person (`ItemTransforms`' builder has that
  line; the third-person pair has none), and the left/right **mirror is applied
  at draw time, not baked** (`ItemTransform.apply` negates `translation.x`,
  `rotation.y`, `rotation.z`; `handheld` authors its left pre-mirrored so the
  two cancel, and baking it would double it). **The swing clock is not a new
  machine**: `LocalPlayer` is an ordinary `LivingEntity` and `Player.aiStep`
  calls `updateSwingTime`, so it takes an id in M19's swing table —
  `tick_swings` iterates the swing map, not the entity map — and M34's
  inventory supplies the held item the duration needs, which is the real join
  between the two milestones. **Two clocks, easily conflated**: `attackAnim` is
  the entity's; the equip height is `ItemInHandRenderer`'s own and ticks per
  *tick*, not per frame. **The hand has its own projection** —
  `calculateHudFov` returns a hard-coded **70** vertical — and vanilla
  **clears depth** before drawing it, without which a wall a block away slices
  your arm off. Three things measured not assumed: the arm chain's translates
  are block units with cube vertices divided by 16 (with it the arm lands
  1.1 blocks below the eye; without it, ten blocks away), the item quads really
  are 0..16, and the pass is the GUI-item pass with two differences (a
  view-projection push constant, the world's flipped viewport). **The 1.36x was
  not there**: the render was first committed with that unexplained width
  discrepancy, bisecting showed the geometry matched a hand derivation to a
  tenth of a pixel, and the fault was the **detector**, which was also counting
  the hotbar's dirt icons — re-measured cleanly, every edge lands within a
  pixel. That was the **third detector error of the milestone**, all the same
  shape (non-black against a painted sky, brown against a brown hotbar, cyan
  against a blue sky), so `handshot` is built around avoiding the class: a
  synthetic **magenta** cube, with an empty frame asserted to contain none.
  Gate `rewo handshot --check` **22/22** (two of its own witnesses were wrong
  first — the fallback check used a *stick*, which parents `item/handheld` and
  authors both hands; and the fail-closed count caught 19 declared as 17).
  **The bare arm** draws for the main hand only, from one named part with
  `resetPose()` plus a fixed `zRot` of ±0.1 rad. It rendered as *nothing* at
  first because **the model's UVs are texels, not fractions** — an arm's span
  16..56 of a 64 px skin, so remapping without dividing by the skin size sends
  them outside the atlas, where the sampler clamps to a transparent edge. The
  geometry was there the whole time (72 verts, uploaded), so looking proved
  nothing and printing the UV range settled it in one run. **623 tests**; demo
  PNG byte-identical to M15 onward. **The use-driven poses** landed with the plumbing they
  needed: right-click became a hold (`use_item` + `RELEASE_USE_ITEM`), and the
  local use clock needed **no new machine** — `startUsingItem` sets shared-flag
  bit 0, which `set_living_flags` already decodes, so the local id goes through
  the same door M23 built. Three invertible details: **`hasCustomArmTransform`
  moves a transform rather than adding one** (true for EAT/DRINK/SPEAR — the
  resting offset applies *after* the pose), the **brush cycles on
  `remaining % 10`** rather than on progress through a duration, and **BLOCK
  excepts a real shield** (it carries its own display transform and would be
  posed twice). Spyglass is the absence of a pose — vanilla guards all of
  `submitArmWithItem` on `!isScoping()`. Two gate witnesses failed first for one
  reason: **`transform_point3(ZERO)` sees only translation**, so a trailing
  rotation (the brush sweep) measures as motionless — sample an offset point.
  Gate **29/29**. **Open:** `SPEAR`'s use rig and the crossbow charge need
  inputs the wire does not carry; the arm wears the default skin.
- **M39 shift-click, the quick-move (2026-07-28)** — `ContainerInput.QUICK_MOVE`
  is a **different input**, not a modifier on PICKUP. **The routing is not "the
  other half of the inventory"**: `quickMoveStack` checks armour and the
  off-hand *first*, for an item that fits and whose target is **empty** — which
  is why shift-clicking a helmet equips it, and why a second helmet does not
  swap the first out. The crafting result is the one destination walked
  **backwards**, so a craft fills the hotbar from the right.
  **`moveItemStackTo` is two asymmetric passes**: the merge pass runs the whole
  range, the placement pass takes one empty slot and **breaks** — so a stack
  tops up a partial one before taking an empty, but never scatters across
  several. `doClick`'s outer `while` is what repeats it. Gate
  `inventoryshot --check` 44 → **49/49**, plus a live check: shift-click and
  plain click both accepted with **0 container resyncs**. **Open:** tooltips
  (needs `en_us.json` + text layout), drag/quick-craft, number-key swap,
  Q-drop, durability bars, armour icons.
- **M40 the rest of the inventory screen (2026-07-28)** — armour icons,
  tooltips, and every remaining interaction. **The suppressed items were
  suppressed for the wrong reason**: M22 called the five non-`model`
  definition types state-dependent, but **all 71 `select`s carry a
  `fallback`** and every `condition` an `on_false`, so for a component-free
  stack those *are* the answer, not a default. The rule is suppress the
  **property** you cannot evaluate, not the type — **1,390 → 1,438 resolved,
  147 → 99 suppressed**. The reduction must recurse (a bow is a `condition`
  whose `on_true` is a `range_dispatch`), and `display_context` selects
  different **geometry**, not a transform (a spear is a flat sprite in a slot
  and a 3D model in the hand), hence `HeldItemModel::gui_quads`. A witness
  caught the diagnostics naming the definition's *root* type rather than the
  node the walk stopped at. **Tooltips** are one line — the display name from
  the jar's `en_us.json`, preferring `block.minecraft.<id>` because
  `BlockItem` overrides `getDescriptionId` (`item.minecraft.dirt` does not
  exist); everything vanilla adds beyond it comes from a component Rewo cannot
  read. Layout traps: the height starts at **-2 for a single line**; the
  horizontal recovery is a **flip** whose `x` is the **already-offset** one
  (my witness expected 306, the answer is 318); the vertical is a clamp using
  `h + 3`. **The interactions**: `SWAP`'s button is a **third coordinate
  system** (an inventory index, 0..9 or a literal 40) and its range **rejects
  rather than clamps**; `THROW`'s trailing `while` never runs twice;
  `PICKUP_ALL` is two passes whose first **skips full stacks**, gated on the
  clicked slot being empty; `QUICK_CRAFT` packs `type << 2 | header` into one
  byte, is three packets, and **a one-slot drag collapses into a `PICKUP`**.
  Gates `inventoryshot` 44 → **70**, `itemshot` 28 → **33**; all four
  interactions live-verified with **0 container resyncs**. **Blocked, not
  skipped:** durability bars and enchantment/lore tooltip lines need the
  *contents* of a `DataComponentPatch`, which Rewo does not decode.
- **M41 the `DataComponentPatch` decode (2026-07-28)** — the blocker every
  milestone since M35 named. **The patch has no length prefix**: each entry's
  value uses that component's own stream codec, so an untranscribed one cannot
  be *skipped* — the reader parks mid-value and the rest of the packet is
  garbage. That is why M19 knew 3 of 111 codecs and treated the rest as fatal.
  Nearly all 104 syncable codecs compose from a dozen primitives, so
  `rewo-net/src/component_wire.rs` writes them as **data** (a `Shape` tree per
  component) and one interpreter walks them: **97 of 111 transcribed**, and 7
  of the 14 remaining are **never network-synchronised**, so there are 7 real
  gaps. Wire facts that read backwards: **a chat component is one NBT tag**
  (`fromCodecWithRegistries` — which makes `custom_name`/`item_name`/`lore`
  walkable with no chat codec at all); **`Unit` is zero bytes**; **`holderSet`'s
  var-int is `count + 1` and a literal 0 means a *tag name* follows**, not an
  empty set; `holder` is `id + 1` with 0 = inline while `holderRegistry` is
  raw; `either` writes **true for the left**. A sorted digest of every entry's
  (type id, raw bytes) makes **`isSameItemSameComponents` exact** — M35 could
  only ask "carries components at all", so every patched stack swapped and two
  identically-enchanted books could not stack; a *removal* folds in its id
  because `getOrDefault` answers it with the type's default, not the item's
  prototype. **Durability bars**: `round(13 - damage * 13 / max)` **counts
  down**, colour is `hsvToRgb(health / 3, 1, 1)`, the draw is a 13x2 black bed
  under a 1px bar, and `isBarVisible` is `isDamaged()` so a pristine tool has
  none; only the numerator is on the wire, so `gen_item_props.py` grew a
  `max_damage` column (84 items). Tooltips gained the name override, lore and
  `Unbreakable`. **Two witnesses caught real bugs** — the tooltip box was drawn
  in panel space while its text was in screen space (and `t4` had agreed with
  the implementation until rewritten to bracket the *text*), and `swingshot`'s
  "unwalkable" fixture named `enchantments`, which M41 transcribes, so it
  silently stopped testing its claim (now an impossible id). Gate
  `inventoryshot` 70 -> **79**; **628 tests**; live: named stacks **merge** and
  differently-named ones **swap**, 0 container resyncs. **Open:** the
  enchantment registry (a datapack registry Rewo does not decode) blocks the
  enchantment tooltip lines and the glint.
- **M42 the enchantment registry (2026-07-28)** — M41's other half. The
  registry **has to come from the wire**: `minecraft:enchantment` is a
  **datapack** registry, so its contents *and its id order* are the server's,
  and it arrives in Configuration's `registry_data` (kept in wire order — the
  index **is** the protocol id, the same rule M16 records for dimension types).
  `max_level` is **top-level in the entry compound, not nested under
  `definition`**, because `EnchantmentDefinition.CODEC` is a `MapCodec` whose
  fields inline into the parent. The **strings and tags come from the client
  jar** — `en_us.json` plus `data/minecraft/tags/enchantment/{curse,
  tooltip_order}.json`, the vanilla datapack the jar carries (where M19 already
  reads `ItemTags.SPEARS`). Three `getFullname` rules: the level numeral is
  suppressed **only when `level == 1 && maxLevel == 1`** (so a level-1 Mending
  has none and a level-1 Sharpness does — suppressing on `level == 1` alone
  loses it from every single-level enchant applied); a curse is **red**; and
  the order is the **`tooltip_order` tag**, then the rest. An unsynced id
  yields **no line**. **The render caught a bug**: `SlotText::is_empty` gates
  whether a stack's text is recorded at all and had not been taught the new
  field, so a stack carrying *only* enchantments looked empty and was dropped.
  Gate `inventoryshot` 79 -> **85**; **629 tests**; live, a four-enchantment
  sword renders curse-first-and-red, `Sharpness V`, `Unbreaking III`,
  `Mending` with no numeral. **Open:** the glint (a second render pass, not a
  tooltip concern).
- **M43 the enchantment glint (2026-07-28)** — a **second pass over the same
  geometry**, and almost all of it is state rather than new maths.
  `setupGlintTexturing`: two offsets on **110 s and 30 s** periods (so the
  pattern never visibly repeats), u **negative** and v positive (which sends
  the sheen diagonally), and the cast to `long` **before** the modulo. **JOML
  post-multiplies**, so `translation().rotateZ().scale()` reads as
  scale-then-rotate-then-translate on the coordinate — the reverse of the call
  order. Scale **8.0** for an item, 0.5 entity, 0.16 armour. **The UV fed in is
  the quad's own `0..1` coordinate, not its atlas position** — otherwise the
  pattern depends on where the packer put the item. Three pieces of pipeline
  state, each load-bearing: `BlendFunction.GLINT` is `(SRC_COLOR, ONE, ZERO,
  ONE)` so a dark texel adds nothing and alpha is left alone (the headless
  gates read it back); depth **EQUAL with no write**, which lands the sheen on
  the item's own fragments and nowhere else; and **REPEAT + LINEAR** sampling,
  because scale 8 samples far outside `0..1` and the `.mcmeta` sets
  `blur: true`. The phase is **wall-clock**, not the tick. **The render caught
  a bug**: `hasFoil()` is *not* `isEnchanted()` — `ENCHANTMENT_GLINT_OVERRIDE`
  wins **both ways**, so a golden apple can glint and a Sharpness V sword can
  be told not to. **And three `item_stack` fixtures rotted the same way
  `swingshot`'s did in M41** — they named a real-but-uncovered component id as
  their "unknown codec" and M43 gave it one; both now use an *impossible* id.
  Gate `inventoryshot` 85 -> **91**; **630 tests**; live, all four `hasFoil`
  cases correct and the sheen moves (311 of 2,500 slot pixels differ across
  seven seconds). **Open:** the glint on the first-person hand, on ground /
  mob-held items (scale 0.5) and on worn armour (0.16).
- **M44 the glint on the first-person hand (2026-07-28)** — M43's transform,
  blend, depth rule and sampler unchanged (the item scale is 8.0 in both
  contexts), so the milestone is about where the second pass hangs. **The
  glint geometry has to be the item geometry to the bit**: the pass
  depth-tests `EQUAL` against what the hand pass just wrote, so a vertex a
  fraction of a unit away is rejected fragment by fragment and draws nothing —
  the glint builder repeats the pose derivation (use branch, swing branch,
  display transform, left-hand mirror) rather than re-deriving it a second,
  subtly different way. **Only items glint** — the bare arm is skin, and
  `submitArmWithItem` takes the arm branch before any foil. `hasFoil` comes
  from the **inventory**, not the equipment feed, because a server never sends
  a player their own equipment. **The bug**: the first build drew nothing
  because `init_hand` *destroys and rebuilds the pass*, and the glint was
  installed **before** it, so every rebuild threw it away — no error, no
  warning, no validation message; a rebuilt pass with no glint is perfectly
  valid, and the only signal was two frames that should have differed and did
  not. Gate `handshot` 29 -> **34**; **635 tests**. **Open:** ground and
  mob-held items (scale 0.5) and worn armour (0.16), both through the entity
  pass.
- **M45 the glint on world-space items (2026-07-28)** — ground stacks and
  mob-held ones. `ENTITY_GLINT_TEXTURING`'s scale is **0.5** against the item
  contexts' 8.0, a factor of sixteen, so a dropped sword wears broad bands
  where an icon wears a fine weave. **Worn armour is the fourth surface and is
  not reachable**: Rewo renders no armour on any entity, so the 0.16 scale has
  nothing to apply to — the glint is complete for everything Rewo draws. The
  glint quads are pushed **from inside the two item emitters**, beside the
  vertex they shadow: the pipeline depth-tests `EQUAL`, and a dropped stack
  carries a death topple, a bob, a spin and a per-copy jitter, so a parallel
  derivation would have four more chances to disagree. It is a **third vertex
  range** (solid, text, glint) drawn after the solid pass and before the
  translucent ones, with **no lightmap term** — vanilla's glint shader
  multiplies by `GlintAlpha` and the fog fade and nothing else, so a dropped
  enchanted sword shimmers as brightly in a cave as in daylight. `hasFoil`
  rides in with the stack (on `HeldItem`, and in the `DATA_ITEM` metadata
  tuple) because it exists only in the component patch. **The gate measured
  zero and was right to**: `itemshot` calls `init_entities` directly rather
  than through the app's helper, so it never installed the glint — the same
  shape as the `swingshot`/`install_shapes` gap M41 hit, and the general rule
  is that *a gate reimplementing a slice of the app's setup will miss whatever
  the app adds to it*. `entities.rs` is also one of the **mixed CRLF/LF** files
  §0.0 warns about (1,969 CRLF against 3,763 LF), so the scripted edits had to
  match either ending. Gate `itemshot` 33 -> **37**; **629 tests**.
- **M46 worn armour (2026-07-28)** — M45 called this "the fourth surface and
  not reachable"; this makes it reachable. An item names an **asset**
  (`Equippable.assetId()`, in the prototype, never on the wire — so
  `gen_item_props.py` extracts it), and the asset names **layers** whose
  textures are **64x32** sheets, not 64x64 skins. Only two humanoid layers
  exist because `usesInnerModel` is `slot == LEGS`: the leggings sit *inside*
  the chestplate at deformation 0.5 against 1.0, which is what stops them
  z-fighting. **The body is in two pieces at once** — CHEST covers
  `{body, both arms}` and LEGS covers `{both legs, body}` — and the leg boxes
  are a **replacement**, `texOffs(0,16)` at `extend(-0.1)`. The armour is posed
  from the **same `xf` the body just used**, since it is a render layer over a
  model whose angles are already set. **The layer follows the RENDERER, not the
  mesh**: all eight `HumanoidArmorLayer` sites are player/zombie/skeleton/
  piglin families, so an **allay** (arms, no legs), an **illager** and a
  **creaking** — each with enough humanoid mesh to pass a geometric test — wear
  nothing in vanilla. **Only the player has a `body` part** (M19 gave it one
  for `setupAttackAnimation`); every mob's torso cube is on the static root, so
  a chestplate's body box resolved to nothing and mobs wore armoured arms over
  a bare chest — **and the witness passed anyway, because it asked the player
  model**, the one humanoid with the named part. A **trace beat four
  screenshots**: several rounds of squinting at crops (one of which was a husk,
  another comparing two live runs whose scenes had drifted) never settled
  whether the arms were armoured; logging which part each box resolved to
  answered it in one run, and the bare green mass every crop had been read as
  "arms" was the **torso**. An armoured zombie also rendered with a villager's
  texture, which looked exactly like an atlas collision from the fifteen new
  sheets — it is **pre-existing** (a stashed pre-M46 build reproduces it), needs
  more than one entity in the scene, and `mobshot` is structurally blind to it
  because its check substitutes per-face debug colours and so verifies UV/face
  correspondence rather than which *sheet* is sampled (recorded in §0.0). Gate
  `itemshot` 37 -> **42**; **629 tests**. **Open:** leather is undyed (a layer
  is a *list* — dyeable base plus overlay — and Rewo takes the first, so the
  greyscale base is never tinted by `dyed_color`), no trims, the inventory
  preview does not wear its armour, and baby mobs use the adult parts.
- **M47 the leather dye (2026-07-28)** — M46 shipped leather grey and called it
  "the dyeable base drawn untinted"; both halves were wrong. **Zero is not a
  black tint, it is "do not draw this layer"** — `renderLayers`' guard is
  `if (color != 0)`, and that is the entire implementation of
  `Layer.onlyIfDyed`, whose `Dyeable` carries *no* `color_when_undyed`. Three
  states hide behind one `Optional<Dyeable>` (absent = untinted always,
  present-with-a-colour = tinted always, present-without = only when dyed), so
  it survives as `Option<Option<u32>>`. **An undyed leather piece is brown, not
  grey**: `LEATHER_COLOR` is `0xA06540`, and the sheet is authored greyscale
  *because* it is always tinted — there is no path that draws it untinted. A
  layer type maps to a **list**: surveyed on the jar, 20 humanoid lists of one
  and 3 of two, all three of them leather's (a dyeable base plus an untinted
  overlay, which is what keeps the studs their own colour on a dyed piece).
  `DyedItemColor`'s stream codec is **`ByteBufCodecs.INT`** — a fixed
  big-endian i32 among the var-ints, M34's trap again — holding an **RGB**,
  which is why `getOrDefault` is the thing that calls `ARGB.opaque`, and why an
  absent dye is `0` while a *black* dye is `0xFF000000`. The tint is a **vertex
  colour** (`submitModel(..., color, ...)`; `entity.fsh` does
  `texture * vertexColor`), riding the same channel as the directional shade,
  so untinted is exactly `tint = 1`. **The pixel witness caught a key-format
  break**: `d4` measures red/green and red/blue over the armour's own pixels,
  and its first run measured **zero** — correctly, because M47 changed the
  atlas key to `<layer>/<texture>` while the renderer's slot filter still
  looked for `"/humanoid"` as a substring, so **all** armour had gone
  invisible, not just leather. Gate `itemshot` 42 -> **46**; **631 tests**.
  **Open:** no trims, the glint-order rule is transcribed but unreachable until
  armour glints, and `usePlayerTexture` (the elytra cape) is read as data and
  never honoured.
- **M48 armour trims (2026-07-28)** — the third armour layer, and the one that
  is **not a texture in the jar**: `armor_trims.json` declares a
  `paletted_permutations` source and the client generates every
  `pattern x material` sprite at load by swapping colours through a palette
  pair. Two invertible details — the match is on **RGB with alpha masked off**
  (so a half-transparent pixel of a palette colour still maps, taking
  `pixelAlpha * valueAlpha / 255`), and an **unmatched pixel is not dropped**,
  because `getOrDefault` returns `opaque(pixelRGB)` whose alpha 255 leaves it
  untouched. Working in RGBA bytes sidesteps whether `NativeImage.getPixels` is
  ARGB or ABGR. `trim_material` and `trim_pattern` are two more **datapack**
  registries (M42's rule: contents *and* id order are the server's; index = id),
  and their `MapCodec`s inline, so `asset_name`/`override_armor_assets` are
  top-level fields. **`assetId(equipmentAsset)` is what stops a trim
  disappearing**: it is `overrides.getOrDefault(equipmentAsset, base)`, keyed by
  the *equipment asset*, and iron/gold/diamond/netherite/copper each override to
  `<material>_darker` for their own armour — else an iron trim paints iron onto
  iron. The trim draws with **depth EQUAL, no write**
  (`ARMOR_DECAL_CUTOUT_NO_CULL`), M43's glint trick, and it is the only sane
  option: Rewo's reversed-Z `GREATER` would reject a coplanar redraw outright.
  Vanilla's two pipelines (`decal` vs not) **collapse to one here** because the
  trim's geometry is the armour's to the bit. It is a **fourth vertex range**
  (`solid | text | glint | trim`), drawn under the foil as vanilla does. 612
  possible sheets means a **demand-filled pool** (M22's item-pool arithmetic
  again): 64 slots, keyed by sprite path; `ATLAS_H` grew 1280→1408 with the
  pool at the **top** and the skin/item pools redefined downward, so every
  existing address is unchanged and `mobshot` stayed 243/243. **A leak the
  gates caught**: the new pipeline was never destroyed, and
  `VUID-vkDestroyDevice-device-05137` fired in three gates with **zero failed
  witnesses** — the 0-VUID bar caught what a green witness count could not.
  Gate `itemshot` 46 → **51**; **633 tests**. **Open:** trims are not on GUI
  icons (M40 suppresses the `select` property it cannot evaluate), no
  `humanoid_baby` layer, and a trim does not glint.
- **M49 trims on GUI icons (2026-07-28)** — the blocker M48 named. The icon is a
  **`select` on `minecraft:trim_material`** whose `when` values are material
  **registry ids** (not the `asset_name` suffix the worn sheet uses), each case
  naming a different model — 337 of them, each an ordinary two-layer
  `item/generated`. Its layer1 sprite comes from a **second** paletted-
  permutations atlas (`items.json`, four 16x16 sheets, same key palette and
  same sixteen permutations as `armor_trims.json`), so M48's `apply_palette`
  was already the whole generator. **The bake refactor**: `ItemModels` was
  keyed by item name and baked once, so a variant goes in under
  **`"<item>#<material id>"`** rather than the key becoming a pair — every
  existing lookup is untouched, and `HeldItems::any` falls back from a composed
  name to the base, which is required and not a nicety (an item can be trimmed
  with a material its own definition names no case for, and vanilla's answer
  there is the `fallback`). Variants come from the definition's own `cases`,
  not the material registry, because this is a bake of the **jar** and the
  registry is the **server's**. **The bug that hid the whole feature**:
  everything resolved and the icons still rendered plain, because a multi-layer
  sprite is **coplanar by construction** (`ItemModelGenerator` puts every layer
  in the same `z 7.5..8.5` slab) and the GUI pipeline depth-tested strict
  `GREATER`, rejecting layer1 at exactly layer0's depth. Vanilla tests `LEQUAL`;
  the reversed-Z counterpart is **`GREATER_OR_EQUAL`** — one word. That is the
  third time this arc a depth *comparison* was the whole story, so it is worth
  reaching for first when geometry is provably present and provably invisible.
  Gate `itemshot` 51 → **54** (`u1`: a variant bakes one more sprite layer than
  its base; `u2`: an unnamed material falls back to the base's single layer);
  **633 tests**.
- **M50 the worn-armour glint, and the glint's colour space (2026-07-28)** —
  M45 called worn armour "the fourth surface and not reachable"; M46 made it
  reachable and this draws it. **Two of the facts gathered in advance were
  wrong.** `VIEW_OFFSET_Z_LAYERING` is not the foil's mechanism: all three
  armour render types carry it (`ARMOR_CUTOUT_NO_CULL`,
  `ARMOR_DECAL_CUTOUT_NO_CULL`, `ARMOR_ENTITY_GLINT`), each with the same bias
  on a fresh `getModelViewMatrixCopy()`, so it **cancels within the stack** —
  what it separates is armour from *body* — and `RenderPipelines.GLINT` is
  `DepthStencilState(CompareOp.EQUAL, false)`, so the foil is the same
  depth-EQUAL pass Rewo had shipped three times. And the foil is **untinted**:
  `POSITION_TEX` has no Color element, `glint.vsh` declares no colour
  attribute, and `writeDynamicTransforms` passes `ColorModulator` as WHITE, so
  `submitModel`'s colour is dropped. The fact that held is the headline —
  **the trim must not glint**, because `renderLayers` clears `renderFoil`
  inside the layer loop and submits the trim after it. **Then the real
  finding**: the foil went in structurally correct and rendered a byte-delta of
  **exactly 0**. `BlendFunction.GLINT` is `(SRC_COLOR, ONE)`, so the
  contribution is `src²` — and **squaring is not invariant under the sRGB
  transfer function**. Vanilla evaluates it in gamma space (no sRGB framebuffer,
  no sRGB texture views); Rewo was blending in linear, where a mid texel adds
  +0.9/255 against vanilla's +16/255 and quantises away. **The item glint had
  the same error since M43** and hid it, because a dropped stack sits against a
  *dark* background where the sRGB curve is steep enough to show a tiny linear
  increment (measured on one frame: item 137, armour 0). No fixed-function
  blend can bridge it — every candidate needs to read the destination — so the
  glint now renders through a **UNORM view of the same image**
  (`MUTABLE_FORMAT` + format list on the offscreen image and the swapchain,
  `world::draw` reopening its scope around each glint draw, sheets uploaded
  UNORM), and both glint shaders are vanilla's line verbatim. **Without
  `VK_KHR_swapchain_mutable_format` no glint is drawn at all** — check that
  first if glints ever go missing. Structure: one `EntityGlint` per sheet over
  one shared pipeline, a fifth vertex range
  (`solid | text | glint | trim | armor_glint`), and the foil drawn **before**
  the trim because `SubmitNodeStorage` drains its phases in ascending `order`.
  Gate `itemshot` 54 → **62**; **633 tests**; demo PNG byte-identical to M15
  onward. **Three detector errors, all mine** (M38's pattern again): a `> 8`
  threshold built for the item glint read a real 5/255 sheen as nothing; a
  per-channel linear comparison sat below the 8-bit quantisation step; and the
  first fixture used two *bright* dyes whose red and green pinned at 255. The
  fix moved the measurement into the space the blend now works in — vanilla's
  add is base-independent **in bytes**, and the byte delta between two opposite
  dyes comes out **0**. **The live frame-diff was rejected as an oracle**: a
  same-item control differed in 41,284 pixels against the test's 16,329, so the
  wire path was verified by a *property* instead (`ench=[(28,4)]` decoded,
  `foil=true` at the renderer). And the first live run failed on the
  **harness**: the summon used the pre-1.21.5 `enchantments:{levels:{…}}`
  wrapper, so 26.2 silently produced an unenchanted piece — same shape as M35's
  stale state id and M20.1's build gate.
### The Velvet type stack, the visual freeze, and four headless subsystems (2026-07-28)

Pushed as `4c0fd6b..f7901f2`. Everything here is **headlessly verified** — the
demo PNG stayed `2cc56b4acbfb92cb` through all of it, which is the check that
none of it changes a rendered pixel.

**The module port (`M52a`).** The survey's top-ranked item. Full Bright, FOV
Control, Zoom, Toggle Sprint, Toggle Sneak, in `crates/rewo-app/src/modules.rs`.
The catalog is **not** redefined — `ewo_core::modules::REGISTRY` already calls
itself the single source of truth, so Rewo is its third reader; only `rewo-app`
takes the dep and `rewo-gpu` keeps taking plain floats. Config reads the **same
`profiles/<active>/modules.toml` the launcher writes**, so Settings → Modules
applies to a Native instance with no new contract.

Three invertible details: Full Bright pins vanilla's **maximum gamma** rather
than bypassing the lightmap, so night vision and darkness keep composing (a
bypass would silently defeat both); Zoom **divides** whatever FOV is in effect
rather than setting one, so it composes with FOV Control; Toggle Sprint/Sneak
guard on `!event.repeat`, or a held key flips the state dozens of times a
second.

**Two modules are vacuous in Rewo** — `no_view_bob` and `no_damage_tilt`
disable behaviours Rewo never implemented. They are absent from `RenderModules`
rather than wired to a no-op, with a test asserting toggling them changes
nothing. To port the disable you must first build the thing being disabled.

**The Velvet type stack (`M52b`).** See `REWO_VELVET_UI_PLAN.md`. Glyph cache
(`swash`, quantized key, shelf atlas, variable axes, blurred shadow glyphs),
text pass, SDF chrome pass, one widget (Coords), and `rewo hudshot --check`
(41 witnesses, mutation-verified). Load-bearing facts:

- **Rasterize-and-cache, not MSDF.** The fidelity target is pixel-faithful
  against the Skia originals and SDF reconstruction approximates the outline.
- **The key is quantized** (1/8 px, 1/2 axis unit) because an unquantized size
  mints a scaler per frame of a scale drag — the shape of the 2026-05-31 leak.
- **`swash`'s `linear_scale(s)` multiplies by a FACTOR; `scale(ppem)` divides
  by units-per-em.** Using the former returns font units: Fraunces' cap height
  read 25200 instead of 12.6 and every advance was ~1400× too wide. An
  assertion of `> 0.0` accepted it; only a two-sided bound caught it.
- **Six Skia `draw_rrect` calls collapse to one fragment shader**, because a
  mask blur over a rounded rect is a smoothstep over the SDF — no blur pass.
- **The Velvet passes must be built with `world::unorm_of(target_format)` and
  drawn inside `with_gamma_space`.** EwoClient's `rgba()` has no transfer
  function, so Skia composites in gamma space; an sRGB attachment blends in
  linear. The half that actually bites is the **pipeline format**: a mismatch
  is a validation error, not a subtle colour shift.

**The visual freeze.** Four steps in, the scope was cut back: the type stack
lands, the widget transcription **stops at one**, the editor is not started,
and the palette is de-baked **now** while it is one shader and one widget.
Reason: the HUD is getting a visual overhaul and anything transcribed now
would be redone. The music terms deliberately stayed structural — `border.a`
is the *resting* alpha and the drive gains scale from it, so a new palette
recolours without flattening the reaction.

**Tooltips through the Velvet pass (`M52b`).** The tooltip line went from
`(String, [f32;3])` — one string, one colour, nowhere to put "italic" — to
`tooltip::Line = Vec<Span>`. The fidelity gain is lore: `ItemLore.LORE_STYLE`
is `withColor(DARK_PURPLE).withItalic(true)` and Rewo had the colour right and
dropped the slant, because the type could not hold it. The `Span` type is
**font-agnostic** on purpose, so it outlives the visual direction.

The half that is easy to skip is **measurement**: once the tooltip draws in
Newsreader, sizing its box with the bitmap advances measures a font it no
longer uses. Also: vanilla's tooltip `y` is the line's **top** and Velvet lays
out from the **baseline**, and the atlas sync must run *before* the draw and
*outside* the rendering scope. The headless path deliberately passes `None`
and keeps the bitmap tooltip, so the gates' golden images are not moved by a
typeface change unrelated to what they test.

**Ping (`M52c`), and a correction.** The spec claimed the client could time a
keep-alive round trip. **It cannot** — `keep_alive` and `ping` are
*server-initiated* probes; the server sends, the client echoes, and the
**server** times it. A client cannot measure RTT from a packet it did not
initiate, and the play protocol gives it nothing to initiate. Vanilla's tab
list does not compute a ping, it displays one it was told. So the only source
is `UPDATE_LATENCY` on `player_info_update` — which Rewo was already decoding
and discarding as `let _latency = r.varint()?;`. One line was the whole gap.
A **negative latency is a state**, not a decode error (`PlayerTabOverlay`
buckets `< 0` into the no-connection icon), and `None` ≠ `Some(0)`.

**Chat styling (`M52d`).** `chat_style.rs` — legacy `§` codes and component
trees into styled runs, renderer-agnostic. (**It lived in `crates/rewo-net/`
until M126 moved it to `crates/rewo-world/`**, because `rewo_world::chat` has
to name `ChatSpan` and the dependency runs net → world; `rewo-net` re-exports
it, so the old paths still resolve.) Six rules a
plausible implementation gets silently wrong, each pinned: a **colour code
clears the five format flags** (`§c§lX` is bold red, `§l§cX` is plain red);
**`§r` resets to the enclosing style, not white**; an unrecognised code
consumes **both** characters; an explicit `false` beats an inherited `true`;
a `#` colour is `Integer.parseInt(_, 16)` **not CSS**, so `#f00` is `0x000F00`;
and a top-level list makes element 0 the **parent** of the rest.

**Component codecs + a latent bug (`M52e`).** The last 7 syncable
`DataComponentPatch` codecs. The gaps were fatal rather than cosmetic because
**the patch has no length prefix** — an untranscribed component cannot be
skipped. `can_place_on` needed a new primitive: it reaches
`TypedDataComponent`, which is the patch's own rule a second way.

It exposed a **latent bug in M41**: `MAX_DEPTH` charged the budget for every
combinator including static ones, free only because nothing was deep enough to
notice. `can_place_on` is — a legitimate adventure predicate would have
reported `Stuck` and cost the rest of its packet. Only recursive shapes charge
depth now. The 7 non-syncable components are **named in a test, not counted**,
so a version that starts syncing one fails as a missing codec.

**Tab list (`M52f`) and chunk cache (`M52g`).** Both model-only, nothing wired.
The tab list transcribes `PlayerTabOverlay` — cap 80, `MAX_ROWS_PER_COL` 20,
the four-key comparator (with `wrapping_neg`, because `-Integer.MIN_VALUE`
wraps in Java), the column-search loop, and the ping buckets. The chunk cache
is a Bobby-style store with a **version check by equality, not `>=`**, so a
downgrade cannot misread a newer file; `Container`/`Section`/`Column` fields
went `pub(crate)` so the encoder **destructures** — adding a field breaks the
build rather than silently writing an entry that decodes into a plausible
column missing the new state.

**Known limits, all recorded:** none of the four subsystems is wired to
anything; `ChunkCache` is not thread-safe and nothing decides when a cached
column is stale; `TabEntry::team` is always `None` because Rewo does not decode
the scoreboard-team packet; and `TOOLTIP_TEXT_GUI_PX = 9.0` is an unverified
calibration guess awaiting one eyeball.

### Three headless wire subsystems (2026-07-28, second batch)

All three chosen by one test — **no eyeball, no design decision** — and each
completes something already built. Nothing is wired to a renderer.

**`bundle_contents` (committed as M61 — see the numbering caveat above; the
same number also names the wavy cape).** `container::bundle_chrome` and
`tooltip::bundle_image` were built and graded by `inventoryshot` but wired to
nothing, because `walk_item_template` discarded the id, count and nested patch
it read.

The design choice that matters: **capture and walk consume the same bytes by
construction, not by two implementations agreeing.** `walk_item_template` is
now `Ok(read_item_template(..)?.is_some())`. The patch has no length prefix,
so a capturing reader that drifted from the walking one would corrupt every
packet carrying a bundle.

Three states, not two: `None` is absence (resolves through
`BundleContents.EMPTY`), `Some(vec![])` is an *explicitly empty* bundle
(vanilla draws the empty-bundle blurb, not "no image"), and a removal resolves
like `None`. `selectedItem` is **not on the wire** — the codec maps through the
one-arg constructor, so a selection is client-side screen state.

**The blocker moved rather than closed.** It is no longer the decode, it is the
carrier: `ItemSlot` is `Copy` on purpose (the click arithmetic moves it through
a dozen struct-update expressions) and `SlotText` would need its `is_empty`
taught the new field, or a bundle carrying *only* `bundle_contents` is recorded
as textless and dropped — exactly the bug M42's enchantments hit. Also,
`getWeight` needs to know whether an element is itself a bundle or holds bees,
which needs the nested patch's *contents*; `patched` is one bit and cannot
answer it, so the grid and counts are drawable and the weight bar is not.

**M62 — the tab list's wire inputs.** `tab_list.rs` transcribed vanilla's
four-key comparator and three keys were inert. Now decoded: `tab_list_order`
(action 6), `gamemode` (action 2), and `set_player_team` (new
`rewo-net/src/teams.rs`, plus the `Scoreboard` state machine).

**It found a drift M52c introduced.** Extracting a pure parser so tests could
drive the real walk had created *two* copies of the entry walk, and they had
already diverged: the test copy capped a profile signature at 32767 where
`GAME_PROFILE_PROPERTIES` says **1024**, which the production copy had right —
so the tests were validating a walk the client does not use. They are now one
function that `apply_player_info` also runs.

Facts worth keeping: `GameType.byId` is `ByIdMap.continuous(ZERO)`, so an
**out-of-range mode is Survival, not an error** (same for visibility, collision
rule, team colour); every field is `Option` because the packet is a **delta**
and an unset action bit means *unchanged*, so defaulting would report a
spectator returning to survival on every latency-only update; a team packet
naming an **unknown team returns early and discards its roster**; and
`shouldHavePlayerList` includes method 0, so an ADD carries parameters *and* a
roster — mis-reading the parameters by one byte silently eats the roster.
Team-by-name → uuid is a **lazy two-step** lookup, which is what vanilla's
`PlayerInfo.getTeam` does and matters because the two packets have no ordering
guarantee.

**M63 — the sound packets, decode only.** Rewo has no audio at all, and the
survey puts ~117M downloads of demand behind that one prerequisite. Decoding a
packet needs no listening; making a noise does — that split is the task.
**No audio crate, no device, no mixer.** `sound`, `sound_entity`, `stop_sound`,
ids resolved by name and all `req!`. `custom_sound` does not exist in 26.2.

Four details where the wrong answer is plausible, all mutation-tested:

- Position is `(int)(coord * 8.0)` on the wire and the accessor is
  `this.x / 8.0F` — an **int/float** divide, so Java rounds to `f32` *before*
  widening to `double`. An `f64` divide agrees near spawn and drifts past
  ~2²¹ blocks; dividing by 16 puts every sound at half its true distance,
  audible as wrong attenuation and never as an error.
- The sound event is `ByteBufCodecs.holder` — `id + 1`, `0` meaning an inline
  definition follows. Reading it raw shifts every sound by one *and* then reads
  the inline body as the next field.
- `stop_sound` reads source **first**, then name, and only when its flag is
  set. Name-first works for flags 1 and 2 and corrupts flags 3.
- `sound_entity`'s id is a var-int where `sound`'s coordinates are fixed i32s.

The model lives in `rewo-net`, not `rewo-world`: `ParticleEvent` is in
`rewo-world` because `rewo-world` *simulates* particles, whereas a sound has no
client-side state, so filing it there adds a hop through a crate that only
forwards it.

**Integration hazard, recorded because it nearly bit twice.** These agents
branched from the same base and shared `play.rs`, `ids.rs` and `lib.rs`, so
each was applied as a **3-way patch, not a file copy** — a copy would have
compiled cleanly and silently deleted the previous one's work. Verify the
earlier symbols are still present afterwards rather than trusting the build.

**Mutation-testing found three decorative tests across the batch**, none of
them findable by reading. The sharpest: a depth witness sized as
`MAX_DEPTH + 2` is *self-calibrating* — raising the bound raises the payload,
so it passes at 8 and at 64 alike and only ever witnesses "recursion
terminates".

### The sound registry and the server-driven display packets (M64, M65)

Two more headless subsystems, same test as the batch before: no eyeball, no
design decision. Nothing is wired to a renderer.

**M64 — the `sound_event` registry table.** M63 named it as step 1 toward
playback. Parsed at load from the datagen report, matching
`particle_types.rs`; 1,968 entries, dense ids, both-direction lookup.

**The alphabetisation trap here is the sharpest "invisible to every gate" case
in the project so far.** `serde_json`'s default `Map` is a sorted `BTreeMap`,
so iterating `entries` hands you the registry **alphabetically**. The real 26.2
registry is not: ids 0–6 are the seven `entity.allay.*` events and
`ambient.cave` is id 7, where sorted order would put
`ambient.basalt_deltas.additions` at 0. An `enumerate()`-based table therefore
gives **a different wrong name for every one of 1,968 sounds** — and no decode
gate can catch it, because the ids still round-trip and the strings are still
real sound names. It is visible only to someone *listening*. Read `protocol_id`
off each entry; never derive an id from position.

Two resolution rules: an **inline** sound event returns its own identifier
*without* consulting the table, because it may name a resource-pack sound with
no registry id anywhere; and an unknown registry id returns `None`, never a
substitute — a wrong sound is harder to notice than a missing one.

**M65 — scoreboard objectives/scores/display, boss bars, tab header/footer.**
Six packets Rewo decoded none of. `Scoreboard` now **owns** M62's `Teams`
rather than sitting beside it, because vanilla's `Scoreboard` is one object and
the halves touch; `PlaySession.teams` became `PlaySession.scoreboard`.

**Two enum-decoding conventions sit one field apart, and only the decompile
distinguishes them.** `RenderType`, `BossBarColor`, `BossBarOverlay` and the
boss `OperationType` are `readEnum` — an array index, so out-of-range is an
**error**. `DisplaySlot` is `ByIdMap.continuous(…, ZERO)` — out-of-range is
**`LIST`**. Assuming either convention globally is wrong half the time.

Other findings, each witnessed: `NumberFormat`'s body length depends on its
registry id (`blank` is **zero bytes**), and an unnameable id is not skippable —
M41's no-length-prefix rule again — so it is a decode error; `reset_score` with
**no** objective name means *every* objective, not none; `set_display_objective`
naming an unknown objective **clears** the slot rather than being ignored, or a
stale sidebar is stranded; `removeObjective` keeps an emptied holder while
`resetSinglePlayerScore` drops one; a repeat boss-bar ADD replaces **in place**
(`LinkedHashMap::put` keeps insertion order), so re-pushing the id would
reorder bars on screen; and `tab_list`'s "no header" is a component whose
*flattened text* is empty, not an absent field.

**The mutation survivor was a real gap, not an equivalent mutant** — the first
in this project's batches where that turned out to be true. Deleting
`display.retain(...)` in `remove_objective` survived because the witness
asserted `display_objective(Sidebar).is_none()`, which resolves *through* the
objective map and so reports `None` for a stale entry too: **the witness was
measuring the wrong thing.** Fixed with an accessor that sees the stored name,
plus a behavioural test that re-creating a removed objective of the same name
must not resurrect its old sidebar — the actual bug, since servers run
remove/re-add cycles constantly.

**Integration.** Both agents branched from the same base and both added a
module, a `GameData` field and a load call to the same three regions of
`crates/rewo-data/src/lib.rs`. The 3-way patch **conflicted**, which is the
correct outcome — a file copy would have deleted the other silently. Both sides
were purely additive, so the resolution was the union.

**`crates/rewo-data/src/lib.rs` is the third file to hit the mixed-CRLF trap**
(after `entities.rs` and `chunk.rs`), and both agents hit it independently: an
editing tool normalised its 95 CRLF / 58 LF into a 60–123-line diff for a
7-line change. Both recovered byte-precisely. A `.gitattributes` policy would
retire this class of problem, but it touches line endings repo-wide and is a
decision, not a cleanup.

### The audio asset layer and the packet coverage audit (M66, M67)

Same test as the batches before — no eyeball, no design decision.
**`REWO_PACKET_COVERAGE.md` is the important artefact here**; read it before
planning any protocol work.

**The audit measured something nobody had.** Rewo's packet handling grew
milestone by milestone, so what it decodes was a *historical accident, not a
decision* — twice recently a whole family turned out to be simply absent
(M63's sounds, M65's scoreboard set), each found by noticing it was not in
`ids.rs`.

**141 clientbound-play packets: 56 consumed, 0 resolved-but-ignored, 85 never
resolved.** The zero is a real negative finding — the `cb_play_*` fields and
the dispatch chain agree exactly, so the whole gap is *names never resolved*.
The 85 split 31 pure-state / 20 needs-rendering / 23 needs-a-subsystem /
11 not-applicable.

**It also undermined a claim this file and REWO_PLAN both lean on.**
`rewo play`'s **`CORRECTIONS 0` proves less than it has been cited as
proving**: the harness walks on flat ground and is never knocked back,
exploded at, or mounted, so `explode` (whose `playerKnockback` is
`addDeltaMovement` on the local player), `set_entity_motion`, `move_vehicle`
and `set_passengers` are **structurally outside what it can test**. The number
is real and the physics port may well be right; the evidence is narrower than
the phrasing suggests. Treat it as "no correction *on the paths the harness
exercises*".

Two more gaps worth knowing: **`set_player_inventory`** is
`container_set_slot`'s index-addressed twin and only one of the pair was ever
handled — M34/M35 built a *predicting* inventory whose sole correction path is
a full state-id resync, and these exist to correct it without one. And
**`update_tags`**: Rewo reads `ItemTags.SPEARS` (M19) and the enchantment tags
(M42) from the **jar**, so a datapack that retags an item yields a wrong swing
duration or a missing tooltip line **with no error anywhere** — M64's
alphabetisation trap one layer up.

**"Handled" is not "complete"** (audit §4): six consumed packets decode less
than their body carries, and the greps the audit uses would call every one of
them handled. Sharpest — `game_event` consumes 4 of 14 types, so
`CHANGE_GAME_MODE`, the local player's own gamemode change, is matched and
dropped.

**M66 — the audio asset layer.** `sounds.json`'s weighted-variant index and
`level_event`'s id→sound table. **The data is not in the client jar** — it
arrives through the *asset index*, and so does every `.ogg`, which makes
`validateSoundResource` real rather than assumed: a variant whose file is
absent is dropped and the event's weights move with it. 1,968 events, 8,024
variants, 61 of them `type: "event"` **redirects** — and a redirect contributes
the **target's** total weight, not its own declared `weight`.

`forLocalAmbience(sound, **pitch**, volume)` takes pitch *second* — reading the
argument list left to right makes 1032's portal three times too loud at a fixed
pitch. `globalLevelEvent` and `levelEvent` are **disjoint switches**, so a
mismatched global flag is silence in vanilla too. Three ids are deliberately
unresolved rather than guessed, each with its derivation recorded: 1010
(jukebox song), 2001 (per block-state `SoundType`), 3008 (`BrushableBlock`).

**Still no audio.** No crate, no device, no mixer — that is the part needing a
human to listen.

**Two process facts from this batch.**

*A build passing proves the working tree is good, not the commit.* A disk-full
error during `git add` produced a commit that declared two modules whose files
were not staged — `cargo build` passed locally because the files were on disk.
Only `git show --stat` showed 2 files instead of 4. After any git operation
that errors, re-check what actually landed.

*The M-number ladder is now unusable as an index.* M52, M61, M64 and M66 each
name two unrelated pieces of work, because concurrent sessions numbered
independently. Read `git log --oneline` subjects.

### The three gaps the coverage audit ranked first (M68, M69)

The first work chosen *by the audit* rather than by what was next in a plan —
all three from `REWO_PACKET_COVERAGE.md`'s "pure state, no rendering" class,
all headlessly verifiable.

**M69 — the server's authoritative writes.** `set_player_inventory`,
`set_cursor_item`, `update_tags`. M34/M35 built a *predicting* inventory whose
only correction path is a full state-id resync; these are how the server fixes
it without one.

**`update_tags` exists in BOTH states — configuration 13 and play 134 — and the
audit listed only the play one, because the audit surveyed clientbound-play
only.** The *configuration* copy is what a vanilla server sends on join, right
after `registry_data`; the play copy is the `/reload` case. Resolving only play
134 would have looked like it worked until someone reloaded. That is a limit of
the audit's scope, not of the packet, and it is the first thing a whole-protocol
sweep would catch.

**The two inventory coordinate systems are worse than a different origin.**
`InventoryMenu`'s `SLOT_IDS` is `{HEAD, CHEST, LEGS, FEET}` at backing index
`39 - i`, so **the armour ranges run in opposite directions**: inventory 36 is
FEET / menu 8, inventory 39 is HEAD / menu 5. Subtracting a constant — the
obvious reading of "36 here, 5 there" — puts boots on the head, and produces
output of the right type, in the right range, for the right item category. No
decode gate can see it. There is now one conversion
(`menu_slot_of_inventory_index`) and `Inventory::hotbar` routes through it.

It returns **three** outcomes, not two: `Applied`, `NoMenuSlot` (indices 41/42 —
`SLOT_BODY_ARMOR` and `SLOT_SADDLE` are real `EntityEquipment` slots the 46-slot
menu does not expose), and `OutOfRange`. Collapsing the middle makes "Rewo has
nowhere to put this" look like a decode failure. Same instinct one layer up:
`TagOverrides::contains` returns `Option<bool>` so silence is distinguishable
from a negative — a bare `bool` reads every unsent tag as "not a member", which
poses every spear as `ArmPose::Item` against a server that omits the item
registry.

`set_player_inventory` carries **no state id** and bypasses the container menu,
so the write must not touch `state_id` — advancing it would make the next click
echo a number the server never issued, which is the exact resync the packet
exists to avoid. Its slot is a **VarInt**, not the `i16` its sibling
`container_set_slot` uses: M34's recorded trap does not generalise.

The tag override is modelled and **deliberately not wired**. M19's `SPEARS` is
one `ItemTag::from_ids` away but needs ~8 call sites plumbed and no gate would
grade it. M42's enchantment tags are **blocked, not unplumbed** — `rewo_data`
stores names where the packet carries ids, and bridging needs the wire-order
registry read at a moment the two packets have no ordering guarantee about.

**M68 — the four packets that move the local player.** `explode`,
`set_entity_motion`, `move_vehicle`, `set_passengers`.

**My brief for this one was wrong, and verifying it was the most valuable thing
the agent did.** I said velocity was fixed point, thousandths of a block per
tick, as a short. **That encoding does not exist in 26.2.**
`ClientboundSetEntityMotionPacket` composes `Vec3.LP_STREAM_CODEC` →
`net/minecraft/network/LpVec3.java`: three 15-bit mantissas against **one shared
integer scale**, a **one-byte zero sentinel**, and an optional continuation
VarInt. No `8000.0` exists anywhere in the protocol tree. Implementing the brief
as written reads 6 bytes of a body that can be 2.

Two more, both verified: `move_vehicle` carries **no entity id** (the client
resolves `getRootVehicle()`), and `explode`'s `blockCount` is a **fixed
big-endian i32 between `radius` and the knockback**, so a VarInt reading
silently reports "no knockback" from a packet carrying one. And `explode`'s
knockback is `Vec3.STREAM_CODEC` (full doubles) where `set_entity_motion` is
`LP_STREAM_CODEC` — **two different `Vec3` encodings in adjacent packets**.

**The gate is the point, not the decode.** `rewo play --motion-check` drives a
paced command stream (boat → `ride mount` → `ride dismount`; then resistance →
TNT → `/damage … by <zombie>`), fail-closed on **observation** — a command the
server ignored leaves a counter at zero and turns it red.

**A live mutation found a bug in the gate itself**, and the important half is
not the timing slip: **the correction meter structurally cannot catch a dropped
knockback.** Vanilla's move check flags a client that moves too *much*; one
ignoring a shove moves too *little*. The witness is now the measured change in
the client's own velocity. So, precisely — `CORRECTIONS 0` over a flat walk is
unchanged and still true; the knockback path is now exercised; but *correct
handling* rests on the |Δv| witness, not the meter. **Riding accuracy is
unprovable by any correction count** — `ServerGamePacketListenerImpl` skips move
validation entirely for a passenger — so mount-phase corrections are reported
and explicitly not graded.

`move_vehicle` is **structurally unreachable** and the gate says so rather than
passing quietly: both send sites are inside `handleMoveVehicle`, the server
*rejecting* a serverbound vehicle move, which a passenger-only client never
provokes.

**The collision that the build would not have caught.** A concurrent session
landed **M70** (entity-label visibility) between M68's base and `main`, and it
decodes `set_passengers` too. The 3-way patch applied **cleanly** and left a
duplicate struct field *plus a silently unreachable second dispatch arm* — the
field fails the build, the arm does not. The two effects are disjoint (M70
builds the riding graph that suppresses a ridden entity's floating label; M68
applies the local player's mount state to physics), so the resolution is one
field and one arm doing **both**, not a winner. `body` is a `&[u8]`, so the
second read is safe and deliberate — folding either decode into the other would
couple two milestones with no reason to share a walk. **The general lesson: a
clean 3-way apply is not evidence of no collision.** Grep for the symbol.

`motion.rs` also arrived 991/991 CRLF while every other file in its crate is LF;
normalised rather than left to become the fourth file in the mixed-endings trap.

**Gates:** rewo-net 390, rewo-world 291, rewo-app 80; `inventoryshot` 143 →
**152**, `mobshot` 243/243, `swingshot` 97, `eventshot` 28; demo PNG
`2cc56b4acbfb92cb` byte-identical. **27 mutations across the two milestones, 26
caught**; both survivors were real — M69's was a `a != b || { true }` tautology
the agent found in its own witness before the battery ran, and M68's was a test
asserting only `is_err()` where both the intended and the mutated path error and
only the error's *shape* distinguishes them.

### M71 — the ten `game_event` types, and what "handled" was hiding

The audit's §4 claim — **"handled" is not "complete"** — worked as a closed
example. `game_event` passed every grep as handled and consumed **4 of 14**
types: M33 took the four weather ids and the other ten were matched and thrown
away, `CHANGE_GAME_MODE` among them.

**There are two params, not one.** Vanilla computes
`int param = Mth.floor(paramFloat + 0.5F)` at the top of `handleGameEvent`, and
**only** `CHANGE_GAME_MODE` and `GUARDIAN_ELDER_EFFECT` use it. Every other
branch reads the **raw float** — `DEMO_EVENT` compares exact literals
(`101.0F`, `102.0F`…), `IMMEDIATE_RESPAWN` is `p == 0.0F`, `LIMITED_CRAFTING`
is `p == 1.0F`. Using one where the other belongs is invisible for every
integral param a server actually sends.

**`IMMEDIATE_RESPAWN` is inverted, and the name is what inverts it:**
`setShowDeathScreen(paramFloat == 0.0F)`, so param **0 shows** the death screen.
Transcribe from the event name rather than the setter and you get a client that
shows the screen exactly when it should not.

**An unknown type id is a silent no-op, not an error.** `Type.TYPES.get(id)`
returns null and every `==` against it is false. The instinct on a decode task
is to make an unrecognised discriminant an error; here that would disconnect a
client from a server sending a type it merely does not care about.

**`ClientLevel.playSeededSound` reads backwards from the server.** Its body is
`if (except == this.minecraft.player)` — the client plays the sound **only**
when the "except" argument *is* the local player, which is the opposite of what
the parameter means server-side. `handleGameEvent` passes the local player, so
all three of its sounds are audible; any other reading is silence.

Three more, each verified: the join-time values of `IMMEDIATE_RESPAWN` /
`LIMITED_CRAFTING` ride the **login packet**, so ids 11/12 are only the
mid-session gamerule change; `setLocalMode` guards the previous-mode write on
the mode actually changing, so a repeat must not clobber it; and `handleRespawn`
copies `showDeathScreen` to the new player but **not** `doLimitedCrafting` —
that asymmetry is vanilla's, and is why nothing is cleared on a dimension
change.

**Applied 7, modelled 3, one deliberately left homeless.** Applied:
`CHANGE_GAME_MODE` (reusing M62's `GameMode::by_id` — `ByIdMap.continuous(ZERO)`,
so out-of-range is Survival), `IMMEDIATE_RESPAWN`, `LIMITED_CRAFTING`,
`NO_RESPAWN_BLOCK_AVAILABLE` (queued as a **translation key** and resolved
against `baked.lang` at the edge, which is what `Component.translatable` does),
and the three sounds into M63's queue. Modelled only: `WIN_GAME`, `DEMO_EVENT`,
`LEVEL_CHUNKS_LOAD_START` — screens and a load tracker Rewo has no equivalent
of, and Demo's hints need keybind names Rewo cannot supply, so the hint is
recorded rather than fabricated. **Homeless on purpose:**
`GUARDIAN_ELDER_EFFECT`'s particle, because `ELDER_GUARDIAN` is not one of M37's
six transcribed kinds and M37's own rule is that an unknown kind is dropped
rather than rendered as something else.

**Gamemode is modelled, not acted on.** `rewo-world::physics` has no flight,
no-clip or invulnerability concept and neither `player_abilities` packet is in
`ids.rs`; the four-step job is written into the coverage doc's new §4.1 rather
than half-started.

**Two structural findings, both from mutation testing, and both bigger than the
milestone.** `PlaySession`'s fan-out was **entirely unwitnessed** — it owns a
socket and there is no test module for it anywhere in the repo, so dropping the
weather branch, the state branch or the eye-height all survived the whole suite.
The logic moved into a tested `game_event::apply` behind a 6-line adapter, with
a signature taking `&PlayerState` rather than loose coordinates so a transposed
axis or an `eye_y`-for-`feet_y` swap is *unrepresentable*. And the first
refactor left `weathershot` grading a path the client no longer took — **M45's
`install_shapes` failure exactly**: a gate that reimplements a slice of the
app's setup misses whatever moves out from under it. Caught by mutating the
weather branch and watching the gate drop 35 → 32.

**Gates:** rewo-net 418, rewo-world 291, rewo-app 80; `weathershot` 35/35,
`inventoryshot` 152, `particleshot` 34; demo PNG `2cc56b4acbfb92cb`
byte-identical.

### M72 + M73 — the two halves other milestones had to stub

Both landed from concurrent sessions and both close a gap an earlier milestone
*recorded rather than faked*, which is the pattern working as intended: M70 and
M68 each wrote down what they could not evaluate, and someone later read the
note and evaluated it.

**M72 — where a rider actually sits.** M70 decoded `set_passengers` into a
riding graph and consumed it for `Entity.isVehicle()` alone, so a rider still
rendered at its own stale synced position. **The seat is entity-type DATA in
26.x, not a constant** — `getPassengerRidingPosition` reads
`EntityDimensions.attachments()`, declared by the `EntityType` builder
(`tools/gen_entity_attachments.py`, 158 types, 57 declaring seats, 24 a vehicle
point). Three builder conventions invert if assumed: a bare float in
`passengerAttachments` is a **Y offset**, `ridingOffset(r)` is **negated** into
the VEHICLE point, and PASSENGER's fallback is **AT_HEIGHT** (the top of the
bounding box), not AT_FEET.

**There are two tables, keyed by two types.** `positionRider` subtracts the
*rider's own* VEHICLE point, rotated by the *rider's* yaw — a player's is
`(0, 0.6, 0)`, which is the whole reason a mounted player sits in a saddle
rather than standing on the horse's head.

**A passenger does not interpolate**, and the pre-M72 error was never a constant
offset — **it was a lag**. `ClientLevel.tickEntities` skips passengers outright;
a rider is reached only via `tickPassenger` → `rideTick`, which ticks it and
*then* overwrites its position. So Rewo derives into `cur` at the end of
`tick_lerp`, after every entity's own step has moved `prev = cur`.

Overrides dispatch on the **Java class**, most-derived-first, because that is
what `super` does. Gate: `rewo rideshot --check`, 24 witnesses. **18 mutations,
17 bit first time**; the 18th repeated M70's `b4` in a new shape and its named
partner was *unreachable by construction* — **a named mutation partner that
cannot be reached is not a partner**, so the witness was rewritten to name the
detach that is load-bearing.

**M73 — the entity raycast, and the label clause M70 stubbed.** `shouldShowName`
is `entity.shouldShowName() || (hasCustomName() && entity ==
crosshairPickEntity)`, and Rewo's raycast was voxel-only, so M70 transcribed the
second disjunct, graded it both ways, and fed it a hard `false` live.

**It is not a second, label-only raycast.** `Minecraft.pick` assigns
`crosshairPickEntity` from *the* hitResult — the same one that decides which
block you are mining. In 26.2 that lives in a private static `LocalPlayer.pick`,
not in `GameRenderer`.

The inflation is `entity.getPickRadius()` — **0.0F for everything but a
Projectile** — and **not** the `DEFAULT_ENTITY_HIT_RESULT_MARGIN = 0.3F`
declared beside it, which belongs to the projectile overload. **`isPickable()`'s
default is `false`**: a dropped item, an experience orb and a text_display are
invisible to the crosshair, and so is the ender dragon, which overrides it back
to false and delegates to unregistered `EnderDragonPart` hitboxes.

**Neither range is hard-coded** — both are RangedAttributes, so creative mode's
`+2.0` entity-range modifier applies by itself. That exposed a real gap:
`apply_update_attributes` opens with `getEntity(id) == null` and **the local
player is not in the EntityTable**, so every snapshot addressed to it was being
dropped. `PlaySession` now keeps `local_attributes`.

**A mutation survived and found something.** `g5` claimed a dead heat goes to
the block via `>=`, and mutating it to `>` left the gate green — vanilla
enforces that precedence **twice** (the sweep is truncated at the block hit
*and* the survivor is compared against it), and because the truncation feeds
`maxValue`, whose test is strict, the tie is already excluded by the sweep
bound. **Neither half alone is observable.** Also: two witnesses hand-computed a
`0.3` half-width and landed a hundred-millionth off the bound they claimed to
sample, because **vanilla halves the width as a float** — a mob's near face sits
at `x - 0.30000001192`.

Both sessions updated `REWO_PLAN.md` and the coverage doc but **not `CLAUDE.md`**
— these entries are that catch-up. **Merged state: 1247 tests** (net 420, world
318, gpu 205, data 175, mesh 38, proto 11, app 80); `rideshot` 24, `labelshot`
47, `weathershot` 35, `inventoryshot` 152, `mobshot` 246/246; demo PNG
`2cc56b4acbfb92cb` byte-identical.

### M54–M86 — the fidelity arc, the coverage sweep, class B, and the bug eighty milestones of gates could not see (2026-07-28 → 07-30)

Twenty milestones, caught up here on 2026-08-02 after a session found this file
still ending at M73. **Verified from a cold start rather than read off a doc:**
`origin/main` `0ddbc66`, tree clean, **no branch anywhere holds a commit not on
`main`**; release build clean; **1623 tests, 0 failures** (net 565, world 489,
gpu 249, data 179, app 85, mesh 45, proto 11); `mobshot --check` **246/246**;
**32 serverless gate commands** green, 0 VUIDs; demo PNG still
`2cc56b4acbfb92cb`.

**`REWO_PACKET_COVERAGE.md` is at 107 consumed / 0 ignored / 34 absent, and
classes A and B are both empty** — every clientbound-play packet Rewo *can*
render is rendered. The 34 remaining are 23 needing a subsystem Rewo lacks
(container/menu screens ×6, recipe book ×5, chat input ×4, advancements ×2,
resource-pack fetch ×2, dialog ×2, map + transfer ×2) and 11 not applicable.
**Picking work there now means choosing a subsystem, not a packet.**

**M54–M60 — data and fidelity.**

- **M54 the language map.** `en_us.json` **is not the language map**; it is step
  1 of three. `loadFromJson` rewrites every unsupported format specifier
  (`%d`/`%f` → `%s`, which is why `decomposeTemplate` only understands `s`;
  inert on 26.2 and transcribed anyway, because its absence is invisible until
  a pack carries a `%d` and the whole line collapses to its raw pattern), then
  `deprecated.json`'s `applyToMap` applies **383 removals and 146 renames**
  — remove first, then rename, and the order is load-bearing. **105 of the 146
  rename targets do not appear in `en_us.json` at all**, and 41 that do are
  overwritten, changing **27 item display names** (the eighteen smithing
  templates stop reading "Smithing Template") — every change *toward* vanilla.
- **M55 entity attributes.** `MAX_HEALTH` is not metadata, it is an
  **attribute**, and `update_attributes` (131) was falling off the dispatch
  chain. The holder is `holderRegistry` — **raw 0-based**, third time this has
  bitten (M16 dimensions, M21 damage types) — and here the failure is *quiet*:
  `max_health` is 23 and `max_absorption` 22, both real syncable attributes on
  the same entity, so an off-by-one clamps against the wrong range rather than
  throwing. The operation is a **VarInt, not a byte**, and an out-of-range id
  is `ADD_VALUE`, not an error. `ADD_MULTIPLIED_BASE` reads the *post-*`ADD_VALUE`
  base and every such modifier reads that same base — they do not compound.
- **M56 the tooltip's image pass.** `GuiGraphicsExtractor.tooltip` walks its
  components **twice with `localY = y` between**, and the two loops advance
  identically — so the split is a **layering device, not a layout one**: it
  guarantees every image draws after every text line whatever the component
  order. Run as one cursor and the grid drops below its box by the height of
  all the text (57 px in the fixture). `lines.size() == 1 ? -2 : 0` counts
  **components**, not text lines. Three brief errors the decompile settled: the
  `+N` badge is the **bottom-right** cell; thirteen stacks show **eight** items,
  not twelve; and the badge counts hidden **items**, not stacks (thirteen full
  stacks badge `+320`, never `+1`).
- **M57 entity fidelity — emissive, ETF, the dye tint.** Eight mobs have
  emissive layers in vanilla and none glowed; a warden, whose whole visual
  identity is bioluminescence, had none. Both `RenderLayer` shapes re-render the
  mob's **own model** with a second texture at full brightness, so the geometry
  is the same quads re-pointed at another texture. The warden's tendril layer
  samples the **base** warden texture, not an overlay.
- **M58 the bundle grid's chrome.** `container/bundle/slot_highlight_back` is
  **not** the `container/slot_highlight_back` M35 already loads for the
  inventory hover box — both exist, both 24×24, and reusing the inventory's
  renders something that looks approximately right. The badge cell gets **no
  chrome at all**: `extractCount`'s entire body is one `centeredText`.
- **M59 the health bar's render half — the first Rewo feature with no vanilla
  oracle.** Vanilla renders no health bar over any entity, so there was nothing
  to transcribe; the numbers were written down first as a *decision*
  (`REWO_HEALTH_BAR_SPEC.md`) and the gate grades against that. **The gate
  re-declares the spec's constants rather than importing the implementation's**
  — importing them asserts only that the implementation equals itself, which is
  M41's `t4` failure mode exactly. Two spec witnesses are unobservable from
  outside and say so in their detail strings rather than being quietly dropped.
- **M60 the vanilla cape.** Scoped as needing "the milestone's one structural
  change" (`Rx·Rz·Ry`, which Rewo's `Rz·Ry·Rx` parts cannot produce) and needed
  **none**: `rotateBy`'s leading `rotateY(-PI)` exists to **cancel the pose**,
  so the net rotation *replaces* the `PartPose` rotation — while the pose's
  **translation still applies**, which is the asymmetry that makes it easy to
  get wrong in either direction.

**M74–M78 — the coverage re-audit, then the class-A sweep.**

- **M74 the re-audit.** Ten of 141 rows were wrong, **all in one direction**
  (`absent` about code that was present). The mechanism is not neglect: M67
  wrote the table by grepping a moving tree and four packets landed the same
  day. M67 *saw* it happening and worked around it twice, both of which made it
  worse — a predictive "After §7" column describing a moment that never
  existed, and milestone markers written into the **status** column, putting
  four rows outside any grammar a future check could read. **Annotating decay
  is not fixing it.** The fix is a unit test in `ids.rs`, deliberately *not* a
  `*shot` gate, because it must fire on the event that **causes** the drift
  (someone editing `ids.rs`). It also found a **live flow-control divergence
  hiding as a missing decode**: Rewo answered every `chunk_batch_finished` with
  a hard-coded `64.0` where vanilla's seeded opening bid is **3.5**, so it
  over-bid the server ~18× on every batch of every session and never adapted.
- **M75 abilities and flight.** The flags byte is `1/2/4/8` then two floats —
  nine fixed bytes; **the serverbound twin is one byte**, so writing the
  clientbound body there desyncs the stream by eight. An unauthorised flying
  claim is **ignored, not kicked**. **Flight does not go through
  `travelFlying`** — that was the central misdirection, and the method is for
  mobs and swimming. `Player.travel`'s flying arm captures `originalMovementY`,
  delegates to the *ordinary* `travelInAir`, then **overwrites** the Y it just
  computed with `originalMovementY * 0.6`: so flight has **no gravity term**,
  vertical drag **0.6**, and flying into a ceiling does not zero your upward
  velocity. `walkingSpeed` is **not** the client's walking speed (its only
  client consumer is the FOV modifier's divisor). `SPECTATOR` sets `flying =
  true` while `CREATIVE` only sets `mayfly`.
- **M76 rotation and world spawn.** The brief *and* this project's own coverage
  doc were wrong about the headline: `player_rotation` carries **no relative
  bitfield**. It is four fields with each `BOOL` sitting **after** the float it
  qualifies — and a reader written from the wrong description **decodes every
  packet without erroring**, because the arity happens to work out. The
  `Set<Relative>` is real one layer up, so the two teleport packets **share
  their semantics and not their layout**. The clamp is on the **sum**, not the
  step; the yaw gets neither clamp nor wrap.
- **M77 the minecart's own interpolation.** Framed as replace-or-feed; the
  answer is **neither** — it overrides the generic lerp at the *render* seam and
  leaves it running, and four separate places in the decompile have to agree for
  that to be true. Vanilla **measures one against the other**: a passenger's
  offset is literally the schedule minus the generic lerp. Mirror image of M72,
  where the rider's own lerp is computed and thrown away.
- **M78 session, server metadata, chat.** `bundle_delimiter` is a **pipeline
  instruction**, not an inert packet — its `handle` throws `AssertionError` if
  it ever reaches a listener, so decoding it as a no-op is the one way of being
  wrong that leaves no trace. A bundle is applied **all at once on close**;
  **the coverage doc's "in one tick" was wrong** — nothing defers a bundle to a
  tick boundary, the guarantee is that no *frame* renders part-way through. An
  unterminated bundle is **withheld**, neither dropped nor applied. There is **no
  nesting**, so a depth counter — the natural implementation — never closes.

**M79–M85 — class B, everything that needed a renderer.** The recurring finding:
**the class letter changes the gate, not the standard.** Each of these has an
exact vanilla oracle, so decode *and* render are transcribed line by line and
graded, with a pixel read-back half on top of the model half.

- **M79 titles, XP, cooldown.** A subtitle on its own **shows nothing** (only
  `setTitle` arms the clock). A negative animation field means *leave
  unchanged*, and the packet **re-arms a live title at its full duration** —
  `/title times` mid-title hands the title its whole life back. `/title clear`
  and `/title reset` differ in what the *next* title does, not what is on
  screen. **`set_experience`'s wire order is not its declaration order**, and
  reading top-to-bottom swaps two var-ints, decodes without erroring, and puts
  lifetime XP in the level display.
- **M80 the world border — six packets, one object.** Splitting the decode from
  the wall would have left the state machine with nothing to test against. The
  lerp's clock is **ticks**, not wall-clock, and the `gameTime` argument is
  **inert** — but the wall's *texture scroll* really is wall-clock milliseconds,
  the only such quantity in the feature, so the instinct was right one layer
  over. **`getMinX()` is the previous tick's size**: every non-rendering
  consumer (collision, the vignette) measures against the previous tick's box
  while the renderer alone passes a real partial.
- **M81 the hurt tilt, block cracks, item pickup.** Packet 42 is what made
  `no_damage_tilt` real — the Velvet batch's *"to port the disable you must
  first build the thing being disabled"* named the condition and this was it.
  It drives vanilla's own `damageTiltStrength` accessibility slider to its off
  end rather than branching around the tilt, so toggling mid-animation cannot
  strand the camera at an angle. **The server already subtracted the camera
  yaw** before sending, so the tilt direction is **frozen at the hit** and does
  not track subsequent turning.
- **M82 the screen framework and the death screen.** The coverage doc called the
  screens "a design decision rather than a transcription" — **half right**. The
  decision was real and *smaller* than it sounded: **vanilla has one screen
  slot, not a stack**; the nesting that looks like a stack is a replacement
  carrying a `BooleanConsumer`. The rest was ordinary transcription with the
  usual inversions: a hovered **disabled** button draws the plain disabled
  sprite (the three-arg `WidgetSprites` makes `disabledFocused` *be*
  `button_disabled`); `isHovered` and `isMouseOver` disagree **on purpose**, and
  because `getChildAt` uses the latter an **inactive** widget is not found at all
  and the click falls straight through; **`Esc` does nothing** on a death screen
  and `setScreen(null)` *re-opens* it.
- **M83 the locator bar.** `writeEither` writes **`true` for the left** (the
  UUID). The identifier is the **colour of last resort**, and a live vanilla
  server sends `colour=None`, so on a real connection the hash **is** the
  colour. The self-skip is gotcha 13 in both directions at once: the observer is
  never in `EntityTable`, so it must come from the session's own UUID — and a
  client that dropped the check **looks perfectly correct on vanilla**, because
  the server never sends you your own waypoint.
- **M84 the statistics screen — the packet that closes class B.** `Stat`'s
  two-level dispatch would normally be the `DataComponentPatch` hazard in
  miniature (an untranscribed variant cannot be skipped), and **here it cannot
  happen**, structurally: every `StatType`'s second level is a single VarInt, so
  what the first level selects is *which registry resolves the id*, not a
  different wire shape. Resolution is deferred, so an unresolvable value costs
  one dropped row rather than a dropped packet. **`StatsScreen.isInGameUi()` is
  false**, so it does not dim the world the way the inventory does.
- **M85 server links.** Three of the four things the brief said about the packet
  were corrections. The pause screen shows **one button, not a list** (it opens
  a separate dialog screen — three screens, not one). The disconnect screen
  shows **at most one link and only ever `BUG_REPORT`**, filled only on the
  client's *own* error paths — so **a server that kicks you politely shows no
  link however many it advertised, and one whose packet crashes your client
  shows exactly one**. And the packet exists in the **configuration** state too;
  third time (M69 `update_tags`, M78 `custom_payload`), so the rule is now
  reliable: **if the handler is on `ClientCommonPacketListener`, look for the
  configuration copy.**

**M86 — the bug eighty milestones of gates could not see.** `LiveApp::resumed`'s
init closure did `self.baked.take()` and **dropped the bake at its closing
brace**, so `self.baked` was `None` for the whole windowed session and every
`if let Some(baked) = self.baked.as_ref()` in `LiveApp::frame` was dead code.
**Nine shipped features had never once rendered in `rewo live`** — item icons
(M34), the inventory screen (M35), the player preview (M36), the first-person
hand (M38), clouds and precipitation (M33), the rain-fog band (M33b), particles
(M37), the world border (M80) and block-breaking decals (M81). Live since M3.
All of them are honest *headlessly*, because `run_headless` owns the bake as a
plain value — **which is exactly why eighty milestones of gates never saw it**.
The restore is four lines.

It was **not landable alone**: turning the paths on took a 10-second windowed
run from 0 validation errors to **40,532**, every one
`VUID-vkDestroyBuffer-buffer-00922`. **Eight** passes opened their `set_*` with
`free_buf(gpu, self.vbuf.take())`, destroying a buffer submitted command buffers
still reference — unobservable before only because none was ever constructed in
the windowed client, and unobservable headlessly because a one-frame oracle
never overlaps itself. The rule that came out of it, now in
`crates/rewo-gpu/src/buf_ring.rs`:

> **`ring >= fif + 1` for a ring written before `render`; `ring >= fif` for one
> written inside it** — because a `set_*` runs in the app's frame loop *before*
> `render`, so the most recent fence wait was the *previous* frame's.

**The gate it left behind is the one to remember: `rewo live --render-check`.**
It is the only check that drives the **windowed** client, and therefore the only
one that can see a render path the windowed client never reaches. **Run it after
any milestone that adds one.** It does not stage its own hotbar and **fails
closed when you don't** — `REWO_PRECMD="give @s minecraft:diamond_sword 1;give
@s minecraft:dirt 64"` against an opped username; 17/18 bare, 18/18 staged.

### M87 — the container/menu screens (2026-08-02)

Twelve commits (`f99ad5c..bd39954`, merged `2cd5635`). **The first bite out of
class C**, and a worked example of what that class costs: `open_screen` and
`container_set_data` are eleven lines of decode between them, and the other
eleven commits are what makes those lines mean anything. Before it,
`apply_container_set_content` opened with `if container != 0 { return false }`
and its own comment called that *"the whole truth about what this client can
show"* — on a real server you could not open a chest.

**Findings that invert, in the order they bite:**

- **`crafter_3x3` puts its result slot AFTER the player inventory** — grid
  0..8, `addStandardInventorySlots` 9..44, result at **45**. Every other menu
  appends the player's 36 last, so "container slots, then the player's" puts
  the crafter's output *inside* the player's inventory and shifts nothing
  else. **`crafting` inverts the other way**: its result is slot 0, before the
  grid.
- **`lectern` has one slot, no player inventory, and no container screen.**
  `LecternMenu` never calls `addStandardInventorySlots`, and `LecternScreen
  extends BookViewScreen` — the same fact from both sides. Any
  `slots.len() - 36` is a panic there. So it is **24 container screens and one
  book viewer**, not 25.
- **`open_screen`'s menu type is `registry(...)` — raw 0-based, not `holder`'s
  `id + 1`.** Fourth time (M16, M21, M55) and the quietest: id 2
  (`generic_9x3`) reads as 1 (`generic_9x2`), a real menu with a real screen
  and nine fewer slots, so a chest opens with its bottom row missing.
- **`container_set_data` is a VarInt then two *signed* `readShort`s** in a
  mostly-var-int protocol. Negatives are real (the anvil's cost, the beacon's
  "no effect").
- **Six screens override the title's x, in two different ways.** `dispenser`,
  `crafter_3x3`, `brewing_stand` compute `(imageWidth - font.width(title)) / 2`
  — a server-chosen name, so not storable as a constant; `anvil` (60),
  `crafting` (29), `smithing` (44 + `titleLabelY` 15) are literals.
- **The blit's sheet size is a per-call argument.** Twenty-one backgrounds pass
  `256, 256`; **`MerchantScreen` passes `512, 256`**, because a 276 px panel
  cannot come off a 256-wide texture. A global 256 gives `u1 = 1.078` and the
  sampler repeats its left edge across the right-hand third of the trade
  screen — which reads as a texture bug, not an arithmetic one.
- **A chest's background stops one pixel short of its declared height**
  (`114 + rows*18` vs blits covering `rows*18 + 113`). Vanilla's arithmetic;
  closing the gap samples a row of `generic_54.png` vanilla never samples.

**Three process results, each of which changed the work:**

1. **A checker, not a generator.** Every other bulk fact in Rewo comes from a
   `tools/gen_*.py`; here that is measurably the wrong tool. The 25 menus use
   **four idioms** (direct `addSlot`, a nested loop, a field assigned earlier,
   a fluent builder consumed by a base class) and five declare no slots and
   inherit them — one extractor reaches **17 of 25**, and chasing the rest is a
   small Java interpreter whose failure mode is a silently *short* slot list.
   `tools/check_menu_layouts.py` re-derives independently and diffs; it earned
   that on run one by **refusing to proceed**, seeing four slots in
   `BrewingStandMenu` where the table has five (`IngredientsSlot` takes a
   leading argument a four-arg pattern misses). The table was right.
2. **One dispatcher per packet id.** First a hazard — `container_close` is
   already owned by `route_client_state`, and the play loop is a chain of
   `else if`s, so a second claimant either steals M74's counter or never fires,
   with no error either way. Then a design constraint:
   `container_set_content` is one id addressing two menus, so `route_inventory`
   grew a `&mut Menus` rather than the container path getting its own router.
3. **A half-landed feature is a bug, not half a feature.** M87j shipped the
   panel setter *uncalled* on purpose: the icons and hover still keyed off the
   player's 176x166 origin, so setting only the panel would paint a chest sheet
   with the player's icons 28 px off — broken, not unfinished. M87k landed all
   four consumers together, choosing the menu **once** and threading it.

**The gate, and the witness that caught its own vacuity.** `rewo containershot
--check` — serverless, validation-required, fail-closed, **13 witnesses**,
grading against oracles the tables cannot influence (the slot geometry
re-derived from `ChestMenu`'s constructor; the panel against `generic_54.png`
itself). **Its first run failed on the witness written to detect exactly
that**: `p3` asks whether `p2`'s probes can distinguish the two readings, and
on a six-row chest they cannot — `split` is 125, the lower band maps
`y -> y + 1`, and the two candidate source rows are *adjacent* and identical
wherever the art is flat. The band probes now use a **one**-row chest (offset
91) and centring the **six**-row one (28 px vs 2). One fixture cannot serve
both claims.

**Measured:** 1683 tests, 0 failures; `containershot` 13/13, `inventoryshot`
152/152 **unchanged across all twelve commits**, `itemshot` 75/75, `handshot`
34/34, `menucheck` 25/25, demo PNG `2cc56b4acbfb92cb` byte-identical
throughout. `REWO_PACKET_COVERAGE.md` 107/0/34 → **109/0/32**, class C 23 → 21.
**`live --render-check` 18/18, validation ON, 0 validation errors** — note
validation is `cfg!(debug_assertions)`-gated for `live`, so a *release* binary
reports `r17` false and makes `r18` vacuous.

**What that check did NOT prove, and it mattered:** `--render-check` opens the
*inventory*, not a chest, so it graded the windowed client's health with M87 in
it and **not** that a container rendered there — the same shape of blind spot
M86 was. **M88 closed this**; see below.

### M88 + M89 — proving the container renders, then making it work (2026-08-02)

**M87's merge commit said "Rewo can open a chest" and that was an over-claim.**
It built the *render*. These two make it true. Detail in `REWO_PLAN.md` §15.

**M88 (`9666045`)** closed the render-check gap with `r19` (a container screen
was drawn — 1513 of 3551 frames) and `r20` (its panel was its own, 168, not the
player's 166). The container is opened by injecting a raw `open_screen` body
through the **production router**, per M17: injection is the deterministic
proof where a live encounter depends on the server's timing and the client
aiming at the right block.

**`r20` was wrong in its first cut**, and that is the transferable part: it read
`image_h` off the open menu's **layout**, which answers 168 for a chest whether
or not the panel builder returned one — so it could not tell a working
container from a silent fallback to the player's panel, the failure actually
worth naming. It now reads the height back **out of the renderer** after the
draw set it. *A value witness is only a value witness if it reads the value the
draw used* — reading one that merely **implies** the draw is a proxy that looks
more rigorous than it is. Mutation-tested: a silent `None` fallback drops `r19`
to 0 frames and `r20` to `None`; the first cut stayed green through it.

**M89 (`6123058`)** made a container *usable*. Three things were still
player-keyed, all reachable today (open a container, press E, click):

1. **Nothing opened the screen** on `open_screen` — the menu was recorded and
   nothing shown unless the player independently pressed E. In vanilla
   `handleOpenScreen` **is** `MenuScreens.create`; decode and screen are one
   action.
2. **Every click operated on the player's menu** — all sixteen sites used
   `session.inventory`, so clicking a chest's slot 5 picked up the player's
   crafting grid.
3. **The click packet hard-coded container 0 and the player's `state_id`** —
   and `stateId` is per-menu (`incrementStateId` is an instance counter; the
   resync test is against the menu the click *names*), so the server would
   apply a chest click to the inventory or reject it on a stale id.

The fix is **one accessor** (`PlaySession::shown_menu{,_mut}`,
`shown_container_id`) that every consumer goes through — the five click
actions, the prediction apply, the hover, and the packet's two ids. A
per-call-site choice is *how* they came to disagree. The hover needed **both**
halves: `screen_to_gui` centres the panel to find the origin and `slot_at`
scans that layout's slots, so asking the player's 176x166 while a 176x222 chest
is up shifts the cursor 28 px **and** looks it up in the wrong slot list — the
two errors do not cancel.

**`r21` isolates the new behaviour by ordering** — the container is injected at
0.4, *before* the gate force-opens the inventory at 0.5, so frames in that
window can only exist if the packet opened the screen. **And that reordering
silently broke M86's own coverage**: the forced-open branch guarded on
`!inventory_open()`, which the injected container now satisfies, so the branch
was skipped — including the **cursor park**, the only thing that lays out a
tooltip and therefore the only door to `VelvetTextPass::sync_atlas`. `r16`
stayed green while proving nothing it was written for. **The tell was a number
that was too good** (`r21` counting all 2244 frames rather than a ~290-frame
window) — the shape of a guard that has stopped firing. *A test can be disabled
by a change to an unrelated part of its harness, and it reports success while
it happens.*

**Measured:** 1683 tests; `containershot` 13 → **17**, `live --render-check`
18 → **21** validation ON 0 VUIDs, `inventoryshot` 152/152, demo PNG
byte-identical.

### M90 — shift-click routes by the menu's own quickMoveStack (2026-08-02)

The last silently-wrong path in the arc, and a second one under it.
`quickMoveStack` is a **per-menu-class override** and Rewo's routing was
hard-coded to `InventoryMenu`'s ranges, so shift-clicking a chest's slot 0
routed as though it were the crafting result.

**Nine of the 25 menus share one shape** (the six chests, dispenser, hopper,
shulker box): `slot < containerSize` → the player range **backwards** (the
hotbar's right-hand end, since `addStandardInventorySlots` appends it last),
else → the container range forwards. The furnace and crafting families have
their own and are **not** transcribed — they answer `QuickMove::Unimplemented`
and the caller **declines**, because moving nothing is inert where a
shift-click under another menu's rules moves the wrong stack and the server
applies it.

**The second bug was found by a witness, not by reading.** The first cut failed
on the container→player direction only: `move_stack_to` calls `slot_kind(i)` —
the *player's* — which returns `None` past 45, so the `?` aborted the whole
move. Nine call sites shared it, and the consequence is wider than shift-click:
**plain clicks past a chest's slot 45 also silently did nothing**, and below 45
read the wrong kind. M89 routed *which menu* a click applies to and not the
slot-kind lookup, so it fixed only the visible half. **When a type is
generalized, the functions it calls generalize with it — and the ones taking a
bare index rather than `&self` are the ones that get missed, because they do
not look like they belong to anything.** `slot_kind` is now
`MenuLayout::slot_kind`, with `SlotKind::Plain` for a container's slots and
`None` (decline) for an untranscribed menu.

### M92 — the rest of `container_set_data`, the crafting quick-move, and the first bespoke widget (2026-08-03)

Eight commits, and all three of M91's recorded open items. Detail in
`REWO_PLAN.md` §15.

**Three data consumers, each inverting against the last.** The brewing stand's
slots are the **reverse** of the furnace's — `getBrewingTicks()` is `get(0)`
and `getFuel()` is `get(1)`, where the furnace puts fuel at 0; both menus are
five bytes on the wire and naming them by analogy swaps a 0..20 fuel level with
a 0..400 tick counter. Its timer counts **down**, its arrow grows downward
while its bubbles grow upward (one function apart), its fuel bar grows rightward
— three directions on one screen — and its arrow **truncates where the furnace
ceils**, so at 399 ticks vanilla shows no arrow where a ceil shows a pixel.
`BUBBLELENGTHS` ends in **0**, so one frame in seven is blank.

The enchanting table's costs are **only a third of what its rows need**: the
lapis is the *count of the stack in menu slot 1* (a different packet) and the XP
level and creative flag are the player's. **The lapis requirement is the row
INDEX plus one, not the cost.** There are **three row states, not two** — an
empty row draws its background and nothing else; an unaffordable one draws the
same background *plus* its numeral. `col` does double duty and is reassigned
before the cost text, so a row's name and cost are different colours and the
cost's does not track the hover. The highlight and the tooltip use **different
rectangles**, and neither is a slip.

The beacon says "absent" with **0 and shifts real ids up by one**, where the
enchanting table one menu earlier uses **-1** — two conventions in the same
signed short on the same packet. And **an invisible button moves a visible
one**: the upgrade slot is counted into the column's `totalWidth` while
`visible = false`, so dropping it slides regeneration 12 px.

**The crafting quick-move needed a structural change**: `quickMoveStack` there
is a **fallback chain** (`if (!moveItemStackTo(grid)) { cross-move }`), which a
single-range return cannot express — it must either always try the grid or
never. This is the branch that makes a crafting table *fill its grid* on a
shift-click, which `InventoryMenu` does not do. The two crafting menus put
their result at **opposite ends** (CraftingMenu slot 0, CrafterMenu slot 45).

**`container_button_click`** is two var-ints and the **whole** input surface for
four screens; it carries **no state id**, unlike its sibling. The enchanting
table's click gate is **not** its render gate — it additionally requires slot 0
to hold something and tests the level against both `row + 1` and `costs[row]`.

**The bug it uncovered is bigger than the milestone.** Five `mob_effect` ids
(night vision, darkness, haste, conduit power, mining fatigue) were read from a
`registry_data` branch **that cannot fire** — `MOB_EFFECT` is a
`BuiltInRegistries` entry and appears **zero times** in
`RegistryDataLoader.java`'s synchronised list. So M13's night vision/darkness
and M19's dig-speed adjustment had **never worked live**, and no gate could see
it because `lightmapshot` and `swingshot` are serverless and *construct* the
effect state, supplying the very ids the live path fails to obtain. **When a
gate supplies an input production must derive, the derivation is untested by
construction** — worth a sweep for other instances. Fixed by the rule
`attributes.rs` already states: a built-in registry resolves **by name from the
report**.

**Three detector errors and a harness bug**, all mine: a control frame that
differed in its *background* rather than its subject; a probe on a glyph's
transparent row; a probe on a button that its icon repainted independently of
its chrome; and a mutation harness whose `mv` restore preserved the **older**
mtime, so cargo skipped the rebuild and the next run silently graded the
mutated binary (it presented as a green witness regressing with no code
change). A fourth finding came out of the same battery: `enchant_row_sprites`
had its own copy of the numeral mapping, so emptying `EnchantRow::numeral()`
changed nothing rendered — a model accessor graded by tests the app did not
call.

**Measured:** 1712 → **1773 tests**, all seven crates confirmed reporting;
`containershot` 17 → **27**; `live --render-check` 21 → **22/22** with
validation ON and 0 errors (it must be run from a **debug** build — validation
is `cfg!(debug_assertions)`-gated for `live`); demo PNG `2cc56b4acbfb92cb`
byte-identical; 41 mutations, 40 killed and the survivor shown to be doubly
guarded in vanilla too. `REWO_PACKET_COVERAGE.md` 109 / 0 / 32.

**Open:** `container_set_data` is now consumed by every menu that sends it.
`quickMoveStack` still declines for the brewing stand (three item predicates)
and the enchantment table (its last branch is not a range move — it places
exactly one item). Of the bespoke widgets the enchanting rows are done; the
loom and crafter now only need their button lists, the beacon needs
`set_beacon`, the anvil a text field, and the merchant and stonecutter are
blocked on class-C packets. **M93 took the beacon's and three of the eight
item-combiner menus' quick-moves** — see below.

### M93 — the single-input quick-moves, and the derivation nobody was grading (2026-08-03)

Two commits. The plan called the eight item-combiner / single-input menus "a
few lines each"; **the decompile does not support that**, and the correction is
worth more than the code. They are **four shapes**, and two of the plan's own
claims invert.

- **`MerchantMenu.quickMoveStack` consults nothing at all.** The merchant is
  listed as blocked on class-C `merchant_offers` — true of the trade-list
  *widget*, false of the quick-move, which never routes a player stack into
  slots 0 or 1. **Vanilla will not load a trade for you.**
- **`ItemCombinerMenu`'s player branch is a guard that CONSUMES, not a fallback
  chain** (M92e's `CraftingMenu` shape). `canMoveIntoInputSlots` defaults to
  `true`, so for the **anvil** the two main/hotbar arms below it are
  structurally unreachable: an anvil does not cross-move your inventory, and a
  full anvil moves *nothing*. Behaviour, not an omission.
- **The beacon's count test is in the branch, not in `mayPlace`** — so the same
  item routes two ways by count (one diamond claimed, two cross-moved). Its
  guard failing *does* fall through, unlike the combiner's. Vanilla's fifth
  beacon arm is **dead** and deliberately not transcribed. Its tag comes from
  the jar via `tools/gen_beacon_payment.py` (the `gen_fuel_values.py`
  precedent), and its slot gets `SlotKind::BeaconPayment` so a **plain** click
  respects the tag too — `Plain` would keep the quick-move exact and let an
  ordinary click predict a placement the server rejects.

**M93b–d then took the stonecutter**, and found that
`stonecutterRecipes().acceptsInput` is **not** a `RecipePropertySet` — that
registry's seven keys are smithing×3, the three furnaces and campfire.
`RecipeAccess` exposes it separately as a `SelectableRecipe.SingleInputSet`.
The difference does not reach the accepted-input table (`Ingredient.test` is
item identity) but *is* what the recipe **list widget** needs, so that stays
blocked on `update_recipes`. It is also the **third guard behaviour in three
menus**: the anvil's is always true, the beacon's falls through when it fails,
and the stonecutter's falls through when the *guard* fails but **moves nothing
when the MOVE fails** — two exits from one branch, only the first cross-moves.
Its predicate is branch-only (slot 0 is a bare `Slot`), and vanilla's
`player.drop` of an unfitting result remainder is **recorded, not modelled**.

**M93e then took the grindstone**, which **inverts the arrangement of every
other menu here**: the beacon and stonecutter ask about the *item* in the
branch and let the slot accept anything, while the grindstone asks about the
*slots* (`!input.isEmpty() && !additional.isEmpty()`) and puts the item
predicate in `mayPlace`. So `SlotKind::GrindstoneInput` is load-bearing for the
**shift-click**, not merely an ordinary click — and when it refuses, the move
returns false and vanilla `return`s, so a stick shift-clicked into an empty
grindstone moves **nothing**.

> **⚠ A blocker recorded above was wrong.** M93a said the loom and cartography
> table needed a prototype-component model Rewo lacks. **Rewo has one** —
> `rewo_data::item_components_table::prototype_has_component`, generated by
> **M56** for the tooltip's component count, covering all 1537 items. Check it
> before calling any component question unanswerable. The reusable shape:
> `has(X)` = *removed → false, patch-set → true, else prototype*.

**M93f then took the cartography table** — the only menu here whose branch
predicates and `mayPlace` predicates are the **same two tests, written twice**
(the branch picks which slot to try, `mayPlace` confirms it will take it;
neither is droppable). **`has(MAP_ID)` is tested first, and that ordering IS
map cloning**: `filled_map` carries the component and takes slot 0, while
`minecraft:map` is a *different item* with no component and falls through to
the paper slot. Vanilla writes the middle arm as a triple negation, so the
paper slot is the branch reached when the stack *is* one of the three —
transcribed forwards with the arms swapped. No prototype carries MAP_ID, so
this is the cleanest three-step `has()` case: one bit. The value is read and
discarded but **must** be read, or the length-prefix-less patch desynchronises.

**M93g then took the loom, closing the arc bar one.** Its banner test is
`instanceof BannerItem` — a **class** — and the obvious data stand-in is wrong
by exactly one item: every banner's prototype carries
`minecraft:banner_patterns`, set by the very lambda that constructs the
`BannerItem`, **and so does `Items.SHIELD`**. `BannerItem` has exactly one
construction site (the 16-colour `ColorCollection`) and `#minecraft:banners`
holds exactly those 16, which the generator asserts rather than assumes. Its
other two predicates are genuine **conjunctions** (`is(#LOOM_DYES) &&
has(DYE)`), each with its own removal bit — two rather than one shared, since
an item is in at most one tag and a shared bit would falsify the wrong
predicate.

**Seven of the eight single-input quick-moves are done**; nine of the 25 menus
route a shift-click. **Only `smithing` remains**, blocked on
`RecipePropertySet` off `update_recipes` (class C) — and note these sets are
*not* jar-derivable the way M91's smelting ones were, because a smithing
recipe's three ingredient slots are per-recipe rather than one flat
`ingredient` field.

> **⚠ Do NOT use `ItemSlot::enchanted` for the grindstone.** Its doc comment
> says `ItemStack.isEnchanted`; the assignment is `c.has_foil()`, and M43
> proved those differ (`ENCHANTMENT_GLINT_OVERRIDE` wins both ways). It
> compiles, reads correctly, and is wrong for exactly the cases that component
> exists to create. `hasAnyEnchantments` is also `ENCHANTMENTS` **or**
> `STORED_ENCHANTMENTS` — an enchanted *book* is the canonical grindstone input.

**A fixture rotted, loudly this time.** M90's "an untranscribed menu declines"
test named the **anvil**, which M93 transcribes — the rot M41 found in
`swingshot` and M43 in two `item_stack` fixtures, where it was *silent*. It now
asks the registry which menus are undone, proves the property on all of them,
and fails if that set is ever empty. **Two witnesses were wrong before the code
was:** a full anvil answers `None` (not `Some`-with-no-changes), so it is
paired with a one-free-input anvil that must answer `Some`; and `click_pickup`
*does* return `Some` with an empty change set, an asymmetry that is vanilla's.

**M93b is the part that generalises — and it is the M92 sweep applied to my own
code the same session.** M93a shipped with exactly the hole M92 names: all
eight witnesses hand-build an `ItemProps`, so `beacon_payment` could have been
wired to nothing and every one would stay green while a real beacon cross-moved
every diamond. `containershot` now calls the production `live_cmd::item_props`
and grades what it returns for real registry ids (`d1`), with a second witness
(`d2`) pinning something the same call must get right in the *other* direction,
because the negative alone would pass against a function that resolved nothing.
**The general sweep is still open** — grep for a `*shot` gate that builds a
struct production resolves from a table or the wire.

**A generated file was lying about itself, and the extraction caught it.** The
stonecutter needed M91's recursive tag expander, so it moved to
`tools/recipe_ingredients.py`; an extraction is only safe if provably inert, so
the check was to re-run `gen_smelting_inputs.py` and diff. **54 deletions.** The
data was byte-identical — what the diff showed is that `smelting_table.rs` says
*"Do not edit. Re-run the generator"* and carried five hand-added tests the
generator never emitted, **including the one pinning M91's own headline
finding**. The generator emits them now. The fifth was dropped deliberately: a
`.len()` assertion is fine hand-written and **vacuous once generated**, so the
guard moved to the generator's recipe-count floor, where a re-run cannot
recalibrate it.

**A surviving mutation found a hole spanning M93a**: nothing witnessed
`backwards` for *any* of the four menus, which is the difference between a
taken result landing in the hotbar's right-hand end and in the first free main
slot. And a fourth witness of the session was wrong before the code was —
**stone is both stonecuttable and smeltable** (slabs, and smooth stone), so the
disjoint pair is andesite/beef, with cobblestone pinned as M91's log one menu
over.

**The grindstone's second disjunct is why the `enchanted` warning above
matters.** `hasAnyEnchantments` admits an **enchanted book**, which is not
damageable at all — and `ItemSlot::enchanted` misses it twice over: it is
`has_foil()` (M43) *and* `isEnchanted()`, which reads `minecraft:enchantments`
alone while a book's live in `stored_enchantments`. `c.enchantments` was
already the union of both, so the correct bit was one line.

**A mutation caught M92's finding in my own new code**: the decoder's
damage-removal flag was untested by construction, because the only witness
constructed an `ItemSlot` with it already set. Now asserted from **bytes**.

**M93f's decode witnesses were written WITH the feature**, not after a
surviving mutation as M93e's were — and its battery came back **13/13 clean
first time**, which is what that bought. Their first run failed on the
**harness**: the shape table decides *walkability* and the interpretation
decides *meaning*, kept separate on purpose, so a fabricated test id absent
from `install_test_shapes` reads as unwalkable however well the interpreter
handles it. Production was never affected.

**Two mutation lessons from M93g.** One **survived and was shown equivalent
rather than fixed**: dropping a conjunction's second term changes no answer the
jar can produce, because every item in `#loom_dyes` also carries the component
— so `d9` pins the *coincidence* instead, and fires if a version ever breaks
it. Two others were real and shared a shape: **a witness on one of two mirrored
terms leaves the other free to be deleted** (the dye removal was witnessed and
the pattern one was not; both on the quick-move path and neither on the
plain-click path).

**And a timeout kill left a mutation on disk** — the battery restores in a
`finally`, which a killed process skips. **Grep for the mutation markers before
anything else after an interrupted battery**, and split batteries so each stays
inside the 10-minute tool cap.

**Measured:** 1773 → **1942 tests**, 0 failures, seven crates reporting (world
717, net 609, gpu 255, data 212, app 97, mesh 45, proto 11); `containershot`
27 → **76**; `inventoryshot` 152, `itemshot` 75, `handshot` 34, `swingshot` 97,
`mobshot` 246/246; `live --render-check` **22/22** validation ON, 0 errors
(re-run at M93q, the first of the arc to touch a render path); demo PNG
`2cc56b4acbfb92cb` byte-identical; **205 mutations across M93a–y, 199 killed,
2 shown equivalent, 1 alive by construction (named)**.

### M103 — the ghost recipe, and two vanilla quirks no Minecraft grid can show

M93y decoded `place_ghost_recipe` and nothing consumed it — the last
decoded-but-unrendered packet in this area.

**The item is sandwiched between two washes of DIFFERENT colours** — `0x30FF0000`
red *under*, `0x30FFFFFF` white *over*, both alpha 48. They land in different
halves of the container pass (the icons are a separate pass that runs between
them), hence a new `front_overlays` list. **And only the wash beneath widens**
for a big result slot; widening the veil too rings the icon in white.
`isBiggerResultSlot()` is **true by default**, false only for `InventoryScreen`.

**The families place differently:** shaped crafting **centres** a small recipe in
a big grid via `PlaceRecipeHelper`; shapeless fills the first
`min(ingredients, slots)` in order. A furnace ghosts its **fuel only if the fuel
slot is empty**. A stonecutter or smithing display ghosts the result alone.

**Two `placeRecipe` quirks no Minecraft grid can show**, each found by a mutation
that survived until a non-Minecraft fixture existed: the centring test is
**strict** and `<=` is indistinguishable on 2x2/3x3 (a 4x4 shows it); and the row
skip advances the row a **second** time, which needs `gridHeight >= 5` to matter
(a 6-tall grid shows it). My doc had claimed the strictness mattered generally —
corrected.

A witness was wrong twice more: a 3-wide 1-tall recipe centres **vertically** in
a 3x3, and the row skip advances **one** row rather than jumping to `startPos`.

**And the mutation harness gave a false SURVIVED** — second wrong verdict today
after M95's em-dash decode. A shapeless off-by-one reported SURVIVED in batch and
died immediately when run alone; the rest were run directly. *A harness wrong
twice is a detector to check, not to trust.*

**2101 tests**; containershot 89, inventoryshot 152, itemshot 75, handshot 34,
mobshot 246/246; **`live --render-check` 23/23** validation ON 0 errors (run
because `set_state` changed); demo PNG `2cc56b4acbfb92cb` byte-identical; **12
mutations, 12 killed**.

### M104 — the which-of-these overlay, and three clamps that round three different ways

M98 wrote the gap into `BookAction::Recipe`'s own doc — *"Rewo has no overlay,
so a right-click on a multi-recipe cell is reported and does nothing."* This
reads that note.

**`OverlayRecipeComponent.init` nudges the panel back on screen in whole 25-px
steps, three times, and the three roundings are not stylistic.** The horizontal
clamp truncates with a C-style `(int)` cast, so a positive quotient floors — an
overlay overhanging by 1..24 px is not moved at all, and one overhanging by 38
moves 25 and still overhangs by 13. The bottom clamp takes `Mth.ceil` of a
**positive** quotient and is the only one guaranteed to clear its bound. The top
clamp takes `Mth.ceil` of a **negative** one, and since `Mth.ceil` is a true
ceiling, `ceil(-0.6) == 0` makes it a complete no-op below one step. Same
function, opposite effect, decided by the sign alone. Reaching for a symmetric
"clamp into the box" diverges on the whole right-hand column.

**`centerY`'s `+ 13` is inert, and no fixture can catch it.** Every cell's `y`
is `31 + 25r`, so `y ≡ 6 (mod 25)`, and the two candidate bounds are 13 apart —
inside one quantisation step, so both overflows land in the same `ceil` bucket
for every cell and every count. The witness was written asserting the opposite,
failed, and became an exhaustive proof of inertness instead. It joins
`extractRenderState`'s unread `int border = 4;`.

**It opens on a right-click and accepts only left-clicks**, so a second
right-click closes it. **An open overlay is modal** — the overlay branch is an
unconditional `return true`, so a click on the arrows, the search box, the tabs
or the menu's own slots underneath all reach it and nothing else. **Selecting
does not close it** (only the else-branch calls `setVisible(false)`), which
reads like an oversight and is what makes the feature usable. **And it is a
snapshot, not a view**: `init` resolves everything once and `updateCollections`
leaves it alone, so crafting while it is open does not re-sort or re-grey it —
which is why `Open` is stored rather than recomputed.

Smaller inversions: the 4-or-5 row width keys off the **total** (16 recipes are
four rows of four, 17 are four rows of five); the padding is asymmetric (4/5 and
5/4); the button is 24 on a 25 pitch so there **is** a gutter a click falls
into; `Pos` is the ingredient's **centre**, because `scale(0.375F)` sits between
two translates; the ingredient cycle is **one** level where the cell's is two,
on the same clock; the button class follows the **menu**, not the display; and
shaped centres through `PlaceRecipeHelper` while shapeless is a bare
`i % 3, i / 3` with the 3 a literal.

**`blitNineSlicedSprite` became tested geometry** in `rewo_world::nine_slice`.
`rewo_gpu::screen::nine_slice` already exists and is left alone — different
pass, different vertex format, and **no unit tests at all** — so this is the
arithmetic on its own, with tests, rather than a silent second copy. On
`overlay_recipe` the tile-vs-stretch choice is **unobservable** (flat centre,
edge bands constant along their repeat axis), recorded so a green pixel gate is
not read as having graded it.

**The witnesses were wrong nine times and the code twice**, and the shapes
repeat: a binding shadowed 200 lines away made three gate witnesses probe the
recipe cell's corner (and the rename then missed the one using `c0y` alone); an
`any` over two right-hand corners could only see total failure; two fixtures
could not express their claim (a mutant equivalent *by construction*, and two
**symmetric** shaped fixtures blind to a transposition — a 2x1 recipe, centring
on one axis only, is what pins it); a control depended on what happened to be
underneath (a button's centre and the cell it covers are both 139); and one
witness counted quads instead of placing them.

**67 mutations, 65 killed, 2 proven equivalent.** A killed battery **left a
mutation on disk** when it hit the 10-minute cap and its `finally` never ran —
caught by grepping the markers first. And `cargo run -q` swallowed a compile
error, so a debug print that never appeared read as "branch not reached" rather
than "did not build" — third detector error of that shape in the log.

**2139 tests** (world 860, net 613, gpu 255, data 212, app 143, mesh 45, proto
11); `containershot` 89 → **96**; **`live --render-check` 24/24** validation ON
0 errors, with a new r24 that required splitting `book_quads_max` in two,
because the claim is a *difference* and one max cannot see it; demo PNG
`2cc56b4acbfb92cb` byte-identical. **Open:** overlay and recipe tooltips,
`tryPlaceRecipe`'s `lastPlacedRecipe` guard (unmodelled since M98), `useMaxItems`,
and the page counter text.

### M102 — the two crafting fills, and a fourth comment that described what the code did not do

M96 recorded one approximation and left another unrecorded, both in the same
eight lines. `hasCraftable`'s contents come from **two disjoint fills** —
`Inventory.fillStackedContents` (the ITEMS) and
`menu.fillCraftSlotsStackedContents` (the GRID).

**The range:** `Inventory.items` is menu slots **5..46** — not the 2x2 grid
(1..5, which arrives through the second fill) and not the craft **result**
(slot 0, which arrives through neither). M96 walked all 46, which double-counts
the grid *and* adds the result, so a recipe could read as craftable off its own
output.

**The predicate:** `accountSimpleStack` gates on `isUsableForCrafting` =
`!isDamaged() && !isEnchanted() && !has(CUSTOM_NAME)`. M96's comment named it and
applied nothing. **Fourth comment this session describing behaviour its code did
not have** (M93t's `setCanLoseFocus`, M96's note, `any_enchantments`' doc, this).

**`isEnchanted()` is the middle of three near-identical flags:**
`ItemSlot::enchanted` is `has_foil()`; `ItemSlot::any_enchantments` is
ENCHANTMENTS **or** STORED (the grindstone's `hasAnyEnchantments`);
`SlotText::is_enchanted` is ENCHANTMENTS alone and is the right one.
`any_enchantments`' doc claimed to be `isEnchanted()` too — corrected. An
enchanted **book** separates them, and M93 recorded this trap one field over.

**The fills differ in gating, not just range:** the crafting container is gated,
the furnace **block entity** calls bare `accountStack` and contributes its whole
container **including the result**. A damaged pickaxe counts in a furnace and not
on a grid.

A mutation deleting the craft-slot half **survived** — the fill sat in a
`PlaySession` path — so it moved to `crafting_contents`, taking the max-stack
lookup as a closure. M97's lesson, fourth application.

**2077 tests**; containershot 89, inventoryshot 152, itemshot 75, handshot 34,
mobshot 246/246; demo PNG `2cc56b4acbfb92cb` byte-identical; **12 mutations, 12
killed**; no render path changed.

### M101 — the caret blinks, and the field it blinks in never scrolled

M100 recorded the blink as a shared gap between the book's field and the anvil's.
Fixing it in the extracted renderer fixed both — and exposed two older bugs.

**`showCursor` is THREE conditions** — `isFocused() && isCursorVisible(millis -
focusedTime) && cursorOnScreen` — where M93t had only the first. The blink is
`/300 % 2 == 0` measured from `focusedTime`, which `setFocused(true)` resets and
`setFocused(false)` does not, so a freshly focused field shows its caret at once.
And `setFocused` is gated on `canLoseFocus || focused`: **the anvil sets that
false**, so its caret blinks as long as the screen is open where the book's stops
on losing focus.

**First older bug:** M93t's comment claimed `setCanLoseFocus(false)` and the code
did only `setInitialFocus`. Nothing pinned the focus for eight milestones,
because those lines sat in a path needing a `PlaySession` and so were unreachable
from a test — they are `anvil_field_new()` now.

**Second older bug, surfaced by the caret's own gate: the field never scrolled.**
Vanilla's `insertText → setCursorPosition → scrollTo` keeps the cursor visible;
Rewo's `set_cursor_position` cannot, because `scroll_to` needs a font width the
`EditBox` does not own. So `display_pos` never moved and a field typed past its
width kept showing the head of the string. **Before this the caret was drawn
anyway at a bogus x; with `cursorOnScreen` correct it vanished — which is what
made the gap visible.** `follow_cursor` is the missing half, called from every
input path, for both fields.

The headless renderer takes a **fixed clock of 0**: a blinking caret would render
the same scene two ways depending on when the gate ran.

**2068 tests**; containershot 89, inventoryshot 152, itemshot 75, handshot 34,
mobshot 246/246; demo PNG `2cc56b4acbfb92cb` byte-identical; **11 mutations, 11
killed** — two only after witnesses that could reach them were written.

### M100 — the search field's text, and a nine-slice that degenerates to two blits

The field typed and filtered since M99 and drew nothing.

**The nine-slice is two blits, measured not assumed.** `widget/text_field` is
200x20 border 1 — but the PNG is **1-bit paletted**, exactly two colours (border
160-grey, white when focused; interior black). Every one of the nine regions is
uniform, so a stretched 1x1 source is **pixel-identical** to a tiled one: one
blit of the whole rect from a border texel, one of the interior from a centre
texel, and the 1 px the first still shows *is* the border.

**The hint goes on FOCUS, not on the first character** —
`displayed.isEmpty() && !isFocused()` — so clicking an empty box blanks
"Search..." before you type. It is a styled component (GRAY + ITALIC), so its
own colour beats the field's white; the italic is not reproduced (no slant in
the bitmap pass).

**The bordered case decides all three text numbers and none is obvious:**
`textX = getX() + 4`, `textY = getY() + (height - 8) / 2` (**3**, not `getY()`),
`getInnerWidth() = width - 8` (**73**, not 81 — the inset comes off both ends).

**Third meaning of `WidgetSprites::get` on one screen:** the field passes
`(isActive(), isFocused())`, exactly what the names say, where a tab passes
`selected` as *focused* and the filter passes `filtering` as *enabled*. One
convention across this screen is wrong two times out of three.

The renderer is **extracted** from the anvil's, not copied — a second copy of
the caret-x/insert/selection arithmetic is three chances to drift by a pixel.

**A staging trap:** `io.open(p,'w')` truncates when the file object is created,
*before* its argument is evaluated — so `open(p,'w').write(sub(open(p).read()))`
wrote an empty `server.properties`, Minecraft regenerated a default, and the run
died with a bare "Failed to initialize server". Read first, then write.

**2059 tests**; containershot 89, inventoryshot 152, itemshot 75, handshot 34,
mobshot 246/246; **`live --render-check` 23/23** validation ON 0 errors, r23
rising 8 → 10 quads as the field's two blits reach the windowed client; demo PNG
`2cc56b4acbfb92cb` byte-identical; **9 mutations, 9 killed**. **Open:** the caret
does not blink (`isCursorVisible`, 300 ms) — a shared gap with the anvil's field,
not a new one.

### M99 — the search box, and a suffix array the consumer does not need

`updateCollections`' second stage (M93z's unfed `matches_search`) plus typing.

**The suffix array is unnecessary here, measured rather than assumed.** Vanilla
indexes every *suffix*, so a search is a substring match; the array exists for
speed and for a defined result order, and **neither is used** — the result goes
into a set read only via `contains`, and survivors keep their existing order
(`removeIf`, not a re-sort). `contains` is exactly equivalent.

**Two indexes, a colon picks between them:** no colon → **names only** (the ids
are *not* searched, though the tree holds them); a colon →
`namespace ∩ (path ∪ name)` with both halves **trimmed**. For Rewo a result's
"tooltip lines" are its display name alone — exact, since a recipe's result is a
bare id with no components. **An empty query skips the stage** rather than
matching everything: a collection with no searchable text is kept by the skip
and dropped by a match-everything reading.

**A duplicate flag was the bug and removing it was the fix.** The first cut kept
`search_focused` on `BookState` *and* mirrored it into the `EditBox`, whose
`can_consume_input` gates keystrokes on its **own** flag. A test caught it
(typing produced nothing); then a mutation deleting the mirror **survived**,
because `book_press` needs a `PlaySession`. So the flag is gone — `focus_change`
is a pure function of the hit and the `EditBox` is the only owner. **Shrink the
untestable surface rather than pretend to cover it.**

Two more caught by tests: the field takes the **book's** max length (50), not
`EditBox::default`'s 32, so `ScreenState`'s `Default` is written out rather than
derived; and a witness could not isolate the colon query's name half because
`plank` matches both "Wooden Plank" and `oak_planks` — third time this session a
fixture could not express its own claim.

**2053 tests**; containershot 89, inventoryshot 152, itemshot 75, handshot 34,
mobshot 246/246; demo PNG `2cc56b4acbfb92cb` byte-identical; **17 mutations, 17
killed**. **Open:** the field's *text* is not drawn — it types and filters, and
nothing renders the characters or caret (M93t's anvil seam, one screen over).

### M98 — the book takes clicks, and one it does not want is still swallowed

Tabs, pages, the filter, hover, and the two serverbound packets. The book had
been drawable and inert since M94.

**The order is a contract and it is not the draw order:** the **page** first
(arrows, then cells), then the search box, then the filter, then the tabs — and
the whole book before `super.mouseClicked`, so it dispatches ahead of every
other screen widget. **And a second rule in the else-branch:** a click the book
does *not* want is still **swallowed** when the window is too narrow and the
book is open, because there the book covers the menu.

**Four inversions.** A **selected tab's hit rect does not move with its sprite**
— the 2 px shift is draw-time only, so its leftmost two columns are painted and
unclickable. The **magnifier counts as the search box** and its rect *overlaps*
the box rather than abutting it. A click anywhere but the **page** unfocuses the
search field, because `setFocused(false)` sits unconditionally in the
else-branch and the page path returns before reaching it. And **switching tabs
resets the page**, while re-selecting the tab you are on does nothing
(`selectedTab != button`).

**The packets:** `recipe_book_change_settings` carries an ordinal and **both**
flags, read out of the local settings rather than passed — so toggling the
filter re-reports open, and vice versa, because the server persists both.
`place_recipe` places **the recipe the cycle is showing**, not the collection's
first. A right-click on a multi-recipe cell is consumed and does nothing (no
overlay; placing an unchosen recipe is worse than nothing).

**The hover comes from the same `book_hit` the press uses**, with the cursor
converted to book space once in `apply_screen` — M95's note that this needed the
renderer was wrong.

**A surviving mutation was a weak fixture, not an equivalent mutant** — M93z's
lesson again: swapping `clamp_page` for a clamp-to-last-page survived a
**one-page** fixture, where reset-to-front and clamp-to-last are both 0.

**2039 tests**; containershot 89, inventoryshot 152, itemshot 75, handshot 34,
mobshot 246/246; **`live --render-check` 23/23**, validation ON, 0 errors (run
because `apply_screen`'s signature changed and a new dispatch sits at the front
of the click chain); demo PNG `2cc56b4acbfb92cb` byte-identical; **15 mutations,
15 killed**. **Open:** the search box focuses and does nothing (needs the recipe
search tree); no page counter, overlay, ghost slots or tooltips; `useMaxItems`
is always false.

### M97 — closing M96's own recorded gap: the book's derivation, graded

M96 shipped `hasCraftable` graded at its two **ends** — the solver's tests
below, the gate's chrome witness above — and nothing in between. The arithmetic
turning an inventory into a per-slot flag, which is what M96 added, was
untested: M92's shape, M93b's close.

**The obstacle was structural.** `PlaySession` owns a socket and cannot be built
in a test, so the fix is M71's lesson rather than a fixture — *logic in a place
with no test module is untestable, so move it.* `live_recipe_book` is the
session half (lookups) and `book_render_from` the derivation (grouping, tab
membership, paging, cycle, craftable), taking plain values.

Nine tests name rules neither end could see — notably that an entry with **no**
requirements is never craftable while one declaring an **empty** list is (the
distinction `canCraft`'s opening line makes, which the solver alone cannot
express because it never sees the entry), and that asking about one collection
does not spend another's items (a consuming solver would light the first slot
and grey the second).

**10 mutations, 10 killed** — including *"nothing is ever craftable"*, which is
exactly M96's pre-state and would otherwise have been indistinguishable from it.

**2019 tests**; containershot 89, inventoryshot 152, mobshot 246/246; demo PNG
`2cc56b4acbfb92cb` byte-identical; no render path changed.

### M96 — the craftable solver, and two of vanilla's guards that do not matter

`StackedContents` ported, fed and wired — the blocker M35, M94 and M95 all
named. Every recipe slot wore the *uncraftable* chrome because nothing could
answer the question.

**It is a bipartite matching, not a subtraction.** Walking the ingredients
decrementing a count is wrong whenever accept-sets overlap: one `#planks` slot
and one `oak_planks` slot, against a stack of oak and a stack of birch, is
craftable only if `#planks` takes the birch. Vanilla finds it with augmenting
paths (`RecipePicker`).

**Two of my own witnesses were wrong before the code was** — both claimed one
item *type* satisfies one slot only, so a stack of 64 dirt could not fill a
nine-slot recipe. `try_pick` loops, `take`s per satisfied ingredient, and
`hasAtLeast` re-reads the decremented amount: the matching is over **(item,
ingredient) pairs** and what runs out is the count, not the type.

**Two mutations survived and both are genuinely equivalent**, which is the
opposite of the natural assumption. Transposing either bit-matrix index is a
**relabelling** — each region is read and written only through its own index
function and both formulas are bijections onto the same range (the module doc
claimed the reverse; corrected in place). And dropping the `count > 0` filter is
an **optimisation**: a zero-count item enters the matrix and `hasAtLeast`
refuses it anyway. Settled by a **brute-force oracle** sharing no code, order or
bit layout — 27,648 problems (3 item types × counts 0..=2 × 3 slots × all 8
accept-subsets × capacities 1..=2), all agreeing — so "equivalent" is a
measurement, not a claim.

**The wire half:** M93y walked `craftingRequirements` and discarded it, so the
ingredients are captured now — each a `HolderSet`, an inline id list **or a tag
name**, and a tag resolves against `update_tags`, **which M69 decoded and
nothing had consumed**. An unknown tag yields nothing, so its ingredient is
unsatisfiable: greying a recipe you could make is a smaller lie than lighting
one you cannot. `canCraft` opens `craftingRequirements.isEmpty() ? false`, so a
recipe carrying none is **never** craftable — the two states stay distinct.

**Recorded approximation:** vanilla fills from the inventory **and** the open
menu's craft slots; Rewo counts the inventory alone, so a recipe whose last
ingredient sits on the grid reads uncraftable. The craft-slot range differs per
menu class and guessing it would be a confident wrong answer.

**Process:** a hung mutant left its test binary holding the link output, so the
*next* mutation reported BUILD-FAIL rather than the previous one's hang — and a
botched harness left a mutation **on disk**, caught only by a grep. The harness
reaps strays now and counts a hang as a kill.

**2010 tests**; `containershot` 89, `inventoryshot` 152, `mobshot` 246/246; demo
PNG `2cc56b4acbfb92cb` byte-identical; **13 mutations — 11 killed, 2 proven
equivalent**. **Open:** the inventory→solver→flag derivation is graded at both
ends but not end to end (driving it needs a `PlaySession` — the M92/M93b sweep
shape); nothing can click the book.

### M95 — the recipe book's items, and the tab structure M93z got wrong

Tab icons and recipe results, on the book's origin — plus a correction to each
of the two milestones before it.

**M93z modelled the tabs wrong.** Its `Tab` enum was the four
`SearchRecipeBookCategory` values, and those are *the search tab of each of the
four books*, not the tabs within one. Each book has its own hand-written list —
**crafting five, furnace four, blast furnace three, smoker two** — the first of
each a search tab with a **compass** icon. M94 therefore drew four tabs on every
book. And the search flag must be **explicit**: a smoker's search tab holds
exactly one category, the same one its single category tab does, so a
"several categories" heuristic is right for three books out of four.

**M94 left the menu's icons behind** — it threaded the displacement through the
panel and the hover and missed `menu_slot_rects`, so with the book open every
slot icon sat 77 px left of its slot. M90's reason: *a function taking bare
numbers does not look like it belongs to the menu.*

**The items:** `getDisplayStack`'s cycle is **two levels** (`% entryCount` picks
the recipe, `/ entryCount` picks which of *that* recipe's display items), so
three recipes with two forms each cycle through six, not three;
`resolveForStacks` resolves only the context-free arms (`Item`, `Stack`,
`Composite`, and `WithRemainder`'s **input**) and yields **nothing** for the six
that need a `ContextMap`, because an arbitrary tag member would be a confident
wrong answer; the shadow copy is the **same stack drawn twice**.

**Three gate findings.** b8 measured 26 icons against the 27 it named — the
missing one was M93z's error surviving in the gate's own **fixture**. **Two
mutations survived b8–b11**, one putting the book's icons on the menu's origin
and one leaving the menu's icons centred: **counting icons cannot see a wrong
origin**, so b12/b13 measure positions. And b13's first draft told the two
apart **by position**, which is circular when position is what it measures.

**A harness bug of M93v's family**: the mutation runner's `'PASS —' in out` used
`text=True`, which decodes with the Windows locale codec, so the em dash became
mojibake and **every gate verdict read KILLED whether or not anything failed** —
which is what hid the two survivors. Uses the exit code now. Third detector bug
of this arc, all the same shape: *cannot tell "passed" from "could not tell".*

**1992 tests**; `containershot` 83 → **89**; `live --render-check` 23/23,
validation ON, 0 errors; demo PNG `2cc56b4acbfb92cb` byte-identical; **14
mutations, 14 killed**. **Open:** `hasCraftable` is still false everywhere (it
needs `StackedItemContents`); nothing can click the book; no search box, page
counter, overlay popup, ghost slots or recipe tooltips.

### M94 — the recipe book renders, and two errors only the windowed client could show

M93z built the model; this draws it. Panel, tabs, recipe slots, arrows, filter.

**Opening the book MOVES the menu**, so this is not a pure addition:
`updateScreenPosition` swaps `(width - imageWidth) / 2` for
`177 + (width - imageWidth - 200) / 2` — 77 px for a 176-wide panel. Rewo
measures slot hit-testing, slot icons and the hover box from that origin, so
drawing the book without the shift leaves the menu centred under a book that
overlaps it and every click lands on the wrong slot — **silently**, because the
render still looks plausible. `topPos` does not move. The draw and the hit test
now resolve through one `Placement`, M89's one-accessor rule applied to
geometry.

**Two design errors, both found by `live --render-check`, neither visible
headlessly:**

* The book was hung off `ContainerPanel` — which is `None` for the player's own
  inventory (the path `inventoryshot` pins), and **the player's inventory is one
  of exactly four screens that HAS a book**. So it was undrawable in its
  commonest case, and `containershot` structurally cannot see that: it only ever
  drives an open container.
* The gate's "crafting table" fixture used menu type **13, which is
  `enchantment`** — same 176x166 size, so every headless witness measured what
  it expected and passed. `crafting` is 12.

Both are the M86 shape. **Run `--render-check` on any milestone that adds a
render path.**

**Four chrome inversions:** a tab's sprite tracks **selection, not hover**
(`get(true, this.selected)` hard-codes `enabled` and passes `selected` as
*focused*), while the filter toggle inverts the same record the other way
(`get(filtering, hovered)` — and the sprite names make reading it as the
widget's own state easy, giving a button that never changes when clicked); a
selected tab shifts 2 px left **and takes its icon with it**; the stacked-recipe
look is **two items at (5, 3)**, because vanilla draws a copy at `offset + 1`
then *decrements* offset — reading it as applying to the back copy gives (4, 4),
which renders as one item; and the panel is sampled from **(1, 1)**.

**The gate's rewrite was the instructive half. b1 passed while measuring nothing
it claimed** — it probed the book's centre and compared open against shut,
reading `[0,0,0]` against `[255,255,255]`, and neither was the book: the probe
sat on a recipe slot's black border, and the *control* frame had the menu over
that position because an open book moves the menu. **A frame diff may not let
its control change with its subject.** Every witness now names its sprite's
value read out of the PNG (`tab.png` 139 vs `tab_selected.png` 198,
`slot_craftable` 139 vs `slot_uncraftable` 106) — "different from the backdrop"
would pass with every tab drawing the same art. And a mutation deleting
`take(view.shown)` **survived the gate** and was killed by the model's test,
because the gate's fixture sized its slot vec to `shown` and made the guard a
no-op.

**1979 tests**; `containershot` 76 → **83**; **`live --render-check` 22 →
23/23**, validation ON, 0 errors; demo PNG `2cc56b4acbfb92cb` byte-identical;
**18 mutations, 18 killed**. **Open:** no items in the book (grid results, tab
icons); `hasCraftable` is `false` for every collection so every slot wears the
uncraftable chrome (it needs `StackedItemContents`, and guessing `true` is
worse); nothing can click it, so tab/page are pinned to 0 and hover never
reaches the arrows or filter; no search box, page counter, overlay popup or
ghost slots.

### M93z — the recipe book's UI model, and a filter button that toggles one stage of three

M93y decoded the packets and named the book as the subsystem that had to
follow. This is its **model** — geometry, tabs, collections, filtering,
pagination. **The render is separate and not in it**, on M63's split.

**It is positioned against the WINDOW, and nothing else Rewo draws is.** Every
other screen is panel-relative. `getXOrigin` is `(width - 147) / 2 - xOffset`,
centred on the *window* then pushed left 86 to flank the menu — and **`xOffset`
collapses to 0 on a narrow window**, which is what makes the book cover the menu
rather than hang off the edge. Deriving its origin from the open menu's panel is
right at one window size and wrong at every other, invisibly until a resize.

**`updateCollections` is three `removeIf` stages and the FIRST IS
UNCONDITIONAL** — the filter button toggles only the third (`hasCraftable`).
Read as gating the filter, it takes the first (`hasAnySelected`) with it, and
"show all recipes" then lists furnace recipes in a crafting table.

**The crafting tab lists `equipment` first**, not in the registry's id order
(building_blocks, redstone, equipment, misc) — `includedCategories()` is a
separate hand-written order, so deriving a tab's contents from ids reorders
every crafting collection. **Three of the 13 categories belong to NO tab**
(stonecutter, smithing, campfire — those screens have their own UI), so the
lookup must be allowed to answer nothing. **Zero collections give ZERO pages**,
and `clamp_page`'s `totalPages <= currentPage` resets an index *equal* to the
count, to the **front** rather than the new last page.

**Ordering is deliberately not a contract**: a group takes its first-seen
member's position, and insertion order is preserved because a stable book beats
an arbitrary one — **not** because vanilla guarantees it (its input is a
`HashMap`'s `values()`). The opposite of M93s's stonecutter, where the index a
click sends made order load-bearing.

**The witness was wrong before the code, again**: it shrank to 20 collections —
**one** page, whose last index *is* 0 — and asserted the reset was not to "the
new last page", so `assert_ne!(0, 0)` failed and the fixture could not have
expressed its claim either way. Now it shrinks a five-page list.

**1956 tests**; `containershot` 76, `mobshot` 246/246; demo PNG
`2cc56b4acbfb92cb` byte-identical; **10 mutations, 10 killed**. It also found
**§0.0's own drift running in reverse** — the prose was current through M93t
while the coverage number was four milestones stale at 109/0/32, because a
milestone that ships a finding writes the paragraph and forgets the table.
**Open:** the book's render (panel, tabs, 20 grid buttons, arrows, the search
field on M93t's `EditBox`, the filter toggle), ghost placement into container
slots, and the two serverbound packets — reachable now, still unsent.

### M93y — the recipe book's decode, and a class-C claim that IS one

Four packets decoded into session state, plus the `SlotDisplay` (11 variants)
and `RecipeDisplay` (5) trees. **The class-C label here is correct** — unlike
the four M91–M93u overturned — and saying so matters: the book is a tabbed,
searchable, filterable list with ghost placement, none of which exists. This is
the half that comes first, on **M63's split**: decoding needs no listening.

**Dispatched rather than left resolved-but-ignored, and M74's check is why** —
it caught the ids the moment they resolved and named the class the coverage doc
keeps at **zero**: a packet whose id resolves and whose body is dropped reads as
*handled* to every grep, which is worse than absent.

The registries are **built-in**, so they come from the report (M92's rule) — and
the alphabetisation trap bites harder here than in M64, because the variants
have **different body lengths**, so a wrong table **desyncs the reader
mid-packet** rather than mislabelling. `group` is `OPTIONAL_VAR_INT`, the `+1`
family in optional form (**0 is absent; group 0 rides as 1**). A shaped recipe's
**width and height precede** the ingredients they describe. **`replace` clears
the book** — true on join, false per unlock.

**Verified against a real server, not only its fixtures.** The nine decode tests
drive bytes *I* wrote; a temporary counter against a live 26.2 server showed the
book reaching one entry on join, through the production path. Worth doing
because **"no warning" is also what a packet that never arrived looks like** —
the render check was green either way, and only a *positive* assertion about
what was decoded tells them apart.

1942 tests; coverage **110/0/31 → 114/0/27**, class C 20 → **16**; `live
--render-check` 22/22 validation ON 0 errors. **Open:** the book's UI (its
search field now has M93t's `EditBox` to build on) and the two serverbound
packets, unsent because nothing can yet click what would send them.

### M93x — the trade button's chrome, and reading WHICH witness fires

`Button.Plain`'s `extractDefaultSprite` — `widget/button` nine-sliced from a
200×20 sheet with border 3, empty label. **Only two of `WidgetSprites`' four
cases are reachable**: vanilla toggles the button's `visible` and never its
`active`, so a row past the end of the list draws *nothing* rather than a greyed
one.

**The slicing is the find.** At 88×20 on a 200×20 sheet the height matches, so
the nine-slice degenerates to horizontal-only — and vanilla's `NineSlice`
**tiles** rather than stretches, so a narrower button draws **one partial
tile**: the middle is a 1:1 slice of the sheet's first `w - 6` face pixels.
"Scale the middle" would resample every pixel of the face and blur it. The
witness that pins it: the button's **last column is `(0,0,0)`**, source x 199 —
the sheet's black border — where a naive 1:1 blit from x 0 would give
`(112,112,112)`.

**The transferable part is which witness fired.** All three mutations died, but
inverting the hover pair was killed by **z3, not z2** — because z2 asserted only
that the two frames *differ*, which is symmetric. Exactly M93t's x5 flaw, and it
surfaced *only* because the kill came from the wrong witness. **Reading which
witness a mutation kills is worth as much as reading whether one did.**

1929 tests; `containershot` 73 → **76**; `live --render-check` 22/22 validation
ON 0 errors. The merchant is complete; the one remaining limit is not a widget
but the component-predicate decline.

### M93w — the discounted price pair, and an override that defeats a rule

`extractAndDecorateCostA`. **One icon, not two** — `fakeItem` is called once,
outside the branch, with the **modified** cost, so the discounted display is two
*numbers* over a single item. The strikethrough at `+7` crosses the **first**
number rather than the gap, because the labels are right-aligned into the icon's
16 px box. And **a count of 1 normally draws nothing**, so the
`count == 1 ? "1" : null` override exists *solely* to defeat that rule — passing
`null` throughout drops a digit exactly when a discount has reached 1.

**A witness had to be narrowed rather than fixed.** It first claimed the two
digits as well as the strikethrough and measured **0 changed pixels** —
correctly, because the gate's frame builds the **panel** and the count labels
come from `screen_icons`, which it never calls. M45's shape again: a gate
reimplementing a slice of the app's setup misses what lives outside it. The
strikethrough is a panel overlay and is witnessed; the digits are graded at the
model level, and the gate now says so in a comment so the next reader does not
read it as an omission.

One equivalent mutant, labelled in the code: `icon_for` ignores the count, so
which count the cost-A icon call passes cannot matter.

1927 tests; `containershot` 71 → **73**; `live --render-check` 22/22 validation
ON 0 errors. **Open on the merchant:** only the trade button's own
`Button.Plain` chrome, and the component-predicate decline.

### M93v — the XP bar, and a blit argument I read as a size

M93u recorded this as blocked on `VillagerData`'s thresholds and
`getFutureTraderXp`. **Neither was.** The thresholds are five ints, and the
future xp is *derived by vanilla itself* — `updateSellItem` matches the payment
slots against the offers and takes the matched offer's xp.

**`traderLevel < 5` gates the background too**, so a maxed villager shows
nothing rather than a full bar; `getMinXpPerLevel` and `getMaxXpPerLevel` both
return **0** outside the levelling range, so their difference is the bar's
divisor and only the two guards keep it safe; the fill is the fraction of the
**level**, not the career; and `getRecipeFor`'s `selectionHint > 0` is
**strictly** greater, so selecting the *first* trade falls through to the scan.

**Two mutations survived and both were real.** One was M71/M93t's shape — logic
in `live_cmd`, which has no test module — now `satisfied_offers`. **The other
is a lesson about reading a signature**: mutating the result segment's source
offset changed *nothing*, because I had read
`blitSprite(sprite, 102, 5, u, v, x, y, w, h)`'s first two arguments as the
source **rect's** size when they are the **sheet's**. Every segment was drawing
the whole bar squeezed into its width, so the offset could not matter. **A
surviving mutation is a question, not just a verdict** — what it asks about is
sometimes not what you mutated.

**And an instrument failure of my own**: the shell totalling test counts used
`grep -v "0 passed; 0 failed"`, which matches `71`**`0 passed`**`; 0 failed`, so
it silently dropped `rewo-world` the moment its count hit a multiple of ten.
M91's finding in the measuring tool rather than the build — the only signal was
a total moving the wrong way.

1924 tests; `containershot` 67 → **71**; `live --render-check` 22/22 validation
ON 0 errors. **Open:** the discounted-price pair with its strikethrough, and a
cost carrying a component predicate declines rather than guesses (M41 has a
digest, not per-component values).

### M93u — the merchant, and the fourth class-C claim to fall

The coverage doc filed `merchant_offers` as class C. It needed nothing Rewo had
not built — `ItemStack` (M34/M41) and the `TypedDataComponent` walker M52e
wrote for `can_place_on`. **That is four this arc, and the reasons differ**,
which is the part worth keeping: M91's furnace recipes and M93s's stonecutter
list were **jar data**; M93's merchant quick-move **never consulted** the
packet; and here the data really **is** server-rolled — only the decode was
mislabelled. So "blocked on a packet we don't decode" deserves a check against
what the packet carries *and* what decoding it would cost.

**Traps:** the order is **costA, result, costB** (the sold item sits *between*
the costs, while every constructor lists them costA/costB/result); the numerics
are `writeInt`, **fixed big-endian** in a var-int protocol, so a var-int reading
turns a discount into a surcharge; and `Item.STREAM_CODEC` is `holderRegistry`,
**raw 0-based** — the fifth appearance. In the price, `demandDiff` clamps at 0
from below while `specialPriceDiff` is added *after* and is not floored (that is
the discount), and **only cost A is modified**.

**The scroll is not the stonecutter's with new numbers**: `scrollOff` is an
**offer index**, one notch is one offer, and the drag rounds the index rather
than a fraction. The thumb's bottom override is load-bearing for **short**
scrollable lists (8/9/10 offers land at 91/106/111) and redundant for long ones,
where `min(113, …)` caps an overshoot — the opposite of what I first asserted,
and only visible by computing both regimes.

**Reading the render loop caught two offsets no witness covered**:
`offerY = yo + 16 + 1` against the buttons' `+ 2`, so a row's items sit one
pixel **above** its button; and `sellItem1X = xo + 5 + 5`, so cost A adds the
button's 5 **twice**. And M93s's lesson landed twice — the arrow's x was wrong
(`5 + 5 + 20` for `xo + 5 + 35 + 20`), and when the witness failed again I
explained it *wrongly* rather than re-reading the sprites.

A surviving mutation is **equivalent here and not in vanilla**: dropping the
visibility guard changes nothing because this computes `row = i - scroll_off`
directly, where vanilla's `offerY` advances only inside the drawn branch.

1916 tests; `containershot` 63 → **67**; `live --render-check` 22/22 validation
ON 0 errors; coverage **110/0/31**, class C 21 → 20. **Open:** the XP bar's fill
(needs `VillagerData` thresholds and `getFutureTraderXp`) and the
discounted-price pair with its strikethrough.

### M93t — the EditBox, a subsystem Rewo never had, and a red band

M93n shipped the anvil's semantics and recorded that **nothing could type** —
Rewo read `PhysicalKey` and never a character. This is `EditBox`'s editing core
plus the `KeyEvent.text` seam, wiring the anvil end to end.

**The buffer is `Vec<u16>` and that is not fussiness**: every index in vanilla's
EditBox is a Java String index, and one rule — `isHighSurrogate(charAt(max - 1))`,
which stops a truncation splitting a pair — is only *expressible* in UTF-16.
M93n had already counted the anvil's 50 in code units for the same reason.

Findings that read backwards: `insertText`'s room is a **double negative**
(`maxLength - length - (start - end)`, so the selection's width is added *back*);
`setValue` truncates with **no** surrogate check where `insertText` has one; an
**uneditable box still swallows** backspace, because `return true` sits outside
the `if (isEditable)`; Insert and the vertical arrows share the **`default`**
label and so are treated as unrecognised; **word motion is not symmetric**, so
Ctrl+Left then Ctrl+Right does not return you; and the four shortcuts need
control down **and shift up and alt up**.

**`AnvilScreen.keyPressed` reaches `super` only when the box neither handled
the key nor could have** — so with an item in slot 0 **every non-escape key is
swallowed**: E does not close the anvil, a number key does not swap, Q does not
drop. That reads like a bug and is exactly what typing requires.

**A red band said the chrome was missing.** `anvil.png` carries a pure
`255,0,0` band exactly where the name field goes, and `extractBackground`
covers it with a sprite chosen by slot 0. Rewo drew the panel and not the
sprite, so the first run of these witnesses read `[255,0,0]` for "the bare
panel". **A placeholder in a vanilla texture is a deliberate signal.**

**Three mutations survived with three different verdicts**, which is the
transferable part: the swallow rule was a **real gap** (it lived in `live_cmd`,
which has no test module — M71's finding — and is now `anvil::key_consumed`);
the field-background inversion was a **real gap behind a symmetric witness**
(x5 asserted the two frames *differ*, so swapping them passed — it now names
the values, read out of the PNGs); and `deleteWords`' selection guard is
**equivalent**, because `deleteCharsToPos` carries the same check and vanilla
is doubly guarded too.

1899 tests; `containershot` 58 → **63**; `live --render-check` 22/22 validation
ON 0 errors; demo PNG byte-identical. **Open:** the clipboard is **in-process**,
not the OS's (no crate pulls one in, `winit` exposes none); no IME pre-edit; no
click-to-position or drag-select inside the field.

### M93s — the stonecutter, and an order that is a wire contract

The plan called this widget "genuinely class-C (`update_recipes`)". **It is
not** — the third such claim this arc to not survive the decompile, after M91's
furnace recipes and M93's merchant quick-move. The pattern is worth carrying:
*"blocked on a packet we don't decode" deserves a check against what the packet
actually carries*, because for vanilla content the answer is usually in the jar.

**The contents were never the hard part; the ORDER is, and it is part of the
wire contract.** A click sends an *index*, and the server resolves it against
`selectByInput` — a **filter**, which preserves the master list's order. Get the
order wrong and every click cuts a different block than the one drawn, **with no
error anywhere**: M64's alphabetisation trap somewhere nastier, because there
the ids merely came out wrong while here the server acts on it. It reproduces
because `RecipeManager.prepare` loads into a `SortedMap<Identifier, _>` — and
**`Identifier.compareTo` is path first, then namespace**, not the combined
`namespace:path`. The generator sorts by the file stem explicitly rather than
the filename: those agree only because `.` (0x2E) is below every character
`[a-z0-9_]` uses.

**One cell has three y-origins** and vanilla means all three — `+2` for the icon
/ highlight / tooltip, `+1` for the chrome and the cursor, **`+0` for the
click**. The first witness called the top two pixel rows "clickable but not
highlighted"; they are not. Both boxes are 18 tall on an 18 pitch, so they
**tile** — the offset is a *shear*, not a gap — and those rows highlight the row
**above**. A click lands one row *below* the lit cell at every boundary, and
away from a boundary they agree, which is why it is easy to miss. The scrollbar
likewise has three origins (grab `+9`, drag track `+14`, draw `+15`) and the
drag divides by 39 while the draw multiplies by 41, so **vanilla's thumb
overshoots its own track by two pixels**.

**The fourth detector error of the arc, same shape as the other three.** `w2`
proved "cell 6 draws no chrome" by comparing against the bare panel — and
`recipe_selected.png`'s centre is `(81, 73, 58)`, *exactly* what
`stonecutter.png` reads at that probe, so a cell 6 wrongly drawing a **selected**
chrome would have passed. The control is now a twelve-recipe view, differing in
one thing only. Reading the sprite PNGs' pixels *before* writing the witnesses
is what made the rest sound. Also: `mouse_gui` is GUI pixels and the first cut
converted the other way, so the hover never landed.

**A surviving mutation was a real gap, not an equivalent mutant** — swapping
`selected` and `hovered` changed nothing, because the orderings differ *only* on
a cell that is both and no witness hovered the selected cell.

1879 tests; `containershot` 52 → **58**; 6 mutations, 6 killed; demo PNG
byte-identical. **Open:** no tooltip on a recipe button, and the datapack caveat
now has teeth — a pack that *reorders* stonecutting recipes makes a click cut
the wrong block.

### M93r — the self-calibrating-witness sweep, and what it did NOT find

M93q's closing line asked for a sweep of anywhere a `*shot` witness computes an
expectation from a `pub const` the renderer also reads. Run over ~1,400
witnesses in 34 gates: **96 of 480 SCREAMING consts are read by both a gate and
production**, narrowed to value-shaped ones, each checked for **value** (against
the decompile) and for **pinning** (by mutation, the only real evidence).

**Three real holes, all with correct values** — process debt, not a rendering
bug. `GLINT_STRENGTH`: `handshot`'s `n3` asserts every glint vertex carries it
while *reading* it; mutating `0.75 → 0.55`, a 27% change in the foil's alpha,
left `handshot` (34), `inventoryshot` (152) and all of `rewo-gpu`'s tests green.
`DARK_GRAY`: same shape in the advanced tooltip, and the mutation used was
`0x555555 → 0x3F3F3F` — **the exact wrong grey M93p shipped for the loom**,
which is the point: a plausible flat grey is what a guess produces, and no
amount of pixel-reading catches one when the reader shares the value.
`DYE_DIFFUSE_COLORS` is a different failure — **duplicated** across `rewo-data`
(banners/signs) and `rewo-gpu` (fish, the sheep derivation), and neither crate
depends on the other, so **no test *could* have compared them**; the agreement
test has to live in `rewo-app`.

**What it did not find matters as much, and both shapes look like the bug.**
An **enum comparison** (`== PotWobble::Positive`) names which outcome is
expected — an identity, not a transcribed value. And a constant used as a
**search key** self-guards: `blockentityshot` finds a sign by
`line_height == HANGING_LINE_HEIGHT` then asserts `line_y` against literals, so
a wrong constant makes `find` return `None` and fails. Most value-shaped consts
are also pinned already, **often inside the gate rather than a unit test**
(`BAR_W == 182`, `scale_armor == 0.16`, `(ANCHOR_ACCEL - 0.0139…).abs() <
1e-17`) — a first detector looking only in `#[cfg(test)]` called all of those
unpinned, so **the mechanical search has a high false-positive rate and its
shortlist must be read**.

**The best pin in the codebase is a derivation, not a literal.**
`SHEEP_WOOL_COLORS` is self-calibrating in `mobshot` and needs no fix, because
`entities.rs` pins it *by rule* — `floor(diffuse * 0.75)`, white overridden to
`0xE6E6E6` — and **12 of its 16 rows would differ under `round`**, so the test
proves the rule rather than the numbers. Prefer that form for any derived table.
Also: there are two different `TITLE_SCALE`s (hud 4, death screen 2), both
right, which a name-keyed search reports as one value.

**1865 tests / 0 failures**; 5 mutations, all alive before the pins and all
killed after; demo PNG `2cc56b4acbfb92cb` byte-identical.

### M93q — the overlay colour quad, and two ways a pixel gate goes blind

The loom's preview is `fill` then `blit`, and the overlay path could not draw
the first half: `overlays` is `(sprite, PanelBlit)` and every sprite index
samples the atlas. M93q adds an untextured mode (a negative-`u` sentinel in the
fragment shader, a per-quad `tint`, a `FILL_SPRITE` index), the 43
banner-pattern textures, and the loom arm — closing the loom **end to end**.

**The milestone is the two blind spots, not the quad.**

**A gate that cannot reach a call site does not test it.** `o19`/`o20` grade the
fill primitive from a hand-made overlay list and pass whether or not any menu
emits one, because `container_panel_for_open_menu` hardcoded `loom: None` —
**delete the whole loom arm and both stay green**. This is M92's finding one
level over: M92's case was a gate *supplying* an input production derives; this
is a gate *unable to enter* the branch. The wrapper now carries the view (the
M93m precedent), and `o21` drives the real arm with a control frame and a
two-sided test that catches the fill/pattern order.

**And a witness can be sound on one property of the same draw and vacuous on
another.** `o21` reads `LOOM_PREVIEW_BACKING` to compute its expectation, so a
wrong constant moves render and expectation together — sound for the **order**,
self-calibrating for the **value**. The value was wrong.
`DyeColor.GRAY.getTextureDiffuseColor()` is the **third** constructor argument,
`4673362` = `0x474F52`, faintly blue; M93p shipped `0x3F3F3F`, which is none of
GRAY's three colours — and the trap is that both neighbouring arguments
(`fireworkColor` `0x434343`, `textColor` `0x808080`) are **more neutral than the
right answer**, so a plausible flat grey is exactly what a guess produces.
Mutating it back demonstrates the asymmetry: `containershot` — 52 witnesses,
validation on, real pixels — **survives**, and three lines of unit test stating
the decompile's literal **kill it**. **Pin a number against its source, not
against itself.** Worth a sweep: any `*shot` witness computing an expectation
from a `pub const` the renderer also reads covers everything about that draw
except the constant.

**Recorded, not fixed:** `--render-check` never opens a loom, so the fill's
windowed call site is unexercised. The blocker is not injection —
`loom_display_patterns` is false without a banner **and** a dye in the slots, so
an injected empty loom would witness nothing; staging it is a harness of its
own. The loom's **scrollbar drag** is also unwired, so only the first 16
patterns are reachable.

### M93l — the beacon's press state machine and `set_beacon`

M92d shipped the chrome, the geometry **and** `beacon_button_hovered`, so
unlike M93h the call site already existed and the model is not built against a
guessed one.

**The guard that reads backwards:** choosing a new primary **discards** the
secondary — *unless* the secondary is already the same effect. A secondary is
only meaningful alongside the primary it was chosen with, and the exception is
the "primary at level II" double. Inverting it keeps exactly what should be
discarded and discards what should be kept.

**The upgrade button is not a fourth kind of press** —
`BeaconUpgradePowerButton extends BeaconPowerButton` with `isPrimary = false`,
and `updateStatus` re-points its effect at the primary, so it presses as an
ordinary *secondary* holding the primary's effect.

**A press only happens on an active, visible button**, so the gate is
`beacon_button_state(..)` rather than a re-derivation — `updateStatus`'s rules
stay the single source for both what is drawn and what responds.

**`MobEffect.STREAM_CODEC` is `holderRegistry` — a RAW 0-based id**, not
`holder`'s `id + 1`. That has now bitten in M16, M21, M55 and M92d, and it is
quiet every time: an off-by-one names a **real** effect, so the beacon grants
the wrong one. The witness pins effect id **0**, which is where the two
conventions disagree most visibly.

**M93m wired the press — and the choice had to stop being derived.** M92's own
comment admitted it: *"Rewo has no click path here yet, so this reads the data
slots directly"*. Vanilla's `BeaconScreen` owns `primary`/`secondary`, seeded
from the menu and then **moved by clicks** before the server hears anything, so
a click-driven beacon cannot re-derive them each frame.

**The seeding rule is odder than it looks**: `dataChanged` re-reads *both*
effects on **any** slot id — including the pyramid levels — so a beacon growing
under you discards an unconfirmed pick. Hence the watermark is a per-menu
**data-write counter**, not the menu identity (misses it) and not the effect
slots (also misses it, because the clobbering write is to a different slot).
Only the two effects are screen-owned; levels and payment are re-read every
frame, so a payment arriving mid-selection lights Confirm without disturbing
the pick.

**A dark button does not consume the click** — `AbstractWidget.mouseClicked`
returns true only when it fires, so a disabled beacon button falls through to
the slot logic exactly as a disabled enchanting row does.

**One mutation survived, and it was the render**: every witness drove the
menu's data slots, so reverting the render to the derived choice changed
nothing they could see — a click would have moved a choice nothing drew, M93i's
"correct but invisible" one screen over.

**Recorded, not fixed:** the confirm closes the **client's** screen only. Rewo
resolves no *serverbound* `container_close` — `ids.rs` has the clientbound one
alone — so the server still believes the menu is open. That predates this and
affects **every** screen close.

### M93n — the anvil's rename

Listed as "needs a text field"; it needs *two* things and only one is the field.

**`validateName`'s `length() <= 50` counts UTF-16 code units, not characters.**
An emoji is 2 there and 1 to `chars().count()`, so a char-count check accepts
names the server rejects — silently, since `setItemName` just returns false
while the client has drawn the text. 25 emoji are legal, 26 are not.

**Typing an item's own name means *clear* the name.** No `CUSTOM_NAME` plus a
typed string equal to the hover name sends `""` — there is nothing to set.
Without the `!has(CUSTOM_NAME)` half, renaming a named item back to its
displayed name *clears* it; without the equality, every rename becomes a clear.

Also: `None` (too long) is **not** `Some("")` (a legal clear); a rejected name
does **not** advance the stored name; and the empty string is meaningful on the
wire, so a sender that suppressed it could never un-name anything. Sent on
every accepted keystroke, not on a confirm — the anvil has none.

**Recorded, not built:** `EditBox`. Rewo's key handler reads
`PhysicalKey`/`KeyCode` and never `KeyEvent.text`, so **nothing can type** — a
subsystem it has never had, shared with the class-C chat/command-input cluster.

`containershot` `d12` pins that the container arc now needs **four distinct
serverbound screen packets** — `container_button_click` 17,
`container_slot_state_changed` 20, `rename_item` 48, `set_beacon` 52. Four
screens, four packets, none a mode of another.

### M93o — the loom, and two of three recorded blockers that were wrong

M93h listed three. **One was real.** The occupied-pattern-slot case does *not*
need the `PROVIDES_BANNER_PATTERNS` HolderSet value off the wire — the
component is on the item's **prototype**, which never crosses the wire — and it
does *not* need the `banner_pattern` registry, because the value is a **named**
HolderSet, i.e. a tag id whose contents are jar data. The real blocker was that
`expand_tag` hardcoded `tags/item`.

**Same shape as M93e's correction**, and the lesson repeats: *a blocker
recorded from the wire's point of view can be wrong because the answer was
never on the wire.*

The item→tag mapping is **extracted from both sides** (`Items.java` names a
constant per item, `BannerPatternTags` maps it to a tag id) rather than
inferred — `flower_banner_pattern → pattern_item/flower` looks like a rule and
is a naming coincidence.

Four screen details that invert: an item with **no** patterns offers
`ImmutableList.of()`, **not** the default set (falling back would let junk
unlock everything); the grid needs a **dye**, not just a banner; `canScroll` is
strictly `> 16`; and the bounds test and the **range** test are separate, so a
cell past the end is *hit* then *rejected* and must not consume the click.

**One mutation shown equivalent — in Rust only.** Deleting `index >= 0` changes
nothing because `(-1i32) as usize` wraps past any representable bound; it is
load-bearing in Java, where `<` does not wrap. Rewritten as `try_from` so the
intent does not lean on the wrap.

**M93p landed the preview's geometry, not its render** — and says so, with a
surviving mutation as the evidence: reverting the pass to ignore the new source
size changes nothing observable, because no overlay uses it yet.

Transcribed: a **5x10** destination at `(cell + 4, cell + 2)` sampling the
**21x40** region of the 64x64 banner texture starting **one pixel down**. The
ratio is **not uniform** (21/5 vs 40/10), so it cannot be a scale factor —
hence `ProgressBlit.src` and `PanelBlit.{sw,sh}`, inert for every 1:1 blit
before it. And the pattern is drawn **untinted over flat grey**
(`DyeColor.GRAY.getTextureDiffuseColor()`) — not the banner's base colour, not
the dye's; tinting it with the dye would look plausible and be wrong for every
button.

**Left, precisely:** the 43 banner textures into the **overlay atlas**
(mechanical, but M48's lesson is that atlas growth is where addresses move),
and a way to draw the **solid grey backing** — `overlays` is
`(sprite, PanelBlit)` with no colour and no untextured mode, a third structural
change.

### M93h — the crafter's slot toggles, and a scoping claim that was wrong twice

The first bespoke-widget work, and it opens by **correcting the plan**: the
claim that "the loom and crafter need only their button lists" was wrong about
both.

**The crafter does not use `container_button_click`.** `CrafterMenu` has no
`clickMenuButton` override; `CrafterScreen` sends
`container_slot_state_changed` — **id 20** against the button click's 17. Only
the loom, the enchanting table and the two class-C screens are button-click
screens.

**The loom needs far more than a button list** (and is not shipped):
`getSelectablePatterns` is `BannerPatternTags.NO_ITEM_REQUIRED` when the
pattern slot is empty — a **`banner_pattern`** tag, where `expand_tag`
hardcodes `tags/item` — and otherwise the stack's `PROVIDES_BANNER_PATTERNS`
**HolderSet value**, which Rewo walks and discards, resolved through the
`minecraft:banner_pattern` registry that `parse_registry_data` does not
capture.

Four crafter facts that invert:

1. **`containerData[i] == 1` is DISABLED** — `setSlotState` takes an
   `isEnabled` and stores its inverse, so reading the value as "enabled"
   disables exactly the slots the player left on.
2. **`isSlotDisabled`'s `< 9` is load-bearing**: index 9 is the **power flag in
   the same array**, and 9 is a legal index, so a powered crafter would read as
   having a ninth disabled slot with nothing faulting.
3. **PICKUP is asymmetric** — re-enabling is unconditional, disabling needs an
   **empty cursor**, because clicking an empty enabled slot while holding
   something is a placement.
4. **The toggle is ADDITIVE** — `slotClicked` ends in an unconditional
   `super.slotClicked(...)`.

**And the packet body inverts against its sibling**: `container_button_click`
is `(containerId, button)`, this is `(slotId, containerId, newState)` — slot
first. The transposition yields a *well-formed* packet that toggles the wrong
slot of the wrong menu.

**M93i wired it into both click paths, and the wiring exposed a defect in
M93h.** `crafter_toggle` took an `is_swap: bool` and so treated **every**
non-swap input as PICKUP — but vanilla's `switch` has `case PICKUP` and
`case SWAP` and **no default**, so a shift-click would have silently re-enabled
a disabled slot. **No witness could see it because the function had no
caller.** That is the argument against leaving a model unwired: the shape of
the call site is an input to the design.

One funnel (`PlaySession::crafter_slot_click`) called from both paths —
including `finish_drag`'s one-slot-drag re-dispatch, which vanilla routes back
through PICKUP and which would otherwise have quietly not toggled. And because
**`PlaySession` has no test module anywhere in the repo** (M71's hazard — it
owns a socket), everything the adapter does except the send is extracted into a
tested function.

**M93j drew it, pixel-graded.** The cover is a **third slot geometry** — the
icon is 16x16 at the slot, M35's highlight 24x24 at `slot - 4` *bracketing* it,
and the cover 18x18 at `slot - 1`. And it **replaces** the slot's render rather
than layering over it (`extractSlot` never reaches `super`), the opposite
composition from the toggle's additive one. Vanilla writes the redstone arrow
in **screen coordinates**, alone in the class; the two forms agree only for the
standard 176x166 panel, so the witness re-derives it at three window sizes.

**Two render mutations survived first, and both taught something.** The arrow
*swap* survived a bbox witness — same box, different sprite — and the witness
that fixed it was itself wrong: it asserted the powered arrow is *brighter*,
where measured it is luma 68 against 124, because lit redstone is saturated
**red** (`0.299·255 ≈ 76`) against pale grey. **Luma is the wrong statistic for
"lit"**; redness separates them 0.0 vs 227.5. The witness derived its
expectation from the art, so the art corrected the premise rather than the
premise inverting the witness. The item-suppression mutation survived because
`containershot` never calls `init_gui_items`, so no pixel witness there reaches
the icon pass — graded instead by calling the production `screen_icons`.

**M93k added the `gui.togglable_slot` hint — and found a fourth hover that
was never made container-aware.** `screen_tooltip` was handed
`session.inventory` and the free `slot_at` (which *is* `PLAYER.slot_at`), so
with a chest open it named whatever the player had at the same index, at a
differently-centred origin. The highlight and the icons were both fixed; this
one was missed — **M89's "a per-call-site choice is how they come to disagree",
surviving in a fourth site.**

The hint is **derived, not transcribed**: vanilla's five conditions are exactly
the preconditions of a PICKUP that would *disable* the slot, so it asks
`crafter_toggle(PICKUP, ..) == Disable` and cannot promise an action the click
will not take. (It shows on an **enabled** slot — the constant is named
`DISABLED_SLOT_TOOLTIP` and reads "Click to disable slot".)

**Two mutations survived and each needed a different fixture.** Dropping the
grid gate survived because every witness hovered a crafter's grid — so every
empty slot in every menu would have offered to disable itself. And dropping the
panel-size half survived because **the crafter's panel IS 176x166**, making the
two forms identical for it; only a six-row chest (176x222, origin 28 px off
against an 18 px pitch) can see it.

**The crafter is complete end to end**: decode, model, packet, click routing,
render, hint. Only `requestCursor(POINTING_HAND)` remains, and Rewo has **no
cursor-shape concept at all** — winit plumbing, not a transcription.

 `live --render-check` not re-run —
M93 adds no render path.

*(An earlier draft of this entry said M87–M92 were all unmerged. They were not: `main` was already at M91, and only M92 was outstanding. The claim came from trusting REWO_PLAN §0.0's stale 2026-08-02 audit line instead of reading `git log` — the exact failure that section warns about. M92 is merged now.)*

### M91 — the furnace family (2026-08-03)

Five commits. A furnace takes a shift-clicked stack to the right slot and
shows its flame and progress arrow — `container_set_data`'s first consumers.
Detail in `REWO_PLAN.md` §15.

**The premise this was scoped on was wrong, and checking it before building
saved the work.** A fuel table alone unblocks nothing: vanilla checks
`canSmelt` **before** `isFuel`, and a log is **both** — fuel, and smeltable to
charcoal — so without `canSmelt` the *first* branch is unevaluable for every
item. **What unblocked it: the recipes are in the jar.** `canSmelt` reads a
`RecipePropertySet` the client normally gets from `update_recipes` (class C),
but for vanilla its contents are the ingredient sets of
`data/minecraft/recipe/*.json` — already the source for `ItemTags.SPEARS`
(M19) and the enchantment tags (M42). A class-C blocker that turned out not to
be one. **The caveat is the same one M19/M42 carry** and is stated in the
generated file: a datapack that adds or removes a smelting recipe makes the
table wrong with no error anywhere.

**Generators here, where M87a's layouts are a hand table**, and the difference
is measurable rather than stylistic: `FuelValues` is one regular builder idiom
whose only cross-file work is expanding tags (data), where the layouts were
four idioms plus cross-class builders that defeated extraction at 17 of 25.
280 fuels; FURNACE 156 / BLAST 62 / SMOKER 9 accepted inputs.

**The generator's arithmetic was wrong first, and the near-miss is the
lesson.** I wrote the evaluator left-to-right *and said so in a comment*; Java
respects precedence, so `1 + baseUnit * 20` is 4001, not 4020. The *other*
`1 + …` term gives **67 under either reading** — spot-checking that one (the
distinctive number, the natural choice) would have confirmed a broken
evaluator. Only `dried_kelp_block` separates them, out of 280. **Pin a set of
known-good values, not a representative one: the space is not uniform.**

**Three accepted-input sets, not one** — a smoker takes food and not ore, a
blast furnace the reverse, and a log is smeltable in a furnace *only*, so in a
smoker it is merely fuel.

**The flame grows upward**: its source and destination `y` move together, so
the bottom edge is fixed and the top rises. Anchoring at a fixed top makes it
shrink downward — an animation rather than an error.

**Two instrument failures, both found here:** M87f's screen survey did not
follow `extends`, so it recorded six centred titles where there are **nine**
(the furnaces inherit theirs); the checker now walks the chain. And **my own
test-totalling loop could not tell "0 tests passed" from "no tests ran"** —
`rewo-app`'s tests stopped compiling while its library built, every gate
passed, and the total silently fell 1712 → 1620. Ninety-two tests were not
running, and the only tell was a number moving the wrong way.

**Open on the container arc:** the ~11 bespoke-widget screens (anvil text
field, enchantment buttons, beacon, merchant trade list, loom/stonecutter
scroll grids, crafter toggles); `container_set_data` is consumed by the
furnace family and by **nothing else** (brewing bubbles, enchantment levels,
the beacon); and the crafting `quickMoveStack` shape, which declines rather
than guess.

- **Verification policy (user mandate): headless-first.** `rewo --headless N
  --chart-demo --out x.png` renders offscreen (no window) to a PNG;
  `rewo --run-seconds N` soaks windowed and prints percentile stats. Every
  Rewo milestone must ship a self-check path like these — the user does not
  manually test what a machine can check.
- Render disciplines already load-bearing: shader color constants are
  authored sRGB and **must convert to linear in-shader** (SRGB attachments
  encode on store); UI passes **mask alpha writes** so readback PNGs stay
  opaque. Shaders are GLSL compiled by glslc from the installed Vulkan SDK
  (`VULKAN_SDK` env; validation layers come with it).
- The user's network (Phase H's "chickenedin") is now named **Frogsy** and
  is Rewo's staging target (D1 in the plan); public servers are out of
  scope for Rewo until the user says otherwise (anti-cheat ban risk).

---

## Docs audit, 2026-08-07 (after M107)

A pass over **every** `.md` in the repo root, not just the ones the milestone
touched. Five documents were stale and two carried statements that were
actively false. What it found is more useful than the diffs:

- **This file has a GENERATED mirror** (the other of the two agent-instruction
  files in the repo root — its own header says which it is and gives the
  regeneration command), and the mirror had drifted **3,061 lines** (2,599
  against 5,660), about thirty-five milestones. It was still calling the Rewo
  work branch "72 commits ahead of `origin/main` and unmerged, the largest
  non-code risk in the project", which has been false since 2026-07-27. Its
  header already warns about exactly this, having caught a 634-line drift in
  July. **Regenerate it whenever you edit this file**; it is not a document to
  hand-edit. Note the generator is a blind whole-file rename, so a sentence
  naming *both* files reads as nonsense on one side — refer to "the mirror".
- **`AGENT_LOOP_BRIEF.md` duplicated §0.0's gate list and status**, and both
  had rotted — 435 tests against a real 2161, `mobshot` 243/243 against
  246/246, and a "Current state" section still describing the M10–M18 arc as
  unpushed local work. Rather than reset the numbers, those two sections now
  **point at §0.0**, which is the only place they belong. Its test-server
  recipe was also wrong in a way that costs an hour: it said
  `nohup java … &`, and the server **stops on stdin EOF**, so a backgrounded
  shell kills it instantly.
- **`REWO_HEALTH_BAR_SPEC.md` said `crosshairPickEntity` "needs an entity
  raycast Rewo does not have"** and that the pick clause is fed a hard `false`.
  **M73 built that raycast** and the clause resolves from it. Corrected in
  place rather than rewritten, because the reasoning for suppressing was sound.
- **The mixed-CRLF file list had drifted in BOTH directions**, was measured to
  exactly four on 2026-08-07 — and **is now empty**. A second measurement the
  same day, after M113, found **zero mixed files** under `crates/rewo-*`: every
  `.rs` is all-CRLF, `core.autocrlf` is false and there is no `.gitattributes`,
  so the working tree is exactly what is stored. The hazard has inverted — the
  documented failure needed a *mixed* file to normalise, and the remaining risk
  is a tool that writes LF into a CRLF file. `REWO_PLAN.md` §0.0 gotcha 9
  carries the current form. **Re-measure rather than trust any of this.**
- `README.md`'s "What's next" offered two items that had both shipped (merging
  the Rewo branch; an inventory model), and its gate count said fourteen where
  there are **33**.

**The pattern, for the third documented time:** a number with a test behind it
stays true (`REWO_PACKET_COVERAGE.md`'s table is machine-checked by a unit test
in `ids.rs` and was exact again), and the sentence next to it does not. The
generalisable fix is not to re-check prose more often — it is to **stop keeping
the same number in two places**, which is what the `AGENT_LOOP_BRIEF` sections
now do by pointing rather than restating.

---

## The chat arc — M108–M113 (2026-08-07)

Six milestones in one session, each merged `--no-ff`. Chat went from
`chat_log: Vec<String>` and eight truncated lines to a complete subsystem, and
the packet coverage went **114 / 0 / 27 → 116 / 0 / 25** with class C at 14.
`REWO_PLAN.md` §15 has the per-milestone detail; this is what a future session
should carry.

**M108 — the chat HUD.** `ChatComponent`, the wrap under it, the signature
cache, `delete_chat`, and the text render. **`ComponentRenderUtils.wrapComponents`
calls a DIFFERENT `splitLines` overload** from the one M85 transcribed: same
breaks, but a per-line `isWrapped` flag that is `!isNewLine` (a width wrap
indents its continuation, an explicit `\n` does not), and `"a\n"` yields TWO
lines. `forEachLine` emits **top-row-first**, and that order is load-bearing for
the tag-icon accumulator. **`delete_chat` is unreadable without a
`MessageSignatureCache`** — `Packed.read` is `readVarInt() - 1`, so the
signature is usually a cache index, and the cache is a move-to-front LRU that
dedupes rather than a ring. `system_chat`'s `overlay` bool was being read and
discarded; it routes to the **action bar**.

**M109 — the backdrop.** A colour channel on the HUD vertex, which first
exposed the `v.len() * 16` hardcode beside `VERTEX_STRIDE` — M21's shape,
latent until the vertex grew. **A witness caught a wrong draw order I had
justified with invented reasoning**: chat is a later stratum than the hotbar and
draws over it.

**M110 — `ChatScreen`.** `normalizeChatMessage` collapses *internal* whitespace;
`historyPos` starts one past the list and that slot is a **buffer, not an
entry**; `isDraftRestorable` is asymmetric; `shouldDiscardDraft` **keeps** the
draft on Esc; the wheel is clamped **before** it is multiplied.

**M111 — the scrollbar.** 1 px of colour plus 1 px of light grey, because the
second fill's x arguments are backwards and `fill` normalises. `HudFill` grew an
`rgb`, and it must be handed over in **linear** space — black is 0 in both and
hid that for two milestones.

**M112 — `isHovering`, and the bug under it.** Three handoffs had named the
narrow-window override as "one predicate with five consumers". **Four of those
consumers were not using the predicate at all**: `ScreenState::hovered`
converted through `Placement::centred`, so the click, the double-click, the drag
and the item-hover highlight all ignored the recipe book's 77 px displacement.
Third occurrence of M89's finding, first to reach an input path. There is now
one conversion and one visibility predicate, and a consumer has to ask.

**M113 — the Brigadier tree.** 2,017 nodes off a real server, **consumed
exactly**. An argument node's properties have no length prefix and only its own
type knows their size; 44 of 57 types are singletons and the other 13 are
transcribed. `time` has **no flags byte**; the numeric ranges are fixed
big-endian; the suggestion id is read **after** the properties. The registry
names are **namespaced**, and matching the bare name compiles and reads zero
bytes.

### Process, which generalises past this arc

* **Read a gate's EXIT CODE, never a substring.** M109 grepped for witness names,
  saw `ok` on every line, and missed that the gate was red on a declared-count
  assert. Then the consequence: **a mutation battery run against an
  already-failing command reads KILLED for every entry** — eight mutations across
  two batteries were vacuous and looked like 8/8. **Every battery now carries a
  no-op control** that must SURVIVE.
* **`cargo build` passing says nothing about whether the tests compile.** M110's
  signature change broke `rewo-app`'s test module while the build stayed green;
  the totalling loop counts `test result` lines, so that crate contributed 0 and
  read as silence. Read each crate's exit code.
* **Probe the port you are about to use.** M111's first run reported 27/28
  against a server that had crashed on `FAILED TO BIND`. **Most witnesses passed
  because they are injected** — only r25, which needs a real server, could tell.
  A gate whose witnesses are mostly self-driven can look healthy against nothing.
* **A mutation must be run against the check that covers it.** M111's sRGB
  mutation survived the pixel gate (which builds its input by hand) and died
  against the unit tests. A survivor is a question about the instrument as much
  as about the code.
* **Witnesses were wrong more often than the code.** Across the arc: roughly a
  dozen witness errors against three code errors, and the recurring shapes were
  a fixture sitting exactly where two candidate readings agree (an empty cache,
  a truncated body, the default line spacing) and a control that changes with
  its subject.
* **M97's lesson applied twice more** (`apply_chat_events`, `book_visible_for`),
  both found by a mutation surviving because the rule lived somewhere no test
  could reach.

---

## M141 — the ten tickable ramps, their velocity, and every ordinary trigger (2026-08-11)

`SoundEngine.tickInGameSound` drove **one `tick()` body out of ten** since
M131, because `EntityBoundSoundInstance` was the only subclass Rewo modelled.
`crates/rewo-net/src/tickable.rs` is the other nine, and the engine now drives
all of them. Detail in `REWO_PLAN.md` §15; three things belong here.

**The headline is a vanilla bug that punishes a careful reader.**
`MinecartSoundInstance.java:16` declares `private float pitch = 0.0F;` over
`AbstractSoundInstance`'s `protected float pitch = 1.0F;`, and **Java field
access is statically bound** — so `getPitch()`, declared in the superclass, is
the only reader and never sees the subclass field. `PITCH_MIN`, `PITCH_MAX` and
`PITCH_DELTA = 0.0025F` name a ramp that reaches nothing. Transcribing the class
in isolation gives every minecart ride a twenty-second pitch glissando vanilla
does not have. It is the only field shadow in the whole
`client/resources/sounds` package.

**Two more "the named constant is not the ceiling" cases, both from `Mth.lerp`
taking its factor first.** The bee's volume is
`lerp(clamp(speed, 0, 0.5), 0, 1.2)`, so the factor saturates at 0.5 and the
ceiling is **0.6** against the declared 1.2; the minecart's is 0.35 against 0.7.
And the bee's *pitch* clamps the factor to the **pitch range**, pinning an adult
bee at a constant 0.98 and a baby at 1.54 — never the bands the getters
describe. Meanwhile `RidingEntitySoundInstance` uses `Mth.clampedLerp`, which
clamps the factor to `0..1` and so is a different function. Two adjacent classes
mapping speed to volume, incompatibly.

**Two live fixes rode along.** The per-tick entity position was not narrowed
through f32 (the constructor was, and had a test) — with a comment that did not
merely omit the cast but *justified* omitting it, which is worse, because a
reader checking that line comes away reassured. Its witness could never have
caught it: the fixture moved the entity to three coordinates exactly
representable in f32. And `SoundWorld::entity_position` is gone in favour of
`RampWorld::position` — one name for one query, M89's finding, which has now
recurred at M90, M106b and M112.

**The batteries found more about instruments than about code**, which is the
pattern worth carrying: reading a mutation battery's **exit code cannot
distinguish a failing test from a failing build**, and this one's no-op control
came back KILLED because the previous run's binary still held the link output
(M138d's linker-1104 hazard). `tools/m141_mutate.py` reads the `test result:`
line and retries once; every earlier battery in `tools/` still has the hazard.
Two of my own witnesses were also wrong before any code was — one measuring a
bee's switch against an instance that had simply not been reclaimed yet, and one
asserting a direction vector from a remembered note rather than from the
expression, which on being worked out revealed that
**`Vec3.directionFromRotation` IS `Entity.calculateViewVector`** by another
route.

**M141e built the first trigger** — the elytra, which is now the one tickable
sound this client constructs. Its input, the local player's `fall_flying`, gates
the ramp at *both* ends (the survival guard `time <= 20 || isFallFlying()` and
the `onSyncedDataUpdated` rising edge), so one decode closed both. The decode
itself is **M73's asymmetry for the third time**: vanilla's local player is in
the level and its metadata is processed like anyone else's, but `EntityTable`
has no row for you, so the router dropped it.

Its finding is the sort that only a mutation surfaces: **`canPlaySound()` is a
per-class override that six of the ten declare and four decline**, so the
elytra — which does not declare it — must *not* be silence-gated on its player,
and Rewo's `Binding::Entity` had been meaning "follow" and "gate" at once.
`Ramp::silence_gated_entity()` is deliberately not `Ramp::entity()`.

The rising edge is also not "the flag changed": `assignValues` fires
`onSyncedDataUpdated` once per *entry in the packet* with **no** change guard,
and what makes the edge terminate is `wasFallFlying` being sampled once per
tick — which means two flag-carrying packets inside one tick each start a
sound, and vanilla has no dedup.

**M141f then took the bee and the minecart**, which are one vanilla method
(`postAddEntitySoundInstance`) with two arms — so implementing half an
`if/else if` would have been half a transcription. **Three of the ten ramps are
constructed now.**

Its finding is an index that needed counting twice. `Bee.DATA_ANGER_END_TIME` is
**19**, and the count that gets it wrong is `AgeableMob`'s: it declares **two**
accessors (`DATA_BABY_ID` *and* an `AGE_LOCKED` no earlier milestone noted), so
reading M20's "index 16 BOOLEAN is baby" as the whole of it puts this on
`Bee.DATA_FLAGS_ID`. The serializer catches that only by luck — one slot is a
BYTE and the other a LONG — and would not on a neighbour of the same type.

And anger is a **deadline, not a flag**: `endTime > 0 && endTime - gameTime > 0`,
whose second half changes every tick with no packet arriving. Storing a boolean
would freeze a bee's anger at whatever it was when the last metadata came in,
which is why the sound world grew a clock rather than the table growing a flag.

**M141g then took the guardian and the sniffer** (`handleEntityEvent` 21 and
63), so **five of the ten ramps are constructed**. The guardian's input is the
one among the ten that is **not a decode at all** — `clientSideAttackTime` is a
counter vanilla runs in `aiStep`'s client branch, and its rules read backwards
twice: it increments only while there *is* a target and never counts down, and
what zeroes it is **the metadata arriving, not the target going away** (there is
no change guard in `assignValues` — M141e's finding again).

**And it found a live decode bug on the way.** `(17, 35..=37)` claimed the
sniffer's, armadillo's and copper golem's state enums share an index. They do
not: `AgeableMob` declares **two** accessors, so the sniffer's and armadillo's
are at **18** while the copper golem's really is at 17 — which is presumably how
"their shared index" got written. On a sniffer, 17 is `AgeableMob.AGE_LOCKED`, a
BOOLEAN, so the state silently never arrived from a real server. **No gate could
see it**: the gesture rigs are driven by `REWO_FORCE_GESTURE` and
`mobshot --gesture`, which inject the state rather than decode it — and the unit
test encoded the bug rather than catching it.

The transferable half is the method, not the fix. An hour earlier I had
"found" that Rewo's serializer ids were off by one and the fault was my
counting, so the `extends` walk was run mechanically over several classes and
checked against two known-good readings (`SpellcasterIllager -> 17`, which is
live-verified, and `Bee -> 18`, which M141f had just shipped) **before** the
sniffer's answer was believed. **A counting method is an instrument; calibrate
it against a known reading before reporting what it finds.**

**M141h then closed the ordinary triggers with the riding pair**, so **seven of
the ten ramps are constructed**. Its finding is that the trigger does *not*
choose a sound: `LocalPlayer.startRiding`'s minecart arm plays **both**
instances at once — dry and underwater — and each mutes itself from the same
submersion input, so the crossfade belongs to the ramp. Picking one at mount
time is the natural implementation and is silent for half of every ride,
because diving does not re-fire `startRiding`. Two more, both invertible: the
loop is `Attenuation.NONE` (you are sitting on the thing), and it is
silence-gated on the **vehicle**, so a silenced cart silences its rider — the
same `canPlaySound()` trap M141e found, in the one place where binding it to
the obvious entity reads perfectly correctly.

It also has the clearest example of a limit on mutation testing: the "there are
two minecart instances" claim is pinned by a **destructure**
(`let [wet, dry] = RIDING_MINECART`), so shortening the array is a compile error
and the mutant never runs. **A battery can only grade claims that survive
compilation** — anything the types make unrepresentable is pinned harder and
shows up as BUILD-FAIL noise, so mutate the runtime claim underneath it instead.

**M142 then took the ambient handlers**, the subsystem that was left — and its
first finding is that **the class name is not the feature**:
`UnderwaterAmbientSoundHandler` plays the three rare sub-sounds and *nothing
else*, while the loop is minted by `LocalPlayer.updateIsUnderwater()`'s rising
edge alongside two positioned one-shots it never sees. Nor does any of that
handler's state do anything — `tickDelay` starts at 0, is decremented
unconditionally, and is only ever assigned 0, so it never gates; its four named
chance constants are declared and never read. **The three chances partition one
draw**, so the real rates are 0.0001 / 0.0009 / 0.009, not what the constants
are called, and **a spectator hears the additions and nothing else**, because
the early return that suppresses the loop lives in `updateIsUnderwater` while
the handler has no spectator gate at all.

The load-bearing one for anyone adding another: **a tickable ambient instance
must be constructed at volume 1.0**, because `SoundEngine.play` returns
`NOT_STARTED` for a zero-volume instance unless `canStartSilent()` — which none
of these classes overrides. Building a fading-in loop at 0.0 *because it is
about to fade in* makes it never play, with a debug log as the only trace. And
`relative` does **not** imply `Attenuation.NONE`; these three classes are
exactly what falsifies that pairing.

**The Overworld's cave sounds come from its DIMENSION TYPE**, not from any
biome: `DimensionTypes.java:43` sets `LEGACY_CAVE_SETTINGS` there, and no
vanilla Overworld or End biome sets the attribute at all. Drop that layer and
those dimensions go silent; hard-code it as a universal default and
`ambient.cave` plays in the Nether, which declares nothing. A biome
**replaces** the record rather than merging it, samples at the **raw quart**
with no fiddle (unlike M14's colour path), and does not interpolate.

Its battery came back **23/32 first time, and all eight survivors were real
gaps in my own witnesses** — including a partition test that *re-implemented
its subject* and so could not see a widened band, and a placement test whose
threshold the wrong answer already satisfied. Strictness needed an exact tie:
`<` versus `<=` differs only when a draw equals the chance (2⁻⁵³, and 4M seeds
produced none), so the witness reads the draw off a cloned RNG and uses it *as*
the chance.

**M142c then wired the bubble column**, whose scan has its own inversion:
"X varies fastest" means the **priority is the reverse** — `betweenClosed`
visits a whole Z slice before the next, so with `findFirst()` the winner is the
lowest Z, then Y, then X. My witness asserted it backwards, and **its Y case
passed for the wrong reason** because the block it expected to win on X was
also the lower one in Y. Two more: the property is serialised `drag` (not
`drag_down`) and the block's **default state is `drag=true`**, so a missed
lookup makes every column a whirlpool rather than silence; and a missing chunk
**empties the whole scan**, which the handler reads as "no column" and so
*re-arms* on, firing when the chunk arrives. Its table is graded from the real
bake by four `blockentityshot` witnesses, because every unit test supplies its
own — and the battery needed a **per-file runner**, since `assets::bake` is
unreachable from `cargo test` and a test-only harness would call both its
mutations SURVIVED. Its one survivor (39/40) is the milestone's only genuinely
**equivalent** mutant rather than a hole in a witness: every `bubble_column`
state declares `drag`, so the branch it defaults cannot be reached — pinned as
a coincidence, so a version that breaks it makes the branch live again.

**M142d then wired the biome loop, and all three handlers now reach a running
client.** It came last because it is the one that could not be expressed as
"play a sound": vanilla's handler **holds** its `LoopSoundInstance`s and calls
`fadeOut()`/`fadeIn()` on them, so Rewo's handler names the outcome and the
engine applies it to the live set — which *is* vanilla's map, filtered to
biome-loop ramps, so the reuse falls out rather than being arranged. Two rules
there read like bugs and are not: **every** loop fades out on a transition
including the incoming one (that `min(fade, 40)` is the only place a runaway
fade is capped, and the tick never bounds it upward), and **a live instance is
reused** rather than replaced, so crossing back inside ~41 ticks resumes the
same voice instead of restarting the sample. The transition keys on the
**sound**, not the biome, so two biomes sharing a loop cross silently. And one
snapshot feeds all three features while only the loop is change-gated —
otherwise standing still would stop every addition.

Its battery's two misses were both about the harness rather than the
transcription: an anchor that matched **twice** (so the mutation was skipped,
which is not the same as surviving — the count is reported for exactly this
reason), and a genuine gap whose consequence is worse than the mutation looks.
Dropping the **ramp-kind** guard from the reuse lookup lets an ordinary sound
that happens to share the bed's identifier stand in for it, at which point the
fade fails silently and **the bed never starts at all**. The **directional sound is a different feature** — the End flash from
`ClientLevel.tick`, needing `EndFlashState` — and one reader in M142's survey
claimed that class is dead in vanilla and advised deleting Rewo's ramp. It is
not.

**M141d fed them the velocity**, which was the input gating four of the ten,
and its finding is the sort that punishes a sensible implementation: a remote
entity's `getDeltaMovement()` is **a decaying echo of the last
`set_entity_motion` packet**, not a velocity. A client never integrates it into
a position, so a bee gliding steadily past with no motion packets has it falling
to zero while it is visibly moving — and its buzz fades with it. A finite
difference over the interpolated positions is more truthful about the bee and is
not what vanilla sounds like.

Nor is it one rule. The 0.98 decay is the **`else` of the interpolation
branch**, so an entity still catching up does not decay at all; it is skipped
for a vehicle the local player rides (authority is inherited from the
controlling passenger); the deadband that follows has **two forms** (a player's
joint `< 9.0E-6`, everything else's per-axis `< 0.003`, disagreeing at
`(0.0025, 0.0025)`); and **none of it runs for a minecart**, because `aiStep` is
`LivingEntity`'s and both minecart behaviours' client branches touch position
only. `EntityTableWorld`'s doc carries the full table of what it can and cannot
answer — and now says how to re-derive that count, having been wrong in both
directions.
