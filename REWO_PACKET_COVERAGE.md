# Rewo clientbound-play packet coverage — what the server sends that Rewo ignores

Audit date **2026-07-29** (M67), **re-derived 2026-07-29** (M74), against
**26.2 / protocol 776**. Ground truth is the bundled datagen report
(`%APPDATA%/EwoClient/rewo/26.2/datagen/generated/reports/packets.json`) — the
same file `crates/rewo-net/src/ids.rs` resolves against — plus the Vineflower
decompile beside it. Nothing here is inferred from a wiki.

**Why this exists.** Rewo's packet handling grew milestone by milestone, so
what it decodes is a *historical accident, not a decision*. Twice a whole
family turned out to be simply absent and was found by noticing it was not in
`ids.rs` at all — the sound packets (M63) and the scoreboard / boss-bar /
tab-list set (M65). This is the enumeration that makes that failure mode
impossible to repeat: every clientbound-play packet in the report appears in
§5 with a status.

> ## ⚠ This table is machine-checked. Edit it when you edit `ids.rs`.
>
> `ids::tests::the_coverage_table_matches_the_code` (in
> `crates/rewo-net/src/ids.rs`) recomputes §2's counts and every §5 status
> from the source and fails `cargo test -p rewo-net` when they disagree. It
> exists because **the M67 audit went stale within hours of being written** —
> see §8. If you resolve a new packet, flip its §5 row to `handled` and bump
> §2; the test will tell you precisely which rows are wrong.

---

## §0 Handoff — the eight things worth knowing

1. **141 clientbound-play packets. §2 has the live counts** — the machine check
   recomputes them, so a number quoted in prose here goes stale and a number in
   §2 cannot. No packet is resolved-but-ignored: the `cb_play_*` field set and
   the dispatch chain agree exactly, which is a real (and slightly surprising)
   property of this codebase — see §1.
2. **The gaps carry one class each** — pure state, needs-rendering,
   needs-a-missing-subsystem, not-applicable — and §2 counts them. The
   pure-state ones are decodable and headlessly gateable *today*, with no
   renderer and no design decision: the same test the M52–M76 batches were
   chosen by.
3. **The hand-maintained version of this document decayed at the rate the
   codebase changed.** M67 wrote it by grepping; four packets landed in
   `ids.rs` the same day, three of them from M68. By the time M74 re-derived
   it, **ten of the 141 rows were wrong** — all in the same direction, all
   saying `absent` about code that was present. §8 has the mechanism and the
   fix, and the fix is the machine check above, not vigilance.
4. **`bundle_delimiter` (0) is the sharpest remaining gap**, because its
   failure mode is a rendering glitch rather than a protocol error. Vanilla
   applies everything between two delimiters in **one** tick; Rewo applies each
   packet as it drains, so an `add_entity` and its `set_entity_data` can land
   a frame apart and a mob renders for one frame with default metadata. §3.
5. **The positional / rotational teleport asymmetry is closed (M76).**
   `player_position` (72) had worked since M3 while `player_rotation` (73) and
   `player_look_at` (71) were never resolved, so a server turning your head did
   nothing at all — and the working half misdirected the diagnosis. All three
   are handled; §3 keeps the entry as the worked example of a gap whose failure
   mode is *silence in one direction only*.
6. **`hurt_animation` (42) is the input `M52a`'s vacuous `no_damage_tilt`
   module has nothing to disable.** The Velvet-batch note "to port the disable
   you must first build the thing being disabled" has a packet behind it.
7. **"Handled" is not "complete."** Six currently-handled packets decode only
   the part a milestone needed — §4 names them, because a partially-consumed
   packet looks identical to a fully-consumed one from `ids.rs`, and the
   machine check in §1 cannot see the difference either.
8. **`player_abilities` (64) is claimed.** A concurrent milestone is landing
   it together with the flight / no-clip physics it feeds (§4.1). It is listed
   `absent` here because it was absent when this was re-derived; whoever lands
   it flips the row.

---

## §1 Method, and what "handled" means here

Three questions, asked separately, because conflating them is exactly how M63
and M65 stayed hidden:

1. **Is it in the report?** All 141 rows below come from `packets.json`'s
   `play.clientbound` table, sorted by `protocol_id`. Rewo resolves ids **by
   name**, so the id column is informational — a renumber is not a gap.
2. **Does `ids.rs` resolve it?** Parsed mechanically out of the `resolve`
   block: every `cb_play_*: req!(p, P, C, "<name>")` / `opt!(…)` entry.
3. **Does anything outside `ids.rs` dispatch on that field?** M67 asked this
   as a word-boundary grep across all of `crates/`. M74 asked it more
   strictly: does the field appear in an `==` / `!=` comparison, or as a
   dispatch-table field, **inside `rewo-net`**? The looser question counts a
   field a gate merely mentions; the stricter one counts only fields an
   incoming packet id is actually tested against.

**Both instruments give the same answer, and it is a negative finding: the
resolved-but-unreferenced set is empty.** Every one of the 72 resolved ids
reaches a dispatch arm in `play.rs` or a `route_*` in `lib.rs`. So the gap is
entirely in question (2) — 69 names that were never resolved.

That is still a coarser instrument than it sounds, and §4 is the correction:
a dispatch arm proves a field is *tested*, not that the body is fully consumed.

### The machine check, and the grammar it needs

`ids::tests::the_coverage_table_matches_the_code` re-runs questions (2) and
(3) against this document at every `cargo test -p rewo-net`. It reads three
files at compile time via `include_str!` — this document, `ids.rs` and the
dispatch chain — so it needs no datagen report, no network and no cwd, and it
cannot silently skip.

It asserts:

- §5 has exactly 141 rows, with ids `0..=140`, contiguous and unique.
- Every §5 row's status is `handled` **iff** the packet is resolved in
  `ids.rs` **and** dispatched in `rewo-net`.
