"""Stand up a FRESH vanilla test server and run `rewo live --render-check`.

    python tools/render_check.py [--keep] [--username NAME]

Written down because the recipe is the expensive part, not the gate: every trap
below has cost a previous session a run or an hour, and they are all recorded in
docs/history/AGENT_LOOP_BRIEF.md's "Test server" section.

* **A fresh DIRECTORY, not a shared one.** Unlocked recipes persist into the
  save, and r25 needs a multi-page recipe book — 2026-08-07's M105b looked green
  against a world where an earlier run's `recipe give` had already landed.
  `libraries/` and `versions/` are copied because they are the server jar's own
  extraction cache; `world/`, `logs/` and `usercache.json` are not.
* **PROBE the port.** M111 lost a run to an invented one: the server died on
  FAILED TO BIND and 27 of 28 witnesses passed anyway, because most inject their
  own scenes and only r25 needs a live server. So this binds a socket to pick
  the port AND greps the log before trusting the run.
* **`eula.txt` is copied byte-for-byte.** Writing it from PowerShell gives it a
  UTF-8 BOM that the server rejects.
* **The server stops on stdin EOF**, so it cannot be backgrounded from a shell.
  This holds the pipe open for the life of the run and sends `stop` at the end.
* **The op name must be the name you connect with**, with that name's offline
  UUID, or every REWO_PRECMD command is silently rejected and it reads as a
  client bug.
* **Two caller requirements.** r14 needs items in the hotbar and r25 needs a
  multi-page recipe book, so REWO_PRECMD below grants both. A bare run scores
  17/18 and is not a pass.
* Validation is `cfg!(debug_assertions)`-gated for `live`, so this insists on the
  DEBUG binary: in a release build r17 is false and r18 is vacuous.
* **It stages the two BLOCKING configuration tasks** (M166): `resource-pack`,
  `resource-pack-id` and `enable-code-of-conduct` plus a `codeofconduct/`
  directory. Unlike every other staging here this one is not about reaching a
  witness -- it is about reaching the run at all. Without M166's replies the
  server's configuration queue never advances, the client sits in
  `run_configuration` answering keep-alives, and the gate scores **nothing**.
* **It BUILDS.** It used only to check the binary existed, so a source change
  not followed by a manual `cargo build` was graded against the previous
  compile — which is how M151's first attempt to prove r47 non-vacuous reported
  a mutation as SURVIVED.
"""
import argparse
import base64
import hashlib
import io
import json
import os
import shutil
import socket
import subprocess
import sys
import tempfile

