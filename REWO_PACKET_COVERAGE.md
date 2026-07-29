# Rewo clientbound-play packet coverage — what the server sends that Rewo ignores

Audit date **2026-07-29**, against **26.2 / protocol 776**. Ground truth is the
bundled datagen report
(`%APPDATA%/EwoClient/rewo/26.2/datagen/generated/reports/packets.json`) — the
same file `crates/rewo-net/src/ids.rs` resolves against — plus the Vineflower
decompile beside it. Nothing here is inferred from a wiki.

**Why this exists.** Rewo's packet handling grew milestone by milestone, so
what it decodes is a *historical accident, not a decision*. Twice recently a
whole family turned out to be simply absent and was found by noticing it was
not in `ids.rs` at all — the sound packets (M63) and the
scoreboard / boss-bar / tab-list set (M65). This is the enumeration that makes
that failure mode impossible to repeat: every clientbound-play packet in the
report appears in §5 with a status.

---

## §0 Handoff — the seven things worth knowing

1. **141 clientbound-play packets. Rewo resolves and consumes 56 of them. 85
   are not in `ids.rs` at all.** No packet is resolved-but-ignored: the
   `cb_play_*` field set and the dispatch chain agree exactly, which is a real
   (and slightly surprising) property of this codebase — see §1.
2. **The 85 gaps split 31 / 20 / 23 / 11** across pure state, needs-rendering,
   needs-a-missing-subsystem, and not-applicable. The 31 pure-state ones are
   decodable and headlessly gateable *today*, with no renderer and no design
   decision — the same test the M52–M65 batches were chosen by.
3. **`update_tags` is the sharpest gap in the list**, because its failure
   mode is silence. Rewo reads `ItemTags.SPEARS` (M19) and the enchantment
   `curse` / `tooltip_order` tags (M42) *from the jar*. A server whose datapack
   changes those tags diverges from Rewo with no error, no warning, and nothing
   for a gate to catch — the M64 alphabetisation trap, one layer up.
4. **A whole class of gaps is invisible to the physics gate by construction.**
   `explode`'s `playerKnockback`, `set_entity_motion`, `move_vehicle` and
   `set_passengers` are all *velocity and mount* inputs to the local player.
   `rewo play` reports `CORRECTIONS 0` over 600 ticks of walking — but the
   harness is never knocked back, never rides anything, and never gets exploded
   at, so the number is silent on all four. §3.
5. **`hurt_animation` (42) is the input `M52a`'s vacuous `no_damage_tilt`
   module has nothing to disable.** The Velvet-batch note "to port the disable
   you must first build the thing being disabled" has a packet behind it.
6. **`chunk_batch_start` (12) is half of a control loop Rewo answers with a
   constant.** Vanilla times the interval between `chunk_batch_start` and
   `chunk_batch_finished` to compute `getDesiredChunksPerTick()`. `play.rs`
   replies `p.f32(64.0)`.
7. **"Handled" is not "complete."** Six currently-handled packets decode only
   the part a milestone needed — §4 names them, because a partially-consumed
   packet looks identical to a fully-consumed one from `ids.rs`.

---

## §1 Method, and what "handled" means here

Three questions, asked separately, because conflating them is exactly how M63
and M65 stayed hidden:

1. **Is it in the report?** All 141 rows below come from `packets.json`'s
   `play.clientbound` table, sorted by `protocol_id`. Rewo resolves ids **by
   name**, so the id column is informational — a renumber is not a gap.
2. **Does `ids.rs` resolve it?** Parsed mechanically out of the `resolve`
   block: every `cb_play_*: req!(p, P, C, "<name>")` / `opt!(…)` entry.
3. **Does anything outside `ids.rs` reference that field?** A word-boundary
   grep across all of `crates/`. This is the question that separates "resolved"
   from "handled", and it is asked of the *field*, not the packet — a name
   resolved into a struct nothing reads is a gap wearing a handled costume.

The result of (3) is worth stating plainly because it is a negative finding:
**the resolved-but-unreferenced set is empty.** Every one of the 56 resolved
ids reaches a dispatch arm in `play.rs` or a `route_*` in `lib.rs`. So the
gap is entirely in question (2) — 85 names that were never resolved.

That is a coarser instrument than it sounds, and §4 is the correction: grep
proves a field is *read*, not that the body is fully consumed.

### Classification

