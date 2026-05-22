# Phase F — Profiles & Dashboard

The first feature phase past the locked v1 + v2 build sequence. Run like Phase E:
numbered steps, each a working build, per-step detail recorded here as it lands.

This file is a **forward plan** until F6 ships, then it becomes a record (like
`PHASE_E_PLAN.md`).

---

## Purpose

Three things, two of them independent "profile" systems:

1. **Accounts** — multiple Microsoft accounts, switchable. Auth today is
   single-account; `auth.toml` holds exactly one token.
2. **Client profiles** — named, hot-swappable bundles of *client config*
   (preferences, keybinds, HUD layout). Think Lunar/Badlion presets: a "PvP"
   profile, a "Building" profile, swapped live.
3. **The dashboard** — a real launcher home screen that replaces the minimal
   asymmetric main menu.

---

## Locked decisions

- **Accounts and client profiles are orthogonal.** Two independent switchers;
  any account × any client profile.
- **Client profiles are global**, not per-instance. Your "PvP" profile follows
  you across every instance.
- **Keybinds in a client profile are EwoClient's own keys** — *not* Minecraft's
  `options.txt`. Phase F does not re-key Minecraft itself.
- **The keybind system is a module-extensible registry.** F ships with one
  keybind (overlay-open) but the registry is built so future EwoClient
  **modules** contribute their own bindable actions. "Modules" — EwoClient's own
  planned legit client features — are out of scope for F; the registry is the
  seam they plug into later.
- **The dashboard replaces `main_menu`.** Settings / About / Quit relocate into
  the dashboard chrome.
- Persistence stays TOML via the existing `dirs`-based conventions. No new deps
  unless a step explicitly calls for one. Offline-first invariant unchanged.

---

## Disk layout (after F)

```
<config>/EwoClient/
  auth.toml                 ← { accounts: [...], active: <uuid> }   (was: one account)
  profiles.toml             ← { profiles: [name...], active: <name> }
  profiles/<name>/
    client.toml             ← preferences + keybinds + HUD settings
    hud.toml                ← HUD layout (moved from <config>/EwoClient/hud.toml)
  settings.toml             ← GLOBAL-only leftovers (paths, log level) — see F2
  instances.toml            ← unchanged
```

Migration runs once on the first F-build launch (see F0 + F2). Vanilla-launcher
disk interop under `shared/` is untouched.

---

## Data model sketch

**Accounts** — `auth.toml`:
- `AccountStore { accounts: Vec<StoredAccount>, active: Option<Uuid> }`
- `StoredAccount { uuid, name, ms_refresh_token }` — the live `minecraft_token`
  stays runtime-only, re-fetched per session.

**Client profile** — `profiles/<name>/client.toml`:
- `ClientProfile { name, preferences: Preferences, keybinds: BTreeMap<ActionId, KeyChord>, hud: HudSettings }`
- `Preferences` — the profile-scoped slice of today's settings (F2 split below).
- HUD *layout* stays in the sibling `hud.toml`, Phase E format unchanged.

**Keybind registry** — static list (new `keybind` module):
- `KeybindAction { id: &'static str, label: &'static str, module: &'static str, default: KeyChord }`
- F registers one: `overlay.open` → Right Shift.

---

## Steps

### F0 — Account data model + migration (no UI)
- `auth.toml` schema → `AccountStore`. Rewrite `auth/persistence.rs`.
- Migration: an existing single-account `auth.toml` wraps into
  `{ accounts: [it], active: it.uuid }`.
- `AuthService` / `MinecraftAccount` keep current behavior — still drives exactly
  the *active* account; F0 only makes the store plural underneath.
- **Acceptance:** an already-signed-in user keeps their session after upgrade;
  nothing visible changes.

### F1 — Account switcher UI ✅ shipped
- The Account tab is now a list — each account a row with a monogram avatar,
  name, short UUID, an active marker, and a remove-×. Click a row to make it
  active; click × to remove. An "Add account" / "Sign in with Microsoft" /
  "Try again" button runs the interactive OAuth flow.
- `AuthService` reworked to own the `AccountStore` + an `AuthOp` (Idle /
  Working / Failed) — the single source of truth. `set_active` / `remove` /
  `start_interactive`; `refresh_active_token` brings a switched-to account's
  token live. The F0 `persistence::{load,save,clear}` shims are gone.
- **Avatars are monograms** (a Velvet-tinted disc + initial, tint hashed from
  the UUID) — *not* skin heads. Real skin-head avatars are deferred: they need
  the profile fetch (`chain.rs::McProfile`) to capture the skin texture URL
  plus a threaded skin-image fetch/cache. Tracked as an F6-polish item.
- **Acceptance:** sign into 2+ accounts, switch active, a launch uses the
  active one. *(Visual verification of the new tab layout still pending.)*

### F2 — Client-profile data model + settings split ✅ shipped
- New launcher-side `profile` module (`crates/ewo-launcher/src/profile.rs`).
  Disk layout: `profiles.toml` (registry), `profiles/<name>/client.toml`
  (profile-scoped config), `settings.toml` (now global-only).
- **Split shipped** — *profile-scoped:* the five tweak tokens (motion /
  breath / density / warmth / accent-hue), theme, vsync, max-fps, audio
  levels. *Global:* game dir, downloads dir, window mode, auto-backup, log
  level, telemetry. (Window-mode / auto-backup / telemetry weren't in the
  headline list — classified global as system, not cosmetic, settings.)