- Every name resolved in `ids.rs` appears as a §5 row — so adding a packet
  without adding a row fails, which is the case that actually happened.
- Every `absent` row carries a class letter.
- §2's four status counts and four class counts equal the computed tally.

It therefore requires §5's rows to keep this shape, and §2's tables to keep
their labels:

```
| <id> | `<packet name>` | handled | <resolution> | <note> |
| <id> | `<packet name>` | absent  | **<A|B|C|D>** | <note> |
```

**A milestone marker is not a status.** The M67 table wrote `**M69**` in the
status column for four rows that had landed after the snapshot, which is what
made it unparseable and is half of why the drift went unnoticed. Attribution
belongs in the note column; the status column has two values.

### Classification

Each of the 69 gaps carries one class. The classes are about **what it would
take**, not about how much anyone wants it:

| Class | Meaning |
|---|---|
| **A** — pure state | Decoding it changes a value Rewo could act or gate on **without drawing anything new**. A witness can prove the decode; no human has to look at it. This is the class the M52–M74 batches drew from. |
| **B** — needs rendering | The decode is possible today, but the packet's purpose is a visual Rewo does not have (a title overlay, a world border, an XP bar). Landing the *feature* needs an eyeball; landing the *decode* does not. |
| **C** — needs a subsystem Rewo lacks | A screen/menu framework, a chat-input path, a recipe book, a resource-pack fetcher, a map image pipeline, a reconnect flow. The decode is not the hard part and shipping it alone buys nothing. |
| **D** — not applicable | Debug/dev tooling, integrated-server-only warnings, or a reply to a serverbound request Rewo never sends. Each row states which. |

The A/B line is drawn at *what the decode itself unlocks*, so a packet whose
state feeds physics (`explode`) is A even though explosions also have
particles, and a packet whose only consumer is a texture (`block_destruction`)
is B even though its body is three fields.

### Codec resolvability

Every codec below was located in the decompile. Spot checks confirmed the
dispatch registries the awkward ones need are present in `registries.json`:
`minecraft:stat_type` (9), `minecraft:custom_stat` (77),
`minecraft:debug_subscription` (16), `minecraft:recipe_display` (5),
`minecraft:menu` (25). **One row is marked not-verified**: `show_dialog` (140)
takes a `Holder<Dialog>` over the **datapack** `minecraft:dialog` registry plus
`Dialog`'s full codec tree; that is resolvable in principle and was not walked
here. No packet was found whose codec is unresolvable from the report plus the
decompile.

---

## §2 The counts

Machine-checked — see §1. Change these together with §5 or the test fails.

| Status | Count |
|---|---|
| Resolved **and** consumed | **76** |
| Resolved but ignored | **0** |
| Not resolved at all | **65** |
| **Total clientbound-play** | **141** |

The 65 gaps, by class:

| Class | Count | Share of the gap |
|---|---|---|
| **A** pure state, no rendering | **11** | 17% |
| **B** needs rendering | **20** | 31% |
| **C** needs a subsystem Rewo lacks | **23** | 35% |
| **D** not applicable | **11** | 17% |

M67 audited 56 / 0 / 85 with class A at 31. Sixteen packets separate that
published 56 from this 72, and **ten of them had already landed when M67
published** (§8) — three from M67's own sibling work, three from M68, three
from M69, and `set_passengers` from the M68/M70/M72 trio. The other six are
M74's.

---

## §3 The three most valuable gaps

Ranked by *how badly the failure hides*, not by how visible the feature is.
M67's original top three — the velocity/mount cluster, the inventory's
authoritative writes, and `update_tags` — are all closed, by M68, M69 and M69
respectively. These are what replaced them.

### 1. `bundle_delimiter` (0) — the one whose failure looks like a render bug

Empty body, and the cheapest row in the table. `ClientboundBundlePacket` wraps
a run of packets between two delimiters and vanilla applies **the whole run in
one tick**; the delimiter is how the client knows where the run ends.

Rewo applies each packet as it drains the socket. The server bundles precisely
the things that are wrong apart: an `add_entity` with its `set_entity_data`,
its `set_equipment` and its `update_attributes`. Split across frames, a mob
renders for a frame or two as an un-named, un-equipped, default-metadata
version of itself — a slime at size 1, an armour stand with no armour, a
zombie with no held item — and then pops into correctness.

That is indistinguishable from a renderer bug, which is what makes it worth
ranking first: every other gap in this section announces itself as *nothing
happening*, and this one announces itself as the wrong subsystem.

### 2. `player_rotation` (73) and `player_look_at` (71) — **closed, M76**

Kept because the *shape* of the failure is the lesson, and because two of the
three things this section said about them were wrong.

`player_position` (72) was handled and its rotational twin was not, so a server
that moved your body worked and one that turned your head did nothing. The
failure was silent and one-sided, and the working half is what made it hard to
find: the natural diagnosis is "teleports work, so it isn't the teleport path".

**What this section got wrong.** It said `handleRotatePlayer` sets yaw and pitch
"with per-axis relative flags, the same `RelativeMovement` set the positional
teleport uses". The *semantics* are shared — `handleRotatePlayer` builds a
`Set<Relative>` and calls the same `calculateAbsolute` — but the **wire layout
is not**: `ClientboundPlayerRotationPacket` is `FLOAT, BOOL, FLOAT, BOOL`,
ten fixed bytes with each flag *after* the float it qualifies, and carries no
packed mask at all. A reader written from this paragraph would have consumed
the yaw's four bytes as the mask. The handler is also named `handleLookAt`, not
`handlePlayerLookAt`.

Both were class A and both were left alone by M74 because they write
`rewo_world::physics::PlayerState`, which M75 owned while it landed flight and
no-clip (§4.1). M75 landed; M76 took them.

