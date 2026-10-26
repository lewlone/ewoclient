# Rewo vs vanilla playtest (2026-10-26)

The shared playtest harness (`Desktop\playtest-harness`, phase 3) now drives Rewo. Each scenario in
`tools/playtest/` is played twice on one fresh vanilla 26.2 server: once by vanilla's own client (the
harness's Fabric "puppet") and once by Rewo (`REWO_SCRIPT`, `live_cmd/script.rs`). The runs are then
compared frame by frame (SSIM per region, side-by-sides), sound by sound (id, volume, pitch, position,
and the variant each play resolved) and tick by tick (position trace, server corrections). Opus
reviews the flagged frames. Neither client shows a window or touches the cursor.

    python tools/playtest.py                  # all 21 scenarios, both clients, compare
    python tools/playtest.py rewo_hud --review

## In one paragraph

Rewo gets a lot right: walking physics matches vanilla to four decimals on every tick, the boat's
path to 0.02 blocks, server sounds arrive with the right position, volume and pitch, and the book,
crafting, stonecutter, loom and options sounds all match. The riding HUD, held maps, the air bar
and the book screen look like vanilla's. The big gaps are:

- **Shading.** Everything is too bright and flat, and night looks like dusk. Terrain shading
  happens in the wrong colour space. A verified fix is on a separate branch.
- **Missing own sounds.** Rewo plays none of the player's own sounds: steps, place, break, doors,
  hurt.
- **HUD and screen elements missing.** No stack counts, offhand slot, container titles, banner
  icons or furnace output.

Two bugs are fixed on this branch: unseeded sounds always played the same variant, and chests,
beds and shulker boxes couldn't be targeted.

Renderer-level differences (anti-aliasing, mipmaps, font hinting, the Velvet tooltip style) are
expected and not listed. **A difference here is a finding, not a verdict.**

## Fixed on this branch (tests, gates.py 51/51 and the render check green)

1. **Chests, beds and shulker boxes could not be targeted.** `rewo_containers`: every right-click
   on a chest opened nothing, and the harness's click log showed Rewo's crosshair target as `null`
   where vanilla's was the chest. The bake derived a state-dependent outline from the block model,
   and a chest's model (`builtin/entity`) has no geometry, so the outline was empty and the ray went
   straight through. Such a block now takes the shape `special_collision` already transcribes;
   `block_entity_rendered_blocks_are_targetable` fails without the fix. Rerun: every chest and
   barrel opens.
