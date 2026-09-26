---
paths:
  - "crates/ewo-render/**"
  - "crates/ewo-ui/**"
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
- Pearl-dust halos are a baked 64 px sprite stamped with per-mote alpha. The
  inner berry glow is baked once per window size (`GLOW_CACHE`).
- CSS blur radius → Skia sigma: `σ = radius / 2`. Spread = inflate/deflate the
  source rrect (Skia's mask blur has no spread).
- Inner velvet-fold layers screen-blend **each** layer (`mix-blend-mode: screen`
  per `.vf-layer`); alpha-blending them first reads visibly dimmer.
- Glass panels: `SaveLayerRec::backdrop` blur (sigma 16) + dark fill, 135° tint +
  top radial with an 8 s breath on the **tint only**, 4 rims on a 12 s cycle
  (+0/+3/+6/+9 s offsets), 1 px hairline last. Entrance animations target chrome
  layers only (non-negotiables #2/#3).
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