### 3. `disguised_chat` (33) — chat that is simply never shown

`Component` plus a `ChatType.Bound`, and Rewo already renders chat lines, so
this is decode-and-append with no new subsystem. What sends it is
`/msg`-style and plugin-routed chat — `disguised_chat` is what a server uses
when a message should render as chat without carrying a player signature.

So on a plugin-driven server an entire category of message never appears, with
no error, no log line and nothing missing from the screen to point at. It ranks
here for the same reason M67 ranked `update_tags` first: the client is not
broken in any way an observer can name.

**Honourable mention, for a different reason:** `store_cookie` (120) is the
other half of a loop Rewo already answers. `cookie_request` is resolved and
replied to — with nothing, because nothing ever stores a cookie. On a network
that uses `transfer` between backends that is a session that silently forgets
itself on every hop.

---

## §4 "Handled" is not "complete"

`ids.rs` cannot express *partially*, and neither can the machine check in §1 —
it proves a dispatch arm exists, not that the arm reads the whole body. These
are the known partials, found by reading the arms. Listed because a future
audit run by the same instruments will call every one of them handled.

**`game_event` closed in M71** and **`chunk_batch_finished` closed in M74**;
both are kept below as worked examples of the class, because the "what is not"
column is what turned each of them into a milestone.

| Packet | What is consumed | What is not |
|---|---|---|
| `game_event` (38) — **closed, M71** | All fourteen types, via `rewo_net::game_event`. Ten are applied: the four weather levels, `CHANGE_GAME_MODE` → `ClientGameState::game_mode`, the `IMMEDIATE_RESPAWN` / `LIMITED_CRAFTING` flags, `WIN_GAME` / `DEMO_EVENT` / `LEVEL_CHUNKS_LOAD_START` as markers, `NO_RESPAWN_BLOCK_AVAILABLE` as a queued translation key, and the three local sounds. | **`GUARDIAN_ELDER_EFFECT`'s particle** — `ParticleTypes.ELDER_GUARDIAN` is not one of M37's six transcribed kinds, and M37's rule is that an unknown kind is dropped rather than rendered as something else. The gamemode is **modelled, not acted on** (§4.1). `WIN_GAME` and `DEMO_EVENT` open screens Rewo has no screen system for. |
| `chunk_batch_finished` (11) — **closed, M74** | The `batchSize` **VarInt**, folded into `ChunkBatchSizeCalculator` along with `chunk_batch_start`'s clock stamp, and the reply is now `getDesiredChunksPerTick()`. | Nothing. Recorded because the row it used to have was **wrong in two ways**: it called the unread field "the `batchSize` float" (it is a VarInt — the *float* in this exchange is the serverbound reply's `desiredChunksPerTick`), and it classed a hard-coded `p.f32(64.0)` as an unread field when it was a **live flow-control divergence**: vanilla's seeded opening bid is `3.5`, so Rewo over-bid the server ~18× on every batch of every session and never adapted. See §8. |
| `explode` (36) | The physics prefix — position and the `Optional<Vec3> playerKnockback` that M68 applies to the local player. | The particle / sound / weighted-block-list tail, deliberately: the particles need M37 kinds Rewo has not transcribed and the sound needs playback, which M63 scoped out. `crate::motion::read_explode` returns how many bytes it used precisely so this stays honest. |
| `level_event` (46) | The particle half, through M37's `route_level_event`. | The sound half of the same id table — deliberately, per M63: playback, not decode. |
| `block_changed_ack` (4) | The id. The arm is a `log::debug!`. | The sequence number and the block-prediction rollback it acknowledges — Rewo does not predict block changes, so there is nothing to roll back yet. |
| `container_set_content` / `container_set_slot` (18 / 20) | Container id **0** — the player's own inventory. `apply_container_set_content` returns early on any other id. | Every other container id, dropped whole (M34's documented choice: there is no screen to put them in). `set_player_inventory` (108) and `set_cursor_item` (96) share the rule. |
| `player_info_update` (70) | `ADD_PLAYER`, `UPDATE_GAME_MODE`, `UPDATE_LISTED`, `UPDATE_LATENCY`, `UPDATE_LIST_ORDER`, `UPDATE_HAT`, and the walk past the rest. | `UPDATE_DISPLAY_NAME` and `INITIALIZE_CHAT` are walked and discarded rather than stored. The walk is correct — M62 unified it into one function after finding a drifted copy — but the values do not survive it. |

### §4.1 What wiring the gamemode to physics would actually take

M71 models `CHANGE_GAME_MODE` and stops there, because acting on it is a
larger job than the packet suggests. `MultiPlayerGameMode.setLocalMode` ends in
`GameType.updatePlayerAbilities(abilities)`, which writes four booleans —
`mayfly`, `instabuild`, `invulnerable`, `flying` (and note **SPECTATOR sets
`flying = true` while CREATIVE only sets `mayfly`**, so entering creative does
not start you flying). Rewo has **none of those concepts**. The work is:

1. An abilities struct on `PlayerState`, and `GameType::updatePlayerAbilities`
   transcribed onto it.
2. A flight branch in `rewo_world::physics::tick` — vanilla's creative flight
   is its own velocity model, not gravity with a different constant.
3. Spectator no-clip: `physics` currently always consults `baked.solid`.
4. `player_abilities` (clientbound **and** serverbound) — neither is in
   `ids.rs`. The server is authoritative about `mayfly`, and the client must
   send its flying state back or the server rubber-bands it.

**All four shipped in M75**, which is why M74 left `player_rotation` and
`player_look_at` alone despite ranking them second in §3: they write the same
`PlayerState`. **M76 took them once M75 landed.**