- `profile::load` reconstructs the unified `SettingsConfig` + `Settings`
  tokens; `profile::save` splits them back. A pre-F `settings.toml` is
  migrated on first run — profile slice into `profiles/Default/`,
  `settings.toml` rewritten global-only. Launcher-only change: `ewo-render`'s
  `SettingsConfig` stays unified, `ewo-core::Settings` untouched.
- **`hud.toml` is NOT migrated into the profile yet** — it's in-game-side
  (read/written by `crates/ewo-jni`). Folding it into the profile dir is
  deferred to F5 (in-game hot-swap), which already touches that side.
- One "Default" profile, no UI (that's F3). 3 split/merge round-trip tests.
- **Acceptance:** launcher behaves identically post-migration, sourced from
  `profiles/Default/`.

### F3 — Profile management UI ✅ shipped
- New **Profiles tab** in the Settings sidebar (2nd, after Account) — a
  list of profile rows: click to switch active, per-row delete-×, plus
  "New profile" and "Duplicate current" buttons.
- `profile` module gains the management API: `list` / `active_name` /
  `switch` / `create` / `duplicate` / `delete`. Switching re-applies the
  profile's config live (Settings widgets, cosmetic tokens, vsync,
  backdrop density) via `App::apply_loaded_config`.
- The Settings screen already edits the active profile (since F2 — every
  settings save splits into the active `client.toml`).
- **Rename deferred to F6** — it needs text-input plumbing; new profiles
  are auto-named ("Profile 2", "<name> copy") for now.
- **Keybind registry moved to F5** — a remap UI does nothing until the
  in-game side reads keybinds from the profile (F5's job); building it in
  F3 would be a non-functional shell.
- **Acceptance:** two profiles with different warmth/theme; switching
  swaps the look instantly. *(Visual verification pending.)*

### F4 — In-game HOME / overview tab ✅ shipped

**Re-scoped 2026-05-21:** "the dashboard" is a HOME tab in the *in-game
overlay*, not a launcher home screen — the launcher main menu stays as-is.

- `OverlayView::Home` added to the overlay (`crates/ewo-jni/src/hud.rs`);
  the tab strip is now HOME · HUD · MODS · SETTINGS and HOME is the
  default view — opening the overlay lands on the overview, not the HUD
  editor.
- `draw_home` — an overview panel: 5 session stat cards (FPS, ping,
  playtime, coords, server), an account + active-client-profile line, and
  7 per-HUD-widget quick-toggle chips (flip a widget on/off without
  entering the HUD editor; a click writes `hud.toml`).
- **Data pipeline bumped v2 → v3** — `EwoHudData.java` + `hud.rs` gained
  playtime / server / player-name fields; `SCHEMA_VERSION` is 3 on both
  sides (a mismatch blanks the HUD, so they move in lockstep).
- Active client-profile name read from `profiles.toml` (read-only) once
  at `Editor` construction.
- **Verification pending** — builds clean (Rust workspace + the in-game
  mod jar); needs an in-game look (launch Minecraft, Right Shift).

### F5 — In-game profile hot-swap
- Move `hud.toml` under `profiles/<name>/` (deferred from F2) so HUD layout
  is genuinely per-profile, and teach `crates/ewo-jni` the active profile.
- **Decided (user, 2026-05-21):** the in-game profile switcher is a profile
  **picker row at the top of the overlay SETTINGS tab** (above the
  paint-rate controls) — *not* a 4th tab. It's a switcher only; create /
  rename / delete stay launcher-side (F3).
- Switching in-game re-reads the profile and re-applies HUD layout + HUD
  settings live, through the JNI bridge (`crates/ewo-jni`) — same write-back
  pattern as Phase E6's overlay-mods.
- Build the module-extensible keybind registry + a remap-row widget (moved
  from F3) — keybinds become functional here, where the in-game side reads
  them from the profile.
- **Acceptance:** swap profile from inside Minecraft, HUD re-lays-out with no
  restart.

### F6 — Polish + parity
- Velvet-parity pass on the dashboard + all new widgets.
- Entrance animations, hover affordances, breathing where the design calls for
  it. `prefers-reduced-motion` honored.
- ✅ **3D skin viewer (shipped early)** — the in-game HOME tab shows the
  signed-in player's skin as a drag-rotatable 3D model, cape included. A
  Skia software renderer (`crates/ewo-jni/src/skin.rs`) — projected
  textured quads, back-face culled, painter-sorted, per-face shaded. The
  mod (`EwoSkinExport`) downloads the skin/cape PNGs from the GameProfile
  texture URLs. Supersedes the flat skin-heads originally planned.
  Wide-model only — slim arms are a follow-up.
- Profile rename (deferred from F3) — reuse the instances inline text-input
  pattern; until then new profiles are auto-named.
- (Launcher-side account avatars stay monograms — the 3D viewer is in-game.)

---

## Deferred / out of scope for F

- **Modules** — EwoClient's own client features as toggleable modules. F builds
  the keybind *seam* only; modules are a later phase.
- Re-keying Minecraft's own `options.txt` from a client profile.
- Per-instance client-profile overrides (profiles stay global).
- Cloud sync of profiles or accounts.