from gpu_lock import machine_lock
import time
import uuid
import struct
import zlib

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BASE = os.path.join(os.environ["APPDATA"], "EwoClient", "rewo", "26.2")
SRC = os.path.join(BASE, "testserver")
REWO = os.path.join(ROOT, "target", "debug", "rewo.exe")
PRECMD = (
    # M236 -- no hostile spawns. `spawn-monsters=false` in server.properties
    # no longer stops them, a flat world spawns slimes, and the use witnesses
    # run in survival: one slime killed the bot mid-tail ("slain by Slime")
    # and others left it at 3 health, failing a different witness each time.
    "gamerule spawn_monsters false;"
    "give @s minecraft:diamond_sword 1;"
    "give @s minecraft:dirt 64;"
    "recipe give @s *;"
    # M168 -- r57 / r58. An iron chestplate is 6 armour (three full icons);
    # the effect is INFINITE so it never reaches the 200-tick fade. The
    # `hideParticles` argument is deliberately NOT passed: `/effect give`
    # builds `new MobEffectInstance(effect, duration, amplifier, false,
    # particles)` (EffectCommands.java:186), and that five-argument
    # constructor is `this(.., visible, visible)` (MobEffectInstance.java:61)
    # -- so hiding the particles hides the ICON too. The first cut staged
    # `... 0 true` and r58 correctly measured zero icons.
    "item replace entity @s armor.chest with minecraft:iron_chestplate;"
    "effect give @s minecraft:speed infinite 0;"
    # M194 -- r71. A real item frame from the server, so its metadata slots
    # (8 DIRECTION, 9 ITEM_STACK, 10 INT) are calibrated against 26.2 rather
    # than against the decompile's count. `Fixed` keeps it from popping off
    # with nothing behind it; facing 2 and rotation 3 are both non-default,
    # so the server must send all three slots.
    # M179 -- r74. Two root advancements, so the advancements screen has two
    # tabs and a click on the one it did not open on is a CHANGE, which is
    # the only thing `PlayerAdvancements.setSelectedTab` echoes.
    # M184 -- r75. A lectern holding a two-page written book two blocks from
    # the bot; the client right-clicks it, turns a page, and closes it.
    "setblock ~2 ~ ~ minecraft:lectern[has_book=true]{Book:{id:\"minecraft:written_book\","
    "count:1,components:{\"minecraft:written_book_content\":{title:\"rc\",author:\"rc\","
    "pages:[\"one\",\"two\"]}}}};"
    # M216 -- r76. A librarian that cannot wander (NoAI) three blocks from
    # the bot, with two known trades: the client right-clicks it and the
    # decoded `merchant_offers` must be exactly these.
    "summon minecraft:villager ~ ~ ~3 {NoAI:1b,VillagerData:{profession:\"minecraft:librarian\","
    "level:2,type:\"minecraft:plains\"},Offers:{Recipes:["
    "{buy:{id:\"minecraft:emerald\",count:5},sell:{id:\"minecraft:book\",count:1},maxUses:12},"
    "{buy:{id:\"minecraft:paper\",count:24},sell:{id:\"minecraft:emerald\",count:1},maxUses:16}"
    "]}};"
    # M217 -- r78. Persistent oak leaves two blocks from the bot (destroy time
    # 0.2, no tool needed; leaves, unlike a carpet, survive the water r53's
    # waterlogged slab spreads around spawn): the client mines them in
    # survival.
    "setblock ~ ~ ~-2 minecraft:oak_leaves[persistent=true];"
    # M219 -- r83. A bed two blocks out (foot, then head to its west): the
    # client sleeps in it at night and leaves through the in-bed screen.
    "setblock ~-2 ~ ~2 minecraft:red_bed[facing=west,part=foot];"
    "setblock ~-3 ~ ~2 minecraft:red_bed[facing=west,part=head];"
    # M220 -- r84 / r85. A tamed, saddled donkey with a chest (five columns)
    # and three apples at chest index 7, standing still: the client opens its
    # inventory, rides it, and opens it again with the inventory key. West of
    # the bot: the item frame below hangs in the bot's head block on its
    # north side, and a ray to anything north-east crosses it first.
    "summon minecraft:donkey ~-3 ~ ~-1 {Tame:1b,ChestedHorse:1b,NoAI:1b,"
    "equipment:{saddle:{id:\"minecraft:saddle\",count:1}},"
    "Items:[{Slot:7b,id:\"minecraft:apple\",count:3}]};"
    # M221 -- r87. A pool five wide and fifteen long at ground level (water
    # replaces the grass layer) and an oak boat on it facing +z (yaw 0): the
    # client rides it and steers it with the production keys.
    "fill ~5 ~-1 ~4 ~9 ~-1 ~18 minecraft:water;"
    "summon minecraft:oak_boat ~7 ~ ~6 {Rotation:[0f,0f]};"
    # M222 -- r88. A tamed, saddled horse with a fixed speed and jump
    # strength, standing still, well away from the spawn's flood: the client
    # rides it, walks it west and jumps it.
    "summon minecraft:horse ~-10 ~ ~8 {Tame:1b,NoAI:1b,"
    "equipment:{saddle:{id:\"minecraft:saddle\",count:1}},"
    "attributes:[{id:\"minecraft:movement_speed\",base:0.3},"
    "{id:\"minecraft:jump_strength\",base:0.7}]};"
    # M225 -- r89. A saddled camel (vanilla's speed and jump), standing still
    # north-west of the bot on a lane of its own: the client rides it,
    # sprints it west and dashes it. With NBT given, `/summon` skips
    # `finalizeSpawn`, so LastPoseTick stays 0: standing, no transition.
    "summon minecraft:camel ~-8 ~ ~-12 {NoAI:1b,"
    "equipment:{saddle:{id:\"minecraft:saddle\",count:1}}};"
    # M230 -- r95. A floor lever and a stone button east of r87's pool, on
    # the ground: neither has a collision box, so only an outline-shape
    # pick can target them.
    "setblock ~12 ~ ~4 minecraft:lever[face=floor,facing=north];"
    "setblock ~10 ~ ~4 minecraft:stone_button[face=floor,facing=north];"
    # M232 -- r97 / r98. A crafting table and a furnace near the use spot,
    # the furnace holding an iron ingot in its result slot.
    "setblock ~14 ~ ~8 minecraft:crafting_table;"
    "setblock ~13 ~ ~9 minecraft:furnace[facing=north]{Items:[{Slot:2b,id:\"minecraft:iron_ingot\",count:1}]};"
    # M236 -- r102 / r103. An anvil and an enchanting table beside them.
    "setblock ~12 ~ ~9 minecraft:anvil[facing=north];"
    # M247 -- r109. A chest beside it: a block entity whose 64x64 sheet must
    # page into the entity atlas's large band, or its draw emits nothing.
    "setblock ~11 ~ ~9 minecraft:chest[facing=north];"
    # M248 -- r109 too: a bell, clear of r87's boat pool (~5..~9 x ~4..~18),
    # whose bell only a block-entity renderer draws. (r109's lectern book is
    # M184's lectern at ~2 ~ ~, which holds one.)
    "setblock ~15 ~ ~10 minecraft:bell[attachment=floor,facing=north];"
    # M252 -- r111. A minecart off the rails, south-west of the spawn and
    # clear of the pool and the use tail.
    "summon minecraft:minecart ~-6 ~ ~-6;"
    # M253 -- r112. A falling block and primed TNT, floating (NoGravity) so
    # neither lands nor explodes during the run: the gravel's Time starts
    # far below the 600-tick discard, the TNT's fuse is 32767 ticks.
    "summon minecraft:falling_block ~-8 ~2 ~-6 {BlockState:{Name:\"minecraft:gravel\"},NoGravity:1b,Time:-100000};"
    "summon minecraft:tnt ~-8 ~2 ~-8 {fuse:32767,NoGravity:1b};"
    # M254 -- r113. A stone wall and a 1x1 painting hung on its north face
    # (facing is the 2D value: 2 = north), off r89's camel lane (z -12)
    # and r87's pool.
    "fill ~-3 ~ ~16 ~-1 ~2 ~16 minecraft:stone;"
    "summon minecraft:painting ~-2 ~1 ~15 {facing:2b,variant:\"minecraft:kebab\"};"
    # M255 -- r114. A text display (a hologram), gold, facing the camera.
    "summon minecraft:text_display ~2 ~2 ~-4 {billboard:\"center\",shadow:1b,text:{text:\"RewoHolo\",color:\"gold\"}};"
    # M261 -- r120. Minecarts that carry something: a hopper and a chest.
    "summon minecraft:hopper_minecart ~-6 ~ ~-4;"
    "summon minecraft:chest_minecart ~-6 ~ ~-2;"
    # M260 -- r119. A block display of a state no bake makes a model for.
    "summon minecraft:block_display ~4 ~2 ~-7 {block_state:{Name:\"minecraft:oak_stairs\",Properties:{facing:\"east\"}}};"
    # M259 -- r118. Evoker fangs that bite 60 ticks after the summon, clear
    # of the camel's lane (z ~-12) and of the bot.
    "summon minecraft:evoker_fangs ~1 ~ ~-10 {Warmup:60};"
    # M258 -- r117. Three small projectiles held still (no acceleration,
    # no gravity, no motion), up out of reach of the bot.
    "summon minecraft:wither_skull ~-2 ~3 ~-9 {acceleration_power:0d,Motion:[0d,0d,0d]};"
    "summon minecraft:llama_spit ~-3 ~3 ~-9 {NoGravity:1b,Motion:[0d,0d,0d]};"
    "summon minecraft:shulker_bullet ~-4 ~3 ~-9 {NoGravity:1b,Motion:[0d,0d,0d]};"
    # M265 -- r123. A stone block display the run scales 1 -> 3 at 2 s;
    # M266 -- r124: and teleports up a block at 3 s, gliding over 20 ticks.
    "summon minecraft:block_display ~5 ~2 ~-9 {Tags:[\"rewo_interp\"],teleport_duration:20,block_state:{Name:\"minecraft:stone\"}};"
    # M271 -- r129. The classic server hologram: an invisible, named armour
    # stand with no gravity.
    "summon minecraft:armor_stand ~-8 ~ ~5 {Invisible:1b,NoGravity:1b,CustomNameVisible:1b,CustomName:\"RewoStand\"};"
    # M270 -- r128. A posed armour stand (head tipped, right arm raised).
    "summon minecraft:armor_stand ~-8 ~ ~3 {ShowArms:1b,Pose:{Head:[45f,0f,0f],RightArm:[-90f,0f,0f]}};"
    # M269 -- r127. Primed TNT with a 60-tick fuse, held fourteen blocks up
    # and away from everything, so its blast breaks nothing and hurts no one.
    "summon minecraft:tnt ~ ~14 ~-20 {fuse:60,NoGravity:1b};"
    # M264 -- r122. A redstone block beside r109's bell (placed after it, so
    # the neighbour change rings it rather than placing it powered).
    "setblock ~16 ~ ~10 minecraft:redstone_block;"
    # M257 -- r116. An end crystal, invulnerable so a stray click cannot
    # blow up the bot.
    "summon minecraft:end_crystal ~2 ~ ~-8 {Invulnerable:1b};"
    # M256 -- r115. An item display (an amethyst shard), fixed context;
    # M267 -- r125: dark by its brightness override (0, 0), which daylight
    # cannot match.
    "summon minecraft:item_display ~3 ~2 ~-5 {item:{id:\"minecraft:amethyst_shard\",count:1},item_display:\"fixed\",billboard:\"center\",brightness:{sky:0,block:0}};"
    "setblock ~10 ~ ~9 minecraft:enchanting_table;"
    # M237 -- r104. A brewing stand east of the use spot.
    "setblock ~13 ~ ~5 minecraft:brewing_stand;"
    # M238 -- r105. A plain armour stand and one with arms, no base plate
    # and an iron helmet.
    "summon minecraft:armor_stand ~-4 ~ ~3 {Rotation:[30f,0f]};"
    "summon minecraft:armor_stand ~-6 ~ ~3 {ShowArms:1b,NoBasePlate:1b,"
    "equipment:{head:{id:\"minecraft:iron_helmet\",count:1}}};"
    # M278 -- r137. A glowing armour stand (the Glowing tag: shared flag 6).
    "summon minecraft:armor_stand ~-14 ~ ~2 {Glowing:1b,NoGravity:1b};"
    # M281 -- r139. A burning armour stand (invulnerable, so it burns the
    # whole run).
    "summon minecraft:armor_stand ~-16 ~ ~3 {Fire:32767s,Invulnerable:1b,NoGravity:1b};"
    # M279 -- r138. And one invisible as well: its outline alone.
    "summon minecraft:armor_stand ~-14 ~ ~5 {Glowing:1b,Invisible:1b,NoGravity:1b};"
    # M277 -- r136. A creeper the run ignites at 2 s: NoAI and invulnerable,
    # with a fuse the server never reaches, clear of the lanes.
    "summon minecraft:creeper ~-14 ~ ~-2 {NoAI:1b,Invulnerable:1b,Fuse:32767s,Tags:[\"rewo_creeper\"]};"
    # M285 -- r143. A charged creeper (never ignited: no tag), beside it.
    "summon minecraft:creeper ~-16 ~ ~-2 {NoAI:1b,Invulnerable:1b,Fuse:32767s,powered:1b};"
    # M286 -- r144. A breeze, NoAI (it never shoots), beside them.
    "summon minecraft:breeze ~-18 ~ ~-2 {NoAI:1b,Invulnerable:1b,Silent:1b};"
    # M287 -- r145. A wither below half health (100 of 300), NoAI and silent,
    # further along the same row.
    "summon minecraft:wither ~-22 ~ ~-2 {NoAI:1b,Invulnerable:1b,Silent:1b,Health:100f};"
    # M288 -- r146. A wither frozen in its invulnerable phase: Invul 220 never
    # counts down under NoAI (no `customServerAiStep`), so it never explodes.
    "summon minecraft:wither ~-26 ~ ~-2 {NoAI:1b,Silent:1b,Invul:220};"
    # M294 -- r150. A horse in iron horse armour, clear of the lanes.
    "summon minecraft:horse ~-18 ~ ~-6 {NoAI:1b,Tame:1b,Silent:1b,"
    "equipment:{body:{id:\"minecraft:iron_horse_armor\",count:1}}};"
    # M297 -- r153. A chested llama in a red carpet, beside it.
    "summon minecraft:llama ~-14 ~ ~-8 {NoAI:1b,Tame:1b,Silent:1b,ChestedHorse:1b,"
    "equipment:{body:{id:\"minecraft:red_carpet\",count:1}}};"
    # M298 -- r154. A saddled pig and a saddled strider, clear of the lanes.
    "summon minecraft:pig ~-10 ~ ~-8 {NoAI:1b,Silent:1b,"
    "equipment:{saddle:{id:\"minecraft:saddle\",count:1}}};"
    "summon minecraft:strider ~-17 ~ ~-9 {NoAI:1b,Silent:1b,"
    "equipment:{saddle:{id:\"minecraft:saddle\",count:1}}};"
    # M299 -- r155. A wolf in wolf armour.
    "summon minecraft:wolf ~-21 ~ ~-7 {NoAI:1b,Silent:1b,"
    "equipment:{body:{id:\"minecraft:wolf_armor\",count:1}}};"
    # M302 -- r158. A happy ghast in a white harness, floating clear of the
    # ground lanes.
    "summon minecraft:happy_ghast ~-26 ~6 ~14 {NoAI:1b,Silent:1b,"
    "equipment:{body:{id:\"minecraft:white_harness\",count:1}}};"
    # M303 -- r159. A saddled camel husk, clear of r89's camel lane (z ~-12).
    "summon minecraft:camel_husk ~-20 ~ ~-17 {NoAI:1b,Silent:1b,"
    "equipment:{saddle:{id:\"minecraft:saddle\",count:1}}};"
    # M304 -- r160. A giant (12 blocks tall), well clear of every lane.
    "summon minecraft:giant ~-34 ~ ~-24 {NoAI:1b,Silent:1b};"
    # M305 -- r161. Magma cubes of size 1 (NBT Size is the size minus one)
    # and size 4, clear of the lanes.
    "summon minecraft:magma_cube ~-30 ~ ~0 {NoAI:1b,Silent:1b,Size:0};"
    "summon minecraft:magma_cube ~-30 ~ ~4 {NoAI:1b,Silent:1b,Size:3};"
    # M306 -- r162. A slime (size 2), clear of the lanes.
    "summon minecraft:slime ~-30 ~ ~8 {NoAI:1b,Silent:1b,Size:1};"
    # M307 -- r163. An adult sulfur cube (size 2) and a baby (AgeableMob's
    # negative Age), clear of the lanes.
    "summon minecraft:sulfur_cube ~-30 ~ ~12 {NoAI:1b,Silent:1b,Size:1};"
    "summon minecraft:sulfur_cube ~-30 ~ ~15 {NoAI:1b,Silent:1b,Age:-24000};"
    # M308 -- r164. A mannequin with its default (empty) profile.
    "summon minecraft:mannequin ~-27 ~ ~18 {immovable:1b};"
    # M309 -- r165. A mannequin with a profile known only by a name no
    # account has.
    "summon minecraft:mannequin ~-24 ~ ~18 {immovable:1b,profile:\"RewoDummyA\"};"
    # M310 -- r166. A mannequin with two outer layers hidden.
    "summon minecraft:mannequin ~-21 ~ ~18 {immovable:1b,"
    "hidden_layers:[\"hat\",\"jacket\"]};"
    # M334 -- r188. A donkey foal and a mule foal.
    "summon minecraft:donkey ~45 ~ ~21 {NoAI:1b,Silent:1b,Age:-24000};"
    "summon minecraft:mule ~48 ~ ~21 {NoAI:1b,Silent:1b,Age:-24000};"
    # M333 -- r187. A baby bee.
    "summon minecraft:bee ~42 ~ ~21 {NoAI:1b,Silent:1b,Age:-24000};"
    # M332 -- r186. An ocelot kitten.
    "summon minecraft:ocelot ~39 ~ ~21 {NoAI:1b,Silent:1b,Age:-24000};"
    # M331 -- r185. A panda cub (the normal gene, so it draws the base
    # sheet Rewo knows).
    "summon minecraft:panda ~36 ~ ~21 {NoAI:1b,Silent:1b,Age:-24000,"
    "MainGene:\"normal\",HiddenGene:\"normal\"};"
    # M330 -- r184. A kit.
    "summon minecraft:fox ~33 ~ ~21 {NoAI:1b,Silent:1b,Age:-24000};"
    # M328 -- r183. A baby strider.
    "summon minecraft:strider ~30 ~ ~21 {NoAI:1b,Silent:1b,Age:-24000};"
    # M327 -- r182. A hoglet (immune to zombification: the overworld
    # converts a hoglin as it does a piglin) and a baby zoglin.
    "summon minecraft:hoglin ~24 ~ ~21 {NoAI:1b,Silent:1b,Age:-24000,"
    "IsImmuneToZombification:1b};"
    "summon minecraft:zoglin ~27 ~ ~21 {NoAI:1b,Silent:1b,IsBaby:1b};"
    # M326 -- r181. A baby squid and a baby glow squid. On land a squid
    # loses air in `baseTick` whatever its AI, so each starts with 27 min.
    "summon minecraft:squid ~18 ~ ~21 {NoAI:1b,Silent:1b,Age:-24000,Air:32767s};"
    "summon minecraft:glow_squid ~21 ~ ~21 {NoAI:1b,Silent:1b,Age:-24000,"
    "Air:32767s};"
    # M325 -- r180. A baby zombie villager, helmeted against the daylight.
    "summon minecraft:zombie_villager ~15 ~ ~21 {NoAI:1b,Silent:1b,IsBaby:1b,"
    "equipment:{head:{id:\"minecraft:leather_helmet\",count:1}}};"
    # M324 -- r179. A dolphin calf (NoAI on land: it does not dry out
    # while its AI is off, and the witness reads only that it drew).
    "summon minecraft:dolphin ~12 ~ ~21 {NoAI:1b,Silent:1b,Age:-24000};"
    # M323 -- r178. A red lamb.
    "summon minecraft:sheep ~9 ~ ~21 {NoAI:1b,Silent:1b,Age:-24000,Color:14b};"
    # M322 -- r177. A piglet and a chick.
    "summon minecraft:pig ~3 ~ ~21 {NoAI:1b,Silent:1b,Age:-24000};"
    "summon minecraft:chicken ~6 ~ ~21 {NoAI:1b,Silent:1b,Age:-24000};"
    # M321 -- r176. A calf and a mooshroom calf.
    "summon minecraft:cow ~-3 ~ ~21 {NoAI:1b,Silent:1b,Age:-24000};"
    "summon minecraft:mooshroom ~0 ~ ~21 {NoAI:1b,Silent:1b,Age:-24000};"
    # M320 -- r175. A baby piglin (immune to zombification: the overworld
    # converts a piglin after 15 s, whatever its AI) and a baby zombified
    # piglin.
    "summon minecraft:piglin ~-9 ~ ~21 {NoAI:1b,Silent:1b,IsBaby:1b,"
    "IsImmuneToZombification:1b};"
    "summon minecraft:zombified_piglin ~-6 ~ ~21 {NoAI:1b,Silent:1b,IsBaby:1b};"
    # M319 -- r174. A baby drowned (helmeted against the sun).
    "summon minecraft:drowned ~-12 ~ ~21 {NoAI:1b,Silent:1b,IsBaby:1b,"
    "equipment:{head:{id:\"minecraft:leather_helmet\",count:1}}};"
    # M318 -- r173. A baby villager.
    "summon minecraft:villager ~-15 ~ ~21 {NoAI:1b,Silent:1b,Age:-24000};"
    # M317 -- r172. A baby zombie (a helmet keeps the sun off it) and a baby
    # husk.
    "summon minecraft:zombie ~-21 ~ ~21 {NoAI:1b,Silent:1b,IsBaby:1b,"
    "equipment:{head:{id:\"minecraft:leather_helmet\",count:1}}};"
    "summon minecraft:husk ~-18 ~ ~21 {NoAI:1b,Silent:1b,IsBaby:1b};"
    # M316 -- r171. A polar bear cub.
    "summon minecraft:polar_bear ~-24 ~ ~21 {NoAI:1b,Silent:1b,Age:-24000};"
    # M315 -- r170. A baby turtle.
    "summon minecraft:turtle ~-27 ~ ~21 {NoAI:1b,Silent:1b,Age:-24000};"
    # M314 -- r169. A ghastling (AgeableMob's negative Age), floating clear.
    "summon minecraft:happy_ghast ~-30 ~3 ~20 {NoAI:1b,Silent:1b,Age:-24000};"
    # M311 -- r167. A wolf in badly damaged armour (60 of 64 spent).
    "summon minecraft:wolf ~-18 ~ ~18 {NoAI:1b,Silent:1b,"
    "equipment:{body:{id:\"minecraft:wolf_armor\",count:1,"
    "components:{\"minecraft:damage\":60}}}};"
    # M274 -- r132. An armour stand wearing a carved pumpkin (no equipment
    # asset, so `CustomHeadLayer`'s item branch draws it).
    "summon minecraft:armor_stand ~-12 ~ ~3 {equipment:{head:{id:\"minecraft:carved_pumpkin\",count:1}}};"
    "advancement grant @s only minecraft:story/root;"
    "advancement grant @s only minecraft:adventure/root;"
    "summon minecraft:item_frame ~ ~1 ~ "
    "{Facing:2b,Fixed:1b,ItemRotation:3b,Item:{id:\"minecraft:diamond_sword\",count:1}}"
)

