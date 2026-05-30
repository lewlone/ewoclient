# Phase H — Social: Friends, Presence, Live Server Stats, Roblox-style Join

The third feature phase past the locked v1 + v2 build sequence. Run like
Phases E–G: numbered steps, each a working build, per-step detail recorded
here as it lands.

**Status (updated 2026-05-30): mostly BUILT, not yet live.** This file's
per-step text below is the original forward plan and is now partly stale —
read the "Phase H — Social" section of `CLAUDE.md` for the authoritative
current state. Summary:
- **H1–H5 BUILT** across all three repos (launcher `social/`, chickenbot
  `api.py`/`database.py`, ChickenLink `/launcher-link`). The launcher↔bot
  wire contract was verified handler-by-handler 2026-05-30 and matches.
- chickenbot Phase H code is **committed**; the launcher + ChickenLink
  plugin pieces were **uncommitted** until the 2026-05-30 checkpoint.
- **H6** (live server widget + Roblox-style join) **BUILT 2026-05-30** —
  `--quickPlayMultiplayer` launch via a shared `start_launch` helper,
  `in_game` presence, 15s server-status poller, main-menu network widget,
  friend "Join" button. Bot `GET /api/server-status` made public (needs
  redeploy). Widget visual placement unverified. **H7** (WebSocket) not
  done (polling by design).
- The in-game **FRIENDS overlay tab is not added** yet (tab strip is
  HOME · HUD · CROSSHAIR · MODULES · MODS · SETTINGS).
- **Real remaining blocker is ops**: deploy the bot to the VPS with nginx
  routing `/bot/api/*` → `:8080`, then run a live end-to-end test. H0's
  `ssh ewo-vps` key was generated 2026-05-27; public-key deploy may still
  be pending.

---

## Purpose

The launcher should know your friends, their presence on the chickenedin
network, and let you join them with a click — Roblox-style. The CLAUDE.md
non-negotiable still holds: **"OFFLINE FIRST. NOTHING PHONES HOME."**
remains literally true when signed out. Social is strictly opt-in.

> Social is the launcher's first feature that crosses the network. It must
> stay opt-in, the bot must remain the source of truth, and the launcher
> must keep working with zero network connectivity for the user who never
> signs in.

---

## What we plug into (not what we build)

The user already operates a real Minecraft network ("chickenedin"). The
existing stack lives at `C:/Users/valtteri/Desktop/FULLSTACK/`:

- **chickenBOT** — Python aiohttp, owns a MySQL DB, exposes a Bearer-token
  HTTP API. Already has `/api/server-status` (live online players + TPS +
  count), `/api/levels/leaderboard`, `/api/me/linked`, the `links` table
  mapping `minecraft_uuid ↔ discord_id`.
- **ChickenLink** — Paper plugin v2.1.0, runs on the MC servers. Owns the
  in-game `/link` flow (player runs `/link` → 6-digit code → bot's
  `/api/link-code` → user types code in Discord → bot writes the
  `links` row) and pushes server status + per-player stats to the bot.
- **chickenedin-website** — Next.js, talks to the bot via the
  `botApi()`/`botApiSafe()` helpers in `src/lib/bot-api.ts` with a shared
  bearer token (`BOT_API_TOKEN` env var ↔ `Config.API_SECRET`).

**Phase H plugs the launcher into the bot the same way the website does.**
We do NOT build a parallel social service. Three new tables in the bot's
MySQL, six new endpoints on the bot, a Friends sidebar + main-menu live
widget on the launcher. The Discord-MC link is the canonical identity.

---

## Locked decisions

- **MS auth stays the launcher's primary identity. Discord-link unlocks
  social.** A player whose MC UUID is in the `links` table sees the
  Friends/Presence UI; a player who hasn't linked sees a "link your
  Discord on chickenedin.com" affordance. The launcher remains usable
  off-network for unlinked players (the offline-first invariant holds
  identically — they just see no social).
- **Identity = MC UUID; social key = linked Discord ID.** Launcher
  resolves once at MS sign-in via `GET /api/links/by-uuid`; caches the
  Discord ID for the session.
- **Per-user API token, not a shared `BOT_API_TOKEN`.** The website ships
  a single token everyone with the repo can read; the launcher ships to
  end users. A new `social_tokens` table issues a 256-bit token per
  linked Discord ID at first launcher-link (via a `/link` companion flow
  — see H0). The token persists locally next to MS auth in `auth.toml`.
  Revocable per-user, never the whole population.
