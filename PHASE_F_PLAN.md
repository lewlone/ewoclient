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

### F1 — Account switcher UI
- The Account tab becomes a list: each account a row with skin head + name +
  active marker.
- Add account (runs the existing OAuth flow, appends to the store), remove,
  set-active.
- Skin head: fetch + cache the face crop from the account's skin URL (already in
  the profile-fetch payload).
- **Acceptance:** sign into 2+ accounts, switch active, a launch uses the active
  one.

### F2 — Client-profile data model + settings split
- Settle the **profile-scoped vs global** split:
  - *Profile-scoped:* motion-speed, breath-amp, density, warmth,
    accent-hue-shift, theme, vsync, max-fps, audio levels, HUD settings.
  - *Global (stays in `settings.toml`):* game dir, downloads dir, log level.
- Create `profiles/`, `client.toml`, `profiles.toml`. Migrate today's
  `settings.toml` + `hud.toml` into a `"Default"` profile.
- `Prefs` (settings-screen widget state) + `ewo-core::Settings` become reads of
  the *active* profile.
- **Acceptance:** launcher behaves identically post-migration, now sourced from
  `profiles/Default/`.

### F3 — Profile management UI + keybind registry
- Create / rename / delete / duplicate client profiles.
- A profile picker (vdrop) in the Settings chrome or a small Profiles tab.
- The Settings screen edits the *active* profile; switching re-applies prefs
  live.
- Build the keybind registry + a remap-row widget (real UI even with one
  keybind).
- **Acceptance:** two profiles with different warmth/theme/HUD; switching swaps
  the look instantly.

### F4 — The dashboard
- New `screens/dashboard.rs`, replaces `main_menu`, becomes the default screen.
- Active-account card (skin head, name, switch). Active client-profile chip
  (quick-swap vdrop).
- Instance quick-launch cards: name, version/loader badge, last-played,
  one-click Launch.
- Settings / About / Quit relocate into dashboard chrome.
- Add a `last_played` timestamp to `Instance`, written on launch.
- **Acceptance:** cold start → dashboard → one click launches the most-recent
  instance.

### F5 — In-game profile hot-swap
- The overlay SETTINGS tab (Phase E6) gets a client-profile picker.
- Switching in-game re-reads the profile and re-applies HUD layout + HUD
  settings live, through the JNI bridge (`crates/ewo-jni`) — same write-back
  pattern as Phase E6's overlay-mods.
- **Acceptance:** swap profile from inside Minecraft, HUD re-lays-out with no
  restart.

### F6 — Polish + parity
- Velvet-parity pass on the dashboard + all new widgets.
- Entrance animations, hover affordances, breathing where the design calls for
  it. `prefers-reduced-motion` honored.

---

## Deferred / out of scope for F

- **Modules** — EwoClient's own client features as toggleable modules. F builds
  the keybind *seam* only; modules are a later phase.
- Re-keying Minecraft's own `options.txt` from a client profile.
- Per-instance client-profile overrides (profiles stay global).
- Cloud sync of profiles or accounts.
