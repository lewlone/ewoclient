# Phase E — In-game HUD: build plan

The working plan for EwoClient's in-game HUD. `CLAUDE.md` is the master project
doc; **this file is the durable Phase E sub-plan** — the equivalent of v1's
build-sequence table, scoped to Phase E.

**Fresh session? Read in this order:** `CLAUDE.md` (the "Phase E" section) →
this file → the auto-memory `phase_e_spike.md` and `feedback_phase_e_constraints.md`.
Then start at the first `TODO` row in the table below.

---

## Status

| #  | Step | Status |
|----|------|--------|
| E0 | **Spike** — Skia paints over Minecraft via a dedicated GL context | ✅ DONE (2026-05-20) |
| E1 | Two-clock paint/composite refactor + `HudPaintRate` cap | ✅ DONE (2026-05-20) |
| E2 | First real widget (FPS) — data pipeline + text engine | ✅ DONE (2026-05-20) |
| E3 | Remaining read-only widgets + shared state block | ✅ DONE (2026-05-20) |
| E4 | Input plumbing + overlay open/close keybind | ✅ DONE (2026-05-20) |
| E5 | HUD editor (drag / anchor / toggle stage) | ✅ DONE (2026-05-20) |
| E6 | In-game settings overlay (mod toggles, prefs, palette) | ✅ DONE (2026-05-20) |
| E7 | Polish — refract decision, Velvet re-skin pass, perf | **TODO — start here** |

Each step is a working, launch-verifiable increment. Don't skip ahead.

---

## Where things stand (E0–E6)

The spike proved the hard part: `ewo-render`'s Skia pipeline renders over a
running Minecraft 26.1, stable, verified live (title screen + in-world). E1
split that into the two-clock paint/composite model. E2 added the first real
widget + the JVM→Rust data pipeline. E3 finished the read-only widget set —
the full default HUD (FPS, Coords, Ping, Keystrokes, Armor, Potions,
TargetHUD). E4 added overlay input — a keybind opens a cursor-freeing screen
that forwards mouse/keyboard to Rust. E5 made the HUD editable — drag widgets,
toggle them, anchor them, all persisted to `hud.toml`. E6 turned the overlay
into a 3-tab dashboard (HUD · MODS · SETTINGS) — in-game bundled-mod toggles
and the paint-rate cap.

What exists:
- **`crates/ewo-jni/`** — a `cdylib` in the Cargo workspace. Loaded into the MC
  JVM; creates a **dedicated GL context** on MC's window, builds a Skia
  `DirectContext`, and runs the two-clock model: `paint` renders the HUD to an
  offscreen GPU surface (rate-gated by `HudPaintRate`), `composite` blits it
  onto `fbo 0` every frame. `lib.rs` is the bridge; `hud.rs` holds the widgets.
- **`ingame-mod/`** — Fabric mod `ewo-hud`. `EwoHudMod` loads the cdylib +
  registers the shared data block; `EwoHudData` fills it each frame;
  `EwoHudMixin` (a class-literal `@Mixin(RenderSystem.class)`) injects
  `flipFrame` HEAD → `capture()` + `nativeRender()`. Plain `javac`+`jar` build
  (`build.ps1`) on a **JDK 25** toolchain, no Loom.
- **`EwoLoaderV1/manifest/0.1.0/26.1.json`** — a `file://` `libraries[]` entry
  (`dev.lewlone:ewo-hud:0.1.0`) puts the mod on the classpath.

Iteration loop:
```
cargo build -p ewo-jni                  # Rust side → target/debug/ewo_jni.dll
ingame-mod\build.ps1                    # ONLY if the mod's Java changed
# then delete %APPDATA%\EwoClient\shared\libraries\dev\lewlone\ewo-hud\...\*.jar
# so the launcher re-stages, then relaunch MC 26.1 from the EwoClient launcher
```
The mod auto-finds the dll at `~/Desktop/EwoClientV3/target/{debug,release}/ewo_jni.dll`.
Logs: `%TEMP%\ewo-jni.log` + the per-launch log under `instances/<name>/logs/`.

---

## Architecture — locked decisions

These hold for all of E1–E7. Changing one is a real decision, not a tweak.

1. **GL isolation: a dedicated context, never shared.** Skia and Minecraft each
   own a GL state machine; sharing one corrupts both (flicker, then a driver
   crash — proven in the spike). Every frame: `wglMakeCurrent` to our context,
   do all Skia work, hand the thread's context back to Minecraft untouched.

