---
paths:
  - "crates/ewo-launcher/src/social/**"
  - "crates/ewo-render/src/screens/friends.rs"
  - "crates/ewo-render/src/screens/launcher_link_modal.rs"
  - "crates/ewo-jni/src/social.rs"
  - "docs/history/PHASE_H_PLAN.md"
---

# Social — friends, presence, launcher-link, join (Phase H)

Plugs the launcher into the user's own Minecraft network (formerly
"chickenedin", **renamed Frogsy in mid-2026**; verify domains before relying on
any). Don't build a parallel social stack. Full history:
`docs/history/ewoclient-v2-phases.md` ("Phase H — Social") and
`docs/history/PHASE_H_PLAN.md` (partly stale). The API host `chickenedin.com`
no longer resolves (the network was renamed Frogsy); see `docs/REVIEW-2026-09.md`.

**Offline-first holds.** Signed out means zero network calls. Signed in but
unlinked means MS-auth calls only. Social calls start only once there's a
per-user `social_token`.

## Three repos, one contract

1. **Bot (Python)**: `C:/Users/valtteri/Desktop/FULLSTACK/chickenbot/`,
   `database.py` + `api.py`. System calls authenticate with `API_SECRET`
   (`check_auth`), launcher calls with a `social_tokens` bearer
   (`check_user_token`).
2. **ChickenLink (Paper plugin)**: `FULLSTACK/NETWORK/ChickenLink/`.
   `/launcher-link` mints a 6-digit code.
3. **Launcher**: `social/mod.rs` (HTTP + state machine), `screens/friends.rs`,
   `screens/launcher_link_modal.rs`.

Base URL `https://chickenedin.com/bot` (override `EWO_BOT_API_BASE`). nginx
routes `/bot/api/*` to the bot's `:8080`, which itself serves `/api/...`.

```
GET    /api/links/by-uuid?minecraft_uuid=<dashed>   PUBLIC  → {linked}
POST   /api/launcher-link-code                      system  {minecraft_uuid} → {code, expires_at}
POST   /api/launcher/link                           PUBLIC  {code} → {social_token, discord_id}
POST   /api/presence/heartbeat                      user    {minecraft_uuid, location, screen?, server_addr?, visibility?}
GET    /api/friends                                 user    → {friends[], incoming[], outgoing[]}
POST   /api/friends/request                         user    {target_mc_name}
POST   /api/friends/respond                         user    {request_from_discord_id:int, action}
DELETE /api/friends/{discord_id}                    user
GET    /api/server-status                           PUBLIC  (POST stays system-authed)
```

**Contract gotchas:** `discord_id` is a **string** in `/api/friends` entries
(parsed with `.as_str()`; a number silently drops every friend) but a **number**
in `/respond`'s body. UUIDs go **dashed** (`social::uuid_with_dashes`).

## Join (H6)

`App::active_server` plus a shared `start_launch(idx, server, time)`. When a
server is set, `try_real_launch` appends `--quickPlayMultiplayer <addr>` (the
1.20+ replacement for `--server`/`--port`), and presence reports
`in_game · <addr>` while the JVM is alive. The main-menu server widget polls
every 15 s on the main menu only.

## Open

The live end-to-end test (sign in → `/launcher-link` → presence → join) has
never run. It needs the bot deployed and routed on the VPS. H7 (WebSocket push)
is deliberately not done. The in-game FRIENDS tab is a file bridge; see
`ewo-ingame-hud.md`.
