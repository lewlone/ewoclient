---
paths:
  - "crates/ewo-launcher/src/social/**"
  - "crates/ewo-render/src/screens/friends.rs"
  - "crates/ewo-render/src/screens/launcher_link_modal.rs"
  - "crates/ewo-jni/src/social.rs"
  - "docs/history/PHASE_H_PLAN.md"
---

# Social — friends, presence, launcher-link, join (Phase H)

Plugs the launcher into the user's own Minecraft network, **Frogsy** (formerly
"chickenedin"; the old `chickenedin.com` host no longer resolves, and anyone
re-registering it would have received linked users' tokens, so it is no longer
compiled in anywhere). Don't build a parallel social stack. Full history:
`docs/history/ewoclient-v2-phases.md` ("Phase H — Social") and
`docs/history/PHASE_H_PLAN.md` (partly stale: it predates the gating below).

## Off unless configured, silent unless linked

- **No default host.** The API base comes only from `EWO_BOT_API_BASE` and
  must be `https://` with no credentials, query or fragment; anything else
  logs a warning and disables social. Unset = no social request at all.
- **One gate**, `social::call_allowed`, sits in front of every request:
  nothing without a configured base, and nothing but the user-initiated
  launcher-link redemption (`POST /api/launcher/link`) without the active
  account's `social_token`. The heartbeat, the friends list, friend actions
  and the 15 s server-status poll all wait for a token.
- **A `social_token` is the proof of a link** (the bot mints `/launcher-link`
  codes only for Discord-linked accounts). The old `/api/links/by-uuid` probe,
  which sent every signed-in account's UUID before any opt-in, is removed, and
  `uuid_with_dashes` went with it. The Settings line derives from the token.
- A redeemed token lands on the account the code was submitted for, not
  whichever is active when the response arrives; redeeming while signed out
  fails locally.
- When disabled the UI degrades: the Settings link line is hidden, Friends
  shows a "no social server is configured" notice, and the main-menu network
  widget is hidden.

## Three repos, one contract

1. **Bot (Python)**: `C:/Users/valtteri/Desktop/FULLSTACK/chickenbot/`,
   `database.py` + `api.py`. System calls authenticate with `API_SECRET`
   (`check_auth`), launcher calls with a `social_tokens` bearer
   (`check_user_token`).
2. **ChickenLink (Paper plugin)**: `FULLSTACK/NETWORK/ChickenLink/`.
   `/launcher-link` mints a 6-digit code.
3. **Launcher**: `social/mod.rs` (HTTP + state machine), `screens/friends.rs`,
   `screens/launcher_link_modal.rs`.

(Repo and directory names predate the Frogsy rename; verify the deployed host
before pointing `EWO_BOT_API_BASE` at it.) nginx routes `/bot/api/*` to the
bot's `:8080`, which itself serves `/api/...`. Endpoints the launcher calls:

```
POST   /api/launcher/link                           PUBLIC  {code} → {social_token, discord_id}
POST   /api/presence/heartbeat                      user    {minecraft_uuid, location, screen?, server_addr?, visibility?}
GET    /api/friends                                 user    → {friends[], incoming[], outgoing[]}
POST   /api/friends/request                         user    {target_mc_name}
POST   /api/friends/respond                         user    {request_from_discord_id:int, action}
DELETE /api/friends/{discord_id}                    user    (id percent-encoded)
GET    /api/server-status                           PUBLIC  (POST stays system-authed)
```

**Contract gotchas:** `discord_id` is a **string** in `/api/friends` entries
(parsed with `.as_str()`; a number silently drops every friend) but a **number**
in `/respond`'s body. The heartbeat sends the account's UUID as the launcher
stores it (undashed); the old dashed form went with `uuid_with_dashes`, and the
bot side of that has not been re-checked.

## Friends screen behaviour

- The list refreshes on a 30 s idle cadence, **immediately on arriving at
  Friends** (tab bar or menu), after a friend action, and after a successful
  link redeem (only when the redeemed account is the active one, since the
  list cache belongs to it). All of these go through `call_allowed`.
- A settled friend-action toast (Done / Failed) stays `FRIEND_ACTION_TOAST_S`
  (6 s) and clears itself; Failed also logs a warning. It fades with the silk
  curve and moves by whole pixels (no text scaling).

## Join (H6)

`App::active_server` plus a shared `start_launch(idx, server, time)`. When a
server is set, `try_real_launch` appends `--quickPlayMultiplayer <addr>` (the
1.20+ replacement for `--server`/`--port`), and presence reports
`in_game · <addr>` while the game is alive. The main-menu network widget exists
only while social is configured and the active account is linked; it polls
every 15 s on the main menu only and shows a one-line strip of online player
names. Its JOIN target is `EWO_LOBBY_ADDR` (no default; unset = not joinable).

## Open

The live end-to-end test (sign in → `/launcher-link` → presence → join) has
never run. It needs the bot deployed and routed on the VPS under the Frogsy
domain. H7 (WebSocket push) is deliberately not done. The in-game FRIENDS tab
is a file bridge; see `ewo-ingame-hud.md`.
