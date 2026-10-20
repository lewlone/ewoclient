# Autonomous run — 2026-10-18 to 2026-10-20

An autonomous stretch while the owner was away (phone only), started by the
Control Room session alongside five other projects on the same PC. Branch
`claude/autonomous-run-1020-82937b`, fast-forwarded to the local `main` at
`e09f02f` (the end of the 2026-10-15 overnight push) with M194's unfinished
branch `claude/item-frames` merged in. Nothing was pushed and nothing was
released.

## For Enni (short)

*(Kept current through the run; the last update is the one that counts.)*

> **Headline (2026-10-20 18:07 checkpoint):** M194–M345 (152 Rewo
> milestones, ~330 commits, nothing pushed). Themes: displays, item frames,
> heads and worn blocks, steering vehicles, missing player actions, about
> thirty baby-mob models, and lately mob variants (collars, black cats, white
> horses, parrot, fox and rabbit coats). Figures at `43217330` (M345): 4,496 tests,
> gates 51/51, render check 199/199 (from ~67). Needs you: nothing blocking;
> the listening pass, a fresh-machine release zip and the in-game HUD are
> still yours. Next: rabbit coats and Toast (M346, in verification), the
> brown mooshroom, then the mob-held block layers.

### In one minute

- **What happened**: 169 Rewo milestones, M194 to M362, in about 361 commits on
  `claude/autonomous-run-1020-82937b`. Nothing was pushed or released, and
  the local `main` is still at `e09f02f`.
- **Checks at the end** (`26f17d53`): 4,528 tests pass (4,244 at the
  start); all 51 offscreen gates pass; the live render check passes 214 of
  214 witnesses (about 67 at the start); clippy and `cargo fmt --check` are
  clean.