2. **Unseeded sounds played one variant forever.** UI clicks, page turns, music and underwater
   additions always resolved with seed 0: 3/3 page turns were `open_flip1`, 9/9 music picks were
   `dry_hands`. `SoundEngine::resolve` now draws a splitmix64 seed stream (deterministic per engine;
   not a counter, see soundshot's `s6` note). Rerun: page turns vary.

## Fixed after the report (phase B, same branch)

Each item below was re-played against vanilla on the harness; the numbers are the scenario's
sound comparison (events matched within two ticks, same id, volume and position; pitch and
variant are random on both sides).

1. **The player's own sounds.** Generated tables (`tools/gen_block_sounds.py`: every block's
   `SoundType`, the `BlockSetType` / `WoodType` sounds, the step tags) and ports of
   `applyMovementEmissionAndPlaySound` / `Player.playStepSound` (`rewo_world::step_sound`),
   `BlockItem.place`, the local 2001 break event, `continueDestroyBlock`'s hit, the door,
   trapdoor, gate and button sounds (`click_use::block_use_sound`) and `handleDamageEvent`'s
   hurt sound (the damage type's `effects`).
   - `rewo_walk`: 5/5 (was 0/5). `rewo_audio_blocks`: 21/21 (was 1/21). `rewo_hud`: the hurt
     sound plays. `rewo_drowning`: `hurt_drown` plays.
   - The place sound waits for the server's `block_update` instead of a client prediction
     (Rewo does not predict placements): one round trip late on a remote server.
2. **Ambience.** `ClientLevel.animateTick`'s sounds (flowing water, lava, fire, lit furnaces,
   smoker, campfire, nether portal; `rewo_world::animate_tick`) and `tickWeatherEffects`' rain
   (`World::rain_sound`, seeded from the game time as vanilla's).
   - `rewo_maps`: `block.water.ambient` 34 times in Rewo, 33 in vanilla over the whole run
     (positions are random on both sides, so events do not pair up).
   - `rewo_sky_weather`: `weather.rain` 6/6 with the same volume and pitch, each within two
     ticks; the spot differs because the rain level ramps on each client's own tick alignment.
   - `rewo_crafting_furnace` runs under `/tick freeze`, which stops vanilla's animate tick too,
     so it cannot witness the furnace crackle.
3. **Block reach** is the synced `block_interaction_range` (5.0 in creative), not a constant
   4.5. `rewo_audio_blocks`' second door click targeted grass 4.74 blocks away in vanilla and
   nothing in Rewo. Also: the mining inputs asked for `minecraft:`-prefixed attribute names and
   always got the defaults (Efficiency did nothing client-side).
4. **HUD** (a worker branch, merged): hotbar stack counts, the off-hand pocket, the compass,
   recovery compass and clock (needle frames), F1 hiding what `Hud.isHidden` hides (crosshair,
   hotbar, bars, effects, titles, chat, the hand and the outline), and no hearts or food in
   creative (the game mode is read from the login/respawn, not the tab list). `rewo_hud` with
   the Opus review: hotbar, item names, off-hand shield, compass in hand and F1 hiding the HUD
   and the hand all match; the grass colour (the colour-space item) and the missing
   attack-cooldown indicator remain.
5. `rewo_drowning`'s air clock offset is join timing, not Rewo: the positions match tick for
   tick, the world is not frozen, and the server drains air from the spawn, before tick 0.
   The pops are 30 ticks apart on both sides.

6. **Screens** (a worker branch, merged): container titles and the "Inventory" label (with
   each screen's own offsets: the stonecutter, loom, cartography table, merchant and beacon
   overrides), the loom's banner preview, the stonecutter's and loom's button tooltips, and the
   inventory's player model following the recipe-book-shifted panel. The Opus reviews of
   `rewo_containers`, `rewo_loom`, `rewo_stonecutter` and `rewo_recipe_book` confirm each.
   Still open: patterned banner icons (the loom preview shows the base and the pending layer;
   earlier layers need the banner-pattern registry), and a hovered slot's icon reads pale
   (both highlight sprites are drawn; a semi-transparent white blended in the linear
   framebuffer is stronger than vanilla's gamma-space blend, which is the colour-space item).
7. `options.renderClouds` is honoured (`"false"` drew clouds), and mounting shows vanilla's
   "Press Left Shift to Dismount" (`mount.onboard`) on the action bar.

8. **HUD, round 2** (worker): the crosshair's attack-cooldown indicator
   (`Player.attackStrengthTicker`, `Hud.extractCrosshair`), and the hand drawn under an open
   screen as vanilla's is. "A giant copy of the block behind the container" was `/give`'s fake
   item entity: spawned at eye height, never despawning under `/tick freeze`, drawn with no
   face culling, so from inside it filled the view; a dropped block whose bounds contain the
   camera now draws none of it. "The held block sits low" was the swing: Rewo's frame was 4
   ticks after a place click and mid-swing, the pose math matches the decompile at rest.
9. **World** (worker): sign boards and posts, hanging signs, shelves and dried ghasts (26.2
   draws a sign's board as a block model with a 32x32 sprite, which the 16x16 terrain array
   refused; the sprite is now cut into tiles), entity shadows (`ShadowFeatureRenderer`), and
   villager outfits (`VillagerProfessionLayer`; the farmer's hat needed the layers to write
   depth). `rewo_mobs`: every mob, shadows and the farmer's outfit match.
10. **The crosshair inverts** what is behind it (`BlendFunction.INVERT`, in gamma space), and
    **the block outline is vanilla's**: the state's outline shape, black at alpha 0.4,
    `getAppropriateLineWidth()` pixels wide as screen-space quads, pulled toward the camera
    (`VIEW_OFFSET_Z_LAYERING`). It was the unit cube as a 1 px line at alpha 0.7.

Verified on db8177ea: cargo test 4732/0 (`REWO_REQUIRE_ASSETS=1`), gates.py 51/51, render check
223 PASS / 0 FAIL with no validation message.

11. **Riding and arrows** (worker): the horse rears on a jump (`AbstractHorse`'s stand clock from
    the synced flags), and the rider's seat lift `0.15 * standAnimO` closes the 0.15-block apex
    gap: the trace now rises 0.7, 0.675, 0.558, 0.453 like vanilla's. Critical arrows trail crit
    particles; a landed arrow is lit at its eye, not inside its block; a ridden horse or camel
    walks and looks where its rider does; a paddled boat lowers the hands (`isHandsBusy`); the
    camel's idle rig holds its ears up. **Item frames and paintings** hang where the client
    recomputes them (`recalculateBoundingBox` from the packet's block corner): the framed map was
    half a block low and squashed. `rewo_horse`, `rewo_bow`, `rewo_boat` and `rewo_maps` match.
12. **GUI** (worker): the recipe book applies `updateCollections`' three stages (a 3x3 recipe is
    not listed in the 2x2 book; the craftable filter filters; tabs concatenate their categories in
    order), so `rewo_recipe_book` lists vanilla's pages. The hotbar icons draw under an open
    screen, and the screen's dim blends in gamma space (it was half as dark). An offline player
    learns its UUID from `login_finished`, so it wears its default skin, not Steve.

Branch merged into `claude/autonomous-run-1026` at a1ca707b (Enni's session), then continued from
it. Verified on 95c7a8f1: cargo test 4802/0, gates.py 51/51; render check 228/231, with r70 (the
injected item frame), r76 and r77 (clicks on the villager) failing after the frame-placement fix,
being fixed.

### Corrections to the findings

- **"The furnace output slot is empty" was the harness, not Rewo.** The server smelts in real
  time; between the two scenario ticks the vanilla puppet spent 38 s (it runs slowly with a
  screen open) and Rewo 2.5 s, so vanilla's furnace had smelted three ingots and Rewo's none
  (8 raw iron and a barely started arrow, which is right). `rewo_crafting_furnace` now fills the
  result slot by command.
- **A mob's hurt and death sounds are not the client's.** `Entity.playSound` is
  `level.playSound(null, ..)`, which a client level plays only for its own player; the server
  broadcasts the mob's sound, and Rewo already played it. Only the local player's hurt and
  death sounds are client-side (`Player.playSound` excepts itself); both are in now.
  `rewo_combat` covers this (a first cut that played mob sounds locally doubled every one).

Underwater fog moved to Enni's EwoClientV3 session (M422 on `claude/autonomous-run-1026`,
every `FogRenderer` environment, render-check r230/r231).

## Verified, not merged: shading in the wrong colour space (branch `claude/rewo-srgb-terrain`)

Since landed for terrain, water, the hand and particles as M432 (958f37ae on
`claude/autonomous-run-1026`); entity shading and the sky follow there.

Every Rewo frame is 10-30% brighter than vanilla's, with flat side faces, grey-blue grass and a
dusk-like midnight. `rewo-gpu/shaders/world.frag` multiplies the texel, which the sRGB image has
already decoded to **linear**, by `v_color` (vertex colour × lightmap), whose factors are
sRGB-space numbers. Vanilla does the multiply and the fog mix on gamma-encoded values, so in Rewo
every factor f acts as f^(1/2.2). On `rewo_lighting`'s white concrete:

| RGB | vanilla | Rewo now | Rewo with the fix |
|---|---|---|---|
| north face (shade 0.8), noon | 155 159 160 | 182 187 188 | 155 159 160 |
| west face (shade 0.6), noon | 123 126 127 | 163 168 169 | 123 126 127 |
| grass, noon | 84 104 51 | 112 124 88 | 85 106 52 |
| grass, midnight | 64 72 37 | 99 104 74 | 65 73 37 |

The fix encodes the texel, multiplies, fogs with the encoded fog colour and decodes once, in
`world.frag` and `water.frag`. With it Rewo matches vanilla within 1-2 levels, and `cargo test`
(4614) and the render check pass. It isn't merged for three reasons:

- **`lightmapshot` encodes the old model.** Its 58 expected values are `to_srgb(lm)` for a white
  texel, while vanilla's byte is `255·lm`, which the fix renders exactly (lm 0.1804 → 46). The gate
  needs re-deriving from vanilla's arithmetic.
- **`tools/demo_hash.txt`** needs re-pinning.
- **Other passes have the same multiply.** `hand.frag` and `particle.frag` do it too, and the sky is
  too light at night. They should be fixed in the same change, or the hand looks brighter still
  next to correct terrain.

## Findings

### Sound

- **Fixed (phase B 1-2).** **None of the player's own sounds play** (`rewo_audio_blocks` 1 of 21, `rewo_walk` 0 of 5,
  `rewo_maps` 1 of 20). Missing: footsteps on every material, block place, break and hit, door
  open/close, `entity.player.hurt` and `hurt_drown` (`rewo_hud`, `rewo_drowning`), the blocks'
  `animateTick` sounds (`block.water.ambient` 26 times in `rewo_maps`; a lit furnace's
  `fire_crackle`) and the rain ambience (`weather.rain`, `rewo_sky_weather`). Vanilla plays all of
  these locally (`level.playSound(player, ...)`: the server sends them to everyone except the player).
  Rewo has no block `SoundType` table and none of these call sites. A feature to build.
- Server-sent sounds are right: 9/9 positional `playsound` events (`rewo_audio_distance`); the
  horse's and camel's steps, gallops and dash; the boat's paddling (8/8, within a tick; vanilla's own runs vary by a tick too); chest and barrel open/close (all 8).
  Pitches differ only by the server's random.