Two other authoritative sources also feed this state and are not wired: the
**login packet** carries `showDeathScreen` and `doLimitedCrafting` (the
`game_event` ids 11/12 are only the mid-session gamerule change), and
**`spawn_info`** carries `gameType` + `previousGameType` on both login and
respawn — `crates/rewo-net/src/spawn_info.rs` already decodes both fields and
nothing consumes them. `handleRespawn` also copies `showDeathScreen` onto the
new player but **not** `doLimitedCrafting`, so that one resets in vanilla too.

---

## §5 The full table

`handled` = resolved in `ids.rs` **and** dispatched. `absent` = not in
`ids.rs`, with its class from §1. Machine-checked — see §1 for the grammar.

| id | packet | status | resolution / class | note |
|---:|---|---|---|---|
| 0 | `bundle_delimiter` | absent | **A** | Empty body. Delimits a `ClientboundBundlePacket` — vanilla applies everything between two delimiters in one tick. Rewo applies each packet as it drains, so a spawn and its metadata can land a frame apart. §3. |
| 1 | `add_entity` | handled | `req!` → `cb_play_add_entity` | |
| 2 | `animate` | handled | `req!` → `cb_play_animate` | M19/M20 — combat swings. |
| 3 | `award_stats` | absent | **B** | Statistics screen. `Stat.STREAM_CODEC` dispatches on `minecraft:stat_type` (9) then the per-type value registry — both in `registries.json`. |
| 4 | `block_changed_ack` | handled | `opt!` → `cb_play_block_ack` | §4 partial — the arm is a log line. |
| 5 | `block_destruction` | absent | **B** | The crack overlay for a block someone else is mining. |
| 6 | `block_entity_data` | handled | `req!` → `cb_play_block_entity_data` | |
| 7 | `block_event` | handled | `req!` → `cb_play_block_event` | |
| 8 | `block_update` | handled | `req!` → `cb_play_block_update` | |
| 9 | `boss_event` | handled | `req!` → `cb_play_boss_event` | |
| 10 | `change_difficulty` | handled | `req!` → `cb_play_change_difficulty` | **M74.** A VarInt `Difficulty` id through a **WRAP** out-of-bounds map — a third enum convention, neither `readEnum` nor `ByIdMap ZERO` — then a bool. |
| 11 | `chunk_batch_finished` | handled | `req!` → `cb_play_chunk_batch_finished` | **M74** consumed the `batchSize` VarInt and fixed the reply. §4. |
| 12 | `chunk_batch_start` | handled | `req!` → `cb_play_chunk_batch_start` | **M74.** Empty body. Stamps the clock the batch-size calculator measures against; without it the reply is a constant, not an estimate. |
| 13 | `chunks_biomes` | handled | `opt!` → `cb_play_chunks_biomes` | |
| 14 | `clear_titles` | absent | **B** | Title overlay. |
| 15 | `command_suggestions` | absent | **C** | Chat/command input. |
| 16 | `commands` | absent | **C** | The Brigadier command tree. Worthless without command input. |
| 17 | `container_close` | handled | `req!` → `cb_play_container_close` | **M74.** One VarInt container id, which vanilla reads and then **ignores** — `handleContainerClose` closes whatever is open without comparing ids. |
| 18 | `container_set_content` | handled | `req!` → `cb_play_container_set_content` | §4 partial — container id 0 only. |
| 19 | `container_set_data` | absent | **C** | Furnace/brewing/enchanting progress — needs the non-player menus M34 excluded. |
| 20 | `container_set_slot` | handled | `req!` → `cb_play_container_set_slot` | §4 partial — container id 0 only. |
| 21 | `cookie_request` | handled | `opt!` → `cb_play_cookie_request` | Answered with nothing; see `store_cookie` (120). |
| 22 | `cooldown` | absent | **B** | The hotbar cooldown sweep. |
| 23 | `custom_chat_completions` | absent | **C** | Chat input autocomplete. |
| 24 | `custom_payload` | absent | **A** | `Identifier` + the rest of the body. `minecraft:brand` is the one every server sends. |
| 25 | `damage_event` | handled | `req!` → `cb_play_damage_event` | |
| 26 | `debug/block_value` | absent | **D** | Debug subscription — sent only to a client that subscribed via `debug_subscription`, which Rewo never will. |
| 27 | `debug/chunk_value` | absent | **D** | As above. |
| 28 | `debug/entity_value` | absent | **D** | As above. |
| 29 | `debug/event` | absent | **D** | As above. |
| 30 | `debug_sample` | absent | **D** | Remote tick/ping profiler samples for vanilla's F3 chart; requires a serverbound subscription first. |
| 31 | `delete_chat` | absent | **C** | Needs the signature-keyed chat history. Rewo renders chat lines and keeps no message store to delete from. |
| 32 | `disconnect` | handled | `req!` → `cb_play_disconnect` | |
| 33 | `disguised_chat` | absent | **A** | `Component` + `ChatType.Bound`. The chat overlay already exists, so this is decode-and-append. §3. |
| 34 | `entity_event` | handled | `req!` → `cb_play_entity_event` | M17. |
| 35 | `entity_position_sync` | handled | `req!` → `cb_play_entity_position_sync` | |
| 36 | `explode` | handled | `req!` → `cb_play_explode` | **M68.** §4 partial — the physics prefix only; the particle/sound/weighted-list tail is deliberately unconsumed. |
| 37 | `forget_level_chunk` | handled | `req!` → `cb_play_forget_chunk` | |
| 38 | `game_event` | handled | `req!` → `cb_play_game_event` | All 14 types since M71. Was 4 of 14 — the §4 worked example. |
| 39 | `game_rule_values` | absent | **A** | `Map<ResourceKey<GameRule>, String>`. |
| 40 | `game_test_highlight_pos` | absent | **D** | Game-test tooling. |
| 41 | `mount_screen_open` | absent | **C** | Horse/nautilus inventory screen. |
| 42 | `hurt_animation` | absent | **B** | The damage camera yaw-tilt. **This is the input `M52a`'s vacuous `no_damage_tilt` module has nothing to disable.** |
| 43 | `initialize_border` | absent | **B** | World border. |
| 44 | `keep_alive` | handled | `req!` → `cb_play_keep_alive` | |
| 45 | `level_chunk_with_light` | handled | `req!` → `cb_play_level_chunk` | |
| 46 | `level_event` | handled | `opt!` → `cb_play_level_event` | §4 partial — the particle half only. |
| 47 | `level_particles` | handled | `opt!` → `cb_play_level_particles` | M37. |
| 48 | `light_update` | handled | `opt!` → `cb_play_light_update` | |
| 49 | `login` | handled | `req!` → `cb_play_login` | |
| 50 | `low_disk_space_warning` | absent | **D** | `Minecraft.sendLowDiskSpaceWarning` — the integrated server warning about its own save directory. |
| 51 | `map_item_data` | absent | **C** | Map-item colour patches + decorations; needs a map image pipeline and a map renderer. |
| 52 | `merchant_offers` | absent | **C** | Villager trade screen. |
| 53 | `move_entity_pos` | handled | `req!` → `cb_play_move_entity_pos` | |
| 54 | `move_entity_pos_rot` | handled | `req!` → `cb_play_move_entity_pos_rot` | |
| 55 | `move_minecart_along_track` | absent | **A** | A list of interpolation steps for one minecart — entity movement state. |
| 56 | `move_entity_rot` | handled | `req!` → `cb_play_move_entity_rot` | |
| 57 | `move_vehicle` | handled | `req!` → `cb_play_move_vehicle` | **M68.** Carries **no entity id** — the client resolves `getRootVehicle()`. Sent only as a rejection of a serverbound vehicle move, so a passenger-only client never receives one. |
| 58 | `open_book` | absent | **C** | Book screen. |
| 59 | `open_screen` | absent | **C** | The menu framework — `minecraft:menu` registry + a screen per type. |
| 60 | `open_sign_editor` | absent | **C** | Sign edit screen. |
| 61 | `ping` | handled | `req!` → `cb_play_ping` | |
| 62 | `pong_response` | absent | **D** | The reply to a serverbound `ping_request` Rewo never sends (`pingDebugMonitor`). |
| 63 | `place_ghost_recipe` | absent | **C** | Recipe book. |
| 64 | `player_abilities` | handled | — | Flags byte + `flyingSpeed` + `walkingSpeed`, nine fixed bytes. Landed by **M75** with the flight / no-clip physics it feeds and the `GameType` binding M71 left unstarted. The **serverbound** twin is one byte carrying only `FLAG_FLYING` — writing the clientbound body there desyncs the stream by eight. |
| 65 | `player_chat` | handled | `opt!` → `cb_play_player_chat` | |
| 66 | `player_combat_end` | absent | **A** | Vestigial — vanilla's handler is an empty method. |
| 67 | `player_combat_enter` | absent | **A** | Vestigial, empty body, empty handler. |
| 68 | `player_combat_kill` | absent | **B** | The death screen. |
| 69 | `player_info_remove` | handled | `req!` → `cb_play_player_info_remove` | |
| 70 | `player_info_update` | handled | `req!` → `cb_play_player_info_update` | §4 partial — display name and chat session are walked and discarded. |
| 71 | `player_look_at` | handled | `req!` → `cb_play_player_look_at` | **M76.** `/teleport … facing`. An anchor `readEnum`, three doubles, a flag, and **only then** a conditional entity + anchor pair. An unresolvable entity falls back to the packet's own coordinates, which are the sender's snapshot of `toAnchor.apply(entity)` — not a placeholder. |
| 72 | `player_position` | handled | `req!` → `cb_play_position` | The positional teleport. Its rotational twin (73) landed in **M76**, closing §3's asymmetry. |
| 73 | `player_rotation` | handled | `req!` → `cb_play_player_rotation` | **M76.** Ten fixed bytes: `FLOAT yRot, BOOL relativeY, FLOAT xRot, BOOL relativeX` — **two interleaved booleans, not** the packed `Relative` mask 72 carries. It is the only one of the two that answers the server. |
| 74 | `recipe_book_add` | absent | **C** | Recipe book. |
| 75 | `recipe_book_remove` | absent | **C** | Recipe book. |
| 76 | `recipe_book_settings` | absent | **C** | Recipe book. |
| 77 | `remove_entities` | handled | `req!` → `cb_play_remove_entities` | |
| 78 | `remove_mob_effect` | handled | `req!` → `cb_play_remove_mob_effect` | |
| 79 | `reset_score` | handled | `req!` → `cb_play_reset_score` | M65. |
| 80 | `resource_pack_pop` | absent | **C** | Server resource-pack pipeline (download, prompt, apply). Rewo loads a CEM pack from disk; it fetches none. |
| 81 | `resource_pack_push` | absent | **C** | As above. |
| 82 | `respawn` | handled | `req!` → `cb_play_respawn` | |
| 83 | `rotate_head` | handled | `req!` → `cb_play_rotate_head` | |
| 84 | `section_blocks_update` | handled | `req!` → `cb_play_section_blocks_update` | |
| 85 | `select_advancements_tab` | absent | **C** | Advancements screen. |
| 86 | `server_data` | absent | **A** | MOTD `Component` + `Optional<byte[]>` icon. |
| 87 | `set_action_bar_text` | absent | **B** | Action-bar overlay. |
| 88 | `set_border_center` | absent | **B** | World border. |
| 89 | `set_border_lerp_size` | absent | **B** | World border. |
| 90 | `set_border_size` | absent | **B** | World border. |
| 91 | `set_border_warning_delay` | absent | **B** | World border. |
| 92 | `set_border_warning_distance` | absent | **B** | World border. |
| 93 | `set_camera` | handled | `req!` → `cb_play_set_camera` | **M74.** One VarInt entity id; an id the client cannot resolve leaves the camera **where it was**. Feeds `LabelViewer::camera_entity`, which M70 had hard-wired to the local player. |
| 94 | `set_chunk_cache_center` | handled | `req!` → `cb_play_set_chunk_cache_center` | M67. |
| 95 | `set_chunk_cache_radius` | handled | `req!` → `cb_play_set_chunk_cache_radius` | M67. |
| 96 | `set_cursor_item` | handled | `req!` → `cb_play_set_cursor_item` | **M69.** The server's authoritative carried stack — M35's predicted cursor had no other correction path short of a full resync. |
| 97 | `set_default_spawn_position` | handled | `req!` → `cb_play_set_default_spawn_position` | **M76.** `LevelData.RespawnData` = a dimension **identifier string** + a packed `BlockPos` long + two floats. Stored verbatim: `STREAM_CODEC` does not apply `RespawnData.of`'s wrap/clamp. A dimension change **resets** it to `(8, 64, 8)` of the new level — the opposite of the difficulty beside it, which `handleRespawn` copies across. |
| 98 | `set_display_objective` | handled | `req!` → `cb_play_set_display_objective` | M65. |
| 99 | `set_entity_data` | handled | `req!` → `cb_play_set_entity_data` | |
| 100 | `set_entity_link` | absent | **A** | Leash holder id. Rendering the rope is separate (B). |
| 101 | `set_entity_motion` | handled | `req!` → `cb_play_set_entity_motion` | **M68.** The body is `Vec3.LP_STREAM_CODEC` (`LpVec3`), **not** the legacy `short / 8000.0` fixed point, which no longer exists in 26.2. |
| 102 | `set_equipment` | handled | `req!` → `cb_play_set_equipment` | |
| 103 | `set_experience` | absent | **B** | XP bar and level number. |
| 104 | `set_health` | handled | `opt!` → `cb_play_set_health` | |
| 105 | `set_held_slot` | handled | `req!` → `cb_play_set_held_slot` | |
| 106 | `set_objective` | handled | `req!` → `cb_play_set_objective` | M65. |
| 107 | `set_passengers` | handled | `req!` → `cb_play_set_passengers` | **M68 + M70 + M72** landed disjoint halves and the packet is now fully consumed: the riding graph for `isVehicle()`, the local player's mount state for physics, and every rider's position from its vehicle's PASSENGER attachment point. Remaining, and not about this packet: a mounted humanoid's seated leg pose, and the camel/horse animation-driven seat offsets. |
| 108 | `set_player_inventory` | handled | `req!` → `cb_play_set_player_inventory` | **M69.** An authoritative write addressed by **inventory index**, not menu slot — and the two armour ranges run in opposite directions, so a constant offset puts boots on the head. |
| 109 | `set_player_team` | handled | `req!` → `cb_play_set_player_team` | M62. |
| 110 | `set_score` | handled | `req!` → `cb_play_set_score` | M65. |
| 111 | `set_simulation_distance` | handled | `req!` → `cb_play_set_simulation_distance` | M67. |
| 112 | `set_subtitle_text` | absent | **B** | Title overlay. |
| 113 | `set_time` | handled | `req!` → `cb_play_set_time` | |
| 114 | `set_title_text` | absent | **B** | Title overlay. |
| 115 | `set_titles_animation` | absent | **B** | Title overlay timings. |
| 116 | `sound_entity` | handled | `req!` → `cb_play_sound_entity` | M63 — decode only, no playback. |
| 117 | `sound` | handled | `req!` → `cb_play_sound` | M63 — decode only, no playback. |
| 118 | `start_configuration` | handled | `opt!` → `cb_play_start_configuration` | |
| 119 | `stop_sound` | handled | `req!` → `cb_play_stop_sound` | M63. |
| 120 | `store_cookie` | absent | **A** | The cookie store Rewo already answers `cookie_request` from — with nothing, because nothing ever stores one. §3. |
| 121 | `system_chat` | handled | `opt!` → `cb_play_system_chat` | |
| 122 | `tab_list` | handled | `req!` → `cb_play_tab_list` | M65. |
| 123 | `tag_query` | absent | **D** | The reply to a serverbound `/data get` query Rewo never sends. |
| 124 | `take_item_entity` | absent | **B** | The pickup animation (the item flies to its collector). The removal itself already arrives via `remove_entities`. |
| 125 | `teleport_entity` | handled | `req!` → `cb_play_teleport_entity` | |
| 126 | `test_instance_block_status` | absent | **D** | Game-test tooling. |
| 127 | `ticking_state` | handled | `req!` → `cb_play_ticking_state` | **M74.** An f32 `tickRate` then a bool `isFrozen`. Decode and state only — the 20 Hz loop does not consult it yet. |
| 128 | `ticking_step` | handled | `req!` → `cb_play_ticking_step` | **M74.** One VarInt. |
| 129 | `transfer` | absent | **C** | Reconnect to another host — needs a transfer/reconnect flow. |
| 130 | `update_advancements` | absent | **C** | Advancements screen. |
| 131 | `update_attributes` | handled | `req!` → `cb_play_update_attributes` | M52/M73. |
| 132 | `update_mob_effect` | handled | `req!` → `cb_play_update_mob_effect` | M13. |
| 133 | `update_recipes` | absent | **C** | Recipe property sets + stonecutter recipes; recipe book / crafting. |
| 134 | `update_tags` | handled | `req!` → `cb_play_update_tags` | **M69.** The play copy is the `/reload` case; the join-time copy is **configuration 13**, and resolving only this one would have looked like it worked until someone reloaded. |
| 135 | `projectile_power` | absent | **A** | A projectile entity's `accelerationPower`. |
| 136 | `custom_report_details` | absent | **D** | Key/value metadata to attach to a crash report. |
| 137 | `server_links` | absent | **B** | Links rendered on the pause and disconnect screens. |
| 138 | `waypoint` | absent | **B** | The locator bar. |
| 139 | `clear_dialog` | absent | **C** | The dialog framework. |
| 140 | `show_dialog` | absent | **C** | The dialog framework. `Holder<Dialog>` over the **datapack** `minecraft:dialog` registry plus `Dialog`'s codec tree — resolvable in principle, **not verified in detail** here. |