- **HTTP polling for v1; WebSocket as H7 polish.** Bot already runs
  aiohttp so `web.WebSocketResponse` is close; ship it only if polling
  proves laggy in practice.
- **Presence is push-only and lossy.** Launcher heartbeats every 30s;
  bot considers a row stale after 60s and the player is effectively
  offline. No eager deletes — staleness is a query filter.
- **Bot is the sole source of truth.** Launcher never persists friend
  lists or presence to local disk. Loss of connectivity = blank UI with
  "reconnecting…", not a stale cache that lies.
- **Friendships are stored once per pair, not twice.** Row convention
  `user_a < user_b`. Status enum carries direction for the pending
  states. Cuts storage in half; queries cheap on both sides because of
  symmetric indices.
- **Visibility settings are minimal.** `public` / `friends` / `hidden`.
  No allowlists, no per-friend overrides for v1 — those are the kind of
  knob nobody touches and would double the test surface.

---

## What's actually new

### Bot side (3 tables, 7 endpoints)

New tables in `chickenbot/database.py::init_tables`:

```sql
CREATE TABLE social_tokens (
    discord_id BIGINT PRIMARY KEY,
    token CHAR(64) NOT NULL UNIQUE,        -- 256-bit hex, generated server-side
    issued_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    last_used_at TIMESTAMP NULL,
    revoked BOOLEAN NOT NULL DEFAULT FALSE,
    INDEX idx_token (token)
);

CREATE TABLE friendships (
    user_a BIGINT NOT NULL,                -- always the smaller discord_id
    user_b BIGINT NOT NULL,                -- always the larger discord_id
    status ENUM(
        'pending_a_to_b', 'pending_b_to_a',
        'accepted',
        'blocked_by_a', 'blocked_by_b'
    ) NOT NULL,
    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP,
    PRIMARY KEY (user_a, user_b),
    INDEX idx_user_a (user_a, status),
    INDEX idx_user_b (user_b, status)
);

CREATE TABLE presence (
    discord_id BIGINT PRIMARY KEY,
    mc_uuid CHAR(36) NOT NULL,
    location ENUM('launcher', 'in_game') NOT NULL,
    server_addr VARCHAR(255) NULL,         -- "play.chickenedin.com:25565" or null
    screen VARCHAR(64) NULL,               -- "main_menu", "instances", etc.
    visibility ENUM('public', 'friends', 'hidden') DEFAULT 'friends',
    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP ON UPDATE CURRENT_TIMESTAMP
);
```

New endpoints in `chickenbot/api.py` (all Bearer-token authed against
either the existing `API_SECRET` for system calls or a per-user token
from `social_tokens` for client calls):

```
GET    /api/links/by-uuid?minecraft_uuid=<uuid>         (system)
       → { linked: bool, discord_id?: str, display_name?: str }

POST   /api/launcher/link                               (system)
       body: { discord_id, minecraft_uuid, link_code }
       → { social_token: "..." }
       ChickenLink generates a launcher-link code in-game alongside the
       Discord-link code; entering it in the launcher returns the token.

POST   /api/presence/heartbeat                          (user-token)
       body: { mc_uuid, location, server_addr?, screen?, visibility? }
       → { ok: true }

GET    /api/friends                                     (user-token)
       → { friends: [{ discord_id, mc_uuid, display_name,
                       presence: {…} | null }, …],
           incoming: […], outgoing: […] }

POST   /api/friends/request                             (user-token)
       body: { target_discord_id }   OR   { target_mc_name }
       → { status: 'pending' | 'already_friends' | 'already_requested' }

POST   /api/friends/respond                             (user-token)
       body: { request_from_discord_id, action: 'accept' | 'decline' }
       → { status: 'accepted' | 'declined' }

DELETE /api/friends/{discord_id}                        (user-token)
       → { status: 'removed' }
```

### Launcher side (one social module + UI hooks)

- New crate-level module `crates/ewo-launcher/src/social/` with
  `mod.rs`, `client.rs` (HTTP), `state.rs` (in-memory cache),
  `heartbeat.rs` (the 30s background thread).
- `SocialState` lives on `App` alongside `AuthService`. Drives:
  - Settings → Account tab gains a "ChickenedIn link" row showing linked
    status + a "Link launcher" button when the MC account is linked to
    Discord but the launcher itself doesn't yet have a `social_token`.
  - Main menu gains a live "ChickenedIn · X/Y online · TPS Z" widget
    (right column, below the SETTINGS link).
  - New `Screen::Friends` for the main launcher.
  - In-game overlay tab strip extends to HOME · HUD · **FRIENDS** ·
    MODULES · MODS · SETTINGS.