Each of the 85 gaps carries one class. The classes are about **what it would
take**, not about how much anyone wants it:

| Class | Meaning |
|---|---|
| **A** — pure state | Decoding it changes a value Rewo could act or gate on **without drawing anything new**. A witness can prove the decode; no human has to look at it. This is the class the M52–M65 batches drew from. |
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

**These are the counts as audited, before §7's three land.** They are stated
that way on purpose: the point of the document is the shape of the gap, and
folding M67's own work into the headline would make the table describe a
moment that never existed. §7 says what moved.

| Status | Count | After §7 |
|---|---|---|
| Resolved **and** consumed | **56** | 59 |
| Resolved but ignored | **0** | 0 |
| Not resolved at all | **85** | 82 |
| **Total clientbound-play** | **141** | 141 |

The 85 gaps, by class:

| Class | Count | Share of the gap |
|---|---|---|
| **A** pure state, no rendering | **31** | 36% |
| **B** needs rendering | **20** | 24% |
| **C** needs a subsystem Rewo lacks | **23** | 27% |
| **D** not applicable | **11** | 13% |

After §7, class A is **28**. Reproduce with the two greps §1 describes; the
row-by-row result is §5, where the three carry a `Decoded by this task` note
and still read `absent` — that column is the audit's own snapshot.

---

## §3 The three most valuable gaps

Ranked by *how badly the failure hides*, not by how visible the feature is.

### 1. The local player's velocity and mount state — `explode` (36),
`set_entity_motion` (101), `move_vehicle` (57), `set_passengers` (107)

Rewo's physics port is graded by `rewo play`, whose headline number is
`CORRECTIONS 0` over 600 ticks. That gate is real and it is **structurally
blind to all four of these**: the bot walks and jumps on flat ground, and is
never knocked back, never exploded at, never put in a boat and never given a
passenger. So the strongest evidence Rewo has for its physics says nothing
about the four packets that change a player's motion from outside their own
input.

Concretely, from `ClientPacketListener`:

- `handleExplosion` ends with
  `packet.playerKnockback().ifPresent(this.minecraft.player::addDeltaMovement)`
  — an `Optional<Vec3>` added straight onto the local player's delta movement.
- `handleSetEntityMotion` is `entity.lerpMotion(packet.movement())`. For the
  local player's own id this is knockback from a hit, from water, from a
  wind charge.
- `handleSetEntityPassengersPacket` calls `vehicle.ejectPassengers()` then
  `passenger.startRiding(vehicle, …)`. **M70 decodes this packet** and keeps
  the riding graph both ways, but only to answer `Entity.isVehicle()` for the
  label predicate. The positional half is untouched: Rewo still renders a
  mounted mob at its own last-reported position, which for a boat passenger is
  *approximately* right and for a horse rider is a floating body.
  **Closed by M72** — and the "approximately right for a boat" guess above was
  wrong in both directions: a boat's seat comes from an override that ignores
  the attachment table entirely, and the error a rider actually shows is not a
  constant offset but a **lag**, because its own three-tick lerp is still
  chasing a position the server stopped updating.
- `move_vehicle` is the position half of the same contract, and has a
  serverbound echo.

All four are class A. The first three would be graded by a knockback scenario
the play harness does not yet have — which is itself the finding.

> **Closed by M68 (2026-07-29), with three corrections to the paragraph above.**
> All four are decoded, and `rewo play --motion-check` is the knockback/mount
> scenario this section said the harness lacked.
>
> 1. **`set_entity_motion` is not a fixed-point short.** 26.2 replaced that
>    encoding with `Vec3.LP_STREAM_CODEC` (`LpVec3`) — three 15-bit mantissas
>    against one shared integer scale, with a **one-byte zero sentinel**. There
>    is no `/ 8000.0` and no `Mth.clamp(±3.9)` anywhere in the decompile.
> 2. **`move_vehicle` cannot be exercised by this client at all.** Both send
>    sites are inside `ServerGamePacketListenerImpl.handleMoveVehicle` — the
>    server *rejecting* a serverbound vehicle move. Rewo rides as a passenger
>    and never claims to drive, so it never provokes one. It is decoded and
>    unit-tested; the live gate reports its count and does not require it.
> 3. **`CORRECTIONS` is weaker here than "structurally blind" suggests.** It is
>    blind to these packets *and* it does not reliably catch mishandling them
>    even once they arrive: vanilla's move check flags a client that moves too
>    **much**, while one that ignores a shove moves too **little**. A mutation
>    that decoded the knockback and dropped it produced **zero** corrections.
>    The gate therefore grades a direct observation — the measured change in
>    the client's own velocity — and treats the correction count as the
>    secondary witness.
>
> A fourth, about riding: a mounted player's movement is **not validated at
> all** (`if (this.player.isPassenger())` snaps and returns), so no correction
> count can say anything about riding accuracy.

