---
paths:
  - "crates/ewo-render/**"
  - "crates/ewo-core/src/theme.rs"
  - "crates/ewo-core/src/color.rs"
  - "crates/ewo-core/src/easing.rs"
  - "assets/**"
  - "style/**"
  - "StyleSheet1"
  - "StyleSheet2"
---

# EwoClient visuals — Velvet & Pearl (launcher + HUD rendering)

Applies to the Skia launcher renderer and the widget/screen code. The 7 design
non-negotiables are in `CLAUDE.md`; this file carries what you need to apply
them. Build history and the reasoning behind every value: `docs/history/ewoclient-v1.md`.

## Reference materials (read these when designing anything visual)

All in the repo root. **Don't move them without asking the user.**

- **`StyleSheet1`**: fonts, `:root` tokens, app-window shell, backdrop layers
  (velvet folds, caustics, bokeh, pearl dust, petals), glass panel, breathing
  text, button, pbar, slider, dropdown, status line, launcher layouts, reduced motion.
- **`StyleSheet2`**: main menu, screen head, instances, launching + log,
  settings tabs, rename, new-instance modal, toggles/path-fields/danger buttons.
  Both carry the author's perf-decision comments inline (blur 40→32, modal
  entrance blur removed, …). They explain *why* a value was chosen.
- **`EwoClient · Velvet & Pearl prototype.htm`**: JSX components. `VelvetFolds`
  (~6438: `feTurbulence` fractalNoise, baseFrequency `0.007 0.011`, octaves 2,
  seed 4, `feDisplacementMap scale=28`), `PearlDust` (~6482), `BokehOrb` (~6604),
  `Petals` (~6628).
- **`style/*.png`**: 10 reference screenshots, the visual ground truth for
  colours and proportions where the CSS leaves room.

## Velvet theme tokens (canonical — keep in sync with `ewo-core::theme::Theme::VELVET`)

```
--bg-core #000000   --bg-wine-a #0A0006   --bg-wine-b #120010
--panel-glass rgba(180,130,160,0.06)
--text-pearl #F4E8EA   --text-mauve #9A8087
--accent-berry #B47491 --accent-rose #E5B8C5 --accent-lav #C9A5D4
--accent-champ #E8D4A8 --accent-ember #C96A7A (error)
--silk cubic-bezier(0.22, 1, 0.36, 1)
--motion-speed 1.0  --breath-amp 1.0  --density 1.0  --warmth 0.6  --accent-hue-shift 0deg  (user-tunable)
```

Inline hues: `#FFF6F0` warm-white, `#FFF0F4` pearl highlight, `#C4AFB5`
mid-pearl, `#6B555C` deep mauve, `#D4889A` error text, `#A35A6C` / `#8A6E7E`
error gradient stops. The Rust palette must match the CSS `:root` byte for byte.

