# Rewo Velvet UI — the spec, written before it is built

**Status: spec, nothing implemented.** Written in the pattern `M53 spec` set —
the appearance is settled on paper first. M52a (the module port) shipped; this
is the machinery M52b needs before a single HUD widget can be ported.

**Scope (resolved §8):** all 17 widgets, the in-game editor, and Rewo
measuring its own ping. **Mandate (user, 2026-07-28):** *"this is something that must not be rushed and
is definitely fine to build machinery for. We want it to be performant and run
at high fps like 120 which is why we don't use something like web overlays. And
also since we want it to look beautiful and have real effects it's bound to be
more complicated."* Fidelity target: **pixel-faithful, gated.**

---

## §0 Why this exists

`REWO_FEATURE_SURVEY.md` ranks porting EwoClient's module + HUD set as one
milestone. M52a shipped the modules whose hooks already existed. The HUD
widgets stopped dead against a wall:

> `draw_coords` needs `fonts.fraunces_axes(18.0, 30.0, 0.0, 500.0, Some(36.0))`,
> `measure_tracked_em` for letter-spacing, `draw_iw_shell` for the glass chip,
> and a shadow stack. **Rewo's `TextPass` is the vanilla 8px bitmap font and
> nothing else.**

The widgets are **not** ported from mods. They exist, in Velvet, in
`crates/ewo-jni/src/hud.rs` — 17 widget kinds, 9 anchors, a per-profile
`hud.toml`, a drag editor. What is missing is a renderer that can draw them.
EwoClient draws them with Skia. Rewo has raw Vulkan and no 2D vector stack.

**The scope is the renderer, not the widgets.** Once this exists, each widget
is a transcription of layout constants.

---

## §1 Three subsystems, very different costs

Reading the Skia originals, the work splits three ways — and two of the three
are far cheaper than they look, because they are already SDF shader maths that
merely happens to be hosted in SkSL.

| # | Subsystem | Nature | Cost |
|---|---|---|---|
| 1 | **Glyph** — variable-font text, tracking, shadow stacks | Genuinely new machinery | **large** |
| 2 | **Chrome** — rounded rects, rings, gradients, blurred shadows | Analytic SDF, one pipeline | small |
| 3 | **Liquid glass** — refracting plate | SkSL → GLSL, near-verbatim | small |

### The load-bearing discovery

`crates/ewo-render/src/widgets/liquid_glass.rs` is **not Skia logic**. It is a
fragment shader over a signed-distance field:

```
float sdRoundBox(float2 p, float2 b, float r)   // distance to the rounded rect
float2 n = central differences on sdRoundBox    // outward normal, no fwidth()
float bend = t*t*t                              // cubic bevel: flat centre, hard rim
bend *= 1 + uRipple * sin(ang*3 + t*1.10) * (...)   // the liquid standing wave
float2 off = n * (uStrength * bend)             // refraction, pushed OUTWARD
```

Every line of that is portable GLSL. It already avoids `fwidth`/screen-space
derivatives (SkSL runtime effects cannot rely on them), which is exactly the
constraint that makes it drop into a Vulkan fragment shader unchanged. It takes
**two** backdrop samplers — `rim` (lightly blurred, so the bevel has structure
to bend) and `frost` (heavily blurred, so text sits on mush) — which Rewo can
supply from its own offscreen far more cheaply than Skia can.

So the honest estimate is: **the text stack is the milestone.** The glass, the
thing that looks hardest, is a port.

---

## §2 The glyph subsystem (the real work)

### What it must do

From the widget renderers, the requirements are exactly:

- **Three families, six files**, already in `assets/fonts/`: `Fraunces.ttf`,
  `Fraunces-Italic.ttf`, `Newsreader.ttf`, `Newsreader-Italic.ttf`,
  `JetBrainsMono.ttf`, `JetBrainsMono-Italic.ttf`.
- **Variable axes** — Fraunces `SOFT`/`WONK`/`opsz`/`wght`, Newsreader
  `opsz`/`wght`. Non-default axes are load-bearing identity, not decoration:
  CLAUDE.md pins the launcher title at `SOFT 50, WONK 1`, and `draw_coords`
  asks for `SOFT 30, wght 500, opsz 36`.
- **Tracked text** — CSS `letter-spacing` in em, per-glyph advance offsets
  (`measure_tracked_em` / `draw_tracked_em`).
- **Cap-height baselines** — layout uses `font.metrics().cap_height`, not the
  em box. Getting this wrong shifts every widget vertically.
- **Shadow stacks** — the in-world text draws several offset copies.
- **A user scale multiplier 0.5..3.0** per widget (`SCALE_MIN`/`SCALE_MAX`).