---

## §6 What this audit does NOT verify

Stated explicitly, because the counts above are easy to over-read.

- **It does not verify that the 72 handled packets are decoded correctly.**
  It verifies that the id is resolved and that a dispatch arm tests it.
  Correctness of those 72 is what the `*shot --check` gates and the unit tests
  cover, and they cover it unevenly: `inventoryshot` is exhaustive about the
  container packets, while `player_info_update`'s walk is graded only by unit
  tests inside `rewo-net`.
- **It does not verify that the 72 are decoded *completely*.** §4 lists the
  known partials found by reading the arms; there may be more. Nothing
  mechanical distinguishes "consumed the body" from "read the first field" —
  and that includes the machine check in §1, which is why §4 exists.
- **It does not measure what a real server actually sends.** No capture was
  taken. A packet in the report may never appear on a vanilla connection
  (`game_test_highlight_pos`), and the ones that do appear are not weighted by
  frequency. The class-D reasons are read out of `ClientPacketListener`, not
  observed.
- **`show_dialog` (140)'s codec was not walked.** It is the one row whose
  resolvability is asserted rather than checked — see §1.
- **The classes are a judgement.** The A/B boundary in particular is a call
  about what a decode buys on its own; `set_experience` (B) and
  `projectile_power` (A) are the same shape on the wire and differ only in
  whether anything but a renderer would consume them.
