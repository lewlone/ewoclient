# Phase G — EwoClient Modules

The second feature phase past the locked v1 + v2 build sequence. Run like Phase
E and F: numbered steps, each a working build, per-step detail recorded here as
it lands.

**Status: complete — G0–G8 all shipped.** This file is now a record, not a
forward plan. The `ewo-hud` mod jar still needs a `build.ps1` rebuild (with the
game closed) to compile the G3–G7 mixins — see the Phase G section in
`CLAUDE.md`.

---

## Purpose

Phase F5c built the keybind registry as *"the seam future EwoClient modules plug
their bindable actions into"* and stopped there. Phase G builds the modules.

A **module** is an in-game, legit-client feature with an on/off state, optional
numeric settings, and an optional keybind — Full Bright, FOV control, Toggle
Sprint, FreeLook. The quality-of-life features Lunar/Badlion ship. Phase G
delivers the module *framework* plus seven starter modules.

**Hard constraint (Phase E #4): legit-client features only.** No hacked-client
modules — not now, not ever. No KillAura, no reach, no ESP, no names from that
world. The in-game MODULES view is a clean Velvet feature list, styled like the
existing MODS list — explicitly *not* a hacked-client ClickGUI grid.

---

## The core problem

Everything in-game today flows one way — **Java → Rust**. `EwoHudData` is a
shared direct `ByteBuffer` the `ewo-hud` mod fills with game state each frame;
`ewo-jni` reads it and *paints* the HUD. Mod toggles and keybinds round-trip
through per-instance files consumed at the *next* launch.

Modules are different. Full Bright, FOV, Toggle Sprint, FreeLook all have to
**change the running game**, and only the Java mod can touch Minecraft. So
Phase G adds the missing direction: a **live Rust → Java channel**.

```
Launcher Settings ──┐
                    ├──► profiles/<name>/modules.toml ◄──┐  per-profile config
in-game MODULES tab ┘         (sibling of hud.toml)       │
                                                         │
                          ewo-jni Editor ────────────────┘  loads at start,
                                │                            writes on toggle
                                │ writes the buffer every frame
                                ▼
              EwoModuleData buffer   ← NEW: Rust→Java, symmetric to EwoHudData
                                │ read every frame
                                ▼
                     EwoModules (Java) ──► mixins apply the effects
```

---

## Locked decisions

- **Module config is per client profile.** A separate `profiles/<name>/modules.toml`,
  exactly parallel to `hud.toml`. It joins the keybinds + tweak tokens that
  Phase F already made profile-scoped. Both the launcher and `ewo-jni` read and
  write it; they never run concurrently.
- **No overrides-dance.** Bundled mods need `overlay-mod-overrides.toml`
  (consumed next launch) because a mod jar can't be hot-swapped. Modules apply
  *live* — `modules.toml` is written directly by whichever side changed it.
- **The catalog lives in `ewo-core`** (`ewo_core::modules::REGISTRY`) so the
  launcher *and* `ewo-jni` share one source of truth. The launcher's
  `keybind::REGISTRY` is derived from it — each module contributes a
  `KeybindAction`, so module hotkeys flow through the Phase F seam unchanged.
- **Effects are non-destructive.** Every module overrides a *computed* value via
  a mixin — gamma, FOV, view-bob, hurt-tilt, camera rotation. Nothing writes
  Minecraft's `options.txt`; toggling a module off restores vanilla behavior
  exactly.
- **A second buffer, not an extension of `EwoHudData`.** `EwoHudData` is
  Java→Rust; the module channel is Rust→Java. Separate direction, separate
  lifecycle, separate `SCHEMA_VERSION`. The mod allocates both at init.
- **The keybind for a module toggles it; FreeLook's is hold-to-activate.** Six
  of the seven modules toggle on a key press. FreeLook's key is momentary — the
  `ModuleDef.hold_key` flag marks it.
- Persistence stays line-based TOML, hand-parsed on the `ewo-jni` side (it
  already hand-parses `hud.toml` / `profiles.toml` — no `toml` crate in the
  cdylib). Offline-first invariant unchanged: modules touch nothing on the
  network.

---

## Disk layout (after G)

```
<config>/EwoClient/profiles/<name>/
  client.toml      ← unchanged (already carries module keybinds via G0)
  hud.toml         ← unchanged
  modules.toml     ← NEW — per-module { enabled, settings }
```

`modules.toml` format:

```
# EwoClient modules — per client profile.

[fullbright]
enabled = false

[fov]
enabled = true
fov = 95.0
```

Absent file → every module at its `REGISTRY` default. Absent section/field →
that module/setting at its default. A hand-edited or older file never breaks.

---

## The shared buffer — `EwoModuleData`

A direct `ByteBuffer`, allocated by the mod, address resolved once by Rust.
Rust writes it every frame (unconditional — ~120 bytes, not rate-gated like the
HUD paint); Java reads it every frame to drive the effect mixins.

```
offset  type   field
   0    i32    schema_version        (EwoModuleData.SCHEMA_VERSION, starts at 1)
   4    i32    module_count          (== modules::REGISTRY.len() — drift guard)
   8    record[module_count]         16 bytes each, in REGISTRY order:
          +0   i32   enabled (0 / 1)
          +4   f32   setting[0]
          +8   f32   setting[1]
          +12  i32   reserved
```

`CAPACITY` 256 bytes (room for 15 modules). The Java side mirrors the layout in
`EwoModuleData.java`; `SCHEMA_VERSION` guards the two against drift, exactly as
`EwoHudData` already does.

Two new natives on `EwoHudNative`:

- `nativeInitModules(ByteBuffer)` — register the buffer (once, at mod init).
- `nativeModuleToggle(int index)` — flip module `index`'s enabled flag. Called
  from the Java key handler when a module's toggle key is pressed; Rust owns the
  state, so the keypress round-trips through here.

---

## The module set

| Module | Category | Effect | Hook (Mojmap names verified per-step against the on-disk MC jar) |
|---|---|---|---|
| Full Bright | Visual | World renders fully lit | Override the computed gamma / lightmap brightness |
| FOV Control | Visual | FOV set to a slider value, past the 110° cap | Override the computed field-of-view |
| Toggle Sprint | Movement | Sprint held without the key down | Force the sprint `KeyMapping` from the frame hook |
| Toggle Sneak | Movement | Sneak held without the key down | Force the sneak `KeyMapping` from the frame hook |
| No Damage Tilt | Camera | No camera lurch on taking damage | `@Inject` cancel on the hurt-tilt method |
| No View Bob | Camera | No walk view-bob | `@Inject` cancel on the view-bob method |
| FreeLook | Camera | Free camera while the key is held | `Camera` + mouse-look mixins; separate camera yaw/pitch |

All seven default **off** and **unbound** — the user opts in.

---

## Steps

### G0 — Module catalog + keybind-registry refactor ✅ shipped
- `crates/ewo-core/src/modules.rs` — `ModuleDef`, `ModuleCategory`,
  `ModuleSetting`, `MAX_SETTINGS`, `REGISTRY` (the seven modules). Pure
  `&'static` data, zero new deps. Exported as `ewo_core::modules`.
- `keybind::REGISTRY` goes from a `const &[KeybindAction]` to a
  `LazyLock<Vec<KeybindAction>>` built from the core `overlay.open` action plus
  one per module. `KeyChord` gains an `UNBOUND` sentinel (key `0`) +
  `is_bound()`; `label()` renders it `"Unbound"`. The Keybinds tab,
  `client.toml`, and `ewo-keybinds.txt` pick up the module actions for free.
- Slice-deref keeps every `keybind::REGISTRY` call site working; only the two
  bare `for … in keybind::REGISTRY` loops need `.iter()`.

### G1 — `modules.toml` persistence + the `EwoModuleData` channel ✅ shipped
- `ewo-jni` side: a `ModuleConfig` (per-module enabled + settings), loaded from
  `profiles/<active>/modules.toml` (hand-parsed, like `HudLayout::load`),
  saved on change. Owned by the overlay `Editor`.
- Java: `EwoModuleData.java` (buffer + layout mirror), `EwoModules.java`
  (reads the buffer), `nativeInitModules` + `nativeModuleToggle` on
  `EwoHudNative`, allocation wired in `EwoHudMod`.
- Rust: the two new JNI exports; `Hud::frame` writes the buffer every frame.
- `profile::duplicate` copies `modules.toml` into the new profile.
- Channel proven with a log line; no effects yet.

### G2 — In-game MODULES overlay tab ✅ shipped
- A fifth `OverlayView::Modules`; the tab strip + dispatch grow to five.
- `draw_modules` / `modules_layout` — a Velvet feature list modelled on the
  MODS tab: per module a category dot, name, description, on/off toggle.
- A minimal overlay slider primitive for FOV's setting.
- Toggling a module writes `modules.toml` and flows through the buffer.

### G3 — Full Bright + FOV Control ✅ shipped
- Two render-override mixins, both non-destructive (vanilla `options.txt` is
  never written — toggling a module off restores vanilla exactly):
  - `CameraMixin` `@Redirect`s the lone `options.fov()` read in
    `Camera.calculateFov`, so FOV Control's slider value replaces the base
    FOV (past the 110° cap) while the speed/death/fluid FOV effects still
    layer on top.
  - `LightmapRenderStateExtractorMixin` `@Inject`s at the return of `extract`
    and cranks `LightmapRenderState.brightness` (the gamma-derived field the
    GPU lightmap shader reads) past the vanilla cap.
- Targets verified against the 26.1.1 Mojmap bytecode. Note: 26.x is GPU-
  driven — `LightTexture` → `Lightmap`, FOV moved into `Camera.calculateFov`.

### G4 — No Damage Tilt + No View Bob ✅ shipped
- `GameRendererMixin` — `@Inject(at = HEAD, cancellable = true)` on
  `GameRenderer.bobHurt` (the damage camera-tilt) and `bobView` (the walking
  view-bob). Each cancels its method when its module is on, so the camera
  motion is skipped; the vanilla View Bobbing option is left untouched.

### G5 — Toggle Sprint + Toggle Sneak ✅ shipped
- `EwoModules.applyMovement` (run each frame from `flipFrame`) forces the
  `keySprint` / `keyShift` `KeyMapping` down while the module is on, and
  releases the key exactly once when the module turns off so it never sticks.

### G6 — FreeLook ✅ shipped
- `EwoFreeLook` holds the free camera's yaw/pitch — it polls the bound key
  each frame (`glfwGetKey`), snapshots the body's facing on the rising edge,
  and accumulates mouse deltas with `Entity.turn`'s 0.15 factor.
- `MouseHandlerMixin` `@Redirect`s the `LocalPlayer.turn` call in `turnPlayer`
  — while FreeLook is active the delta drives `EwoFreeLook`, not the player,
  so the body's facing stays frozen.
- `CameraMixin` `@ModifyVariable`s both `Camera.setRotation` arguments to the
  freelook yaw/pitch while active; the camera snaps back to the body on release.
- `EwoKeybinds` extended to expose every action's bound code (not just the
  overlay key) — the seam G7 reuses.

