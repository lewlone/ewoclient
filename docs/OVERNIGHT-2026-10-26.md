# Autonomous run — 2026-10-26

Started at Control Room's request (on Enni's word, Enni asleep): resume the
Rewo queue from `docs/OVERNIGHT-2026-10-20.md` "Paused here", fix the
recurring flakes, look at Dependabot PR #6, and write a recommendation on
moving Rewo to 26.3. Branch `claude/autonomous-run-1026` from `main`
`234fa1bd` (the merge of the 2026-10-20 run and local main's McBuilder
commits, pushed 2026-10-25). **Nothing pushed.**

## For Enni (short)

*(Kept current through the run; the last update is the one that counts.)*

- **Done so far**: M402 (gliding and riptide-spinning players and
  mannequins), M403 (the elytra wings, never drawn before), M404 (a rider on
  a rearing horse sits where the server seats it), M405/M406 (banners and
  shields wear their patterns as icons, dropped, framed and held), M407
  (decorated pot items wear their sherds), M408 (the first-person hand
  draws skinned heads and patterned items), M409 (the ender dragon flies
  on its flight history: the audit's last open row), M410 (mannequins'
  "NPC" description and below-name scores), M411 (the cursor draws a
  carried stack's own copy), M412 (entities, items and name tags take the
  terrain's fog; they never had), M413 (the opt-in wavy cape gives way to
  the vanilla one on a tilted body), M414 (the inventory preview wears
  your armour and elytra), M415 (remote players' motion eases in as
  vanilla's does, instead of decaying like a mob's), M416 (baby zombies,
  husks, drowned, zombie villagers and piglins wear armour and elytras),
  M417 (horses rear: the body, head and legs, with the tail, saddle and
  chests on the body), M418 (enchanted armour and held items shimmer in
  the inventory preview), M419 (dropped and framed tridents, spyglasses
  and spears show their flat sprite, as vanilla's do), M420 (an enchanted
  item worn on the head shimmers), M421 (fog bands use vanilla's
  `linear_fog_value`; Rewo had a 1-block floor on their width), M422 (fog
  under water, in lava and powder snow, and under blindness and darkness,
  with water vision; Rewo had only the open-air fog), M423 (the
  first-person hand fogs with the world), M424 (render check r230: the
  live client under water draws the water's fog), M425 (and its clouds
  fade at the water's band, r231); the
  flakes (`srv` ports, r89, r214, r91, and the r329 class narrowed);
  CI's `audit` failure on main (two TLS advisories, lockfile only);
  Dependabot PR #6 diagnosed.
- **Found on the way**: since M400/M401 a swimming or sleeping player's
  armour, cape, worn head and held items stood upright beside the lying
  body (every layer had its own copy of the placement chain); r91's flake
  was a real bug (rearing was never modelled); main's last CI run failed
  `audit` and two render-check witnesses (see "Needs Enni").
- **Needs you**: the 26.3 decision (recommendation below). Nothing else
  blocking.

## Needs Enni

### Should Rewo move to 26.3? (recommendation)

**Yes, as one deliberate version-bump run, once you confirm what the Frogsy
network runs.** Rewo should support exactly one version at a time (it pins
the bundled jar's `version.json`); a dual 776/777 client is not worth its
cost.

Why move:

- Your own servers are moving: the friend server McBuilder targets runs
  vanilla 26.3 (protocol **777**), which the 26.2 client (protocol **776**)
  cannot join.
- McBuilder links `rewo-{data,world,mesh,gpu}` and is 26.3-only. Today the
  shared crates are split: the block tables are 26.3, `ao_facts_table` and
  everything else 26.2, and the client loads 26.2's `blocks.json` at run
  time with 26.3's extra names inert. One version removes that seam.
- 26.3 is a stable release (`version.json`: stable, world version 5023).

What it costs (measured from the two datagen reports and decompiles):

- **Packets**: three new clientbound (`post_effects` in configuration and
  play, `add_transient_block`, `swing_animation`), serverbound `swing`
  renamed `punch`; ~120 renumberings, free because Rewo resolves ids by
  name.
- **Data components**: 113 → 124. The largest wire task: an untranscribed
  component parks the `DataComponentPatch` reader (no length prefix), so all
  eleven need `component_wire` shapes before any slot packet is safe.
- **Entity metadata**: the base classes' accessor counts are unchanged
  (Entity 8, LivingEntity 7, Mob 1, AgeableMob 2, Player 4, Avatar 2,
  TamableAnimal 2), so existing indices hold. New entities: the cushion and
  the poplar boats.
- **Data**: 1196 → 1286 blocks (32366 → 35723 states), +121 items, +23
  sounds; every `tools/gen_*.py` table regenerated from the 26.3 decompile
  (the block generators already run on 26.3), and `ao_facts` ported to
  26.3's reworked `isViewBlocking` (`gen_ao_facts.py` refuses 26.3 today).
- **Verification**: every gate re-pinned (demo hash, mob views, counts), the
  render check against a staged 26.3 server, `load_check`.

Estimate: roughly 10 to 20 milestones of the size this file records, one
focused run. What decides "now or later" is the Frogsy network: if it is
still on 26.2, moving breaks the client there until it updates (the plan
keeps public servers out of scope, so Frogsy and the friend server are the
targets that matter). Staying on 26.2 meanwhile, as asked.

### main's CI on `234fa1bd`

- `audit` failed on two advisories published 2026-10-19/20
  (rustls-webpki 0.103.15, webpki-roots 1.0.9): fixed on this branch,
  lockfile only (`50130820`).
- The self-hosted `gates` workflow's render check failed r214 (the
  frame-hitching flake, fixed here), and r106 and r101 together: the use
  tail timed out in stage 77 with no firework rocket ever seen. Both passed
  in the two local runs of the same commit before the push; rerun pending
  (the GPU was held by another session's G4 run for most of this run).
  **Rerun three times on this branch: r106 and r101 passed every time**,
  so the CI failure did not reproduce here (the runner shares this desktop;
  a busier moment is the likeliest difference).

## What changed

| Commit | What |
|---|---|
| `637b3353` | **M402** — gliders and riptide spinners; every layer shares the body's frame; mobtexshot m116/m117, r224 |
| `21cdd7ec` | `srv` tests: the fake nameserver's UDP+TCP port is drawn below the ephemeral range |
| `50130820` | deps: rustls-webpki 0.103.16, webpki-roots 1.0.11 (CI `audit`) |
| `a1b30116` | dependabot: windows-rs minor/major bumps wait for a deliberate upgrade (PR #6) |
| `7bebbb7c` | **M403–M406** — the elytra; rearing horses' seats (r91's flake); patterned banners and shields as icons, dropped, framed and held; r89/r213/r214/r329 witnesses |
| `36b07250` | **M407** — decorated pot items wear their sherds; inventoryshot g24, r228 |
| `d8ac1c37` | **M408** — the first-person hand draws a stack's own copy; r229 |
| `c2ca650d` | **M409** — the ender dragon flies on its flight history; mobtexshot m119 |
| `334342ca` | **M410** — the below-name line (a mannequin's description, below-name scores); labelshot f17 |
| `3523cbfe` | **M411** — the cursor draws a carried stack's own copy |
| `00f7edcc` | **M412** — entities take the terrain's fog; mobtexshot m120 |
| `337d9ffd` | **M413–M415** — the wavy cape on a tilted body; the dressed inventory preview; remote players' motion lerp; capeshot y5, p6, p7 |
| `c5c3333e` | **M416** — baby humanoids wear armour and elytras (and M403's baby elytra offset); mobtexshot m121, m122 |
| `5fdc8e9e` | **M417** — horses rear; mobtexshot m123 |
| `6286f464` | **M418** — the inventory preview's glints; capeshot p8 |
| `294fadb5` | **M419** — dropped and framed tridents, spyglasses and spears are sprites; itemframeshot b6, p12, mobtexshot m124 |
| `0e3bd12e` | **M420** — an enchanted worn head item shimmers; mobtexshot m125 |
| `45994330` | **M421** — fog bands follow `linear_fog_value`; lightmapshot 9(e) |
| `fad13c31` | **M422** — the fog environments (water, lava, powder snow, blindness, darkness); unit tests |
| `6ff40fb0` | **M423** — the first-person hand takes the world's fog; handshot g8 |
| `773dbce8`, `528a982f` | **M424** — a live witness for the water's fog; render check r230 |
| (next) | **M425** — under water the clouds fade at the water's band; render check r231 |

## Flakes

- **`srv` port pairs**: random explicit ports below the ephemeral range
  (`21cdd7ec`). Measured here: pairing alone fails 2 in 3000, never twice
  in a row; the five-together failures point at the ephemeral range being
  briefly exhausted under the suite's load.
- **r214** (arrow under frame hitching): the 0.25 s window stretches to
  five frames, at most 0.6 s, still short of the second an unflown arrow
  hung for. Read 36/38 and 43/46 frames on the next runs.
- **r89** (camel step read twice): the per-tick step is the distance over
  the ticks it spans; a stalled frame no longer doubles it. 0.4101 both runs.
- **r91** (horse seat): a real bug, M404.
- **r329 class** (tail witnesses finding nothing staged near the bot):
  `REWO_PRECMD` now waits for the bot to stand and pins every `~` command to
  that one origin with `execute positioned`. That ruled out the staging
  theory: in one of three runs since, seven tail witnesses still missed,
  with the staging correctly at ground level and the bot itself standing
  3.2 blocks above the origin during the timed witnesses. Cause still open;
  the next sighting should log the bot's position when the lectern stage
  starts.

## Dependabot PR #6

Its `test`, `shipped` and `msrv` failures have one cause: the group bumped
ewo-jni's `windows-core` 0.62 → 0.100 without the `windows` crate, so two
cores were linked and the audio backend's `#[implement]` COM handler no
longer satisfied `Interface`/`IUnknownImpl` (E0277 ×20); `windows-core`
0.100 also needs rustc 1.95 over the 1.89 MSRV. Its `audit` failure is
main's (above). Its `zip` 9.0.5 and `arboard` bumps are fine. The PR is
left open; `.github/dependabot.yml` now holds back windows-rs minor and
major bumps, so the next weekly group arrives without it.

## Follow-ups found, not done

- ~~The horse family's rearing pose~~: M417 (adults). Left: eating
  (`eatAnimation`) and the baby horses' and donkeys' animation (their
  models are static in Rewo).
- ~~The wavy cape takes only the death topple~~: M413 (a tilted wearer
  draws the vanilla cape).
- ~~The inventory preview draws no armour~~: M414; its glints M418. Left:
  its trims (their sprites live in the world pass's pool).
- ~~Remote players' motion is applied directly~~: M415.
- ~~Baby humanoids wear no armour~~: M416. Found on the way: M403's baby
  elytra was 12 px too high (fixed there too).
- ~~Rewo has only the atmospheric (rain) environmental fog~~: M422 (water,
  lava, powder snow, blindness, darkness, water vision). Left: a live
  witness with the bot under water (M424: r230), the void
  darkness, the boss bar's world darkening, the open air's cloud fade
  (Rewo pins a 12-chunk deck where vanilla's range defaults to 128).
- ~~Entities take no fog at all~~: done in M412; the first-person hand
  in M423. Left: the hand's glint does not fade in fog.
- ~~A dropped or framed trident shows its 3D model~~: M419 (and the
  spyglass and spears).
- Corrected follow-up: the swirls (M285-M287) not reaching a glowing
  entity's outline is vanilla's behaviour (`energySwirl` and `breezeWind`
  set no outline property), not a gap.