- **Serverbound is out of scope.** The report lists 69 serverbound-play
  packets and this audit says nothing about them. Several class-A gaps above
  have serverbound halves that would also be needed to *act* on them
  (`move_vehicle`, `container_close`).
- **Configuration and login states are out of scope**, though both are fully
  resolved in `ids.rs` today. M69 records the cost of that scope: `update_tags`
  exists in **both** configuration (13) and play (134), and a play-only survey
  named only the second — the one a vanilla server sends *last*.
- **`rewo play`'s `CORRECTIONS 0` proves less than it is often cited as
  proving.** The harness walks on flat ground; M68 added a knockback and mount
  scenario, but the meter itself is one-sided — vanilla's move check flags a
  client that moves too **much**, and one that ignores a shove moves too
  **little**. Treat the number as "no correction *on the paths the harness
  exercises*".

---

## §7 What M67 decoded

Three packets, chosen for being small, self-contained, class A, and one
coherent thing rather than three unrelated ones: **the server's view area**.

| id | packet | body |
|---:|---|---|
| 94 | `set_chunk_cache_center` | two VarInts (chunk x, chunk z) |
| 95 | `set_chunk_cache_radius` | one VarInt |
| 111 | `set_simulation_distance` | one VarInt |

They land in `crates/rewo-net/src/view_area.rs` as one `ViewArea` struct on
`PlaySession`. **Decode and state only** — nothing unloads a column, throttles
a mesh, or gates an entity tick on them yet.