---

## Step plan

Each H-step is a working build. Build sequence:

### H0 — Foundations

- SSH key generated 2026-05-27 (✅ done): `~/.ssh/ewo_vps_claude`,
  alias `ssh ewo-vps` configured. Public key pending deploy to
  `gneef@77.90.29.221:~/.ssh/authorized_keys`.
- VPS hygiene (user does these once key is verified): rotate the
  password that was shared in chat, disable password auth in
  `/etc/ssh/sshd_config`.
- Add `EWO_BOT_API_BASE` to launcher settings (default the chickenedin
  production base — see [crates/ewo-launcher/src/main.rs](crates/ewo-launcher/src/main.rs) for the existing settings pattern).
  Plain HTTPS, ureq, no auth at this stage.
- Pick the in-game `/launcher-link` command name — proposal: in
  ChickenLink, `/launcher-link` mints a single-use 6-digit code, stored
  in a new `launcher_link_codes` table mirroring the existing
  `link_codes` pattern. 5 min TTL. The launcher trades the code for a
  `social_token` via `POST /api/launcher/link`.

**Verification: SSH alias works and lands at a shell as `gneef`.**

### H1 — Identity probe

- Add `/api/links/by-uuid` to the bot. **Public, rate-limited** (60
  req/min/IP, same as the `/api/levels/*` endpoints). Returns only
  `{ linked: bool }` — deliberately NOT the linked discord_id, since
  that's sensitive enough to require the per-user token from H2.
  Yes/no is all the launcher needs to gate the Friends UI.
- Launcher's `AuthService` calls it on successful MS sign-in. Caches a
  `LinkStatus` enum (`Unknown` / `Probing` / `Linked` / `NotLinked` /
  `ProbeFailed`) on a new `SocialState` held by `App`.
- Settings → Account tab renders linked status. No social UI yet.

**Why the design correction**: the original plan said "Bearer-authed
against `API_SECRET`", which was wrong — the launcher ships to end users
and can't safely embed the admin token. The clean fix is to keep the
endpoint public + drop the discord_id from the response. The discord_id
endpoint shape stays usable inside the bot for any future Bearer-authed
caller (just call `get_link_by_uuid` directly).

**Verification: sign in as Vwyla, see "Linked to Discord" on the Account
tab.**

### H2 — Per-user token + launcher-link flow

- ChickenLink plugin: new `/launcher-link` command. Generates a 6-digit
  code, posts to bot's new `POST /api/launcher-link-code` (mirror of
  `/api/link-code`), tells the player to enter it in the launcher.
- Bot: `POST /api/launcher/link` accepts `{ discord_id, minecraft_uuid,
  link_code }`. Validates the code, mints a 256-bit hex token, inserts
  into `social_tokens`, returns the token.
- Launcher: Settings → Account tab gains a "Link launcher" affordance
  visible when the MC account is linked but the launcher doesn't have
  a `social_token` saved. Modal with a 6-digit input.
- Tokens persist alongside MS auth in `auth.toml` (`AccountStore` gains
  `launcher_social_token: Option<String>` per account).
- New middleware on the bot: `check_user_token(request) -> Optional[int]`
  returns the discord_id if the bearer matches a live `social_tokens`
  row, else None. Future endpoints use this.

**Verification: in-game `/launcher-link` → copy code → paste in launcher
modal → "✓ launcher linked" status persists across launcher restarts.**

### H3 — Presence push

- Bot: `presence` table + `POST /api/presence/heartbeat` (user-token).
- Launcher: `heartbeat.rs` background thread, fires every 30s while
  the launcher has a valid `social_token`. Sends `{ mc_uuid, location:
  "launcher", screen: <current Screen as string>, visibility: <user
  setting> }`. Switches `location: "in_game", server_addr: <addr>` when
  the Launching screen handoffs to a real JVM spawn.
- Stale-presence query: `WHERE updated_at > NOW() - INTERVAL 60 SECOND`.
- Default visibility = `friends`.

**Verification: open the bot's Discord DM channel, write a bot debug
command that prints the launcher's presence row — see it update every
30s with the current screen.**

### H4 — Friend graph + endpoints

- Bot: `friendships` table + the four CRUD endpoints (`GET /api/friends`,
  `POST /api/friends/request`, `POST /api/friends/respond`, `DELETE
  /api/friends/{discord_id}`).