# M166 -- the two BLOCKING configuration tasks, staged so the gate exercises the
# hang it fixes. `addOptionalTasks` queues a code-of-conduct task and a
# resource-pack task, neither of which finishes itself, so before M166 a server
# with either of these set left the client in `run_configuration` forever: no
# window, no error, and the 30s socket timeout unreachable because a keep-alive
# lands every 15s. With the fix missing this run scores **0 witnesses**, not 54.
#
# The server never fetches the URL -- `getServerPackInfo` only checks it is
# non-empty -- so an unroutable one is fine and keeps the run offline.
PACK_ID = "0f1e2d3c-4b5a-4988-8776-655443322110"
PACK_URL = "http://127.0.0.1:1/rewo-render-check.zip"
# Single line, ASCII, no section signs: the server joins the file's lines with a
# newline and runs `StringUtil.stripColor` over the result, so anything else
# would make the staged string and the delivered one disagree for reasons that
# have nothing to do with the client.
COC_TEXT = "Be excellent to each other."


def offline_uuid(name):
    """`UUID.nameUUIDFromBytes(("OfflinePlayer:" + name).getBytes(UTF_8))` —
    an MD5 UUID with the version and variant nibbles forced."""
    b = bytearray(hashlib.md5(("OfflinePlayer:" + name).encode("utf-8")).digest())
    b[6] = (b[6] & 0x0F) | 0x30
    b[8] = (b[8] & 0x3F) | 0x80
    return str(uuid.UUID(bytes=bytes(b)))