### HUD

- **Fixed (phase B 4).** No **offhand slot**; no **stack counts** in the hotbar (32, 16, 5, 8
  arrows); no **compass** icon (in the hotbar or the hand).
- **Fixed (phase B 4).** **Creative mode** still shows hearts and hunger (vanilla hides them).
- **Fixed (phase B 4).** **F1** hides only the sidebar, the tab list and name labels. Vanilla's `Hud.isHidden` also hides
  the crosshair, hotbar and bars, effects, boss bar, action bar, title, chat, the hand
  (`renderItemInHand`) and the block outline.
- **Fixed (phase B 7, 8).** No **attack-cooldown indicator**; no **"Press Left Shift to Dismount"** message when mounting.
- **Fixed (phase B 10; the HUD draws under screens since phase B 8).** The **crosshair** is plain white (vanilla's inverts what is behind it) and stays drawn over menus.

### World and entities

- **Fixed (phase B 10).** The **block outline** is a broken 1-pixel line; vanilla's is a solid line about 2 pixels wide.
- **Moved to M422** (`claude/autonomous-run-1026`). **Underwater** there is no fog or tint: vanilla goes dark blue, Rewo stays bright
  (`rewo_drowning`). Water surfaces are pale grey-blue (partly the colour-space issue).
- **Fixed (phase B 9).** No **entity shadows** (`rewo_horse`, `rewo_mobs`).
- **Fixed (phase B 9).** A **standing sign** draws its text with no board or post (`rewo_blocks`).
- **Fixed (phase B 9).** The **farmer villager** has no profession outfit (`rewo_mobs`).
- **Fixed (phase B 11).** A **map in an item frame** hangs about half a block lower than vanilla's (`rewo_maps`).
- Grass block sides under water show a bright green fringe at the pond's edge (`rewo_boat`).
- **Explained and fixed (phase B 8).** **Holding a block item:** the first-person block is mostly hidden behind the hotbar, and behind
  an open container a giant copy of the block fills the view (`rewo_containers`, `rewo_audio_blocks`).
- **Sky** (`rewo_sky_weather`): no sunrise or sunset glow and no dusk dimming; clouds are drawn
  although `options.txt` has `renderClouds:"false"` (**fixed, phase B 7**).
- **Fixed (phase B 11).** **Horse jump physics:** the horse jumps, but the rider's second-tick rise is 0.61 where vanilla's
  is 0.68, and the apex ends 0.15 blocks lower (`rewo_horse`, positions log). Worth checking
  against `AbstractHorse`'s jump in the decompile.
- **Fixed (phase B 11).** **In a boat** Rewo shows the arm while paddling and no oar pieces at the bottom corners (Opus,
  `rewo_boat`).
- **Arrows** (Opus, `rewo_bow`): none is visible in flight right after release, and a weak shot's
  arrow on the ground is small and dark. Vanilla shows both.
- **Partly fixed (phase B 11: ears, ridden walk; the dash rig is open).** **Camel** (Opus, `rewo_camel`): the head and ears don't animate while walking or dashing.

### Screens

- **Fixed (phase B 6).** Container screens draw **no title and no "Inventory" label** (Crafting, Chest, Barrel, Furnace,
  Loom, Stonecutter).
- **Not a Rewo bug (see Corrections).** The **furnace output slot is empty**: both furnaces smelted three raw iron (5 of 8 left), and
  vanilla shows 3 iron ingots in the output where Rewo shows nothing.
- **Fixed (phase B 6).** **No tooltips** over the stonecutter's recipe buttons or the loom's patterns. Item-slot tooltips
  work.
- **Partly fixed (phase B 6).** The **banner item has no icon** (loom banner and result slots), and the loom's **banner preview**
  is an empty panel; the pattern list, selection and scrolling are right.
- **Model fixed (phase B 6); the list is open.** **Recipe book:** with the book open, the player model is drawn at the old panel position on top
  of the book while the player box is empty. The first tab's icon and the craftable filter button
  are missing, and the book lists 29 pages where vanilla's lists 11 (recipes not grouped into
  collections?).