2. **Two clocks: `paint` and `composite`.** (Constraint #5.)
   - `paint(t)` renders the whole HUD to an **offscreen Skia surface** (a
     GPU texture, window-sized). Early-returns if `t - last_painted < 1/hud_paint_rate`;
     otherwise renders and stamps `last_painted = t`.
   - `composite(t)` blits the most recent offscreen texture onto `fbo 0`. Runs
     **every** frame, so the HUD never tears or vanishes between paints.
   - `hud_paint_rate`: `Match game` (default) / 120 / 60 / 30. "Match" → paint
     every composite.
   - HUD animations (breathing, rim cycle, particles) advance on the **paint**
     delta, not the composite delta — so motion stays consistent with the
     chosen budget.

3. **Data pipeline: JVM → Rust, via a shared buffer.** HUD widgets need live
   game data (fps, coords, ping, armor, potions, target, keys). The mod gathers
   it on the Java side and Rust reads it. Recommended: the mod allocates a
   direct `ByteBuffer`, hands its pointer to Rust once at init, then writes
   into it each frame — Rust reads it with zero per-frame JNI marshaling. E2
   may start simpler (scalar JNI args) and migrate to the buffer in E3.

4. **Input: captured only while the overlay is open.** A keybind toggles
   "overlay open." Open → mixins forward mouse/keyboard to Rust and suppress
   them from the game; the cursor is ungrabbed. Closed → the HUD is
   display-only, the game owns all input.

5. **Persistence: `<config>/EwoClient/hud.toml`.** HUD layout (enabled widgets,
   positions, anchors) and HUD prefs (paint rate, etc.) live in their own TOML.
   Rust owns HUD state and writes the file; the mod passes the EwoClient config
   dir to Rust at init.

### The five non-negotiables (from `feedback_phase_e_constraints.md`)

1. Visual pipeline is `ewo-render` Skia — never a browser engine.
2. Zero Minecraft-vanilla UI shape — no vanilla `Tessellator`/font/widgets for HUD visuals.
3. Velvet & Pearl theme — re-skin every prototype visual to `Theme::VELVET`.
4. Legit client only — HUD widgets + bundled-mod toggles, **no hacked-client modules**.
5. Decoupled paint clock with a user cap — see decision #2 above.

---

## Build steps

### E1 — Two-clock paint/composite refactor ✅ DONE (2026-05-20)

Converted `ewo-jni` from draw-direct to the cached two-clock model. Still just
the spike's rotating panel — this step was the architecture, not new content.

**What shipped** (all in `crates/ewo-jni/src/lib.rs`):
- The spike type `Spike` was renamed `Hud` — this is the real HUD's
  architecture now, not a throwaway.
- `Hud` owns an offscreen GPU `Surface` (`surfaces::render_target`,
  window-sized, RGBA8888 premul, recreated by `ensure_offscreen` on resize).
- `paint(now)` — clears the offscreen surface to transparent, draws the panel
  to it, stamps `last_painted`. Rate-gated: early-returns if
  `now - last_painted < 1/rate`. Animation samples wall-time *at the paint
  moment*, so a capped rate is chunky-but-real-speed, not slowed.
- `composite()` — `image_snapshot()`s the offscreen surface and blits it onto
  the `fbo 0`-wrapped surface. Runs every frame; one `flush_and_submit` ends
  the frame. The snapshot is transient (dropped each frame) so the next
  `paint` renders the offscreen in place — no copy-on-write texture copy.
- `HudPaintRate` enum (`Match` / 120 / 60 / 30). Defaults to `Match`. The real
  setting UI is E6; for now the `EWO_HUD_PAINT_RATE` env var is a test
  override (`30`/`60`/`120`/`match`) — read once at `Hud::create`. The MC JVM
  inherits it from the launcher process.
- Periodic log line `N composites, M paints (rate …)` every 600 composites —
  in `Match` mode `paints == composites`; under a cap `paints` lags, which is
  the proof the decoupling works.

**Verified live:** Match mode renders identically to E0; `EWO_HUD_PAINT_RATE=30`
gives a visibly chunky 30 fps panel while Minecraft holds 400–500 fps.

**The tradeoff, now confirmed live:** painting to an offscreen surface means
glass panels no longer backdrop-blur the *live* game (the spike got that for
free with draw-direct — the panel's refract layer blurred whatever was already
on `fbo 0`). The E1 panel shows tint + rim lights + breathing but no live-game
blur. Fine for small HUD widgets; the call on whether the big settings-overlay
panels get it back (by sampling `fbo 0` at composite time) is an E7 decision.

### E2 — First real widget: FPS ✅ DONE (2026-05-20)

The first genuine HUD widget, with the data pipeline and text behind it.

**Prerequisites (resolved):**
- **JDK 25 installed** at `%APPDATA%/EwoClient/jdks/temurin-25/` — Temurin
  25.0.3+9, extracted from the Adoptium zip (no installer). `build.ps1` points
  at it; the box's JDK 21 stays for everything else.
- **The Minecraft jar is directly compilable.** `shared/versions/26.1/26.1.jar`
  ships **Mojmap-named** (`net.minecraft.client.Minecraft` + ~10.7k readable
  classes) — Mojang ships 26.x deobfuscated. EwoLoader logs `Mappings not
  present!` and does no remapping: the on-disk jar *is* the runtime namespace.
  So the mod compiles straight against it — no tiny-remapper, no `.fabric`
  cache, no mappings download. The "need a Mojmapped jar somehow" worry was
  unfounded.

**What shipped — Java side (`ingame-mod/`):**
- `build.ps1` — JDK 25 toolchain; classpath is the MC jar + every
  `shared/libraries/**/*.jar`. Output stays `--release 21` (v65): a Java-25 JVM
  runs it, JDK 25's javac still reads the v69 MC classes off the classpath, and
  `ewohud.mixins.json` keeps `compatibilityLevel: JAVA_21` unchanged. (Keep
  `.ps1` files ASCII — PowerShell 5.1 reads them as ANSI, so em-dashes break
  the parser.)
- `EwoHudMixin` — now a plain class-literal `@Mixin(RenderSystem.class)`. The
  spike's `@Mixin(targets="…")` string and the `TracyFrameCapture` compile-only
  stub are gone (`stub/` deleted). The `flipFrame` handler reads
  `Minecraft.getInstance().getFps()` and hands it to Rust.
- `EwoHudNative.nativeRender()` → `nativeRender(int fps)` — the first scalar of
  the JVM→Rust pipeline. E3 swaps scalar args for a shared buffer.

**What shipped — Rust side (`crates/ewo-jni/`):**
- New `hud.rs` — `draw_fps`, a Velvet re-skin of `hud.jsx`'s `fps` element
  (`.hud-stat`): a wine chip + rose hairline, a Fraunces number, a tracked
  JetBrains Mono "FPS" eyebrow, a soft drop shadow for legibility over any
  game background.
- `lib.rs` — `Hud` now holds a `FontStore` + the live `fps`; `paint` draws the
  FPS widget (the spike's rotating panel is gone). Fixed top-left anchor
  (`FPS_ANCHOR`); the draggable editor stage is E5.
- **Font sourcing settled:** `FontStore::new()` works as-is from the cdylib —
  `ewo-render` bakes the workspace `assets/fonts/` path in at compile time and
  the dll is built on the same box. `include_bytes!` bundling is only needed if
  the dll ever ships to another machine (a later concern).

**Verified live:** a live FPS chip sits top-left on the title screen and
in-world; the number tracks F3.

### E3 — Remaining read-only widgets ✅ DONE (2026-05-20)

Ported the rest of the legit-client widget set from `hud.jsx::HUD_ELEMENTS`
(`arraylist` skipped — it's the hacked-client module list, constraint #4).
Landed in two verifiable pushes.

**What shipped — the shared data block.** The mod allocates one direct
`ByteBuffer` (`EwoHudData`, 4 KB), hands it to `nativeInit` once; Rust resolves
its address with `GetDirectBufferAddress` (via the `jni-sys` crate) and reads
everything from memory each frame — `nativeRender()` takes no args, zero
per-frame JNI marshaling. A fixed-offset layout mirrored byte-for-byte between
`EwoHudData.java` and `hud.rs`; `SCHEMA_VERSION` (now 2) guards the two sides
against drift. Strings (target name, potion names) are length-prefixed UTF-8
regions; `EwoHudData.putString` truncates on a char boundary, Rust reads with
`from_utf8_lossy`.

**What shipped — push 1: Coords, Ping, Keystrokes** (numeric — no strings/lists):
- `HudData` view + the `Anchor` 9-point model; `draw_stat` (a shared FPS/Ping
  chip), `draw_coords`, `draw_keystrokes` (WASD cross + space bar, keys lit on
  a rose→lavender gradient).

**What shipped — push 2: Armor, PotionHUD, TargetHUD** (the data-rich set):
- `draw_armor` — 4 durability gauges, rose→lavender fill rising from the
  bottom, % centred. `draw_potions` — a column of effects, colour-keyed icon
  (`MobEffect.getColor()`) + name + roman amplifier + `m:ss`/∞ countdown.
  `draw_target` — `Minecraft.crosshairPickEntity` → an avatar (entity initial),
  name, distance, health bar.
- MC reads: `Player.getX/Y/Z`, `getConnection().getPlayerInfo(uuid).getLatency`,
  `Options.key*`, `getItemBySlot` + `ItemStack.getDamageValue/getMaxDamage`,
  `getActiveEffects`, `crosshairPickEntity` + `LivingEntity.getHealth`.

Fixed default anchors (FPS/Coords top-left, Keystrokes bottom-left, Ping
bottom-right, Armor bottom-centre lifted above the hotbar, Potions upper-right,
Target top-centre); each widget hides when not relevant. The draggable editor
that makes placement user-configurable is E5.

**Verified live:** the full HUD renders with correct live data in-world.

### E4 — Input plumbing + overlay open/close ✅ DONE (2026-05-20)

A keybind toggles the overlay; while open, input routes to Rust and the game
is frozen out; while closed, the HUD is display-only.

**Approach — a custom `Screen`, not mouse-handler mixins.** The plan first
imagined mixins into MC's mouse + keyboard handlers, but a custom `Screen`
(`EwoOverlayScreen`) turned out cleaner: while it is the active screen MC
already frees the cursor, routes all mouse/keyboard to it, and starves the
game world of input — exactly decision #4, for free. It renders nothing
(`extractRenderState` overridden empty); the Skia HUD paints the overlay.
`isPauseScreen()` returns false so singleplayer doesn't pause.

**What shipped:**
- `EwoOverlayScreen` — the input sink. Its `mouse*`/`key*` overrides convert
  MC's GUI-scaled coords to window pixels and forward to Rust.
- `KeyboardHandlerMixin` — injects `KeyboardHandler.keyPress` HEAD; on Right
  Shift with no screen open it opens the overlay. (Closing is the screen's own
  job — Right Shift in `keyPressed`, or Esc — so the two halves never collide.)
- Four JNI input exports — `nativeMouseMove` / `nativeMouseButton` /
  `nativeMouseScroll` / `nativeKey` — feeding a `hud::Input` on the
  render-thread `Hud` (via a `with_hud` helper). A `FLAG_OVERLAY` buffer bit
  tells Rust the overlay is open.
- A placeholder overlay (`hud::draw_overlay`) — a dimmed backdrop + a centred
  panel showing live cursor / clicks / last-key / scroll, with a rose ring
  tracking the cursor. Proves every input path; E5 replaces it with the editor.

**MC 26.x note:** the input API is event-object based — `KeyEvent`,
`MouseButtonEvent` (records). Screen input methods take those, not raw params.

**Verified live:** Right Shift opens/closes the panel, the cursor frees while
open, the readouts track input, and game input is untouched while closed.

### E5 — HUD editor ✅ DONE (2026-05-20)

`hud.jsx`'s editor model, brought into the overlay. Landed in two pushes.

**Approach — direct manipulation, not a scaled-stage preview.** The prototype
edits widgets on a shrunk 1920×1080 stage; in-game it's better to drag the
*real* widgets in place. Positions are still fractional (0..1 of the window)
+ an anchor, so the coordinate model is resolution-independent.

**What shipped — push 1 (drag + persist):**
- `HudLayout` — every widget's placement (`enabled`, `anchor`, fractional
  `x`/`y`); `hud.rs::draw` is now layout-driven, and records each widget's
  drawn bounds for hit-testing.
- Drag — while the overlay is open, each widget shows a drag outline; click +
  drag repositions it. The layout saves to `<config>/EwoClient/hud.toml` on
  every drag-release and loads on startup (hand-rolled `[section] key = value`
  reader/writer; Rust self-computes the config dir from `%APPDATA%`).
- **Snap-to-align** (a refinement the user asked for) — a dragged widget's
  edges/centres snap to other widgets' within 6px, with a faint rose guide
  line. Gentle: easy to drag past.

**What shipped — push 2 (editor furniture):**
- A left-edge **side panel**: a 7-widget list (dot + name + on/off toggle) and
  a 3×3 **anchor preset grid**. Clicking a toggle shows/hides a widget;
  clicking a row selects it; clicking a grid cell jumps the selected widget to
  that corner/edge/centre. This also lets you position *hidden* widgets.

**Also fixed (pre-existing E4 bugs surfaced here):** Esc now closes the
overlay (the `keyPressed` override has to handle it — the default path is
bypassed); the `keyPress` mixin now only opens on `GLFW_PRESS`, so the release
after a close can't reopen the overlay.

**Verified live:** drag + snap + toggle + anchor all work; layout persists
across close/reopen and a full relaunch.

### E6 — In-game settings overlay ✅ DONE (2026-05-20)

The overlay became a **dashboard** — a top-centre tab strip switching between
views — rather than a single settings page (the user asked for the prototype's
`app.jsx` shell shape). Landed in three pushes.

**What shipped — push 1 (the shell):** `OverlayView` + a top-centre tab strip;
the E5 editor became one view; a Settings scaffold; view-switching.

**What shipped — push 2 (3 tabs + Settings):** the tab set is `HUD · MODS ·
SETTINGS`. The **Settings view** is real — a paint-rate selector
(`MATCH/120/60/30`) for the `HudPaintRate` cap, which was env-var-only since
E1. The cap is now a persisted pref — a `[prefs]` section in `hud.toml`.

**What shipped — push 3 (the MODS view):** a Velvet re-skin of a ClickGUI
module list — one row per bundled mod (category dot · name · category·version ·
on/off toggle), from `bundled::CATALOG`. The launcher↔overlay write-back
(`crates/ewo-launcher/src/overlay_mods.rs`): the launcher writes
`overlay-mods.toml` (catalog + state) into the instance dir pre-launch; an
in-game toggle writes a delta to `overlay-mod-overrides.toml`; the launcher
consumes + applies + deletes that delta on the next launch. The delta form
means an in-game toggle never clobbers a launcher-UI toggle made meanwhile.

**Deviation from the plan:** the plan imagined "heavy reuse of launcher widgets
(`vtoggle`, `vslider`, `vdrop`)" — but those live in `ewo-ui` (taffy etc.),
which the cdylib shouldn't pull. The overlay's controls are drawn directly in
`hud.rs` to the same Velvet look. The Cmd/Ctrl+K command palette was left as a
later nicety.

**Verified live:** all three tabs work; the paint-rate cap takes effect +
persists; the MODS list renders and toggling a mod takes effect next launch.

### E7 — Polish

- **Decide the glass-refract question:** should the settings overlay's big glass
  panels blur the live game behind them? Options: accept tinted-glass-without-blur,
  or sample `fbo 0` into the overlay region at composite time. (HUD widgets
  don't need it — this is an overlay-only call.)
- Full Velvet re-skin pass against the prototype screenshots in `Query.zip/uploads/`.
- Perf profiling at high refresh; tune the paint-rate default and widget cost.

---

## Open design questions (decide when the step reaches them)

- **Glass refract over the live game** — E1 trades it away for the two-clock
  cache; E7 decides whether/how the overlay gets it back.

## Out of scope for E1–E7 (future)

- Profiles / loadouts (HUD layout + mod toggles + prefs as named presets).
- The Phase E "host" idea of *replacing* the pause menu — the HUD composites
  *over* the game; a full screen-replacement is a later question.
- macOS/Linux — the cdylib's GL bridge is currently Win32/WGL only.

---

## Where to look when context is thin

- **`CLAUDE.md`** → "Phase E" section — the spike summary + the locked findings.
- **This file** — the build sequence and architecture.
- **`crates/ewo-jni/src/lib.rs`** — the entire Rust bridge; well-commented.
- **`ingame-mod/`** — the Fabric mod; `README.md` there explains the build.
- **`Query.zip`** (in `~/Downloads`) — `hud.jsx` is the editor + widget model;
  `styles.css` is structural reference (re-skin all tokens to Velvet).
- **Auto-memory** — `phase_e_spike.md` (spike state), `phase_e_prototype.md`
  (Query.zip file map), `feedback_phase_e_constraints.md` (the five rules).