### 2. The inventory's authoritative writes — `set_player_inventory` (108),
`set_cursor_item` (96), `container_close` (17)

M34/M35 built a *predicting* inventory: the click packet carries the client's
belief and the only resync trigger is a state-id mismatch. Two packets exist
precisely to correct the prediction without a resync, and Rewo has neither:

- `handleSetPlayerInventory` is
  `player.getInventory().setItem(packet.slot(), packet.contents())` — and its
  `slot` is an **inventory index**, the coordinate system M34's notes call out
  as never lining up with the wire's 46 menu slots. M34 handled
  `container_set_slot` (menu slots) and this one is its index-addressed twin.
- `handleSetCursorItem` is `containerMenu.setCarried(packet.contents())` — the
  server overwriting the carried stack directly.

`container_close` is in the same cluster and is one byte: nothing currently
tells Rewo's inventory screen to close.

### 3. `update_tags` (config **13** *and* play **134**) — the divergence with no symptom

> **Correction (M69).** This audit surveyed clientbound-**play** only, so it
> listed `update_tags` at play 134 alone. The packet is also sent in
> **configuration** (id 13), and *that* is the copy a vanilla server sends on
> join, right after `registry_data`; the play copy is the datapack-reload case.
> Resolving only the play id would have looked like it worked until someone ran
> `/reload`. A whole-protocol sweep would have caught this; the play-only scope
> is a limit of the audit, not of the packet.

Rewo reads vanilla's datapack tags out of the **client jar**: `ItemTags.SPEARS`
for M19's swing durations, `enchantment/curse` and `enchantment/tooltip_order`
for M42's tooltip lines. `handleUpdateTags` is the packet where the *server*
tells the client what its tags actually are, and it is applied for every
non-memory connection.

The reason this ranks above a dozen more visible gaps is the failure shape:
a server whose datapack retags one item produces, in Rewo, a wrong swing
duration or a missing tooltip line and **no error anywhere**. It is the same
class as M64's alphabetisation trap — ids still round-trip, strings are still
real, and only someone who knows the right answer can see it. It is class A
and the decode is a map of registry key → tag name → id list.

**Honourable mention, for a different reason:** `chunk_batch_start` (12) is an
empty body and closes a control loop Rewo currently answers with the literal
`64.0`. It is the cheapest row in the whole table.

---

## §4 "Handled" is not "complete"

`ids.rs` cannot express *partially*. These are consumed by the dispatch and
decode less than the body carries. Listed because a future audit run by the
same two greps will call every one of them handled.

**`game_event` closed in M71** and is kept below as a worked example of the
class — the row it used to have is the "what is not" column that turned into a
milestone.