### G7 — Module keybinds end-to-end ✅ shipped
- The launcher→file→mod chain was already complete: G0 put the module actions
  in `keybind::REGISTRY` (so the Keybinds tab + `ewo-keybinds.txt` carry them),
  and G6 extended `EwoKeybinds` to parse every action's code.
- `EwoModules.handleKeyPress` maps a pressed key to its toggle module and flips
  it via `nativeModuleToggle` (Rust owns module state).
- `KeyboardHandlerMixin` routes presses: the overlay key, then module toggle
  keys (in-world only, so a bound key still types in chat / menus). FreeLook's
  key stays a hold, polled by `EwoFreeLook`.

### G8 — Launcher Settings → Modules tab ✅ shipped
- A `SettingsTab::Modules` (8th tab) — a custom-layout tab modelled on
  Keybinds: a Velvet toggle per catalog module, plus FOV Control's slider.
- The tab edits `prefs.module_toggles` / `module_fov` directly; a change sets
  `modules_changed`, and the main loop writes `profiles/<active>/modules.toml`
  via `profile::save_modules` — the same file `ewo-jni` reads.
- `profile::load_modules` is applied on startup and on profile switch.

---

Phase G has folded into `CLAUDE.md`, like E and F. This file is now a record.

| # | Step | Status |
|---|---|---|
| G0 | Module catalog + keybind refactor | ✅ shipped |
| G1 | `modules.toml` + `EwoModuleData` channel | ✅ shipped |
| G2 | In-game MODULES tab | ✅ shipped |
| G3 | Full Bright + FOV | ✅ shipped |
| G4 | No Damage Tilt + No View Bob | ✅ shipped |
| G5 | Toggle Sprint + Toggle Sneak | ✅ shipped |
| G6 | FreeLook | ✅ shipped |
| G7 | Module keybinds end-to-end | ✅ shipped |
| G8 | Launcher Modules tab (optional) | ✅ shipped |