def nautilus_staging(name):
    """M227 -- r90. A channel three deep (the grass and both dirt layers
    turned to water, over the bedrock) running 43 blocks west on a lane of
    its own, and a nautilus in it: tamed by the bot (`Owner`, which
    `TamableAnimal` reads as tame) and saddled, so a plain right-click rides
    it; NoAI keeps it put until then. The owner is the UUID as a STRING:
    NBT's `[I;...]` int array carries a `;`, which is REWO_PRECMD's command
    separator, and `EntityReference.readWithOldOwnerConversion` takes a
    string longer than 16 characters through `UUID.fromString`."""
    return (
        ";fill ~-10 ~-3 ~-21 ~-52 ~-1 ~-19 minecraft:water"
        ";summon minecraft:nautilus ~-12 ~-2 ~-20 {NoAI:1b,Owner:\"%s\","
        "equipment:{saddle:{id:\"minecraft:saddle\",count:1}}}" % offline_uuid(name)
    )


# M272 -- r130. A player head wearing a skin of its own: its profile's
# `textures` property names a textures.minecraft.net URL. The client is told
# to read every head skin from a local file instead (`REWO_HEAD_SKIN_FILE`, a
# gates knob), so the run stays offline; the file is a synthetic skin whose
# texel (x, y) is (4x, 4y, 0x80, 0xff), which the witness checks.
HEAD_SKIN_URL = "http://textures.minecraft.net/texture/" + "ab" * 32
HEAD_TEXTURES = base64.b64encode(
    json.dumps({"textures": {"SKIN": {"url": HEAD_SKIN_URL}}}).encode("ascii")
).decode("ascii")
HEAD_STAGING = (
    ";setblock ~-10 ~ ~1 minecraft:player_head{profile:{properties:[{name:\"textures\",value:\"%s\"}]}}"
    # M273 -- r131. An armour stand wearing a player head in the same skin.
    ";summon minecraft:armor_stand ~-10 ~ ~3 {equipment:{head:{id:\"minecraft:player_head\",count:1,"
    "components:{\"minecraft:profile\":{properties:[{name:\"textures\",value:\"%s\"}]}}}}}"
    # M275 -- r133. A player head ITEM in the same skin, in the bot's last
    # main-inventory slot (clear of the hotbar the use tail fills).
    ";item replace entity @s inventory.26 with minecraft:player_head"
    "[minecraft:profile={properties:[{name:\"textures\",value:\"%s\"}]}]"
    # M289 -- r147. The same head dropped (never picked up or despawned) and
    # in a floor-mounted item frame.
    ";summon minecraft:item ~-11 ~ ~6 {PickupDelay:32767,Age:-32768,Item:{id:\"minecraft:player_head\","
    "count:1,components:{\"minecraft:profile\":{properties:[{name:\"textures\",value:\"%s\"}]}}}}"
    ";summon minecraft:item_frame ~-11 ~ ~8 {Facing:1b,Fixed:1b,Item:{id:\"minecraft:player_head\","
    "count:1,components:{\"minecraft:profile\":{properties:[{name:\"textures\",value:\"%s\"}]}}}}"
    # M290 -- r148. An armour stand holding the same head in its main hand.
    ";summon minecraft:armor_stand ~-13 ~ ~7 {ShowArms:1b,equipment:{mainhand:{id:\"minecraft:player_head\","
    "count:1,components:{\"minecraft:profile\":{properties:[{name:\"textures\",value:\"%s\"}]}}}}}"
    % (HEAD_TEXTURES, HEAD_TEXTURES, HEAD_TEXTURES, HEAD_TEXTURES, HEAD_TEXTURES, HEAD_TEXTURES)
)