- The **carried stack** on the cursor seems not to be drawn in the stonecutter (Opus).
- **Fixed (phase B 8: left out under a screen).** The held-item name ("Map") is drawn **on top of an open inventory**.
- No **menu background blur**. The options screens draw no background at all, so the HUD and hand
  show through.
- **Options:** Rewo's Options screen has three buttons where vanilla has the FOV slider, World
  Options and a grid of ten. The pause menu lacks the icon-button row.
- **Fixed (phase B 12).** The inventory's player model and the first-person arm use **Steve**; vanilla uses the default skin
  the offline UUID picks.

### What matches

- **Physics:** the walk matches to four decimals on every tick; the boat to 0.019 blocks; horse and
  camel rides follow the same path within a block until the dismount.
- **Riding HUD:** the horse's hearts and the charged jump bar are pixel-identical at this scale.
- **Held maps** in both hands, one hand and the offhand, with the map's colours and player marker.
- **Air bar:** it drains bubble by bubble at vanilla's rate and pops with `ui.hud.bubble_pop` every
  30 ticks, but each pop comes 6-9 ticks after vanilla's (vanilla pops at 1, 29, 59 ... 264; Rewo at
  7, 38, 68 ... 244, so it has popped 9 times by tick 360 where vanilla has popped 10).