- Friend resolution by MC name: bot looks up MC UUID via Mojang's
  username → UUID API (with a 24h cache in a new tiny `mc_name_cache`
  table), then resolves UUID → linked Discord ID via `links`. If the
  target hasn't linked, 404. If they have, request lands.
- Friend list response joins `presence` so the launcher gets each
  friend's online/offline state in one call. Honors visibility:
  `friends` always visible to mutual friends; `hidden` always offline;
  `public` visible to anyone (relevant only for H6's join-by-name
  lookup).

**Verification: two test accounts linked, exchange a friend request via
the bot's existing Discord-bot commands (`/friend add <name>` mirrors
the launcher API), confirm `GET /api/friends` returns the mutual
friendship.**

### H5 — Friends UI (launcher + in-game)

- Launcher main menu: collapsible Friends sidebar in the right column.
  Each card: 8×8 head crop (rendered from the existing skin loader in
  [crates/ewo-jni/src/skin.rs](crates/ewo-jni/src/skin.rs) — that
  module's box-UV unwrap already isolates the head face), display name,
  status pill (`Offline` / `In launcher · Main menu` / `In-game · SMP`),
  and a Join button when applicable.
- Empty state when not linked → "Link your Discord on chickenedin.com"
  with a link to `https://chickenedin.com/dashboard`.
- New `Screen::Friends` for full friend management (add / remove /
  accept requests / search).
- In-game: HUD overlay grows a FRIENDS tab. Same data, same widgets,
  rendered into the existing 3-tab dashboard pattern from Phase E.
  Reuses `crates/ewo-jni/src/hud.rs` conventions.

**Verification: Vwyla and a friend both signed in, both see each other's
presence + status pill update within 30s of switching screens. Friend
shows up on both the launcher main menu and the in-game FRIENDS tab.**

### H6 — Live server-status widget + Roblox-style join

- Launcher main menu widget: poll `/api/server-status` every 15s when
  on the main menu screen. Render a Velvet card "ChickenedIn · X/Y
  online · TPS Z" with an avatar grid (max 8 visible, "+N more" overflow).
- Click the card → launches MC with `--server play.chickenedin.com:25565`.
  The launcher's `LaunchProfile` already accepts a `--server` arg;
  wire it through.
- Friend card "Join" button: if friend's presence has `server_addr`,
  click → spawn MC with `--server <addr>` against the friend's MC
  version (taken from presence — needs an extra field `mc_version` on
  the presence push from H3).
- Failure modes: server down (status endpoint 5xx) → render the widget
  as "ChickenedIn · offline"; join fails (server private, allowlist) →
  the JVM exits non-zero, the launcher's existing Launching error-state
  catches it.

**Verification: with chickenedin's lobby up, click the widget → MC
launches and connects automatically. Repeat for a friend's server.**

### H7 — WebSocket push (optional polish)

- New endpoint `WS /api/social/stream` (user-token in `Sec-WebSocket-Protocol`
  or first message). Bot pushes:
  - Friend request received
  - Friend accepted / declined
  - Friend's presence changed (online ↔ offline, screen switch,
    server change)
  - Friend removed
- Launcher: opens WS on first social activity, keeps open; falls back
  to polling on disconnect. Updates `SocialState` cache in place.
- Ship only if H3–H6 produce visible polling lag in practice. The 30s
  presence cadence is intentionally generous; the launcher main menu
  refresh is at 15s. Friend-list updates poll on Friends-tab focus.
  All three should feel live enough without WS.

**Verification: with WS enabled, friend's status updates appear within
~1s of their screen switch. Without WS, within 30s.**

---

## Out of scope for Phase H

Deferred to a hypothetical Phase I:

- **Messaging.** DMs would need a real-time channel + history sync +
  offline push + notification UX. Roughly doubles the surface area of
  Phase H. The natural home is to extend H7's WebSocket once it lands,
  but the design (storage schema, attachment handling, read receipts,
  notification batching) deserves its own pass.
- **Friend leaderboards.** The bot already has
  `/api/levels/leaderboard` and per-user XP. Wiring the launcher Friends
  tab to render a friends-filtered slice is ~20 lines once the
  friendship endpoints exist. Easy add-on; pulled out of Phase H to
  keep the scope honest.
- **Party / co-launch.** "Click 'invite to party' and we all launch
  into the same server" — appealing but needs a multi-client
  coordination layer the bot doesn't have today.
