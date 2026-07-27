<!-- GENERATED FILE — do not edit directly.

     AGENTS.md is a verbatim copy of CLAUDE.md with the self-references
     renamed. The two had drifted 634 lines apart by 2026-07-27 (AGENTS.md was
     four days and roughly twenty milestones stale), which is exactly the
     failure mode a duplicated source of truth invites.

     Edit CLAUDE.md, then regenerate:
         python -c "import io; c=io.open('CLAUDE.md',encoding='utf-8',newline='').read();                     io.open('AGENTS.md','w',encoding='utf-8',newline='').write(c.replace('CLAUDE.md','AGENTS.md'))"
     and re-add this header. -->

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

The two dead directories have tombstone `AGENTS.md` files redirecting here. If you're a fresh session and you opened the wrong one by accident, this file is the one to read. The live repo's `git remote -v` shows `lewlone/ewoclient`.

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
├── AGENTS.md                # this file
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

1. **This file (AGENTS.md)** — locked decisions, scope, non-negotiables, build sequence.
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
- Rendering wraps each half in `Canvas::save_layer` with a layer paint that combines `set_alpha_f` (opacity) and `set_image_filter(image_filters::blur(...))` (blur sigma 3 → 0). The layer is then translated for the slide-in/out: incoming `translateY(8 → 0)`, outgoing `translateY(0 → -10)` per CSS `@keyframes status-in` / `status-out`. Note: blurring text *during a transient transition* is allowed; AGENTS.md non-negotiable #3 forbids blur on entrance to text-bearing surfaces (modals/panels), not on transient text crossfades.
- Wired into `screens::launching::LaunchingState::stage`. The launching tick calls `stage.set(progress_at(scaled).1)` each frame; identical strings no-op.

### Mods list (`screens::instances::draw_mod_section`)

- Renders below the inst-config bottom hairline (with the CSS 28px margin-bottom) when the selected instance has any mods. Active instance ships with 7 mods (Sodium / Iris / Distant Horizons / Continuity / Lithium / Mod Menu / Carpet — last is off by default).
- Layout matches the CSS 4-column grid (28px / 1fr / auto / auto, 14px gap): toggle circle / mod name / category eyebrow / version. Categories are uppercase Mono 9 with 0.18em tracking; off rows render at 0.4 alpha so they read as inactive without disappearing.
- Toggle circle is a 22px hairline-rose ring; when on, an 8×8 pearl-gradient (white→rose→lavender) sits in the center with a 4-sigma rose halo blurred underneath.
- New `Slot::ModToggle(usize)` carries the row index. Hit-testing returns one entry per mod; clicking flips `InstancePrefs::mods_on[i]` and the head's "X of Y enabled" count updates immediately.

---

### New-instance modal (`screens::new_instance_modal`)

- **Two-layer composition.** Layer 1 = `.modal-shroud` — full card-content rect with a radial dim gradient (rose-tinted black 0.55 → 0.85) + 4px backdrop blur. Layer 2 = `.modal-card` — 560px-wide rounded card centered, with three sub-passes: drop-shadow stack (40-sigma + 14-sigma + rose-bloom), backdrop blur 40 + dark-wine fill, 135° tint + warm-white top radial fade, hairline rim, then inner content clipped to the rrect.
- **Entrance animation.** 240ms silk crossfade — opacity 0→1, translateY(10→0), scale(0.97→1). The shroud fades in slightly faster (linearly ramped to peak by `anim ≥ 0.67`). No blur during entrance per AGENTS.md non-negotiable #3 — text-bearing surfaces never re-rasterize.
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
2. `EwoLoaderV1/manifest/0.1.0/26.1.json::libraries[]` — adds the download artifact entry.
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
- Threading: every long-running task spawns a `std::thread` and reports back via `mpsc`. Polled by `App` once per frame in `RedrawRequested`. No tokio/smol — see AGENTS.md non-negotiables.
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
post-G iteration sprints — what was originally AGENTS.md non-negotiable
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
`codex/rewo-m19-combat-swings` — 72 commits ahead of `origin/main`, unmerged.*
---

## Rewo — from-scratch native Minecraft client (M0–M33b shipped: online play, native CEM, exact light/colour, dimensions, the combat + block-entity arcs, weather and clouds)

**[REWO_PLAN.md](REWO_PLAN.md) is the plan of record — a fresh session must
read its §0.0 HANDOFF first** (it consolidates current state, what to do next,
the headless verification toolkit, the load-bearing gotchas, and a categorized
list of every known issue/gap/deviation, explicitly framed for critique).
**Everything through M33b is shipped, gated and pushed** to
`codex/rewo-m19-combat-swings` — which is **72 commits ahead of `origin/main`
and unmerged**, the largest non-code risk in the project. Rewo (from
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
next feature rather than the next milestone. Two things from it are
load-bearing anywhere in the repo: (1) **Sodium, EntityCulling and Xaero's
Minimap are source-available but NOT open source** (Polyform Shield / bespoke
protective / All-Rights-Reserved) — their source must not be read as a
reference for Rewo, though bundling the jars in EwoLoader is a separate
question; (2) 33.7% of all client-mod download mass exists only because the
game is a JVM client with a mod loader, which is the strongest external
validation Rewo has.

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