The four rules where a plausible implementation is silently wrong are
documented at their sites and each is mutation-tested:

1. **`calculateStorageRange(viewRange) = max(2, viewRange) + 3`.** A server
   radius of 2 still storages 5. Using the packet's number directly evicts
   columns the server still considers loaded, and does so only at small render
   distances — invisible at 12, wrong at 2.
2. **A server radius of `0` means "no cap", not "render nothing".**
3. **`inRange` is Chebyshev, not Euclidean** — a square.
4. **The login packet carries the initial pair**, and `read_login_prefix` now
   returns them from the walk it was already doing rather than a second copy.

### The mutation survivor was a real gap

13 witnesses, 16 deliberate mutations, **one survived**. Replacing
`read_simulation_distance`'s `r.varint()` with `r.varlong()? as i32` passed
every test: for any `i32` a server writes, a VarLong reader consumes the same
bytes and its low 32 bits are the same number. The two differ only on a
**malformed** body with a sixth continuation byte. So the arity witness was
measuring the field's *length in the happy case* and calling it "this is a
VarInt". Fixed by an explicit overlong-rejection witness — a pattern M74's new
VarInt readers each copy.

---

## §8 What M74 did — the re-derivation, and the drift

M74 re-derived §5 from the code rather than patching the rows it was told were
wrong, and then implemented six of the class-A gaps.

### The drift, and its mechanism

**Ten of M67's 141 rows were wrong**, all in one direction — `absent` about
code that was present:

| id | packet | M67 said | truth |
|---:|---|---|---|
| 36 | `explode` | absent | handled (M68) |
| 57 | `move_vehicle` | absent | handled (M68) |
| 94 | `set_chunk_cache_center` | absent | handled (M67 itself) |
| 95 | `set_chunk_cache_radius` | absent | handled (M67 itself) |
| 96 | `set_cursor_item` | `**M69**` | handled |
| 101 | `set_entity_motion` | absent | handled (M68) |
| 107 | `set_passengers` | `**M68 + M70 + M72**` | handled |
| 108 | `set_player_inventory` | `**M69**` | handled |
| 111 | `set_simulation_distance` | absent | handled (M67 itself) |
| 134 | `update_tags` | `**M69**` | handled |

