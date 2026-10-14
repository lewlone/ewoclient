---
paths:
  - "crates/ewo-core/src/modules.rs"
  - "crates/ewo-core/src/pvp.rs"
  - "crates/ewo-jni/src/modules.rs"
  - "crates/ewo-jni/src/pvp.rs"
  - "crates/ewo-launcher/src/keybind.rs"
  - "crates/ewo-launcher/src/profile.rs"
  - "crates/rewo-app/src/modules.rs"
  - "ingame-mod/**"
  - "docs/history/PHASE_G_PLAN.md"
---

# EwoClient modules — the catalog, the legit / pvp split, PvP Utils

Full history: `docs/history/ewoclient-v2-phases.md` ("Phase G" and
"Post-ban refactor: legit / pvp split").

## The rule

**The default build is legit-client only: zero packet synthesis.** An
anticheat ban landed with the macros switched *off*, which pointed at
class-name fingerprinting. Packet-touching assist modules therefore live behind
a build flag and their classes **must not exist in the legit jar at all**.

## Catalog and data flow

- `ewo_core::modules::REGISTRY` is the single source of truth, read by the
  launcher, `ewo-jni` and Rewo (`rewo-app/src/modules.rs`). The launcher's
  `keybind::REGISTRY` is **generated** from it.
- Slots **0..11 are the legit modules** in stable order (Full Bright, FOV
  Control, Toggle Sprint, Toggle Sneak, No Damage Tilt, No View Bob, FreeLook,
  No Fire Overlay, Crosshair on Reach, No Pumpkin Overlay, Hit Color, Hit
  Indicator). Slots **12..25 are assist**, present only under `--features pvp`.
  Legit slot indices never move between builds.
- Config: per profile, `profiles/<name>/modules.toml` (`enabled` + settings).
  Both the launcher and `ewo-jni` read and write it, and modules apply **live**.
- Rust→Java via the `EwoModuleData` buffer. `MODULE_COUNT` is read from buffer
  offset 4, so `enabled(slot)` is false past the live count. JNI adds
  `nativeInitModules` and `nativeModuleToggle` (a keybind press round-trips
  through Rust, which owns module state).
- Every legit module is **non-destructive**: it overrides a computed value via
  a mixin and never writes `options.txt`, so toggling off restores vanilla
  exactly.

## Build mechanics (legit vs pvp)

- Rust: `pvp` feature on `ewo-core` (which gates the registry), propagated by
  `ewo-jni` and `ewo-launcher`. `cargo build` builds legit;
  `cargo build --features pvp` builds pvp.
- Java assist sources live in `dev.lewlone.ewohud.assist.*` (slot constants in
  `assist.AssistSlots`, mixin `assist.mixin.PlayerAttackAssistMixin`). **The
  dependency points from assist to legit, never back**: `EwoModules` ticks and
  routes keys through the neutral `EwoExtensionHook`, and the pvp-only
  `assist.EwoAssistEntry` client entrypoint (declared only in
  `fabric-pvp.mod.json`) registers into it. The legit jar has no class, string
  or reflective name for the assist package. (Until 2026-10 `EwoModules` did
  `Class.forName("…assist.EwoAssist")`, which put the name in the legit jar's
  constant pool.)
- Two mixin configs: `ewohud.mixins.json` always, and `ewohud-pvp.mixins.json`
  in the pvp jar only. `build.ps1 -Pvp` ships `fabric-pvp.mod.json` as
  `fabric.mod.json`.
- **`build.ps1` asserts the split**: the legit jar fails the build if any entry
  sits under `dev/lewlone/ewohud/assist/` or any entry's bytes mention the
  package; the pvp jar fails without `EwoAssistEntry`. CI (`gates.yml`) builds
  both with `-NoDeploy`.
- Regression guards in `ewo-core` fail if a deleted id (`auto_crit`,
  `mace_combo`, `wind_charge_mlg`) reappears or the legit registry exceeds 12.
  `triggerbot` was renamed and humanized into `swing_cadence` (min interval,
  jitter, reaction delay), so a class-name fingerprint no longer matches.

## Mixin hooks (26.x Mojmap) — where rendering moved

`LightTexture` became `net.minecraft.client.renderer.Lightmap`, and the
lightmap is GPU-driven (`LightmapRenderState` UBO; Full Bright cranks
`brightness` after `LightmapRenderStateExtractor.extract`). FOV is no longer
`GameRenderer.getFov`. It's `Camera.calculateFov` (FOV Control `@Redirect`s the
`options.fov()` read there). No Damage Tilt and No View Bob cancel
`GameRenderer.bobHurt` / `bobView`. FreeLook is a spectator-style freecam: it
redirects `LocalPlayer.turn` in `MouseHandler.turnPlayer`, modifies
`Camera.setRotation`, injects at `Camera.alignWithEntity` RETURN, forces the
detached camera type, and freezes the body by forcing the movement
`KeyMapping`s up. Its fly keys (and the release-resync) follow the player's own
key mappings (`keyUp/Down/Left/Right/Jump/Shift/Sprint`, resolved through
`KeyMapping.saveString` and `InputConstants.getKey`, cached until a rebind),
not physical W/A/S/D; an unbound action does nothing, and a failed lookup falls
back to the vanilla default key.

## PvP Utils layer (separate from legit/pvp)

Launcher `SettingsTab::PvpUtils` + `ewo_core::pvp.rs` + per-profile `pvp.toml`;
Java `dev.lewlone.ewohud.pvp.*` ships in **both** builds. `EwoHitRange` is
called from the **legit** `PlayerAttackMixin` on purpose. **Do not "clean up"
that call.**

`pvp.toml` is written atomically (temp file, fsync, rename; `PvpConfig::save`
returns the error) because `EwoPvpModule` polls it: every 250 ms of wall clock
it stats the file once, and re-reads `profiles.toml` only when that file's
(mtime, size) changes; a switch of the active profile reloads even if the two
mtimes match. Paths and profile names go through `ewo_core::pvp::profile_file`
(see `ewo-ingame-hud.md`). `EwoKeybinds` parses `ewo-keybinds.txt` per line, so
one malformed line no longer loses the bindings after it (including the overlay
key).