| Packet | What is consumed | What is not |
|---|---|---|
| `game_event` (38) — **closed, M71** | All fourteen types, via `rewo_net::game_event`. Ten are applied: the four weather levels (unchanged, still `WeatherState`), `CHANGE_GAME_MODE` → `ClientGameState::game_mode`, the `IMMEDIATE_RESPAWN` / `LIMITED_CRAFTING` flags, `WIN_GAME` / `DEMO_EVENT` / `LEVEL_CHUNKS_LOAD_START` as markers, `NO_RESPAWN_BLOCK_AVAILABLE` as a queued translation key the app resolves into chat, and the three local sounds (`PLAY_ARROW_HIT_SOUND`, `PUFFER_FISH_STING`, `GUARDIAN_ELDER_EFFECT`'s conditional curse). | **`GUARDIAN_ELDER_EFFECT`'s particle** — `ParticleTypes.ELDER_GUARDIAN` is not one of M37's six transcribed kinds, and M37's rule is that an unknown kind is dropped rather than rendered as something else, so it is recorded as a gap instead of being given a fake home. The gamemode is **modelled, not acted on**: nothing in `rewo-world::physics` has a flight, no-clip or invulnerability concept, so `updatePlayerAbilities` has nothing to update (see §4.1). `WIN_GAME` and `DEMO_EVENT` open screens Rewo has no screen system for. |
| `level_event` (46) | The particle half, through M37's `route_level_event`. | The sound half of the same id table — deliberately, per M63: playback, not decode. |
| `chunk_batch_finished` (11) | The id, as a trigger. | The `batchSize` float, and the batch clock it feeds. The reply is the constant `64.0`. |
| `block_changed_ack` (4) | The id. The arm is a `log::debug!`. | The sequence number and the block-prediction rollback it acknowledges — Rewo does not predict block changes, so there is nothing to roll back yet. |
| `container_set_content` / `container_set_slot` (18 / 20) | Container id **0** — the player's own inventory. | Every other container id, dropped whole (M34's documented choice: there is no screen to put them in). |
| `player_info_update` (70) | `ADD_PLAYER`, `UPDATE_GAME_MODE`, `UPDATE_LATENCY`, `UPDATE_LIST_ORDER`, and the walk past the rest. | `UPDATE_DISPLAY_NAME` and `INITIALIZE_CHAT` are walked and discarded rather than stored. The walk is correct — M62 unified it into one function after finding a drifted copy — but the values do not survive it. |

### §4.1 What wiring the gamemode to physics would actually take

M71 models `CHANGE_GAME_MODE` and stops there, because acting on it is a
larger job than the packet suggests. `MultiPlayerGameMode.setLocalMode` ends in
`GameType.updatePlayerAbilities(abilities)`, which writes four booleans —
`mayfly`, `instabuild`, `invulnerable`, `flying` (and note **SPECTATOR sets
`flying = true` while CREATIVE only sets `mayfly`**, so entering creative does
not start you flying). Rewo has **none of those concepts**: a grep for
`abilities`, `may_fly`, `flying` or `no_clip` across `rewo-world` and
`rewo-app` finds nothing but the elytra `fall_flying` cape term. The work is
therefore:

1. An abilities struct on `PlayerState`, and `GameType::updatePlayerAbilities`
   transcribed onto it.
2. A flight branch in `rewo_world::physics::tick` — vanilla's creative flight
   is its own velocity model, not gravity with a different constant.
3. Spectator no-clip: `physics` currently always consults `baked.solid`.
4. `player_abilities` (clientbound **and** serverbound) — neither is in
   `ids.rs`. The server is authoritative about `mayfly`, and the client must
   send its flying state back or the server rubber-bands it.

Two other authoritative sources also feed this state and are not wired, which
is why `ClientGameState` is complete for the *packet* and not for the *state*:
the **login packet** carries `showDeathScreen` and `doLimitedCrafting` (the
`game_event` ids 11/12 are only the mid-session gamerule change), and
**`spawn_info`** carries `gameType` + `previousGameType` on both login and
respawn — `crates/rewo-net/src/spawn_info.rs` already decodes both fields and
nothing consumes them. `handleRespawn` also copies `showDeathScreen` onto the
new player but **not** `doLimitedCrafting`, so that one resets in vanilla too.

---

## §5 The full table

`handled` = resolved in `ids.rs` **and** referenced by a dispatch arm or
`route_*`. `absent` = not in `ids.rs`, with its class from §1.