**`Theme` is the single colour source; new colours must be tokens.** The
prototype's inline hues are named `Theme` fields (`text_pearl_hot`,
`accent_rose_dusk`, `bg_card_top`, …). `ewo-render` reads them only through
`crates/ewo-render/src/tokens.rs` (`tokens::accent_rose()`, `_a(alpha)`), never
a colour literal or per-file const in a draw function. `draw_frame` /
`Backdrop::draw` publish the frame's theme per thread with
`tokens::set_active(theme, settings)`; code that never calls it (examples, the
HUD's reuse of widgets, tests) paints stock Velvet. **`accent_hue_shift`**
rotates the berry/rose/lavender/champagne accents (OkLCH, chroma-reducing gamut
map, exact identity at 0°) and nothing else: neutrals, backgrounds and the
ember error family keep their hue; the slow-layer and glow caches key on the
shift. **`motion_speed`** reaches every ambient loop through
`ewo_core::motion::scaled_time` / `scaled_dt` (frozen under reduced motion);
don't divide a period by the speed by hand. `tests/default_theme_golden.rs`
pins full `draw_frame` renders of every screen, modal, the dev overlay and the
hover/error states against `tests/golden/default_theme.txt` (regenerate with
`EWO_UPDATE_GOLDEN=1`, only for an intentional rendering change, and say why).

Fonts are bundled variable TTFs in `assets/fonts/`, resolved next to the exe
first (portable `dist/`), then the workspace path. No system fonts.
Fraunces (display; `SOFT` 0–100, `WONK` 0/1, `opsz`, `wght`), Newsreader (body,
italic taglines, labels), JetBrains Mono (eyebrows, logs). **The launcher title
uses `SOFT 50, WONK 1`**; that's identity, not a default.

## Render graph (one frame, top to bottom)

```
1 Clear
2 Backdrop: wine radial → velvet folds (3 oklch radials, turbulence+displacement,
  blur 40px, screen blend) → caustics (2 layers, blur 30px, screen, 38s + 52s
  reversed) → bokeh (50vmin, blur 40px, screen, 60s cross) → pearl dust (screen)
  → petals (alpha) → vignette
3 Screen stage: glass panels (backdrop blur → tint → 4 rims → content), widgets,
  text last and never inside a scaled container
4 Dev overlay (--dev)   5 Window chrome (inset hairline rim)   6 Present
```

Backdrop layer order in code is wine → folds → caustics → bokeh → dust → petals
→ vignette (the vignette is applied globally, a deliberate simplification).

## Load-bearing implementation rules

- **Never construct a Skia shader, image filter or variable-font `Typeface`
  inside a per-frame draw.** Each allocates C++/FreeType state that Skia's
  tracked caches don't bound. The per-frame `fractal_noise` chain in
  `velvet_folds` and an uncached `newsreader_italic` clone leaked ~2.6 GB/hour
  while focused. Build once (a `thread_local` or a quantized typeface cache like
  `fraunces_cache`), then clone the refcounted handle.
- **The backdrop has a slow clock.** Wine + folds + caustics + bokeh (the three
  full-screen blurs) render to an offscreen cache at `CACHE_REFRESH_HZ` (20 Hz)
  and are blitted each frame. Pearl dust, petals and the vignette draw live.
  Invalidate the cache on resize.
- **Offscreen caches are physical-size.** The launcher paints in logical
  pixels under a `scale_factor` CTM; the backdrop slow-layer cache and the
  inner-glow cache come from `offscreen.rs`: a surface at the device-pixel
  size, drawn into with the same scale, blitted with the transform reset so one
  cache pixel is one screen pixel. Cache keys include the physical size (and
  the glow's the hue shift), so changing monitor scale rebuilds them. A new
  cache must do the same; `tests/hidpi_cache.rs` renders at several scale
  factors and fails a logical-size cache.
- Pearl-dust halos are a baked 64 px sprite stamped with per-mote alpha. The
  inner berry glow is baked once per window size (`GLOW_CACHE`).
- Gradient shaders go through `ewo_render::gradients::{linear, radial, sweep}`
  (skia-safe deprecated the `gradient_shader` free functions in 0.93).
- CSS blur radius → Skia sigma: `σ = radius / 2`. Spread = inflate/deflate the
  source rrect (Skia's mask blur has no spread).
- Inner velvet-fold layers screen-blend **each** layer (`mix-blend-mode: screen`
  per `.vf-layer`); alpha-blending them first reads visibly dimmer.
- Glass panels (`widgets/glass_panel.rs`), in draw order: refract
  (`SaveLayerRec::backdrop` blur, CSS 32 px → sigma 16, + dark-mauve fill),
  tint (135° gradient + top radial, optional 8 s breath on the **tint only**),
  the four rims (12 s linear slide, delays 0/−3/−6/−9 s; drawn **under** the
  content, CSS z=3 vs z=4), the caller's content, then the 1 px hairline. The
  panel itself never scales or blurs; the prototype's `panel-mount` keyframe
  (scale 0.98 + blur 8 px) is deliberately not implemented.
- **No scaled text (non-negotiable #2).** The modal cards (about, new
  instance, launcher link), the dropdown and the pressed button are each two
  layers: the chrome (shadows, refract, tint, rims, menu body) keeps the
  prototype's scale-in so the silhouette still settles, and the text layer only
  fades and translates by **whole pixels**. At rest the layers coincide.
  `crates/ewo-render/tests/text_not_scaled.rs` measures the text bounding box
  of entrance and press frames against the settled frame.
- **Reduced motion (non-negotiable #6)** lives in `ewo-core/src/motion.rs`.
  The launcher reads `SPI_GETCLIENTAREAANIMATION` at startup, re-reads it at
  most once a second and on every focus gain (Windows only; Linux keeps
  animations on), and drops the backdrop cache when it flips. When reduced,
  every transition divides by `motion::dur(..)` and lands in 0.01 ms (stagger
  delays collapse too), and the ambient loops freeze on their CSS **base
  style**, as `StyleSheet1`'s `prefers-reduced-motion` block (~L1563) implies:
  `motion::ambient_time` returns phase 0, and the layers whose base style
  differs from the 0 % keyframe (bokeh, folds, button rim, firefly, meta dot)
  branch on `motion::is_reduced`. Pearl dust and petals freeze; the celebrate
  burst is skipped. New animation code must go through `motion::dur` /
  `ambient_time`; `tests/reduced_motion.rs` checks every tick completes in one
  frame and that frames are byte-identical across clock values.
- Blurring text during a *transient* crossfade (vstatus) is allowed.
  Non-negotiable #3 forbids blur on entrance of text-bearing surfaces.
- Tofu: the serif fonts lack arrow glyphs. Use vector chevrons
  (`draw_chevron_right`), not `›`/`→`.
- Intentional OLED divergences from the CSS numbers (keep them): bokeh peak
  opacity raised to 0.45/0.55/0.40, vignette outer darkening 0.65→0.50, inner
  berry glow 0.15→0.22. Resting-state text blooms were tried and rejected (soft
  type).

## Verifying visual changes

Render to a PNG instead of launching the app:
`crates/ewo-render/examples/dropdown_shot.rs` is the harness pattern (it
diagnosed the dropdown "square corner" as shadow pooling). Pixel parity against
`style/*.png` is checked manually by the user; don't add headless-browser tests
of the prototype.

## Glossary

Velvet (the theme), Pearl (unimplemented light theme), boudoir aesthetic, glass
panel, pearl dust (screen-blended motes), petals (4 baseline → 140 on
celebrate), velvet folds (turbulence + displacement), caustics, bokeh orb,
breathing text (8 s letter-spacing 0 ↔ 0.02 em, per glyph), silk easing,
celebrate state, `bt-g` (per-glyph span class), disturb (click the backdrop;
settled dust decays at `*= 0.93/frame`).