- **Real bugs you would have seen, now fixed**:
  - holes in the terrain on normal worlds at render distance 16 (M205);
  - every sign's text drawn upside down (M263);
  - invisible entities drawn whole, so server holograms showed an armour
    stand under the text (M271);
  - block displays drawing the wrong block (the M260 fix);
  - the render distance option ignored (M206);
  - inventory icons lit wrongly (M245);
  - the rider's seat in the wrong place (M228);
  - every tamed or owned mob losing the data after its owner, so a tame
    cat's or wolf's coat (and the wolf's collar) never arrived (M342);
  - black cats drawn tabby and white horses drawn brown, because the server
    never sends a mob's default variant (M340);
  - every adult cat drawn a quarter too big (M343).
- **New, broadly**:
  - item frames;
  - blended animated textures and flowing water and lava;
  - text, item and block displays (interpolation, brightness, shadows,
    background plates);
  - end crystals, small projectiles, evoker fangs, swelling TNT, posed
    armour stands and minecart contents;
  - player heads in their own skins (placed, worn and as icons), and skulls,
    pumpkins and blocks worn on the head;
  - the enchanting table's book;
  - creepers that swell and flash before they blow, and lit TNT that blinks;
  - the Glowing effect's outline, in team colours, through walls;
  - empty-slot icons in the inventory and container screens;
  - flames on burning entities;
  - the vanilla vignette (dark corners in dark places, red near the border);
  - the pumpkin view, spyglass scope, freezing frost and sleep fade;
  - steering boats, horses, camels and the nautilus;
  - everyday actions that were never sent (drop, swap hands, pick block,
    sprint, leave bed);
  - baby animals and monsters as their own models (M307, M314 to M337):
    about thirty kinds, from calves, lambs and piglets to baby zombie
    villagers, hoglets and camel calves. Before, most were a half-size adult,
    and ten had their skins scrambled onto the adult's shape (since M175);
    nine of those are fixed, the armadillo's is not (Follow-ups).
- **Test harness**: the render check's old "damage from an unknown source"
  flake was slimes that the check's own `kill` command split; fixed in
  `5f8c2e9a`. Two new render-check flakes showed up on 2026-10-20 (each once,
  each green on a rerun with no client change): nine tail witnesses missing
  their staged blocks and mobs (M329), and r91's horse seat (M333). Both are
  under Follow-ups.
- **Needs you**: nothing blocking. One incident on the shared PC was
  reported at the time (see "Needs Enni"). The checks only you can do are
  unchanged: the audio listening pass, the release zip on a fresh machine,
  and the HUD in a running Minecraft.

### Milestone by milestone

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
- **M221**: you can steer a boat (W/S, A/D), the way vanilla does it: the
  client moves the boat and the server checks it. Checked live: the server
  accepted every move and agreed exactly where the boat ended up. Rewo has no
  boat model yet (boats are a placeholder blob), and sitting in one used to
  black out the view; that's fixed for now, and a real boat model is next.
  Horses next after that.
- **M222**: you can ride horses (also donkeys, mules, skeleton and zombie
  horses): walk, strafe, back up slowly, and jump with the jump bar, as
  vanilla does. Checked live: a horse walked twelve blocks at vanilla's speed,
  jumped three blocks high, and the server agreed exactly where it ended up.
  Camels and the nautilus are not done yet.
- **M223**: boats have their real models now (all twenty: every wood, the
  bamboo raft, and the chest versions), instead of a placeholder blob.
  Sitting in one shows the hull around you. The paddles don't row yet, and
  water can show inside a floating boat (vanilla masks it out; not done).
- **M224**: the mining check that failed about one run in five is fixed, and
  the client was not at fault. The spawn's flood water sometimes reaches the
  test's leaves, so after the client breaks them the hole fills with water
  within a tick, and the check only accepted air. Found by asking the server
  directly. Ten runs in a row are clean now. A second flaky check (r20) had
  the same kind of mistake and is fixed too.
- **M225**: you can ride camels: walk, sprint (which only a camel allows, as
  in vanilla), and dash with the jump bar, which then cools down. Checked
  live: the sprint speed, the dash and the cooldown all match vanilla's
  numbers, and the server agreed exactly where the camel ended up. Also
  fixed on the way: Rewo used to tell the server you were sprinting while on
  a horse or in a boat, which vanilla never does.
- **M226**: a correction to M222 and M225. Horses and camels rode 2 % faster
  than vanilla, and boats, horses and camels all reacted to the keys one
  tick early. Vanilla's vehicle uses the rider's slightly scaled input from
  the tick before. The checks had compared against a formula I derived
  with the same mistake, so they passed. They now pin vanilla's real speeds
  (horse 0.6476, camel 0.4101 a tick), and both match.
- **M227**: you can ride a nautilus underwater: it swims where you look
  (up and down too), turns smoothly, and dashes with the jump bar. Checked
  live against vanilla's numbers, and the server agreed exactly where it
  ended. On the way: horses and camels now float up in deep water while
  ridden, as in vanilla, and a crash in rainy weather far from the world's
  center (debug builds only) is fixed.
- Every mount is done: horses, donkeys, mules, camels, the nautilus, boats.
- **M229** (Control Room's new list: everyday survival actions): right-click
  now works the way vanilla's does. Before, you could only eat or block
  while looking at the sky, and buckets, fishing rods, snowballs, ender
  pearls and fireworks did nothing at all. Checked live in survival: eating
  bread while looking at the ground, filling a bucket from water and
  pouring it out, throwing a snowball, casting and reeling a fishing rod.
- **M230**: you can now aim at buttons, levers, flowers, torches, signs,
  crops and rails. Before, the crosshair looked straight through anything
  you can walk through. Checked live: pressing a lever and a button while
  holding food presses them (and doesn't eat).
- **M231**: eating, drawing a bow and blocking now slow you to a fifth of
  your walking speed, as in vanilla (spears excepted). Before, you walked at
  full speed. Checked live: the ratio is exactly 0.2.
- **M232**: crafting tables and furnaces work end to end, checked with real
  blocks in survival. That found a real bug: anything you took out of a
  chest, a crafting table or a furnace went missing from your hotbar and
  inventory after closing the screen (the server had it; Rewo didn't show
  it). Fixed.
- **M233**: the view now widens when you sprint, fly or have Speed, and
  narrows when you draw a bow or look through a spyglass, eased tick by tick
  as in vanilla. Before, the FOV never moved. Checked live: a full bow draw
  narrows the view to exactly 0.85.
- **M234**: a bow you draw now shows its pull frames, a crossbow shows
  loading and then the loaded arrow or firework, and a goat horn is raised
  while it plays, in your hand and in other players' hands. Before, they
  never changed. Checked live (the drawn bow's model) and in the item gate
  (every frame, at vanilla's thresholds).
- **M235**: shield blocking checked live in survival. It already worked:
  a hit from the front costs nothing with the shield raised, and the same
  hit without it costs health. No fix needed.
- **M236**: **the enchanting table could not be used at all.** No item or
  lapis could be put on it: Rewo refused every click on that screen
  without telling the server. Fixed, and checked live: a real table
  enchanted a pickaxe for one lapis and one level. The anvil now shows its
  "Enchantment Cost" line (red when you can't afford it) and renaming works
  (checked live).
- **M237**: **the brewing stand could not be used either** (the same bug).
  Fixed and checked live: a real stand took a water bottle, nether wart
  and blaze powder (shift-clicked, sorted into the right slots as vanilla
  does) and started brewing.
- **M238**: armour stands now look like armour stands (they were a grey
  capsule), with their arms and base plate shown or hidden as set, small
  stands half size, and armour worn on them. Custom poses (raised arms and
  so on) still show the default pose.
- **M239**: **firework rockets now boost elytra flight.** Before, using a
  rocket while gliding did nothing (in vanilla the push is applied by the
  client, and Rewo never did). Checked live: the glide went from 0.17 to
  1.67 blocks a tick and the server accepted it.
- **M240**: thrown snowballs, eggs, ender pearls, potions, fireballs,
  flying rockets and the fishing bobber now show as their items/sprite
  facing you, instead of grey capsules. (Arrows and tridents still are
  capsules.)
- **M241**: the fishing line is drawn, from the bobber to your hand (and to
  other players' hands).
- **M242**: chests, shulker boxes, mob heads and copper golem statues have
  inventory icons (they were blank). Fixing that turned up two older bugs
  that affected every 3D icon. The icons were drawn inside out (a furnace
  showed its back) and leaned on an edge instead of standing on a corner.
  The cause was a rotation applied in the wrong order. The same mistake also
  tilted items held in third person and turned sideways shulker boxes to
  face south. All fixed; the icons now look like vanilla's, grass top
  included.
- **M243**: beds, decorated pots and conduits have icons too, and a bed in
  someone's hand is drawn. Still blank: banners, the shield and the
  trident.
- **M244**: banners have icons in their colour (patterns not yet). A
  placed copper golem statue in its standing pose was drawn 1.5 blocks
  underground; fixed.
- **M246 (important)**: chests, shulker boxes, banners, signs, heads, pots
  and every other block entity were **invisible in the game window** since
  they were added (late July). Their textures are bigger than the 16×16
  slots the entity renderer paged in, so nothing was drawn; every check
  looked at the draw list, none at pixels. They draw now (checked on
  pixels: a placed chest, a held chest, a held shield). Shields and tridents
  also show in hands now.
- **M247**: and a chest, banner, head or other block entity **placed while
  you play** did not exist on the client at all until the chunk reloaded.
  Vanilla's client creates it itself; Rewo now does too. The render check
  now places a chest and checks that it draws in the game window.
- **M248**: bells now have their bell, and lecterns their open book (both
  were missing). The enchanting table's floating book is still missing.
- **M250**: arrows and thrown tridents look like arrows and tridents
  (they were grey capsules).
- **M251**: experience orbs are the glowing green orbs again (they were
  grey capsules), sized by their value.
- **M252**: minecarts are minecarts (they were grey capsules). Their
  contents (a chest in a chest minecart, etc.) are not drawn yet.
- **M253**: falling sand and gravel, and lit TNT, are blocks again (they
  were grey capsules). Lit TNT does not flash yet (it swells since M269).
- **M254**: paintings are paintings (they were grey capsules), every
  vanilla motif and size.
- **M255**: holograms (text displays) show their text, in colour, where
  they were grey capsules. Useful on servers like Frogsy. No dark
  background box behind the text yet.
- **M256**: item displays (floating items servers decorate with) draw their
  item, posed the way vanilla poses them, glint included.
- **M257**: end crystals are end crystals (they were grey capsules), the
  glass spinning and bobbing. No beam to the dragon yet, and the texture is
  drawn at half its resolution (it is too big for the entity atlas).
- **M258**: wither skulls, llama spit, shulker bullets and dragon fireballs
  draw as themselves (they were grey capsules). A blue wither skull looks
  like a black one, and the shulker bullet has no glowing outer shell.
- **M259**: an evoker's fangs now rise out of the ground and snap, where
  they were a grey capsule standing there from the moment they spawned.
- **M260**: block displays (the blocks servers float, turn and scale for
  decoration) draw their block. Any block works, built the first time it
  is seen; a chest or other block-entity block as a display still draws
  nothing.
- **M260 fix**: block displays and minecart contents (M260, M261) were
  drawn with the wrong block's model (an oak stair came out in cinnabar
  bricks; full cubes like stone not at all). Every automated check passed,
  because they counted quads; looking at a rendered image caught it. Fixed,
  and the check now compares the stair's actual texture.
- **M261**: minecarts show what they carry: chests, hoppers, furnaces
  (lit when fuelled), TNT, spawners, command blocks, or a custom block a
  server put in one. Until now every minecart was drawn empty.
- **M262**: holograms now have vanilla's dark see-through plate behind
  their text, so they read like they do in vanilla.
- **M263, an old visible bug**: all sign text (and the holograms) was
  drawn upside down in the windowed client, ever since signs got text
  (M25e). Every automated check compared the words, not the picture.
  Fixed, and a new check looks at the actual pixels of a letter.
- **M264**: a bell swings when it is rung (it used to hang still).
- **M265**: animated holograms and display decorations (servers move,
  turn and scale them smoothly) now slide like vanilla instead of jumping
  from step to step.
- **M266**: displays moved by a server now snap or glide exactly as in
  vanilla (glide when the server asks for it, otherwise jump).
- **M267**: holograms and displays that a server lights itself (for
  example to stay bright at night) are lit that way.
- **M268**: hologram text with a shadow (a common server style) has it.
- **M269**: lit TNT swells just before it blows, and falling or lit blocks
  of any kind draw (before, only sand, gravel and the like did). It does
  not flash white yet.
- **M270**: posed armour stands (a server decoration staple) show their
  poses; every stand used to stand stiffly at the default.
- **M271, a visible bug on servers**: invisible entities were drawn
  whole. Old-style holograms (invisible armour stands with a name) showed
  a full armour stand under the text, and invisible players and mobs were
  plainly visible. They are invisible now, with their armour, held items
  and names still showing, as in vanilla.
- **M272**: placed player heads show their own skins (server head walls
  and decorations all showed Steve).
- **M273, a visible bug on servers**: a skull or head worn by an armour
  stand, a mob or a player drew nothing, so the usual armour-stand NPC
  stood headless. Worn skulls draw now, and a worn player head in its own
  skin.
- **M274**: anything else worn on the head draws too (a carved pumpkin, a
  block, a banner hat), as vanilla draws it.
- **M275, visible in every server menu**: player head icons in the
  inventory, the hotbar and container screens show their own skins; menus
  built from custom heads all showed Steve. Heads in a hand, on the
  cursor, dropped or in a frame still do.
- **M276**: enchanting tables have their floating book again, opening
  and turning toward you as you come near and flipping its pages.
- **M277, a gameplay warning that was missing**: a creeper about to blow
  now swells and flashes white, and lit TNT blinks white. (Also corrected:
  the red damage flash was one step too strong.)
- **M278, a big one for servers**: the Glowing effect's outline (team
  colours, spectral arrows, minigames) now draws, through walls, as
  vanilla's outline post-process does. Rewo drew nothing for it.
- **M279**: an invisible player or mob that glows (hit by a spectral
  arrow, say) now shows as its outline alone, as in vanilla.
- **M280**: empty slots show vanilla's outline icons again (the armour and
  shield slots in your inventory, the brewing stand, enchanting table,
  loom and mount slots).
- **M281**: things on fire show flames (a burning player, a zombie in the
  sun). Rewo drew no fire on entities at all.
- **M282**: the vanilla vignette (darker screen corners in dark places,
  red near a world border) is drawn; Rewo had none.
- **M283**: the pumpkin-head view, the spyglass scope, the freezing frost
  and the fade to dark when you sleep are all drawn.
- **M284**: sleeping now lays the view down in the bed, facing along it,
  and hides the hand, as vanilla does; Rewo kept the standing view.
- **M285**: a charged creeper (struck by lightning) now shows its blue
  energy swirl; Rewo drew it as a plain creeper.
- **M286**: a breeze's wind funnel is now see-through and swirls, as in
  vanilla; Rewo drew it as a solid, still shell.
- **M287**: a wither below half health now shows its blue armour swirl.
- **M288**: a freshly built wither now grows from its smaller, blinking
  blue form, as in vanilla.
- **M289**: a player head dropped on the ground or put in an item frame now
  shows its own skin instead of Steve's.
- **M290**: so does a player head held by another player or mob.
- **M291**: the smithing table's empty slots now show their cycling hints
  (templates, then the armour or tools and materials a template takes).
- **M292**: the horse, donkey and llama screens now show the animal in
  their window, turning to follow the mouse.
- **M293**: horses were drawn a tenth too small, and donkeys and mules too
  big; they now have vanilla's sizes.
- **M294**: horse armour is drawn (leather in its dye, iron, gold, diamond,
  copper, netherite); it was invisible.
- **M295**: saddles on horses, donkeys, mules and the undead horses are
  drawn, with reins while someone rides.
- **M296**: a donkey or mule carrying a chest now shows its saddlebags.
- **M297**: llamas show chests only when they carry them (every llama
  showed them), and their carpets (and the trader llama's blanket) are
  drawn.
- **M298**: saddled pigs and striders show their saddles.
- **M299**: wolf armour is drawn (dyed overlay included), and a texture
  bleed between neighbouring mob sheets in Rewo's atlas is gone.
- **M300**: a saddled camel shows its saddle and bridle, with reins while
  ridden.
- **M301**: the nautilus shows its armour and saddle.
- **M302**: a happy ghast shows its harness, with the goggles lowered while
  ridden.
- **M303**: the camel husk is drawn as a camel husk (it was a placeholder
  capsule), with its saddle.
- **M304**: the giant is drawn (a zombie six times the size; it was a
  capsule too).
- **M305**: slimes and magma cubes are drawn at their real size. Since the
  26.2 port every one had drawn at the same medium size, because 26.2 moved
  the size field and Rewo kept reading the old slot.
- **M306**: see-through mob layers are drawn see-through. A slime now shows
  its core, eyes and mouth through a translucent shell (it was a solid
  green block).
- **M307**: the sulfur cube is drawn (core inside a see-through shell, the
  small model for babies); it was a placeholder capsule.
- **M308**: players without a skin (everyone on an offline server) wear
  vanilla's default skins, chosen by their UUID as vanilla does, instead of
  all being Steve; mannequins are drawn (as slim Alex by default).
- **M309**: a mannequin given a profile wears that profile's skin: its real
  skin when the profile carries one, otherwise the default skin vanilla
  derives from the name.
- **M310**: a player's or mannequin's skin layers (hat, jacket, sleeves,
  trouser legs) show or hide as that player chose in their skin
  customisation; Rewo drew all of them always.
- **M311**: damaged wolf armour shows its cracks.
- **M312**: a harnessed happy ghast holding a leash shows its ropes.
- **M313**: fixes a regression M310 introduced (the first-person arm had lost
  its sleeve); the sleeve now also follows the skin customisation setting.
- **M314**: baby happy ghasts (ghastlings) are drawn as themselves, small and
  in their own colours, instead of as a half-size adult.
- **M315**: baby turtles are drawn as themselves (a tiny turtle on its own
  sheet) instead of as a half-size adult.
- **M316**: polar bear cubs are drawn as themselves.
- **M317**: baby zombies and baby husks look right. Since an earlier
  milestone (M175, before this run) their skins had been scrambled, painted
  onto the adult's shape, and the same is still true of baby villagers,
  sheep, goats, armadillos, dolphins, drowned, zombie villagers and
  zombified piglins (Follow-ups).
- **M318**: baby villagers look right (their skins were scrambled the same
  way).
- **M319**: baby drowned look right, with their outer layer.
- **M320**: baby piglins and baby zombified piglins look right.
- **M321**: calves and mooshroom calves are their own model, not half-size adults.
- **M322**: piglets and chicks are their own models too.
- **M323**: lambs look right, fleece and all (and lose the fleece when shorn).
- **M324**: dolphin calves look right.
- **M325**: baby zombie villagers look right.
- **M326**: baby squids and baby glow squids are their own model, not half-size adults.
- **M327**: hoglets and baby zoglins too.
- **M328**: baby striders too.
- **M329**: the mob texture gate's crowd test is meaningful again (it had started flagging correct mobs as too small).
- **M330**: fox kits are their own model, walking on vanilla's own kit animation.
- **M331**: panda cubs are their own model.
- **M332**: ocelot kittens too (cat kittens need their coat variants first).
- **M333**: baby bees too.
- **M334**: donkey and mule foals too.
- **M335**: camel calves too, on their own walk animation.
- **M336**: rabbit kits too, on their own hop.
- **M337**: goat kids look right (their skins were scrambled since M175).
- **M338**: baby llamas are their own model and wear their own coat (creamy, white, brown or gray).
- **M339**: horse foals too, in all seven coats; and a fix to M334: donkey and mule foals held their tails 15 degrees too high.
- **M340**: kittens are their own model, in all eleven coats (read from the server's cat registry); and a fix: black cats and white horses had always been drawn tabby and brown (the server never sends a mob's default variant).
- **M341**: wolf puppies too, in every coat, wild or tame.
- **M342**: tame wolves and puppies wear their collar, in its colour (Rewo drew none before); and a decode fix: every tamed or owned mob had lost the metadata after its owner (a tame cat's or wolf's coat, among others).
- **M343**: tame cats and kittens wear their collar too; and a fix: adult cats had been drawn a quarter too big (vanilla's cat is four fifths of an ocelot).
- **M344**: parrots come in all five colours (every parrot was red and blue).
- **M345**: snow foxes and their kits are white (every fox was red).
- **M362**: Enni's arrow report: arrows were born with no velocity and moved only on the server's once-a-second updates. They now fly on the client (velocity, gravity, drag, block hits).
- **M361**: Enni's jitter report: the camera (and every entity's rotation) moved twenty times a second while the mount under it moved every frame. Both now blend between ticks.
- **M359**: horses, donkeys and mules look around (with their bridles), walk with vanilla's stride and lift their tails.
- **M358**: sheep graze (head down, nibbling); an audit of every adult mob's pose against vanilla found ~35 more to fix (`docs/rewo/rest-pose-audit-2026-10-21.md`).
- **M357**: cats and kittens sit; every cat's and ocelot's tail tip had hung straight down through the ground, and they walked like a cow (fixed).
- **M356**: wolves and puppies sit (no pet ever sat before).
- **M355**: angry wolves look angry and raise their tails; a tame wolf's tail droops as it loses health.
- **M354**: bees bob as they hover, turn red-eyed when angry, carry pollen, and lose the stinger once they sting.
- **M353**: snow golems wear their pumpkins (a sheared one shows its face).
- **M352**: endermen carry their blocks, arms up (they were empty-handed, and swung their arms too far).
- **M351**: mooshrooms carry their mushrooms (two on the back, one on the head).
- **M350**: warm zombie nautili wear their corals (and hide them under armour).
- **M349**: cold pigs and chickens have their own shapes, and warm ones and every piglet and chick their coats (every pig and chicken was temperate).
- **M348**: cold and warm cows have their own shapes and coats, and their calves their coats (every cow was the temperate one).
- **M347**: brown mooshrooms and their calves are brown (every mooshroom was red).
- **M346**: rabbits wear their six coats, the killer bunny included, and a rabbit named Toast is Toast (every rabbit was brown).
- **M245**: inventory icons are now lit like vanilla's: bright top, medium
  left side, dark right side. Before, a block's top was its darkest face and
  colours looked flat. A block held in first person also had its top shaded
  as its bottom.
- **M228**: when you ride something, you now sit where vanilla puts you.
  Before, you sat at the vehicle's center: too low in a boat, and with the
  camera inside a horse's back. Checked live against the server's own idea
  of where you sit, on all four ridden vehicles.
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
| `50529ba` | **M220** — the horse inventory: `mount_screen_open` decoded (class C closed), mount menu layouts per column count with inactive slots, the mount slots' `mayPlace` from `allowedEntities` (new `EQUIP_ALLOWED` table), vanilla's quick-move, the horse/nautilus screen, E sends `OPEN_INVENTORY` while riding; the player preview no longer drawn over containers; render-check tail with r83 moved in, **r84-r86**. |
| `e169f2d` | **M221** — boat steering: `rewo_world::boat` (`AbstractBoat.tick` transcribed, closed-form tests), the steered boat simulated in the session with `move_vehicle` / `paddle_boat` and vanilla's passenger packet order, server position packets ignored for it (base kept), rejections snapped and answered; the ridden vehicle's placeholder capsule no longer drawn around the camera; render-check **r87**. |
| `c40a3694` | Test fix: `srv`'s TCP-retry test recorded the query after answering it, a race that failed one full run. |
| `26f17d53` | **M362** — projectiles fly on the client: the spawn velocity read, `step_flight` (gravity, drag, rotation, block hits), `IN_GROUND`; unit tests and render check **r214**. |
| `b40fbbec` | **M361** — the camera's position and entities' rotations blended by the partial tick (Enni's riding jitter); unit tests and render check **r213**. |
| `533092da` | **M359** — the horse family's head group (a part, the bridle and reins on it), legs and tail; unit tests and mobtexshot **m81**. |
| `cc07efb7` | **M358** — grazing sheep (entity event 10, `sheep_eat_pose`, `SheepHead`) and the adult rest-pose audit document; unit tests, mobtexshot **m80** and render-check **r212**. |
| `e503ea97` | **M357** — sitting cats and kittens, the feline tail tip's rest (it hung through the ground) and the feline gait; unit tests, mobtexshot **m79** and render-check **r211**. |
| `1145f1fb` | **M356** — sitting wolves and puppies (`setSittingPose`; the body and mane now parts); unit tests, mobtexshot **m78**, render-check **r210** and `mobshot --sit`. |
| `ad48fcdf` | **M355** — angry wolves (index 22 LONG, eighteen angry sheets) and `getTailAngle()` (angry, tame by health, wild); unit tests, variant check **n14**, mobtexshot **m77** and render-check **r209**. |
| `800a1d72` | **M354** — the bee: flags (index 18 BYTE) and anger pick its four sheets, the stinger hides, the bone bobs (`bobUpAndDown`); unit tests, variant check **n13**, mobtexshot **m76** and render-check **r208**. |
| `8ea530c3` | **M353** — the snow golem's pumpkin (index 16 BYTE 0x10, `SnowGolemHeadLayer`); unit tests, variant check **n12** and render-check **r207**. |
| `27d53c05` | **M352** — the enderman's carried block (index 16 OPTIONAL_BLOCK_STATE, `CarriedBlockLayer`) and its own arm and leg rig; unit tests, variant check **n11** and render-check **r206**. |
| `be59d7f5` | **M351** — the mooshroom's three mushrooms on the block-model path, posed by the mob's own draw; a unit test against hand-worked positions and render-check **r205**. |
| `5079b77a` | **M350** — the coral zombie nautilus (registry and holder, the coral mesh with a new `Show::Unarmoured`, shell twins); unit tests, n7, mobtexshot **m75** and render-check **r204**; mobshot's ambiguous kinds 9, m5's small cap 26. |
| `b8a1c295` | **M349** — cold and warm pigs and chickens (two registries and holders, two new meshes, the warm and young sheets); unit tests, variant check n7 with both, mobtexshot **m74** (and m5's small cap 25) and render-check **r203**. |
| `a9913a89` | **M348** — cold and warm cows (the `cow_variant` registry and holder, two new meshes, the calves' sheets; atlas +256 rows); unit tests, variant check n7 with the cow, mobtexshot **m73** and render-check **r202**. |
| `0d2ac02f` | **M347** — the brown mooshroom and calf (index 18 INT on the mooshroom); a unit test, variant check n7 with the mooshroom, mobtexshot **m72** and render-check **r201**. |
| `8420ac42` | **M346** — rabbit coats and Toast (index 18 INT on the rabbit, the custom name); a unit test, variant check n7 with the rabbit, mobtexshot **m71** and render-check **r200**. |
| `43217330` | **M345** — the snow fox and kit (index 18 INT on the fox); a unit test, variant check n7 with the fox, mobtexshot **m70** and render-check **r199**. |
| `2505550f` | **M344** — the parrot's five colours (index 20 INT, newly decoded); a unit test, variant check n7 with the parrot, mobtexshot **m69** and render-check **r198**. |
| `0f7fe4ea` | **M343** — the cat's and the kitten's collar (inflated copies; index 23 INT on the cat); the adult cat at vanilla's 0.8 (it was drawn at the ocelot's size); two unit tests, variant check n10, mobtexshot **m68** and render-check **r197**. |
| `5e8c2d3e` | **M342** — the wolf's and the puppy's collar (index 21 INT on the wolf, default red, the wolf's mesh again in the coplanar range, tinted); the owner-reference skip fixed (16 bytes, not a varint: every owned mob lost its later metadata); four unit tests, `mobshot --variant-check` n9, mobtexshot **m67** and render-check **r196**. |
| `ea22f1f1` | **M341** — the puppy on `BabyWolfModel` in its coat's baby sheet (wild or tame), tail at a wild wolf's π/5; two unit tests, mobshot 378 views, mobtexshot **m66** and render-check **r195**. |
| `b3f2c7fd` | **M340** — the kitten on `BabyCatModel` in its coat's baby sheet; the cat and wolf registries' baby assets parsed; a missing variant is the class's default (black cats drew tabby, white horses brown); four unit tests, mobshot 375 views, mobtexshot **m65** and render-check **r194**. |
| `540074c8` | **M339** — the horse foal on `BabyHorseModel` in its coat's baby sheet, neck and tail at `setupAnim`'s rest; M334's donkey-foal tail fixed (it lacked `getTailXRotOffset` + π/6); three unit tests, mobshot 372 views, mobtexshot **m64** and render-check **r193**. |
| `86216028` | **M338** — the baby llama on `BabyLlamaModel` in its coat's baby sheet, the first baby with coat variants (`llama_baby_texture`, routed by `vanilla_variant`); two unit tests, mobshot 369 views, mobtexshot **m63** and render-check **r192**. |
| `cd556190` | **M337** — the goat kid on `BabyGoatModel` (one of M175's scrambled swaps), its head on a new yaw-only look at `setupAnim`'s π/8; a unit test, mobshot 366 views, mobtexshot **m62** and render-check **r191**. |
| `2159b9c5` | **M336** — the rabbit kit on `BabyRabbitModel`, hopping on `BABY_RABBIT_HOP`; a unit test, mobshot 363 views, mobtexshot **m61** (which cannot tell the kit's sheet from the adult's) and render-check **r190**. |
| `74e100a2` | **M335** — the camel calf on `BabyCamelModel`, walking on `CAMEL_BABY_WALK`; a unit test, mobshot 360 views, mobtexshot **m60** and render-check **r189**. |
| `4aedc64a` | **M334** — the donkey and mule foals on `BabyDonkeyModel` (their 64 x 64 sheets had no swap row); a unit test, mobshot 357 views, mobtexshot **m59** and render-check **r188**. |
| `b6cb8bd1` | **M333** — the baby bee on `BabyBeeModel`, airborne; a unit test, mobshot 351 views, mobtexshot **m58** and render-check **r187**. |
| `d09c8e83` | **M332** — the ocelot kitten on `BabyFelineModel`'s layer, walking on vanilla's feline gait; a unit test, mobshot 348 views, mobtexshot **m57** and render-check **r186**. |
| `d146e9ce` | **M331** — the panda cub on `BabyPandaModel` (its sheet goes through a variant record M175 could not read); a unit test, mobshot 345 views, mobtexshot **m56** and render-check **r185**. |
| `78f91c75` | **M330** — the kit on `BabyFoxModel`, walking on `FOX_BABY_WALK` (now extracted by `gen_anim_defs.ps1`); a unit test, mobshot 342 views, mobtexshot **m55** and render-check **r184**. |
| `4b90d065` | **M329** — mobtexshot's m5/m6 crowd at 2400 x 1350, pins re-measured (104 graded, 17 small, 89 discriminating, 15 ambiguous); a tiny-cow mutant fails it. |
| `eaf6c574` | **M328** — the baby strider on `BabyStriderModel` (body at `customAnimations`' 17.25, not the layer's 16.75); a unit test, mobshot 339 views, mobtexshot **m54** and render-check **r183**. |
| `26f196c4` | **M327** — the hoglet and the baby zoglin on `BabyHoglinModel` (ears at `setupAnim`'s ∓2π/9, not the `PartPose`'s); a unit test, mobshot 336 views, mobtexshot **m53** and render-check **r182**. |
| `2249cd5e` | **M326** — the baby squid and glow squid on `BabySquidModel` (both sheets differ in size from the adults', so M175 never baked them); a unit test, mobshot 330 views, mobtexshot **m52** and render-check **r181**. |
| `cfbbbc37` | **M325** — the baby zombie villager on `BabyZombieVillagerModel` (one of M317's scrambled swaps); a unit test, mobshot 324 views, mobtexshot **m51** and render-check **r180**. |
| `1ad317b0` | **M324** — the dolphin calf on `BabyDolphinModel` (one of M317's scrambled swaps); a unit test, mobshot 321 views, mobtexshot **m50** (which cannot tell the calf's sheet from the adult's: their palettes cover each other) and render-check **r179**. |
| `6728fca8` | **M323** — the lamb on `BabySheepModel` with its coplanar fleece (one of M317's scrambled swaps); the coplanar range now admits a shearable layer; a unit test, mobshot 318 views, mobtexshot **m49** and render-check **r178**. |
| `ff6c0e1c` | **M322** — the piglet on `BabyPigModel` and the chick on `BabyChickenModel` (neither sheet was in the swap table); two unit tests, mobshot 315 views, mobtexshot **m48** and render-check **r177**. |
| `af518178` | **M321** — calves on `BabyCowModel` (neither baby sheet was in the swap table); mobtexshot now runs on a 64 MiB thread (its debug frame overflowed the main thread); a unit test, mobshot 309 views, mobtexshot **m47** and render-check **r176**. |
| `39124d08` | **M320** — the baby piglins on `BabyPiglinModel` (the piglin's baby sheet had never been in the swap table); a unit test, mobshot 303 views, mobtexshot **m46** and render-check **r175**. |
| `772dca7c` | **M319** — the baby drowned: `BabyZombieModel`'s layer with its 0.25 outer layer on its own sheet (the coplanar head lifted); a unit test, mobshot 297 views, mobtexshot **m45** and render-check **r174**. |
| `763ad016` | **M318** — the baby villager on `BabyVillagerModel`; a unit test, mobshot 294 views, mobtexshot **m44** and render-check **r173**. |
| `ed6b6b3d` | **M317** — the baby zombie and husk on `BabyZombieModel`; the finding that M175's ten same-size baby swaps paint baby sheets onto the adult mesh, whose layout they do not share; unit tests, mobtexshot n1/n2 on the new model, render-check **r172**. |
| `fd8f6c47` | **M316** — the polar bear cub: `BabyPolarBearModel` on its own sheet, picked by the baby flag; a unit test, mobshot 285 views, mobtexshot **m43** (draws from its sheet; cannot tell the overlapping palettes apart) and render-check **r171**. |
| `6b439011` | **M315** — the baby turtle: `BabyTurtleModel` on its own sheet, picked by the baby flag; the app's baby scaling as one table (`baby_scales`); unit tests, mobshot 282 views, mobtexshot **m42** and render-check **r170**. |
| `98fd6736` | **M314** — the ghastling: `HAPPY_GHAST_BABY`'s model on its own sheet, picked by the baby flag; n4 and m8 reworded; a unit test, mobshot 279 views, mobtexshot **m41** and render-check **r169**. |
| `f7e55cd7` | **M313** — the first-person sleeve: M310 had moved it onto a child part `append_arm` never drew; now drawn by the local player's sleeve bits; handshot **a4**. |
| `a7b5c29c` | **M312** — the happy ghast's ropes: `RopesLayer`'s inflated boxes on their own texture slot, shown while it holds a leash and wears a harness; a unit test, mobtexshot **m40** and render-check **r168** (r72's ghast, injected harness). |
| `5a241a5c` | **M311** — wolf armour cracks: the worn piece's damage off the equipment packet, `Crackiness.WOLF_ARMOR`'s three sheets over the armour in the translucent range; a unit test, mobtexshot **m39** and render-check **r167**. |
| `488762b1` | **M310** — the player model's six outer layers on their own parts shown by the customisation mask (players, mannequins, the inventory preview); a unit test, mobtexshot **m38** and render-check **r166**. |
| `cf6de374` | **M309** — a mannequin's own profile: the `RESOLVABLE_PROFILE` metadata value read whole, `avatar_skin` (real textures, else the default of its id or its name's offline UUID, with a hand-rolled MD5); unit tests and render-check **r165**. |
| `12deb50c` | **M308** — default skins by profile id (`DefaultPlayerSkin.get`: the jar's eighteen, served from memory through the skin loader's own path) for skinless players and the local player; the mannequin as an avatar; unit tests and render-check **r164**. |
| `473285e2` | **M307** — the sulfur cube: adult and small (baby) kinds on the translucent range, its scale and offset folded into the model, the atlas 256 rows taller; unit tests, mobshot 276 views, mobtexshot **m37** and render-check **r163**. |
| `26864a9a` | **M306** — the `entityTranslucent` range (gamma-space blend after the wind, `entity.frag` with `GAMMA_OUT`, sorted, depth written), `Model::translucent_from`, the slime's inner body; mobtexshot's blend oracle reads the pixel beneath; a unit test, mobtexshot **m36** and render-check **r162**. |
| `36856ebe` | **M305** — cube-mob size at index 18 (26.2's `AbstractCubeMob extends AgeableMob`), class-gated, default 1; two unit tests and render-check **r161** (magma cubes of size 1 and 4). |
| `7b27deb8` | **M304** — the giant: the zombie's mesh at scale 6.0 on its sheet, with humanoid armour; a unit test, mobshot 270 views, mobtexshot **m35** and render-check **r160**. |
| `5543532e` | **M303** — the camel husk: a model kind (the camel's mesh on its own sheet, last before the capsule), its `camel_husk_saddle` layer; mobshot 267 views, mobtexshot **m34** and render-check **r159**. |
| `80c94138` | **M302** — the happy ghast's harness: `HappyGhastHarnessModel`'s harness and goggles (a ridden twin), one more late layer, the atlas 256 rows taller; a unit test, mobtexshot **m33** and render-check **r158**. |
| `c53406fd` | **M301** — the nautilus's armour and saddle (both nautilus kinds): shell-only boxes split off `nautilus_with`, the flat panel lifted 0.002 px; a unit test, mobtexshot **m32** and render-check **r157**. |
| `1de22b01` | **M300** — the camel's saddle: `CamelSaddleModel`'s saddle, bridle and ridden-only reins on the camel's parts, one more late layer; a unit test, mobtexshot **m31** and render-check **r156**. |
| `dcbf91d7` | **M299** — wolf armour (`WOLF_BODY` over the wolf at 0.2) and the atlas's one-texel extruded gutters (the vex bleed m5 caught), `ATLAS_H` 2,880; unit tests, mobtexshot **m30** and render-check **r155**. |
| `5087410f` | **M298** — pig and strider saddles: the pig at 0.5 and the strider's own mesh as saddle shells, two more late layers, the coplanar strider saddle through the trim range; a unit test, mobtexshot **m29** and render-check **r154**. |
| `21406809` | **M297** — llama chests and carpets: chests on a `Show::Chested` part, `LLAMA_BODY` over the 0.5 decor shell (trader llama blanket by default), the atlas grown 256 rows; a unit test, mobtexshot **m28** and render-check **r153**. |
| `cdfaa4b8` | **M296** — donkey and mule chests: `ABSTRACT_CHESTED_HORSE` generated, `DATA_ID_CHEST` read, the two chest boxes on a `Show::Chested` part; a unit test, mobtexshot **m27** and render-check **r152**. |
| `ee241881` | **M295** — the horse family's saddles: the saddle slot's piece kept, five saddle layers loaded late, `EquineSaddleModel` (body mesh plus saddle boxes, reins while ridden) per kind and scale; a unit test, mobtexshot **m26** and render-check **r151**. |
| `b1e80151` | **M294** — horse body armour: the body slot kept, `horse_body` sheets loaded after the rest, the 0.1 shells (living and undead) drawn in the worn layer's sheet and dye; a unit test, mobtexshot **m25** and render-check **r150**. |
| `9c6ac946` | **M293** — the equines' model scales: horse 1.1 (`livingHorseScale`), donkey 0.87, mule 0.92, from `LayerDefinitions`; a unit test and mobtexshot **m24** (silhouette height ratios). |
| `94f6c2bd` | **M292** — the mount in the horse screen's window: this frame's draw of the mount copied, posed in `MOUNT_PREVIEW` (26,18)-(78,70), size 17, offset 0.25, through the preview pass; `PreviewWindow`; capeshot **p5** and render-check **r149**. |
| `30c0ffd9` | **M291** — the smithing table's cycling slot icons: a `CyclingSlotBackground` port (30-tick steps, four-tick crossfade), vanilla's template lists, fifteen appended slot sprites, ticked per client tick and drawn on the widget layer; a unit test and inventoryshot **gc1**. |
| `e4506ce7` | **M290** — held player heads in their own skin: the equipment hand slots' profile kept (`hand_profiles`), requested, and swapped in `resolve_held_items`; render-check **r148**. |
| `33e8b0c6` | **M289** — dropped and framed player heads in their own skin: the slot-8 and slot-9 stacks' profile kept (`stack_profiles`), requested, and swapped to the per-skin item model by `HeadSkins::item_model`; render-check **r147**. |
| `98e31255` | **M288** — the wither's invulnerable phase: `DATA_ID_INV` read, `wither_invulnerable.png` as an appended vanilla variant chosen by the renderer's blink rule, the `2 - t / 220 · 0.5` shrink through the draw's stretch; two unit tests, the wither in mobshot's variant check and render-check **r146**. |
| `0d8b1c17` | **M287** — the powered wither's armour swirl: `isPowered` from health and the max-health attribute, the wither shell at deformation 0.5, `wither_armor.png` on its own sheet with `Mth.cos(t · 0.02) · 3` as its u offset, in the swirl range after the creeper's; two unit tests, mobtexshot **m23** and render-check **r145**. |
| `c6cf0346` | **M286** — the breeze's wind as its own layer: the slot-1 funnel leaves the body for `BREEZE_WIND` (own REPEAT sheet scrolled by `ageInTicks · 0.02`, lit, unshaded, translucent in gamma space, sorted), the pass's ninth range; a unit test, mobtexshot **m22** and render-check **r144**. |
| `a9b60422` | **M285** — the charged creeper's energy swirl: `DATA_IS_POWERED`, the creeper shell at deformation 2, `creeper_armor.png` (REPEAT, NEAREST) scrolled by `ageInTicks`, `0xFF808080`, additive in gamma space as the pass's eighth range; three unit tests, mobtexshot **m21** and render-check **r143**. |
| `f7e81b6b` | **M284** — the sleeping camera: `camera_pose` turns the render camera to the bed's facing at the sleeping eye (0.2 + 0.15), levels the pitch and hides the hand; picking keeps the player's eye; the listener follows the camera; a unit test and render-check **r142**. |
| `16a2903c` | **M283** — the camera overlays and the sleep fade: pumpkin blur, spyglass scope (with `scopeScale`), powder-snow frost, the sleep counter's fill, through the generalised overlay pass; a unit test, mobshot's alpha-blend check and render-check **r141**. |
| `7fadf105` | **M282** — the HUD vignette: `rewo_world::vignette` (vanilla's brightness follow, border warning and colour), the `sky_light_level` track, the `vignette` option, `VignettePass` (multiply blend through the UNORM view); a unit test, mobshot's per-pixel check and render-check **r140**. |
| `381b7f15` | **M281** — burning entities: `fire_draw` (shared flag 0, the fire-immune list), the flame frames baked as textures, `emit_flame` (`FlameFeatureRenderer`); a unit test, mobtexshot **m20** and render-check **r139**. |
| `5a498f53` | **M280** — empty-slot icons: `empty_slot_icons` (per-menu `getNoItemIcon` rules), fifteen `container/slot/*` sprites, `empty_slot_widgets` on the widget layer; a unit test and inventoryshot **ge1** (jar texels at vanilla's positions). |
| `22ba2d7c` | **M279** — an invisible glowing entity draws as its outline alone (the model emitted again, outline copies only); mobshot's outline-only check and render-check **r138**. |
| `b783723f` | Render check: a lost port race (`FAILED TO BIND`) retries on a new port, up to three. |
| `cda37fcd` | **M278** — the glowing outline: `glow_colour` (shared flag 6, team colour), the outline vertex range, `world/outline.rs` (`OutlinePass`: geometry into its own UNORM target and depth, `entity_sobel`, the two box blurs, the alpha composite through the UNORM view), five shaders; mobshot's rim/far/core pixel check and render-check **r137**. |
| `73b09728` | **M277** — the white flash: `overlay_value` and the shader's white columns (red alpha corrected 179 → 178), the creeper's client swell (`tick_creepers`, `creeper_swelling`, `EntityDraw::stretch` and `white_overlay`), TNT's `isLit` on the block-entity draw; unit tests, hurtshot **c6** and render-check **r135** / **r136**. |
| `96db1636` | **M276** — the enchanting table's book: `EnchantBook` (`bookAnimationTick` and its render interpolation), `World::tick_enchanting_books`, the book baked at rest with six animated groups posed by `enchanting_book_parts`, `MAX_PARTS` 4 → 7; unit tests (the tick; the posed book equals the lectern's) and render-check **r134**. |
| `17d2c03c` | **M275** — head icons in their skins: `SlotText::profile_textures`, the per-skin item model from `add_skin_head_model`, `skinned_icon_model` in `screen_icons` and `hotbar_models`, the GUI pass's copy rebuilt on arrival; unit test, inventoryshot **g21** (pixels) and render-check **r133**. |
| `ef2c348b` | **M274** — items worn on the head: the `head` display transform baked through the model chain into `HeldItemModel::head`, `WornHead::item` and `emit_worn_head`'s `translateToHead` branch; a unit test for the bake, mobtexshot **m19** (pixels from the pumpkin's textures, centroid on the decompile's position; a control without the −0.25 fails) and render-check **r132**. |
| `5f8c2e9a` | Render check: the use tail shrinks slimes to size 1 before killing them (a killed big slime split, and its children hit the bot through r101-r106). |
| `15df47eb` | **M273** — worn skulls: `minecraft:profile` decoded (`profile_textures`), the head slot's profile kept per entity, `EntityDraw::worn_head` and `emit_worn_head` (`CustomHeadLayer`'s skull branch, per-renderer transforms); mobtexshot **m18** (pixels from the skull sheet, centroid on the decompile's head position; a control without the scale fails) and render-check **r131** (a stand wearing a skinned head). |
| `bbe57010` | **M272** — player heads wear their own skins: `HeadSkins` (a worker thread, one fetch per URL, vetted to `textures.minecraft.net`, at most 16), `HeldItems::add_skin_head_model` (a per-skin copy of the head model) and a per-position model in `collect_block_entities`; render-check **r130** (a synthetic skin from a local file). |
| `4ab41389` | **M271** — invisible bodies: `EntityDraw::body_hidden` for an invisible living entity (not seen by a spectator); the model and capsule skip, layers and name stay; mobshot pixel check (fails on the old behaviour) and render-check **r129**. |
| `cff0a842` | **M270** — armour stand poses: indices 16-21 into `MobState`, `Anim::StandPose` as a delta from the baked default, the body sticks on the body part; render-check **r128**. |
| `8e259f94` | **M269** — primed TNT: fuse and block metadata (`PrimedState`), the client countdown, `TntRenderer`'s swell; any falling or primed block through the run-time state models; render-check **r127**. |
| `75928753` | **M268** — the text display's drop shadow (style bit 1), vanilla's quarter-colour copy one px down-right, by depth between glyphs and plate; render-check **r126**. |
| `9db1a606` | **M267** — the display brightness override (index 16) replaces the world's light for text, item and block displays; render-check **r125**. |
| `1a50f06e` | **M266** — display position updates: a per-entity lerp length, 0 (snap) for displays, `teleport_duration` (index 10) for a glide; render-check **r124**. |
| `27874c9f` | **M265** — display transformations interpolate: indices 8-9 and `Display.tick`'s render state, `Transformation.slerp` when drawn; render-check **r123** merges a 40-tick scale-up mid-run. |
| `5301f57e` | **M264** — a rung bell swings: the bell's block event recorded and timed, `BellModel.setupAnim`'s swing as an animated group; render-check **r122** rings one with redstone. |
| `7b828638` | **M263** — world text (signs, holograms) was upside down since M25e: `emit_world_text` gave a glyph's top the cell's bottom; it now maps top to top. New `itemshot` pixel witness `c3f.world_text_reads_upright` (83 to 84), failing on the old mapping. |
| `0bf26963` | **M262** — the text display's background plate: an optional backdrop on world-text lines drawn from the white texel behind the glyphs, vanilla's colour rules; render-check **r121**. |
| `33047437` | **M260 fix** — run-time block models read a state's quads through `render[state]` (`BakedAssets::state_quads`), not `models[state]` (an oak stair drew cinnabar bricks, full cubes another model); r119 and r120 check the textures too (a control run with the bug put back fails r119). |
| `f8f91001` | **M261** — minecart contents: indices 11-13 into a `MinecartDisplay` side map, each type's default block and offset, `AbstractMinecartRenderer`'s pose; a chest as its block-entity model, any other block through M260's run-time models; render-check **r120**. |
| `abad4262` | Comments only: thirteen doc comments that the run's patches had stranded on the wrong item (a new item inserted between a doc and its owner) moved back, found by scanning every commit for an insertion right after a `///` line. |
| `2fc6186d` | **M260** — block displays: the state into `DisplayState`, the model built at run time from the baked terrain quads (`BakedAssets::add_state_model`, `block_quads_as_held`), handed to the GPU incrementally (`extend_held_items`); render-check **r119**. |
| `bd4758d0` | **M259** — evoker fangs: entity event 4 stamps the bite (`EntityClasses::is_evoker_fangs`), vanilla's progress clock, the base and jaws as animated groups, drawn only while biting; render-check **r118**. |
| `35a3137` | **M258** — wither skulls, llama spit, shulker bullets (block-entity emitter, `small_projectile_pose`) and dragon fireballs (billboard, the XP orb's quad); render-check **r117**. |
| `88a6195` | M257 follow-up: `collect_projectiles`' doc comment, which M257's insertion had left on the crystal function. |
| `6fcbd6b` | Test fixes: `connect_tests`' dead address is loopback port 1 (an ephemeral port bound and closed could be taken back by a parallel test), and `inbound`'s `settle()` no longer reads an unstarted reader's 0 as settled. |
| `3a749af` | **M257** — end crystals: `EndCrystalModel` (two models, with and without the base) through the block-entity emitter, the nested glass animation as part transforms, `DATA_SHOW_BOTTOM`, the 128x64 sheet halved for the atlas; render-check **r116**. |
| `a03a237` | **M256** — item displays: the stack, context and billboard into `DisplayState`; vanilla's pose (`YP(π)`, the context's display transform, `translate(-0.5)`) through the block-entity emitter with glint; render-check **r115**. |
| `0061293` | **M255** — text displays: `Display` metadata into a `DisplayState` side map, vanilla's pose and billboards, line split/wrap/alignment, coloured runs on the world-text path; no capsules for displays, interactions, markers, effect clouds; render-check **r114**. |
| `2b02b11` | **M254** — paintings: the `painting_variant` registry synced, the variant at metadata index 9, every jar variant baked from `PaintingRenderer`'s geometry and drawn by facing; render-check **r113**. |
| `4ed6b77` | **M253** — falling blocks and primed TNT drawn as their block models (`rewo:be/state/<id>` baked for every fall-capable state and TNT, `collect_carried_blocks`); render-check **r112**. |
| `9a6d655` | **M252** — `MinecartModel` for all seven minecart types (the boat's fold, without the quarter turn); mobshot 264 views, mobtexshot small cap 11, render-check **r111**. |
| `f0351a6` | **M251** — experience orbs: `DATA_VALUE` decoded (index 8, orb-gated), the eleven icons baked and drawn camera-facing in their pulsing colour, unshaded, +7 block light (`Billboard::tint`, `entity_light_boosted`); itemshot **g11**. |
| `2a1d6c2` | **M250** — `ArrowModel` (arrow, spectral arrow) and the thrown trident drawn by their renderers' rotations through the block-entity emitter (`collect_projectiles`), no capsule; render-check **r110** (a shot arrow's 6 quads); the bow stage now waits before emptying the hand, so it really fires. |
| `ecace99` | **M249** — the session logs every hit on the local player (damage type id, cause entity and its type), for the render-check flake that lost health to an unknown source. |
| `efcf90b` | **M248** — the bell (`BellModel`) and a lectern's book (`BookModel` at `forAnimation(0, 0.1, 0.9, 1.2)` under `LecternRenderer`'s transform) as block-entity statics; blockentityshot **k31**; **r109** grades chest, bell and book. |
| `081032f` | **M247** — block entities created on the client when a block change makes one (`gen_block_entity_blocks.py`, `BlockEntities::sync_block_change`, as `LevelChunk.setBlockState` does); `remove` clears animation state; per-draw block-entity quad counts; render-check **r109** (a staged chest draws all 18 quads). |
| `de43c7c` | **M246** — the entity atlas gets a 64×64 large-item band (`ATLAS_H` 2368): block-entity sheets never paged in, so every block entity drew nothing in the windowed client since M25b; shield and trident models; a slot's own model brings its `gui` transform and lighting; itemshot **c3c/c3d/c3e** (0 px with the band disabled). |
| `ec3a771` | **M245** — slot lighting: `gui_normal` (display rotation + the slot's y flip), the diffuse decoded to linear, block items' `dir` remapped from `FACE_NAMES` to `Direction` order, block-entity `FACE_DIRS` corrected, parts' `dir` turned with their affine; inventoryshot **g10** (0.647 / 0.394 against 0.651 / 0.400). |
| `74e1e23` | **M244** — banner items (pole and bar + the dye-tinted base on the flag, `TexturePool::tinted`); branch `transformation`s composed onto the leaf (the statue's `select`); `setupAnim`'s `root.y = 0` for statues (a standing statue was 1.5 blocks underground); `gui_light: front` for banners and the pot; itemshot a3c bounds. |
| `39e275f` | **M243** — `ItemGeometry::Parts`: `composite` of block models (the beds: head + foot one block south), the conduit's shell and the decorated pot's base + four plain sides as items; itemshot **a3c** bounds for all three, **c3b**. |
| `c77e002` | **M242** — `minecraft:special` items drawn by block-entity models (chests, shulker boxes, heads, the statue) placed by their `transformation`; the GUI icon depth un-negated; JOML `rotationXYZ` (`Rx·Ry·Rz`) fixed in `gui_item::place`, `held::rotate_xyz` and `be_transform::rot_xyz` (the pot keeps a `rot_zyx`); inventoryshot **g9**, itemshot **a3c**. |
| `61e3c95` | **M241** — the fishing line: `FishingHookRenderer`'s 16-segment line to the owner's hand (first-person near-plane point for the local player), a black ribbon on the leash pass; render-check **r108**. |
| `87b0146` | **M240** — thrown items as `ThrownItemRenderer` draws them (the item, GROUND context, facing the camera; fireballs scaled and fullbright) and the fishing bobber's sprite, instead of capsules; render-check **r107**. |
| `f9af58c` | **M239** — the firework boost: `FireworkRocketEntity.tick`'s client-side push on the local glider, the rocket's attachment decoded (index 9); render-check **r106** (glide 0.167 → 1.673 blocks a tick, no correction; fails with the push disabled). |
| `e9b453d` | **M238** — the armour stand model: `ArmorStandModel` at the default pose on named parts (armour hangs on them), its client flags (arms, base plate, small) routed by kind from index 15, the base plate counter-rotated; render-check **r105**. Synched poses not read. |
| `cb8ed1a` | **M237** — the brewing stand's clicks (no `QuickMove` arm, nothing sent): slot kinds and the shift-click from `BrewingStandMenu`; `tools/gen_brewing.py` reads `PotionBrewing`'s ingredients from the decompile; render-check **r104** (a real stand brews). |
| `ff8b256` | **M236** — the enchanting table's clicks (its menu had no `QuickMove` arm, so nothing was sent); the anvil's cost line; render-check **r102** (anvil rename: cost line red/green, level spent) and **r103** (real enchanting); harness: the tail's flakes were a slime killing the survival bot (`gamerule spawn_monsters false`), plus no structures, r96 per-tick steps, tail cap 90 s. |
| `a9feba5` | **M235** — shield blocking checked live: render-check **r101** (a right-click raises the off-hand shield; the server's own damage: 20 → 16.64 unblocked, no loss blocked). Witness only, no client change. |
| `f333aef` | **M234** — item-model use variants: the bow's and crossbow's pull frames, the loaded crossbow's arrow/firework, the tooting goat horn, baked as `<item>#<model ref>` and selected with the live use state; the crossbow's projectile id now decoded; itemshot **a10**/**a11**, render-check **r100**. |
| `2a9f7e8` | **M233** — vanilla's dynamic FOV (`getFieldOfViewModifier` + `tickFov` easing + water): sprint, speed, flying, bow draw, spyglass; render-check **r99** (a drawn bow's frame FOV at exactly 0.85 of standing). |
| `e452419` | **M232** — crafting tables and furnaces end to end: render-check **r97** and **r98** with real blocks in survival through the production press/release; the open container's player part mirrored into the player inventory (a taken stack was missing from the hotbar after closing). |
| `d864d0f` | **M231** — using an item slows the walk (`itemUseSpeedMultiplier` from `use_effects`) and holds back a new sprint; render-check **r96** (walking while eating at exactly 0.2 of walking). |
| `4e6c8a9` | **M230** — the crosshair clips outline shapes: `raycast::cast_shapes` (`BlockGetter.clip`, OUTLINE, fluids ignored, the true hit point and `inside`), `BakedAssets::outline` from `gen_collision_shapes.py`'s new outline pass; render-check **r95** (a lever and a button pressed while holding food), mutation-checked. |
| `654115b` | **M229** — the right-click as `startUseItem`: both hands, `use_item_on` with the real hit point then `use_item` on a local pass, the held repeat and release; `rewo_world::click_use` (one rule per overriding class, from `tools/gen_click_use.py`), `Blocks::property`; render-check **r92-r94** (eating, buckets, snowball and fishing rod, in survival). |
| `c5e35d6` | **M228** — the local rider's seat: `EntityTable::seat_of` (M72's `positionRider` for a rider outside the table) and the session's `seat_local_player` on every vehicle; render-check **r91** against the server's seat read through a marker (a passenger's saved `Pos` is the vehicle's x/z); rideshot's local-seat row; r53's slab and r70's frame moved off a seated rider's sightlines. |
| `9d8989a` | **M227** — nautilus riding: `physics::tick_ridden_nautilus` (3D ridden input, the halfway turn, the look-direction dash, gravity-free water travel), `move_relative` with a y; `floatInWaterWhileRidden` for the tagged mounts; a debug-build overflow in `rain_column` fixed; r58 counts the timed run; render-check **r90**, tail cap 30 s. |
| `0beef60` | **M226** — the rider's input reaches its vehicle as vanilla's does: the modified move vector (×0.98, square stretch) from the previous tick, for boats, horses and camels; r88 and r89 pin vanilla's real per-tick speeds (their earlier "vanilla" formulas left out the 0.98). |
| `df0afe6` | **M225** — camel riding: `physics::CamelRide` (refuse-to-move, the sprint bonus, the dash), the dash cooldown and stand-up prediction in the session, sprinting while riding only on a camel (`vehicleCanSprint`); render-check **r89**, r82 moved onto it, tail cap 20 s. |
| `62252cc` | **M224** — r78's flake pinned (the flood's water fills the broken leaves within a tick; the witness graded air, the property is "gone") and fixed; r20 takes the first panel after its chest's injection, not r76's merchant screen. Render check 10/10 consecutive runs. |
| `326b70d` | **M223** — the boat model: `BoatModel`, `RaftModel` and their chest variants for the twenty boat types, with the wood picked by type name; the entity atlas grown to 2112 rows for the eight wood sheets. |
| `7ddc07e` | **M222** — horse steering: `physics::Body` and `tick_ridden` (`travelRidden`, `executeRidersJump`), the steered horse in the session with its own attributes, the jump meter's release as `onPlayerJump`; the render check's injected horse held still, r63 at its staged witness, r72's jump level and r16's ratio limited to the timed run; render-check **r88**. |
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
   now capped at 15 s more). Its first home before the options pages kept
   missing its 0.11 deadline on slower runs. (It was also suspected of causing
   r78's flakes; r78 failed again later without it, so that was wrong; M224
   found the real cause.) The
   tail costs about 9 s per run and holds r83-r88.
25. **The horse screen's window stayed empty** (until M292) instead of showing the mount
   (vanilla draws it there, following the mouse). Building a single entity's
   draw outside the world pass is a refactor of `collect_entities`; it is
   proposed as follow-up work rather than folded in here.
26. **Boats and horses are two milestones (M221, M222).** A boat has its own
   physics class (`AbstractBoat`), a horse is a `LivingEntity` whose travel
   is the player's own with the horse's size and attributes; Rewo's player
   physics fixes the player's box sizes, so horses need that generalised
   first.
27. **The ridden vehicle's placeholder capsule is not drawn** (M221). It
   enclosed the camera and hid the world. Reversible in one line once boats
   have a model; the honest fix is the model.

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
- After M221: **4,306 tests, 1 failure under full-suite load** (`srv`'s
  `a_truncated_answer_is_retried_over_tcp`, passing five times alone);
  gates **51/51** (demo hash unchanged); render check **87/87** on every run
  since r87 landed (three); clippy (gates and default features) and
  `cargo fmt --check` clean.
- After M222: **4,308 tests, 0 failed; gates 51/51** (demo hash unchanged);
  render check **88/88** on two consecutive runs (earlier runs while it was
  built found the injected-horse, jump-key and r16-ratio issues the §15
  entry lists); clippy (gates and default features) and `cargo fmt --check`
  clean.
- After M223: **4,308 tests, 0 failed; gates 51/51** (demo hash
  unchanged); render check **88/88** with an in-boat F2 capture looked at;
  clippy (gates and default features) and `cargo fmt --check` clean.
- After M224: render check **88/88 on ten consecutive runs** (no witness
  failed in any of them); clippy (gates and default features) and
  `cargo fmt --check` clean; `rewo-app` tests pass. In the 23 runs of the
  loops before the fix, r78 failed six times and r20 twice.
- After M225: **4,306 tests passed, 5 failed under full-suite load**: all five
  `srv` TCP tests, at the test helper finding "a port free for both UDP and
  TCP" (100 tries), and all pass alone (three runs); render check **89/89**
  on two runs; clippy (gates and default features) and `cargo fmt --check`
  clean.
- After M226: **4,312 tests passed, 0 failed**; render check **89/89**
  on two runs (r88 now reading 0.6476 and r89 0.4101, vanilla's); clippy
  (gates and default features) and `cargo fmt --check` clean.
- After M227: **4,316 tests passed, 0 failed; gates 51/51** (demo hash unchanged);
  render check **90/90** (the first full runs found the debug-build rain
  overflow, which panicked, and r58's tail leak; both fixed); clippy (gates
  and default features) and `cargo fmt --check` clean.
- After M228: **4,316 tests passed, 0 failed; gates 51/51** (rideshot 46, its new
  local-seat row checked against its mutation partner); render check
  **91/91** on two consecutive runs; clippy (gates and default features)
  and `cargo fmt --check` clean.
- After M229: **4,326 tests passed, 0 failed; gates 51/51**; render check **94/94**
  on two runs; clippy (gates and default features) and `cargo fmt --check`
  clean.
- After M230: **4,330 tests passed, 0 failed; gates 51/51**; render check **95/95**
  on two runs (and 94/95 with the outline table withheld: r95 discriminates);
  clippy (gates and default features) and `cargo fmt --check` clean.
- After M231: **4,331 tests passed, 0 failed; gates 51/51**; render check **96/96**
  on two runs; clippy (gates and default features) and `cargo fmt --check`
  clean.
- After M232: **4,332 tests passed, 0 failed; gates 51/51**; render check **98/98**
  on two consecutive runs after the witnesses waited for the screen's
  content (one earlier run read r98 at 97/98: its clicks beat the
  container's `set_content`); clippy (gates and default features) and
  `cargo fmt --check` clean.
- After M233: **4,338 tests passed, 0 failed; gates 51/51**; render check **99/99** on two runs; clippy (gates and default features) and `cargo fmt --check` clean.
- After M234: **4,340 tests passed, 1 failed** (`connect_tests::the_first_address_that_answers_wins`, a pre-existing loopback flake: its just-closed "dead" port answered, a TCP self-connect; it passes 3/3 alone, see follow-ups); gates 51/51; render check **100/100** on two runs; clippy (gates and default features, after eliding one lifetime) and `cargo fmt --check` clean.
- `rewo-net` `connect_tests::the_first_address_that_answers_wins` flaked once (M234's run): its `dead_addr()` binds and closes an ephemeral loopback port, and the connect to it can succeed by TCP self-connect (the client's ephemeral source port equals the closed port). Fix: hold the dead port's listener with no `accept`, or use a port from a reserved range. Not touched in this run.
- After M235: render-check only change: `rewo-app` tests (gates) 296 passed; render check **101/101** on two runs; clippy (gates) and `cargo fmt --check` clean.
- After M236: **4,344 tests passed, 0 failed** (before the last witness-only and harness edits; `rewo-app` gates tests 296 after them); gates 51/51; render check **103/103 on five consecutive runs** after the slime fix (earlier runs failed about one in three on the tail, now explained); clippy (gates and default features) and `cargo fmt --check` clean.
- After M237: **4,347 tests passed, 0 failed; gates 51/51**; render check **104/104** on three runs; clippy (gates and default features) and `cargo fmt --check` clean.
- After M238: **4,347 tests passed, 0 failed; gates 51/51**; render check **105/105** on three runs; clippy (gates and default features) and `cargo fmt --check` clean.
- Armour-stand POSES (`DATA_HEAD_POSE` .. `DATA_RIGHT_LEG_POSE`, six `Rotations`) are not read (M238): a posed stand shows the default pose. Needs per-instance part rotations in the entity pass (the model builds per kind today).
- After M239: **4,348 tests passed, 0 failed; gates 51/51**; render check **106/106** on three runs (plus the push-disabled control failing r106 as it should); clippy (gates, after one `int_plus_one` fix in the witness, and default features) and `cargo fmt --check` clean.
- After M240: **4,349 tests passed, 0 failed; gates 51/51**; render check **107/107** on three runs (after the texture-path fix); clippy (gates and default features, after collapsing one `if`) and `cargo fmt --check` clean.
- Text displays draw no see-through mode or text opacity yet (M255; the
  plate came in M262, the shadow in M268).
  Displays have their transformation and position interpolation and
  brightness override (M265-M267), but no shadow or glow outline; a block display of a block-entity block (a chest) or a
  fluid draws nothing.
- End crystals (M257) draw no beam, and their sheet is halved to fit the
  entity atlas's 64x64 large slots; a larger slot would draw it at full
  resolution.
- Still a capsule after M259: the wind charge (its texture scrolls through the `breezeWind` render type, which the block-entity emitter has no equivalent of). A blue wither skull draws the black texture; the shulker bullet's translucent shell is not drawn (no display entity is a capsule; M260 drew block displays). A minecart's slope pitch is not drawn (its contents are since M261). (M241 drew the fishing line; M250 the arrows and the thrown trident.)
- After M241: **4,350 tests passed, 0 failed; gates 51/51**; render check **108/108** on three runs; clippy (gates and default features) and `cargo fmt --check` clean.
- After M242: **4,356 tests; 4,351 passed in the full run and the 5 failures, all `rewo-net` `srv` TCP tests at the fake nameserver's port pairing ("a port free for both UDP and TCP"), passed on rerun (34/34): the shared PC's sockets, not Rewo; gates 51/51** (inventoryshot 169, itemshot 78); render check **108/108** on two runs; clippy (gates and default features, after one needless `clone`) and `cargo fmt --check` clean.
- After M243: **4,357 tests passed, 0 failed; gates 51/51** (itemshot 79); render check **108/108** on two runs; clippy (gates and default features) and `cargo fmt --check` clean.
- After M244: **4,361 tests passed, 0 failed; gates 51/51** (itemshot 79, blockentityshot 191); render check **108/108** on two runs; clippy (gates and default features, after one `chunks_exact_mut` → `as_chunks_mut`, re-run clean) and `cargo fmt --check` clean.
- After M245: **4,365 tests; 4,364 passed in the full run, and the one failure (`rewo-audio` `a_slow_read_is_not_re_requested_on_every_tick`, which waits 80 ms of wall time for a worker to open a stream) passed 5/5 on rerun; gates 51/51** (inventoryshot 170); render check **108/108 on runs 2 and 3**, run 1 107/108 with r96 at a 0.31 eating-slowdown ratio (0.2000 on the others; M245 touches no movement); clippy (gates and default features) and `cargo fmt --check` clean.
- After M246: **4,365 tests passed, 0 failed; gates 51/51** (itemshot 82, capeshot 69 after f1/f2 took the new bottom band; demo hash unchanged); render check **108/108** on two runs; clippy (gates and default features, after four needless `&mut gpu` in c3e) and `cargo fmt --check` clean. Control: with the large band disabled, c3c/c3d/c3e read 0 px.
- After M247: **4,370 tests passed, 0 failed; gates 51/51**; render check **109/109** on two runs; clippy (gates and default features) and `cargo fmt --check` clean. Control: before the client-side creation, r109 failed with the chest absent from the draw list.
- After M248: **4,370 tests passed, 0 failed; gates 51/51** (blockentityshot 192); render check **109/109 on runs 1, 3 and 4**; run 2 read 100/109: during r96's eat-while-walking the bot took 3.2 + 1.6 + 1.6 damage from an unlogged source, was knocked off its path, and the use tail never reached the furnace (the use spot moves with the random spawn; the other three runs walked the same stage at full health). Clippy (gates and default features) and `cargo fmt --check` clean.
- After M250: **4,371 tests passed, 0 failed; gates 51/51**; render check **110/110** on two runs; clippy (gates and default features) and `cargo fmt --check` clean.
- After M251: **4,373 tests passed, 0 failed; gates 51/51** (itemshot 83); render check **110/110** on two runs; clippy (gates and default features) and `cargo fmt --check` clean.
- After M252: **4,373 tests passed, 0 failed; gates 51/51** (mobshot 264 views); render check **111/111** on two runs; clippy (gates and default features) and `cargo fmt --check` clean.
- After M253: **4,373 tests passed, 0 failed; gates 51/51**; render check **112/112** on two runs; clippy (gates and default features) and `cargo fmt --check` clean.
- After M254: **4,376 tests; the full run passed 4,375 and failed `reconfigure.rs`'s `malformed_bodies_are_counted_once_each`, whose truncated-registry fixture was `painting_variant`, now a parsed registry: the four variant parsers stopped short silently, so a short variant registry is now counted (`short_registry`) and rewo-net passes 1,380 + 7 + 4; gates 51/51**; render check **113/113** on two runs; clippy (gates and default features) and `cargo fmt --check` clean.
- After M255: **4,379 tests passed, 0 failed; gates 51/51**; render check **114/114** on two runs; clippy (gates and default features) and `cargo fmt --check` clean.
- After M256: **4,381 tests passed, 0 failed; gates 51/51**; render check **115/115** on two runs; clippy (gates and default features) and `cargo fmt --check` clean.
- After M257: **4,385 tests; two full runs each failed one different load-sensitive rewo-net test (`connect_any`'s reused dead port, then the inbound budget test's 50 ms settle), both pass alone and are fixed in `6fcbd6b`, after which the workspace ran **4,385/4,385**; gates 51/51**; render check **116/116** on two runs; clippy (gates and default features) and `cargo fmt --check` clean.
- After M258: **4,387 tests passed, 0 failed; gates 51/51**; render check **117/117** on two runs; clippy (gates and default features) and `cargo fmt --check` clean.
- After M259: **4,390 tests passed, 0 failed; gates 51/51**; render check **118/118** on two runs (r118 caught the bite in 917 and 826 frames); clippy (gates and default features) and `cargo fmt --check` clean.
- After M260: **4,392 tests passed, 0 failed; gates 51/51**; render check **119/119** on two runs; clippy (gates and default features) and `cargo fmt --check` clean.
- After M261: **4,395 tests passed, 0 failed; gates 51/51**; render check **120/120** on two runs; clippy (gates and default features) and `cargo fmt --check` clean.
- After the M260 fix: **4,396 tests passed, 0 failed; gates 51/51**; render check **120/120** on four runs (two before and two after the texture checks); clippy (gates and default features) and `cargo fmt --check` clean; a control run with the bug put back failed r119.
- After M262: **4,397 tests passed, 0 failed; gates 51/51**; render check **121/121** on two runs; clippy (gates and default features) and `cargo fmt --check` clean; the plate looked at in a scratch render (translucent, behind the glyphs).
- After M263: **4,397 tests; the full run failed one, `srv`'s `a_truncated_answer_is_retried_over_tcp` (a race in the test: the fake server recorded the query after answering it), fixed in `c40a3694`; gates 51/51 (itemshot 84)**; render check **121/121** on two runs; clippy (gates and default features) and `cargo fmt --check` clean; c3f fails on the old mapping.
- After M264: **4,400 tests passed, 0 failed; gates 51/51** (blockentityshot rerun after its d2/d3 update; the first run failed d2, which pinned a bell as having no block-event body); render check **122/122** on two runs; clippy (gates and default features) and `cargo fmt --check` clean.
- After M265: **4,403 tests passed, 0 failed** (rewo-net and rewo-world rerun after the render-flag fix: 1,387 + 813 passed); **gates 51/51**; render check **123/123** on two runs after the fix (r123 failed on the two before it); clippy (gates and default features) and `cargo fmt --check` clean.
- After M266: **4,405 tests passed, 0 failed; gates 51/51**; render check **124/124** on two runs (r124: 22 and 21 ticks part-way); clippy (gates and default features) and `cargo fmt --check` clean.
- After M267: **4,407 tests passed, 0 failed; gates 51/51**; render check **125/125** on two runs; clippy (gates and default features) and `cargo fmt --check` clean.
- After M268: **4,407 tests passed, 0 failed; gates 51/51**; render check **126/126** on two runs; clippy (gates and default features) and `cargo fmt --check` clean; a scratch render of the finished hologram looked like vanilla's.
- After M269: **4,409 tests passed, 0 failed; gates 51/51**; render check **127/127** on two runs (r127: 155 and 181 frames swelling); clippy (gates and default features) and `cargo fmt --check` clean.
- After M270: **4,411 tests passed, 0 failed; gates 51/51** (mobshot unchanged: the default pose bakes as before); render check **128/128** on two runs; clippy (gates and default features) and `cargo fmt --check` clean.
- After M271: **4,411 tests passed, 0 failed; gates 51/51** (labelshot 53, mobshot's hidden-body check); render check **129/129** on two runs (a third attempt lost its server's port-bind race and never started; an earlier full run hit the SRV tests' environmental 'no port free for both UDP and TCP' flake, clean on rerun); clippy (gates and default features) and `cargo fmt --check` clean.
- After M362: **4,528 tests passed, 0 failed; gates 51/51**; render check **214/214** on both runs (r214: the shot arrow 14.28 blocks out in its first 0.25 s, moving on 57/58 and 97/98 frames; with the client flight removed, the mutant, 0.00 blocks and 0/102); clippy (both) and `cargo fmt --check` clean.
- After M361: **4,526 tests passed, 0 failed; gates 51/51**; render check **213/213** on both runs (r213: the camera moved on 6721/6721 and 6972/6972 of the frames its mount moved; the camera-unblended mutant read 341/5980); clippy (both) and `cargo fmt --check` clean.
- After M359: **4,524 tests passed, 0 failed; gates 51/51** (mobtexshot 81); render check **212/212** on runs 2 and 3 (run 1 read 211/212: r78's survival leaf break stopped 5 ticks after the press against vanilla's 4, a timing flake in a witness the horse does not touch); clippy (both) and `cargo fmt --check` clean.
- After M358: **4,522 tests passed, 0 failed; gates 51/51** (mobtexshot 80); render check **212/212** on two runs (r212's lambs grazed in both); clippy (both) and `cargo fmt --check` clean.
- After M357: **4,520 tests passed, 0 failed; gates 51/51** (mobtexshot 79); render check **211/211** on two runs; clippy (both) and `cargo fmt --check` clean.
- After M356: **4,517 tests passed, 0 failed; gates 51/51** (mobtexshot 78); render check **210/210** on two runs; clippy (both) and `cargo fmt --check` clean.
- After M355: **4,516 tests passed, 0 failed; gates 51/51** (mobtexshot 77, the variant check 19 with n14, n1 at 159); render check **209/209** on two runs; clippy (both) and `cargo fmt --check` clean.
- After M354: **4,512 tests passed, 0 failed; gates 51/51** (mobtexshot 76, the variant check 18 with n13, n1 at 141); render check **208/208** on two runs; `cargo fmt --check` clean; clippy (both) clean after a doc-comment rewording (the chain's run caught `doc_lazy_continuation` on a line starting "+ π/4").
- After M353: **4,510 tests passed, 0 failed** (on a rerun: the chain's run had five `srv` loopback tests fail to find a port free for both UDP and TCP, an environmental flake, see Follow-ups)**; gates 51/51** (the variant check at 17 witnesses with n12; without the snow golem's routing n12 fails); render check **207/207** on two runs; clippy (both) and `cargo fmt --check` clean.
- After M352: **4,508 tests passed, 0 failed; gates 51/51** (the variant check at 16 witnesses with n11; without the enderman's routing n11 fails); render check **206/206** on two runs; clippy (both) and `cargo fmt --check` clean.
- After M351: **4,506 tests passed, 0 failed; gates 51/51**; render check **205/205** on two runs; clippy (both) and `cargo fmt --check` clean.
- After M350: **4,505 tests passed, 0 failed; gates 51/51** (mobshot 390 views with nine ambiguous kinds, mobtexshot 75, the variant check's n7 over fourteen mobs); render check **204/204** on two runs; clippy (both) and `cargo fmt --check` clean.
- After M349: **4,504 tests passed, 0 failed; gates 51/51** (mobshot 390 views, mobtexshot 74, the variant check's n7 over thirteen mobs and n1 at 135); render check **203/203** on two runs; clippy (both) and `cargo fmt --check` clean.
- After M348: **4,502 tests passed, 0 failed; gates 51/51** (mobshot 384 views, mobtexshot 73, the variant check's n7 over eleven mobs and n1 at 129, etf-check 8/8, capeshot 70 after its shelf ledger gained M348's growth); render check **202/202** on two runs; clippy (both) and `cargo fmt --check` clean.
- After M347: **4,498 tests passed, 0 failed; gates 51/51** (mobtexshot 72, the variant check's n7 over ten mobs and n1 at 127); render check **201/201** on two runs; clippy (both) and `cargo fmt --check` clean.
- After M346: **4,497 tests passed, 0 failed; gates 51/51** (mobtexshot 71, the variant check's n7 over nine mobs and n1 at 125); render check **200/200** on two runs; clippy (both) and `cargo fmt --check` clean.
- After M345: **4,496 tests passed, 0 failed; gates 51/51** (mobtexshot 70, the variant check's n7 over eight mobs and n1 at 111); render check **199/199** on two runs; clippy (both) and `cargo fmt --check` clean.
- After M344: **4,495 tests passed, 0 failed; gates 51/51** (mobtexshot 69, the variant check's n7 over seven mobs and n1 at 109); render check **198/198** on two runs; clippy (both) and `cargo fmt --check` clean.
- After M343: **4,494 tests passed, 0 failed; gates 51/51** (mobtexshot 68, the variant check's 15 witnesses); render check **197/197** on two runs; clippy (both) and `cargo fmt --check` clean.
- After M342: **4,493 tests passed, 0 failed; gates 51/51** (mobtexshot 67, the variant check's 14 witnesses); render check **196/196** on two runs after the owner-reference fix (the first chain's runs read 195/196: r196's tame wolf had no collar, which is how the decode bug was found; that chain also failed two unit tests whose counts now include the collar copy, fixed to count the bare mesh); clippy (both) and `cargo fmt --check` clean.
- After M341: **4,490 tests passed, 0 failed; gates 51/51** (mobtexshot 66, mobshot 378 views, the variant check at 105); render check **195/195** on two runs; clippy (both) and `cargo fmt --check` clean.
- After M340: **4,489 tests passed, 0 failed; gates 51/51** (mobtexshot 65, mobshot 375 views, the variant check at 88); render check **194/194** on two runs, after the default-variant fix (the first chain's two runs read 193/194, r194's black kitten on variant 0, which is how the bug was found); clippy (both) and `cargo fmt --check` clean.
- After M339: **4,487 tests passed, 0 failed; gates 51/51** (mobtexshot 64, mobshot 372 views, the variant check at 78 declared sheets); render check **193/193** on two runs; clippy (both) and `cargo fmt --check` clean. The chain paused about 18 min (18:35-18:53) for another project's G4 run.
- After M338: **4,484 tests passed, 0 failed; gates 51/51** (mobtexshot 63, mobshot 369 views; the chain's first sweep read 50/51 on `mobshot --variant-check` n1, whose pinned count of declared variant sheets was still 69 against the three new coats' 72, fixed and re-swept); render check **192/192** on two runs; clippy (both) and `cargo fmt --check` clean.
- After M337: **4,482 tests passed, 0 failed; gates 51/51** (mobtexshot 62, mobshot 366 views); render check **191/191** on two runs; clippy (both) and `cargo fmt --check` clean. The chain paused 28 min (17:37-18:05) for another project's G4 run, per the shared-PC rule.
- After M336: **4,481 tests passed, 0 failed; gates 51/51** (mobtexshot 61, mobshot 363 views); render check **190/190** on two runs; clippy (both) and `cargo fmt --check` clean.
- After M335: **4,480 tests passed, 0 failed; gates 51/51** (mobtexshot 60, mobshot 360 views); render check **189/189** on two runs; clippy (both) and `cargo fmt --check` clean. The chain paused about 31 min (16:38-17:09) for another project's G4 run, per the shared-PC rule.
- After M334: **4,479 tests passed, 0 failed; gates 51/51** (mobtexshot 59, mobshot 357 views); render check **188/188** on two runs; clippy (both) and `cargo fmt --check` clean.
- After M333: **4,478 tests passed, 0 failed; gates 51/51** (mobtexshot 58, mobshot 351 views); render check **187/187** on runs 1 and 3; **run 2 read 186/187**: r91's horse seat was 0.7 off in x from the server's marker (boat, camel and nautilus matched), the horse still moving between the two samples; M333 touches no riding code (recorded under Follow-ups). Clippy (both) and `cargo fmt --check` clean.
- After M332: **4,477 tests passed, 0 failed; gates 51/51** (mobtexshot 57, mobshot 348 views); render check **186/186** on two runs; clippy (both) and `cargo fmt --check` clean.
- After M331: **4,476 tests passed, 0 failed; gates 51/51** (mobtexshot 56, mobshot 345 views); render check **185/185** on two runs; clippy (both) and `cargo fmt --check` clean.
- After M330: **4,475 tests passed, 0 failed; gates 51/51** (mobtexshot 55, mobshot 342 views); render check **184/184** on two runs; clippy (both) and `cargo fmt --check` clean.
- After M329: **4,474 tests passed, 0 failed; gates 51/51** (mobtexshot 54 at the new crowd size, mobshot 339 views); render check **183/183** on runs 1 and 3; **run 2 read 174/183**: nine tail witnesses (r42, r61, r75–r78, r81, r88, r91) could not find what `REWO_PRECMD` staged near the bot. No live code changed in M329; the stage sequence matched run 1's, but the 105-command batch took 232 ms to send (2 ms in run 1). Recorded under Follow-ups. Clippy (both) and `cargo fmt --check` clean.
- After M328: **4,474 tests passed, 0 failed; gates 51/51** (mobtexshot 54, mobshot 339 views); render check **183/183** on two runs; clippy (both) and `cargo fmt --check` clean.
- After M327: **4,473 tests passed, 0 failed; gates 51/51** (mobtexshot 53, mobshot 336 views); render check **182/182** on two runs; clippy (both) and `cargo fmt --check` clean.
- After M326: **4,472 tests passed, 0 failed; gates 51/51** (mobtexshot 52, mobshot 330 views); render check **181/181** on two runs; clippy (both) and `cargo fmt --check` clean.
- After M325: **4,471 tests passed, 0 failed; gates 51/51** (mobtexshot 51, mobshot 324 views); render check **180/180** on two runs; clippy (both) and `cargo fmt --check` clean.
- After M324: **4,470 tests passed, 0 failed; gates 51/51** (mobtexshot 50, mobshot 321 views); render check **179/179** on two runs; clippy (both) and `cargo fmt --check` clean.
- After M323: **4,469 tests passed, 0 failed; gates 51/51** (mobtexshot 49, mobshot 318 views); render check **178/178** on two runs; clippy (both) and `cargo fmt --check` clean.
- After M322: **4,468 tests passed, 0 failed; gates 51/51** (mobtexshot 48, mobshot 315 views); render check **177/177** on two runs; clippy (both) and `cargo fmt --check` clean.
- After M321: **4,466 tests passed, 0 failed; gates 51/51** (mobtexshot 47, mobshot 309 views); render check **176/176** on two runs; clippy (both) and `cargo fmt --check` clean.
- After M320: **4,465 tests passed, 0 failed; gates 51/51** (mobtexshot 46, mobshot 303 views); render check **175/175** on two runs; clippy (both) and `cargo fmt --check` clean.
- After M319: **4,464 tests passed, 0 failed; gates 51/51** (mobtexshot 45, mobshot 297 views); render check **174/174** on two runs; clippy (both) and `cargo fmt --check` clean.
- After M318: **4,463 tests, 5 failed in the chain's run**: the `srv::tests` loopback flake again (the fixture's free-port race); alone they passed 34/34 three times. Gates 51/51 (mobtexshot 44, mobshot 294 views); render check **173/173** on two runs; clippy (both) and `cargo fmt --check` clean.
- After M317: **4,462 tests passed, 0 failed; gates 51/51** (mobtexshot 43 with n1/n2 on the new model, mobshot 291 views); render check **172/172** on two runs; clippy (both) and `cargo fmt --check` clean.
- After M316: **4,461 tests passed, 0 failed; gates 51/51** (mobtexshot 43, mobshot 285 views); render check **171/171** on two runs (r171 on 27,125 and 26,458 frames); clippy (both) and `cargo fmt --check` clean.
- After M315: **4,460 tests passed, 0 failed; gates 51/51** (mobtexshot 42, mobshot 282 views); render check **170/170** on two runs (r170 on 27,590 and 26,808 frames); clippy (both) and `cargo fmt --check` clean.
- After M314: **4,459 tests passed, 0 failed; gates 51/51** (mobtexshot 41, mobshot 279 views); render check **169/169** on two runs (r169 on 25,846 and 26,751 frames); clippy (both) and `cargo fmt --check` clean.
- After M313: **4,458 tests passed, 0 failed; gates 51/51** (handshot 42); render check **168/168** on two runs; clippy (both) and `cargo fmt --check` clean.
- After M312: **4,458 tests passed, 0 failed; gates 51/51** (mobtexshot 40, mobshot 276 views); render check **168/168** on two runs (r168 on 21,698 and 23,188 frames, r72 unchanged); clippy (both) and `cargo fmt --check` clean.
- After M311: **4,457 tests passed, 0 failed; gates 51/51** (mobtexshot 39, the atlas still packing); render check **167/167** on two runs (r167 on 26,649 and 27,399 frames); clippy (both) and `cargo fmt --check` clean.
- After M310: **4,456 tests passed, 0 failed** (after fixing the one test the chain caught: `player_world_orientation_at_yaw_zero` found the right arm's extent by the arm's animation, and the sleeve now rides a child part; the test follows the parent chain now, with the geometry unchanged); gates 51/51; render check **166/166** on two runs (r166 on 25,737 and 23,674 frames); clippy (both) and `cargo fmt --check` clean.
- After M309: **4,455 tests, 5 failed in the chain's run**: `rewo-net` `srv::tests`' loopback TCP cases panicked on their fixture's `expect("a port free for both UDP and TCP")` while other jobs held ports (the flake recorded at M285); alone they passed 34/34 three times. Gates 51/51; render check **165/165** on two runs (r165 on 26,277 and 24,433 frames); clippy (both) and `cargo fmt --check` clean.
- After M308: **4,452 tests passed, 0 failed; gates 51/51**; render check **164/164** on two runs (r164 on 27,059 and 25,660 frames); clippy (both) and `cargo fmt --check` clean.
- After M307: **4,450 tests passed, 0 failed; gates 51/51** (mobtexshot 37, mobshot 276 views, the ETF fixture packing); render check **163/163** on two runs (both sulfur cube models seen on both); clippy (both) and `cargo fmt --check` clean.
- After M306: **4,448 tests passed, 0 failed; gates 51/51** after the emissive gate learned translucency (it read 50/51 first: the slime counted as glowing at light zero); render check **162/162** on two runs (r162 on 16,165 and 15,455 frames); clippy (both) and `cargo fmt --check` clean.
- After M305: **4,447 tests passed, 0 failed; gates 51/51**; render check **161/161** on two runs (r161: both sizes seen on both); clippy (both) and `cargo fmt --check` clean.
- After M304: **4,446 tests passed, 0 failed; gates 51/51** (mobtexshot 35, mobshot 270 views); render check **160/160** on two runs (r160 on 26,105 and 27,652 frames); clippy (both) and `cargo fmt --check` clean.
- After M303: **4,445 tests passed, 0 failed; gates 51/51** (mobtexshot 34, mobshot 267 views); render check **159/159** on two runs (r159 on 24,693 and 19,788 frames); clippy (both) and `cargo fmt --check` clean.
- After M302: **4,445 tests passed, 0 failed; gates 51/51** (mobtexshot 33; the ETF fixture packs again after the atlas growth); render check **158/158** on two runs (r158 on 27,805 and 27,913 frames); clippy (both) and `cargo fmt --check` clean.
- After M301: **4,444 tests passed, 0 failed; gates 51/51** (mobtexshot 32); render check **157/157** on two runs (r157 on 25,523 and 28,709 frames); clippy (both, pre-checked) and `cargo fmt --check` clean.
- After M300: **4,443 tests passed, 0 failed; gates 51/51** (mobtexshot 31); render check **156/156** on two runs (r156: saddled on 30,700 and 17,328 frames, ridden on 2,089 and 1,678); clippy (both, pre-checked) and `cargo fmt --check` clean.
- After M299: **4,442 tests passed, 0 failed; gates 51/51** (mobtexshot 30, m5 green again after the gutter; mobshot's ETF fixture packs with no warning after the second growth); render check **155/155** on two runs (r155 on 31,746 and 31,584 frames); clippy (both, pre-checked) and `cargo fmt --check` clean; the demo hash unchanged.
- After M298: **4,440 tests passed, 0 failed; gates 51/51** (mobtexshot 29: its first run caught the strider saddle at 0 px, coplanar under the strict depth test); render check **154/154** on two runs (r154 on 28,239 and 28,800 frames); clippy (both, pre-checked) and `cargo fmt --check` clean.
- After M297: **4,439 tests passed, 0 failed; gates 51/51** (mobtexshot 28; no "didn't pack" warning after the atlas growth; capeshot f2 extended); render check **153/153** on two runs (r153 on 22,408 and 27,897 frames); clippy (both, pre-checked) and `cargo fmt --check` clean; the demo hash unchanged.
- After M296: **4,437 tests passed, 0 failed; gates 51/51** (mobtexshot 27); render check **152/152** on two runs (r152 on 27,200 and 30,412 frames); clippy (both, pre-checked) and `cargo fmt --check` clean.
- After M295: **4,436 tests passed, 0 failed; gates 51/51** (mobtexshot 26); render check **151/151** on two runs (r151: saddled on 29,108 and 32,664 frames, ridden on 1,746 and 1,845); clippy (both, pre-checked) and `cargo fmt --check` clean; the demo hash unchanged.
- After M294: **4,435 tests passed, 0 failed; gates 51/51** (mobtexshot 25; no atlas packing warning); render check **150/150** on two runs (r150 on 29,631 and 31,496 frames); clippy (both, pre-checked) and `cargo fmt --check` clean; the demo hash unchanged.
- After M293: **4,434 tests passed, 0 failed; gates 51/51** (mobtexshot 24); render check **149/149** on two runs; clippy (both, pre-checked) and `cargo fmt --check` clean.
- After M292: **4,433 tests passed, 0 failed; gates 51/51** (capeshot 70: p5 294 px in the window, none outside, centroid on centre); render check **149/149** on two runs (r149 on the 6 frames the donkey screen was open; r86 still 0 player previews in 4,168 and 4,256 container frames); clippy (both, pre-checked) and `cargo fmt --check` clean.
- After M291: **4,433 tests passed, 0 failed; gates 51/51** (inventoryshot 173: gc1 matched every opaque texel of each sprite); render check **148/148** on two runs (unchanged: no stage opens a smithing table); clippy (both, pre-checked) and `cargo fmt --check` clean.
- After M290: **4,432 tests passed, 0 failed; gates 51/51**; render check **148/148** on two runs (r148 on 35,426 and 32,182 frames); clippy (both configurations, pre-checked) and `cargo fmt --check` clean.
- After M289: **4,432 tests passed, 0 failed; gates 51/51**; render check **147/147** on two runs (r147: the dropped head skinned on 27,848 and 33,113 frames, the framed one on 4,187 and 4,422); clippy (both configurations, pre-checked) and `cargo fmt --check` clean.
- After M288: **4,432 tests passed, 0 failed; gates 51/51** (mobshot's variant check: 69 declared sheets, the wither's in its own atlas slot); clippy and `cargo fmt --check` clean. Render check: run 1 145/146, r142 failing its own control (the bot's yaw happened to be -90.0, the bed camera's, left so by an earlier tail stage) and run 2 146/146; after the witness fix (the bot faces south before the bed click) two more runs read **146/146** (r146 on every run; r142's own yaw back at ~0).
- After M287: **4,430 tests passed, 0 failed; gates 51/51** (mobtexshot 23); render check **145/145** on two runs (r145: the wither's swirl sub-range filled on every frame it drew powered); clippy (both configurations, also run before the chain this time) and `cargo fmt --check` clean.
- After M286: **4,428 tests passed, 0 failed; gates 51/51** (mobtexshot 22: m22 over 2,631 wind px, by layer count 26 / 1,299 / 275 / 555 / 466 / 10); render check **144/144** on two runs (r144: the wind range filled on every frame the breeze drew); `cargo fmt --check` clean. Clippy failed once (`needless_borrows_for_generic_args` on the new emitter call); after the one-character fix both clippy configurations are clean, rewo-gpu's 361 tests pass and mobtexshot reads 22/22. Mutants: an opaque wind and an unsorted funnel each fail m22 (its first criterion, "half need two or more layers", let the unsorted one through).
- After M285: **4,427 tests passed, 0 failed; gates 51/51** (mobtexshot 21: m21 over 2,292 body px, 1,653 single layers, 1,631 sky px all with two or more); render check **143/143** on two runs (r143: the swirl range filled on every frame the charged creeper drew); `cargo fmt --check` clean. Clippy failed once on the new quad sort (`iter_cloned_collect`); after the one-line fix (`.to_vec()`) both clippy configurations are clean, rewo-gpu's 360 tests pass and mobtexshot reads 21/21 again. Before the chain, three mutants of the swirl (a white colour, no sort, a face shade) each failed m21; its first draft, graded against the jar's ramp texture, had let two of them through. One process slip: a single incremental `cargo check` for M285 started about three minutes after another session's `G4-RUNNING` marker appeared at 05:19 (the check was issued without re-reading the marker); every heavy step after that waited for it, and it cleared at 05:38.
- After M284: **4,424 tests passed** (the full run: 4,419 passed and 5 `rewo-net` `srv` TCP tests failed at "a port free for both UDP and TCP" under machine load; rerun alone and as the whole `rewo-net` library, 1,392/1,392 passed); **gates 51/51**; render check **142/142** on two runs (r142: camera yaw −90 against the player's ~0, no hand on 129 and 187 asleep frames); clippy and `cargo fmt --check` clean.
- After M283: **4,423 tests passed, 0 failed; gates 51/51** (mobshot's pumpkin-blur check: worst pixel 1); render check **141/141** on two runs; clippy and `cargo fmt --check` clean. The unit test's first draft expected the half-scale scope square one pixel off; vanilla's integer division puts it at (172, 67), which the code had.
- After M282: **4,422 tests passed, 0 failed; gates 51/51** (mobshot's vignette check: worst pixel 1); clippy and `cargo fmt --check` clean. Render check: run 2 **140/140**; run 1 139/140, r89 (the camel sprint) reading a step of 0.8203 against vanilla's 0.4101, exactly two ticks' worth in one sample, nothing M282 touches; a third run read **140/140** with r89's step at 0.4101, vanilla's.
- After M281: **4,421 tests passed, 0 failed; gates 51/51** (mobtexshot 20); render check **139/139** on two runs; clippy and `cargo fmt --check` clean.
- After M280: **4,420 tests passed, 0 failed; gates 51/51** (inventoryshot 172); render check **138/138** on two runs; clippy and `cargo fmt --check` clean.
- After M279: **4,419 tests passed, 0 failed; gates 51/51** (mobshot's outline-only check: 7,093 px, 0 stray, 0 inside); render check **138/138** on two runs (both with the port-race retry in place); clippy and `cargo fmt --check` clean.
- After M278: **4,419 tests passed, 0 failed; gates 51/51** (mobshot's outline check: 1,194 of 1,194 rim px, 0 far, 0 core); render check **137/137** on two runs, validation clean; clippy and `cargo fmt --check` clean. The mobshot criterion's first draft expected the rim to brighten toward red; vanilla's chain leaves 0.2–0.4 of the colour at full alpha, so over a light background the rim darkens to red, and the check grades the hue.
- After M277: **4,419 tests passed, 0 failed; gates 51/51** (hurtshot 57: c1 predicts [208, 116, 87] with 178 and the GPU gives it exactly; c6's white flash [223, 189, 143] exactly); render check **136/136** on two runs (r136: 39,271 / 33,180 swollen frames, 214 / 221 white); clippy and `cargo fmt --check` clean.
- After M276: **4,417 tests passed, 0 failed; gates 51/51**; render check **134/134** on two runs (r134's turn range 0.347, the full ±cos 80° a turning book gives); clippy (gates and default features) and `cargo fmt --check` clean.
- After M275: **4,415 tests passed, 0 failed; gates 51/51** (inventoryshot 171); render check **133/133** on two runs; clippy and `cargo fmt --check` clean. A first chain broke at clippy: g21 was edited in while that chain ran and did not compile (a closure needed `mut`); the rerun is on the fixed tree. Rule kept since: no source edits while a chain runs.
- After M274: **4,415 tests passed, 0 failed; gates 51/51** (mobtexshot 19); render check **132/132** on two runs; clippy (gates and default features) and `cargo fmt --check` clean. The bake test's first draft expected the feather to inherit `item/generated`'s `head`; the jar's feather declares its own, and the bake was right.
- After M273: **4,414 tests passed, 0 failed; gates 51/51** (mobtexshot 18); clippy (gates and default features) and `cargo fmt --check` clean; render check **131/131** on run 1 and on both runs after `5f8c2e9a`. Run 2 of the first chain read 126/131: slimes split by the use tail's own kill hit the bot from stage 70 on and r101-r104 and r106 timed out (fixed in `5f8c2e9a`, see Follow-ups). m18's control (no 1.1875 scale) fails at 2.52 px.
- After M272: **4,413 tests passed, 0 failed; gates 51/51**; render check **130/130** on two runs (r130 drew on 33,296 and 35,912 frames); clippy (gates and default features) and `cargo fmt --check` clean. r130 cannot pass on the old code (the per-skin model it looks for did not exist). The first chain attempt was stopped to raise the witness pin; its gate process outlived the stop and held `rewo.exe`, so the rerun's build failed until it was killed (verified its command line first).
- M205's load test is a scratch script, not a gate (it needs a 3 GB
  server and minutes of world generation): a normal world, seed 20261018,
  render distance 16, 75 s. Before: `276 columns, 197 deferred` at 94% of
  4M vertices. After: grown to 16M, `473 columns, 0 deferred`, no
  validation messages.

## Needs Enni

- **An incident on the shared PC (2026-10-19 13:28):** stopping a failed
  verification chain, I force-killed a `java.exe` (PID 37804, ~456 MB)
  assuming it was my render check's test server. It was not (my chain had
  not reached the render check; no `testserver-rc` directory existed), so it
  was some other session's server or Gradle daemon. Control Room was told at
  once. Since then I check a process's command line before stopping it.

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
  horse inventory~~ (M220), ~~steering boats~~ (M221), ~~steering horses~~
  (M222), ~~camels~~ (M225), ~~the nautilus~~ (M227). That list is done.
- ~~r78's survival-mining flake~~ (M224: the witness, not the client).
- ~~`tools/render_check.py`'s port probe lost a race once ("server FAILED TO
  BIND"): it picks a free port, then the server binds it later. A retry on
  that message would remove the one non-Rewo failure seen in the loops.~~ Retried on a new port since `b783723f` (up to three ports).
- r89 (the camel sprint) read a step of 0.8203 once (M282's first run),
  exactly twice vanilla's 0.4101: two ticks' movement landed in one of its
  samples. Its step measure should take the per-tick delta from the tick
  counter, not from consecutive samples.
- Two flakes seen during M245's checks, not investigated: `rewo-audio`'s
  `a_slow_read_is_not_re_requested_on_every_tick` (80 ms of wall-clock ticks
  for a worker to open a stream; failed once under the full workspace run,
  5/5 alone) and render-check r96 (walking while eating read a 0.31 ratio
  once against 0.2000 on the next two runs).
- ~~A render-check flake seen once (M248's run 2): the bot took damage from
  an unlogged source while walking east for r96, and the whole use tail
  failed after it.~~ Found during M273 (`5f8c2e9a`): slimes. The
  tail's `kill @e[type=minecraft:slime]` SPLIT any slime bigger than size 1
  (`AbstractCubeMob.remove`), and the medium children it made (ids 53-55,
  logged by M249's damage line) hit the bot through r101-r106. The tail now
  shrinks every slime to size 1 (`Size:0`) before the kill; a tiny slime
  neither splits nor deals damage. Why a slime exists at all with
  `gamerule spawn_monsters false` staged is not known (most likely one
  spawned in the seconds before the staging ran). Since M249 every hit on you is logged (`net: you took
  damage: damage type N, cause entity ..`; the type is the `damage_type`
  registry's wire index, the order of its `registry_data`), so the next
  sighting names the source.
- ~~A boat model~~ (M223).
- Player heads (M272-M275): a name-only profile (vanilla asks Mojang's
  profile service) and the profile's `texture` skin patch show Steve, and
  so does a skinned head in a hand, on the cursor, dropped or framed. Worn skulls do not animate,
  and a worn item shows no enchantment glint. The
  entity atlas's large band is a 64-slot ring, not an LRU: past 64 distinct
  big textures on screen at once it would recycle slots mid-frame.
- A mannequin's pose and description label are not drawn (nor the
  first-person arm's sleeve by the mask),
  and a bare name is not resolved to the account's real skin (Rewo phones
  nothing, so it shows the name's default skin); the sulfur
  cube's swallowed block, fuse swell and squish are not drawn.
- Banner and shield items show no patterns (the stack's `banner_patterns`
  never reach the item path), and a pot item shows plain sides. A dropped
  or framed trident shows its 3D model (vanilla: the sprite). The dragon
  head's 256×256 sheet is still too big for the large pool.
- ~~Not drawn: the enchanting table's floating book~~ (M276). M248 drew the bell and
  the lectern's book; ~~a rung bell does not shake~~ (it does: M264's
  `bell_shake`; this note was stale). Sign boards ARE drawn: in 26.2 they are ordinary block
  models (checked: oak sign 10-15 quads, wall and hanging signs too); an
  earlier draft of this note said otherwise.
- The render check still injects the sign's block entity by hand (r48/r49);
  since M247 the client would create it from the block update, so the
  injection could go. A decorated pot's sherds
  (`pot_decorations`) are not read: its icon shows four plain sides. The
  Christmas chest's date select takes its fallback.
- M242's third-person rotation fix (`held::rotate_xyz`) is unit-tested but
  not looked at in a capture: a held block or tool in another player's hand
  should now sit as vanilla's does.
- ~~Grass block GUI icon: top missing and untinted~~ (M242: the rotation
  order, not the tint). Still open on boats: the rowing animation, the
  hurt wobble, and the water mask (water can show inside a floating hull).
- `rewo-net`'s `srv` test `a_truncated_answer_is_retried_over_tcp` failed once
  under full-suite load (an empty TCP answer) and passed five times alone;
  the same loopback-timing class as the others listed here. After M225 five
  `srv` TCP tests failed together at the helper's "a port free for both UDP
  and TCP" (it binds a TCP ephemeral port, then UDP on the same number, 100
  tries); a helper that falls back to two separate ports for the TCP tests
  would remove this class.
- ~~The mount in the horse screen's window~~ (M292; horse armour does not
  show there) ~~and empty-slot icons~~ (M280).
- `rewo-net`'s `reconfigure` test `start_configuration_round_trips_back_to_play`
  failed once under full-suite load ("connection closed") and passed five
  times alone; the same class as the `play::inbound` timing tests.
- Skinned player heads in the first-person hand and on the cursor still
  draw Steve: inventory icons (M275), worn heads (M273), dropped and framed
  heads (M289) and heads other entities hold (M290) take the skin. The hand needs the hand pass's own held-item
  copy rebuilt when a skin arrives, and a live witness that holds the head
  without the bot's right-clicks placing it.
- The swirls (M285, M287) and the breeze's wind (M286): a glowing
  entity's layer is not copied into its outline, and fog is not applied to
  them.
- ~~The sleeping camera~~: done in M284. Left: vanilla eases the camera's
  eye height half the gap a tick (crouching too); Rewo's camera snaps.
- Several gate failure messages carry runs of spaces where a `\` line
  continuation was collapsed (e.g. `build_sounds`' panic, bookshot,
  containershot); cosmetic, since the September module split.
- `rewo-net`'s three `play::inbound` timing tests failed once under heavy
  machine load (they passed alone and in every other full run).
- `rewo-net`'s five `srv` TCP-retry tests failed together once (M284's
  full run) at `expect("a port free for both UDP and TCP")`: 100 tries found
  no TCP port whose UDP twin was free while the machine was loaded. They
  passed alone and in the whole library right after.
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
- **A render-check flake seen in M329's run 2** (174/183, then 183/183 on a
  third run, with no live code changed): nine tail witnesses (r42, r61,
  r75-r78, r81, r88, r91) found nothing where `REWO_PRECMD` had staged it
  near the bot ("no lectern block entity near the bot", "no villager near
  the bot"). The use tail's stage sequence matched run 1's; what differed is
  that the 105-command batch took 232 ms to send (2 ms in run 1) and the
  spawn sat at a (-3, +4) offset. Suspect: relative (`~`) placements landing
  while the bot's server-side position is still settling, now that the
  batch is longer (about 15 summons were added on 2026-10-20). Not
  investigated further.
- r91 (the rider's seat) failed alone in M333's run 2: the horse's client
  seat was 0.7 off in x (0.15 in y) from the server marker while the boat,
  camel and nautilus matched; runs 1 and 3 passed. It reads the seat and
  the marker on different ticks while the horse can still be moving.
- **Baby models still to do** (the sweep M314-M337 covered the
  rest): baby axolotls (M338-M341 did the llama's, horse's, cat's and
  wolf's coats; the axolotl needs its own keyframe rigs, seven of them,
  chosen by whether it is on the ground or in water and moving) (their adults draw one of several
  coats through `vanilla_variant`, and the baby sheets are separate files);
  the baby armadillo needs its own keyframe rigs (`BabyArmadilloAnimation`)
  and the shell swap. Until then those draw as half-size adults, and the
  armadillo still has M175's scrambled skin. M175's swap rows for kinds that now have their own
  model are dead (harmless) and could be dropped.
- **Gaits and poses the babies copy from their adults**: Rewo's hoglin uses
  the generic quadruped gait (vanilla's is frequency 1.0, amplitude 1.2);
  the adult feline swings its legs at 1.4 (vanilla 1.0; the ocelot kitten
  has vanilla's); the equine gait (0.5 and 0.8 amplitudes) and the equine
  head look are not modelled; the adult rabbit's head does not look (the
  kit's does, as vanilla's); no rig plays its scale channels; the fox's,
  panda's, feline's and camel's sitting and sleeping poses are not drawn.
- `mobtexshot`'s crowd (m5/m6) shrinks a little with every kind added
  (about 8% of a cell's width per row of ten). M329 enlarged it; when babies
  start moving other kinds between its buckets again, enlarge it again or
  grade the babies only up close.
- The default-variant fix (M340) covers the cat, wolf, frog, horse, llama
  and axolotl. The tropical fish needs none: its default
  (`TropicalFish.DEFAULT_VARIANT`, kob, white on white) packs to 0, which is
  what Rewo already reads a missing value as. (An earlier draft of this note
  said otherwise; the packing, `SMALL`'s id 0, kob's index 0 and white's dye
  id 0, says 0.)
- **The adult models' rest poses against their `setupAnim`** (found in
  M357: the adult feline's tail tip): audited on 2026-10-21 (read-only, five
  reviewers): `docs/rewo/rest-pose-audit-2026-10-21.md` lists ~35 confirmed
  discrepancies (villagers and illagers 6% too big, the piglins' meshes and
  ears, horse heads and legs, the rabbit's head, the turtle's
  flippers and egg belly, the strider, hoglin, snow golem, allay, guardian,
  dolphin, nautilus, happy ghast, warden, ravager, wither, ender dragon,
  breeze, bat, parrot, tadpole, bee antennae, camel/armadillo look clamps,
  axolotl, creeper phases) and the flag-driven states not modelled. Being
  worked through from M359.
- **An audit of every baby layer's rest pose against its `setupAnim`**
  (2026-10-20, after M339): six of the babies' rest angles turned out to be
  `setupAnim`'s and not the layer's (the hoglet's ears, the strider's body
  height, the goat kid's head, the foal's neck and tail, the puppy's tail;
  each done in its milestone), and the donkey foal's tail had been built
  wrong in M334 and was fixed in M339. Every other rotated part in the baby
  layers is either never assigned by its model or only in poses Rewo does not
  draw. One gap left, adult and baby alike: a hovering bee's `bobUpAndDown`
  tilts its body 0.1 and sets its front legs to π/8 (Rewo holds the body
  level and every leg at π/4). ~~Done in M354~~ (and the angry and nectar
  sheets and the stinger with it); a bee is still never posed on the ground.
- **A test flake on the shared PC (M353's chain, 23:1x)**: five
  `rewo-net` `srv::tests` (the loopback fake nameservers) could not find a
  port free for both UDP and TCP in 100 tries (`srv.rs` "a port free for both
  UDP and TCP"); all 34 passed on an immediate rerun. Environmental (other
  jobs holding loopback ports), but a hundred tries is the test's whole
  budget, so it can recur on a busy machine.
- **Gate messages with runs of spaces**: a few `format!` strings were written
  with `\` line continuations through a shell heredoc that turned each into a
  run of spaces (`gaugeshot`'s speed/poison row, `inventoryshot`'s description-
  key row); cosmetic, the pass/fail logic is unaffected (found in M352, whose
  own n11 message was fixed).
- **Blocks that mobs carry are not drawn** (found in M347): ~~the mooshroom's
  mushrooms~~ (M351), ~~the enderman's held block~~ (M352) ~~and the snow
  golem's pumpkin~~ (M353); all three done
  (`CarriedBlockLayer`) and the snow golem's pumpkin. Rewo draws block
  models for entities only as whole-entity blocks (falling blocks, TNT,
  minecart contents); a carried block needs the mob's part transform
  composed with the block draw.