| id | packet | status | resolution / class | note |
|---:|---|---|---|---|
| 0 | `bundle_delimiter` | absent | **A** | Empty body. Delimits a `ClientboundBundlePacket` — vanilla applies everything between two delimiters in one tick. Rewo applies each packet as it drains, so a spawn and its metadata can land a frame apart. |
| 1 | `add_entity` | handled | `req!` → `cb_play_add_entity` | |
| 2 | `animate` | handled | `req!` → `cb_play_animate` | |
| 3 | `award_stats` | absent | **B** | Statistics screen. `Stat.STREAM_CODEC` dispatches on `minecraft:stat_type` (9) then the per-type value registry — both in `registries.json`. |
| 4 | `block_changed_ack` | handled | `opt!` → `cb_play_block_ack` | |
| 5 | `block_destruction` | absent | **B** | The crack overlay for a block someone else is mining. |
| 6 | `block_entity_data` | handled | `req!` → `cb_play_block_entity_data` | |
| 7 | `block_event` | handled | `req!` → `cb_play_block_event` | |
| 8 | `block_update` | handled | `req!` → `cb_play_block_update` | |
| 9 | `boss_event` | handled | `req!` → `cb_play_boss_event` | |
| 10 | `change_difficulty` | absent | **A** | `Difficulty` (`readEnum`, so out-of-range is an error) + `locked` bool. |
| 11 | `chunk_batch_finished` | handled | `req!` → `cb_play_chunk_batch_finished` | |
| 12 | `chunk_batch_start` | absent | **A** | Empty body. The batch clock vanilla times to compute `getDesiredChunksPerTick()`. **Rewo replies to `chunk_batch_finished` with a hard-coded `64.0`** (`play.rs`), so the throttle it advertises is a constant. |
| 13 | `chunks_biomes` | handled | `opt!` → `cb_play_chunks_biomes` | |
| 14 | `clear_titles` | absent | **B** | Title overlay. |
| 15 | `command_suggestions` | absent | **C** | Chat/command input. |
| 16 | `commands` | absent | **C** | The Brigadier command tree. Worthless without command input. |
| 17 | `container_close` | absent | **A** | One container id. Nothing currently tells Rewo's inventory screen to close. |
| 18 | `container_set_content` | handled | `req!` → `cb_play_container_set_content` | |
| 19 | `container_set_data` | absent | **C** | Furnace/brewing/enchanting progress — needs the non-player menus M34 excluded. |
| 20 | `container_set_slot` | handled | `req!` → `cb_play_container_set_slot` | |
| 21 | `cookie_request` | handled | `opt!` → `cb_play_cookie_request` | |
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
| 33 | `disguised_chat` | absent | **A** | `Component` + `ChatType.Bound`. The chat overlay already exists, so this is decode-and-append. |
| 34 | `entity_event` | handled | `req!` → `cb_play_entity_event` | |
| 35 | `entity_position_sync` | handled | `req!` → `cb_play_entity_position_sync` | |
| 36 | `explode` | absent | **A** | **Decoded by M68** (physics prefix only — the particle/sound/weighted-list tail is deliberately not consumed). `playerKnockback` is `addDeltaMovement` on the local player — **physics state**. The particle and sound halves are separate (B / audio). |
| 37 | `forget_level_chunk` | handled | `req!` → `cb_play_forget_chunk` | |
| 38 | `game_event` | handled | `req!` → `cb_play_game_event` | All 14 types since M71 (`rewo_net::game_event`). Was 4 of 14 — the §4 worked example. |
| 39 | `game_rule_values` | absent | **A** | `Map<ResourceKey<GameRule>, String>`. |
| 40 | `game_test_highlight_pos` | absent | **D** | Game-test tooling. |
| 41 | `mount_screen_open` | absent | **C** | Horse/nautilus inventory screen. |
| 42 | `hurt_animation` | absent | **B** | The damage camera yaw-tilt. **This is the input `M52a`'s vacuous `no_damage_tilt` module has nothing to disable.** |
| 43 | `initialize_border` | absent | **B** | World border. |
| 44 | `keep_alive` | handled | `req!` → `cb_play_keep_alive` | |
| 45 | `level_chunk_with_light` | handled | `req!` → `cb_play_level_chunk` | |
| 46 | `level_event` | handled | `opt!` → `cb_play_level_event` | |
| 47 | `level_particles` | handled | `opt!` → `cb_play_level_particles` | |
| 48 | `light_update` | handled | `opt!` → `cb_play_light_update` | |
| 49 | `login` | handled | `req!` → `cb_play_login` | |
| 50 | `low_disk_space_warning` | absent | **D** | `Minecraft.sendLowDiskSpaceWarning` — the integrated server warning about its own save directory. |
| 51 | `map_item_data` | absent | **C** | Map-item colour patches + decorations; needs a map image pipeline and a map renderer. |
| 52 | `merchant_offers` | absent | **C** | Villager trade screen. |
| 53 | `move_entity_pos` | handled | `req!` → `cb_play_move_entity_pos` | |
| 54 | `move_entity_pos_rot` | handled | `req!` → `cb_play_move_entity_pos_rot` | |
| 55 | `move_minecart_along_track` | absent | **A** | A list of interpolation steps for one minecart — entity movement state. |
| 56 | `move_entity_rot` | handled | `req!` → `cb_play_move_entity_rot` | |
| 57 | `move_vehicle` | absent | **A** | **Decoded by M68.** The local player's vehicle position/rotation. Correction: it carries **no entity id** (the client resolves `getRootVehicle()`), and it is sent *only* as a rejection of a serverbound `ServerboundMoveVehiclePacket` — so a passenger-only client never receives one and the live gate cannot trigger it. |
| 58 | `open_book` | absent | **C** | Book screen. |
| 59 | `open_screen` | absent | **C** | The menu framework — `minecraft:menu` registry + a screen per type. |
| 60 | `open_sign_editor` | absent | **C** | Sign edit screen. |
| 61 | `ping` | handled | `req!` → `cb_play_ping` | |
| 62 | `pong_response` | absent | **D** | The reply to a serverbound `ping_request` Rewo never sends (`pingDebugMonitor`). |
| 63 | `place_ghost_recipe` | absent | **C** | Recipe book. |
| 64 | `player_abilities` | absent | **A** | **Decoded by this task.** Flags byte + `flyingSpeed` + `walkingSpeed`. |
| 65 | `player_chat` | handled | `opt!` → `cb_play_player_chat` | |
| 66 | `player_combat_end` | absent | **A** | Vestigial — vanilla's handler is an empty method. |
| 67 | `player_combat_enter` | absent | **A** | Vestigial, empty body, empty handler. |
| 68 | `player_combat_kill` | absent | **B** | The death screen. |
| 69 | `player_info_remove` | handled | `req!` → `cb_play_player_info_remove` | |
| 70 | `player_info_update` | handled | `req!` → `cb_play_player_info_update` | |
| 71 | `player_look_at` | absent | **A** | Forces the local player's rotation to face a point (`/teleport … facing`). |
| 72 | `player_position` | handled | `req!` → `cb_play_position` | |
| 73 | `player_rotation` | absent | **A** | Sets the local player's yaw/pitch with per-axis relative flags. |
| 74 | `recipe_book_add` | absent | **C** | Recipe book. |
| 75 | `recipe_book_remove` | absent | **C** | Recipe book. |
| 76 | `recipe_book_settings` | absent | **C** | Recipe book. |
| 77 | `remove_entities` | handled | `req!` → `cb_play_remove_entities` | |
| 78 | `remove_mob_effect` | handled | `req!` → `cb_play_remove_mob_effect` | |
| 79 | `reset_score` | handled | `req!` → `cb_play_reset_score` | |
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
| 93 | `set_camera` | absent | **A** | The spectator camera's target entity — pure state that redirects the eye. |
| 94 | `set_chunk_cache_center` | absent | **A** | **Decoded by this task.** Two VarInts. |
| 95 | `set_chunk_cache_radius` | absent | **A** | **Decoded by this task.** One VarInt. |
| 96 | `set_cursor_item` | **M69** | — | The server's authoritative carried stack. M35 *predicts* the cursor and its only correction path is a full `container_set_content` resync. |
| 97 | `set_default_spawn_position` | absent | **A** | `LevelData.RespawnData` — the compass target and respawn point. |
| 98 | `set_display_objective` | handled | `req!` → `cb_play_set_display_objective` | |
| 99 | `set_entity_data` | handled | `req!` → `cb_play_set_entity_data` | |
| 100 | `set_entity_link` | absent | **A** | Leash holder id. Rendering the rope is separate (B). |
| 101 | `set_entity_motion` | absent | **A** | **Decoded by M68.** `lerpMotion` on one entity — velocity, including the local player's knockback. Correction: the body is `Vec3.LP_STREAM_CODEC` (`LpVec3`), **not** the legacy `short / 8000.0` fixed point, which no longer exists in 26.2. |
| 102 | `set_equipment` | handled | `req!` → `cb_play_set_equipment` | |
| 103 | `set_experience` | absent | **B** | XP bar and level number. |
| 104 | `set_health` | handled | `opt!` → `cb_play_set_health` | |
| 105 | `set_held_slot` | handled | `req!` → `cb_play_set_held_slot` | |
| 106 | `set_objective` | handled | `req!` → `cb_play_set_objective` | |
| 107 | `set_passengers` | **M68 + M70 + M72** | — | Three milestones landed disjoint halves of the same packet, and the packet is now **fully consumed**. M70 decoded the riding graph for `Entity.isVehicle()`, which suppresses a ridden entity's floating label; M68 applied the local player's own mount state to its physics; **M72** added the positional half — every rider is derived from its vehicle's PASSENGER attachment point once per tick (`tickPassenger` → `rideTick` → `positionRider`), including the multi-seat clamp, the per-vehicle overrides, and the body yaw a horse or chicken forces onto a living rider. Remaining, and neither is about this packet: a mounted humanoid's **seated leg pose** (`HumanoidModel.setupAnim`'s `isPassenger` block) and the camel's / horse's animation-driven seat offsets, which need metadata Rewo does not decode. |
| 108 | `set_player_inventory` | **M69** | — | An authoritative inventory write addressed by **inventory index**, not menu slot — the third coordinate system M34 names. M34 handled `container_set_slot` and not this. |
| 109 | `set_player_team` | handled | `req!` → `cb_play_set_player_team` | |
| 110 | `set_score` | handled | `req!` → `cb_play_set_score` | |
| 111 | `set_simulation_distance` | absent | **A** | **Decoded by this task.** One VarInt. |
| 112 | `set_subtitle_text` | absent | **B** | Title overlay. |
| 113 | `set_time` | handled | `req!` → `cb_play_set_time` | |
| 114 | `set_title_text` | absent | **B** | Title overlay. |
| 115 | `set_titles_animation` | absent | **B** | Title overlay timings. |
| 116 | `sound_entity` | handled | `req!` → `cb_play_sound_entity` | |
| 117 | `sound` | handled | `req!` → `cb_play_sound` | |
| 118 | `start_configuration` | handled | `opt!` → `cb_play_start_configuration` | |
| 119 | `stop_sound` | handled | `req!` → `cb_play_stop_sound` | |
| 120 | `store_cookie` | absent | **A** | The cookie store Rewo already answers `cookie_request` from — with nothing, because nothing ever stores one. |
| 121 | `system_chat` | handled | `opt!` → `cb_play_system_chat` | |
| 122 | `tab_list` | handled | `req!` → `cb_play_tab_list` | |
| 123 | `tag_query` | absent | **D** | The reply to a serverbound `/data get` query Rewo never sends. |
| 124 | `take_item_entity` | absent | **B** | The pickup animation (the item flies to its collector). The removal itself already arrives via `remove_entities`. |
| 125 | `teleport_entity` | handled | `req!` → `cb_play_teleport_entity` | |
| 126 | `test_instance_block_status` | absent | **D** | Game-test tooling. |
| 127 | `ticking_state` | absent | **A** | `tickRate` + `isFrozen`. Rewo's session assumes a hard 20 Hz. |
| 128 | `ticking_step` | absent | **A** | How many frozen ticks to step. |
| 129 | `transfer` | absent | **C** | Reconnect to another host — needs a transfer/reconnect flow. |
| 130 | `update_advancements` | absent | **C** | Advancements screen. |
| 131 | `update_attributes` | handled | `req!` → `cb_play_update_attributes` | |
| 132 | `update_mob_effect` | handled | `req!` → `cb_play_update_mob_effect` | |
| 133 | `update_recipes` | absent | **C** | Recipe property sets + stonecutter recipes; recipe book / crafting. |
| 134 | `update_tags` | **M69** | — | **The server's datapack tags.** Rewo reads `ItemTags.SPEARS` (M19) and the enchantment tags (M42) from the *jar*; a server that changes them diverges with no error anywhere. |
| 135 | `projectile_power` | absent | **A** | A projectile entity's `accelerationPower`. |
| 136 | `custom_report_details` | absent | **D** | Key/value metadata to attach to a crash report. |
| 137 | `server_links` | absent | **B** | Links rendered on the pause and disconnect screens. |
| 138 | `waypoint` | absent | **B** | The locator bar. |
| 139 | `clear_dialog` | absent | **C** | The dialog framework. |
| 140 | `show_dialog` | absent | **C** | The dialog framework. `Holder<Dialog>` over the **datapack** `minecraft:dialog` registry plus `Dialog`'s codec tree — resolvable in principle, **not verified in detail** here. |

---

## §6 What this audit does NOT verify

Stated explicitly, because the counts above are easy to over-read.

- **It does not verify that the 56 handled packets are decoded correctly.**
  It verifies that the id is resolved and that something reads the field.
  Correctness of those 56 is what the `*shot --check` gates cover, and they
  cover it unevenly: `inventoryshot` is exhaustive about the container
  packets, while `player_info_update`'s walk is graded only by unit tests
  inside `rewo-net`.
- **It does not verify that the 56 are decoded *completely*.** §4 lists the
  six known partials found by reading the arms; there may be more. Nothing
  mechanical distinguishes "consumed the body" from "read the first field".
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
  resolved in `ids.rs` today.

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
a mesh, or gates an entity tick on them yet; that is the half a renderer and a
tuning pass have to grade.

The four rules where a plausible implementation is silently wrong are
documented at their sites and each is mutation-tested:

1. **`calculateStorageRange(viewRange) = max(2, viewRange) + 3`.** The radius
   Rewo would retain columns for is *not* the radius on the wire. A server
   radius of 2 still storages 5. Using the packet's number directly evicts
   columns the server still considers loaded, and does so only at small render
   distances — invisible at 12, wrong at 2.
2. **A server radius of `0` means "no cap", not "render nothing".**
   `Options.getEffectiveRenderDistance` is
   `serverRenderDistance > 0 ? min(local, server) : local`. Clamping to the
   server value unconditionally renders an empty world, and `0` is exactly what
   the field holds before any server has spoken.
3. **`inRange` is Chebyshev, not Euclidean** — `abs(dx) <= r && abs(dz) <= r`,
   a square. A circular test drops the corners the server is still streaming.
4. **The login packet carries the initial pair.** `ClientboundLoginPacket` has
   `chunkRadius` and `simulationDistance` between `maxPlayers` and
   `reducedDebugInfo`, and vanilla's `ClientLevel` constructor is where they
   first take effect. `read_login_prefix` was already walking past both into a
   discard; it now **returns** them from the same walk. It was not duplicated —
   M62's report records a real drift caused by exactly that mistake (a profile
   signature capped at 32767 in a test copy where production correctly had
   1024), so the one walk stays one function and its three other call sites
   read the new struct.

