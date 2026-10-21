# Adult mob rest-pose audit (2026-10-21)

After M357 found the adult feline's tail tip built from the layer's angle
instead of `setupAnim`'s (the "setupAnim trap"), a read-only audit compared
every adult mob Rewo builds (`crates/rewo-gpu/src/mobs.rs` builders and
`entities.rs` `anim_delta`) against the 26.2 decompile's `createBodyLayer` and
`setupAnim` (and the renderers' extra rotations). Five parallel reviewers
covered farm quadrupeds, mounts and small mobs, monsters and aquatic mobs,
golems and bosses, and humanoids. Line numbers are as of commit `e503ea97`; verify each
against the decompile before fixing (the audit was read-only and not
re-checked line by line).

The baby layers had their own audit on 2026-10-20 (see
`docs/OVERNIGHT-2026-10-20.md`, Follow-ups).

## Confirmed, most visible first

| Mob | Finding | Vanilla | Status |
|---|---|---|---|
| horse, donkey, mule, skeleton/zombie horse | head and neck never look (pitch + π/6, yaw clamped ±20°, a walk nod) | `AbstractEquineModel` 121-127 | M359 (adults; the foals still hold still) |
| horse family | legs swing 1.4×; vanilla front 0.8, hind 0.5 | `AbstractEquineModel` 124-125, 138-143 | M359 |
| horse family | tail does not lift while walking (`π/6 + speed·0.75`, y/z offsets) | `AbstractEquineModel` 146-148 | M359 (the random `animateTail` swish not modelled) |
| rabbit | head pitched 0.3927 (layer) and never looks; `setupAnim` assigns the look | `RabbitModel` 28-29 | M364 (the idle head tilt not played) |
| turtle | flippers never move (land paddle `cos(pos·5)·8·speed` yRot, water swim) | `TurtleModel` 20-36 | M365 (on land = out of water; Rewo keeps no `onGround` for other entities) |
| turtle | egg belly drawn on every adult; vanilla `eggBelly.visible = hasEgg` (and the root 1 px up) | `AdultTurtleModel` 24-26 | M365 |
| strider | body 1 px too high (`body.y = 2`, layer 1); body never looks; bristles never sway; legs 1.4× unclamped (vanilla `min(speed, 0.25)`) | `AdultStriderModel` 54-57, `StriderModel` 25-57 | M366 (adult and baby) |
| hoglin, zoglin (and babies) | head pitches with the look; vanilla's xRot is fixed 0.8727 when not attacking | `HoglinModel` 91, 102 | M367 (with the headbutt) |
| hoglin | legs at 0.6662/1.4; vanilla raw pos ·1.2; ears never flap | `HoglinModel` 89-98 | M367 |
| snow golem | upper body and arms never follow a quarter of the head yaw; arm pivots' z off by 1 px | `SnowGolemModel` 57-65 | M368 |
| allay | arms hang straight (vanilla splays them ~25° with an idle bob); body never pitches when flying; idle root bob | `AllayModel` 104-112 | M369 (holding as its steady state, no 5-tick ramp) |
| guardian, elder guardian | the whole body (vanilla's "head") never looks; the tail never sways | `GuardianModel` 102-127 | M370 (tail phase at the idle 0.125/tick; the eye's tracking and the spikes' withdrawal not modelled) |
| dolphin | the body never looks; the swim bob; the tail swims 6° too low (assignment, not addition) | `DolphinModel` 65-69 | M371 (adult and calf; moving = the tick's horizontal displacement) |
| nautilus (all three) | the soft body never follows the look (±10° clamp) | `NautilusModel` 126-131 | M372 |
| happy ghast | tentacles hang straight and still (vanilla 0.4 + 0.2·sin rest/sway); harnessed body not scaled 0.9375 | `HappyGhastModel` 95-101 | M373 for the tentacles; the harnessed body's 0.9375 squeeze is open (a per-part scale the animation path cannot yet express) |
| warden | no idle sway; walk at 0.6662/1.4 (vanilla 0.8662, capped 0.5; right arm on sin) | `WardenModel` 145-170 | M374 |
| ravager | legs 3.5× (vanilla 0.4·speed); mouth rests shut (vanilla π·0.01) | `RavagerModel` 107, 120-124 | M375 (the attack, stun and roar poses not modelled) |
| wither | ribcage and tail breathing frozen at their means; side heads copy the centre head | `WitherBossModel` 68-80 | M376 (side heads at their untargeted rest; target aiming not modelled) |
| ender dragon | static layer pose, head looks (vanilla never), no flap/neck/tail chains; drawn facing backward (renderer `rotY(-yr)`) | `EnderDragonModel` 203-287, `EnderDragonRenderer` 48-55 | open |
| breeze | head looks; vanilla's head is keyframes only | `BreezeModel` 139-147 | M377 |
| bat (flying) | head looks on top of the flying rig; vanilla only when resting | `BatModel` 72-81 | M377 (Rewo's bats always fly) |
| parrot | wings lie flush (vanilla ±0.0873 rest splay); legs and tail never walk | `ParrotModel` 88-100 | M378 (standing and sitting; the flying and party poses not modelled) |
| tadpole | tail never sways (`Anim::FishTail { 0.25, 0.3 }` is exactly it) | `TadpoleModel` 35-36 | M377 (the out-of-water 1.5 not told apart) |
| bee | antennae do not bob with `bobUpAndDown` | `AdultBeeModel` 27-28 | M377 |
| camel, armadillo | head look unclamped (camel ±30° / -25..45°, armadillo ±32.5° / -22.5..25°) | `CamelModel` 53-54, `ArmadilloModel` 59-60 | M377 (with the camel's dash head lift) |
| axolotl | layer pose; vanilla always blends swim/hover/crawl/lay poses on `ageInTicks`, the look goes to the body | `AdultAxolotlModel` 77-203 | open |
| creeper | legs half a cycle out of phase (vanilla swaps the field names); invisible in practice | `CreeperModel` 26-49 | M380 |
| villager, wandering trader, witch, pillager, vindicator, evoker, illusioner | drawn at 1.0; vanilla's layers are `MeshTransformer.scaling(0.9375)` (about 6% too big) | `LayerDefinitions` 192-193, 215, 494, 496 | M360 |
| piglin, piglin brute, zombified piglin | ears frozen at ±30° (vanilla flaps them on `age·0.1 + pos·0.5`); arms without the humanoid idle bob or the attack swing; aggressive poses (`holdWeaponHigh`, crossbow, admiring) | `AbstractPiglinModel` 78-81, `PiglinModel` 31-59 | M380 for the ears and the arms' idle bob; the held-item pose, the attack swing and the piglin-only poses (dancing, admiring, crossbow, weapon held high) open |
| piglin family | mesh: `PlayerModel.createMesh` (true left-limb UVs (32,48)/(16,48), jacket, sleeves and pants at 0.25); Rewo mirrors the right limbs and draws no outer layers (the brute's jacket and pants are missing) | `AdultPiglinModel.createBodyLayer` | M385 |
| drowned | left limbs at (32,48)/(16,48) not mirrored (base and outer layer); the trident throw's `xRot·0.5 − π` post-step missing | `DrownedModel` 22-42 | M382 (the swim pose not modelled) |
| stray, bogged | outer layers are `HumanoidModel.createMesh` (4×12×4 limbs, arms ±5, legs ±1.9; bogged at 0.2, hat 0.7); Rewo uses skeleton limbs at 0.25 | `LayerDefinitions` | M383 |
| parched | `SkeletonModel.createSingleModelDualBodyLayer` (64×64: arms ±5.5, outer limbs, belt, overlays); Rewo uses the plain skeleton | `LayerDefinitions` 399 | M384 (`ParchedRenderer` adds no clothing layer, so the outer layer is unused in vanilla too) |
| enderman | arms lack the humanoid idle bob (zRot ±(cos(age·0.09)·0.05+0.05), not halved) | `EndermanModel` 48-58 | M379 |
| witch | nose (and mole) never wobble (`sin(age·0.01·(id%10))`); the drinking nose lift | `WitchModel` 73-78 | M379 |
| every humanoid | no riding pose (arms −π/5, legs −1.4137 with ±π/10 yaw); jockeys and riders stand | `HumanoidModel` 233-242, `IllagerModel` 85-97 | M381 |

## Uncertain or flag-driven (unmodelled states)

- Feline crouch (tempted) and sprint (fleeing) poses; feline lie-down.
- Chicken wing flap in the air; fox sleeping/sitting/crouching; panda states;
  goat ram head and lost horns; polar bear standing; wolf interested head roll.
- Guardian eye tracking and spike withdrawal; squid tentacle amplitude
  (0.3 against vanilla's π/4); magma cube squish; fish body sway and
  pufferfish puff sizes (renderer-level).
- Copper golem idle replay timing (keyframe gating).

## Clean

Player (both), zombie, husk, giant, zombie villager, skeleton, wither
skeleton, villager's and illager's poses, armor stand, cow (all three meshes), mooshroom, pig (both), sheep (outside grazing), chicken
(on the ground), wolf, cat and ocelot (standing, walking, sitting), fox, goat,
panda, polar bear, llama, spider, cave spider, slime, sulfur cube, phantom,
shulker, silverfish, endermite, blaze, ghast, cod, salmon, pufferfish fins,
tropical fish, iron golem, vex, sniffer, creaking, frog, camel's walk rig,
bee bone/legs/wings/stinger.