### Decision: runtime rasterization into a dynamic atlas — not SDF

An MSDF atlas is the reflexive answer for scalable GPU text and it is **wrong
here**, for a specific reason: the fidelity target is *pixel-faithful against
the Skia originals*, and SDF reconstruction is an approximation of the outline.
It would differ from Skia at every antialiased edge, and the gate in §6 could
never be tight. It is also weakest at small sizes, and this HUD is full of
9–12px mono labels.

Rasterize instead, and cache. That is what Skia itself does — glyph raster is a
*cache* problem, not a per-frame problem.

```
key   = (family, italic, size_px_quantized, axes_quantized, glyph_id)
value = rect in a shared R8 atlas + bearings + advance
```

Cache hit → the frame is quads. Only a novel (size, axes) pair rasterizes, and
the HUD uses perhaps a dozen combinations. **Quantize the key** exactly the way
`ewo-render`'s `fraunces_cache` already does — the launcher learned this the
hard way, and CLAUDE.md records it:

> *never construct a Skia shader / image-filter / variable-font `Typeface`
> inside a per-frame draw — they allocate foreign C++/FreeType state that
> Skia's tracked caches don't bound.*

That leak cost the launcher ~4 MB/s. The same discipline applies to whatever
rasterizer we pick: **build once, cache by a quantized key, clone the handle.**

### Rasterizer choice

`swash` — pure Rust, real variable-axis support (`Setting<f32>` coordinates),
high-quality hinted rasterization, and a scaler API built around exactly the
cache-by-key model above. `fontdue` is faster but its variable support is thin;
`ab_glyph` does not do axes properly; `cosmic-text` drags in a full shaping and
layout stack we do not need — the HUD is Latin, and advances plus manual
tracking are the entire layout model.