- **Screens:** the book (text, arrows, page counter, hover), the crafting table, the stonecutter's
  recipe grid and counts, the loom's pattern grid and scroll, and the options sound page. All
  button, slider, page-turn, stonecutter and loom sounds match (UI 5/5, stonecutter 2/2,
  loom 2/2, recipe book 2/2, book 3/3).

## Enni's lists

| item | scenario | state |
|---|---|---|
| ride a horse / a camel | `rewo_horse`, `rewo_camel` | works; HUD right; no dismount hint, no shadows; horse jump apex 0.15 lower; camel head static (Opus) |
| shoot arrows | `rewo_bow` | works, sticks; arrow not visible in flight (Opus) |
| float a boat | `rewo_boat` | works, path matches, inside of the hull dry; arm shown, oars missing, water colour |
| the HUD | `rewo_hud` | layout right; counts, offhand slot, compass, F1, creative bars wrong |
| button and slider click sounds | `rewo_ui_options` | 5/5 match; Options screen cut down |
| page turns | `rewo_book` | match (and now vary) |
| stonecutter and loom sounds | `rewo_stonecutter`, `rewo_loom` | match; tooltips, banner icons and preview missing |
| drowning bubbles | `rewo_drowning` | bubbles and pops match; no underwater fog; no hurt sound |
| recipe-book button | `rewo_recipe_book` | opens and closes, button sound; player model drawn over the book |
| crafting table and furnace | `rewo_crafting_furnace` | open and work; no titles; furnace output not shown |
| maps in one and two hands, tooltips, markers | `rewo_maps` | hand poses and markers right; frame map low, held-item name over the screen |
| loom pattern scrolling | `rewo_loom` | scrolls right |
| audio listening checklist (§4) | `rewo_audio_blocks`, `rewo_audio_distance`, all | variant variety: was broken, fixed; distance and stereo positions match for server sounds; the player's own sounds are missing; clip ends, glitching and music remain for ears |

## Opus was wrong here (checked against the logs)

- "The horse never jumps and the jump bar stays empty": the positions log shows a 3-block jump,
  and a crop of the bar is identical to vanilla's.
- "The zombie and skeleton burn in Rewo": undead catch fire at random in daylight during the ticks
  before `/tick freeze`, on separate servers. The scenario now gives them helmets.
- In the first batch, missed boardings and a misaimed furnace click were scenario bugs, now fixed.

Every Opus finding in this report was checked against a crop or a log unless it says "(Opus)".
