# M217 — survival block breaking (plan, 2026-10-19)

**The gap.** A world left-click sends `START_DESTROY_BLOCK` once and nothing
else (`app.rs` → `PlaySession::start_dig`). In creative that is enough (the
server breaks on START), and every test server runs creative
(`testserver/server.properties: gamemode=creative`), so nothing ever noticed:
**a survival player cannot mine anything.** `player_action` is sent with only
START (0) and RELEASE_USE_ITEM (5); STOP (2) and ABORT (1) are never sent, and
holding the button does nothing per tick. Found by the 2026-10-19 sweep of
serverbound packets for unwired everyday actions (after M216's entity clicks).

## Ground truth (26.2 decompile)

`MultiPlayerGameMode`:

- `startDestroyBlock(pos, dir)`: creative (`abilities.instabuild`) → predict
  `destroyBlock`, send START, `destroyDelay = 5`. Survival, when not already
  destroying this target: if destroying another, send ABORT for the old pos;
  then (in a prediction) if the block is not air and
  `getDestroyProgress >= 1` → `destroyBlock` (instabreak) else start:
  `isDestroying, destroyBlockPos, destroyingItem = main hand, progress 0,
  ticks 0`; send START with the sequence.
- `continueDestroyBlock(pos, dir)` (every client tick while the attack key is
  held on a block): `destroyDelay > 0 → --, return`. Creative → as start,
  `destroyDelay = 5`. Same target (`pos` equal AND `isSameItemSameComponents`
  main hand) → if air: `isDestroying = false`; else `progress +=
  getDestroyProgress`; hit sound every 4 ticks; `ticks++`; at `progress >= 1`:
  `isDestroying = false`, predict `destroyBlock` + send STOP (sequence),
  `progress = ticks = 0`, `destroyDelay = 5`. Different target →
  `startDestroyBlock`.
- `stopDestroyBlock()` (attack key released / no block target): if destroying,
  send ABORT (pos, DOWN), `isDestroying = false`, `progress = 0`, clear the
  crack overlay, `resetAttackStrengthTicker`.
- `getDestroyStage() = progress > 0 ? (int)(progress * 10) : -1` (the crack
  overlay the local player sees on its own target).

`BlockBehaviour.getDestroyProgress(state, player)`:

```java
float destroySpeed = state.getDestroySpeed(level, pos);   // the block's destroyTime
if (destroySpeed == -1.0F) return 0.0F;                   // unbreakable
int modifier = player.hasCorrectToolForDrops(state) ? 30 : 100;
return player.getDestroySpeed(state) / destroySpeed / modifier;
```

`Player.getDestroySpeed(state)`: the held item's `getDestroySpeed` (its `tool`
component's first rule WITH a speed whose blocks match, else
`defaultMiningSpeed`; no `tool` component → 1.0), `+ MINING_EFFICIENCY` when
`> 1`, `* (1 + (haste/conduit amplifier + 1) * 0.2)` with dig speed, `* {0.3,
0.09, 0.0027, 8.1E-4}[fatigue amplifier]`, `* BLOCK_BREAK_SPEED`, `*
SUBMERGED_MINING_SPEED` with the eye in water, `/ 5` off the ground.
`hasCorrectToolForDrops = !requiresCorrectToolForDrops || tool rule
correct_for_drops` (`Tool.isCorrectForDrops`: first rule WITH the field whose
blocks match, else false).

## Data

- **Destroy time + requires-correct-tool per block**: code, not data —
  `Blocks.java` `.strength(d[, r])`, `.instabreak()`, `.destroyTime(d)`,
  `.requiresCorrectToolForDrops()`, inherited through `ofLegacyCopy` /
  `ofFullCopy` (both copy `destroyTime` and `requiresCorrectToolForDrops`).
  New generator `tools/gen_block_hardness.py` (reuses `gen_block_light.py`'s
  registration walker) → `crates/rewo-data/src/block_hardness.rs`.
- **Tool rules per item**: data — the datagen item component reports
  (`reports/minecraft/components/item/<name>.json`, `minecraft:tool`:
  `rules[{blocks: "#tag" | [names], speed?, correct_for_drops?}]`,
  `default_mining_speed`). New generator `tools/gen_item_tools.py` →
  `crates/rewo-data/src/item_tools.rs`.
- **Block tags**: the server's `update_tags` (already held by the session).
- **Attributes/effects**: the local player's attribute map and mob effects
  (already routed for the HUD).

## Code

- `rewo-world/src/mining.rs` — pure: `tool_speed`, `correct_for_drops`,
  `destroy_progress`, and a `Mining` state machine (`start` / `tick` / `stop`
  → actions `SendStart | SendStop | SendAbort`) with unit tests against
  hand-computed vanilla numbers (dirt by hand 15 ticks; stone by hand never
  correct → 100 modifier; stone with a wooden pickaxe).
- `PlaySession`: `stop_dig`, `abort_dig` (STOP 2 / ABORT 1 with the sequence).
- `LiveApp`: track the held left button; on each client tick call the machine
  with the current block target; release → stop. Creative keeps today's
  START-per-press plus vanilla's 5-tick repeat while held.

## Witness

render-check r78: the client switches itself to survival (`/gamemode
survival`, the bot is op), faces a dirt block `render_check.py` places, holds
the attack through the production path for the machine's tick count, and
passes when the server turns the block to air **after** STOP and no earlier
than the vanilla tick count (dirt by hand: `1 / 0.5 / 30` per tick → 15
ticks), then switches back to creative.