**Not** doing full HarfBuzz shaping is a deliberate repeat of a decision the
launcher already made and documented (build step 8: *"Full Skia `Shaper`
(proper kerning + ligatures) intentionally skipped — visible benefit is
negligible at the sizes this app uses"*). Match that, so the two renderers
agree.

### Atlas

One `R8_UNORM` coverage atlas, shelf-packed like the entity atlas already is
(`mobs.rs` uses a 16²..256² shelf packer). Coverage only — colour is a vertex
attribute, so one atlas serves every tint, and the shadow copies cost nothing
extra. Start 1024², grow by re-pack on exhaustion. Evict nothing: the working
set is bounded by the widget set, and a HUD that stutters when it evicts a
glyph would be worse than one that uses 4 MB.

---

## §3 The chrome subsystem

Everything `draw_iw_shell` does in its non-glass path is analytic from the same
`sdRoundBox` the glass shader already needs:

| Skia call | SDF equivalent |
|---|---|
| drop shadow, `MaskFilter::blur(8)` | offset SDF, smoothstep over the blur radius |
| outer wine ring, 1px stroke, inset −1 | `abs(d + 1) - 0.5` band |
| fill | `cov = clamp(0.5 - d, 0, 1)` |
| inset wine ring, 1px, inset +1 | same band, other sign |
| top pearl highlight, clipped to 2px | band ∩ `p.y < top + 2` |
| music bloom, blur 10–26 | large-radius smoothstep, additive |

### The colour-space trap — M50, repeating

**The Velvet UI must render through a gamma-space (UNORM) view, not the SRGB
attachment.** EwoClient's `rgba()` is a plain `/255` with no transfer
function, so Skia composites `WINE 0.50` over the backdrop in *gamma* space.
Rewo's swapchain is `B8G8R8A8_SRGB`, where fixed-function blending happens in
*linear* — and `dst*(1-a) + src*a` is not invariant under the sRGB transfer, so
the same constants produce a visibly different plate.

M50 hit this exactly: the enchantment glint went in structurally correct and
rendered a byte-delta of **zero**, because the blend space was wrong. The fix
already exists — `SwapchainTargets` carries a UNORM twin view of every image
for precisely this reason.

**Both Velvet passes need it**, not just chrome. If the plate blends in gamma
and the type on top of it blends in linear, they disagree with each other and
the disagreement is worst exactly where they overlap.

**A Gaussian mask blur is a smoothstep over the SDF, not a blur pass.** For a
rounded rect the exact analytic result is available, so the shadow costs one
extra fragment evaluation rather than a ping-pong blur — which is most of why
this can hold 120 fps with many widgets on screen.

One pipeline, one instanced quad per shell, parameters in a vertex-attribute
struct. No CPU-side tessellation.

---

## §4 Liquid glass

Port `liquid_glass.rs`'s SkSL to GLSL. The uniforms are already declared in a
std140-friendly order (the file says so, and lists byte offsets), so the block
maps to a Vulkan push-constant / UBO with no re-layout.

**The two backdrop inputs are the only real integration work.** EwoClient feeds
them from its cached frost surface. Rewo should do the same and can do it
better: it already owns the offscreen (`offscreen.rs`, and M51's capture path
proved a full-scene readback works), and the launcher + `ewo-jni` both
established the pattern that the frost is **cached on a slow clock** —
`refresh_frost` recomputes at ~10 Hz into a quarter-resolution surface and the
per-frame cost is a cubic upscale. Reuse that: `rim` at half res, light blur;
`frost` at quarter res, heavy blur; both refreshed on a slow clock, sampled
every frame.

Deliberately **not** blurring per widget — the shader's own docs say so, and
re-blurring per plate is the obvious way to lose the frame budget.

---

## §5 Performance

Target 120 fps *with the HUD up*, on top of a world render that already hits
~0.23 ms GPU (M6 bench). Budget for the whole UI layer: **≤1 ms GPU, ≤0.2 ms
CPU.**

What that buys, and why it is achievable:

- **Text**: cache hit → zero rasterization. Per frame it is one vertex buffer
  write and one draw. The shadow copies are extra quads, not extra passes.
- **Chrome**: analytic, one instanced draw for every shell on screen.
- **Glass**: one fragment shader over the plate's own area, sampling two small
  cached textures. Bounded by plate pixels, not screen pixels.
- **Backdrop**: on a slow clock, amortized to ~1/12 of frames.

The thing to watch, and the reason for the discipline in §2: **per-frame
allocation of foreign objects**, which is what actually killed the launcher's
frame time and memory before the 2026-05-31 pass. Rasterizer handles, atlas
pages and pipelines are built once.

---

## §6 The gate

`rewo hudshot --check` — serverless, validation-required, fail-closed, in the
`*_cmd.rs` pattern the other fourteen gates follow.

Fidelity is *pixel-faithful*, so the gate asserts against the **ewo-jni
constants**, not against a screenshot:

- **Layout**: for each ported widget, the chip rect, baseline, label and value
  origins computed by Rewo must equal the Skia formula recomputed
  independently in the gate — `pad_x 14`, `pad_y 8`, `gap 14`, `radius 12`,
  `chip_w = 2*pad_x + label_w + gap + value_w`, `baseline = top + pad_y + cap`.
- **Anchors**: all nine `Anchor::origin` cases, and the scale-about-anchor
  transform at `SCALE_MIN`/1.0/`SCALE_MAX`.
- **Glyph metrics**: advance and cap-height for a fixed string at fixed axes,
  against values extracted from the TTF by an independent path — the same
  two-oracle discipline M37 used (`swash` vs a raw `ttf-parser` read).
- **Chrome**: read back a rendered shell and assert the analytic SDF bands land
  where the Skia inset/outset would — with a mutation partner per band, since
  §6-style witnesses that only assert a moment have already been shown in this
  repo not to be guards.
- **Glass**: uniform-texture collapse, the trick `portalshot` used — with a
  constant backdrop the refraction integral is CPU-computable, so the frame is
  a number rather than a vibe.

**Every widget ported must add a witness.** The survey work in this same
session produced a standing lesson worth repeating here: a gate that
reimplements a slice of the app's setup will miss whatever the app adds to it
(`itemshot` measured zero glint for exactly that reason). The gate must drive
the production path.

---

## §7 Sequence

1. **Glyph atlas + `swash` scaler + quantized cache.** Gate: metrics oracle.
2. **Text pass** — tracked advances, cap-height baselines, shadow stacks. Gate:
   layout witnesses for one string at three sizes.
3. **Chrome pass** — SDF rrect fill/ring/shadow/highlight. Gate: band witnesses
   with mutation partners.
4. **First widget: Coords.** Smallest interesting one — mono tracked label,
   Fraunces value, one shell. Proves all three subsystems compose.
5. **Liquid glass** — GLSL port + the two cached backdrops. Gate: uniform
   collapse.
6. **The rest of the legit widgets** — FPS, Keystrokes, Armor, Potions, Target.
   Each a transcription plus a witness.
7. **`hud.toml`** — read the same per-profile file the editor writes, exactly
   as M52a did for `modules.toml`. Layout and enablement flow from EwoClient
   with no new contract.

Steps 1–3 are the machinery. Step 4 is the proof. Steps 5–7 are volume.

---

## §8 Scope, resolved (user, 2026-07-28)

All three open questions were answered "yes, do the whole thing". Recorded
with what each one adds.

### 1. The in-game editor comes too

Rewo owns HUD editing, not just HUD reading. `hud.toml` stops being an import
and becomes a file Rewo **writes**, which makes the two clients symmetric —
arrange the HUD in either, and the other picks it up.

What it needs beyond §1–§4: an overlay screen with pointer capture, per-widget
hit rects, drag with anchor re-binding, snap-to-align guides, a resize handle
for the `scale` multiplier, and a side panel of toggles. Rewo is not starting
from zero — M35's inventory screen already established screen open/close,
cursor capture release, mouse-position mapping and click dispatch
(`set_screen_open`, `click_screen`, `screen_key_action`). The editor is a
second screen on that machinery.

**The snap rule is not cosmetic and must be transcribed, not invented.**
EwoClient's editor snaps a dragged widget to the other widgets' edges and to
the anchor grid; two clients that disagree about where a widget lands will
produce `hud.toml` files that visibly fight each other.

### 2. All widgets — with the legit/pvp split intact

All 17: Fps, Coords, Ping, Keystrokes, Armor, Potions, Target, JumpResetText,
JumpResetBar, HitRange, Cps, Items, ShieldCooldown, Reach, AttackCharge,
Combo, Media.

**Seven are PvP and must be `#[cfg(feature = "pvp")]`.** The post-ban refactor
(CLAUDE.md, 2026-05-26) is explicit that the packet-touching set must not exist
in the legit build *at all* — the ban landed with the macros switched off,
pointing at class-name fingerprinting as the surface. That reasoning applies to
Rewo unchanged: a legit `rewo.exe` must not contain a `HitRange` symbol.
`rewo-app` and `rewo-gpu` therefore each need a `pvp` feature, propagated the
way `ewo-core`/`ewo-jni`/`ewo-launcher` already do it. The widget id table is a
12-entry prefix in the legit build and 17 with `--features pvp`, mirroring how
`ewo_core::modules::REGISTRY` is a 12-entry prefix of 26.

`Media` reads Windows SMTC (`crates/ewo-jni/src/media.rs`) and is
platform-gated rather than feature-gated.

### 3. Ping — Rewo will measure it

The other sixteen widgets have their data natively; ping is the one gap, and it
is a protocol feature rather than a rendering one. Two sources, and the widget
should prefer the first:

- **Server-reported latency.** `ClientboundPlayerInfoUpdate` carries an
  `UPDATE_LATENCY` action with a per-player ping in ms. This is what vanilla's
  tab list shows, it covers *other* players (which `Target` and a future tab
  list also want), and `rewo-net` already decodes the player-info packet for
  M7c skins — so this is a new action arm, not a new packet.
- **Client-measured keep-alive RTT.** `ClientboundKeepAlive` → the matching
  `ServerboundKeepAlive` is a round trip the client can time itself. Truer to
  what the player experiences than the server's own figure, and it works
  before the player list is populated.

Ship both, display the keep-alive RTT, and keep the server figure for other
players. Gate: a witness that a synthetic keep-alive pair at a known interval
produces the expected millisecond value, plus one that an `UPDATE_LATENCY`
arm decodes without desyncing the packet — the `DataComponentPatch` lesson
from M41 applies to every new action arm.

---

## §9 Revised sequence

Machinery first, one proof, then volume — unchanged in shape, longer in tail.

1. **Glyph atlas + `swash` scaler + quantized cache.** Gate: metrics oracle.
2. **Text pass** — tracked advances, cap-height baselines, shadow stacks.
3. **Chrome pass** — SDF rrect fill/ring/shadow/highlight.
4. **Coords** — the proof that all three compose.
5. **Liquid glass** — GLSL port + the two slow-clock backdrops.
6. **`hud.toml` read** — layout + enablement from the shared per-profile file.
7. **The remaining legit widgets** — Fps, Keystrokes, Armor, Potions, Target,
   Items. Each a transcription plus a witness.
8. **Ping** (§8.3) — the two sources and their gate. Unblocks the Ping widget.
9. **The editor** (§8.1) — screen, drag, snap, resize, toggles; `hud.toml`
   write. This is where Rewo stops importing the layout and starts owning it.
10. **The pvp widget set** behind `--features pvp` (§8.2), plus the feature
    plumbing through `rewo-app`/`rewo-gpu`.

Steps 1–3 are the machinery, 4 is the proof, 5–8 volume, 9–10 the second
milestone's worth. Nothing after step 4 is blocked on anything before it
except the machinery, so 7/8/10 can interleave.
