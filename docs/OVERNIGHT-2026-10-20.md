# Autonomous run — 2026-10-18 to 2026-10-20

An autonomous stretch while the owner was away (phone only), started by the
Control Room session alongside five other projects on the same PC. Branch
`claude/autonomous-run-1020-82937b`, fast-forwarded to the local `main` at
`e09f02f` (the end of the 2026-10-15 overnight push) with M194's unfinished
branch `claude/item-frames` merged in. Nothing was pushed and nothing was
released.

## For Enni (short)

*(Kept current through the run; the last update is the one that counts.)*

- **M194, item frames, is done**: frames draw their wood model, the item in
  them (turned in eighths) and a map in them (turned in quarters, with only
  the markers a frame shows). New offscreen gate `itemframeshot`.
- **M195**: the 25 animated block textures vanilla blends (magma, prismarine,
  sculk, command blocks, campfires...) now blend instead of cutting. New
  gate `animshot`.
- **M196 / M197**: water and lava sides use the flowing texture, and a
  flowing surface uses the flowing texture turned to the direction the
  fluid runs (rivers, waterfalls' edges). This changes the demo picture on
  purpose, in exactly the three flowing cells of its pool.
- **M198-M201**, smaller fixes: enchanted items shimmer in frames; water
  against glass, ice or leaves shows vanilla's overlay texture; a loom with
  a full (six-layer) banner hides its pattern grid; the recipe book's search
  box ignores a right click as vanilla does.
- **M202-M204**: F2 screenshots now go to the instance's own
  `screenshots` folder (vanilla's place; they used to go to
  `%APPDATA%/EwoClient/screenshots`) and are named in local time, not UTC;
  the happy ghast's four-rope leash is checked in the real windowed client;
  a named banner's label shows on a map in a frame.
- **M205, a real bug**: on a normal (not flat) world at render distance 16
  the terrain buffer filled up and about 40% of the loaded columns were
  never drawn — holes in the world until something unloaded. None of the
  checks could see it because they all use a flat world. The buffer now
  grows as needed; the same load test went from 197 missing columns to 0.
- **M206**: Rewo now uses the render distance from the instance's
  `options.txt` (vanilla's default is 12). Before, it always asked servers
  for 10 whatever was set, and the distance fog was a fixed band sized for
  the flat test world. The fog now follows the render distance as vanilla's
  does. Your skin-layer toggles and the "allow server listings" choice in
  `options.txt` are now sent to servers as well.
- **M207 / M208**: a new check, `tools/load_check.py`, loads a normal
  world at render distance 16 and fails on missing columns. Looking at that
  world turned up thin dark lines along block edges, cracks from an old
  vertex-saving trick (merging faces), and the trick is now off. Terrain
  uses about 13% more GPU memory and has no cracks, as in vanilla.
- **M209**: the sky is now drawn the way vanilla draws it. Before, a
  distant ocean or plain ended in a hard line against the sky; now the world
  fades into the sky with no edge. This changes the demo picture on purpose
  (only its sky pixels).
- **M210**: at render distance 32 (the game's maximum) the outer ring of
  the world would have been missing; the terrain buffer's ceiling is now
  high enough, checked with a full RD32 load (3,725 of 3,725 columns).
- **M211**: lighting on normal worlds. Huge mushroom caps let full
  daylight through them, and so did water over seagrass, kelp and every
  waterlogged block; both now match vanilla. The live light check can now
  tell Rewo's mistakes from quirks in the server's own light data (vanilla
  leaves a few inconsistent cells at chunk borders), so it works on real
  worlds. Walking the bot over hills and shores on nine worlds: the server
  never had to correct its position.
- **M212**: `tools/load_check.py --light` makes that light-and-walking check
  repeatable. Two light sources had been missing: deepslate redstone ore
  (when lit) and the calibrated sculk sensor.
- **M213**: a named banner's label now shows on a map held in the hand too
  (M204 did maps in item frames).
- **M214, a crash fix**: opening the advancements screen would have
  crashed Rewo (it ran out of memory) as soon as an advancement whose text
  contains a line break was on display. A new live check of the advancement
  tabs found it on its first run. Fixed, and the tab clicks are now checked
  against the real server.
- **M215**: the lectern (open, turn a page, close) is now checked against
  the real server in the windowed client; it worked as built.
- **M216, a missing basic feature**: clicking a mob or villager did
  nothing. Rewo could not attack entities or interact with them (no
  trading, no riding by hand, no name tags or leads). Both now work with
  26.2's packets and are checked against the real server: right-clicking a
  villager opens its trades exactly as summoned, and left-clicking it is a
  hit the server registers.
- **M217, the biggest gap**: in survival mode Rewo could not mine
  anything; holding the mouse button did nothing. Block breaking now works
  as in vanilla (block hardness, tools, efficiency, haste and so on) and is
  checked against the real server in survival. It went unnoticed because
  every test server runs in creative, where blocks break instantly.
- **M218**: Q drops an item, F swaps hands, middle-click picks a block, and
  sprinting is now told to the server; each is checked live. Building those
  checks found a real bug: after pressing a number key, Rewo kept treating
  the previous slot as the held item (in hand, for mining and attacking)
  while the server had switched. Fixed.
- **M219**: you can leave a bed. Rewo did not know the player was asleep,
  so it showed no in-bed screen (no Leave Bed button) and the walk keys
  still worked in bed. Now the in-bed chat appears, Leave Bed and Esc wake
  you, and you can't walk while asleep; checked live against the server.
- **M220**: horse, donkey, llama and nautilus inventories open (E while
  riding, or sneak-right-click), with their saddle and armour slots and chest,
  and shift-click works as in vanilla; checked live on a donkey. Building it
  found a visible old bug: every chest, furnace and other container screen
  had your player model painted over its slots. Fixed.
- Nothing needs a decision from you yet.

## What changed

| Commit | What |
|---|---|
| `8b3cdb5` (merge) | `claude/item-frames` (M194 slices 1-2 and its plan) merged onto `main`'s tip. |
| `a7be406` | **M194** slices 3-6: the FIXED display transform on every item model, the four frame block models, the framed-map band in the entity atlas, the collector and draw builder (`live_cmd/item_frames.rs`), no capsule for frames, and the `itemframeshot` gate (21 witnesses). |
| `185fcf6` | render-check **r69** (M192's held map, injected into the off-hand), **r70** (an injected frame), **r71** (a frame the real server summons: metadata slots 8-10 calibrated against 26.2); `capeshot`'s two atlas-layout rows updated for the new band; §15 entry. |
| `e8dbae7` | §0.0 numbers after M194. |
| `cf38cbe` | **M195** — animated textures interpolate (`render-misc [INTERP]`): the bake reads `.mcmeta` `interpolate`, `anim_tick` blends each frame into the next per mip on every tick (vanilla's `animate_sprite_interpolate.fsh` on bytes). New gate `animshot`, which reads the uploaded layer back from the GPU. |
| `807cf47` | **M196** — fluid sides draw the top-left quadrant of the 32 px flow sprite (all a side samples), as an animated 16 px layer interned after the main bake so no layer index moves. |
| `8d3ce51`, `eb694ad` | **M197** — flowing fluid tops: a 32 px flow texture array on terrain binding 2, picked by the layer's high bit; the mesher reuses physics' `getFlow` for the angle; demo hash rebaselined. |
| `c33336c` | **M198** — the glint on a framed enchanted item (`itemframeshot` p10). |
| `72e0ab5` | **M199** — `water_overlay` on a water side against glass, ice, slime, honey, grates or leaves; the 42-block set generated by `gen_block_props.py` and held to the report's codec types. |
| `c203b77` | **M200** — the loom's `hasMaxPatterns` (banner layer count decoded from `minecraft:banner_patterns`; `containershot` o30). |
| `15ba587` | **M201** — the recipe book's search box and the right button (magnifier vs box). |
| `7fbd115` | This summary, through M201. |
| `df3619b` | **M202** — F2 screenshots: `<game dir>/screenshots` (vanilla's `Screenshot.grab(gameDirectory)`), file names in local time via `GetTimeZoneInformation` on Windows. |
| `461ceb4`, `36b1e79` | **M203** — render-check **r72**: an injected horse leashed to a happy ghast draws M185's quad leash in the windowed client; §0.0 numbers. |
| `a3f7985` | **M204** — banner name labels on framed maps (`MapRenderer`'s `decoration.name` branch; `itemframeshot` p11). |
| `3ce9381` | Fixes from an independent review of M194-M204: per-species quad-leash attach points (horse, chested, camel, sniffer, boat), the flow array installed in `view` / `bench`, animation-upload bookkeeping recorded only after success, an arena-fill log line. |
| `5160a16` | **M205** — the terrain arena grows (4M vertices initially, up to 64M) instead of deferring columns; plus a clippy fix the review commit had let through. |
| `029f454` | §0.0 numbers after M205; this summary through M205. |
| `2463025` | **M206** — `renderDistance`, `modelPart_*` and `allowServerListing` from `options.txt` into `client_information` (was a hard-coded view distance 10); `FogRenderer.setupFog`'s band from the effective distance, measured cylindrically; render-check **r73** with a staged `renderDistance:6`. |
| `5e9c687` | §0.0 numbers after M206; this summary through M206. |
| `af429b2` | **M207** — `tools/load_check.py`: a normal world at render distance 16; fails on a column count other than the server's tracking set (1,057), any deferred column, a refused arena growth or a validation message. |
| `12e8d22`, `1dc1bc3`, `b719c12` | **M208** — greedy face merging off in production (`MERGE_FACES`): its rectangles left T-junction cracks. A new mesher test finds T-junctions geometrically (18 before, 0 after); the merge code stays tested. `b719c12` fixes two clippy errors `12e8d22` went in with. |
| `9587057` | `live --out` is rejected beside `--run-seconds` / `--render-check` (it was silently ignored beside the first and replaced the windowed check beside the second). |
| `11d4ba2`, `a5d4ee9` | This summary through M208. |
| `98e720c` | **M209** — vanilla's sky disc: `SkyRenderer`'s octagonal fan evaluated per pixel with its interpolated fog distances, the uncovered sky in the fog colour, the dark disc below the horizon height; offscreen defaults are plains' sky/fog pair; new `REWO_EYE` / `REWO_YAW` knobs aim the headless shot; demo hash `748e5a1e...` to `e658532b...` (the diff is exactly the sky). |
| `97bda97` | This summary through M209. |
| `16f7eeb` | **M210** — arena ceilings 112M vertices / 168M indices, capped by the device's largest buffer; `load_check.py --rd 32` PASSED (3,725 / 3,725 columns; the run held 64.72M vertices and 97.08M indices, past M205's ceilings). |
| `b34630e` | This summary through M210. |
| `586e216` | **M211** — light dampening from the block's shape and fluid state (huge mushroom caps were sky-transparent; seagrass, kelp and waterlogged states let sky light through water); `play --light-check` classifies disagreements that trace to the server's own data (vanilla's chunk-border source order, stale light) and prints neighbours `server -> recomputed`. |
| `86c8e4e` | §0.0 numbers after M211; this summary through M211. |
| `f538b32` | **M212** — `load_check.py --light` (play bot on a normal world: CORRECTIONS 0 and the light check); `gen_block_light.py` follows `ofLegacyCopy` / `ofFullCopy` (lit deepslate redstone ore 9, calibrated sculk sensor 1; amethyst buds and filled cauldrons non-occluding). |
| `ff07ab8` | `load_check.py` closes the server log before removing its directory (Windows had left 54 empty server folders). |
| `99d32a5` | **M213** — banner labels on a held map: glyph cells and a translucent background cell in the hand atlas, a translucent path in `hand.frag` selected by a negative shade, `handshot` gm7. |
| `e6ef088` | **M214** — render-check **r74** (an advancement tab click the real server echoes); the advancements screen no longer loops forever on a description with a newline (it crashed the client). |
| `3d50b1b` | This summary through M214. |
| `1ca048d` | **M215** — render-check **r75**: a lectern opened by the production right-click, page turned by the server's echo, closed with `container_close`; `lectern_arrow_click` extracted from the mouse handler. |
| `13ca696`, `d9deb3d` | `load_check.py` refuses `--rd` above 16 without `--heavy` (and with under 10 GB free); M214's wrap-loop audit. |
| `2cbd426` | **M216** — attacking and interacting with entities (26.2's `attack` and `interact` packets, the hit location as LpVec3); render-check **r76** (a villager's trades, decoded as summoned) and **r77** (an attack the server answers with damage). |
| `c5e7510` | This summary through M216. |
| `4c97c99` | **M217** — survival block breaking: generated block hardness (1,196 blocks) and item tool tables, vanilla's destroy progress and start/continue/stop machine, the held attack key; render-check **r78** (leaves broken in survival on vanilla's tick, the server agreeing). |
| `485036b` | This summary through M217. |
| `16921fd` | **M218** — Q drop (predicted), F swap hands, middle-click pick block/entity, sprint start/stop sent; `select_hotbar` now sets the client's held slot (it never did); render-check **r79-r82**. |
| `41078c4` | This summary through M218. |
| `7517ce5` | This summary through M219. |
| *(M220)* | **M220** — the horse inventory: `mount_screen_open` decoded (class C closed), mount menu layouts per column count with inactive slots, the mount slots' `mayPlace` from `allowedEntities` (new `EQUIP_ALLOWED` table), vanilla's quick-move, the horse/nautilus screen, E sends `OPEN_INVENTORY` while riding; the player preview no longer drawn over containers; render-check tail with r83 moved in, **r84-r86**. |
| `26559a9` | **M219** — sleeping: `SLEEPING_POS_ID` (index 14) decoded for the local player, moved onto the bed, immobile while asleep; `InBedChatScreen` (Leave Bed button, Esc wakes, `closeOnSubmit` false) driven by `Gui.tick`'s rule; render-check **r83**. |

## Decisions made

1. **The frame model and the framed item ride the block-entity emitter**
   rather than a new draw path: both are static quads at a pose, which is
   what `BlockEntityDraw` already is. The item's FIXED transform is applied
   by the emitter from the model it draws (`fixed_display`), the way the
   dropped-item path applies `ground`, so the app never needs the transform.
2. **A framed map gets its own band in the entity atlas** (128 rows at the
   bottom, M48's recipe, every earlier address unchanged) with seven LRU map
   slots. Past seven framed maps on screen at once the rest draw no pixels; a
   recorded deviation, since vanilla's texture manager has no cap.
3. **Recorded M194 deviations**: no enchantment glint on a framed item; the
   glow frame's map light is taken at block level 13 rather than 13.125; no
   name tag for a framed item with a custom name.
4. **r69 puts the map in the OFF-hand**, so the main-hand witnesses keep their
   sword; the map is then drawn one-handed.
5. **r71 adds one staging command** to `tools/render_check.py`: a `Fixed`
   item frame summoned on the test server. It is the only way to calibrate
   the frame's polymorphic metadata slots against a real server rather than
   the decompile's slot count.
6. **Windowed runs are allowed this time** (the 10-15 run skipped them for a
   recording batch). A note in the shared notes folder tells the other
   sessions when Rewo's GPU checks run.
7. **Work stays on this branch; local `main` is untouched.** The 10-15 run
   merged into `main`; this time `main` is checked out in the main worktree
   and the brief said to merge into this branch. To take the run: `git merge
   --no-ff claude/autonomous-run-1020-82937b` on `main`.
8. **M195's clock**: vanilla keeps per-sprite counters from load; Rewo
   derives them from the session tick (`subFrame = tick % frametime`). Every
   interpolating block texture has one uniform frametime, so the blend is the
   same; only the phase differs, which nothing can see.
9. **M197, the decision the plan had left open**: the flowing top samples
   about 23 px of a 32 px sprite, which a 16 px layer cannot hold. Chosen: a
   separate 32 px texture array for the flow sprites, picked by the layer's
   high bit, sampled in a branch on a flat input. Rejected: halving the
   sprite (half the texel density on every flowing surface) and raising the
   whole block array to 32 px (every consumer of `TEX_SIZE` changes, the
   held-item bake among them). Reversible: the bit, the binding and the bake's
   flow layers are the whole change.
10. **`getFlow` exists once**: the mesher calls physics' `fluid_flow` through
   an adapter rather than transcribing it again; only `Mth.atan2` had to be
   ported a second time (rewo-mesh cannot see rewo-gpu), with a test that
   holds the two ports to the bit.
11. **The demo hash moved on purpose** (`d7dc9e89...` to `748e5a1e...`); the
   pixel diff against the old binary is confined to the three cells that
   flow.
12. **M200: the loom's error sprite is drawn although 26.2 ships it fully
   transparent.** The blit is what vanilla submits; the visible effect is the
   hidden grid, which the gate grades.
13. **M202 moved where screenshots land** (reversible, one function in
   `capture.rs`): vanilla writes into the game directory, and Rewo now has
   one (the launcher's instance folder, or `--game-dir`). Screenshots taken
   before M202 stay in `%APPDATA%/EwoClient/screenshots`; nothing moves them.
14. **Two review findings kept as recorded deviations** rather than fixed:
   the water overlay is drawn double-sided as vanilla's is but without
   vanilla's per-face back-face pairing order, and `blocks_motion` in the
   flow adapter is the physics crate's approximation. Neither changes a
   pixel any gate or the render check can see.
15. **M205's growth blocks** (one device drain and one buffer copy, twice
   per session at RD16 on the load test) rather than streaming into a second
   buffer: simpler, and a hitch once is better than holes. Limits 64M
   vertices (1.79 GB) / 96M indices; past them, or if the GPU refuses the
   memory, columns wait as before.
16. **M206 reads three client-information fields and pins the rest.**
   `mainHand`, `chatVisibility` and `particles` are sent as what Rewo draws
   (right arm, full chat, all particles), not as the file says: reading
   `mainHand:"left"` would make everyone else see a left-handed player while
   Rewo draws a right arm. Reversible once Rewo draws the left-handed case.
17. **The render check's client now has its own game directory** (a scratch
   folder inside the test server's), so its `options.txt` is staged rather
   than whatever sits in the repo root.
18. **M208 turned greedy meshing off rather than fixing it in place.**
   The merge saved 13% of vertices on natural terrain; making it crack-free
   means emitting each rectangle's boundary at every block step, which is
   real work for a small saving. Reversible: one constant, with the merge
   path still covered by its tests and the `meshshot` oracle.
19. **M209 mixes the sky's fog in linear space** like the rest of Rewo's
   fog (vanilla mixes in gamma space), so both sides of the world's edge
   fade to the same colour. The dark disc is drawn inside the sky pass, i.e.
   before the sunrise fan rather than after it (a recorded deviation).
20. **M211's light check excuses two classes of disagreement**, both
   derived from the decompile and both checkable in the server's own data:
   vanilla's order-dependent sky sources at chunk borders, and stale light
   that `isEdgeOccluded` rules out. Everything else, and every block-light
   disagreement, still fails. Each class is counted in the output.
21. **Not fixed, noted:** several older `containershot` witness messages have
   runs of spaces where a shell joined a backslash line continuation (the
   same artifact this run hit and fixed in its own messages). Cosmetic, in
   gate output only.
22. **The horse inventory is its own milestone (M220), not part of M219.**
   The key half is one packet (`OPEN_INVENTORY`), but Rewo had never handled
   the server's answer, `mount_screen_open` (a class-C packet in the coverage
   table): the menu, its slot layout and the horse screen are all new.
   Sending the key without them would have made E on a horse do nothing
   instead of opening the player's inventory, so the key change waits for
   the screen.
23. **r83 runs first, before the options pages**, because a sleeping player
   is immobile and the riding-jump check (r72, 0.32-0.46) needs the jump key,
   and every later slot already has a screen up. The night it needs lasts only
   the round trip (well under a second), and the bot returns to its spawn
   point afterwards so later checks see the world they were written for. Not
   modelled, recorded: the lying-down camera, and gravity while asleep (the
   client settles 0.125 onto the bed; the server allows it).
24. **M220 moved r83 into a new render-check tail** (after the timed 8 s,
   capped at 3 s more). Its first home before the options pages kept missing
   its 0.11 deadline on slower runs, and its teleport there is the likeliest
   cause of r78's two flakes (none in the six runs since the move). The tail
   costs about 1.5 s per run and holds r83-r86.
25. **The horse screen's window stays empty** instead of showing the mount
   (vanilla draws it there, following the mouse). Building a single entity's
   draw outside the world pass is a refactor of `collect_entities`; it is
   proposed as follow-up work rather than folded in here.

## Verification

- `cargo test --workspace --no-fail-fast --features rewo-app/gates`:
  4,253 passed, 0 failed (after slices 3-6; baseline at `e09f02f` was 4,244).
- `python tools/gates.py`: 49/50 after slices 3-6, the one failure being
  `capeshot`'s atlas-layout rows, which encoded the old bottom band; updated
  in `185fcf6`. At `185fcf6` (`tools/numbers.py`): **4,253 tests passed,
  0 failed; gates 50/50** (demo hash unchanged, `d7dc9e8985f0504f`); clippy
  `-D warnings` and `cargo fmt --check` clean.
- `python tools/render_check.py` (a windowed run against a fresh local
  vanilla server): **71/71**, including r68 (the 10-15 recipe-book button,
  never run before), r69, r70 and r71.

- After M205 (at `5160a16`): **4,269 tests passed, 0 failed; gates
  51/51** (demo hash `748e5a1e9bbfbf10`, unchanged since M197); render check
  **72/72**, validation-clean; clippy `-D warnings` and `cargo fmt --check`
  clean.
- After M206 (at `2463025`): **4,276 tests passed, 0 failed; gates 51/51**
  (demo hash unchanged); render check **73/73**, r73 matching on 5,605
  frames (sent 6, server radius 8, 213 of 213 tracked columns, fog 86.4..96);
  clippy (gates and default features) and `cargo fmt --check` clean.
- After M208 (at `1dc1bc3`): **4,277 tests passed, 0 failed; gates 51/51**
  (demo hash unchanged); render check **73/73**; `load_check.py` PASSED
  (1,057 / 1,057 columns, 0 deferred, 19.1M vertices in a 32M arena).
- After M209 (at `98e720c`): **4,277 tests, 0 failed; gates 51/51** (demo
  hash `e658532bcc8c2758`); render check **73/73**, validation-clean.
- M210: `load_check.py --rd 32 --seconds 240` PASSED, 3,725 / 3,725 columns,
  0 deferred; 66 fps average, p99 31.9 ms in the debug build with validation.
- After M211 (at `586e216`): **4,278 tests, 0 failed; gates 51/51**;
  render check **73/73**; `play --light-check` on nine normal-world seeds:
  0 unexplained sky disagreements after the fixes (two seeds exact), and
  CORRECTIONS 0 on every run. One block-light cell (seed 5) did not recur
  on rerun; recorded, not explained.
- After M219: **4,291 tests, 0 failed; gates 51/51** (demo hash unchanged,
  `e658532bcc8c2758`); render check **83/83** on two consecutive runs (a
  first run read 82/83 with r78's survival mining late on its 0.28 deadline,
  passing on the rerun; r83's first version waited for the server's clock
  echo, which is periodic, and timed out once before the click retry);
  clippy (gates and default features) and `cargo fmt --check` clean.
- After M220: **4,301 tests, 1 failure under full-suite load** (`reconfigure`'s
  `start_configuration_round_trips_back_to_play`, "connection closed"; it
  passed five times alone and is a timing test like the `play::inbound` ones);
  gates **51/51** (demo hash unchanged); render check **86/86** on two runs
  (after fixing three of the new witnesses' own aim problems, see the §15
  entry); clippy (gates and default features) and `cargo fmt --check` clean.
- M205's load test is a scratch script, not a gate (it needs a 3 GB
  server and minutes of world generation): a normal world, seed 20261018,
  render distance 16, 75 s. Before: `276 columns, 197 deferred` at 94% of
  4M vertices. After: grown to 16M, `473 columns, 0 deferred`, no
  validation messages.

## Needs Enni

- Nothing blocking. Still open from before (listening pass, the release
  zip on a fresh machine, the HUD's Java side in a running Minecraft): see
  `docs/OVERNIGHT-2026-10-15.md` "Needs the owner".

## Follow-ups found, not done

- ~~Foil on framed items~~ (M198). ~~Banner labels on framed maps~~ (M204).
- ~~Banner labels on a held map~~ (M213).
- One block-light disagreement on a normal world (571 cells of a deepslate
  cave one level brighter than the server, seed 20261018) is unexplained and
  did not recur in eleven reruns; the light check now prints the emitters
  around such a case, so the next sighting names its source.
- ~~A live witness for M179's tab clicks~~ (M214, r74). ~~M184's lectern page
  sends~~ (M215, r75).
- ~~A normal-terrain load test as a real check~~ (M207, `load_check.py`).
- Other everyday actions found unwired by the same sweep (each is a
  `player_action` or `player_command` the client never sends, or a packet it
  has no id for): ~~dropping items (Q), swapping hands (F), middle-click
  pick block, sprint start/stop~~ (M218), ~~leaving a bed~~ (M219), ~~the
  horse inventory~~ (M220); still open: riding boats/horses (`move_vehicle`,
  `paddle_boat`), next.
- The mount in the horse screen's window, and empty-slot icons (the saddle
  and armour outlines vanilla draws in an empty equipment slot, the player's
  armour slots included).
- `rewo-net`'s `reconfigure` test `start_configuration_round_trips_back_to_play`
  failed once under full-suite load ("connection closed") and passed five
  times alone; the same class as the `play::inbound` timing tests.
- The sleeping camera: vanilla lays the view down in the bed (eye height 0.2,
  turned to the bed's facing). Rewo keeps the standing camera while asleep.
- `rewo-net`'s three `play::inbound` timing tests failed once under heavy
  machine load (they passed alone and in every other full run).
- More live witnesses for screens that only have offscreen gates. M214's
  first live run of the advancements screen found a crash that every
  offscreen gate had passed (they fed it synthetic text without a newline).
  Candidates: anything whose input text or data comes from the server.
- A crack-free greedy merge (rectangle boundaries tessellated at every unit
  step), if the 13% of vertices is ever worth it.
- ~~The headless `live --out` shot can't be aimed~~ (M209's `REWO_EYE` /
  `REWO_YAW` knobs). The original note: vanilla spawns a new player
  at a random spot. A console `tp` from the scratch script never reached
  the server (its log shows no command at all), so this is the script's
  stdin handling, not the client; a gates knob that stages the camera
  would be the clean way.