def synthetic_skin_png():
    """A 64x64 RGBA PNG whose texel (x, y) is (4x, 4y, 0x80, 0xff)."""
    raw = b"".join(
        b"\x00" + bytes(v for x in range(64) for v in (4 * x, 4 * y, 0x80, 0xFF))
        for y in range(64)
    )

    def chunk(kind, data):
        return (struct.pack(">I", len(data)) + kind + data
                + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF))

    return (b"\x89PNG\r\n\x1a\n"
            + chunk(b"IHDR", struct.pack(">IIBBBBB", 64, 64, 8, 6, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(raw))
            + chunk(b"IEND", b""))


def free_port():
    s = socket.socket()
    s.bind(("127.0.0.1", 0))
    port = s.getsockname()[1]
    s.close()
    return port



def report_path():
    """Where this run's full output goes: per process, so two runs (or a
    stale file from an earlier run) can never be mistaken for this one."""
    return os.path.join(tempfile.gettempdir(),
                        "rewo-render-check-%d.out" % os.getpid())

def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--keep", action="store_true", help="leave the server dir behind")
    ap.add_argument("--username", default="RewoBot")
    args = ap.parse_args()

    # **Build first.** This used to only check that the binary EXISTED, which
    # made every run grade whatever was last compiled. M151 lost a mutation
    # verification to it: `keys.tab_list` was replaced with a literal `true`,
    # the gate reported the unmutated ~50% frame count, and the mutation read
    # as SURVIVED — the same shape as the leftover-mutant-binary hazard the
    # `*_mutate.py` batteries carry a rebuild for, running the other way round.
    b = subprocess.run(["cargo", "build", "-p", "rewo-app", "--features", "gates"], cwd=ROOT)
    if b.returncode != 0:
        sys.exit("cargo build failed -- nothing below would be about this tree")
    if not os.path.exists(REWO):
        sys.exit("no debug binary at %s after a successful build" % REWO)
    # One GPU verification per machine (tools/gpu_lock.py): a second render
    # check, or a gate sweep, on the same desktop would share the GPU, the
    # window focus and this script's report path.
    with machine_lock("tools/render_check.py"):
        # The port probe can lose a race: `free_port` binds and releases a
        # port, and the server binds it seconds later. A lost race is the
        # harness's, not the client's, so it retries on a new port (seen
        # twice in the 2026-10-20 run).
        for attempt in range(3):
            if run(args) is not BIND_RACE:
                break
            print("server FAILED TO BIND (attempt %d) -- retrying on a new port" % (attempt + 1))
        else:
            sys.exit("server FAILED TO BIND three times")


class BindRace(Exception):
    """The server lost the port to another process before binding it."""


BIND_RACE = "bind-race"


# M206 -- r73: the client's staged render distance. Below the test server's
# view-distance (8), so the server's tracking set is the CLIENT's choice.
CLIENT_VIEW_DISTANCE = 6


def run(args):
    port = free_port()
    dest = os.path.join(BASE, "testserver-rc-%d" % port)
    if os.path.exists(dest):
        sys.exit("%s already exists; a reused directory is not a fresh one" % dest)

    os.makedirs(dest)
    game_dir = os.path.join(dest, "client-game")
    os.makedirs(game_dir)
    io.open(os.path.join(game_dir, "options.txt"), "wb").write(
        ("renderDistance:%d\n" % CLIENT_VIEW_DISTANCE).encode())
    for f in ("server.jar", "eula.txt"):
        shutil.copyfile(os.path.join(SRC, f), os.path.join(dest, f))
    for d in ("libraries", "versions"):
        if os.path.isdir(os.path.join(SRC, d)):
            shutil.copytree(os.path.join(SRC, d), os.path.join(dest, d))
    assert io.open(os.path.join(dest, "eula.txt"), "rb").read().startswith(b"eula=true"), "eula"

    props = io.open(os.path.join(SRC, "server.properties"), "rb").read().decode("utf-8")
    out, hits = [], 0
    # M166 stages these keys (M236 added the fourth). They are REPLACED IN PLACE and each must be hit
    # exactly once: a vanilla server writes its whole default property set, so
    # all three are already present and appending would leave two copies with
    # the server reading whichever comes last -- the gate would then grade a
    # value it did not choose. The hit count also fails loud if a key is ever
    # dropped from the template, which is the direction that would silently
    # un-stage the run.
    staged = {
        "resource-pack": PACK_URL,
        "resource-pack-id": PACK_ID,
        "enable-code-of-conduct": "true",
        # M236 -- the seed is random per run (`level-seed=` is empty), and a
        # flat world still places villages: one run spawned the bot on a
        # roof six blocks up and two dozen witnesses failed with it. No
        # structures, so every spawn is on the flat surface.
        "generate-structures": "false",
    }
    staged_hits = dict.fromkeys(staged, 0)
    for line in props.splitlines():
        if line.startswith("server-port="):
            line = "server-port=%d" % port
            hits += 1
        key = line.split("=", 1)[0]
        if key in staged:
            line = "%s=%s" % (key, staged[key])
            staged_hits[key] += 1
        out.append(line)
    assert staged_hits == dict.fromkeys(staged, 1), (
        "M166 staging did not land exactly once per key: %r" % staged_hits
    )
    assert hits == 1, "expected exactly one server-port line, found %d" % hits
    io.open(os.path.join(dest, "server.properties"), "wb").write(
        ("\n".join(out) + "\n").encode("utf-8")
    )
    # `enable-code-of-conduct=true` with no `codeofconduct/` directory is a
    # startup THROW, not a warning -- so this file is part of the staging, not a
    # nicety.
    os.makedirs(os.path.join(dest, "codeofconduct"))
    io.open(os.path.join(dest, "codeofconduct", "en_us.txt"), "wb").write(
        (COC_TEXT + chr(10)).encode("utf-8")
    )
    io.open(os.path.join(dest, "ops.json"), "wb").write(
        json.dumps(
            [{
                "uuid": offline_uuid(args.username),
                "name": args.username,
                "level": 4,
                "bypassesPlayerLimit": True,
            }],
            indent=2,
        ).encode("utf-8")
    )

    log = io.open(os.path.join(dest, "server-run.log"), "wb")
    print("starting server on port %d in %s" % (port, dest))
    srv = subprocess.Popen(
        ["java", "-Xmx2G", "-jar", "server.jar", "nogui"],
        cwd=dest,
        stdin=subprocess.PIPE,  # held open: the server stops on stdin EOF
        stdout=log,
        stderr=subprocess.STDOUT,
    )
    code = None
    try:
        deadline = time.time() + 180
        ready = False
        while time.time() < deadline:
            time.sleep(2)
            txt = io.open(os.path.join(dest, "server-run.log"), "rb").read().decode("utf-8", "replace")
            if "FAILED TO BIND" in txt:
                raise BindRace(port)
            if srv.poll() is not None:
                sys.exit("server exited early with %s" % srv.returncode)
            if 'Done (' in txt:
                ready = True
                break
        if not ready:
            sys.exit("server never printed Done( within 180s")
        print("server up; running render-check as %s" % args.username)

        env = dict(os.environ)
        env["REWO_PRECMD"] = PRECMD + nautilus_staging(args.username) + HEAD_STAGING
        skin = os.path.join(dest, "head-skin.png")
        io.open(skin, "wb").write(synthetic_skin_png())
        env["REWO_HEAD_SKIN_FILE"] = skin
        # The client no longer injects a dirt stack into slot 0 on spawn
        # unless asked (it was item synthesis in the shipped build). No
        # witness needs the stack, but the baseline was calibrated with slot 0
        # holding dirt and selected, so keep that scene.
        env["REWO_GIVE_DIRT"] = "1"
        # One source of truth for both halves of r55/r56: the same literals
        # that went into server.properties are handed to the client, so the
        # witness compares what came off the WIRE against what the server was
        # configured with. Two hard-coded copies would drift; this cannot.
        env["REWO_RC_PACK_ID"] = PACK_ID.replace("-", "")
        env["REWO_RC_COC"] = COC_TEXT
        # M206 -- r73. The client's render distance comes from options.txt in
        # its game directory (vanilla's `buildPlayerInformation`), so stage
        # one, below the server's view-distance, and tell the witness the same
        # literal. The game directory is a scratch folder inside the server's
        # (removed with it), never the repo root, which was the client's game
        # directory before -- an options save during the run would have
        # written into the working tree.
        env["REWO_RC_VIEW_DISTANCE"] = str(CLIENT_VIEW_DISTANCE)
        # TIMEOUT, because the client's worst failure mode is not a crash.
        # A configuration-state packet the client fails to answer leaves it
        # reading keep-alives forever (M166), and an untimed `run` inherits
        # that: the gate hangs with no output, no exit code and nothing to
        # diagnose. Same family as the mutation-harness hang M138d records,
        # where a hung child took the harness down and left a mutation on disk.
        # A healthy run is ~15s; the ceiling only has to be well clear of a
        # cold asset bake.
        try:
            p = subprocess.run(
                [REWO, "live", "--render-check",
                 "--host", "127.0.0.1", "--port", str(port),
                 "--username", args.username,
                 "--game-dir", game_dir],
                cwd=ROOT,
                env=env,
                capture_output=True,
                timeout=float(os.environ.get("REWO_RC_TIMEOUT", "300")),
            )
        except subprocess.TimeoutExpired as e:
            out = (e.stdout or b"") + (e.stderr or b"")
            io.open(report_path(), "wb").write(out)
            sys.exit(
                "the client did not exit within the timeout -- it is HUNG, not "
                "slow. The usual cause is a configuration-state packet nothing "
                "answers, which stalls the server's task queue while keep-alives "
                "keep the socket alive (see crates/rewo-net/src/config_tasks.rs)."
            )
        code = p.returncode
        text = (p.stdout + p.stderr).decode("utf-8", "replace")
        # **Write bytes, do not `print` this.** The gate's rows contain arrows
        # and em dashes, and Python's stdout on Windows is cp1252 — printing it
        # raises UnicodeEncodeError *after* the run has succeeded, which loses
        # the exit code and reads as a failed gate. Same family as M95's
        # mutation harness, where a locale-decoded em dash made every verdict
        # read KILLED.
        # In TEMP, not the repo: a tool that drops an untracked artifact into
        # the working tree makes `git status` lie about whether a run left
        # anything behind, and that check is worth more than the convenience.
        report = report_path()
        io.open(report, "wb").write(text.encode("utf-8"))
        sys.stdout.buffer.write(text[-9000:].encode("utf-8", "replace"))
        # The tail above can cut off the failing witness (a CI log once showed
        # only "65/66"); name every non-PASS witness line explicitly.
        bad = [l for l in text.splitlines()
               if l.startswith("[rendercheck] ") and " PASS " not in l
               and not l.rstrip().endswith("witnesses")]
        if bad:
            sys.stdout.buffer.write(
                ("\nNOT PASSING:\n" + "\n".join(bad) + "\n").encode("utf-8", "replace"))
        sys.stdout.flush()
        print("\nRENDER_CHECK_EXIT=%s  (full output in %s)" % (code, report))
    finally:
        try:
            srv.stdin.write(b"stop\n")
            srv.stdin.flush()
            srv.stdin.close()
            srv.wait(timeout=60)
        except Exception:
            srv.kill()
        log.close()
        if not args.keep:
            shutil.rmtree(dest, ignore_errors=True)
            print("removed %s" % dest)
        else:
            print("kept %s" % dest)
        # A lost port race is retried by `main`: swallow it here (a `return`
        # in `finally` discards the in-flight exception) rather than exit.
        if isinstance(sys.exc_info()[1], BindRace):
            return BIND_RACE
    sys.exit(code if code is not None else 1)


if __name__ == "__main__":
    main()