Excluded on purpose: `set_chunk_cache_center`'s effect on which columns are
*kept* (that is an eviction policy, not a decode), and vanilla's
`ClientChunkCache.Storage` ring-buffer index (`floorMod`), which is a storage
layout Rewo does not share.

### The mutation survivor was a real gap

13 witnesses, 16 deliberate mutations. Fifteen were caught on the first run;
**one survived**, and it was a missing witness rather than an equivalent
mutant.

Replacing `read_simulation_distance`'s `r.varint()` with
`r.varlong()? as i32` passed every test in the module. It is *very nearly*
equivalent, which is why nothing noticed: for any `i32` a server writes, the
five-byte two's-complement form ends on a byte with no continuation bit, so a
VarLong reader consumes exactly the same bytes and its low 32 bits are the
same number — including for the negatives the arity witness deliberately
included. The two readers differ in one place only: a **malformed** body with
a sixth continuation byte, which a VarInt reader must reject
(`ProtoError::VarIntTooLong`) and a VarLong reader accepts.

So the arity witness was measuring the field's *length in the happy case* and
calling it "this is a VarInt". Fixed by
`an_overlong_var_int_is_rejected_rather_than_read_as_a_var_long`, which is now
the only thing standing between the two readings; re-run **16/16 caught**.

### Gates

`cargo test -p rewo-net` **338** (was 325; +13, all in `view_area`; the
`spawn_info` login-prefix witness was extended rather than added).
`inventoryshot --check`
131/131, `swingshot --check` 97/97, `eventshot --check` 28/28, all with
validation on. Demo PNG SHA-256 still `2cc56b4acbfb92cb…` — this milestone
touches no pixel, which is the check that it did not.

### One unrelated flake fixed, because it makes that gate a coin toss

`cargo test -p rewo-net` failed roughly **one run in six**, on
`sounds::tests::a_registry_sound_ref_resolves_through_the_report_table`, with
`EOF while parsing a value at line 1 column 0`. It predates this work (M64) and
has nothing to do with the view area: three tests call one `sound_registry()`
helper that writes its fixture to a **fixed** path in the temp directory, cargo
runs them on separate threads, and one test's `fs::write` truncates the file
another is part-way through reading. The fixture path is now unique per call.

It is recorded here rather than left alone because the definition of done for
this task is that `cargo test -p rewo-net` passes, and a gate that passes five
times out of six is not one. Verified over **15 consecutive clean runs**; the
same helper was failing within the first six before the change.