- **Voice.** The Simple Voice Chat mod is already bundled (see
  `crates/ewo-launcher/src/bundled.rs::CATALOG`). The launcher
  doesn't need anything extra for in-game voice. A *launcher* voice
  channel (lobby chat) is interesting but post-Phase H.
- **Cross-network friends / federation.** The launcher is tied to one
  bot (chickenedin's) in v1. Multi-network later, behind a clean
  "EwoNetwork" abstraction that lets a second VPS host a parallel
  social graph.

---

## Non-negotiables

- **Offline-first holds.** Signed-out launcher makes zero network calls.
  Signed-in but no Discord link → MS-auth calls only, no social calls.
  Linked launcher → social calls only when the user is interacting
  with social UI (heartbeat + visible-screen polling). No background
  telemetry, no analytics, no auto-update.
- **`API_SECRET` never ships in the launcher binary.** That's the
  website's shared admin token. The launcher uses per-user
  `social_token`s issued at link time.
- **Bot is the sole source of truth.** Launcher caches in memory only,
  never on disk. Stale cache UX is "reconnecting…", not lying.
- **Per-user tokens are revocable per row.** A leaked token affects one
  account, not the population.
- **Honor visibility every step.** `hidden` users never appear in a
  friend's presence even if the friend list query would otherwise
  include them. The bot enforces this — the launcher doesn't filter
  client-side.
- **No PII in launcher logs.** Discord IDs in URLs are fine; tokens in
  request headers are fine; user-visible launcher logs must not contain
  tokens, raw IDs, or join codes.

---

## Useful runtime conventions

- New env / settings keys (defaults match chickenedin production):
  - `EWO_BOT_API_BASE` → `https://chickenedin.com/api` (or wherever the
    bot is reverse-proxied; the bot itself listens on `:8080` behind
    nginx). Settings.toml entry: `[social] bot_api_base = "..."`.
- Per-user token persisted at `<config>/EwoClient/auth.toml` under each
  account: `accounts[<uuid>].social_token = "..."`. Plaintext for now
  (same caveat as MS refresh tokens — DPAPI / keychain before binary
  distribution).
- Bot endpoint conventions:
  - Bearer auth, JSON in / JSON out, `aiohttp` middleware existing
    pattern.
  - Errors: `{ "error": "kebab-case-reason" }` with appropriate status.
  - Rate limits: 120/min/user for presence (heartbeats are 2/min); 30/min/user
    for friend ops; 60/min/IP for public anonymous endpoints (matches
    the existing `/api/levels/*` rate-limit pattern in
    `api.py::_cors_middleware`).
- Schema migrations: Phase H tables added to the static `tables` list in
  `database.py::init_tables`. `CREATE TABLE IF NOT EXISTS` so production
  picks them up on next bot restart with no manual step.

---

## What NOT to do in Phase H

- **Don't extend the existing shared `BOT_API_TOKEN` to the launcher.**
  The website is a single deployment; the launcher is end-user software.
  Per-user tokens or nothing.
- **Don't build the friend list in a separate database** (e.g. add a
  Postgres alongside MySQL). The chickenedin stack is MySQL; new tables
  go next to `links` / `purchases` / `mod_cases`.
- **Don't store the social graph in the launcher.** Avoid the "what
  happens when the cache disagrees with the server" class of bug.
- **Don't add a launcher-side WebSocket library before H7 is justified.**
  Polling-first; promote to WS only when measured lag justifies it.
- **Don't expose `/api/server-status` rewrites — the endpoint exists,
  just consume it.**
- **Don't bypass the link gate.** Anyone who hasn't linked sees the
  link affordance, not the social UI. No "demo mode" with fake friends.

---

## Glossary (Phase H-specific terms)

- **Launcher-link code** — the 6-digit one-shot code minted in-game by
  `/launcher-link`, exchanged in the launcher for a `social_token`.
  Distinct from the existing Discord-link code (which maps Discord ↔ MC).
- **Social token** — 256-bit per-user bearer token persisted by the
  launcher, used as `Authorization: Bearer <social_token>` for all
  user-authed endpoints.
- **Presence** — the live row in `presence` describing where a player
  is right now: launcher screen or in-game server.
- **Heartbeat** — the launcher's 30s POST to `/api/presence/heartbeat`
  that keeps presence fresh.
- **Visibility** — per-user presence-broadcast setting:
  `public`/`friends`/`hidden`.
- **Roblox-style join** — clicking a friend or the live server-status
  widget spawns Minecraft with `--server <ip>:<port>`, joining the
  same world automatically.