The headline counts were wrong by the same ten: **56 / 85 published against
66 / 75 true**, with class A at 31 against a true 21.

**The mechanism is not neglect and not a misreading.** M67 wrote the table by
grepping, and four packets landed in `ids.rs` **the same day** — three of them
from M67's own sibling milestone M68. The audit was a *snapshot of a moving
tree*, and it began going stale within hours of being taken.

M67 saw this happening and worked around it in two ways, both of which made it
worse. It added an "After §7" column to §2 predicting where the counts would
land, so the published table described a moment that never existed. And it
wrote milestone markers (`**M69**`) into the *status* column for rows that had
landed since, which made the table unparseable and put four rows outside the
grammar any future check could read.

**That generalises**: any hand-maintained inventory of a live codebase decays
at the rate the codebase changes, and annotating the decay is not a fix. The
only durable answer is to make the document derive from the code, or fail when
it disagrees.

### The fix — `ids::tests::the_coverage_table_matches_the_code`

A unit test in `crates/rewo-net/src/ids.rs`, not a `*shot` gate command,
because it should fire on **the event that causes the drift** — someone
editing `ids.rs` — and `cargo test -p rewo-net` is what runs then. It reads
this document, `ids.rs` and the dispatch chain with `include_str!`, so it has
no runtime dependency on the datagen report, the network or the cwd.

Deliberately *not* a spot-check of a few rows: a spot-check has the same
failure mode as the grep it replaces. It recomputes all 141 statuses and both
count tables. §1 states the grammar it requires.

Its own limits, stated because the point of §6 is that counts are easy to
over-read: it verifies **status**, not correctness and not completeness. A
packet that is dispatched and half-decoded is `handled` to this test, which is
exactly the gap §4 is maintained by hand to cover.

### The six packets

Two of them are a behaviour fix and four are decode-and-state.

| id | packet | body | where |
|---:|---|---|---|
| 12 | `chunk_batch_start` | empty | `chunk_batch.rs` |
| 11 | `chunk_batch_finished` | one VarInt | `chunk_batch.rs` |
| 10 | `change_difficulty` | VarInt id + bool | `client_state.rs` |
| 17 | `container_close` | one VarInt | `client_state.rs` |
| 93 | `set_camera` | one VarInt | `client_state.rs` |
| 127 | `ticking_state` | f32 + bool | `ticking.rs` |
| 128 | `ticking_step` | one VarInt | `ticking.rs` |

**The chunk-batch pair is a live divergence, not a missing decode.** Rewo
replied `p.f32(64.0)`; vanilla replies `ChunkBatchSizeCalculator
.getDesiredChunksPerTick()`, seeded at `7e6 / 2e6` = **3.5**. Rewo therefore
over-bid the server by ~18× on the first batch of every session and never
adapted, and the server sizes its chunk batches to that number. Both halves
were needed: the calculator without `chunk_batch_start` has no interval to
measure and would only have produced a *differently* wrong constant.

**`Difficulty` is a third enum convention.** The project's notes record two —
`readEnum` (out-of-range is an error) and `ByIdMap.continuous(…, ZERO)`
(out-of-range is the zero value). `Difficulty.STREAM_CODEC` is
`ByteBufCodecs.idMapper` over `ByIdMap.continuous(…, **WRAP**)`, so a VarInt id
of 5 is `EASY` — where ZERO gives `PEACEFUL` and `readEnum` rejects the packet.
And WRAP is `Math.floorMod`, not `%`: a negative id is legal and indexes from
the far end, where Rust's `%` would panic.

**`container_close`'s id is read and ignored.** `handleContainerClose` is one
line with no comparison against `containerMenu.containerId`. Gating on it —
which is exactly what M34/M35 correctly do for `container_set_slot` — would
drop the packet whose only job is to close the screen.

**`set_camera` closes a stub M70 left behind.** `LabelViewer::camera_entity`
was hard-wired to `session.player_id` with a comment reading "Rewo never
detaches the camera"; this is the packet that detaches it. An unresolvable
entity id leaves the camera **where it was** rather than resetting to the
player — and the resolution must treat the local player's own id as valid,
because vanilla's `level.getEntity` finds the player and Rewo's `EntityTable`
never contains it. Without that clause the server could never hand the camera
back at the end of a spectate.

**Excluded on purpose:** `player_rotation` (73) and `player_look_at` (71),
despite ranking second in §3 — they write `rewo_world::physics::PlayerState`,
which the concurrent §4.1 milestone owns. (Both landed in **M76**, after M75
finished with that state.) `TickRateManager` is transcribed in
full, including `tick()`, but the session's 20 Hz loop does **not** consult it:
gating the loop would retime every existing harness and wants its own live
gate.

### One doc claim that the decompile contradicted

Writing `client_state.rs` I asserted in a doc comment that the login packet
carries the difficulty and that `NORMAL` was therefore a stand-in. It does not.
`ClientboundLoginPacket`'s fields are `playerId`, `hardcore`, `levels`,
`maxPlayers`, `chunkRadius`, `simulationDistance`, `reducedDebugInfo`,
`showDeathScreen`, `doLimitedCrafting`, the spawn info, `onlineMode` and
`enforcesSecureChat` — no difficulty. `handleLogin` writes
`new ClientLevelData(Difficulty.NORMAL, …)` with the constant in the source, so
`change_difficulty` is the **only** source and Rewo's default is vanilla's
literal rather than a guess.

It also turned up the reason not to reset it: `handleRespawn` builds its
replacement level data from `this.levelData.getDifficulty()`, carrying the
value across a dimension change. `ClientState` therefore lives on
`PlaySession` and is deliberately untouched by `apply_respawn` — the same rule
`ViewArea` follows, and for the same reason.
