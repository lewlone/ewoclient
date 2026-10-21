//! Mob models — a faithful port of vanilla's `ModelPart.Cube` box builder
//! plus every mob mesh transcribed from the 26.2 decompile.
//!
//! The one rule of this module: **match the decompiled source exactly, don't
//! approximate**. The previous hand-rolled unwrap (`box_uv_faces`) had the
//! face→rect assignment, the per-face vertex→UV-corner order, and the mirror
//! handling all subtly wrong — silhouettes looked right while every texture
//! was scrambled (see docs/history/REWO_MOB_REDO_HANDOFF.md). This port reproduces
//! `ModelPart.Cube` + `Polygon` verbatim: the 8 corners, the per-face vertex
//! arrays, the UV column table, the UP-face vertical flip, and the mirror
//! vertex reversal.
//!
//! Coordinate contract (kept identical to vanilla so meshes transcribe 1:1):
//! - Models are authored in **vanilla model space**: +Y down, front = −Z,
//!   units = model pixels, ground at y=24.
//! - The renderer's transform is vanilla's exactly:
//!   `rotY(180° − yaw) · scale(−1,−1,1) · translate(0,−1.501,0) · (px/16)`,
//!   applied in [`crate::entities`]. At yaw 0 a model point (mx,my,mz) lands
//!   at world `(mx, 24.016−my, −mz)/16` relative to the entity's feet — the
//!   entity faces +Z (south), its right hand is at −X (west), like vanilla.
//! - Face labels ([`Facing`]) are vanilla's model-space names: `Down` is the
//!   minY plane, which is the **world-top** face after the y-flip.

// Angles are vanilla's float literals (e.g. 0.3927F), transcribed exactly.
#![allow(clippy::approx_constant)]

/// Vanilla's `translate(0, -1.501, 0)` in model px (1.501 · 16).
pub const MODEL_EYE_Y: f32 = 24.016;

/// Vanilla `Direction` labels as `ModelPart.Cube` uses them (model space).
/// After the render flip: `Down` shows on the world TOP, `Up` on the world
/// bottom, `North` on the entity's front, `East` on its left.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Facing {
    Down,
    Up,
    West,
    North,
    East,
    South,
}

impl Facing {
    /// Outward normal in model space (y-down).
    pub fn normal(self) -> [f32; 3] {
        match self {
            Facing::Down => [0.0, -1.0, 0.0], // minY plane
            Facing::Up => [0.0, 1.0, 0.0],    // maxY plane
            Facing::West => [-1.0, 0.0, 0.0],
            Facing::North => [0.0, 0.0, -1.0],
            Facing::East => [1.0, 0.0, 0.0],
            Facing::South => [0.0, 0.0, 1.0],
        }
    }

    /// Debug-texture color for this face label (pure channel patterns so a
    /// shaded sample still classifies by chroma). Used by the facelabel
    /// verification pass (`REWO_MOB_DEBUG_TEX` + `rewo mobshot --check`).
    pub fn debug_color(self) -> [u8; 3] {
        match self {
            Facing::North => [255, 0, 0],   // front → red
            Facing::South => [255, 255, 0], // back → yellow
            Facing::West => [0, 255, 0],    // green
            Facing::East => [0, 0, 255],    // blue
            Facing::Down => [255, 0, 255],  // world-top → magenta
            Facing::Up => [0, 255, 255],    // world-bottom → cyan
        }
    }
}

/// Which animation drives a part. Angle/offset formulas are vanilla's
/// `setupAnim` bodies, evaluated in [`crate::entities`] from the walk
/// phase, head state, and wall-clock age (ticks). Payload variants carry
/// the per-part constants vanilla bakes into its loops.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Anim {
    /// Static geometry (any static pose was folded in at build time, or
    /// lives in the part's base `rot`).
    None,
    /// M238 — `ArmorStandModel`'s `basePlate.yRot = -yRot`: the plate turns
    /// back by the stand's own yaw, so it stays aligned to the world grid.
    CounterBodyYaw,
    /// M270 — `ArmorStandArmorModel.setupAnim`: part `i` of the stand's six
    /// poses (head, body, left arm, right arm, left leg, right leg) set
    /// absolutely, as a delta from the default pose the part is baked at.
    StandPose(u8),
    /// Look pitch about X + net head yaw about Y (vanilla `rotateZYX(0,y,x)`).
    Head,
    /// M337 — the look's yaw alone: `BabyGoatModel.setupAnim` sets the
    /// head's `xRot` to π/8 after `QuadrupedModel` set it to the pitch, so the
    /// part carries π/8 as its base and only the yaw moves.
    HeadYaw,
    /// Humanoid arms: `cos(f ± π) · 2.0 · amount · 0.5`.
    ArmRight,
    ArmLeft,
    /// Biped legs: `cos(f ± π) · 1.4 · amount`.
    LegRight,
    LegLeft,
    /// Quadruped legs, diagonal gait: hind-right + front-left in phase.
    QuadHindRight,
    QuadHindLeft,
    QuadFrontRight,
    QuadFrontLeft,
    /// Spider walk: `yRot ±= −cos(2f + phase)·0.4·amt`,
    /// `zRot ±= |sin(f + phase)|·0.4·amt` (left legs negate both).
    SpiderLeg {
        left: bool,
        phase: f32,
    },
    /// Wolf tail wag while walking: `yRot = cos(f)·1.4·amt`.
    TailWagY,
    /// M359 — `AbstractEquineModel.setupAnim`'s `headParts` (the neck, head,
    /// mane, mouth and ears, and the bridle with them): `xRot` π/6 (the
    /// part's rest) + the look's pitch, + cos(walkPos · 0.8) · 0.15 · speed
    /// above a speed of 0.2; `yRot` the net head yaw clamped to ±20°.
    EquineHead,
    /// M359 — the equine legs: `legAnim1 = cos(walkPos · 0.6662 + π)`; the
    /// front legs at ±legAnim1 · 0.8 · speed (left +), the hind at ∓legAnim1
    /// · 0.5 · speed (left −).
    EquineLeg {
        hind: bool,
        left: bool,
    },
    /// M359 — the tail: `xRot` its π/6 rest + speed · 0.75, `y` + speed,
    /// `z` + 2 · speed (× the age scale, 1 for the adults that use it).
    EquineTail,
    /// M365 — `TurtleModel.setupAnim` on top of `QuadrupedModel`'s: each
    /// flipper keeps the quadruped's `xRot` swing; on land the flippers
    /// sweep (`yRot`: front ∓cos(pos·5·lay)·8·speed·layAmp, hind
    /// ∓cos(pos·5)·3·speed, lay 4 and layAmp 2 while laying an egg); in
    /// water the hind `xRot` becomes ±cos(pos·0.6662·0.6)·0.5·speed and the
    /// front `zRot` ∓ the same. With an egg the root is 1 px up.
    TurtleLeg {
        hind: bool,
        left: bool,
    },
    /// M365 — the turtle's head: the look, 1 px up with an egg.
    TurtleHead,
    /// M365 — the turtle's shell and plates: 1 px up with an egg.
    TurtleBody,
    /// M366 — `StriderModel.setupAnim`'s body (the walk speed clamped to
    /// 0.25): the look unless ridden, `zRot` 0.1·sin(pos·1.5)·4·speed, and
    /// `customAnimations`' bob, `y` − k·cos(pos·1.5)·2·speed (k 2 for the
    /// adult, 1 for the baby) about the `y` it assigns (2 / 17.25).
    StriderBody {
        adult: bool,
    },
    /// M366 — a strider leg: `xRot` sin(pos·0.75 (+π right))·2·speed, `zRot`
    /// π/18·cos(the same)·speed, `y` + 2·sin(pos·0.75 (+π left))·2·speed.
    StriderLeg {
        left: bool,
    },
    /// M366 — `animateBristle`: flow cos(pos·1.5 + π)·speed times 0.6 / 1.2
    /// / 1.3, plus a sway on `ageInTicks` (0.1·sin(age·0.4), 0.1·sin(age·0.2),
    /// 0.05·sin(−age·0.4)) for the first / second / third bristle, added to
    /// `zRot` on the adult (the same sign on both sides) and `xRot` on the
    /// baby.
    StriderBristle {
        order: u8,
        baby: bool,
    },
    /// M370 — a guardian spike: `setupSpikes` moves its pivot to
    /// `SPIKE_* · (1 + cos(age·1.5 + i)·0.01 − withdrawal)` (y + 16), the
    /// withdrawal taken as 0 (spikes out).
    GuardianSpike {
        i: u8,
    },
    /// M370 — a guardian tail segment: `yRot` sin(tailAnimation)·π·(0.05,
    /// 0.1, 0.15). The tail phase advances at the idle-in-water steady rate,
    /// 0.125 a tick (vanilla eases it toward 0.5 while moving, 2 out of
    /// water, from per-entity client state Rewo does not keep).
    GuardianTail {
        seg: u8,
    },
    /// M369 — `AllayModel.setupAnim`'s body: `xRot` = flyingFactor · π/4
    /// (flyingFactor = min(speed / 0.3, 1)).
    AllayBody,
    /// M369 — an allay arm: `xRot` holding · lerp(fly, −π/3, −1.134464),
    /// `zRot` ∓(0.43633232 − cos(age·9° + 3π/2)·π·0.075·(1 − fly)·(1 −
    /// holding)), `yRot` ∓0.27925268 · holding (left first).
    AllayArm {
        left: bool,
    },
    /// M368 — `SnowGolemModel.setupAnim`: the upper body turns a quarter of
    /// the head yaw.
    SnowGolemBody,
    /// M368 — a snow golem arm: `yRot` the upper body's (+π right), its
    /// pivot on a 5 px circle at that angle (left `x = cos·5, z = −sin·5`,
    /// right the opposite), so at rest both sit at z 0, not the layer's ±1.
    SnowGolemArm {
        left: bool,
    },
    /// M367 — `HoglinModel.setupAnim`'s head: `yRot` the look, `xRot`
    /// `lerp(headbutt, 0.87266463, −π/9)` (the look's pitch never reaches
    /// it); `BabyHoglinModel` also lowers it 2.5 · headbutt.
    HoglinHead {
        baby: bool,
    },
    /// M367 — a hoglin ear: `zRot` ∓2π/9 ∓ speed · sin(pos).
    HoglinEar {
        left: bool,
    },
    /// M367 — a hoglin leg: right front and left hind cos(pos)·1.2·speed,
    /// the other two half a cycle on (the raw walk position, not ×0.6662).
    HoglinLeg {
        front: bool,
        left: bool,
    },
    /// M358 — `SheepModel.setupAnim` (and `SheepFurModel`'s): while grazing
    /// the head's `y` grows by `headEatPositionScale` · 9 · the age scale
    /// and its `xRot` is `headEatAngleScale`; otherwise it looks, as `Head`.
    SheepHead,
    /// M357 — `AdultFelineModel` / `BabyFelineModel.setupAnim`: the head's
    /// look (as `Head`), and sitting its offset ((0, −3.3, 1) × the age
    /// scale for an adult, (0, 0, 0.75) for a kitten).
    FelineHead,
    /// M357 — the feline body: sitting, an adult's pitches to π/4 (from its
    /// π/2 rest) and moves (−4, +5) × the age scale; a kitten's pitches by
    /// −0.43633232 and drops 1.25.
    FelineBody,
    /// M357 — the first tail segment: sitting, an adult's goes to
    /// 1.7278761 and moves (+8, −2) × the age scale; a kitten's tilts by
    /// 0.5454154 and moves (+4, −0.9).
    FelineTail1,
    /// M357 — the second tail segment: it rests at 1.7278761 (`setupAnim`
    /// sets it every frame, not the layer) and sways π/4 · cos(walkPos) ·
    /// speed while walking; sitting, an adult's goes to 2.670354 and moves
    /// (+2, −0.8) × the age scale.
    FelineTail2,
    /// M357 — the feline's legs: the walk at amplitude 1 (left hind and
    /// right front on cos(f), the others on cos(f + π)); sitting, an
    /// adult's front legs go to −π/20 and move (+2, −2), its hind legs to
    /// −π/2 and (+3, −4), × the age scale; a kitten's hind legs move −0.9
    /// in z.
    FelineLeg {
        hind: bool,
        left: bool,
    },
    /// M356 — `WolfModel.setSittingPose`, on the wolf's and the puppy's
    /// body (`y` + 4, `z` − 2, `xRot` π/4, the puppy's then − π/2: the same
    /// −π/4 from either rest), the adult's mane (`y` + 2, `xRot` 2π/5) and
    /// legs (hind: `y` + 6.7, `z` − 5, `xRot` 3π/2; front: `xRot` 5.811947,
    /// `x` ± 0.01, `y` + 1), every offset times the age scale except the
    /// mane's. Standing, the legs walk as the quadruped's.
    WolfBody,
    WolfMane,
    WolfHind {
        left: bool,
    },
    WolfFront {
        left: bool,
    },
    /// M354 — `BeeModel.bobUpAndDown` (flying and not angry), with `s =
    /// cos(age · 0.18)`: the bone's `xRot` is 0.1 + s·π·0.025 and its `y`
    /// drops by s·0.9; the front legs' `xRot` is π/8 − s·π·0.1 and the back
    /// legs' π/4 − s·π·0.05 (both rest at π/4, the flying pose). An angry
    /// bee holds the flying pose without the bob.
    BeeBone,
    BeeFrontLegs,
    BeeBackLegs,
    /// M352 — `EndermanModel.setupAnim`: the humanoid swing halved and
    /// clamped to ±0.4; while carrying a block both arms hold it up (`xRot`
    /// -0.5, `zRot` ±0.05).
    EndermanArmRight,
    EndermanArmLeft,
    /// M352 — the legs' swing halved and clamped to ±0.4.
    EndermanLegRight,
    EndermanLegLeft,
    /// Iron golem limbs: `triangleWave(pos, 13)` on the raw walk position.
    GolemArmRight,
    GolemArmLeft,
    GolemLegRight,
    GolemLegLeft,
    /// Blaze rod: vanilla re-*positions* the rod each frame —
    /// `(cos(a)·r, y0 + cos((i·yf + age)·ys), sin(a)·r)` with
    /// `a = a0 + age·π·speed + i·π/2`.
    BlazeRod {
        ring: u8,
        idx: u8,
    },
    /// Ghast tentacle sway: `xRot = 0.2·sin(age·0.3 + i) + 0.4`.
    GhastTentacle {
        i: u8,
    },
    /// M393 — a boat's outer root: the renderer's hurt wobble,
    /// `rotX(sin(hurt)·hurt·damage/10·hurtDir)` degrees between the yaw and
    /// the flip (in model space, past the flip, the negated angle).
    BoatHurt,
    /// M394 — a boat's bubble-column tilt: `AbstractBoatRenderer`'s
    /// `Quaternionf().setAngleAxis(angle, 1, 0, 1)`, whose axis JOML does
    /// not normalise ([`bubble_tilt`]).
    BoatBubble,
    /// M393 — a boat paddle, `AbstractBoatModel.animatePaddle` on the side's
    /// rowing time (left 0, right 1), about [`paddle_rest`].
    BoatPaddle {
        right: bool,
    },
    /// M391 — the happy ghast's body (no motion of its own): the part
    /// `HappyGhastModel.setupAnim` scales by 0.9375 while the ghast wears
    /// a harness (`body_squeeze`), its tentacles with it.
    HappyGhastBody,
    /// Squid tentacle curl (approximation of the entity-driven
    /// `tentacleAngle` swim pulse — the wire doesn't carry it).
    SquidTentacle,
    /// Phantom wings: `zRot = ±cos(age·7.448451°)·16°` (tips share it).
    PhantomWing {
        left: bool,
    },
    /// Phantom tail segments: `xRot = −(5 + cos(2·flap)·5)°`.
    PhantomTail,
    /// Allay wings: `yRot = ∓π/4 ± (cos(age·20° + pos)·π·0.15 + amt)`,
    /// `xRot = 0.43633·(1 − min(amt/0.3, 1))`.
    AllayWing {
        left: bool,
    },
    /// Allay dance ROOT (`AllayModel` `state.isDancing` branch, `this.root`):
    /// while dancing, `yRot = isSpinning ? 4π·spin : 0`, `zRot =
    /// cos(danceSpeed)·16°·(1−spin)`, where `danceSpeed = ageInTicks·8° +
    /// animationSpeed` and `spin = spinningProgress`. Idle → 0 (the whole model
    /// stays upright). Only the whole-body root carries it, so a spin rotates
    /// head+body+arms+wings together.
    AllayRoot,
    /// Allay dance HEAD (`AllayModel`, `this.head`): while dancing,
    /// `yRot = cos(danceSpeed)·30°·(1−spin)`, `zRot = cos(danceSpeed)·14°·(1−spin)`
    /// (xRot stays 0). While NOT dancing it is the ordinary look:
    /// `xRot = pitch`, `yRot = netHeadYaw` — i.e. `Anim::Head`.
    AllayHead,
    /// Vex arms: `zRot = ±(π/5 + cos(age·5.5°)·0.1)`.
    VexArm {
        left: bool,
    },
    /// Vex wings: `yRot = ±(1.0995574 + cos(age·45.836624°)·16.2°)`,
    /// with the static x/z tilts from vanilla.
    VexWing {
        left: bool,
    },
    /// Bee wings (airborne — our bees always hover):
    /// `zRot = ±cos(age·120.32113°)·π·0.15`.
    BeeWing {
        left: bool,
    },
    /// Fish tail sway: `yRot = −amp·sin(speed·age)`.
    FishTail {
        amp: f32,
        speed: f32,
    },
    /// Pufferfish blue fins: `zRot = ∓0.2 ± 0.4·sin(age·0.2)`.
    PufferFin {
        left: bool,
    },
    /// Dolphin tail segments while swimming: `xRot = −k·cos(age·0.3)`, an
    /// assignment over the segment's `rest` (M371; still, the rest holds).
    DolphinTail {
        k: f32,
        rest: f32,
    },
    /// M380 — a piglin arm: `HumanoidModel`'s walk swing plus
    /// `bobModelPart`'s idle bob; M392 the hold pose and the attack swing
    /// too (vanilla assigns the swing's `±5·ageScale` arm pivot to the
    /// baby's `±4` arms as well); M390 `PiglinModel`'s poses on top.
    PiglinArm {
        left: bool,
    },
    /// M380 — `AbstractPiglinModel.setupAnim`'s ear flap about its rest:
    /// `zRot` −cos(f·1.2)·a (left) / +cos(f)·a (right), f = age·0.1 +
    /// pos·0.5, a = 0.08 + 0.4·speed.
    PiglinEar {
        left: bool,
    },
    /// M390 — a piglin's head: the look, then `PiglinModel.setupAnim`'s
    /// dance bob (DANCING) or its downward stare (ADMIRING_ITEM).
    PiglinHead,
    /// M390 — a piglin's body: the dance bob.
    PiglinBody,
    /// M379 — `WitchModel.setupAnim`'s nose: `xRot` sin(age·k)·4.5°, `zRot`
    /// cos(age·k)·2.5° with k = 0.01·(entityId % 10); holding an item, the
    /// nose sits at (0, 1, −1.5) tipped −0.9.
    WitchNose,
    /// M388 — an ender dragon part, slot `part` of [`dragon_pose`].
    Dragon {
        part: u8,
    },
    /// M387 — an axolotl part, slot `part` of [`axolotl_pose`]: 0 body,
    /// 1 head, 2 top gills, 3 left gills, 4 right gills, 5 right hind leg,
    /// 6 left hind, 7 right front, 8 left front, 9 tail.
    Axolotl {
        part: u8,
    },
    /// M378 — `ParrotModel.setupAnim`, standing or sitting: the parrot's
    /// body and wings drop 1.9 px when it sits.
    ParrotSit,
    /// M378 — the parrot's head: the look (and 1.9 px down sitting).
    ParrotHead,
    /// M378 — the parrot's tail: `xRot` + cos(f)·0.3·speed standing; π/6
    /// and 1.9 px down sitting.
    ParrotTail,
    /// M378 — a parrot leg: `xRot` + cos(f (+π right))·1.4·speed standing;
    /// π/2 and 1.9 px down sitting.
    ParrotLeg {
        left: bool,
    },
    /// M377 — `CamelModel.applyHeadRotation`: the look clamped (yaw ±30°,
    /// pitch −25..45°); after a dash the head lifts `45° · cooldown / 55`,
    /// clamped to 70°.
    CamelHead,
    /// M377 — `ArmadilloModel.setupAnim`: the look clamped (yaw ±32.5°,
    /// pitch −22.5..25°).
    ArmadilloHead,
    /// M377 — `AdultBeeModel.bobUpAndDown`: an antenna's `xRot` =
    /// cos(age·0.18)·π·0.03 while the bee hovers calmly.
    BeeAntenna,
    /// M376 — `WitherBossModel.setupAnim`'s ribcage: `xRot` + 0.05·cos(age·
    /// 0.1)·π about its 0.065π rest.
    WitherRibcage,
    /// M376 — the wither's tail: `xRot` + 0.1·cos(age·0.1)·π about 0.265π,
    /// its pivot on the ribcage's 10 px arc.
    WitherTail,
    /// M374 — `WardenModel.animateWalk` + `animateIdlePose`'s body, with
    /// `ap = pos·0.8662`, `sm = min(0.5, 3·speed)`, `smm = min(0.35, sm)`:
    /// `zRot` 0.1·sin(ap)·sm + 0.025·sin(age·0.1), `xRot` cos(ap)·smm +
    /// 0.025·cos(age·0.1).
    WardenBody,
    /// M374 — the warden's head: the look, plus `zRot` 0.3·sin(ap)·sm +
    /// 0.06·cos(age·0.1) and `xRot` + 1.2·cos(ap + π/2)·smm + 0.06·sin(age·0.1).
    WardenHead,
    /// M374 — a warden arm: `xRot` −0.8·cos(ap)·sm (left), −0.8·sin(ap)·sm
    /// (right).
    WardenArm {
        left: bool,
    },
    /// M374 — a warden leg: `xRot` cos(ap)·sm (left), cos(ap + π)·sm (right).
    WardenLeg {
        left: bool,
    },
    /// M372 — `NautilusModel.applyBodyRotation`: the soft body follows the
    /// look, each angle clamped to ±10°.
    NautilusBody,
    /// M371 — `DolphinModel.setupAnim`'s body: the look (`xRot`, `yRot`),
    /// and while swimming `xRot + (−0.05 − 0.05·cos(age·0.3))`.
    DolphinBody,
    /// Silverfish/endermite segment wiggle: `yRot = cos(age·0.9 +
    /// i·0.15π)·π·ry·(1+|i−2|)`, `x += sin(same)·π·tx·|i−2|`.
    /// `with_x` mirrors vanilla's wing layers that copy yRot only.
    Crawl {
        i: u8,
        ry: f32,
        tx: f32,
        with_x: bool,
    },
    /// Warden tendrils (`WardenModel.animateTendrils`):
    /// `xRot = ±tendrilAnimation · cos(age·2.25) · π · 0.1`, the left tendril
    /// taking the positive sign. `tendrilAnimation` is the entity's 10-tick
    /// countdown ([`crate::entities::EmissiveState::tendril`]) — 0 at rest, so
    /// the tendrils hang still until the warden hears something.
    WardenTendril {
        left: bool,
    },
    /// `HumanoidModel` **body** under a combat swing:
    /// `body.yRot = sin(sqrt(attackTime)·2π)·0.2`, negated when the attack arm
    /// is the left one. Zero when `attackTime <= 0` — vanilla's
    /// `setupAttackAnimation` short-circuits before touching anything.
    HumanoidBody,
    /// `HumanoidModel` arms: the ordinary walk swing (identical to
    /// [`Anim::ArmRight`] / [`Anim::ArmLeft`]) **plus**
    /// `setupAttackAnimation`'s pivot displacement, shared yRot term and, on
    /// the attack arm, the WHACK or STAB strike.
    ///
    /// Deliberately a separate variant rather than an extra branch inside
    /// `ArmRight`/`ArmLeft`: the attack math bakes `HumanoidModel.createMesh`'s
    /// ±5 arm pivot, and the other humanoid-ish mobs that use `ArmRight` (the
    /// enderman's ±5/−12 arms, the villager's arm block) neither share that
    /// pose nor render this animation in vanilla. Only models built to
    /// `HumanoidModel.createMesh` may use it.
    HumanoidArmRight,
    HumanoidArmLeft,
    /// `AbstractZombieModel` / `ZombieVillagerModel` / `ZombifiedPiglinModel`:
    /// the humanoid pipeline, then `AnimationUtils.animateZombieArms`
    /// **overwriting** the arms and bobbing them a second time (M20).
    UndeadArmRight,
    UndeadArmLeft,
    /// `SkeletonModel.setupAnim`: the humanoid pipeline, then its own attack
    /// override when aggressive and not holding a bow (M20).
    SkeletonArmRight,
    SkeletonArmLeft,
    /// `IllagerModel.setupAnim`: its own walk assignment (which wipes
    /// everything `super.setupAnim` put on the arms), then the
    /// `IllagerArmPose` switch (M20).
    IllagerArmRight,
    /// M382 — the drowned's arms: the zombie's, then `DrownedModel`'s
    /// trident post-step (`xRot·0.5 − π`, `yRot = 0`) on an arm posed
    /// `THROW_TRIDENT`.
    DrownedArmRight,
    DrownedArmLeft,
    IllagerArmLeft,
}

/// `HumanoidModel.createMesh`'s arm pivot x — `PartPose.offset(∓5, 2, 0)`.
/// `setupAttackAnimation` *assigns* `arm.x`/`arm.z` absolutely, so the additive
/// pose pipeline here needs the base it is replacing.
pub const HUMANOID_ARM_PIVOT_X: f32 = 5.0;

/// `net.minecraft.world.item.SwingAnimationType`, mirrored here so `rewo-gpu`
/// stays free of a `rewo-data` dependency (the same pattern as
/// [`AllayDance`] mirroring the world-side counters).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum SwingKind {
    /// Body + arm pivots move, but no arm strike.
    None,
    #[default]
    Whack,
    /// `SpearAnimations.thirdPersonAttackHand`.
    Stab,
}

/// Per-frame combat-swing inputs — the `ArmedEntityRenderState` fields
/// `HumanoidModel.setupAttackAnimation` reads.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SwingPose {
    /// `state.attackTime` = `LivingEntity.getAttackAnim(partialTicks)`.
    /// `<= 0` short-circuits the entire attack animation.
    pub attack_time: f32,
    /// `state.attackArm == HumanoidArm.LEFT`.
    pub left_arm: bool,
    /// `state.swingAnimationType`, from the item held by the attack arm.
    pub kind: SwingKind,
    /// `state.ageScale` — `isBaby() ? 0.5 : 1.0`. Scales the arm pivot swing.
    pub age_scale: f32,
    /// Whether every input behind this pose was resolved **exactly**.
    ///
    /// `false` means an equipment update could not be resolved (an item id
    /// outside the registry, or a component patch this client cannot walk), so
    /// `attack_time` has been forced to 0 and both the built-in pose and CEM's
    /// `swing_progress` are suppressed rather than guessed. Carried rather than
    /// implied so the suppression is observable instead of looking like an
    /// entity that simply is not swinging.
    pub inputs_known: bool,
}

impl SwingPose {
    /// Not swinging: `attackTime = 0` makes every attack term vanish, exactly
    /// like vanilla's `if (!(attackTime <= 0))` guard.
    pub const NONE: SwingPose = SwingPose {
        attack_time: 0.0,
        left_arm: false,
        kind: SwingKind::Whack,
        age_scale: 1.0,
        inputs_known: true,
    };
}

impl Default for SwingPose {
    fn default() -> Self {
        SwingPose::NONE
    }
}

/// `HumanoidModel.ArmPose` — the *hold* pose an arm takes from the item in it,
/// applied by `poseRightArm` / `poseLeftArm` **before**
/// `setupAttackAnimation`. It is not part of the swing; it is the baseline the
/// swing is added to, which is why an armed entity that never swings is still
/// posed by it.
///
/// Vanilla declares eleven, each carrying `(twoHanded, affectsOffhandPose)`:
///
/// ```text
/// EMPTY(false,false)   ITEM(false,false)         BLOCK(false,false)
/// BOW_AND_ARROW(true,true)                       THROW_TRIDENT(false,true)
/// CROSSBOW_CHARGE(true,true)  CROSSBOW_HOLD(true,true)
/// SPYGLASS(false,false)  TOOT_HORN(false,false)  BRUSH(false,false)
/// SPEAR(false,true)
/// ```
///
/// **Rewo models all eleven as of M23.** Through M19-M22 only the three
/// reachable without item-use state were modelled, on the stated grounds that
/// "neither the use-item hand nor the remaining ticks is synchronised for a
/// remote entity". The first half of that is wrong — the hand *is* synchronised,
/// in `DATA_LIVING_ENTITY_FLAGS` bit 2 — and the second half is beside the
/// point, because `LivingEntity.onSyncedDataUpdated` **derives** the remaining
/// ticks client-side rather than receiving them. See
/// [`rewo_data::use_item`](../../rewo_data/use_item/index.html).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ArmPose {
    /// Empty hand. `arm.yRot = 0`.
    #[default]
    Empty,
    /// Any ordinary held item — **the default fall-through for an armed
    /// entity**, and so the overwhelmingly common combat case.
    /// `arm.xRot = arm.xRot * 0.5 − π/10; arm.yRot = 0`.
    Item,
    /// Shield raised (`ItemUseAnimation.BLOCK`). `poseBlockingArm`.
    Block,
    /// Bow drawn. **Writes both arms** — the only pose that does so without
    /// being a crossbow.
    BowAndArrow,
    /// Trident wound up. `arm.xRot = arm.xRot * 0.5 − π`.
    ThrowTrident,
    /// Crossbow being cranked. **Writes both arms**, and is the only pose that
    /// reads the use clock's *elapsed* count rather than merely being gated on
    /// it.
    CrossbowCharge,
    /// Charged crossbow held ready. **Writes both arms.** The only use-driven
    /// pose that is *not* gated on the use clock: its condition is
    /// `!swinging && item is CROSSBOW && CrossbowItem.isCharged(item)`.
    CrossbowHold,
    /// Spyglass raised to the eye.
    Spyglass,
    /// Goat horn at the lips.
    TootHorn,
    /// Brush angled down.
    Brush,
    /// A spear: either `swing_animation` is STAB and the entity is swinging, or
    /// the item is in `minecraft:spears`, or the item is in use with
    /// `ItemUseAnimation.SPEAR`. Poses through
    /// `SpearAnimations.thirdPersonHandUse`.
    Spear,
}

impl ArmPose {
    /// Vanilla's `twoHanded` flag, transcribed from the enum's constructor
    /// arguments (`HumanoidModel:488`).
    pub const fn is_two_handed(self) -> bool {
        match self {
            ArmPose::BowAndArrow | ArmPose::CrossbowCharge | ArmPose::CrossbowHold => true,
            ArmPose::Empty
            | ArmPose::Item
            | ArmPose::Block
            | ArmPose::ThrowTrident
            | ArmPose::Spyglass
            | ArmPose::TootHorn
            | ArmPose::Brush
            | ArmPose::Spear => false,
        }
    }

    /// Vanilla's `affectsOffhandPose` flag. When set, posing this arm
    /// **suppresses the other arm's pose entirely** — which is exactly why the
    /// three both-arm poses can write the other arm without being overwritten.
    pub const fn affects_offhand_pose(self) -> bool {
        match self {
            ArmPose::BowAndArrow
            | ArmPose::ThrowTrident
            | ArmPose::CrossbowCharge
            | ArmPose::CrossbowHold
            | ArmPose::Spear => true,
            ArmPose::Empty
            | ArmPose::Item
            | ArmPose::Block
            | ArmPose::Spyglass
            | ArmPose::TootHorn
            | ArmPose::Brush => false,
        }
    }

    /// Whether this pose's body assigns rotations to the arm it was *not*
    /// called for.
    ///
    /// Not a vanilla flag — vanilla just writes both fields inside the case —
    /// but the per-arm pipeline here needs to know, because computing the left
    /// arm may require running the right arm's pose body.
    ///
    /// It coincides with [`Self::is_two_handed`] for all eleven, and is kept
    /// separate anyway: they mean different things, and a future pose that
    /// writes both arms without being two-handed would silently inherit the
    /// wrong answer from a shared flag.
    pub const fn writes_both_arms(self) -> bool {
        matches!(
            self,
            ArmPose::BowAndArrow | ArmPose::CrossbowCharge | ArmPose::CrossbowHold
        )
    }
}

/// Both arms' [`ArmPose`] plus the handedness that decides which arm vanilla
/// poses first — the `HumanoidRenderState` fields `setupAnim`'s pose dispatch
/// reads.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct ArmPoses {
    /// `state.rightArmPose`.
    pub right: ArmPose,
    /// `state.leftArmPose`.
    pub left: ArmPose,
    /// `state.mainArm == HumanoidArm.RIGHT`.
    pub right_handed: bool,
    /// Whether both hands resolved exactly. `false` suppresses posing entirely
    /// — the same honesty rule [`SwingPose::inputs_known`] applies to the
    /// swing, for the same reason: a guessed baseline is a wrong pose rendered
    /// confidently.
    pub known: bool,
    /// `state.isUsingItem` (M23) — `LivingEntity.isUsingItem()`.
    ///
    /// Populated by `HumanoidMobRenderer.extractHumanoidRenderState`, which is
    /// a **static helper**, not an override: `AvatarRenderer:168` calls it too
    /// (as do the armour-stand, enderman and giant renderers). So a player
    /// carries the same use render state a zombie does, and both reach the
    /// `isUsingItem` dispatch branch.
    pub using_item: bool,
    /// `state.useItemHand == InteractionHand.MAIN_HAND` (M23). Only read when
    /// [`Self::using_item`] is set. Defaults to `true`, matching
    /// `HumanoidRenderState`'s `useItemHand = InteractionHand.MAIN_HAND`.
    pub main_hand_used: bool,
    /// `state.ticksUsingItem` — `getTicksUsingItem(partialTicks)`.
    pub ticks_using_item: f32,
    /// `state.maxCrossbowChargeDuration` — `CrossbowItem.getChargeDuration`,
    /// i.e. `floor(modifyCrossbowChargingTime(stack, user, 1.25F) * 20)`.
    ///
    /// Rewo decodes no enchantments, so the modifier is the identity and this
    /// is the literal `floor(1.25 * 20) = 25` whenever the pose is
    /// [`ArmPose::CrossbowCharge`]. That is not an approximation: an enchanted
    /// crossbow patches `minecraft:enchantments`, which the stack decoder
    /// cannot walk, so such a stack resolves to
    /// [`rewo_world::entities::HandItem::Unknown`] and suppresses the whole
    /// baseline before it ever reaches this field.
    ///
    /// `0` means "not set" — the `HumanoidRenderState` default, which is what
    /// every pose other than `CROSSBOW_CHARGE` leaves it at.
    pub max_crossbow_charge: f32,
}

/// Which arm `setupAnim`'s dispatch poses first, and whether it poses the
/// second at all.
///
/// Vanilla writes this as two nested branches that each call
/// `pose{Right,Left}Arm` in a definite order. The order is not decorative: a
/// pose that writes *both* arms lands on top of whatever the other arm's pose
/// already assigned, so reproducing the sequence is what makes a bow-drawing
/// off-hand override a main-hand item pose rather than the reverse.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PoseOrder {
    /// The arm posed unconditionally (`true` = left).
    pub first_left: bool,
    /// Whether the *other* arm is posed after it — vanilla's
    /// `if (!firstPose.affectsOffhandPose())`.
    pub second_runs: bool,
}

impl ArmPoses {
    /// Both hands empty, right-handed — vanilla's state for an unarmed entity,
    /// and the neutral every non-humanoid model carries.
    pub const EMPTY: ArmPoses = ArmPoses {
        right: ArmPose::Empty,
        left: ArmPose::Empty,
        right_handed: true,
        known: true,
        using_item: false,
        main_hand_used: true,
        ticks_using_item: 0.0,
        max_crossbow_charge: 0.0,
    };

    /// `CrossbowItem.getChargeDuration(stack, user)` with no enchantment
    /// modifier: `Mth.floor(1.25F * 20.0F)`.
    ///
    /// A literal rather than a computation because the input it would take —
    /// `EnchantmentHelper.modifyCrossbowChargingTime` — needs the stack's
    /// enchantments, and a stack carrying any is already suppressed upstream.
    pub const CROSSBOW_CHARGE_DURATION: f32 = 25.0;

    pub const fn for_arm(self, left: bool) -> ArmPose {
        if left {
            self.left
        } else {
            self.right
        }
    }

    /// Whether `setupAnim` calls `pose{Left,Right}Arm` for this arm at all.
    ///
    /// Transcribes the `isUsingItem == false` branch of the dispatch:
    ///
    /// ```text
    /// boolean twoHandedOffhand = rightHanded ? leftArmPose.isTwoHanded()
    ///                                        : rightArmPose.isTwoHanded();
    /// if (rightHanded != twoHandedOffhand) {
    ///    poseLeftArm(state);
    ///    if (!leftArmPose.affectsOffhandPose())  poseRightArm(state);
    /// } else {
    ///    poseRightArm(state);
    ///    if (!rightArmPose.affectsOffhandPose()) poseLeftArm(state);
    /// }
    /// ```
    ///
    /// …and, since M23, the `isUsingItem == true` branch that precedes it:
    ///
    /// ```text
    /// if (state.isUsingItem) {
    ///    boolean mainHandUsed = state.useItemHand == InteractionHand.MAIN_HAND;
    ///    if (mainHandUsed == rightHanded) {
    ///       poseRightArm(state);
    ///       if (!rightArmPose.affectsOffhandPose()) poseLeftArm(state);
    ///    } else {
    ///       poseLeftArm(state);
    ///       if (!leftArmPose.affectsOffhandPose()) poseRightArm(state);
    ///    }
    /// }
    /// ```
    ///
    /// Through M22 this branch was documented as unreachable, because Rewo
    /// never set `isUsingItem`. M23 sets it, so it is transcribed.
    pub const fn order(self) -> PoseOrder {
        let first_left = if self.using_item {
            // `mainHandUsed == rightHanded` → right arm first. The equality is
            // the point: a left-handed entity using its main hand is using its
            // *left* arm.
            self.main_hand_used != self.right_handed
        } else {
            let two_handed_offhand = if self.right_handed {
                self.left.is_two_handed()
            } else {
                self.right.is_two_handed()
            };
            self.right_handed != two_handed_offhand
        };
        let first = if first_left { self.left } else { self.right };
        PoseOrder {
            first_left,
            second_runs: !first.affects_offhand_pose(),
        }
    }

    /// Whether `setupAnim` calls `pose{Left,Right}Arm` for this arm at all.
    pub const fn poses_arm(self, left: bool) -> bool {
        if !self.known {
            return false;
        }
        let o = self.order();
        if o.first_left == left {
            true
        } else {
            o.second_runs
        }
    }
}

impl Default for ArmPoses {
    fn default() -> Self {
        ArmPoses::EMPTY
    }
}

/// `AbstractIllager.IllagerArmPose`, in declaration order (the ordinal is not
/// on the wire — the client *derives* this per illager class, so the order is
/// documentation rather than a codec).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum IllagerArmPose {
    /// Arms folded: the `arms` part shows and the two arm parts hide.
    #[default]
    Crossed,
    Attacking,
    Spellcasting,
    BowAndArrow,
    CrossbowHold,
    CrossbowCharge,
    Celebrating,
    Neutral,
}

/// M390 — `PiglinArmPose` as `Piglin.getArmPose()` answers it (the brute's
/// answers only ATTACKING_WITH_MELEE_WEAPON or DEFAULT), the input to
/// `PiglinModel.setupAnim`'s pose switch.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum PiglinArmPose {
    #[default]
    Default,
    Dancing,
    AdmiringItem,
    AttackingWithMeleeWeapon,
    CrossbowCharge,
    CrossbowHold,
}

/// The synced mob state the M20 arm rigs read, mirrored into `rewo-gpu` so the
/// renderer keeps no `rewo-world` dependency (the same pattern [`AllayDance`]
/// and [`SwingKind`] use).
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct MobCombat {
    /// `Mob.isAggressive()` — deepens the undead arm drop from −π/2.25 to
    /// −π/1.5 and gates the skeleton rig entirely.
    pub aggressive: bool,
    /// `state.getMainHandItemStack() == ItemStack.EMPTY`. Only a **baby**
    /// holding something drops the undead arms; an adult always raises them.
    pub main_hand_empty: bool,
    /// `entity.getMainHandItem().is(Items.BOW)` — a bow suppresses the
    /// skeleton attack rig so the bow-hold pose survives.
    pub holding_bow: bool,
    /// `state.isBaby`.
    pub is_baby: bool,
    /// `state.mainArm == HumanoidArm.LEFT`, for `swingWeaponDown`.
    pub main_arm_left: bool,
    /// The derived `IllagerArmPose`; ignored by the non-illager rigs.
    pub illager_pose: IllagerArmPose,
    /// M352 — the block state an enderman carries (`CarriedBlockLayer`;
    /// the app draws the block, the enderman's arm rig reads its presence).
    pub carried_block: Option<u32>,
    /// M353 — `SnowGolem.hasPumpkin()` (`SnowGolemHeadLayer`; the app draws
    /// the carved pumpkin).
    pub pumpkin: bool,
    /// M354 — `Bee.isAngry()` (the anger deadline against the world clock):
    /// no `bobUpAndDown`.
    pub bee_angry: bool,
    /// M354 — `Bee.hasStung()`: the stinger hidden.
    pub bee_stung: bool,
    /// M355 — `Wolf.getTailAngle()` (angry, tame by health, or wild), the
    /// tail's `xRot`; `None` leaves the part at its π/5 rest. An angry wolf
    /// does not wag (`tail.yRot = 0`).
    pub wolf_tail: Option<f32>,
    /// M355 — the wolf is angry (no wag).
    pub wolf_angry: bool,
    /// M356 — `TamableAnimal.isInSittingPose()` (the wolf's sitting pose).
    pub sitting: bool,
    /// M356 — `LivingEntity.getAgeScale()`: 0.5 for a baby, else 1; the
    /// sitting pose's offsets scale by it.
    pub age_scale: f32,
    /// M358 — a grazing sheep's `(headEatPositionScale,
    /// headEatAngleScale)`; `None` when it is not eating (the head looks).
    pub sheep_eat: Option<(f32, f32)>,
    /// M369 — `AllayRenderState.holdingAnimationProgress`, as its steady
    /// state: 1 while the main hand holds an item (vanilla ramps it over 5
    /// ticks).
    pub allay_holding: f32,
    /// M387 — `AxolotlRenderState`'s factors, taken whole (vanilla eases
    /// them on the client): in water, moving, playing dead.
    pub axolotl_in_water: bool,
    pub axolotl_moving: bool,
    pub axolotl_playing_dead: bool,
    /// M390 — a piglin's or brute's `getArmPose()`.
    pub piglin_pose: PiglinArmPose,
    /// M393 — a boat's `BoatRenderState.rowingTimeLeft` / `Right`.
    pub boat_rowing: [f32; 2],
    /// M393 — a boat's hurt wobble angle in degrees,
    /// `sin(hurtTime)·hurtTime·damageTime/10·hurtDir` (0 unhurt).
    pub boat_wobble: f32,
    /// M394 — a boat's `BoatRenderState.bubbleAngle` in degrees, already 0
    /// when the renderer skips it (under water, or `Mth.equal(angle, 0)`).
    pub boat_bubble: f32,
    /// M381 — `HumanoidRenderState.isPassenger` / `IllagerRenderState
    /// .isRiding`: the seated riding pose (set only for a kind that
    /// [`sits_when_riding`]).
    pub riding: bool,
    /// M379 — the witch's nose wobble rate `0.01·(entityId % 10)` and
    /// `WitchRenderState.isHoldingItem`.
    pub witch_nose_rate: f32,
    pub witch_holding: bool,
    /// M377 — `CamelRenderState.jumpCooldown`: the dash cooldown less the
    /// partial tick (0 when none).
    pub camel_jump_cooldown: f32,
    /// M371 — `DolphinRenderState.isMoving`: the swim bob and the tail's
    /// stroke run only then.
    pub dolphin_moving: bool,
    /// M367 — `HoglinModel`'s headbutt lerp factor, `1 − |10 − 2r| / 10` of
    /// the remaining attack ticks `r` (0 at rest and at both ends).
    pub headbutt: f32,
    /// M366 — `StriderRenderState.isRidden`: the body stops looking.
    pub ridden: bool,
    /// M365 — `TurtleRenderState`: `hasEgg`, `isLayingEgg` and `isOnLand`.
    pub turtle_egg: bool,
    pub turtle_laying: bool,
    pub turtle_on_land: bool,
    /// M363 — `Boat.isUnderWater()` (the eye in water): no water patch.
    pub under_water: bool,
}

impl Eq for Anim {}

/// Per-frame Allay dance inputs — the `AllayRenderState` fields the model
/// consumes, resolved from the client dance counters
/// (`EntityTable::allay_dance_render`). Present only for a *dancing* Allay;
/// `None` selects the ordinary head-look / upright-root pose.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AllayDance {
    /// `state.isSpinning` — the step function `dancingAnimationTicks % 55 < 15`.
    pub is_spinning: bool,
    /// `state.spinningProgress` — the interpolated `spinningAnimationTicks / 15`.
    pub spinning_progress: f32,
}

// ---------------------------------------------------------------------------
// Keyframe animations (vanilla `AnimationDefinition` rigs)
// ---------------------------------------------------------------------------

/// Channel target — rotation deltas (radians) or pivot offsets (model px,
/// vanilla's `posVec` y-negation already baked by the generator). Scale
/// channels are dropped at generation time.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum KfTarget {
    Rot,
    Pos,
}

pub struct KfFrame {
    pub t: f32,
    pub v: [f32; 3],
    /// This frame's interpolation — vanilla uses the *next* keyframe's mode
    /// for each segment.
    pub catmullrom: bool,
}

pub struct KfChannel {
    pub part: &'static str,
    pub target: KfTarget,
    pub frames: &'static [KfFrame],
}

pub struct KfDef {
    pub length: f32,
    pub looping: bool,
    pub channels: &'static [KfChannel],
}

/// How a keyframe animation advances — vanilla's driver call sites.
#[derive(Clone, Copy)]
pub enum KfDriver {
    /// `applyWalk(pos, amt, speed_f, scale_f)`: t = pos·0.05·speed_f
    /// seconds, value scale = min(amt·scale_f, 1).
    Walk { speed_f: f32, scale_f: f32 },
    /// The nautilus blend: `applyWalk(pos + age/age_div, amt + amt_add, …)`.
    WalkPlusAge {
        age_div: f32,
        amt_add: f32,
        speed_f: f32,
        scale_f: f32,
    },
    /// Continuous, age-driven at full scale (bat flight, breeze idle).
    Age,
    /// Age-driven, scaled by walk amount — the rabbit-hop approximation
    /// (vanilla triggers HOP per jump; jump state isn't on our wire, so a
    /// moving rabbit hops continuously).
    AgeGatedByWalk { gate: f32 },
    /// Seconds since the gating gesture's state change, full scale —
    /// vanilla's `apply(animationState, ageInTicks)` one-shots. Only
    /// meaningful under [`KfGate::During`].
    GestureAge,
}

/// A pose/state-driven one-shot animation — vanilla `AnimationState`s
/// started by wire-visible entity state (Pose metadata, sniffer/armadillo
/// state enums). The rig plays from the state-change instant; non-looping
/// rigs hold their last frame while the state persists.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Gesture {
    WardenRoar,
    WardenSniff,
    WardenEmerge,
    WardenDig,
    FrogCroak,
    FrogTongue,
    BreezeShoot,
    BreezeSlide,
    BreezeInhale,
    BreezeJump,
    SnifferFeelingHappy,
    SnifferScenting,
    SnifferSniffing,
    SnifferSearching,
    SnifferDigging,
    SnifferRising,
    ArmadilloRoll,
    /// SCARED — the peek rig, entered fast-forwarded to its held ball pose
    /// (vanilla `fastForward(SCARED.animationDuration())`).
    ArmadilloScared,
    ArmadilloUnroll,
}

impl Gesture {
    /// For the mobshot `--gesture` knob.
    pub fn from_name(s: &str) -> Option<Gesture> {
        Some(match s {
            "warden_roar" => Gesture::WardenRoar,
            "warden_sniff" => Gesture::WardenSniff,
            "warden_emerge" => Gesture::WardenEmerge,
            "warden_dig" => Gesture::WardenDig,
            "frog_croak" => Gesture::FrogCroak,
            "frog_tongue" => Gesture::FrogTongue,
            "breeze_shoot" => Gesture::BreezeShoot,
            "breeze_slide" => Gesture::BreezeSlide,
            "breeze_inhale" => Gesture::BreezeInhale,
            "breeze_jump" => Gesture::BreezeJump,
            "sniffer_happy" => Gesture::SnifferFeelingHappy,
            "sniffer_scenting" => Gesture::SnifferScenting,
            "sniffer_sniffing" => Gesture::SnifferSniffing,
            "sniffer_searching" => Gesture::SnifferSearching,
            "sniffer_digging" => Gesture::SnifferDigging,
            "sniffer_rising" => Gesture::SnifferRising,
            "armadillo_roll" => Gesture::ArmadilloRoll,
            "armadillo_scared" => Gesture::ArmadilloScared,
            "armadillo_unroll" => Gesture::ArmadilloUnroll,
            _ => return None,
        })
    }
}

/// A one-shot animation started by an explicit wire *event*
/// (`ClientboundEntityEventPacket`), NOT by a persistent metadata pose — the
/// distinction matters (`don't pretend they are metadata gestures`): a gesture
/// plays *while* a pose holds; an event fires once and its rig runs from the
/// receipt tick, holding its (neutral) last frame afterward. gpu-local mirror
/// of the rig-driven half of `rewo_world::entities::EntityEvent` (rewo-gpu has
/// no rewo-world dependency). The armadillo peek is deliberately absent: it
/// re-clocks the *existing* SCARED peek gesture (vanilla's one shared
/// `peekAnimationState`), so it is handled on the gesture clock, not here.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ModelEvent {
    /// Warden id 4 — `attackAnimationState.start(tickCount)`.
    WardenAttack,
    /// Warden id 62 — `sonicBoomAnimationState.start(tickCount)`.
    WardenSonicBoom,
}

impl ModelEvent {
    /// Dense count for the fixed-size per-entity event-age array.
    pub const COUNT: usize = 2;

    /// Slot index into the per-entity event-age array.
    pub const fn index(self) -> usize {
        self as usize
    }
}

/// A keyframe rig started by a wire event (see [`ModelEvent`]). Structurally a
/// [`KfAnim`] whose "gate" is the presence of an event age rather than a
/// gesture/walk state; kept as its own type so the event rigs can never be
/// confused with the metadata gesture rigs.
pub struct EventRig {
    pub def: &'static KfDef,
    pub event: ModelEvent,
    /// Channel → part index, resolved at build time (same as [`KfAnim::parts`]).
    pub parts: Vec<u16>,
}

/// Part visibility rule — the armadillo swaps between its body and the
/// rolled-up ball exactly like vanilla's `setupAnim` visibility toggles.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Show {
    Always,
    /// Only while the entity is hiding in its shell.
    ShellOnly,
    /// Hidden while the entity is hiding in its shell.
    NotShell,
    /// Only while this gesture is active (frog `croaking_body`, shown by
    /// vanilla's `croakAnimationState.isStarted()` check).
    During(Gesture),
    /// `IllagerModel.setupAnim`'s tail: `arms.visible = crossedArms`. Shown
    /// only while the derived pose is `CROSSED` (M20).
    IllagerCrossedOnly,
    /// `leftArm.visible = rightArm.visible = !crossedArms` — the counterpart.
    IllagerNotCrossed,
    /// M238 — `ArmorStandModel.setupAnim`: `leftArm.visible =
    /// rightArm.visible = state.showArms`.
    StandArms,
    /// `basePlate.visible = state.showBasePlate`.
    StandBasePlate,
    /// M296 — `DonkeyModel.setupAnim`: `leftChest.visible = rightChest.visible
    /// = state.hasChest`.
    Chested,
    /// M354 — `BeeModel.setupAnim`: `stinger.visible = state.hasStinger`.
    HasStinger,
    /// M365 — `AdultTurtleModel.setupAnim`: `eggBelly.visible =
    /// state.hasEgg`.
    TurtleEgg,
    /// M350 — `ZombieNautilusCoralModel.setupAnim`: `corals.visible =
    /// state.bodyArmorItem.isEmpty()`. Shown unless the draw has body armour.
    Unarmoured,
    /// M310 — `PlayerModel.setupAnim`: `hat.visible = state.showHat`,
    /// `jacket.visible = state.showJacket` and so on, each a
    /// `PlayerModelPart` bit of the avatar's customisation mask (the bit
    /// here). Shown unless [`crate::entities::EntityDraw::avatar_hidden`]
    /// has it.
    AvatarPart(u8),
}

/// M238 — the armour stand's two visibility flags (`DATA_CLIENT_FLAGS` 0x04
/// show arms, 0x08 no base plate). The default is vanilla's flags byte 0:
/// no arms, a base plate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StandParts {
    pub arms: bool,
    pub base_plate: bool,
    /// M270 — `DATA_HEAD_POSE` .. `DATA_RIGHT_LEG_POSE` (indices 16-21),
    /// degrees, in that order: head, body, left arm, right arm, left leg,
    /// right leg.
    pub pose: [[f32; 3]; 6],
}

/// M270 — `ArmorStand.DEFAULT_*_POSE`, degrees, in [`StandParts::pose`]'s
/// order: the pose the model is baked at.
pub const STAND_DEFAULT_POSE: [[f32; 3]; 6] = [
    [0.0, 0.0, 0.0],
    [0.0, 0.0, 0.0],
    [-10.0, 0.0, -10.0],
    [-15.0, 0.0, 10.0],
    [-1.0, 0.0, -1.0],
    [1.0, 0.0, 1.0],
];

impl Default for StandParts {
    fn default() -> Self {
        Self {
            arms: false,
            base_plate: true,
            pose: STAND_DEFAULT_POSE,
        }
    }
}

/// When a keyframe animation plays — vanilla's `setupAnim` call patterns.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum KfGate {
    /// Every frame (walk rigs, breeze idle).
    Always,
    /// Only while the entity's gesture matches. With [`KfDriver::GestureAge`]
    /// this is vanilla's `apply(animationState, age)` one-shot; with a walk
    /// driver it's a state-conditional walk swap (sniffer SNIFF_SEARCH).
    During(Gesture),
    /// Suppressed while the gesture is active (the sniffer walk yields to
    /// the search-walk: `if (isSearching) … else walk`).
    Unless(Gesture),
    /// Suppressed while hiding in the shell (armadillo walk).
    NotShell,
}

/// A keyframe animation attached to a model, channels resolved to part
/// indices at build time. `gate` decides *whether* it plays this frame;
/// `driver` decides *where in time* it samples.
pub struct KfAnim {
    pub def: &'static KfDef,
    pub driver: KfDriver,
    pub gate: KfGate,
    pub parts: Vec<u16>,
}

/// One articulated part: cubes attached to it rotate about `pivot` inside
/// the parent's frame (`parent` = an earlier part index, or the model
/// root). `rot` is the vanilla `PartPose` base rotation; the animation's
/// per-frame deltas are *summed onto it* and composed as one
/// `rotateZYX(z, y, x)` — exactly vanilla's angle math. Some anims also
/// displace the pivot (blaze rods, crawler segments).
pub struct Part {
    pub pivot: [f32; 3],
    pub rot: [f32; 3],
    pub anim: Anim,
    /// Angle multiplier (villager/enderman swing at half amplitude).
    pub amp: f32,
    pub parent: Option<u16>,
    /// Vanilla part name — only needed where keyframe channels target the
    /// part; "" elsewhere.
    pub name: &'static str,
    /// Visibility rule (armadillo shell swap); `Show::Always` elsewhere.
    pub show: Show,
}

/// One textured quad in part-local model px. UVs are in the mob's own
/// texture pixels; [`crate::entities`] normalizes them into the shared atlas
/// once the texture's slot is known.
pub struct RawQuad {
    pub pos: [[f32; 3]; 4],
    pub uv: [[f32; 2]; 4],
    /// Vanilla face label of the source box face (drives the debug texture
    /// + shade). For mirrored cubes the label follows the texture rect, not
    ///   the geometric side — exactly like vanilla.
    pub facing: Facing,
    /// Outward normal with all static folds applied (model space).
    pub normal: [f32; 3],
    /// Which of the mob's textures this quad samples (index into
    /// [`MobDef::textures`]).
    pub tex: usize,
    /// Index into [`Model::parts`].
    pub part: usize,
    /// Baked directional shade (applied in the vertex color).
    pub shade: f32,
}

pub struct Model {
    pub parts: Vec<Part>,
    pub quads: Vec<RawQuad>,
    /// Keyframe rigs playing on this model (channels part-resolved).
    pub keyframes: Vec<KfAnim>,
    /// Event-driven one-shot rigs (`ClientboundEntityEventPacket`), timed from
    /// the receipt tick rather than a metadata pose. Empty for most mobs.
    pub event_rigs: Vec<EventRig>,
    /// Extra scale on top of the 1/16 px→block conversion (player 0.9375,
    /// wither skeleton 1.2, slime 2.0, …).
    pub scale: f32,
    /// Resource-pack CEM animation program (M9c) — per-frame bone channels
    /// driven by the OptiFine expression language. `None` for built-ins.
    pub cem: Option<crate::cem_anim::AnimProgram>,
    /// Per-bone translation *rest baseline* (indexed by part). OptiFine
    /// animation `tx/ty/tz` *replace* a bone's translate rather than adding to
    /// it (FA re-specifies rest + sway), so `eval_program` subtracts this
    /// baseline from driven translation channels, leaving only the sway. The
    /// value differs by bone kind — `invertAxis(pivot)` for a top-level part,
    /// the own relative translate for a submodel. Empty for built-in mobs.
    pub cem_translate: Vec<[f32; 3]>,
    /// Per-bone `.jem` name (top-level `part` / submodel `id`), index-aligned
    /// with `parts`. Diagnostics only (`REWO_CEM_DUMP`). Empty for built-ins.
    pub cem_names: Vec<String>,
    /// Per-bone flag: is this a top-level `part` (vs a submodel)? The two obey
    /// different translate conventions, so the runtime needs to tell them apart.
    pub cem_top: Vec<bool>,
    /// M306 — the first of the quads vanilla draws `entityTranslucent` (a
    /// layer such as `SlimeOuterLayer`); every quad from here on goes to the
    /// translucent range. `None` for a model with none.
    pub translucent_from: Option<usize>,
}

/// A static fold applied to cube vertices at build time: `v → R_zyx(rot)·v +
/// off`. This is exactly vanilla's `translate(pose); rotateZYX(z,y,x)` — a
/// chain of them reproduces nested `PartDefinition` children.
#[derive(Clone, Copy)]
pub struct Fold {
    pub rot: [f32; 3], // (xRot, yRot, zRot), vanilla order
    pub off: [f32; 3],
}

impl Fold {
    pub const fn at(off: [f32; 3]) -> Self {
        Fold { rot: [0.0; 3], off }
    }
    pub const fn rot(rot: [f32; 3], off: [f32; 3]) -> Self {
        Fold { rot, off }
    }
}

/// `rotateZYX(z, y, x)` applied to a vector: Rx first, then Ry, then Rz —
/// matching JOML's `rotationZYX` (M = Rz·Ry·Rx).
pub fn rotate_zyx(v: [f32; 3], rot: [f32; 3]) -> [f32; 3] {
    let [rx, ry, rz] = rot;
    let [mut x, mut y, mut z] = v;
    if rx != 0.0 {
        let (s, c) = rx.sin_cos();
        let (y1, z1) = (y * c - z * s, y * s + z * c);
        y = y1;
        z = z1;
    }
    if ry != 0.0 {
        let (s, c) = ry.sin_cos();
        let (x1, z1) = (x * c + z * s, -x * s + z * c);
        x = x1;
        z = z1;
    }
    if rz != 0.0 {
        let (s, c) = rz.sin_cos();
        let (x1, y1) = (x * c - y * s, x * s + y * c);
        x = x1;
        y = y1;
    }
    [x, y, z]
}

/// Directional shade for a static-folded model-space normal, matching the
/// entity pass's classic 6-way table. Classified after the render y-flip:
/// model −Y is the world-top (brightest), model −Z the front.
pub(crate) fn shade_for(n: [f32; 3]) -> f32 {
    if n[1] < -0.7 {
        1.0 // world top
    } else if n[1] > 0.7 {
        0.5 // world bottom
    } else if n[2] < -0.7 {
        0.80 // front
    } else if n[2] > 0.7 {
        0.62 // back
    } else {
        0.68 // sides
    }
}

// ---------------------------------------------------------------------------
// The faithful Cube port
// ---------------------------------------------------------------------------

/// Exact port of vanilla `ModelPart.Cube` + `Polygon`: given a box (tex
/// offset, min corner, dims, uniform grow, mirror), produce the 6 faces with
/// vanilla's vertex arrays, UV rects, and per-vertex UV corner assignment.
///
/// Ground truth (26.2 `ModelPart.java`):
/// ```text
/// u0=texX  u1=+d  u2=+d+w  u22=+d+2w  u3=+d+w+d  u4=+d+w+d+w
/// v0=texY  v1=+d  v2=+d+h
/// DOWN : {l1,l0,t0,t1}  rect (u1,v0,u2,v1)
/// UP   : {t2,t3,l3,l2}  rect (u2,v1,u22,v0)   // v1 before v0 → vertical flip
/// WEST : {t0,l0,l3,t3}  rect (u0,v1,u1,v2)
/// NORTH: {t1,t0,t3,t2}  rect (u1,v1,u2,v2)
/// EAST : {l1,t1,t2,l2}  rect (u2,v1,u3,v2)
/// SOUTH: {l0,l1,l2,l3}  rect (u3,v1,u4,v2)
/// Polygon remap: verts[0]=(u1,v0) [1]=(u0,v0) [2]=(u0,v1) [3]=(u1,v1);
/// mirror ⇒ swap(minX,maxX) before corners + reverse the vertex array.
/// ```
pub fn cube_faces(
    tex: (f32, f32),
    min: [f32; 3],
    dims: [f32; 3],
    grow: f32,
    mirror: bool,
) -> [(Facing, [[f32; 3]; 4], [[f32; 2]; 4]); 6] {
    let (tex_u, tex_v) = tex;
    let [w, h, d] = dims;
    let [mut x0, mut y0, mut z0] = min;
    let (mut x1, mut y1, mut z1) = (x0 + w, y0 + h, z0 + d);
    x0 -= grow;
    y0 -= grow;
    z0 -= grow;
    x1 += grow;
    y1 += grow;
    z1 += grow;
    if mirror {
        std::mem::swap(&mut x0, &mut x1);
    }
    let t0 = [x0, y0, z0];
    let t1 = [x1, y0, z0];
    let t2 = [x1, y1, z0];
    let t3 = [x0, y1, z0];
    let l0 = [x0, y0, z1];
    let l1 = [x1, y0, z1];
    let l2 = [x1, y1, z1];
    let l3 = [x0, y1, z1];
    let u0 = tex_u;
    let u1 = tex_u + d;
    let u2 = tex_u + d + w;
    let u22 = tex_u + d + w + w;
    let u3 = tex_u + d + w + d;
    let u4 = tex_u + d + w + d + w;
    let v0 = tex_v;
    let v1 = tex_v + d;
    let v2 = tex_v + d + h;

    let face = |facing: Facing, verts: [[f32; 3]; 4], rect: (f32, f32, f32, f32)| {
        // Polygon remap — rect is the passed (u0,v0,u1,v1) quadruple.
        let (ru0, rv0, ru1, rv1) = rect;
        let mut vs = verts;
        let mut uvs = [[ru1, rv0], [ru0, rv0], [ru0, rv1], [ru1, rv1]];
        if mirror {
            // Vanilla reverses the vertex array (each vertex keeps its UV).
            vs.reverse();
            uvs.reverse();
        }
        (facing, vs, uvs)
    };

    [
        face(Facing::Down, [l1, l0, t0, t1], (u1, v0, u2, v1)),
        face(Facing::Up, [t2, t3, l3, l2], (u2, v1, u22, v0)),
        face(Facing::West, [t0, l0, l3, t3], (u0, v1, u1, v2)),
        face(Facing::North, [t1, t0, t3, t2], (u1, v1, u2, v2)),
        face(Facing::East, [l1, t1, t2, l2], (u2, v1, u3, v2)),
        face(Facing::South, [l0, l1, l2, l3], (u3, v1, u4, v2)),
    ]
}

/// Slot index for a face in the per-face UV override array
/// ([`ModelBuilder::cube_f_faceuv`]). Must agree with the array the CEM
/// parser builds in `emit_box`.
const fn face_slot(f: Facing) -> usize {
    match f {
        Facing::North => 0,
        Facing::South => 1,
        Facing::East => 2,
        Facing::West => 3,
        Facing::Up => 4,
        Facing::Down => 5,
    }
}

// ---------------------------------------------------------------------------
// Model builder
// ---------------------------------------------------------------------------

/// Index of the shared static root part (identity, no animation).
pub const STATIC_PART: usize = 0;

pub struct ModelBuilder {
    parts: Vec<Part>,
    quads: Vec<RawQuad>,
    keyframes: Vec<KfAnim>,
    event_rigs: Vec<EventRig>,
    translucent_from: Option<usize>,
}

impl ModelBuilder {
    pub(crate) fn new() -> Self {
        ModelBuilder {
            parts: vec![Part {
                pivot: [0.0; 3],
                rot: [0.0; 3],
                anim: Anim::None,
                amp: 1.0,
                parent: None,
                name: "",
                show: Show::Always,
            }],
            quads: Vec::new(),
            keyframes: Vec::new(),
            event_rigs: Vec::new(),
            translucent_from: None,
        }
    }

    /// M306 — every cube added after this is drawn `entityTranslucent`
    /// ([`Model::translucent_from`]).
    pub(crate) fn begin_translucent(&mut self) {
        self.translucent_from = Some(self.quads.len());
    }

    /// Register an animated part at the model root with no base rotation
    /// (the common case).
    fn part(&mut self, pivot: [f32; 3], anim: Anim, amp: f32) -> usize {
        self.part_ext(pivot, [0.0; 3], anim, amp, None)
    }

    /// Register a part with a vanilla `PartPose` base rotation and/or a
    /// parent part (runtime hierarchy — wing tips on wings, arms on a
    /// tilted body). Parents must be registered first.
    fn part_ext(
        &mut self,
        pivot: [f32; 3],
        rot: [f32; 3],
        anim: Anim,
        amp: f32,
        parent: Option<usize>,
    ) -> usize {
        self.part_named("", pivot, rot, anim, amp, parent)
    }

    /// `part_ext` with the vanilla part name — required wherever a keyframe
    /// channel targets the part.
    fn part_named(
        &mut self,
        name: &'static str,
        pivot: [f32; 3],
        rot: [f32; 3],
        anim: Anim,
        amp: f32,
        parent: Option<usize>,
    ) -> usize {
        debug_assert!(parent.is_none_or(|p| p < self.parts.len()));
        self.parts.push(Part {
            pivot,
            rot,
            anim,
            amp,
            parent: parent.map(|p| p as u16),
            name,
            show: Show::Always,
        });
        self.parts.len() - 1
    }

    /// Set a part's visibility rule (armadillo shell swap).
    fn show(&mut self, part: usize, show: Show) {
        self.parts[part].show = show;
    }

    /// Attach a keyframe rig; every channel's part name must exist (panics
    /// at build otherwise — `registry_builds_clean` catches it).
    fn keyframe(&mut self, def: &'static KfDef, driver: KfDriver) {
        self.keyframe_gated(def, driver, KfGate::Always);
    }

    /// Attach a pose/state-driven one-shot rig (vanilla `apply(state, age)`).
    fn keyframe_gesture(&mut self, def: &'static KfDef, gesture: Gesture) {
        self.keyframe_gated(def, KfDriver::GestureAge, KfGate::During(gesture));
    }

    fn keyframe_gated(&mut self, def: &'static KfDef, driver: KfDriver, gate: KfGate) {
        let parts = self.resolve_channel_parts(def);
        self.keyframes.push(KfAnim {
            def,
            driver,
            gate,
            parts,
        });
    }

    /// Attach a wire-event-driven one-shot rig (vanilla `attackAnimationState`
    /// etc.) — timed from the event's receipt tick, not a metadata pose. Every
    /// channel's part name must exist (so e.g. `WARDEN_SONIC_BOOM`'s
    /// `right_ribcage` channel forces the ribcage to be a real named part).
    fn keyframe_event(&mut self, def: &'static KfDef, event: ModelEvent) {
        let parts = self.resolve_channel_parts(def);
        self.event_rigs.push(EventRig { def, event, parts });
    }

    /// Resolve each of a rig's channels to a model part index by vanilla name
    /// (panics at build if a channel targets a missing part — the registry
    /// build test catches it).
    fn resolve_channel_parts(&self, def: &'static KfDef) -> Vec<u16> {
        def.channels
            .iter()
            .map(|ch| {
                self.parts
                    .iter()
                    .position(|p| p.name == ch.part)
                    .unwrap_or_else(|| panic!("keyframe part {:?} missing from model", ch.part))
                    as u16
            })
            .collect()
    }

    /// Add one vanilla `addBox` to a part. `folds` reproduce nested static
    /// child poses, innermost first; for `STATIC_PART` the outermost fold is
    /// the part's own `PartPose`.
    ///
    /// Zero-area faces of plate boxes (fins/wings authored with a 0 dim)
    /// are skipped — they rasterize nothing, and their UV rects are
    /// degenerate (vanilla even gives some negative offsets there).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn cube_f(
        &mut self,
        part: usize,
        tex: usize,
        uv: (f32, f32),
        min: [f32; 3],
        dims: [f32; 3],
        grow: f32,
        mirror: bool,
        folds: &[Fold],
    ) {
        self.cube_f_faceuv(part, tex, uv, min, dims, grow, mirror, folds, &[None; 6]);
    }

    /// Like [`cube_f`] but with optional per-face UV rects (OptiFine CEM
    /// `uvNorth`/`uvSouth`/… — texture-pixel `[u0,v0,u1,v1]`). A face with an
    /// override uses that rect through the *same* corner winding as the box-UV
    /// path (so setting a face's override to its box-UV rect is a no-op);
    /// faces left `None` fall back to the box-UV unwrap. Used by the CEM
    /// parser for hand-placed detail — creeper/zombie eyes, the pig snout.
    /// Reversed rects (`u0>u1`) mirror the face, exactly like OptiFine.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn cube_f_faceuv(
        &mut self,
        part: usize,
        tex: usize,
        uv: (f32, f32),
        min: [f32; 3],
        dims: [f32; 3],
        grow: f32,
        mirror: bool,
        folds: &[Fold],
        face_uv: &[Option<[f32; 4]>; 6],
    ) {
        let [w, h, d] = dims;
        for (facing, mut pos, mut uvs) in cube_faces(uv, min, dims, grow, mirror) {
            let area = match facing {
                Facing::Down | Facing::Up => w * d,
                Facing::West | Facing::East => d * h,
                Facing::North | Facing::South => w * h,
            };
            if area == 0.0 {
                continue;
            }
            if let Some([a, b, c, e]) = face_uv[face_slot(facing)] {
                // Same corner assignment as cube_faces' `face` closure for
                // rect (a,b,c,e): verts get [(c,b),(a,b),(a,e),(c,e)]. Mirror
                // reverses to stay aligned with the (already-reversed) verts.
                let mut ov = [[c, b], [a, b], [a, e], [c, e]];
                if mirror {
                    ov.reverse();
                }
                uvs = ov;
            }
            let mut normal = facing.normal();
            for f in folds {
                for p in &mut pos {
                    let r = rotate_zyx(*p, f.rot);
                    *p = [r[0] + f.off[0], r[1] + f.off[1], r[2] + f.off[2]];
                }
                normal = rotate_zyx(normal, f.rot);
            }
            self.quads.push(RawQuad {
                pos,
                uv: uvs,
                facing,
                normal,
                tex,
                part,
                shade: shade_for(normal),
            });
        }
    }

    fn cube(
        &mut self,
        part: usize,
        tex: usize,
        uv: (f32, f32),
        min: [f32; 3],
        dims: [f32; 3],
        folds: &[Fold],
    ) {
        self.cube_f(part, tex, uv, min, dims, 0.0, false, folds);
    }

    fn cube_m(
        &mut self,
        part: usize,
        tex: usize,
        uv: (f32, f32),
        min: [f32; 3],
        dims: [f32; 3],
        folds: &[Fold],
    ) {
        self.cube_f(part, tex, uv, min, dims, 0.0, true, folds);
    }

    fn cube_g(
        &mut self,
        part: usize,
        tex: usize,
        uv: (f32, f32),
        min: [f32; 3],
        dims: [f32; 3],
        grow: f32,
        folds: &[Fold],
    ) {
        self.cube_f(part, tex, uv, min, dims, grow, false, folds);
    }

    pub(crate) fn finish(self, scale: f32) -> Model {
        Model {
            parts: self.parts,
            quads: self.quads,
            keyframes: self.keyframes,
            event_rigs: self.event_rigs,
            scale,
            cem: None,
            cem_translate: Vec::new(),
            cem_names: Vec::new(),
            cem_top: Vec::new(),
            translucent_from: self.translucent_from,
        }
    }

    /// A CEM bone with an explicit pivot and optional parent bone (M9c
    /// named-bone models — the `.jem` part/submodel tree). Returns the part
    /// index; boxes added to it must be relative to the bone's *absolute*
    /// pivot, and `pivot` is relative to the parent bone's pivot (root bones
    /// pass their absolute pivot with `parent = None`), matching how
    /// `part_transforms` composes the chain.
    pub(crate) fn cem_part(
        &mut self,
        pivot: [f32; 3],
        rot: [f32; 3],
        parent: Option<usize>,
    ) -> usize {
        debug_assert!(parent.is_none_or(|p| p < self.parts.len()));
        // CEM bones get their per-frame rotation/offset/scale from the
        // animation program (applied in `part_transforms` by part index), not
        // a built-in `Anim` formula — so the marker is `None`. `rot` is the
        // node's static `.jem` `rotate` (a base pose the animation adds onto).
        self.parts.push(Part {
            pivot,
            rot,
            anim: Anim::None,
            amp: 1.0,
            parent: parent.map(|p| p as u16),
            name: "",
            show: Show::Always,
        });
        self.parts.len() - 1
    }
}

// ---------------------------------------------------------------------------
// Mob registry
// ---------------------------------------------------------------------------

/// Which model to render for an entity. `Capsule` is the fallback for
/// everything without a bespoke model (or with a missing texture).
///
/// Keep `ALL` in the same order as the variants — `index()` is derived
/// from it and the mobshot sheet iterates it.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum EntityModelKind {
    Player,
    Zombie,
    ZombieVillager,
    Husk,
    Drowned,
    Skeleton,
    Stray,
    Bogged,
    Parched,
    WitherSkeleton,
    Creeper,
    Spider,
    CaveSpider,
    Enderman,
    Slime,
    MagmaCube,
    Cow,
    Mooshroom,
    Pig,
    Sheep,
    Chicken,
    Wolf,
    Squid,
    GlowSquid,
    Rabbit,
    Villager,
    WanderingTrader,
    Witch,
    Pillager,
    Vindicator,
    Evoker,
    Illusioner,
    Vex,
    Phantom,
    Guardian,
    ElderGuardian,
    Shulker,
    Silverfish,
    Endermite,
    Blaze,
    Ghast,
    Piglin,
    PiglinBrute,
    ZombifiedPiglin,
    Hoglin,
    Zoglin,
    Strider,
    Bat,
    Cat,
    Ocelot,
    Fox,
    Goat,
    Bee,
    Frog,
    Tadpole,
    Armadillo,
    Axolotl,
    Dolphin,
    Turtle,
    Cod,
    Salmon,
    Pufferfish,
    TropicalFish,
    /// The tropical fish's *other* body plan (M68). One entity type, two
    /// meshes — `TropicalFishRenderer.submit` assigns `this.model` from
    /// `state.pattern.base()` before every submission — so it is two kinds
    /// here, chosen by the caller from the packed variant's low bit exactly as
    /// [`EntityModelKind::PlayerSlim`] is chosen from a profile's model.
    TropicalFishLarge,
    Panda,
    PolarBear,
    Camel,
    Llama,
    TraderLlama,
    Parrot,
    Horse,
    Donkey,
    Mule,
    SkeletonHorse,
    ZombieHorse,
    SnowGolem,
    IronGolem,
    Allay,
    Warden,
    Sniffer,
    Breeze,
    Creaking,
    Ravager,
    Wither,
    EnderDragon,
    HappyGhast,
    CopperGolem,
    Nautilus,
    ZombieNautilus,
    /// Slim-armed ("Alex") player — chosen per-entity from the profile's
    /// skin model; never wire-mapped (players resolve to `Player` and the
    /// caller swaps to this when the skin is slim).
    PlayerSlim,
    /// M223 — `BoatModel` (every wood but bamboo; the wood picks the
    /// texture), its chest variant, and `RaftModel` (bamboo) with its chest
    /// variant. Not living, so no walk, head or death animation.
    Boat,
    ChestBoat,
    Raft,
    ChestRaft,
    /// M252 — `MinecartModel`, for all seven minecart types (their contents
    /// block is not drawn). Not living.
    Minecart,
    /// M238 — `ArmorStandModel` at its default pose; its two flags arrive
    /// per draw ([`StandParts`]), its small form as a half scale.
    ArmorStand,
    /// M303 — `CamelHuskRenderer`: the camel's `AdultCamelModel` with the
    /// husk's sheet. Its wire name had no kind, so it drew as a capsule.
    /// Last before the capsule so every earlier kind keeps its index (and
    /// its cell in `mobtexshot m5`'s crowd).
    CamelHusk,
    /// M304 — `GiantMobRenderer`: the humanoid mesh scaled 6.0
    /// (`LayerDefinitions`' `giantScale`) on the zombie's sheet. It drew as
    /// a capsule before.
    Giant,
    /// M307 — `SulfurCubeRenderer`: `SulfurCubeModel`'s inner box under its
    /// `entityTranslucent` outer box. It drew as a capsule before.
    SulfurCube,
    /// M307 — `SmallSulfurCubeModel`, which `SulfurCubeRenderer.submit`
    /// swaps in for a baby: one wire name, two meshes, chosen by the caller
    /// as [`EntityModelKind::TropicalFishLarge`] is.
    SulfurCubeSmall,
    /// M314 — `HAPPY_GHAST_BABY`, the ghastling's own model, which
    /// `AgeableMobRenderer` swaps in for a baby (chosen by the caller).
    HappyGhastBaby,
    /// M315 — `BabyTurtleModel` (`TURTLE_BABY`), swapped in for a baby.
    TurtleBaby,
    /// M316 — `BabyPolarBearModel` (`POLAR_BEAR_BABY`), swapped in for a cub.
    PolarBearBaby,
    /// M317 — `BabyZombieModel` (`ZOMBIE_BABY`), and the husk's on it
    /// (`HUSK_BABY` is the same layer), swapped in for a baby.
    ZombieBaby,
    HuskBaby,
    /// M318 — `BabyVillagerModel` (`VILLAGER_BABY`), swapped in for a baby.
    VillagerBaby,
    /// M319 — `BabyDrownedModel` (`BabyZombieModel`'s layer) with its outer
    /// layer, swapped in for a baby.
    DrownedBaby,
    /// M320 — `BabyPiglinModel` (`PIGLIN_BABY`), and the zombified piglin's
    /// on the same layer, swapped in for a baby.
    PiglinBaby,
    ZombifiedPiglinBaby,
    /// M321 — `BabyCowModel` (`COW_BABY`, `MOOSHROOM_BABY`), swapped in for
    /// a calf.
    CowBaby,
    MooshroomBaby,
    /// M322 — `BabyPigModel` (`PIG_BABY`) and `BabyChickenModel`
    /// (`CHICKEN_BABY`).
    PigBaby,
    ChickenBaby,
    /// M323 — `BabySheepModel` (`SHEEP_BABY`, and `SHEEP_BABY_WOOL`, the same
    /// layer, for the fleece).
    SheepBaby,
    /// M324 — `BabyDolphinModel` (`DOLPHIN_BABY`).
    DolphinBaby,
    /// M325 — `BabyZombieVillagerModel` (`ZOMBIE_VILLAGER_BABY`).
    ZombieVillagerBaby,
    /// M326 — `BabySquidModel` (`SQUID_BABY`, `GLOW_SQUID_BABY`).
    SquidBaby,
    GlowSquidBaby,
    /// M327 — `BabyHoglinModel` (`HOGLIN_BABY`, `ZOGLIN_BABY`).
    HoglinBaby,
    ZoglinBaby,
    /// M328 — `BabyStriderModel` (`STRIDER_BABY`).
    StriderBaby,
    /// M330 — `BabyFoxModel` (`FOX_BABY`).
    FoxBaby,
    /// M331 — `BabyPandaModel` (`PANDA_BABY`).
    PandaBaby,
    /// M332 — `BabyOcelotModel` (`OCELOT_BABY`, `BabyFelineModel`'s layer).
    OcelotBaby,
    /// M333 — `BabyBeeModel` (`BEE_BABY`).
    BeeBaby,
    /// M334 — `BabyDonkeyModel` (`DONKEY_BABY`, `MULE_BABY`).
    DonkeyBaby,
    MuleBaby,
    /// M335 — `BabyCamelModel` (`CAMEL_BABY`).
    CamelBaby,
    /// M336 — `BabyRabbitModel` (`RABBIT_BABY`).
    RabbitBaby,
    /// M337 — `BabyGoatModel` (`GOAT_BABY`).
    GoatBaby,
    /// M338 — `BabyLlamaModel` (`LLAMA_BABY`).
    LlamaBaby,
    /// M339 — `BabyHorseModel` (`HORSE_BABY`).
    HorseBaby,
    /// M340 — `BabyCatModel` (`CAT_BABY`, `BabyFelineModel`'s layer).
    CatBaby,
    /// M341 — `BabyWolfModel` (`WOLF_BABY`).
    WolfBaby,
    /// M348 — `ColdCowModel` (`COLD_COW`) and `WarmCowModel` (`WARM_COW`),
    /// the adult meshes a `cow_variant`'s `model` picks (`CowRenderer`).
    ColdCow,
    WarmCow,
    /// M349 — `ColdPigModel` (`COLD_PIG`) and `ColdChickenModel`
    /// (`COLD_CHICKEN`), the meshes a cold variant's `model` picks.
    ColdPig,
    ColdChicken,
    /// M350 — `ZombieNautilusCoralModel` (`ZOMBIE_NAUTILUS_CORAL`), the mesh
    /// a warm `zombie_nautilus_variant` picks.
    ZombieNautilusCoral,
    Capsule,
}

impl EntityModelKind {
    pub const COUNT: usize = 140;
    pub const ALL: [EntityModelKind; Self::COUNT] = [
        EntityModelKind::Player,
        EntityModelKind::Zombie,
        EntityModelKind::ZombieVillager,
        EntityModelKind::Husk,
        EntityModelKind::Drowned,
        EntityModelKind::Skeleton,
        EntityModelKind::Stray,
        EntityModelKind::Bogged,
        EntityModelKind::Parched,
        EntityModelKind::WitherSkeleton,
        EntityModelKind::Creeper,
        EntityModelKind::Spider,
        EntityModelKind::CaveSpider,
        EntityModelKind::Enderman,
        EntityModelKind::Slime,
        EntityModelKind::MagmaCube,
        EntityModelKind::Cow,
        EntityModelKind::Mooshroom,
        EntityModelKind::Pig,
        EntityModelKind::Sheep,
        EntityModelKind::Chicken,
        EntityModelKind::Wolf,
        EntityModelKind::Squid,
        EntityModelKind::GlowSquid,
        EntityModelKind::Rabbit,
        EntityModelKind::Villager,
        EntityModelKind::WanderingTrader,
        EntityModelKind::Witch,
        EntityModelKind::Pillager,
        EntityModelKind::Vindicator,
        EntityModelKind::Evoker,
        EntityModelKind::Illusioner,
        EntityModelKind::Vex,
        EntityModelKind::Phantom,
        EntityModelKind::Guardian,
        EntityModelKind::ElderGuardian,
        EntityModelKind::Shulker,
        EntityModelKind::Silverfish,
        EntityModelKind::Endermite,
        EntityModelKind::Blaze,
        EntityModelKind::Ghast,
        EntityModelKind::Piglin,
        EntityModelKind::PiglinBrute,
        EntityModelKind::ZombifiedPiglin,
        EntityModelKind::Hoglin,
        EntityModelKind::Zoglin,
        EntityModelKind::Strider,
        EntityModelKind::Bat,
        EntityModelKind::Cat,
        EntityModelKind::Ocelot,
        EntityModelKind::Fox,
        EntityModelKind::Goat,
        EntityModelKind::Bee,
        EntityModelKind::Frog,
        EntityModelKind::Tadpole,
        EntityModelKind::Armadillo,
        EntityModelKind::Axolotl,
        EntityModelKind::Dolphin,
        EntityModelKind::Turtle,
        EntityModelKind::Cod,
        EntityModelKind::Salmon,
        EntityModelKind::Pufferfish,
        EntityModelKind::TropicalFish,
        EntityModelKind::TropicalFishLarge,
        EntityModelKind::Panda,
        EntityModelKind::PolarBear,
        EntityModelKind::Camel,
        EntityModelKind::Llama,
        EntityModelKind::TraderLlama,
        EntityModelKind::Parrot,
        EntityModelKind::Horse,
        EntityModelKind::Donkey,
        EntityModelKind::Mule,
        EntityModelKind::SkeletonHorse,
        EntityModelKind::ZombieHorse,
        EntityModelKind::SnowGolem,
        EntityModelKind::IronGolem,
        EntityModelKind::Allay,
        EntityModelKind::Warden,
        EntityModelKind::Sniffer,
        EntityModelKind::Breeze,
        EntityModelKind::Creaking,
        EntityModelKind::Ravager,
        EntityModelKind::Wither,
        EntityModelKind::EnderDragon,
        EntityModelKind::HappyGhast,
        EntityModelKind::CopperGolem,
        EntityModelKind::Nautilus,
        EntityModelKind::ZombieNautilus,
        EntityModelKind::PlayerSlim,
        EntityModelKind::Boat,
        EntityModelKind::ChestBoat,
        EntityModelKind::Raft,
        EntityModelKind::ChestRaft,
        EntityModelKind::Minecart,
        EntityModelKind::ArmorStand,
        EntityModelKind::CamelHusk,
        EntityModelKind::Giant,
        EntityModelKind::SulfurCube,
        EntityModelKind::SulfurCubeSmall,
        EntityModelKind::HappyGhastBaby,
        EntityModelKind::TurtleBaby,
        EntityModelKind::PolarBearBaby,
        EntityModelKind::ZombieBaby,
        EntityModelKind::HuskBaby,
        EntityModelKind::VillagerBaby,
        EntityModelKind::DrownedBaby,
        EntityModelKind::PiglinBaby,
        EntityModelKind::ZombifiedPiglinBaby,
        EntityModelKind::CowBaby,
        EntityModelKind::MooshroomBaby,
        EntityModelKind::PigBaby,
        EntityModelKind::ChickenBaby,
        EntityModelKind::SheepBaby,
        EntityModelKind::DolphinBaby,
        EntityModelKind::ZombieVillagerBaby,
        EntityModelKind::SquidBaby,
        EntityModelKind::GlowSquidBaby,
        EntityModelKind::HoglinBaby,
        EntityModelKind::ZoglinBaby,
        EntityModelKind::StriderBaby,
        EntityModelKind::FoxBaby,
        EntityModelKind::PandaBaby,
        EntityModelKind::OcelotBaby,
        EntityModelKind::BeeBaby,
        EntityModelKind::DonkeyBaby,
        EntityModelKind::MuleBaby,
        EntityModelKind::CamelBaby,
        EntityModelKind::RabbitBaby,
        EntityModelKind::GoatBaby,
        EntityModelKind::LlamaBaby,
        EntityModelKind::HorseBaby,
        EntityModelKind::CatBaby,
        EntityModelKind::WolfBaby,
        EntityModelKind::ColdCow,
        EntityModelKind::WarmCow,
        EntityModelKind::ColdPig,
        EntityModelKind::ColdChicken,
        EntityModelKind::ZombieNautilusCoral,
        EntityModelKind::Capsule,
    ];

    pub fn index(self) -> usize {
        Self::ALL.iter().position(|k| *k == self).unwrap()
    }

    pub fn name(self) -> &'static str {
        match self {
            EntityModelKind::Player => "player",
            EntityModelKind::Zombie => "zombie",
            EntityModelKind::ZombieVillager => "zombie_villager",
            EntityModelKind::Husk => "husk",
            EntityModelKind::Drowned => "drowned",
            EntityModelKind::Skeleton => "skeleton",
            EntityModelKind::Stray => "stray",
            EntityModelKind::Bogged => "bogged",
            EntityModelKind::Parched => "parched",
            EntityModelKind::WitherSkeleton => "wither_skeleton",
            EntityModelKind::Creeper => "creeper",
            EntityModelKind::Spider => "spider",
            EntityModelKind::CaveSpider => "cave_spider",
            EntityModelKind::Enderman => "enderman",
            EntityModelKind::Slime => "slime",
            EntityModelKind::MagmaCube => "magma_cube",
            EntityModelKind::Cow => "cow",
            EntityModelKind::Mooshroom => "mooshroom",
            EntityModelKind::Pig => "pig",
            EntityModelKind::Sheep => "sheep",
            EntityModelKind::Chicken => "chicken",
            EntityModelKind::Wolf => "wolf",
            EntityModelKind::Squid => "squid",
            EntityModelKind::GlowSquid => "glow_squid",
            EntityModelKind::Rabbit => "rabbit",
            EntityModelKind::Villager => "villager",
            EntityModelKind::WanderingTrader => "wandering_trader",
            EntityModelKind::Witch => "witch",
            EntityModelKind::Pillager => "pillager",
            EntityModelKind::Vindicator => "vindicator",
            EntityModelKind::Evoker => "evoker",
            EntityModelKind::Illusioner => "illusioner",
            EntityModelKind::Vex => "vex",
            EntityModelKind::Phantom => "phantom",
            EntityModelKind::Guardian => "guardian",
            EntityModelKind::ElderGuardian => "elder_guardian",
            EntityModelKind::Shulker => "shulker",
            EntityModelKind::Silverfish => "silverfish",
            EntityModelKind::Endermite => "endermite",
            EntityModelKind::Blaze => "blaze",
            EntityModelKind::Ghast => "ghast",
            EntityModelKind::Piglin => "piglin",
            EntityModelKind::PiglinBrute => "piglin_brute",
            EntityModelKind::ZombifiedPiglin => "zombified_piglin",
            EntityModelKind::Hoglin => "hoglin",
            EntityModelKind::Zoglin => "zoglin",
            EntityModelKind::Strider => "strider",
            EntityModelKind::Bat => "bat",
            EntityModelKind::Cat => "cat",
            EntityModelKind::Ocelot => "ocelot",
            EntityModelKind::Fox => "fox",
            EntityModelKind::Goat => "goat",
            EntityModelKind::Bee => "bee",
            EntityModelKind::Frog => "frog",
            EntityModelKind::Tadpole => "tadpole",
            EntityModelKind::Armadillo => "armadillo",
            EntityModelKind::Axolotl => "axolotl",
            EntityModelKind::Dolphin => "dolphin",
            EntityModelKind::Turtle => "turtle",
            EntityModelKind::Cod => "cod",
            EntityModelKind::Salmon => "salmon",
            EntityModelKind::Pufferfish => "pufferfish",
            EntityModelKind::TropicalFish => "tropical_fish",
            EntityModelKind::TropicalFishLarge => "tropical_fish_large",
            EntityModelKind::Panda => "panda",
            EntityModelKind::PolarBear => "polar_bear",
            EntityModelKind::Camel => "camel",
            EntityModelKind::Llama => "llama",
            EntityModelKind::TraderLlama => "trader_llama",
            EntityModelKind::Parrot => "parrot",
            EntityModelKind::Horse => "horse",
            EntityModelKind::Donkey => "donkey",
            EntityModelKind::Mule => "mule",
            EntityModelKind::SkeletonHorse => "skeleton_horse",
            EntityModelKind::ZombieHorse => "zombie_horse",
            EntityModelKind::SnowGolem => "snow_golem",
            EntityModelKind::IronGolem => "iron_golem",
            EntityModelKind::Allay => "allay",
            EntityModelKind::Warden => "warden",
            EntityModelKind::Sniffer => "sniffer",
            EntityModelKind::Breeze => "breeze",
            EntityModelKind::Creaking => "creaking",
            EntityModelKind::Ravager => "ravager",
            EntityModelKind::Wither => "wither",
            EntityModelKind::EnderDragon => "ender_dragon",
            EntityModelKind::HappyGhast => "happy_ghast",
            EntityModelKind::CopperGolem => "copper_golem",
            EntityModelKind::Nautilus => "nautilus",
            EntityModelKind::ZombieNautilus => "zombie_nautilus",
            EntityModelKind::CamelHusk => "camel_husk",
            EntityModelKind::Giant => "giant",
            EntityModelKind::SulfurCube => "sulfur_cube",
            EntityModelKind::SulfurCubeSmall => "sulfur_cube_small",
            EntityModelKind::HappyGhastBaby => "happy_ghast_baby",
            EntityModelKind::TurtleBaby => "turtle_baby",
            EntityModelKind::PolarBearBaby => "polar_bear_baby",
            EntityModelKind::ZombieBaby => "zombie_baby",
            EntityModelKind::HuskBaby => "husk_baby",
            EntityModelKind::VillagerBaby => "villager_baby",
            EntityModelKind::DrownedBaby => "drowned_baby",
            EntityModelKind::PiglinBaby => "piglin_baby",
            EntityModelKind::ZombifiedPiglinBaby => "zombified_piglin_baby",
            EntityModelKind::CowBaby => "cow_baby",
            EntityModelKind::MooshroomBaby => "mooshroom_baby",
            EntityModelKind::PigBaby => "pig_baby",
            EntityModelKind::ChickenBaby => "chicken_baby",
            EntityModelKind::SheepBaby => "sheep_baby",
            EntityModelKind::DolphinBaby => "dolphin_baby",
            EntityModelKind::ZombieVillagerBaby => "zombie_villager_baby",
            EntityModelKind::SquidBaby => "squid_baby",
            EntityModelKind::GlowSquidBaby => "glow_squid_baby",
            EntityModelKind::HoglinBaby => "hoglin_baby",
            EntityModelKind::ZoglinBaby => "zoglin_baby",
            EntityModelKind::StriderBaby => "strider_baby",
            EntityModelKind::FoxBaby => "fox_baby",
            EntityModelKind::PandaBaby => "panda_baby",
            EntityModelKind::OcelotBaby => "ocelot_baby",
            EntityModelKind::BeeBaby => "bee_baby",
            EntityModelKind::DonkeyBaby => "donkey_baby",
            EntityModelKind::MuleBaby => "mule_baby",
            EntityModelKind::CamelBaby => "camel_baby",
            EntityModelKind::RabbitBaby => "rabbit_baby",
            EntityModelKind::GoatBaby => "goat_baby",
            EntityModelKind::LlamaBaby => "llama_baby",
            EntityModelKind::HorseBaby => "horse_baby",
            EntityModelKind::CatBaby => "cat_baby",
            EntityModelKind::WolfBaby => "wolf_baby",
            EntityModelKind::ColdCow => "cold_cow",
            EntityModelKind::WarmCow => "warm_cow",
            EntityModelKind::ColdPig => "cold_pig",
            EntityModelKind::ColdChicken => "cold_chicken",
            EntityModelKind::ZombieNautilusCoral => "zombie_nautilus_coral",
            EntityModelKind::PlayerSlim => "player_slim",
            EntityModelKind::Boat => "boat",
            EntityModelKind::ChestBoat => "chest_boat",
            EntityModelKind::Raft => "raft",
            EntityModelKind::ChestRaft => "chest_raft",
            EntityModelKind::Minecart => "minecart",
            EntityModelKind::ArmorStand => "armor_stand",
            EntityModelKind::Capsule => "capsule",
        }
    }
}

/// Map a wire entity-type name (`minecraft:<x>`) to a model kind. Players
/// are matched by type id upstream, not by this table. The `name()` of
/// almost every kind IS its wire name, so the table is generated from
/// `ALL`; the handful of aliases follow.
pub fn kind_for_entity_name(name: &str) -> EntityModelKind {
    let Some(short) = name.strip_prefix("minecraft:") else {
        return EntityModelKind::Capsule;
    };
    // M223 — twenty boat types over four models: `<wood>_boat`,
    // `<wood>_chest_boat`, `bamboo_raft`, `bamboo_chest_raft`.
    match short {
        "bamboo_raft" => return EntityModelKind::Raft,
        "bamboo_chest_raft" => return EntityModelKind::ChestRaft,
        s if s.ends_with("_chest_boat") => return EntityModelKind::ChestBoat,
        s if s.ends_with("_boat") => return EntityModelKind::Boat,
        // M252 — the minecart and its six content variants share one model.
        s if s == "minecart" || s.ends_with("_minecart") => return EntityModelKind::Minecart,
        _ => {}
    }
    for k in EntityModelKind::ALL {
        // Player variants are chosen by the caller (type id + skin model),
        // never by the wire name; capsule is the fallback, not a match. The
        // tropical fish's large body plan is the same shape of exception —
        // one wire name, two meshes, selected from the packed variant (M68).
        let picked_elsewhere = matches!(
            k,
            EntityModelKind::Player
                | EntityModelKind::PlayerSlim
                | EntityModelKind::TropicalFishLarge
                | EntityModelKind::SulfurCubeSmall
                | EntityModelKind::HappyGhastBaby
                | EntityModelKind::TurtleBaby
                | EntityModelKind::PolarBearBaby
                | EntityModelKind::ZombieBaby
                | EntityModelKind::HuskBaby
                | EntityModelKind::VillagerBaby
                | EntityModelKind::DrownedBaby
                | EntityModelKind::PiglinBaby
                | EntityModelKind::ZombifiedPiglinBaby
                | EntityModelKind::CowBaby
                | EntityModelKind::MooshroomBaby
                | EntityModelKind::PigBaby
                | EntityModelKind::ChickenBaby
                | EntityModelKind::SheepBaby
                | EntityModelKind::DolphinBaby
                | EntityModelKind::ZombieVillagerBaby
                | EntityModelKind::SquidBaby
                | EntityModelKind::GlowSquidBaby
                | EntityModelKind::HoglinBaby
                | EntityModelKind::ZoglinBaby
                | EntityModelKind::StriderBaby
                | EntityModelKind::FoxBaby
                | EntityModelKind::PandaBaby
                | EntityModelKind::OcelotBaby
                | EntityModelKind::BeeBaby
                | EntityModelKind::DonkeyBaby
                | EntityModelKind::MuleBaby
                | EntityModelKind::CamelBaby
                | EntityModelKind::RabbitBaby
                | EntityModelKind::GoatBaby
                | EntityModelKind::LlamaBaby
                | EntityModelKind::HorseBaby
                | EntityModelKind::CatBaby
                | EntityModelKind::WolfBaby
                | EntityModelKind::ColdCow
                | EntityModelKind::WarmCow
                | EntityModelKind::ColdPig
                | EntityModelKind::ColdChicken
                | EntityModelKind::ZombieNautilusCoral
                | EntityModelKind::Capsule
        );
        if !picked_elsewhere && k.name() == short {
            return k;
        }
    }
    EntityModelKind::Capsule
}

/// One registry entry: texture keys (into the baked mob-texture table) +
/// the model builder. A mob renders as a capsule until every listed texture
/// is present.
pub struct MobDef {
    pub kind: EntityModelKind,
    pub textures: &'static [&'static str],
    pub build: fn() -> Model,
}

// ---------------------------------------------------------------------------
// Dye tint (vanilla `SheepWoolLayer`)
// ---------------------------------------------------------------------------

/// The 16 dye colours as vanilla renders sheep wool, sRGB.
///
/// `SheepWoolLayer` draws the fur model tinted by
/// `ColorLerper.Type.SHEEP.getColor(woolColor)`, which is
/// `DyeColor.getTextureDiffuseColor()` scaled to 0.75 brightness — except
/// white, which the lerper overrides outright to `-1644826` (0xE6E6E6) rather
/// than dimming. Both rules are folded in here, so these are the literal RGB
/// values vanilla multiplies into the wool's vertex colour.
///
/// (The jeb_ rainbow is `getLerpedColor` over this same table on a 25-tick
/// cycle; it keys off the sheep's custom name, which Rewo decodes, but also
/// needs the wool colour.)
pub const SHEEP_WOOL_COLORS: [[u8; 3]; 16] = [
    [230, 230, 230], //  0 white
    [186, 96, 21],   //  1 orange
    [149, 58, 141],  //  2 magenta
    [43, 134, 163],  //  3 light_blue
    [190, 162, 45],  //  4 yellow
    [96, 149, 23],   //  5 lime
    [182, 104, 127], //  6 pink
    [53, 59, 61],    //  7 gray
    [117, 117, 113], //  8 light_gray
    [16, 117, 117],  //  9 cyan
    [102, 37, 138],  // 10 purple
    [45, 51, 127],   // 11 blue
    [98, 63, 37],    // 12 brown
    [70, 93, 16],    // 13 green
    [132, 34, 28],   // 14 red
    [21, 21, 24],    // 15 black
];

/// `DyeColor.getTextureDiffuseColor()`, sRGB — the **undimmed** dye table.
///
/// Not [`SHEEP_WOOL_COLORS`]: that one is `ColorLerper.Type.SHEEP`, which
/// scales this to 0.75 and overrides white. Several entries differ sharply
/// (red is `0xB02E26` here against the wool's `0x84221C`), so tinting a
/// tropical fish from the wool table would be plausibly, visibly wrong — the
/// same trap `rewo_data::block_entity_models::DYE_DIFFUSE_COLORS` records for
/// banners against the sign table. `TropicalFishRenderer.extractRenderState`
/// calls `getTextureDiffuseColor()` on both dyes directly, with no lerper.
pub const DYE_DIFFUSE_COLORS: [[u8; 3]; 16] = [
    [0xF9, 0xFF, 0xFE], //  0 white
    [0xF9, 0x80, 0x1D], //  1 orange
    [0xC7, 0x4E, 0xBD], //  2 magenta
    [0x3A, 0xB3, 0xDA], //  3 light_blue
    [0xFE, 0xD8, 0x3D], //  4 yellow
    [0x80, 0xC7, 0x1F], //  5 lime
    [0xF3, 0x8B, 0xAA], //  6 pink
    [0x47, 0x4F, 0x52], //  7 gray
    [0x9D, 0x9D, 0x97], //  8 light_gray
    [0x16, 0x9C, 0x9C], //  9 cyan
    [0x89, 0x32, 0xB8], // 10 purple
    [0x3C, 0x44, 0xAA], // 11 blue
    [0x83, 0x54, 0x32], // 12 brown
    [0x5E, 0x7C, 0x16], // 13 green
    [0xB0, 0x2E, 0x26], // 14 red
    [0x1D, 0x1D, 0x21], // 15 black
];

/// Which of a mob's textures vanilla renders through a dye tint. Only the
/// sheep's wool today; wolf and cat collars and llama carpets are the same
/// shape at other metadata indices.
pub fn tinted_texture(kind: EntityModelKind) -> Option<&'static str> {
    match kind {
        EntityModelKind::Sheep => Some("sheep_wool"),
        // M323 — the lamb's fleece, `SheepWoolLayer` again.
        EntityModelKind::SheepBaby => Some("sheep_wool_baby"),
        _ => None,
    }
}

/// Which of a mob's textures is a **coplanar** render layer — one whose mesh
/// is the base model's own, so its quads land at exactly the base's depth
/// (M68).
///
/// Only `SheepWoolUndercoatLayer`. It is the reason this is a table at all:
/// every other layer Rewo bakes as a texture slot is *inflated* over the body
/// (the fleece 0.6/1.75/0.5, the fish pattern 0.008) and so wins the solid
/// pass's strict `GREATER` depth test on its own. The undercoat's model layer
/// is `SheepModel.createBodyLayer()` at `CubeDeformation.NONE`, so under
/// `GREATER` it would be rejected fragment-for-fragment and draw *nothing*.
/// The entity pass routes these quads into the same `CompareOp::EQUAL`,
/// no-depth-write range M48's armour trim uses, which is the reversed-Z
/// reading of vanilla's `entityCutout` (`LEQUAL` + write) for geometry that is
/// coplanar by construction: at equal depth both pass, and the write is a
/// no-op because the value already there is the one it would write.
///
/// That the range is drawn *after* the solid one is what makes the fleece
/// still occlude it: an unshorn sheep's depth buffer holds the *inflated*
/// wool's nearer depth, which the undercoat then fails.
pub fn coplanar_layer_texture(kind: EntityModelKind) -> Option<&'static str> {
    match kind {
        // M342 — `WolfCollarLayer` draws the wolf's own model again.
        EntityModelKind::Wolf => Some("wolf_collar"),
        EntityModelKind::WolfBaby => Some("wolf_collar_baby"),
        EntityModelKind::Sheep => Some("sheep_wool_undercoat"),
        // M323 — and the lamb's fleece: `LayerDefinitions` maps
        // `SHEEP_BABY_WOOL` to `sheepBabyBodyLayer`, the lamb's own mesh at
        // `CubeDeformation.NONE`. It is `SheepWoolLayer`'s, so shearing gates
        // it and the undercoat's dye rule does not (`emit_model`).
        EntityModelKind::SheepBaby => Some("sheep_wool_baby"),
        _ => None,
    }
}

/// The two texture slots a tropical fish's packed variant tints (M68):
/// `(body, pattern)`.
///
/// A separate table from [`tinted_texture`] for the reason that one already
/// gives for [`shearable_texture`] — these are independent facts about
/// different lines of different renderers, and they do not even share a colour
/// table: the body is `MobRenderer.getModelTint` overridden to `state
/// .baseColor` and the pattern is `TropicalFishPatternLayer`'s own colour
/// argument, both [`DYE_DIFFUSE_COLORS`], where the sheep's is the lerper's.
pub fn fish_tint_textures(kind: EntityModelKind) -> Option<(&'static str, &'static str)> {
    match kind {
        EntityModelKind::TropicalFish => Some(("tropical_fish", "tropical_fish_pattern_a")),
        EntityModelKind::TropicalFishLarge => {
            Some(("tropical_fish_large", "tropical_fish_pattern_b"))
        }
        _ => None,
    }
}

/// Which of a mob's textures belongs to a render layer a **shorn** mob skips
/// entirely (M64).
///
/// `SheepWoolLayer.submit` opens `if (!state.isSheared)`, so shearing does not
/// recolour or hide the fleece — the fur model is never submitted. Rewo bakes
/// `SheepFurModel`'s inflated boxes into the sheep model as its second texture
/// slot, so "do not submit that layer" is "drop the quads that sample it".
///
/// It is deliberately a second table rather than a reuse of
/// [`tinted_texture`], even though both answer `sheep_wool` for the one mob
/// that has either: they are two independent facts about two different lines
/// of `SheepWoolLayer`, and a wolf's dyed collar will be tinted-but-not-
/// shearable the moment it exists.
///
/// It is also **not** [`coplanar_layer_texture`]: `SheepWoolUndercoatLayer`
/// carries no `isSheared` test at all (M68), so shearing takes this layer and
/// leaves that one — which is the whole reason a shorn dyed sheep still shows
/// colour where a shorn white one is bare.
/// M342 — which of a mob's textures is a collar: drawn only on a tame mob
/// and tinted by its collar's `DyeColor.getTextureDiffuseColor()`.
pub fn collar_texture(kind: EntityModelKind) -> Option<&'static str> {
    match kind {
        // M343 — `CatCollarLayer`: an inflated copy, so not coplanar.
        EntityModelKind::Cat => Some("cat_collar"),
        EntityModelKind::CatBaby => Some("cat_collar_baby"),
        EntityModelKind::Wolf => Some("wolf_collar"),
        EntityModelKind::WolfBaby => Some("wolf_collar_baby"),
        _ => None,
    }
}

pub fn shearable_texture(kind: EntityModelKind) -> Option<&'static str> {
    match kind {
        EntityModelKind::Sheep => Some("sheep_wool"),
        EntityModelKind::SheepBaby => Some("sheep_wool_baby"),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Emissive layers (vanilla `EyesLayer` / `LivingEntityEmissiveLayer`)
// ---------------------------------------------------------------------------

/// How a mob's model parts are filtered into an emissive layer — vanilla's two
/// `MeshDefinition` retain modes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PartFilter {
    /// The whole parent model (`EyesLayer::getParentModel`).
    All,
    /// `retainExactParts(names)` — only these parts, children dropped.
    Exact(&'static [&'static str]),
}

/// The layer's per-frame alpha — vanilla's `AlphaFunction`. `age` is
/// `ageInTicks`; the entity-state inputs come from
/// [`crate::entities::EmissiveState`].
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum EmissiveAlpha {
    /// `(state, age) -> 1.0` — the eye layers and the warden's bioluminescent
    /// skin.
    Always,
    /// `max(0, cos(age·0.045 + phase)·0.25)` — the warden's two pulsating spot
    /// layers, half a period apart (phase 0 and π).
    PulsatingSpots { phase: f32 },
    /// `Warden.tendrilAnimation` — a 10-tick countdown started by entity_event
    /// 61, so the tendrils only glow just after the warden hears a vibration.
    Tendril,
    /// `Warden.heartAnimation` — a 10-tick countdown restarted every
    /// `40 − floor(clamp(anger,0,1)·30)` ticks. Fully client-side.
    Heart,
    /// `Creaking.isActive()` ? 1 : 0 (metadata `IS_ACTIVE`, vanilla default
    /// false).
    EyesGlowing,
}

/// One vanilla emissive layer: a filtered copy of the mob's own geometry
/// re-rendered with an overlay texture at full brightness.
///
/// Both vanilla render types this covers (`RenderTypes.eyes` and
/// `entityTranslucentEmissive`) are, per `RenderPipelines`, translucent blend +
/// `CompareOp.GREATER_THAN_OR_EQUAL` + **depth-write off**; they differ only in
/// `ALPHA_CUTOUT` (0.1 on emissive, none on eyes), which
/// [`cutout`](Self::cutout) carries, and in cardinal lighting — `eyes` defines
/// `NO_CARDINAL_LIGHTING` (flat), emissive defines `PER_FACE_LIGHTING`
/// (directionally shaded).
pub struct EmissiveLayer {
    /// Texture key in the baked mob-texture table. Must have the same pixel
    /// dimensions as the mob's base texture — the layer re-uses the base quads'
    /// UVs verbatim, exactly as vanilla re-renders the same model.
    pub tex: &'static str,
    pub parts: PartFilter,
    pub alpha: EmissiveAlpha,
    /// Vanilla's `ALPHA_CUTOUT` 0.1 (`entityTranslucentEmissive`); the `eyes`
    /// pipeline has none.
    pub cutout: bool,
}

const CUTOUT: bool = true;
const NO_CUTOUT: bool = false;

/// The emissive layers a mob kind renders, transcribed one-for-one from its
/// `EntityRenderer`'s `addLayer` calls in the 26.2 decompile. Kinds with no
/// emissive layer return `&[]` — which is most of them.
///
/// Kept as a function rather than a [`MobDef`] field so the 89-entry `MOBS`
/// table stays untouched.
pub fn emissive_layers(kind: EntityModelKind) -> &'static [EmissiveLayer] {
    use EmissiveAlpha::*;
    use EntityModelKind as K;
    // `SpiderRenderer`/`EndermanRenderer`/`PhantomRenderer` add an `EyesLayer`,
    // which re-renders the *whole* parent model.
    const SPIDER: &[EmissiveLayer] = &[EmissiveLayer {
        tex: "spider_eyes",
        parts: PartFilter::All,
        alpha: Always,
        cutout: NO_CUTOUT,
    }];
    const ENDERMAN: &[EmissiveLayer] = &[EmissiveLayer {
        tex: "enderman_eyes",
        parts: PartFilter::All,
        alpha: Always,
        cutout: NO_CUTOUT,
    }];
    const PHANTOM: &[EmissiveLayer] = &[EmissiveLayer {
        tex: "phantom_eyes",
        parts: PartFilter::All,
        alpha: Always,
        cutout: NO_CUTOUT,
    }];
    // `BreezeEyesLayer` bakes `BreezeModel.createEyesLayer()` —
    // `retainPartsAndChildren({"eyes"})`, and vanilla's `eyes` part is an exact
    // duplicate of `head`'s two boxes at a zero offset, with no children. Our
    // model folds that duplicate away, so filtering to `head` is the same
    // geometry.
    const BREEZE: &[EmissiveLayer] = &[EmissiveLayer {
        tex: "breeze_eyes",
        parts: PartFilter::Exact(&["head"]),
        alpha: Always,
        cutout: NO_CUTOUT,
    }];
    // `CreakingModel.createEyesLayer()` = `retainExactParts({"head"})`.
    const CREAKING: &[EmissiveLayer] = &[EmissiveLayer {
        tex: "creaking_eyes",
        parts: PartFilter::Exact(&["head"]),
        alpha: EyesGlowing,
        cutout: NO_CUTOUT,
    }];
    // `CopperGolemRenderer` re-renders the whole model with the eye texture for
    // its weathering stage; we bake the un-weathered stage.
    const COPPER_GOLEM: &[EmissiveLayer] = &[EmissiveLayer {
        tex: "copper_golem_eyes",
        parts: PartFilter::All,
        alpha: Always,
        cutout: NO_CUTOUT,
    }];
    // `WardenRenderer`, in its `addLayer` order. The tendril layer samples the
    // *base* warden texture — the tendrils light up their own texels.
    const WARDEN: &[EmissiveLayer] = &[
        EmissiveLayer {
            tex: "warden_bioluminescent",
            parts: PartFilter::Exact(&["head", "left_arm", "right_arm", "left_leg", "right_leg"]),
            alpha: Always,
            cutout: CUTOUT,
        },
        EmissiveLayer {
            tex: "warden_pulsating_1",
            parts: PartFilter::Exact(&[
                "body",
                "head",
                "left_arm",
                "right_arm",
                "left_leg",
                "right_leg",
            ]),
            alpha: PulsatingSpots { phase: 0.0 },
            cutout: CUTOUT,
        },
        EmissiveLayer {
            tex: "warden_pulsating_2",
            parts: PartFilter::Exact(&[
                "body",
                "head",
                "left_arm",
                "right_arm",
                "left_leg",
                "right_leg",
            ]),
            alpha: PulsatingSpots {
                phase: std::f32::consts::PI,
            },
            cutout: CUTOUT,
        },
        EmissiveLayer {
            tex: "warden",
            parts: PartFilter::Exact(&["left_tendril", "right_tendril"]),
            alpha: Tendril,
            cutout: CUTOUT,
        },
        EmissiveLayer {
            tex: "warden_heart",
            parts: PartFilter::Exact(&["body"]),
            alpha: Heart,
            cutout: CUTOUT,
        },
    ];
    match kind {
        K::Spider | K::CaveSpider => SPIDER,
        K::Enderman => ENDERMAN,
        K::Phantom => PHANTOM,
        K::Breeze => BREEZE,
        K::Creaking => CREAKING,
        K::CopperGolem => COPPER_GOLEM,
        K::Warden => WARDEN,
        _ => &[],
    }
}

pub static MOBS: &[MobDef] = &[
    MobDef {
        kind: EntityModelKind::Player,
        textures: &["player"],
        build: player,
    },
    MobDef {
        kind: EntityModelKind::PlayerSlim,
        textures: &["player"],
        build: player_slim,
    },
    MobDef {
        kind: EntityModelKind::Zombie,
        textures: &["zombie"],
        build: zombie,
    },
    MobDef {
        kind: EntityModelKind::ZombieVillager,
        textures: &["zombie_villager"],
        build: zombie_villager,
    },
    MobDef {
        kind: EntityModelKind::Husk,
        textures: &["husk"],
        build: husk,
    },
    MobDef {
        kind: EntityModelKind::Drowned,
        textures: &["drowned", "drowned_outer"],
        build: drowned,
    },
    MobDef {
        kind: EntityModelKind::Skeleton,
        textures: &["skeleton"],
        build: skeleton,
    },
    MobDef {
        kind: EntityModelKind::Stray,
        textures: &["stray", "stray_overlay"],
        build: stray,
    },
    MobDef {
        kind: EntityModelKind::Bogged,
        textures: &["bogged", "bogged_overlay"],
        build: bogged,
    },
    MobDef {
        kind: EntityModelKind::Parched,
        textures: &["parched"],
        build: parched,
    },
    MobDef {
        kind: EntityModelKind::WitherSkeleton,
        textures: &["wither_skeleton"],
        build: wither_skeleton,
    },
    MobDef {
        kind: EntityModelKind::Creeper,
        textures: &["creeper"],
        build: creeper,
    },
    MobDef {
        kind: EntityModelKind::Spider,
        textures: &["spider"],
        build: spider,
    },
    MobDef {
        kind: EntityModelKind::CaveSpider,
        textures: &["cave_spider"],
        build: cave_spider,
    },
    MobDef {
        kind: EntityModelKind::Enderman,
        textures: &["enderman"],
        build: enderman,
    },
    MobDef {
        kind: EntityModelKind::Slime,
        textures: &["slime"],
        build: slime,
    },
    MobDef {
        kind: EntityModelKind::MagmaCube,
        textures: &["magma_cube"],
        build: magma_cube,
    },
    MobDef {
        kind: EntityModelKind::Cow,
        textures: &["cow"],
        build: cow,
    },
    MobDef {
        kind: EntityModelKind::Mooshroom,
        textures: &["mooshroom"],
        build: cow,
    },
    MobDef {
        kind: EntityModelKind::Pig,
        textures: &["pig"],
        build: pig,
    },
    MobDef {
        kind: EntityModelKind::Sheep,
        textures: &["sheep", "sheep_wool", "sheep_wool_undercoat"],
        build: sheep,
    },
    MobDef {
        kind: EntityModelKind::Chicken,
        textures: &["chicken"],
        build: chicken,
    },
    MobDef {
        kind: EntityModelKind::Wolf,
        // M342 — and the collar, `WolfCollarLayer`'s sheet on the wolf's own
        // mesh (see `wolf`).
        textures: &["wolf", "wolf_collar"],
        build: wolf,
    },
    MobDef {
        kind: EntityModelKind::Squid,
        textures: &["squid"],
        build: squid,
    },
    MobDef {
        kind: EntityModelKind::GlowSquid,
        textures: &["glow_squid"],
        build: squid,
    },
    MobDef {
        kind: EntityModelKind::Rabbit,
        textures: &["rabbit"],
        build: rabbit,
    },
    MobDef {
        kind: EntityModelKind::Villager,
        textures: &["villager"],
        build: villager,
    },
    MobDef {
        kind: EntityModelKind::WanderingTrader,
        textures: &["wandering_trader"],
        build: villager,
    },
    MobDef {
        kind: EntityModelKind::Witch,
        textures: &["witch"],
        build: witch,
    },
    MobDef {
        kind: EntityModelKind::Pillager,
        textures: &["pillager"],
        build: illager_arms,
    },
    MobDef {
        kind: EntityModelKind::Vindicator,
        textures: &["vindicator"],
        build: illager_crossed,
    },
    MobDef {
        kind: EntityModelKind::Evoker,
        textures: &["evoker"],
        build: illager_crossed,
    },
    MobDef {
        kind: EntityModelKind::Illusioner,
        textures: &["illusioner"],
        build: illager_crossed,
    },
    MobDef {
        kind: EntityModelKind::Vex,
        textures: &["vex"],
        build: vex,
    },
    MobDef {
        kind: EntityModelKind::Phantom,
        textures: &["phantom"],
        build: phantom,
    },
    MobDef {
        kind: EntityModelKind::Guardian,
        textures: &["guardian"],
        build: guardian,
    },
    MobDef {
        kind: EntityModelKind::ElderGuardian,
        textures: &["elder_guardian"],
        build: elder_guardian,
    },
    MobDef {
        kind: EntityModelKind::Shulker,
        textures: &["shulker"],
        build: shulker,
    },
    MobDef {
        kind: EntityModelKind::Silverfish,
        textures: &["silverfish"],
        build: silverfish,
    },
    MobDef {
        kind: EntityModelKind::Endermite,
        textures: &["endermite"],
        build: endermite,
    },
    MobDef {
        kind: EntityModelKind::Blaze,
        textures: &["blaze"],
        build: blaze,
    },
    MobDef {
        kind: EntityModelKind::Ghast,
        textures: &["ghast"],
        build: ghast,
    },
    MobDef {
        kind: EntityModelKind::Piglin,
        textures: &["piglin"],
        build: piglin_normal,
    },
    MobDef {
        kind: EntityModelKind::PiglinBrute,
        textures: &["piglin_brute"],
        build: piglin_normal,
    },
    MobDef {
        kind: EntityModelKind::ZombifiedPiglin,
        textures: &["zombified_piglin"],
        build: piglin_zombified,
    },
    MobDef {
        kind: EntityModelKind::Hoglin,
        textures: &["hoglin"],
        build: hoglin,
    },
    MobDef {
        kind: EntityModelKind::Zoglin,
        textures: &["zoglin"],
        build: hoglin,
    },
    MobDef {
        kind: EntityModelKind::Strider,
        textures: &["strider"],
        build: strider,
    },
    MobDef {
        kind: EntityModelKind::Bat,
        textures: &["bat"],
        build: bat,
    },
    MobDef {
        kind: EntityModelKind::Cat,
        // M343 — and the collar (`CatCollarLayer`, inflated, see `cat`).
        textures: &["cat", "cat_collar"],
        build: cat,
    },
    MobDef {
        kind: EntityModelKind::Ocelot,
        textures: &["ocelot"],
        build: feline,
    },
    MobDef {
        kind: EntityModelKind::Fox,
        textures: &["fox"],
        build: fox,
    },
    MobDef {
        kind: EntityModelKind::Goat,
        textures: &["goat"],
        build: goat,
    },
    MobDef {
        kind: EntityModelKind::Bee,
        textures: &["bee"],
        build: bee,
    },
    MobDef {
        kind: EntityModelKind::Frog,
        textures: &["frog"],
        build: frog,
    },
    MobDef {
        kind: EntityModelKind::Tadpole,
        textures: &["tadpole"],
        build: tadpole,
    },
    MobDef {
        kind: EntityModelKind::Armadillo,
        textures: &["armadillo"],
        build: armadillo,
    },
    MobDef {
        kind: EntityModelKind::Axolotl,
        textures: &["axolotl"],
        build: axolotl,
    },
    MobDef {
        kind: EntityModelKind::Dolphin,
        textures: &["dolphin"],
        build: dolphin,
    },
    MobDef {
        kind: EntityModelKind::Turtle,
        textures: &["turtle"],
        build: turtle,
    },
    MobDef {
        kind: EntityModelKind::Cod,
        textures: &["cod"],
        build: cod,
    },
    MobDef {
        kind: EntityModelKind::Salmon,
        textures: &["salmon"],
        build: salmon,
    },
    MobDef {
        kind: EntityModelKind::Pufferfish,
        textures: &["pufferfish"],
        build: pufferfish,
    },
    MobDef {
        kind: EntityModelKind::TropicalFish,
        textures: &["tropical_fish", "tropical_fish_pattern_a"],
        build: tropical_fish,
    },
    MobDef {
        kind: EntityModelKind::TropicalFishLarge,
        textures: &["tropical_fish_large", "tropical_fish_pattern_b"],
        build: tropical_fish_large,
    },
    MobDef {
        kind: EntityModelKind::Panda,
        textures: &["panda"],
        build: panda,
    },
    MobDef {
        kind: EntityModelKind::PolarBear,
        textures: &["polar_bear"],
        build: polar_bear,
    },
    MobDef {
        kind: EntityModelKind::Camel,
        textures: &["camel"],
        build: camel,
    },
    MobDef {
        kind: EntityModelKind::Llama,
        textures: &["llama"],
        build: llama,
    },
    MobDef {
        kind: EntityModelKind::TraderLlama,
        textures: &["llama"],
        build: llama,
    },
    MobDef {
        kind: EntityModelKind::Parrot,
        textures: &["parrot"],
        build: parrot,
    },
    MobDef {
        kind: EntityModelKind::Horse,
        textures: &["horse"],
        build: horse,
    },
    MobDef {
        kind: EntityModelKind::Donkey,
        textures: &["donkey"],
        build: donkey,
    },
    MobDef {
        kind: EntityModelKind::Mule,
        textures: &["mule"],
        build: mule,
    },
    MobDef {
        kind: EntityModelKind::SkeletonHorse,
        textures: &["skeleton_horse"],
        build: equine_plain,
    },
    MobDef {
        kind: EntityModelKind::ZombieHorse,
        textures: &["zombie_horse"],
        build: equine_plain,
    },
    // M223 — the boats: oak and bamboo are the baked bases, the other woods
    // variant sheets (`rewo_data::mob_variants`).
    MobDef {
        kind: EntityModelKind::Boat,
        textures: &["boat"],
        build: boat,
    },
    MobDef {
        kind: EntityModelKind::ChestBoat,
        textures: &["chest_boat"],
        build: chest_boat,
    },
    MobDef {
        kind: EntityModelKind::Raft,
        textures: &["raft"],
        build: raft,
    },
    // M252 — the minecart.
    MobDef {
        kind: EntityModelKind::Minecart,
        textures: &["minecart"],
        build: minecart,
    },
    MobDef {
        kind: EntityModelKind::ChestRaft,
        textures: &["chest_raft"],
        build: chest_raft,
    },
    MobDef {
        kind: EntityModelKind::ArmorStand,
        textures: &["armor_stand"],
        build: armor_stand,
    },
    MobDef {
        kind: EntityModelKind::SnowGolem,
        textures: &["snow_golem"],
        build: snow_golem,
    },
    MobDef {
        kind: EntityModelKind::IronGolem,
        textures: &["iron_golem"],
        build: iron_golem,
    },
    MobDef {
        kind: EntityModelKind::Allay,
        textures: &["allay"],
        build: allay,
    },
    MobDef {
        kind: EntityModelKind::Warden,
        textures: &["warden"],
        build: warden,
    },
    MobDef {
        kind: EntityModelKind::Sniffer,
        textures: &["sniffer"],
        build: sniffer,
    },
    MobDef {
        kind: EntityModelKind::Breeze,
        textures: &["breeze", "breeze_wind"],
        build: breeze,
    },
    MobDef {
        kind: EntityModelKind::Creaking,
        textures: &["creaking"],
        build: creaking,
    },
    MobDef {
        kind: EntityModelKind::Ravager,
        textures: &["ravager"],
        build: ravager,
    },
    MobDef {
        kind: EntityModelKind::Wither,
        textures: &["wither"],
        build: wither,
    },
    MobDef {
        kind: EntityModelKind::EnderDragon,
        textures: &["ender_dragon"],
        build: ender_dragon,
    },
    MobDef {
        kind: EntityModelKind::HappyGhast,
        // M312 — and `HappyGhastRenderer.GHAST_ROPES`.
        textures: &["happy_ghast", "happy_ghast_ropes"],
        build: happy_ghast,
    },
    MobDef {
        kind: EntityModelKind::CopperGolem,
        textures: &["copper_golem"],
        build: copper_golem,
    },
    MobDef {
        kind: EntityModelKind::Nautilus,
        textures: &["nautilus"],
        build: nautilus,
    },
    MobDef {
        kind: EntityModelKind::ZombieNautilus,
        textures: &["zombie_nautilus"],
        build: nautilus,
    },
    // M303 — the camel's mesh (walk rig included) on the husk's sheet.
    MobDef {
        kind: EntityModelKind::CamelHusk,
        textures: &["camel_husk"],
        build: camel,
    },
    // M304 — `GiantMobRenderer.ZOMBIE_LOCATION`.
    MobDef {
        kind: EntityModelKind::Giant,
        textures: &["zombie"],
        build: giant,
    },
    // M307 — the outer sheet (the renderer's own texture) and the inner
    // (`SulfurCubeInnerLayer`'s), adult and small.
    MobDef {
        kind: EntityModelKind::SulfurCube,
        textures: &["sulfur_cube_outer", "sulfur_cube_inner"],
        build: sulfur_cube,
    },
    MobDef {
        kind: EntityModelKind::SulfurCubeSmall,
        textures: &["sulfur_cube_outer_small", "sulfur_cube_inner_small"],
        build: sulfur_cube_small,
    },
    // M314 — `HappyGhastRenderer.GHAST_BABY_LOCATION`.
    MobDef {
        kind: EntityModelKind::HappyGhastBaby,
        textures: &["happy_ghast_baby"],
        build: happy_ghast_baby,
    },
    // M315 — `TurtleRenderer.BABY_TURTLE_LOCATION`.
    MobDef {
        kind: EntityModelKind::TurtleBaby,
        textures: &["turtle_baby"],
        build: turtle_baby,
    },
    // M316 — `PolarBearRenderer`'s baby sheet.
    MobDef {
        kind: EntityModelKind::PolarBearBaby,
        textures: &["polar_bear_baby"],
        build: polar_bear_baby,
    },
    // M317 — the baby zombie's and husk's sheets (baked by their whole-sheet
    // swaps, which until M317 painted them onto the ADULT mesh, whose UV
    // layout they do not share).
    MobDef {
        kind: EntityModelKind::ZombieBaby,
        textures: &["zombie_baby"],
        build: zombie_baby,
    },
    MobDef {
        kind: EntityModelKind::HuskBaby,
        textures: &["husk_baby"],
        build: husk_baby,
    },
    // M318 — the baby villager's sheet (`VillagerRenderer`).
    MobDef {
        kind: EntityModelKind::VillagerBaby,
        textures: &["villager_baby"],
        build: villager_baby,
    },
    // M319 — `DrownedRenderer.BABY_DROWNED_LOCATION` and
    // `DrownedOuterLayer.BABY_DROWNED_OUTER_LAYER_LOCATION`.
    MobDef {
        kind: EntityModelKind::DrownedBaby,
        textures: &["drowned_baby", "drowned_outer_baby"],
        build: drowned_baby,
    },
    // M320 — `PiglinRenderer.PIGLIN_BABY_LOCATION` and
    // `ZombifiedPiglinRenderer.BABY_ZOMBIFIED_PIGLIN_LOCATION`.
    MobDef {
        kind: EntityModelKind::PiglinBaby,
        textures: &["piglin_baby"],
        build: piglin_baby,
    },
    MobDef {
        kind: EntityModelKind::ZombifiedPiglinBaby,
        textures: &["zombified_piglin_baby"],
        build: zombified_piglin_baby,
    },
    // M321 — the calves' sheets, the temperate cow's and the red mooshroom's
    // (the adults' own: Rewo draws no other cow variant).
    MobDef {
        kind: EntityModelKind::CowBaby,
        textures: &["cow_baby"],
        build: cow_baby,
    },
    MobDef {
        kind: EntityModelKind::MooshroomBaby,
        textures: &["mooshroom_baby"],
        build: cow_baby,
    },
    // M322 — the temperate piglet's and chick's sheets (Rewo draws no other
    // pig or chicken variant).
    MobDef {
        kind: EntityModelKind::PigBaby,
        textures: &["pig_baby"],
        build: pig_baby,
    },
    MobDef {
        kind: EntityModelKind::ChickenBaby,
        textures: &["chicken_baby"],
        build: chicken_baby,
    },
    // M323 — the lamb and its fleece (`SheepWoolLayer`'s baby sheet; the
    // undercoat layer skips a baby).
    MobDef {
        kind: EntityModelKind::SheepBaby,
        textures: &["sheep_baby", "sheep_wool_baby"],
        build: sheep_baby,
    },
    // M324 — the dolphin calf's sheet.
    MobDef {
        kind: EntityModelKind::DolphinBaby,
        textures: &["dolphin_baby"],
        build: dolphin_baby,
    },
    // M325 — the baby zombie villager's sheet.
    MobDef {
        kind: EntityModelKind::ZombieVillagerBaby,
        textures: &["zombie_villager_baby"],
        build: zombie_villager_baby,
    },
    // M326 — the baby squids' sheets.
    MobDef {
        kind: EntityModelKind::SquidBaby,
        textures: &["squid_baby"],
        build: squid_baby,
    },
    MobDef {
        kind: EntityModelKind::GlowSquidBaby,
        textures: &["glow_squid_baby"],
        build: squid_baby,
    },
    // M327 — the hoglet's and the baby zoglin's sheets.
    MobDef {
        kind: EntityModelKind::HoglinBaby,
        textures: &["hoglin_baby"],
        build: hoglin_baby,
    },
    MobDef {
        kind: EntityModelKind::ZoglinBaby,
        textures: &["zoglin_baby"],
        build: hoglin_baby,
    },
    // M328 — the baby strider's warm sheet (Rewo draws no cold strider).
    MobDef {
        kind: EntityModelKind::StriderBaby,
        textures: &["strider_baby"],
        build: strider_baby,
    },
    // M330 — the kit's red sheet (Rewo's adult fox draws no snow variant).
    MobDef {
        kind: EntityModelKind::FoxBaby,
        textures: &["fox_baby"],
        build: fox_baby,
    },
    // M331 — the cub's normal-gene sheet (Rewo's adult panda draws no
    // other gene).
    MobDef {
        kind: EntityModelKind::PandaBaby,
        textures: &["panda_baby"],
        build: panda_baby,
    },
    // M332 — the ocelot kitten's sheet.
    MobDef {
        kind: EntityModelKind::OcelotBaby,
        textures: &["ocelot_baby"],
        build: feline_baby,
    },
    // M333 — the baby bee's calm sheet (Rewo's adult bee draws no angry or
    // nectar sheet either).
    MobDef {
        kind: EntityModelKind::BeeBaby,
        textures: &["bee_baby"],
        build: bee_baby,
    },
    // M334 — the foals' sheets.
    MobDef {
        kind: EntityModelKind::DonkeyBaby,
        textures: &["donkey_baby"],
        build: donkey_baby,
    },
    MobDef {
        kind: EntityModelKind::MuleBaby,
        textures: &["mule_baby"],
        build: donkey_baby,
    },
    // M335 — the camel calf's sheet.
    MobDef {
        kind: EntityModelKind::CamelBaby,
        textures: &["camel_baby"],
        build: camel_baby,
    },
    // M336 — the brown kit's sheet (Rewo's adult rabbit draws the brown
    // coat only).
    MobDef {
        kind: EntityModelKind::RabbitBaby,
        textures: &["rabbit_baby"],
        build: rabbit_baby,
    },
    // M337 — the kid's sheet (one of M175's scrambled swaps).
    MobDef {
        kind: EntityModelKind::GoatBaby,
        textures: &["goat_baby"],
        build: goat_baby,
    },
    // M338 — the creamy kit's sheet; the other three coats are its variants
    // (`mob_variants::llama_baby_texture`).
    MobDef {
        kind: EntityModelKind::LlamaBaby,
        textures: &["llama_baby"],
        build: llama_baby,
    },
    // M339 — the brown foal's sheet; the other six coats are its variants
    // (`mob_variants::horse_baby_texture`).
    MobDef {
        kind: EntityModelKind::HorseBaby,
        textures: &["horse_baby"],
        build: horse_baby,
    },
    // M340 — the tabby kitten's sheet; the registry's other coats are its
    // variants, through `MobVariantDef::baby_texture`. The collar is not
    // drawn, as on Rewo's adult cat.
    MobDef {
        kind: EntityModelKind::CatBaby,
        textures: &["cat_baby", "cat_collar_baby"],
        build: cat_baby,
    },
    // M341 — the pale wild puppy's sheet; the registry's other coats, wild
    // and tame, are its variants (`MobVariantDef::baby_texture`).
    MobDef {
        kind: EntityModelKind::WolfBaby,
        textures: &["wolf_baby", "wolf_collar_baby"],
        build: wolf_baby,
    },
    // M348 — the cold and warm cows' own meshes, each on its own sheet.
    MobDef {
        kind: EntityModelKind::ColdCow,
        textures: &["cold_cow"],
        build: cold_cow,
    },
    MobDef {
        kind: EntityModelKind::WarmCow,
        textures: &["warm_cow"],
        build: warm_cow,
    },
    // M349 — the cold pig's and the cold chicken's own meshes.
    MobDef {
        kind: EntityModelKind::ColdPig,
        textures: &["cold_pig"],
        build: cold_pig,
    },
    MobDef {
        kind: EntityModelKind::ColdChicken,
        textures: &["cold_chicken"],
        build: cold_chicken,
    },
    // M350 — the coral zombie nautilus's mesh, on its own sheet.
    MobDef {
        kind: EntityModelKind::ZombieNautilusCoral,
        textures: &["zombie_nautilus_coral"],
        build: nautilus_coral,
    },
];

// ---------------------------------------------------------------------------
// Humanoids — `HumanoidModel.createMesh` (+ PlayerModel / SkeletonModel /
// EndermanModel variants)
// ---------------------------------------------------------------------------

use std::f32::consts::{FRAC_PI_2, PI};

const NONE: &[Fold] = &[];

/// `HumanoidModel.createMesh(g, 0)` head + hat. Returns the head part index.
/// Split out of [`humanoid_head_body`] so the player can attach its body cube
/// to a real articulated `body` part instead of the static root.
fn humanoid_head(b: &mut ModelBuilder, tex: usize) -> usize {
    let head = b.part([0.0, 0.0, 0.0], Anim::Head, 1.0);
    b.cube(
        head,
        tex,
        (0.0, 0.0),
        [-4.0, -8.0, -4.0],
        [8.0, 8.0, 8.0],
        NONE,
    );
    b.cube_g(
        head,
        tex,
        (32.0, 0.0),
        [-4.0, -8.0, -4.0],
        [8.0, 8.0, 8.0],
        0.5,
        NONE,
    );
    head
}

/// `HumanoidModel.createMesh(g, 0)` head + hat + body; limbs differ per
/// variant so callers add their own. Returns the head part index.
///
/// The body cube rides the static root here: vanilla's `body` part is at
/// `PartPose.offset(0, 0, 0)` with no base rotation, and the mobs using this
/// helper never animate it (their vanilla models override `setupAnim` in ways
/// Rewo does not model — see the M19 scope note in REWO_PLAN §15).
fn humanoid_head_body(b: &mut ModelBuilder, tex: usize) -> usize {
    let head = humanoid_head(b, tex);
    b.cube(
        STATIC_PART,
        tex,
        (16.0, 16.0),
        [-4.0, 0.0, -2.0],
        [8.0, 12.0, 4.0],
        NONE,
    );
    head
}

/// The wide ("Steve", 4-px arms) player model.
fn player() -> Model {
    player_model(false)
}

/// The slim ("Alex", 3-px arms) player model — same skin layout, narrower
/// arm boxes (vanilla `PlayerModel.createMesh(_, slim=true)`).
fn player_slim() -> Model {
    player_model(true)
}

/// Shared `PlayerModel.createMesh` — head/hat/body/jacket/legs are identical
/// between the two arm widths; only the arm + sleeve boxes differ.
/// M310 — `PlayerModelPart`'s mask bits (`1 << ordinal`); bit 0 is the cape.
pub const AVATAR_JACKET: u8 = 1 << 1;
pub const AVATAR_LEFT_SLEEVE: u8 = 1 << 2;
pub const AVATAR_RIGHT_SLEEVE: u8 = 1 << 3;
pub const AVATAR_LEFT_PANTS: u8 = 1 << 4;
pub const AVATAR_RIGHT_PANTS: u8 = 1 << 5;
pub const AVATAR_HAT: u8 = 1 << 6;

/// M310 — an outer-layer part: a child of `parent` at its pivot (so the same
/// transform), shown by the customisation mask's `bit`, as `PlayerModel`'s
/// `hat`, `jacket`, sleeves and trouser legs are.
fn avatar_layer(b: &mut ModelBuilder, name: &'static str, parent: usize, bit: u8) -> usize {
    let p = b.part_named(name, [0.0; 3], [0.0; 3], Anim::None, 1.0, Some(parent));
    b.show(p, Show::AvatarPart(bit));
    p
}

fn player_model(slim: bool) -> Model {
    let mut b = ModelBuilder::new();
    // `humanoid_head`, with the hat on its own part (M310).
    let head = b.part([0.0, 0.0, 0.0], Anim::Head, 1.0);
    b.cube(
        head,
        0,
        (0.0, 0.0),
        [-4.0, -8.0, -4.0],
        [8.0, 8.0, 8.0],
        NONE,
    );
    let hat = avatar_layer(&mut b, "hat", head, AVATAR_HAT);
    b.cube_g(
        hat,
        0,
        (32.0, 0.0),
        [-4.0, -8.0, -4.0],
        [8.0, 8.0, 8.0],
        0.5,
        NONE,
    );
    // `body` is a real articulated part (pivot 0,0,0 / no base rotation, so the
    // rest pose is byte-identical to the static root it replaced) because
    // `HumanoidModel.setupAttackAnimation` rotates it during a combat swing.
    let body = b.part_named(
        "body",
        [0.0, 0.0, 0.0],
        [0.0; 3],
        Anim::HumanoidBody,
        1.0,
        None,
    );
    b.cube(
        body,
        0,
        (16.0, 16.0),
        [-4.0, 0.0, -2.0],
        [8.0, 12.0, 4.0],
        NONE,
    );
    // Jacket overlay on the body.
    let jacket = avatar_layer(&mut b, "jacket", body, AVATAR_JACKET);
    b.cube_g(
        jacket,
        0,
        (16.0, 32.0),
        [-4.0, 0.0, -2.0],
        [8.0, 12.0, 4.0],
        0.25,
        NONE,
    );
    // Arms: wide = 4-px box at min.x −3/−1; slim = 3-px box at min.x −2/−1.
    let (aw, rmin, lmin) = if slim {
        (3.0, -2.0, -1.0)
    } else {
        (4.0, -3.0, -1.0)
    };
    let arm_r = b.part_named(
        "right_arm",
        [-HUMANOID_ARM_PIVOT_X, 2.0, 0.0],
        [0.0; 3],
        Anim::HumanoidArmRight,
        1.0,
        None,
    );
    b.cube(
        arm_r,
        0,
        (40.0, 16.0),
        [rmin, -2.0, -2.0],
        [aw, 12.0, 4.0],
        NONE,
    );
    let right_sleeve = avatar_layer(&mut b, "right_sleeve", arm_r, AVATAR_RIGHT_SLEEVE);
    b.cube_g(
        right_sleeve,
        0,
        (40.0, 32.0),
        [rmin, -2.0, -2.0],
        [aw, 12.0, 4.0],
        0.25,
        NONE,
    );
    let arm_l = b.part_named(
        "left_arm",
        [HUMANOID_ARM_PIVOT_X, 2.0, 0.0],
        [0.0; 3],
        Anim::HumanoidArmLeft,
        1.0,
        None,
    );
    b.cube(
        arm_l,
        0,
        (32.0, 48.0),
        [lmin, -2.0, -2.0],
        [aw, 12.0, 4.0],
        NONE,
    );
    let left_sleeve = avatar_layer(&mut b, "left_sleeve", arm_l, AVATAR_LEFT_SLEEVE);
    b.cube_g(
        left_sleeve,
        0,
        (48.0, 48.0),
        [lmin, -2.0, -2.0],
        [aw, 12.0, 4.0],
        0.25,
        NONE,
    );
    let leg_r = b.part([-1.9, 12.0, 0.0], Anim::LegRight, 1.0);
    b.cube(
        leg_r,
        0,
        (0.0, 16.0),
        [-2.0, 0.0, -2.0],
        [4.0, 12.0, 4.0],
        NONE,
    );
    let right_pants = avatar_layer(&mut b, "right_pants", leg_r, AVATAR_RIGHT_PANTS);
    b.cube_g(
        right_pants,
        0,
        (0.0, 32.0),
        [-2.0, 0.0, -2.0],
        [4.0, 12.0, 4.0],
        0.25,
        NONE,
    );
    let leg_l = b.part([1.9, 12.0, 0.0], Anim::LegLeft, 1.0);
    b.cube(
        leg_l,
        0,
        (16.0, 48.0),
        [-2.0, 0.0, -2.0],
        [4.0, 12.0, 4.0],
        NONE,
    );
    let left_pants = avatar_layer(&mut b, "left_pants", leg_l, AVATAR_LEFT_PANTS);
    b.cube_g(
        left_pants,
        0,
        (0.0, 48.0),
        [-2.0, 0.0, -2.0],
        [4.0, 12.0, 4.0],
        0.25,
        NONE,
    );
    b.finish(0.9375)
}

/// Zombie-family humanoid: vanilla mesh limbs (left = mirrored right rects),
/// arms held straight out (−90°, the iconic pose — vanilla's
/// `animateZombieArms` base angle) so they're static, legs swing normally.
fn zombie_like(scale: f32, overlay: Option<usize>) -> Model {
    let mut b = ModelBuilder::new();
    let head = humanoid_head_body(&mut b, 0);
    // M382 — the drowned (the only zombie with an overlay) is
    // `DrownedModel.createBodyLayer`: its left arm and leg are real boxes at
    // (32, 48) and (16, 48), not the right limbs mirrored, in the base and
    // in the 0.25 outer layer; its arms add the trident throw's post-step
    // (`Anim::DrownedArm*`).
    let drowned = overlay.is_some();
    let (arm_anims, left_arm_uv, left_leg_uv) = if drowned {
        (
            (Anim::DrownedArmRight, Anim::DrownedArmLeft),
            (32.0, 48.0),
            (16.0, 48.0),
        )
    } else {
        (
            (Anim::UndeadArmRight, Anim::UndeadArmLeft),
            (40.0, 16.0),
            (0.0, 16.0),
        )
    };
    // M20: the arms were baked forward by a static `Fold::rot(-π/2)`, which
    // froze them at −90° — vanilla rests at −π/2.25 (−80°) and deepens to
    // −π/1.5 (−120°) when aggressive, and swings from there. They are real
    // animated parts now; the rest angle comes from `animateZombieArms`.
    let arm_r = b.part_named(
        "right_arm",
        [-5.0, 2.0, 0.0],
        [0.0; 3],
        arm_anims.0,
        1.0,
        None,
    );
    b.cube(
        arm_r,
        0,
        (40.0, 16.0),
        [-3.0, -2.0, -2.0],
        [4.0, 12.0, 4.0],
        NONE,
    );
    let arm_l = b.part_named(
        "left_arm",
        [5.0, 2.0, 0.0],
        [0.0; 3],
        arm_anims.1,
        1.0,
        None,
    );
    b.cube_f(
        arm_l,
        0,
        left_arm_uv,
        [-1.0, -2.0, -2.0],
        [4.0, 12.0, 4.0],
        0.0,
        !drowned,
        NONE,
    );
    let leg_r = b.part([-1.9, 12.0, 0.0], Anim::LegRight, 1.0);
    b.cube(
        leg_r,
        0,
        (0.0, 16.0),
        [-2.0, 0.0, -2.0],
        [4.0, 12.0, 4.0],
        NONE,
    );
    let leg_l = b.part([1.9, 12.0, 0.0], Anim::LegLeft, 1.0);
    b.cube_f(
        leg_l,
        0,
        left_leg_uv,
        [-2.0, 0.0, -2.0],
        [4.0, 12.0, 4.0],
        0.0,
        !drowned,
        NONE,
    );
    if let Some(tex) = overlay {
        // Drowned outer layer: the same humanoid mesh inflated 0.25 (hat
        // 0.5+0.25), sampling the overlay texture. Parts mirror the base so
        // the clothing follows the head/legs.
        b.cube_g(
            head,
            tex,
            (0.0, 0.0),
            [-4.0, -8.0, -4.0],
            [8.0, 8.0, 8.0],
            0.25,
            NONE,
        );
        b.cube_g(
            head,
            tex,
            (32.0, 0.0),
            [-4.0, -8.0, -4.0],
            [8.0, 8.0, 8.0],
            0.75,
            NONE,
        );
        b.cube_g(
            STATIC_PART,
            tex,
            (16.0, 16.0),
            [-4.0, 0.0, -2.0],
            [8.0, 12.0, 4.0],
            0.25,
            NONE,
        );
        b.cube_f(
            arm_r,
            tex,
            (40.0, 16.0),
            [-3.0, -2.0, -2.0],
            [4.0, 12.0, 4.0],
            0.25,
            false,
            NONE,
        );
        b.cube_f(
            arm_l,
            tex,
            left_arm_uv,
            [-1.0, -2.0, -2.0],
            [4.0, 12.0, 4.0],
            0.25,
            false,
            NONE,
        );
        b.cube_g(
            leg_r,
            tex,
            (0.0, 16.0),
            [-2.0, 0.0, -2.0],
            [4.0, 12.0, 4.0],
            0.25,
            NONE,
        );
        b.cube_f(
            leg_l,
            tex,
            left_leg_uv,
            [-2.0, 0.0, -2.0],
            [4.0, 12.0, 4.0],
            0.25,
            false,
            NONE,
        );
    }
    b.finish(scale)
}

fn zombie() -> Model {
    zombie_like(1.0, None)
}

/// M317 — `BabyZombieModel.createBodyLayer(NONE)` (64 x 64): a 4 x 5 x 2
/// body, a 6-px head with its 0.25 outer layer (looking), 2 x 5 x 2 arms on
/// the zombie's arm animation, 2 x 4 x 2 legs, every box at its own texture
/// offset (the left limbs not mirrored). Its layout is not the adult's: the
/// baby sheet painted onto the adult mesh, which Rewo did from M175 to M316,
/// scrambles it. At `scale` (the husk's renderer adds 1.0625).
fn zombie_baby_like(scale: f32, outer: Option<usize>) -> Model {
    let mut b = ModelBuilder::new();
    // M319 — the drowned's outer layer is `BabyZombieModel.createBodyLayer`
    // at 0.25 on texture slot `outer`; its head boxes keep their own fixed
    // deformations (0 and 0.25), so they would be coplanar with the base
    // head's: lifted 0.002 px under Rewo's strict depth test.
    let layers: &[(usize, f32, f32)] = match outer {
        Some(t) => &[(0, 0.0, 0.0), (t, 0.25, 0.002)],
        None => &[(0, 0.0, 0.0)],
    };
    let head = b.part([0.0, 15.25, 0.0], Anim::Head, 1.0);
    let arms: Vec<usize> = [
        ("right_arm", [-3.0, 15.5, 0.0], Anim::UndeadArmRight),
        ("left_arm", [3.0, 15.5, 0.0], Anim::UndeadArmLeft),
    ]
    .into_iter()
    .map(|(name, pivot, anim)| b.part_named(name, pivot, [0.0; 3], anim, 1.0, None))
    .collect();
    let legs: Vec<usize> = [
        ([-1.0, 20.0, 0.0], Anim::LegRight),
        ([1.0, 20.0, 0.0], Anim::LegLeft),
    ]
    .into_iter()
    .map(|(pivot, anim)| b.part(pivot, anim, 1.0))
    .collect();
    for &(tex, g, lift) in layers {
        b.cube_f(
            STATIC_PART,
            tex,
            (16.0, 16.0),
            [-2.0, -2.5, -1.0],
            [4.0, 5.0, 2.0],
            g,
            false,
            &[Fold::at([0.0, 17.5, 0.0])],
        );
        b.cube_g(
            head,
            tex,
            (3.0, 3.0),
            [-3.0, -6.25, -3.0],
            [6.0, 6.0, 6.0],
            lift,
            NONE,
        );
        b.cube_g(
            head,
            tex,
            (35.0, 3.0),
            [-3.0, -6.15, -3.0],
            [6.0, 6.0, 6.0],
            0.25 + lift,
            NONE,
        );
        for (&arm, uv) in arms.iter().zip([(36.0, 16.0), (28.0, 16.0)]) {
            b.cube_g(arm, tex, uv, [-1.0, -0.5, -1.0], [2.0, 5.0, 2.0], g, NONE);
        }
        for (&leg, uv) in legs.iter().zip([(8.0, 16.0), (0.0, 16.0)]) {
            b.cube_g(leg, tex, uv, [-1.0, 0.0, -1.0], [2.0, 4.0, 2.0], g, NONE);
        }
    }
    b.finish(scale)
}

/// M319 — `BabyDrownedModel` is `BabyZombieModel`'s layer, with
/// `DrownedOuterLayer`'s baby model (the same layer at 0.25) on its own sheet.
fn drowned_baby() -> Model {
    zombie_baby_like(1.0, Some(1))
}

fn zombie_baby() -> Model {
    zombie_baby_like(1.0, None)
}

fn husk_baby() -> Model {
    // `HuskRenderer.scale` is 1.0625 for the baby too.
    zombie_baby_like(1.0625, None)
}

fn husk() -> Model {
    // Vanilla HuskRenderer scales the zombie model by 1.0625.
    zombie_like(1.0625, None)
}

/// M307 — `SulfurCubeRenderer`. The body is the outer box (`entityTranslucent`,
/// `RenderTypes::entityTranslucent` in `SulfurCubeModel`'s constructor), and
/// `SulfurCubeInnerLayer` draws the inner box opaque, `order(-1)` so before
/// it. `scale` ends `scale(extra)` then `translate(0, vOffset - 1/16, 0)` in
/// that scaled space, which is the boxes moved down by `vOffset - 1/16`
/// blocks of model space (14.68 px adult, 18.84 small) with `extra` (0.5
/// adult, 1.0 small) as the model scale; the size multiplies on top. Not
/// drawn: a swallowed block in place of the inner box, the fuse's swell and
/// flash, and the squish.
fn sulfur_cube_with(small: bool) -> Model {
    let (outer, inner, inner_uv, v_offset, extra) = if small {
        (5.0, 4.0, (0.0, 20.0), 1.24, 1.0)
    } else {
        (9.0, 8.0, (0.0, 36.0), 0.98, 0.5)
    };
    let dy = (v_offset - 1.0 / 16.0) * 16.0;
    let mut b = ModelBuilder::new();
    b.cube(
        STATIC_PART,
        1,
        inner_uv,
        [-inner, -inner + dy, -inner],
        [2.0 * inner; 3],
        NONE,
    );
    b.begin_translucent();
    b.cube(
        STATIC_PART,
        0,
        (0.0, 0.0),
        [-outer, -outer + dy, -outer],
        [2.0 * outer; 3],
        NONE,
    );
    b.finish(extra)
}

fn sulfur_cube() -> Model {
    sulfur_cube_with(false)
}

fn sulfur_cube_small() -> Model {
    sulfur_cube_with(true)
}

/// M304 — `GiantZombieModel` is an `AbstractZombieModel` on the humanoid
/// body layer through `MeshTransformer.scaling(6.0F)`: the zombie's mesh,
/// arms and all, six times the size.
fn giant() -> Model {
    zombie_like(6.0, None)
}

fn drowned() -> Model {
    zombie_like(1.0, Some(1))
}

/// `SkeletonModel`: humanoid head/body + 2×12×2 limbs, walking arm swing.
/// `overlay` adds the clothing layer at that deformation on texture 1 (M383:
/// the humanoid mesh, as vanilla's outer layers are).
/// Returns the head part so variants (bogged) can grow things on the skull.
fn skeleton_build(b: &mut ModelBuilder, overlay: Option<f32>) -> usize {
    let b = &mut *b;
    let head = humanoid_head_body(b, 0);
    // M20: `SkeletonModel.setupAnim` overrides the arms when aggressive and
    // not holding a bow — the rig keeps the humanoid walk otherwise.
    let arm_r = b.part_named(
        "right_arm",
        [-5.0, 2.0, 0.0],
        [0.0; 3],
        Anim::SkeletonArmRight,
        1.0,
        None,
    );
    b.cube(
        arm_r,
        0,
        (40.0, 16.0),
        [-1.0, -2.0, -1.0],
        [2.0, 12.0, 2.0],
        NONE,
    );
    let arm_l = b.part_named(
        "left_arm",
        [5.0, 2.0, 0.0],
        [0.0; 3],
        Anim::SkeletonArmLeft,
        1.0,
        None,
    );
    b.cube_m(
        arm_l,
        0,
        (40.0, 16.0),
        [-1.0, -2.0, -1.0],
        [2.0, 12.0, 2.0],
        NONE,
    );
    let leg_r = b.part([-2.0, 12.0, 0.0], Anim::LegRight, 1.0);
    b.cube(
        leg_r,
        0,
        (0.0, 16.0),
        [-1.0, 0.0, -1.0],
        [2.0, 12.0, 2.0],
        NONE,
    );
    let leg_l = b.part([2.0, 12.0, 0.0], Anim::LegLeft, 1.0);
    b.cube_m(
        leg_l,
        0,
        (0.0, 16.0),
        [-1.0, 0.0, -1.0],
        [2.0, 12.0, 2.0],
        NONE,
    );
    // M383 — the outer layer (`STRAY_OUTER_LAYER` at 0.25,
    // `BOGGED_OUTER_LAYER` at 0.2) is `HumanoidModel.createMesh(g)`, not the
    // skeleton's mesh inflated: the head at g and its hat at g + 0.5, the
    // body, 4 x 12 x 4 arms on the skeleton's ±5 arm pivots and 4 x 12 x 4
    // legs on their own ±1.9 pivots (the skeleton's are ±2), walking with
    // the skeleton's. Until M383 Rewo drew the skeleton's 2-wide limbs at
    // 0.25 for both.
    if let Some(g) = overlay {
        b.cube_g(
            head,
            1,
            (0.0, 0.0),
            [-4.0, -8.0, -4.0],
            [8.0, 8.0, 8.0],
            g,
            NONE,
        );
        b.cube_g(
            head,
            1,
            (32.0, 0.0),
            [-4.0, -8.0, -4.0],
            [8.0, 8.0, 8.0],
            g + 0.5,
            NONE,
        );
        b.cube_g(
            STATIC_PART,
            1,
            (16.0, 16.0),
            [-4.0, 0.0, -2.0],
            [8.0, 12.0, 4.0],
            g,
            NONE,
        );
        b.cube_g(
            arm_r,
            1,
            (40.0, 16.0),
            [-3.0, -2.0, -2.0],
            [4.0, 12.0, 4.0],
            g,
            NONE,
        );
        b.cube_f(
            arm_l,
            1,
            (40.0, 16.0),
            [-1.0, -2.0, -2.0],
            [4.0, 12.0, 4.0],
            g,
            true,
            NONE,
        );
        let cloth_leg_r = b.part([-1.9, 12.0, 0.0], Anim::LegRight, 1.0);
        b.cube_g(
            cloth_leg_r,
            1,
            (0.0, 16.0),
            [-2.0, 0.0, -2.0],
            [4.0, 12.0, 4.0],
            g,
            NONE,
        );
        let cloth_leg_l = b.part([1.9, 12.0, 0.0], Anim::LegLeft, 1.0);
        b.cube_f(
            cloth_leg_l,
            1,
            (0.0, 16.0),
            [-2.0, 0.0, -2.0],
            [4.0, 12.0, 4.0],
            g,
            true,
            NONE,
        );
    }
    head
}

fn skeleton_like(scale: f32, overlay: Option<f32>) -> Model {
    let mut b = ModelBuilder::new();
    skeleton_build(&mut b, overlay);
    b.finish(scale)
}

fn skeleton() -> Model {
    skeleton_like(1.0, Option::None)
}

fn stray() -> Model {
    skeleton_like(1.0, Some(0.25))
}

/// M384 — `SkeletonModel.createSingleModelDualBodyLayer()` (64 x 64), the
/// parched's own layer (`ParchedRenderer` adds no clothing layer): the
/// skeleton's head and body with a 0.2 head overlay at (0, 32), a belt at
/// (28, 0) and a 0.025 body overlay at (16, 48); arms on ±5.5 pivots (not
/// ±5), each a 2-wide bone plus a 3 x 12 x 3 sleeve; legs the same. Until
/// M384 Rewo drew the plain skeleton mesh on this 64 x 64 sheet, which
/// scrambled its layout.
fn parched() -> Model {
    let mut b = ModelBuilder::new();
    let head = b.part_named("head", [0.0; 3], [0.0; 3], Anim::Head, 1.0, Option::None);
    b.cube(
        head,
        0,
        (0.0, 0.0),
        [-4.0, -8.0, -4.0],
        [8.0, 8.0, 8.0],
        NONE,
    );
    b.cube_g(
        head,
        0,
        (0.0, 32.0),
        [-4.0, -8.0, -4.0],
        [8.0, 8.0, 8.0],
        0.2,
        NONE,
    );
    b.cube(
        STATIC_PART,
        0,
        (16.0, 16.0),
        [-4.0, 0.0, -2.0],
        [8.0, 12.0, 4.0],
        NONE,
    );
    b.cube(
        STATIC_PART,
        0,
        (28.0, 0.0),
        [-4.0, 10.0, -2.0],
        [8.0, 1.0, 4.0],
        NONE,
    );
    b.cube_g(
        STATIC_PART,
        0,
        (16.0, 48.0),
        [-4.0, 0.0, -2.0],
        [8.0, 12.0, 4.0],
        0.025,
        NONE,
    );
    for (name, x, anim, bone_uv, sleeve_uv, sleeve_x) in [
        (
            "right_arm",
            -5.5,
            Anim::SkeletonArmRight,
            (40.0, 16.0),
            (42.0, 33.0),
            -1.55,
        ),
        (
            "left_arm",
            5.5,
            Anim::SkeletonArmLeft,
            (56.0, 16.0),
            (40.0, 48.0),
            -1.45,
        ),
    ] {
        let arm = b.part_named(name, [x, 2.0, 0.0], [0.0; 3], anim, 1.0, Option::None);
        b.cube(arm, 0, bone_uv, [-1.0, -2.0, -1.0], [2.0, 12.0, 2.0], NONE);
        b.cube(
            arm,
            0,
            sleeve_uv,
            [sleeve_x, -2.025, -1.5],
            [3.0, 12.0, 3.0],
            NONE,
        );
    }
    for (x, anim, sleeve_uv) in [
        (-2.0, Anim::LegRight, (0.0, 49.0)),
        (2.0, Anim::LegLeft, (4.0, 49.0)),
    ] {
        let leg = b.part([x, 12.0, 0.0], anim, 1.0);
        b.cube(
            leg,
            0,
            (0.0, 16.0),
            [-1.0, 0.0, -1.0],
            [2.0, 12.0, 2.0],
            NONE,
        );
        b.cube(leg, 0, sleeve_uv, [-1.5, 0.0, -1.5], [3.0, 12.0, 3.0], NONE);
    }
    b.finish(1.0)
}

fn wither_skeleton() -> Model {
    // Vanilla WitherSkeletonRenderer scale.
    skeleton_like(1.2, Option::None)
}

/// `EndermanModel`: stretched humanoid, half-amplitude limb swing clamped to
/// ±0.4 (M352), and the carrying pose.
fn enderman() -> Model {
    let mut b = ModelBuilder::new();
    let head = b.part([0.0, -13.0, 0.0], Anim::Head, 1.0);
    b.cube(
        head,
        0,
        (0.0, 0.0),
        [-4.0, -8.0, -4.0],
        [8.0, 8.0, 8.0],
        NONE,
    );
    b.cube_g(
        head,
        0,
        (0.0, 16.0),
        [-4.0, -8.0, -4.0],
        [8.0, 8.0, 8.0],
        -0.5,
        NONE,
    );
    b.cube_f(
        STATIC_PART,
        0,
        (32.0, 16.0),
        [-4.0, 0.0, -2.0],
        [8.0, 12.0, 4.0],
        0.0,
        false,
        &[Fold::at([0.0, -14.0, 0.0])],
    );
    let arm_r = b.part([-5.0, -12.0, 0.0], Anim::EndermanArmRight, 1.0);
    b.cube(
        arm_r,
        0,
        (56.0, 0.0),
        [-1.0, -2.0, -1.0],
        [2.0, 30.0, 2.0],
        NONE,
    );
    let arm_l = b.part([5.0, -12.0, 0.0], Anim::EndermanArmLeft, 1.0);
    b.cube_m(
        arm_l,
        0,
        (56.0, 0.0),
        [-1.0, -2.0, -1.0],
        [2.0, 30.0, 2.0],
        NONE,
    );
    let leg_r = b.part([-2.0, -5.0, 0.0], Anim::EndermanLegRight, 1.0);
    b.cube(
        leg_r,
        0,
        (56.0, 0.0),
        [-1.0, 0.0, -1.0],
        [2.0, 30.0, 2.0],
        NONE,
    );
    let leg_l = b.part([2.0, -5.0, 0.0], Anim::EndermanLegLeft, 1.0);
    b.cube_m(
        leg_l,
        0,
        (56.0, 0.0),
        [-1.0, 0.0, -1.0],
        [2.0, 30.0, 2.0],
        NONE,
    );
    b.finish(1.0)
}

// ---------------------------------------------------------------------------
// Monsters with their own body plans
// ---------------------------------------------------------------------------

/// `CreeperModel`: head + tall body + four stubby legs on the quad gait.
fn creeper() -> Model {
    creeper_with(0.0)
}

/// M285 — `ModelLayers.CREEPER_ARMOR`: `CreeperModel.createBodyLayer(new
/// CubeDeformation(2.0F))`, the charged creeper's swirl shell. The same
/// parts in the same order as [`creeper`], so the base model's part
/// transforms pose it (`CreeperPowerLayer` runs the same `setupAnim` on its
/// own copy). Its texture is `creeper_armor.png`, sized by the layer
/// ([`CREEPER_ARMOR_TEXTURE`]).
pub fn creeper_armor() -> Model {
    creeper_with(2.0)
}

/// M286 — `BreezeWindLayer`: the breeze's wind funnel is texture slot 1 of
/// [`breeze`] (`breeze_wind.png`, `createWindLayer`'s 128 x 128), drawn as
/// its own layer through `RenderTypes.breezeWind` rather than with the body.
pub const BREEZE_WIND_SLOT: usize = 1;
pub const BREEZE_WIND_TEXTURE: [f32; 2] = [128.0, 128.0];

/// `LayerDefinition.create(mesh, 64, 32)` for the creeper layers.
pub const CREEPER_ARMOR_TEXTURE: [f32; 2] = [64.0, 32.0];

/// `CreeperModel.createBodyLayer(g)`.
fn creeper_with(g: f32) -> Model {
    let mut b = ModelBuilder::new();
    let head = b.part([0.0, 6.0, 0.0], Anim::Head, 1.0);
    b.cube_f(
        head,
        0,
        (0.0, 0.0),
        [-4.0, -8.0, -4.0],
        [8.0, 8.0, 8.0],
        g,
        false,
        NONE,
    );
    b.cube_f(
        STATIC_PART,
        0,
        (16.0, 16.0),
        [-4.0, 0.0, -2.0],
        [8.0, 12.0, 4.0],
        g,
        false,
        &[Fold::at([0.0, 6.0, 0.0])],
    );
    // M380 — `CreeperModel` binds its `leftHindLeg` field to the
    // `right_hind_leg` part (and so on for all four), so each part swings on
    // the other side's phase: half a cycle from the quadruped's.
    let legs = [
        ([-2.0, 18.0, 4.0], Anim::QuadHindLeft),
        ([2.0, 18.0, 4.0], Anim::QuadHindRight),
        ([-2.0, 18.0, -4.0], Anim::QuadFrontLeft),
        ([2.0, 18.0, -4.0], Anim::QuadFrontRight),
    ];
    for (pivot, anim) in legs {
        let p = b.part(pivot, anim, 1.0);
        b.cube_f(
            p,
            0,
            (0.0, 16.0),
            [-2.0, 0.0, -2.0],
            [4.0, 6.0, 4.0],
            g,
            false,
            NONE,
        );
    }
    b.finish(1.0)
}

/// `SpiderModel`: head + two body segments + 8 splayed legs (static pose;
/// the leg-wave animation is a follow-up).
fn spider_like(scale: f32) -> Model {
    let mut b = ModelBuilder::new();
    let head = b.part([0.0, 15.0, -3.0], Anim::Head, 1.0);
    b.cube(
        head,
        0,
        (32.0, 4.0),
        [-4.0, -4.0, -8.0],
        [8.0, 8.0, 8.0],
        NONE,
    );
    b.cube_f(
        STATIC_PART,
        0,
        (0.0, 0.0),
        [-3.0, -3.0, -3.0],
        [6.0, 6.0, 6.0],
        0.0,
        false,
        &[Fold::at([0.0, 15.0, 0.0])],
    );
    b.cube_f(
        STATIC_PART,
        0,
        (0.0, 12.0),
        [-5.0, -4.0, -6.0],
        [10.0, 8.0, 12.0],
        0.0,
        false,
        &[Fold::at([0.0, 15.0, 9.0])],
    );
    // (pivot, yRot, zRot, mirror, walk phase) per leg — right legs extend
    // −X, left +X. The splay is the part's base pose; the walk wave adds
    // yRot/zRot on top (vanilla `setupAnim` += deltas).
    const Z58: f32 = 0.58119464;
    let legs: [([f32; 3], f32, f32, bool, f32); 8] = [
        ([-4.0, 15.0, 2.0], PI / 4.0, -PI / 4.0, false, 0.0),
        ([4.0, 15.0, 2.0], -PI / 4.0, PI / 4.0, true, 0.0),
        ([-4.0, 15.0, 1.0], PI / 8.0, -Z58, false, PI),
        ([4.0, 15.0, 1.0], -PI / 8.0, Z58, true, PI),
        ([-4.0, 15.0, 0.0], -PI / 8.0, -Z58, false, PI / 2.0),
        ([4.0, 15.0, 0.0], PI / 8.0, Z58, true, PI / 2.0),
        ([-4.0, 15.0, -1.0], -PI / 4.0, -PI / 4.0, false, PI * 1.5),
        ([4.0, 15.0, -1.0], PI / 4.0, PI / 4.0, true, PI * 1.5),
    ];
    for (pivot, ry, rz, left, phase) in legs {
        let p = b.part_ext(
            pivot,
            [0.0, ry, rz],
            Anim::SpiderLeg { left, phase },
            1.0,
            Option::None,
        );
        let min = if left {
            [-1.0, -1.0, -1.0]
        } else {
            [-15.0, -1.0, -1.0]
        };
        b.cube_f(p, 0, (18.0, 0.0), min, [16.0, 2.0, 2.0], 0.0, left, NONE);
    }
    b.finish(scale)
}

fn spider() -> Model {
    spider_like(1.0)
}

fn cave_spider() -> Model {
    // Vanilla CaveSpiderRenderer scale.
    spider_like(0.7)
}

/// `SlimeRenderer`'s body is `SlimeModel.createInnerBodyLayer` (the 6³
/// core, two eyes and the mouth), and `SlimeOuterLayer` draws
/// `createOuterBodyLayer`'s 8³ shell over it `entityTranslucent`. Until M306
/// Rewo had no translucent entity range and drew the shell alone, opaque.
pub(crate) fn slime() -> Model {
    let mut b = ModelBuilder::new();
    for (uv, min, dims) in [
        ((0.0, 16.0), [-3.0, 17.0, -3.0], [6.0, 6.0, 6.0]),
        ((32.0, 0.0), [-3.25, 18.0, -3.5], [2.0, 2.0, 2.0]),
        ((32.0, 4.0), [1.25, 18.0, -3.5], [2.0, 2.0, 2.0]),
        ((32.0, 8.0), [0.0, 21.0, -3.5], [1.0, 1.0, 1.0]),
    ] {
        b.cube(STATIC_PART, 0, uv, min, dims, NONE);
    }
    b.begin_translucent();
    b.cube(
        STATIC_PART,
        0,
        (0.0, 0.0),
        [-4.0, 16.0, -4.0],
        [8.0, 8.0, 8.0],
        NONE,
    );
    // 8 px spans one block at 2×: the fixed "medium slime" of the old pass.
    b.finish(2.0)
}

// ---------------------------------------------------------------------------
// Farm + passive mobs
// ---------------------------------------------------------------------------

/// Shared quadruped legs (`QuadrupedModel.createLegs`): 4×`leg`×4 boxes at
/// (±x, 24−leg, 7 / −5) with per-side mirror flags.
#[allow(clippy::too_many_arguments)]
fn quadruped_legs(
    b: &mut ModelBuilder,
    tex: usize,
    uv: (f32, f32),
    leg: f32,
    x: f32,
    mirror_left: bool,
    mirror_right: bool,
) {
    quadruped_legs_g(b, tex, uv, leg, x, mirror_left, mirror_right, 0.0);
}

/// [`quadruped_legs`] at a deformation (M298: `QuadrupedModel.createLegs(...,
/// g)`).
#[allow(clippy::too_many_arguments)]
fn quadruped_legs_g(
    b: &mut ModelBuilder,
    tex: usize,
    uv: (f32, f32),
    leg: f32,
    x: f32,
    mirror_left: bool,
    mirror_right: bool,
    g: f32,
) {
    let y = 24.0 - leg;
    let legs = [
        ([-x, y, 7.0], Anim::QuadHindRight, mirror_right),
        ([x, y, 7.0], Anim::QuadHindLeft, mirror_left),
        ([-x, y, -5.0], Anim::QuadFrontRight, mirror_right),
        ([x, y, -5.0], Anim::QuadFrontLeft, mirror_left),
    ];
    for (pivot, anim, mirror) in legs {
        let p = b.part(pivot, anim, 1.0);
        b.cube_f(
            p,
            tex,
            uv,
            [-2.0, 0.0, -2.0],
            [4.0, leg, 4.0],
            g,
            mirror,
            NONE,
        );
    }
}

/// 26.2 `CowModel.createBaseCowModel` — its own mesh, not the generic
/// quadruped: 8×8×6 head with muzzle + horns, 12×18×10 body with udder,
/// 4×12×4 legs at ±4 (left pair mirrored).
fn cow() -> Model {
    cow_of(CowLook::Temperate)
}

/// M348 — the adult meshes `CowRenderer.models` picks by the variant's
/// `model`: NORMAL (`CowModel`), COLD and WARM.
#[derive(Clone, Copy, PartialEq, Eq)]
enum CowLook {
    Temperate,
    Cold,
    Warm,
}

/// M348 — `ColdCowModel.createBodyLayer()`: the base cow with its body
/// replaced (a 0.5-inflated coat at (20, 32) over the body and udder) and
/// its head (the muzzle at (9, 33), and no horn boxes on the head: two
/// 2 × 6 × 2 horns are child parts turned 1.5708 about x).
fn cold_cow() -> Model {
    cow_of(CowLook::Cold)
}

/// M348 — `WarmCowModel.createBodyLayer()`: the base cow with its head
/// replaced (the muzzle, then a 4 × 2 × 2 horn and its 2 × 2 × 2 tip each
/// side, the left pair mirrored).
fn warm_cow() -> Model {
    cow_of(CowLook::Warm)
}

fn cow_of(look: CowLook) -> Model {
    let mut b = ModelBuilder::new();
    let head = b.part([0.0, 4.0, -8.0], Anim::Head, 1.0);
    b.cube(
        head,
        0,
        (0.0, 0.0),
        [-4.0, -4.0, -6.0],
        [8.0, 8.0, 6.0],
        NONE,
    );
    let muzzle = if look == CowLook::Cold {
        (9.0, 33.0)
    } else {
        (1.0, 33.0)
    };
    b.cube(head, 0, muzzle, [-3.0, 1.0, -7.0], [6.0, 3.0, 1.0], NONE);
    match look {
        CowLook::Temperate => {
            for x in [-5.0, 4.0] {
                b.cube(head, 0, (22.0, 0.0), [x, -5.0, -5.0], [1.0, 3.0, 1.0], NONE);
            }
        }
        CowLook::Cold => {
            for (uv, pos, pivot) in [
                ((0.0, 40.0), [-1.5, -4.5, -0.5], [-4.5, -2.5, -3.5]),
                ((0.0, 32.0), [-1.5, -3.0, -0.5], [5.5, -2.5, -5.0]),
            ] {
                b.cube_f(
                    head,
                    0,
                    uv,
                    pos,
                    [2.0, 6.0, 2.0],
                    0.0,
                    false,
                    &[Fold::rot([1.5708, 0.0, 0.0], pivot)],
                );
            }
        }
        CowLook::Warm => {
            for (mirror, horn, tip) in [(false, -8.0, -8.0), (true, 4.0, 6.0)] {
                b.cube_f(
                    head,
                    0,
                    (27.0, 0.0),
                    [horn, -3.0, -5.0],
                    [4.0, 2.0, 2.0],
                    0.0,
                    mirror,
                    NONE,
                );
                b.cube_f(
                    head,
                    0,
                    (39.0, 0.0),
                    [tip, -5.0, -5.0],
                    [2.0, 2.0, 2.0],
                    0.0,
                    mirror,
                    NONE,
                );
            }
        }
    }
    let body = Fold::rot([FRAC_PI_2, 0.0, 0.0], [0.0, 5.0, 2.0]);
    if look == CowLook::Cold {
        b.cube_f(
            STATIC_PART,
            0,
            (20.0, 32.0),
            [-6.0, -10.0, -7.0],
            [12.0, 18.0, 10.0],
            0.5,
            false,
            &[body],
        );
    }
    b.cube_f(
        STATIC_PART,
        0,
        (18.0, 4.0),
        [-6.0, -10.0, -7.0],
        [12.0, 18.0, 10.0],
        0.0,
        false,
        &[body],
    );
    b.cube_f(
        STATIC_PART,
        0,
        (52.0, 0.0),
        [-2.0, 2.0, -8.0],
        [4.0, 6.0, 1.0],
        0.0,
        false,
        &[body],
    );
    let legs = [
        ([-4.0, 12.0, 7.0], Anim::QuadHindRight, false),
        ([4.0, 12.0, 7.0], Anim::QuadHindLeft, true),
        ([-4.0, 12.0, -5.0], Anim::QuadFrontRight, false),
        ([4.0, 12.0, -5.0], Anim::QuadFrontLeft, true),
    ];
    for (pivot, anim, mirror) in legs {
        let p = b.part(pivot, anim, 1.0);
        b.cube_f(
            p,
            0,
            (0.0, 16.0),
            [-2.0, 0.0, -2.0],
            [4.0, 12.0, 4.0],
            0.0,
            mirror,
            NONE,
        );
    }
    b.finish(1.0)
}

/// `PigModel`: generic quadruped (legSize 6, left legs mirrored) with the
/// snouted head.
fn pig() -> Model {
    pig_with(0.0)
}

/// M298 — `ModelLayers.PIG_SADDLE`: `PigModel.createBodyLayer(new
/// CubeDeformation(0.5F))`, `PigRenderer`'s `PIG_SADDLE` equipment layer
/// shell (whatever the pig's variant model). The same parts as [`pig`].
pub fn pig_saddle() -> Model {
    pig_with(0.5)
}

/// M298 — `StriderRenderer`'s `STRIDER_SADDLE` layer draws over
/// `ModelLayers.STRIDER_SADDLE`, which is the strider's own layer.
pub fn strider_saddle() -> Model {
    strider()
}

/// M298 — a model with no boxes (a saddle with no reins).
pub fn empty_model() -> Model {
    ModelBuilder::new().finish(1.0)
}

/// `PigModel.createBasePigModel(g)`: `g` on every box.
fn pig_with(g: f32) -> Model {
    pig_of(g, false)
}

/// M349 — `ColdPigModel.createBodyLayer(NONE)`: the base pig with its body
/// replaced by the body and a 0.5-inflated coat at (28, 32) over it.
fn cold_pig() -> Model {
    pig_of(0.0, true)
}

fn pig_of(g: f32, cold: bool) -> Model {
    let mut b = ModelBuilder::new();
    let head = b.part([0.0, 12.0, -6.0], Anim::Head, 1.0);
    b.cube_f(
        head,
        0,
        (0.0, 0.0),
        [-4.0, -4.0, -8.0],
        [8.0, 8.0, 8.0],
        g,
        false,
        NONE,
    );
    b.cube_f(
        head,
        0,
        (16.0, 16.0),
        [-2.0, 0.0, -9.0],
        [4.0, 3.0, 1.0],
        g,
        false,
        NONE,
    );
    let body = Fold::rot([FRAC_PI_2, 0.0, 0.0], [0.0, 11.0, 2.0]);
    b.cube_f(
        STATIC_PART,
        0,
        (28.0, 8.0),
        [-5.0, -10.0, -7.0],
        [10.0, 16.0, 8.0],
        g,
        false,
        &[body],
    );
    if cold {
        b.cube_f(
            STATIC_PART,
            0,
            (28.0, 32.0),
            [-5.0, -10.0, -7.0],
            [10.0, 16.0, 8.0],
            0.5,
            false,
            &[body],
        );
    }
    quadruped_legs_g(&mut b, 0, (0.0, 16.0), 6.0, 3.0, true, false, g);
    b.finish(1.0)
}

/// `SheepModel` + `SheepFurModel`: quadruped (legSize 12, right legs
/// mirrored) with the sheep head/body, plus the inflated wool overlay on
/// texture 1 (head +0.6 follows the head-look; body +1.75; upper legs +0.5
/// ride the leg swing), plus 26.x's undercoat on texture 2.
///
/// **The undercoat is not a third fleece mesh — it is the body mesh again.**
/// `SheepWoolUndercoatLayer` wraps its model in `SheepFurModel`, but the class
/// only supplies `setupAnim`; the geometry comes from the baked layer, and
/// `LayerDefinitions` maps `SHEEP_WOOL_UNDERCOAT` to **`sheepBodyLayer`** —
/// `SheepModel.createBodyLayer()`, `CubeDeformation.NONE`. So every box below
/// is the texture-0 box repeated, and the layer is coplanar with the sheep
/// itself (see [`coplanar_layer_texture`] for how it is drawn).
fn sheep() -> Model {
    let mut b = ModelBuilder::new();
    let head = b.part([0.0, 6.0, -8.0], Anim::SheepHead, 1.0);
    b.cube(
        head,
        0,
        (0.0, 0.0),
        [-3.0, -4.0, -6.0],
        [6.0, 6.0, 8.0],
        NONE,
    );
    let body = Fold::rot([FRAC_PI_2, 0.0, 0.0], [0.0, 5.0, 2.0]);
    b.cube_f(
        STATIC_PART,
        0,
        (28.0, 8.0),
        [-4.0, -10.0, -7.0],
        [8.0, 16.0, 6.0],
        0.0,
        false,
        &[body],
    );
    quadruped_legs(&mut b, 0, (0.0, 16.0), 12.0, 3.0, false, true);
    // Wool (SheepFurModel). Legs were parts 2..6 in registration order.
    b.cube_g(
        head,
        1,
        (0.0, 0.0),
        [-3.0, -4.0, -4.0],
        [6.0, 6.0, 6.0],
        0.6,
        NONE,
    );
    b.cube_f(
        STATIC_PART,
        1,
        (28.0, 8.0),
        [-4.0, -10.0, -7.0],
        [8.0, 16.0, 6.0],
        1.75,
        false,
        &[body],
    );
    for leg_part in 2..6 {
        b.cube_g(
            leg_part,
            1,
            (0.0, 16.0),
            [-2.0, 0.0, -2.0],
            [4.0, 6.0, 4.0],
            0.5,
            NONE,
        );
    }
    // Undercoat (SheepModel.createBodyLayer again, on texture 2). The legs
    // keep `createBodyMesh(12, false, true, NONE)`'s asymmetric mirror — the
    // *right* pair only — where the fur layer above mirrors neither, so this
    // cannot reuse that loop.
    b.cube(
        head,
        2,
        (0.0, 0.0),
        [-3.0, -4.0, -6.0],
        [6.0, 6.0, 8.0],
        NONE,
    );
    b.cube_f(
        STATIC_PART,
        2,
        (28.0, 8.0),
        [-4.0, -10.0, -7.0],
        [8.0, 16.0, 6.0],
        0.0,
        false,
        &[body],
    );
    for leg_part in 2..6 {
        // `quadruped_legs` registers hind-right, hind-left, front-right,
        // front-left, so the even parts are the mirrored right pair.
        let mirror = leg_part % 2 == 0;
        b.cube_f(
            leg_part,
            2,
            (0.0, 16.0),
            [-2.0, 0.0, -2.0],
            [4.0, 12.0, 4.0],
            0.0,
            mirror,
            NONE,
        );
    }
    b.finish(1.0)
}

/// `AdultChickenModel`: head with beak + wattle, rotated body, biped-swing
/// legs, static wings (the flap animation is airborne-only).
fn chicken() -> Model {
    chicken_of(false)
}

/// M349 — `ColdChickenModel.createBodyLayer()`: the base chicken with its
/// body replaced (the body and a 0 × 3 × 5 tail plane at (38, 9)) and its
/// head replaced (the head and a 6 × 3 × 4 crest at (44, 0)); the beak and
/// wattle are the head's children and survive the replacement
/// (`PartDefinition.addOrReplaceChild` keeps them).
fn cold_chicken() -> Model {
    chicken_of(true)
}

fn chicken_of(cold: bool) -> Model {
    let mut b = ModelBuilder::new();
    let head = b.part([0.0, 15.0, -4.0], Anim::Head, 1.0);
    b.cube(
        head,
        0,
        (0.0, 0.0),
        [-2.0, -6.0, -2.0],
        [4.0, 6.0, 3.0],
        NONE,
    );
    if cold {
        b.cube(
            head,
            0,
            (44.0, 0.0),
            [-3.0, -7.0, -2.015],
            [6.0, 3.0, 4.0],
            NONE,
        );
    }
    b.cube(
        head,
        0,
        (14.0, 0.0),
        [-2.0, -4.0, -4.0],
        [4.0, 2.0, 2.0],
        NONE,
    );
    b.cube(
        head,
        0,
        (14.0, 4.0),
        [-1.0, -2.0, -3.0],
        [2.0, 2.0, 2.0],
        NONE,
    );
    let body = Fold::rot([FRAC_PI_2, 0.0, 0.0], [0.0, 16.0, 0.0]);
    b.cube_f(
        STATIC_PART,
        0,
        (0.0, 9.0),
        [-3.0, -4.0, -3.0],
        [6.0, 8.0, 6.0],
        0.0,
        false,
        &[body],
    );
    if cold {
        b.cube_f(
            STATIC_PART,
            0,
            (38.0, 9.0),
            [0.0, 3.0, -1.0],
            [0.0, 3.0, 5.0],
            0.0,
            false,
            &[body],
        );
    }
    let leg_r = b.part([-2.0, 19.0, 1.0], Anim::LegRight, 1.0);
    b.cube(
        leg_r,
        0,
        (26.0, 0.0),
        [-1.0, 0.0, -3.0],
        [3.0, 5.0, 3.0],
        NONE,
    );
    let leg_l = b.part([1.0, 19.0, 1.0], Anim::LegLeft, 1.0);
    b.cube(
        leg_l,
        0,
        (26.0, 0.0),
        [-1.0, 0.0, -3.0],
        [3.0, 5.0, 3.0],
        NONE,
    );
    b.cube_f(
        STATIC_PART,
        0,
        (24.0, 13.0),
        [0.0, 0.0, -3.0],
        [1.0, 4.0, 6.0],
        0.0,
        false,
        &[Fold::at([-4.0, 13.0, 0.0])],
    );
    b.cube_f(
        STATIC_PART,
        0,
        (24.0, 13.0),
        [-1.0, 0.0, -3.0],
        [1.0, 4.0, 6.0],
        0.0,
        false,
        &[Fold::at([4.0, 13.0, 0.0])],
    );
    b.finish(1.0)
}

/// `AdultWolfModel`: head (skull + ears + snout), two rotated body
/// segments, 2×8×2 legs (right pair mirrored), angled tail.
/// M342 — the wolf, with `WolfCollarLayer` on texture 1: that layer submits
/// the wolf's own model (`getParentModel()`) in the collar's sheet, so its
/// quads are the body's again, coplanar, drawn through the coplanar range
/// (see [`coplanar_layer_texture`]) when the wolf is tame.
fn wolf() -> Model {
    with_collar_copy(wolf_with(0.0), wolf_with(0.0))
}

/// M343 — the adult cat: `ModelLayers.CAT` is the feline body layer through
/// `AdultCatModel.CAT_TRANSFORMER`, `MeshTransformer.scaling(0.8)` about
/// y 24.016 (the renderer's 1.501-block origin, so a model scale is the same
/// transform); until M343 Rewo drew the cat at the ocelot's full size. On
/// texture 1, `CatCollarLayer`'s `CAT_COLLAR` mesh: the same body grown
/// `COLLAR_DEFORMATION` (0.01) through the same scaling, so it sits just
/// outside the cat and draws in the solid range.
fn cat() -> Model {
    let mut m = with_collar_copy(feline_with(0.0), feline_with(0.01));
    m.scale = 0.8;
    m
}

/// M343 — the kitten with `CatCollarLayer`'s `CAT_BABY_COLLAR`: the kitten's
/// own layer through `BabyCatModel.COLLAR_TRANSFORMER`,
/// `MeshTransformer.scaling(1.01)`, so the collar is the kitten scaled 1.01
/// about y 24.016. A part-local quad cannot scale on its own parts, so the
/// copy brings its own (see [`with_scaled_copy`]).
fn cat_baby() -> Model {
    with_scaled_copy(feline_baby(), feline_baby(), 1.01)
}

/// M343 — `base` plus `copy` scaled by `s` about (0, 24.016, 0) the way a
/// `MeshTransformer.scaling(s)` scales a layer, on texture 1 and on its own
/// parts (appended after `base`'s: a root part's pivot scaled about that
/// point, a child's pivot and every quad scaled about its parent's frame,
/// the rotations and animations unchanged, since a uniform scale commutes
/// with them).
fn with_scaled_copy(mut base: Model, copy: Model, s: f32) -> Model {
    const C: f32 = 24.016;
    let n = base.parts.len() as u16;
    for mut part in copy.parts {
        part.pivot = match part.parent {
            None => [
                part.pivot[0] * s,
                C + (part.pivot[1] - C) * s,
                part.pivot[2] * s,
            ],
            Some(_) => part.pivot.map(|v| v * s),
        };
        part.parent = part.parent.map(|p| p + n);
        base.parts.push(part);
    }
    // `STATIC_PART` quads sit in model space: scale them about the same
    // point and keep them static.
    base.quads.extend(copy.quads.into_iter().map(|mut q| {
        let root = q.part == STATIC_PART;
        q.pos = q.pos.map(|p| {
            if root {
                [p[0] * s, C + (p[1] - C) * s, p[2] * s]
            } else {
                p.map(|v| v * s)
            }
        });
        if !root {
            q.part += n as usize;
        }
        q.tex = 1;
        q
    }));
    base
}

/// M342 — `base` plus `copy`'s quads (the same builder run again, so the
/// same parts in the same order) moved to texture 1.
fn with_collar_copy(mut base: Model, copy: Model) -> Model {
    assert_eq!(base.parts.len(), copy.parts.len());
    base.quads
        .extend(copy.quads.into_iter().filter(|q| q.tex == 0).map(|mut q| {
            q.tex = 1;
            q
        }));
    base
}

/// M299 — `ModelLayers.WOLF_ARMOR`: `AdultWolfModel.createBodyLayer(new
/// CubeDeformation(0.2F))` on a 64 x 32 sheet, `WolfArmorLayer`'s
/// `WOLF_BODY` equipment layer shell. The same parts as [`wolf`].
pub fn wolf_armor() -> Model {
    wolf_with(0.2)
}

/// `AdultWolfModel.createBodyLayer(g)`: `g` on every box.
fn wolf_with(g: f32) -> Model {
    let mut b = ModelBuilder::new();
    let head = b.part([-1.0, 13.5, -7.0], Anim::Head, 1.0);
    b.cube_f(
        head,
        0,
        (0.0, 0.0),
        [-2.0, -3.0, -2.0],
        [6.0, 6.0, 4.0],
        g,
        false,
        NONE,
    );
    b.cube_f(
        head,
        0,
        (16.0, 14.0),
        [-2.0, -5.0, 0.0],
        [2.0, 2.0, 1.0],
        g,
        false,
        NONE,
    );
    b.cube_f(
        head,
        0,
        (16.0, 14.0),
        [2.0, -5.0, 0.0],
        [2.0, 2.0, 1.0],
        g,
        false,
        NONE,
    );
    b.cube_f(
        head,
        0,
        (0.0, 10.0),
        [-0.5, -0.001, -5.0],
        [3.0, 3.0, 4.0],
        g,
        false,
        NONE,
    );
    // M356 — the body and the mane are parts now: the sitting pose moves
    // them.
    let body = b.part_ext(
        [0.0, 14.0, 2.0],
        [FRAC_PI_2, 0.0, 0.0],
        Anim::WolfBody,
        1.0,
        Option::None,
    );
    b.cube_f(
        body,
        0,
        (18.0, 14.0),
        [-3.0, -2.0, -3.0],
        [6.0, 9.0, 6.0],
        g,
        false,
        NONE,
    );
    let mane = b.part_ext(
        [-1.0, 14.0, -3.0],
        [FRAC_PI_2, 0.0, 0.0],
        Anim::WolfMane,
        1.0,
        Option::None,
    );
    b.cube_f(
        mane,
        0,
        (21.0, 0.0),
        [-3.0, -3.0, -3.0],
        [8.0, 6.0, 7.0],
        g,
        false,
        NONE,
    );
    let legs = [
        ([-2.5, 16.0, 7.0], Anim::WolfHind { left: false }, true),
        ([0.5, 16.0, 7.0], Anim::WolfHind { left: true }, false),
        ([-2.5, 16.0, -4.0], Anim::WolfFront { left: false }, true),
        ([0.5, 16.0, -4.0], Anim::WolfFront { left: true }, false),
    ];
    for (pivot, anim, mirror) in legs {
        let p = b.part(pivot, anim, 1.0);
        b.cube_f(
            p,
            0,
            (0.0, 18.0),
            [0.0, 0.0, -1.0],
            [2.0, 8.0, 2.0],
            g,
            mirror,
            NONE,
        );
    }
    // Tail: π/5 hang + the walking side-to-side wag.
    let tail = b.part_ext(
        [-1.0, 12.0, 8.0],
        [PI / 5.0, 0.0, 0.0],
        Anim::TailWagY,
        1.0,
        Option::None,
    );
    b.cube_f(
        tail,
        0,
        (9.0, 18.0),
        [0.0, 0.0, -1.0],
        [2.0, 8.0, 2.0],
        g,
        false,
        NONE,
    );
    b.finish(1.0)
}

/// `SquidModel`: big body + 8 tentacles hanging from a radius-5 ring, each
/// yawed to face outward.
fn squid() -> Model {
    let mut b = ModelBuilder::new();
    b.cube_f(
        STATIC_PART,
        0,
        (0.0, 0.0),
        [-6.0, -8.0, -6.0],
        [12.0, 16.0, 12.0],
        0.02,
        false,
        &[Fold::at([0.0, 8.0, 0.0])],
    );
    for i in 0..8 {
        let angle = i as f32 * PI * 2.0 / 8.0;
        let pivot = [angle.cos() * 5.0, 15.0, angle.sin() * 5.0];
        let y_rot = -(i as f32) * PI * 2.0 / 8.0 + PI / 2.0;
        let p = b.part_ext(
            pivot,
            [0.0, y_rot, 0.0],
            Anim::SquidTentacle,
            1.0,
            Option::None,
        );
        b.cube(p, 0, (48.0, 0.0), [-1.0, 0.0, -1.0], [2.0, 18.0, 2.0], NONE);
    }
    b.finish(1.0)
}

/// `AdultRabbitModel`: tilted body with tail + head + ears, front legs,
/// yaw-splayed haunches — hopping on the vanilla HOP rig (vanilla triggers
/// it per jump; jump state isn't on the wire, so a moving rabbit hops
/// continuously — see `KfDriver::AgeGatedByWalk`).
fn rabbit() -> Model {
    let mut b = ModelBuilder::new();
    let body = b.part_named(
        "body",
        [0.0, 23.0, 4.0],
        [-0.3927, 0.0, 0.0],
        Anim::None,
        1.0,
        Option::None,
    );
    b.cube(
        body,
        0,
        (0.0, 0.0),
        [-4.0, -6.0, -9.0],
        [8.0, 6.0, 10.0],
        NONE,
    );
    let tail = b.part_named(
        "tail",
        [0.0, -4.9916, 0.0125],
        [0.0; 3],
        Anim::None,
        1.0,
        Some(body),
    );
    b.cube(
        tail,
        0,
        (20.0, 16.0),
        [-2.0, -3.0084, -1.0125],
        [4.0, 4.0, 4.0],
        NONE,
    );
    // M364 — `RabbitModel.setupAnim` assigns the head's `yRot` / `xRot`
    // from the look every frame (unless the idle head tilt plays, which Rewo
    // does not), so the layer's 0.3927 is never drawn: at a level look the
    // head leans with the body's −0.3927. Until M364 it held the layer's
    // tilt, level, and never looked.
    let head = b.part_named(
        "head",
        [0.0, -5.2929, -8.1213],
        [0.0; 3],
        Anim::Head,
        1.0,
        Some(body),
    );
    b.cube(
        head,
        0,
        (0.0, 16.0),
        [-2.5, -3.0, -4.0],
        [5.0, 5.0, 5.0],
        NONE,
    );
    let ear_l = b.part_named(
        "left_ear",
        [1.5, -3.7071, -0.8787],
        [0.0; 3],
        Anim::None,
        1.0,
        Some(head),
    );
    b.cube(
        ear_l,
        0,
        (32.0, 0.0),
        [-1.0, -4.2929, -0.1213],
        [2.0, 5.0, 1.0],
        NONE,
    );
    let ear_r = b.part_named(
        "right_ear",
        [-1.5, -3.7071, -0.8787],
        [0.0; 3],
        Anim::None,
        1.0,
        Some(head),
    );
    b.cube(
        ear_r,
        0,
        (26.0, 0.0),
        [-1.0, -4.2929, -0.1213],
        [2.0, 5.0, 1.0],
        NONE,
    );
    let front = b.part_named(
        "frontlegs",
        [0.0, -1.5349, -6.3108],
        [0.0; 3],
        Anim::None,
        1.0,
        Some(body),
    );
    let fl_r = b.part_named(
        "right_front_leg",
        [-2.0, 1.9239, 0.3827],
        [0.3927, 0.0, 0.0],
        Anim::None,
        1.0,
        Some(front),
    );
    b.cube(
        fl_r,
        0,
        (36.0, 18.0),
        [-0.9, -1.0, -0.9],
        [2.0, 4.0, 2.0],
        NONE,
    );
    let fl_l = b.part_named(
        "left_front_leg",
        [2.0, 1.9239, 0.4827],
        [0.3927, 0.0, 0.0],
        Anim::None,
        1.0,
        Some(front),
    );
    b.cube(
        fl_l,
        0,
        (44.0, 18.0),
        [-1.0, -1.0, -1.0],
        [2.0, 4.0, 2.0],
        NONE,
    );
    let back = b.part_named(
        "backlegs",
        [0.0, 23.0, 4.0],
        [0.0; 3],
        Anim::None,
        1.0,
        Option::None,
    );
    let hl_r = b.part_named(
        "right_hind_leg",
        [-3.0, 0.5, 0.0],
        [0.0; 3],
        Anim::None,
        1.0,
        Some(back),
    );
    b.cube_f(
        hl_r,
        0,
        (20.0, 24.0),
        [-1.0, 0.0, -5.0],
        [2.0, 1.0, 6.0],
        0.0,
        false,
        &[Fold::rot([0.0, 0.3927, 0.0], [0.0, -0.5, 0.0])],
    );
    let hl_l = b.part_named(
        "left_hind_leg",
        [3.0, 0.5, 0.0],
        [0.0; 3],
        Anim::None,
        1.0,
        Some(back),
    );
    b.cube_f(
        hl_l,
        0,
        (36.0, 24.0),
        [-1.0, 0.0, -5.0],
        [2.0, 1.0, 6.0],
        0.0,
        false,
        &[Fold::rot([0.0, -0.3927, 0.0], [0.0, -0.5, 0.0])],
    );
    b.keyframe(
        &crate::anim_defs::RABBIT_HOP,
        KfDriver::AgeGatedByWalk { gate: 5.0 },
    );
    b.finish(1.0)
}

/// `VillagerModel`: big head with nose + hat (+ rotated hat rim), robed
/// body, crossed arms, half-swing legs. Returns the head part so the witch
/// can stack her hat on it, and (M379) for the witch the nose part
/// (`Anim::WitchNose`, its mole goes on it); the villager's nose stays a
/// box on the head.
fn villager_build(b: &mut ModelBuilder, witch: bool) -> (usize, Option<usize>) {
    let head = b.part([0.0, 0.0, 0.0], Anim::Head, 1.0);
    b.cube(
        head,
        0,
        (0.0, 0.0),
        [-4.0, -10.0, -4.0],
        [8.0, 10.0, 8.0],
        NONE,
    );
    b.cube_g(
        head,
        0,
        (32.0, 0.0),
        [-4.0, -10.0, -4.0],
        [8.0, 10.0, 8.0],
        0.51,
        NONE,
    );
    b.cube_f(
        head,
        0,
        (30.0, 47.0),
        [-8.0, -8.0, -6.0],
        [16.0, 16.0, 1.0],
        0.0,
        false,
        &[Fold::rot([-FRAC_PI_2, 0.0, 0.0], [0.0, 0.0, 0.0])],
    );
    let nose =
        witch.then(|| b.part_ext([0.0, -2.0, 0.0], [0.0; 3], Anim::WitchNose, 1.0, Some(head)));
    match nose {
        Some(n) => b.cube(n, 0, (24.0, 0.0), [-1.0, -1.0, -6.0], [2.0, 4.0, 2.0], NONE),
        Option::None => b.cube_f(
            head,
            0,
            (24.0, 0.0),
            [-1.0, -1.0, -6.0],
            [2.0, 4.0, 2.0],
            0.0,
            false,
            &[Fold::at([0.0, -2.0, 0.0])],
        ),
    }
    b.cube(
        STATIC_PART,
        0,
        (16.0, 20.0),
        [-4.0, 0.0, -3.0],
        [8.0, 12.0, 6.0],
        NONE,
    );
    b.cube_g(
        STATIC_PART,
        0,
        (0.0, 38.0),
        [-4.0, 0.0, -3.0],
        [8.0, 20.0, 6.0],
        0.5,
        NONE,
    );
    let arms = Fold::rot([-0.75, 0.0, 0.0], [0.0, 3.0, -1.0]);
    b.cube_f(
        STATIC_PART,
        0,
        (44.0, 22.0),
        [-8.0, -2.0, -2.0],
        [4.0, 8.0, 4.0],
        0.0,
        false,
        &[arms],
    );
    b.cube_f(
        STATIC_PART,
        0,
        (44.0, 22.0),
        [4.0, -2.0, -2.0],
        [4.0, 8.0, 4.0],
        0.0,
        true,
        &[arms],
    );
    b.cube_f(
        STATIC_PART,
        0,
        (40.0, 38.0),
        [-4.0, 2.0, -2.0],
        [8.0, 4.0, 4.0],
        0.0,
        false,
        &[arms],
    );
    let leg_r = b.part([-2.0, 12.0, 0.0], Anim::LegRight, 0.5);
    b.cube(
        leg_r,
        0,
        (0.0, 22.0),
        [-2.0, 0.0, -2.0],
        [4.0, 12.0, 4.0],
        NONE,
    );
    let leg_l = b.part([2.0, 12.0, 0.0], Anim::LegLeft, 0.5);
    b.cube_m(
        leg_l,
        0,
        (0.0, 22.0),
        [-2.0, 0.0, -2.0],
        [4.0, 12.0, 4.0],
        NONE,
    );
    (head, nose)
}

/// M318 — `BabyVillagerModel.createBodyModel()` (64 x 64): the head (8 x 8 x
/// 7, looking) with its hat (0.3), brim and nose on child folds, the folded
/// arms (a hand pair and a middle bar, each tilted -1.0472 under the `arms`
/// part), the body, the robe (`bb_main`, 0.2) and two short legs on the
/// villager's half-amplitude walk. Its layout is not the adult's: the baby
/// sheet painted onto the adult mesh (M175 to M317) scrambled it. Only the
/// base sheet, as Rewo's adult villager draws (no type or profession layer).
fn villager_baby() -> Model {
    let mut b = ModelBuilder::new();
    let head = b.part([0.0, 16.0, 0.0], Anim::Head, 1.0);
    b.cube(
        head,
        0,
        (0.0, 0.0),
        [-4.0, -8.0, -3.5],
        [8.0, 8.0, 7.0],
        NONE,
    );
    b.cube_f(
        head,
        0,
        (0.0, 30.0),
        [-4.0, -4.0, -3.5],
        [8.0, 8.0, 7.0],
        0.3,
        false,
        &[Fold::at([0.0, -4.0, 0.0])],
    );
    b.cube_f(
        head,
        0,
        (0.0, 45.0),
        [-7.0, -0.5, -6.0],
        [14.0, 1.0, 12.0],
        0.0,
        false,
        &[Fold::at([0.0, -4.5, 0.0])],
    );
    b.cube_f(
        head,
        0,
        (23.0, 0.0),
        [-1.0, 0.0, -0.5],
        [2.0, 2.0, 1.0],
        0.0,
        false,
        &[Fold::at([0.0, -2.0, -4.0])],
    );
    let arms = Fold::at([0.0, 17.5, 0.0]);
    let hands = Fold::rot([-1.0472, 0.0, 0.0], [-3.0, 1.4025, -0.9599]);
    for (uv, min) in [
        ((36.0, 15.0), [-1.0, -2.4925, -1.8401]),
        ((16.0, 15.0), [5.0, -2.4925, -1.8401]),
    ] {
        b.cube_f(
            STATIC_PART,
            0,
            uv,
            min,
            [2.0, 4.0, 2.0],
            0.0,
            false,
            &[hands, arms],
        );
    }
    b.cube_f(
        STATIC_PART,
        0,
        (24.0, 17.0),
        [-2.0, -0.9924, -0.9825],
        [4.0, 2.0, 2.0],
        0.0,
        false,
        &[Fold::rot([-1.0472, 0.0, 0.0], [0.0, 0.9024, -1.8175]), arms],
    );
    b.cube_f(
        STATIC_PART,
        0,
        (0.0, 15.0),
        [-2.0, -2.75, -1.5],
        [4.0, 5.0, 3.0],
        0.0,
        false,
        &[Fold::at([0.0, 18.75, 0.0])],
    );
    b.cube_f(
        STATIC_PART,
        0,
        (16.0, 21.0),
        [-2.5, -8.0, -1.5],
        [4.0, 6.0, 3.0],
        0.2,
        false,
        &[Fold::at([0.5, 24.0, 0.0])],
    );
    for (pivot, anim, uv) in [
        ([-1.0, 21.5, 0.0], Anim::LegRight, (8.0, 23.0)),
        ([1.0, 21.5, 0.0], Anim::LegLeft, (0.0, 23.0)),
    ] {
        let leg = b.part(pivot, anim, 0.5);
        b.cube(leg, 0, uv, [-1.0, -0.5, -1.0], [2.0, 3.0, 2.0], NONE);
    }
    b.finish(1.0)
}

/// M387 — `AdultAxolotlModel.setupAnim`: the body turns with the look,
/// then the swimming, water-hovering, ground-crawling, lying-still and
/// playing-dead poses add in, each weighted by its factor (binary here:
/// `in_water`, `moving`, `dead` — vanilla eases them), then the right legs
/// mirror the left by `1 − min(onGround, moving)`. Returns each slot's
/// rotation delta (see [`Anim::Axolotl`]) and the body's `y` offset.
pub fn axolotl_pose(
    age: f32,
    pitch: f32,
    net: f32,
    in_water: bool,
    moving: bool,
    dead: bool,
) -> ([[f32; 3]; 10], f32) {
    use std::f32::consts::PI;
    let f = |b: bool| if b { 1.0f32 } else { 0.0 };
    let (iw, og, mv, pd) = (f(in_water), f(!in_water), f(moving), f(dead));
    let nm = 1.0 - mv;
    let mut r = [[0.0f32; 3]; 10];
    let mut body_y = 0.0f32;
    const BODY: usize = 0;
    const HEAD: usize = 1;
    const TOP: usize = 2;
    const LGILL: usize = 3;
    const RGILL: usize = 4;
    const RHIND: usize = 5;
    const LHIND: usize = 6;
    const RFRONT: usize = 7;
    const LFRONT: usize = 8;
    const TAIL: usize = 9;
    r[BODY][1] += net;
    // setupSwimmingAnimation(age, xRot, min(moving, inWater))
    let k = mv.min(iw);
    if k > 1.0e-5 {
        let m = age * 0.33;
        let (sn, cs) = (m.sin(), m.cos());
        let sway = 0.13 * sn;
        r[BODY][0] += (pitch + sway) * k;
        r[HEAD][0] -= sway * 1.8 * k;
        body_y -= 0.45 * cs * k;
        r[TOP][0] += (-0.5 * sn - 0.8) * k;
        let g = (0.3 * sn + 0.9) * k;
        r[LGILL][1] += g;
        r[RGILL][1] -= g;
        r[TAIL][1] += 0.3 * (m * 0.9).cos() * k;
        r[LHIND][0] += 1.884_955_8 * k;
        r[LHIND][1] += -0.4 * sn * k;
        r[LHIND][2] += PI / 2.0 * k;
        r[LFRONT][0] += 1.884_955_8 * k;
        r[LFRONT][1] += (-0.2 * cs - 0.1) * k;
        r[LFRONT][2] += PI / 2.0 * k;
    }
    // setupWaterHoveringAnimation(age, min(notMoving, inWater))
    let k = nm.min(iw);
    if k > 1.0e-5 {
        let m = age * 0.075;
        let cs = m.cos();
        let sn = m.sin() * 0.15;
        let bx = (-0.15 + 0.075 * cs) * k;
        r[BODY][0] += bx;
        body_y -= sn * k;
        r[HEAD][0] -= bx;
        r[TOP][0] += 0.2 * cs * k;
        let g = (-0.3 * cs - 0.19) * k;
        r[LGILL][1] += g;
        r[RGILL][1] -= g;
        r[LHIND][0] += (PI * 3.0 / 4.0 - cs * 0.11) * k;
        r[LHIND][1] += 0.471_238_94 * k;
        r[LHIND][2] += 1.727_876_1 * k;
        r[LFRONT][0] += (PI / 4.0 - cs * 0.2) * k;
        r[LFRONT][1] += 2.042_035 * k;
        r[TAIL][1] += 0.5 * cs * k;
    }
    // setupGroundCrawlingAnimation(age, min(moving, onGround))
    let k = mv.min(og);
    if k > 1.0e-5 {
        let m = age * 0.11;
        let cs = m.cos();
        let hind = (cs * cs - 2.0 * cs) / 5.0;
        let front = 0.7 * cs;
        let ht = 0.09 * cs * k;
        r[HEAD][1] += ht;
        r[TAIL][1] += ht;
        let g = (0.6 - 0.08 * (cs * cs + 2.0 * m.sin())) * k;
        r[TOP][0] += g;
        r[LGILL][1] -= g;
        r[RGILL][1] += g;
        let (hx, fx) = (0.942_477_9 * k, 1.099_557_4 * k);
        r[LHIND][0] += hx;
        r[LHIND][1] += (1.5 - hind) * k;
        r[LHIND][2] += -0.1 * k;
        r[LFRONT][0] += fx;
        r[LFRONT][1] += (PI / 2.0 - front) * k;
        r[RHIND][0] += hx;
        r[RHIND][1] += (-1.0 - hind) * k;
        r[RFRONT][0] += fx;
        r[RFRONT][1] += (-PI / 2.0 - front) * k;
    }
    // setupLayStillOnGroundAnimation(age, min(notMoving, onGround))
    let k = nm.min(og);
    if k > 1.0e-5 {
        let m = age * 0.09;
        let (sn, cs) = (m.sin(), m.cos());
        let mv1 = sn * sn - 2.0 * sn;
        let mv2 = cs * cs - 3.0 * sn;
        r[HEAD][0] += -0.09 * mv1 * k;
        r[HEAD][2] += -0.2 * k;
        r[TAIL][1] += (-0.1 + 0.1 * mv1) * k;
        let g = (0.6 + 0.05 * mv2) * k;
        r[TOP][0] += g;
        r[LGILL][1] -= g;
        r[RGILL][1] += g;
        r[LHIND][0] += 1.1 * k;
        r[LHIND][1] += 1.0 * k;
        r[LFRONT][0] += 0.8 * k;
        r[LFRONT][1] += 2.3 * k;
        r[LFRONT][2] -= 0.5 * k;
    }
    // setupPlayDeadAnimation(playingDead)
    if pd > 1.0e-5 {
        r[LHIND][0] += 1.413_716_7 * pd;
        r[LHIND][1] += 1.099_557_4 * pd;
        r[LHIND][2] += PI / 4.0 * pd;
        r[LFRONT][0] += PI / 4.0 * pd;
        r[LFRONT][1] += 2.042_035 * pd;
        r[BODY][0] += -0.15 * pd;
        r[BODY][2] += 0.35 * pd;
    }
    // applyMirrorLegRotations(1 − min(onGround, moving))
    let mirror = 1.0 - og.min(mv);
    if mirror > 1.0e-5 {
        let (lh, lf) = (r[LHIND], r[LFRONT]);
        r[RHIND][0] += lh[0] * mirror;
        r[RHIND][1] += -lh[1] * mirror;
        r[RHIND][2] += -lh[2] * mirror;
        r[RFRONT][0] += lf[0] * mirror;
        r[RFRONT][1] += -lf[1] * mirror;
        r[RFRONT][2] += -lf[2] * mirror;
    }
    (r, body_y)
}

/// M381 — whether a kind's model takes the seated riding pose: a
/// `HumanoidModel` (its arms `HumanoidArm*` / `UndeadArm*` /
/// `SkeletonArm*` / `PiglinArm`) or an `IllagerModel` (`IllagerArm*`). The villager, the
/// witch and the other leg-walkers have no riding pose. Read off the built
/// models once.
pub fn sits_when_riding(kind: EntityModelKind) -> bool {
    static SITS: std::sync::OnceLock<Vec<bool>> = std::sync::OnceLock::new();
    let sits = SITS.get_or_init(|| {
        let mut v = vec![false; EntityModelKind::COUNT];
        for def in MOBS {
            v[def.kind.index()] = (def.build)().parts.iter().any(|p| {
                matches!(
                    p.anim,
                    Anim::HumanoidArmRight
                        | Anim::UndeadArmRight
                        | Anim::SkeletonArmRight
                        | Anim::IllagerArmRight
                        | Anim::PiglinArm { left: false }
                        | Anim::DrownedArmRight
                )
            });
        }
        v
    });
    sits.get(kind.index()).copied().unwrap_or(false)
}

/// M360 — `LayerDefinitions`' `villagerLikeScale`,
/// `MeshTransformer.scaling(0.9375F)`, on the villager's and the wandering
/// trader's layer (and the no-hat one), the witch's and the illagers'
/// (`ModelLayers.EVOKER` / `ILLUSIONER` / `PILLAGER` / `VINDICATOR`); no
/// renderer scales them again. A model scale is the same transform (about
/// the renderer's 24.016 px origin). Until M360 Rewo drew all seven about 6%
/// too big (found by the 2026-10-21 rest-pose audit).
pub const VILLAGER_LIKE_SCALE: f32 = 0.9375;

fn villager() -> Model {
    let mut b = ModelBuilder::new();
    villager_build(&mut b, false);
    b.finish(VILLAGER_LIKE_SCALE)
}

// ---------------------------------------------------------------------------
// Humanoid variants II — piglins, illagers, witch, zombie villager
// ---------------------------------------------------------------------------

/// M385 — `PlayerModel.createMesh`'s sleeves (0.25 overlays on the arms):
/// the right at (40, 32), the left at (48, 48).
fn piglin_sleeves(b: &mut ModelBuilder, arm_r: usize, arm_l: usize) {
    b.cube_g(
        arm_r,
        0,
        (40.0, 32.0),
        [-3.0, -2.0, -2.0],
        [4.0, 12.0, 4.0],
        0.25,
        NONE,
    );
    b.cube_g(
        arm_l,
        0,
        (48.0, 48.0),
        [-1.0, -2.0, -2.0],
        [4.0, 12.0, 4.0],
        0.25,
        NONE,
    );
}

/// Humanoid body + arms + legs shared by the piglin family
/// (`AdultPiglinModel.createBodyLayer` = `PlayerModel.createMesh(NONE,
/// false)` with a replaced 10-wide head + snout + ears, the hat cleared).
/// M385 — the player mesh's true left limbs at (32, 48) / (16, 48) and its
/// 0.25 sleeves, pants and jacket; Rewo mirrored the right limbs and drew no
/// overlay (the brute's jacket and pants were missing).
fn piglin_like(arms_forward: bool) -> Model {
    let mut b = ModelBuilder::new();
    // M390 — a `PiglinModel`'s head and body pose (the zombified piglin is
    // an `AbstractPiglinModel` only: it looks, and its body is still).
    let head_anim = if arms_forward {
        Anim::Head
    } else {
        Anim::PiglinHead
    };
    let head = b.part([0.0, 0.0, 0.0], head_anim, 1.0);
    let body = if arms_forward {
        STATIC_PART
    } else {
        b.part([0.0, 0.0, 0.0], Anim::PiglinBody, 1.0)
    };
    b.cube(
        head,
        0,
        (0.0, 0.0),
        [-5.0, -8.0, -4.0],
        [10.0, 8.0, 8.0],
        NONE,
    );
    b.cube(
        head,
        0,
        (31.0, 1.0),
        [-2.0, -4.0, -5.0],
        [4.0, 4.0, 1.0],
        NONE,
    );
    b.cube(
        head,
        0,
        (2.0, 4.0),
        [2.0, -2.0, -5.0],
        [1.0, 2.0, 1.0],
        NONE,
    );
    b.cube(
        head,
        0,
        (2.0, 0.0),
        [-3.0, -2.0, -5.0],
        [1.0, 2.0, 1.0],
        NONE,
    );
    // M380 — the ears are parts on `AbstractPiglinModel.setupAnim`'s flap
    // about their ±30° (`getDefaultEarAngleInDegrees`).
    for (uv, min, x, z, left) in [
        ((39.0, 6.0), [-1.0, 0.0, -2.0], -4.5, PI / 6.0, false),
        ((51.0, 6.0), [0.0, 0.0, -2.0], 4.5, -PI / 6.0, true),
    ] {
        let ear = b.part_ext(
            [x, -6.0, 0.0],
            [0.0, 0.0, z],
            Anim::PiglinEar { left },
            1.0,
            Some(head),
        );
        b.cube(ear, 0, uv, min, [1.0, 5.0, 4.0], NONE);
    }
    b.cube(
        body,
        0,
        (16.0, 16.0),
        [-4.0, 0.0, -2.0],
        [8.0, 12.0, 4.0],
        NONE,
    );
    if arms_forward {
        // M20: `ZombifiedPiglinModel.setupAnim` runs `animateZombieArms` after
        // the humanoid stage, exactly like `AbstractZombieModel` — so these are
        // animated parts, not a baked −π/2 fold.
        let arm_r = b.part_named(
            "right_arm",
            [-5.0, 2.0, 0.0],
            [0.0; 3],
            Anim::UndeadArmRight,
            1.0,
            None,
        );
        b.cube(
            arm_r,
            0,
            (40.0, 16.0),
            [-3.0, -2.0, -2.0],
            [4.0, 12.0, 4.0],
            NONE,
        );
        let arm_l = b.part_named(
            "left_arm",
            [5.0, 2.0, 0.0],
            [0.0; 3],
            Anim::UndeadArmLeft,
            1.0,
            None,
        );
        b.cube(
            arm_l,
            0,
            (32.0, 48.0),
            [-1.0, -2.0, -2.0],
            [4.0, 12.0, 4.0],
            NONE,
        );
        piglin_sleeves(&mut b, arm_r, arm_l);
    } else {
        // M380 — `HumanoidModel`'s walk swing and idle bob
        // (`Anim::PiglinArm`).
        let arm_r = b.part([-5.0, 2.0, 0.0], Anim::PiglinArm { left: false }, 1.0);
        b.cube(
            arm_r,
            0,
            (40.0, 16.0),
            [-3.0, -2.0, -2.0],
            [4.0, 12.0, 4.0],
            NONE,
        );
        let arm_l = b.part([5.0, 2.0, 0.0], Anim::PiglinArm { left: true }, 1.0);
        b.cube(
            arm_l,
            0,
            (32.0, 48.0),
            [-1.0, -2.0, -2.0],
            [4.0, 12.0, 4.0],
            NONE,
        );
        piglin_sleeves(&mut b, arm_r, arm_l);
    }
    let leg_r = b.part([-1.9, 12.0, 0.0], Anim::LegRight, 1.0);
    b.cube(
        leg_r,
        0,
        (0.0, 16.0),
        [-2.0, 0.0, -2.0],
        [4.0, 12.0, 4.0],
        NONE,
    );
    let leg_l = b.part([1.9, 12.0, 0.0], Anim::LegLeft, 1.0);
    b.cube(
        leg_l,
        0,
        (16.0, 48.0),
        [-2.0, 0.0, -2.0],
        [4.0, 12.0, 4.0],
        NONE,
    );
    // M385 — the pants and the jacket (`PlayerModel.createMesh`'s 0.25
    // overlays; the piglin's own body replaces `body` but keeps its child).
    b.cube_g(
        leg_r,
        0,
        (0.0, 32.0),
        [-2.0, 0.0, -2.0],
        [4.0, 12.0, 4.0],
        0.25,
        NONE,
    );
    b.cube_g(
        leg_l,
        0,
        (0.0, 48.0),
        [-2.0, 0.0, -2.0],
        [4.0, 12.0, 4.0],
        0.25,
        NONE,
    );
    b.cube_g(
        body,
        0,
        (16.0, 32.0),
        [-4.0, 0.0, -2.0],
        [8.0, 12.0, 4.0],
        0.25,
        NONE,
    );
    b.finish(1.0)
}

/// M320 — `BabyPiglinModel.createBodyLayer()` (64 x 64): the body, the head
/// (a 9 x 6 x 7 box and its snout, looking), its two ears (each a box tilted
/// 0.6109 on a child of an ear part at `getDefaultEarAngleInDegrees`' 5
/// degrees, still as Rewo's adult ears are), 2 x 5 x 3 arms, 3 x 4 x 3 legs.
/// The zombified piglin's baby is the same layer with the zombie arms.
fn piglin_baby_like(zombified: bool) -> Model {
    let mut b = ModelBuilder::new();
    // M390 — `BabyPiglinModel` is a `PiglinModel`: its head and body pose
    // as the adult's do (the zombified baby's do not).
    if zombified {
        b.cube_f(
            STATIC_PART,
            0,
            (0.0, 13.0),
            [-3.0, -3.0, -1.0],
            [6.0, 5.0, 3.0],
            0.0,
            false,
            &[Fold::at([0.0, 18.0, -0.5])],
        );
    } else {
        let body = b.part([0.0, 18.0, -0.5], Anim::PiglinBody, 1.0);
        b.cube(
            body,
            0,
            (0.0, 13.0),
            [-3.0, -3.0, -1.0],
            [6.0, 5.0, 3.0],
            NONE,
        );
    }
    let head_anim = if zombified {
        Anim::Head
    } else {
        Anim::PiglinHead
    };
    let head = b.part([0.0, 15.0, 0.0], head_anim, 1.0);
    b.cube(
        head,
        0,
        (21.0, 30.0),
        [-1.5, -3.0, -4.5],
        [3.0, 1.0, 1.0],
        NONE,
    );
    b.cube(
        head,
        0,
        (0.0, 0.0),
        [-4.5, -6.0, -3.5],
        [9.0, 6.0, 7.0],
        NONE,
    );
    let ear = 5.0f32.to_radians();
    for (uv, tilt, ear_rot, ear_at, child_at) in [
        (
            (0.0, 21.0),
            -0.6109,
            -ear,
            [4.2, -4.0, 0.0],
            [1.0, 1.75, 0.0],
        ),
        (
            (18.0, 13.0),
            0.6109,
            ear,
            [-4.2, -4.0, 0.0],
            [-1.0, 1.75, 0.0],
        ),
    ] {
        // M380 — the ear part flaps about its 5° (the tilted box rides it).
        let ear = b.part_ext(
            ear_at,
            [0.0, 0.0, ear_rot],
            Anim::PiglinEar {
                left: ear_at[0] > 0.0,
            },
            1.0,
            Some(head),
        );
        b.cube_f(
            ear,
            0,
            uv,
            [-0.5, -3.0, -2.0],
            [1.0, 6.0, 4.0],
            0.0,
            false,
            &[Fold::rot([0.0, 0.0, tilt], child_at)],
        );
    }
    let (arm_r_anim, arm_l_anim) = if zombified {
        (Anim::UndeadArmRight, Anim::UndeadArmLeft)
    } else {
        (
            Anim::PiglinArm { left: false },
            Anim::PiglinArm { left: true },
        )
    };
    for (name, pivot, anim, uv) in [
        ("left_arm", [4.0, 15.0, 0.0], arm_l_anim, (28.0, 13.0)),
        ("right_arm", [-4.0, 15.0, 0.0], arm_r_anim, (10.0, 30.0)),
    ] {
        let arm = b.part_named(name, pivot, [0.0; 3], anim, 1.0, None);
        b.cube(arm, 0, uv, [-1.0, 0.0, -1.5], [2.0, 5.0, 3.0], NONE);
    }
    for (pivot, anim, uv) in [
        ([-1.5, 20.0, 0.0], Anim::LegRight, (22.0, 23.0)),
        ([1.5, 20.0, 0.0], Anim::LegLeft, (10.0, 23.0)),
    ] {
        let leg = b.part(pivot, anim, 1.0);
        b.cube(leg, 0, uv, [-1.5, 0.0, -1.5], [3.0, 4.0, 3.0], NONE);
    }
    b.finish(1.0)
}

fn piglin_baby() -> Model {
    piglin_baby_like(false)
}

fn zombified_piglin_baby() -> Model {
    piglin_baby_like(true)
}

/// M321 — `BabyCowModel.createBodyLayer()` (64 x 64): the head (a 6 x 6 x 5
/// box, two horns, the left mirrored, and the muzzle; looking), the body and
/// four 3 x 6 x 3 legs on the quadruped walk. The mooshroom calf is the same
/// layer (`MOOSHROOM_BABY`) and wears no mushrooms.
fn cow_baby() -> Model {
    let mut b = ModelBuilder::new();
    let head = b.part([0.0, 13.569, -5.1667], Anim::Head, 1.0);
    b.cube(
        head,
        0,
        (0.0, 18.0),
        [-3.0, -4.569, -4.8333],
        [6.0, 6.0, 5.0],
        NONE,
    );
    b.cube(
        head,
        0,
        (8.0, 29.0),
        [3.0, -5.569, -3.8333],
        [1.0, 2.0, 1.0],
        NONE,
    );
    b.cube_m(
        head,
        0,
        (4.0, 29.0),
        [-4.0, -5.569, -3.8333],
        [1.0, 2.0, 1.0],
        NONE,
    );
    b.cube(
        head,
        0,
        (12.0, 29.0),
        [-2.0, -1.569, -5.8333],
        [4.0, 3.0, 1.0],
        NONE,
    );
    b.cube_f(
        STATIC_PART,
        0,
        (0.0, 0.0),
        [-7.0, -7.0, -1.0],
        [8.0, 6.0, 12.0],
        0.0,
        false,
        &[Fold::at([3.0, 19.0, -5.0])],
    );
    for (pivot, anim, uv) in [
        ([-2.5, 18.0, -3.5], Anim::QuadFrontRight, (22.0, 18.0)),
        ([2.5, 18.0, -3.5], Anim::QuadFrontLeft, (34.0, 18.0)),
        ([-2.5, 18.0, 3.5], Anim::QuadHindRight, (22.0, 27.0)),
        ([2.5, 18.0, 3.5], Anim::QuadHindLeft, (34.0, 27.0)),
    ] {
        let leg = b.part(pivot, anim, 1.0);
        b.cube(leg, 0, uv, [-1.5, 0.0, -1.5], [3.0, 6.0, 3.0], NONE);
    }
    b.finish(1.0)
}

/// M322 — `BabyPigModel.createBodyLayer(CubeDeformation.NONE)` (32 x 32):
/// the body, the head (a 7 x 6 x 6 box grown 0.025 and the snout grown
/// 0.015; looking) and four 2 x 2 x 2 legs on the quadruped walk. Two legs
/// share the body's (0, 0) as vanilla authors them.
fn pig_baby() -> Model {
    let mut b = ModelBuilder::new();
    b.cube_f(
        STATIC_PART,
        0,
        (0.0, 0.0),
        [-3.5, -3.0, -4.5],
        [7.0, 6.0, 9.0],
        0.0,
        false,
        &[Fold::at([0.0, 19.0, 0.5])],
    );
    let head = b.part([0.0, 19.0, -2.0], Anim::Head, 1.0);
    b.cube_g(
        head,
        0,
        (0.0, 15.0),
        [-3.5, -5.0, -5.0],
        [7.0, 6.0, 6.0],
        0.025,
        NONE,
    );
    b.cube_g(
        head,
        0,
        (6.0, 27.0),
        [-1.5, -1.975, -6.0],
        [3.0, 2.0, 1.0],
        0.015,
        NONE,
    );
    for (pivot, anim, uv) in [
        ([2.5, 22.0, -3.0], Anim::QuadFrontLeft, (0.0, 0.0)),
        ([-2.5, 22.0, -3.0], Anim::QuadFrontRight, (23.0, 0.0)),
        ([2.5, 22.0, 4.0], Anim::QuadHindLeft, (0.0, 4.0)),
        ([-2.5, 22.0, 4.0], Anim::QuadHindRight, (23.0, 4.0)),
    ] {
        let leg = b.part(pivot, anim, 1.0);
        b.cube(leg, 0, uv, [-1.0, 0.0, -1.0], [2.0, 2.0, 2.0], NONE);
    }
    b.finish(1.0)
}

/// M322 — `BabyChickenModel.createBodyLayer()` (16 x 16): a 4 x 4 x 4 body
/// that is the chick's head too (no head part, so it never looks) with its
/// beak, two legs (a 1 x 2 x 0 plate and a 1 x 0 x 1 foot each) on the walk,
/// and two 1 x 0 x 2 wings, still as Rewo's adult wings are.
fn chicken_baby() -> Model {
    let mut b = ModelBuilder::new();
    let body = [Fold::at([0.0, 20.25, -1.25])];
    b.cube_f(
        STATIC_PART,
        0,
        (0.0, 0.0),
        [-2.0, -2.25, -0.75],
        [4.0, 4.0, 4.0],
        0.0,
        false,
        &body,
    );
    b.cube_f(
        STATIC_PART,
        0,
        (10.0, 8.0),
        [-1.0, -0.25, -1.75],
        [2.0, 1.0, 1.0],
        0.0,
        false,
        &body,
    );
    for (x, anim, leg_uv, foot_uv) in [
        (1.0, Anim::LegLeft, (2.0, 2.0), (0.0, 1.0)),
        (-1.0, Anim::LegRight, (0.0, 2.0), (0.0, 0.0)),
    ] {
        let leg = b.part([x, 22.0, 0.5], anim, 1.0);
        b.cube(leg, 0, leg_uv, [-0.5, 0.0, 0.0], [1.0, 2.0, 0.0], NONE);
        b.cube(leg, 0, foot_uv, [-0.5, 2.0, -1.0], [1.0, 0.0, 1.0], NONE);
    }
    for (x, uv, min_x) in [(2.0, (6.0, 8.0), 0.0), (-2.0, (4.0, 8.0), -1.0)] {
        b.cube_f(
            STATIC_PART,
            0,
            uv,
            [min_x, 0.0, -1.0],
            [1.0, 0.0, 2.0],
            0.0,
            false,
            &[Fold::at([x, 20.0, 0.0])],
        );
    }
    b.finish(1.0)
}

/// M323 — `BabySheepModel.createBodyLayer()` (64 x 32): a 5 x 5 x 5 head
/// (looking), a 6 x 4 x 9 body authored level, and four 2 x 5 x 2 legs on the
/// quadruped walk, none mirrored. Texture 1 is the fleece: `SHEEP_BABY_WOOL`
/// is the same layer, so every box is repeated there, coplanar (see
/// [`coplanar_layer_texture`]).
fn sheep_baby() -> Model {
    let mut b = ModelBuilder::new();
    let head = b.part([0.0, 15.5, -2.5], Anim::SheepHead, 1.0);
    let legs = [
        ([-2.0, 19.0, 3.0], Anim::QuadHindRight, (0.0, 23.0)),
        ([2.0, 19.0, 3.0], Anim::QuadHindLeft, (24.0, 12.0)),
        ([-2.0, 19.0, -2.0], Anim::QuadFrontRight, (8.0, 23.0)),
        ([2.0, 19.0, -2.0], Anim::QuadFrontLeft, (24.0, 5.0)),
    ]
    .map(|(pivot, anim, uv)| (b.part(pivot, anim, 1.0), uv));
    for tex in [0, 1] {
        b.cube(
            head,
            tex,
            (0.0, 0.0),
            [-2.5, -4.5, -3.5],
            [5.0, 5.0, 5.0],
            NONE,
        );
        b.cube_f(
            STATIC_PART,
            tex,
            (0.0, 10.0),
            [-3.0, -2.0, -4.5],
            [6.0, 4.0, 9.0],
            0.0,
            false,
            &[Fold::at([0.0, 17.0, 0.5])],
        );
        for (leg, uv) in legs {
            b.cube(leg, tex, uv, [-1.0, 0.0, -1.0], [2.0, 5.0, 2.0], NONE);
        }
    }
    b.finish(1.0)
}

/// M324 — `BabyDolphinModel.createBodyLayer()` (64 x 64): the adult's part
/// tree at the calf's size. A 6 x 5 x 8 body (looking and bobbing since
/// M371, as the adult's); under it the head with its nose, the two fins and
/// the back fin, and the two-segment tail on the swim stroke. Unlike the adult's, the tail has no
/// base tilt.
fn dolphin_baby() -> Model {
    let mut b = ModelBuilder::new();
    // M371 — the body is a part: `DolphinModel.setupAnim` turns it with the
    // look (and bobs it while swimming), and everything rides on it.
    let bp = b.part_named(
        "body",
        [0.0, 21.5, 0.0],
        [0.0; 3],
        Anim::DolphinBody,
        1.0,
        Option::None,
    );
    b.cube_f(
        bp,
        0,
        (20.0, 0.0),
        [-3.0, -2.5, -4.0],
        [6.0, 5.0, 8.0],
        0.0,
        false,
        NONE,
    );
    let head = Fold::at([0.0, 1.0, -4.0]);
    b.cube_f(
        bp,
        0,
        (0.0, 0.0),
        [-3.0, -3.5, -4.0],
        [6.0, 5.0, 4.0],
        0.0,
        false,
        &[head],
    );
    b.cube_f(
        bp,
        0,
        (0.0, 9.0),
        [-1.0, -1.0, -2.0],
        [2.0, 2.0, 2.0],
        0.0,
        false,
        &[Fold::at([0.0, 0.5, -4.0]), head],
    );
    for (x, z_rot, uv, mirror) in [
        (1.8, 1.7017, (34.0, 18.0), false),
        (-1.8, -1.7017, (48.0, 18.0), true),
    ] {
        b.cube_f(
            bp,
            0,
            uv,
            [-0.5, -1.5, -0.5],
            [1.0, 3.0, 6.0],
            0.0,
            mirror,
            &[Fold::rot([0.8727, 0.0, z_rot], [x, 0.85, -2.6])],
        );
    }
    b.cube_f(
        bp,
        0,
        (42.0, 0.0),
        [-0.5, -1.0, 1.0],
        [1.0, 3.0, 4.0],
        0.0,
        false,
        &[Fold::rot([0.8727, 0.0, 0.0], [0.0, -1.0, -2.7])],
    );
    let tail = b.part_ext(
        [0.0, 1.0, 4.0],
        [0.0; 3],
        Anim::DolphinTail { k: 0.1, rest: 0.0 },
        1.0,
        Some(bp),
    );
    b.cube(
        tail,
        0,
        (0.0, 13.0),
        [-2.0, -1.5, 0.0],
        [4.0, 3.0, 7.0],
        NONE,
    );
    let tail_fin = b.part_ext(
        [0.0, 0.0, 6.0],
        [0.0; 3],
        Anim::DolphinTail { k: 0.2, rest: 0.0 },
        1.0,
        Some(tail),
    );
    b.cube(
        tail_fin,
        0,
        (22.0, 13.0),
        [-4.0, -0.5, -1.0],
        [8.0, 1.0, 4.0],
        NONE,
    );
    b.finish(1.0)
}

/// M325 — `BabyZombieVillagerModel.createBodyLayer()` (64 x 64): the baby
/// villager's head (8 x 8 x 7, looking) with its hat (0.3), brim and nose on
/// child folds, a body with its robe (0.1), two 2 x 5 x 2 arms on
/// `animateZombieArms` (named, as the adult's are) and two short legs on the
/// humanoid walk. Only the base sheet, as Rewo's adult draws (no type or
/// profession layer).
fn zombie_villager_baby() -> Model {
    let mut b = ModelBuilder::new();
    let body = [Fold::at([0.0, 18.75, 0.0])];
    b.cube_f(
        STATIC_PART,
        0,
        (0.0, 15.0),
        [-2.0, -2.75, -1.5],
        [4.0, 5.0, 3.0],
        0.0,
        false,
        &body,
    );
    b.cube_f(
        STATIC_PART,
        0,
        (16.0, 22.0),
        [-2.0, -2.75, -1.5],
        [4.0, 6.0, 3.0],
        0.1,
        false,
        &body,
    );
    let head = b.part([0.0, 16.0, 0.0], Anim::Head, 1.0);
    b.cube(
        head,
        0,
        (0.0, 0.0),
        [-4.0, -8.0, -3.5],
        [8.0, 8.0, 7.0],
        NONE,
    );
    b.cube_f(
        head,
        0,
        (0.0, 31.0),
        [-4.0, -4.0, -3.5],
        [8.0, 8.0, 7.0],
        0.3,
        false,
        &[Fold::at([0.0, -4.0, 0.0])],
    );
    b.cube_f(
        head,
        0,
        (0.0, 46.0),
        [-7.0, -0.5, -6.0],
        [14.0, 1.0, 12.0],
        0.0,
        false,
        &[Fold::at([0.0, -4.5, 0.0])],
    );
    b.cube_f(
        head,
        0,
        (23.0, 0.0),
        [-1.0, -1.0, -0.5],
        [2.0, 2.0, 1.0],
        0.0,
        false,
        &[Fold::at([0.0, -1.0, -4.0])],
    );
    for (name, pivot, anim, uv) in [
        (
            "right_arm",
            [-3.0, 15.5, 0.0],
            Anim::UndeadArmRight,
            (24.0, 15.0),
        ),
        (
            "left_arm",
            [3.0, 15.5, 0.0],
            Anim::UndeadArmLeft,
            (16.0, 15.0),
        ),
    ] {
        let arm = b.part_named(name, pivot, [0.0; 3], anim, 1.0, None);
        b.cube(arm, 0, uv, [-1.0, -0.5, -1.0], [2.0, 5.0, 2.0], NONE);
    }
    for (pivot, anim, uv) in [
        ([-1.0, 21.5, 0.0], Anim::LegRight, (8.0, 23.0)),
        ([1.0, 21.5, 0.0], Anim::LegLeft, (0.0, 23.0)),
    ] {
        let leg = b.part(pivot, anim, 1.0);
        b.cube(leg, 0, uv, [-1.0, -0.5, -1.0], [2.0, 3.0, 2.0], NONE);
    }
    b.finish(1.0)
}

/// M326 — `BabySquidModel.createBodyLayer()` (32 x 32): an 8 x 10 x 8 body
/// and eight 2 x 6 x 2 tentacles hanging from a radius-3 ring, each yawed to
/// face outward and waving as the adult's do. The glow squid's baby is the
/// same layer.
fn squid_baby() -> Model {
    let mut b = ModelBuilder::new();
    b.cube_f(
        STATIC_PART,
        0,
        (0.0, 0.0),
        [-4.0, -5.0, -4.0],
        [8.0, 10.0, 8.0],
        0.0,
        false,
        &[Fold::at([0.0, 13.0, 0.0])],
    );
    for i in 0..8 {
        let angle = i as f32 * PI * 2.0 / 8.0;
        let pivot = [angle.cos() * 3.0, 18.5, angle.sin() * 3.0];
        let y_rot = -(i as f32) * PI * 2.0 / 8.0 + PI / 2.0;
        let p = b.part_ext(
            pivot,
            [0.0, y_rot, 0.0],
            Anim::SquidTentacle,
            1.0,
            Option::None,
        );
        b.cube(p, 0, (0.0, 18.0), [-1.0, -0.5, -1.0], [2.0, 6.0, 2.0], NONE);
    }
    b.finish(1.0)
}

/// M327 — `BabyHoglinModel.createBodyLayer()` (64 x 64): a body grown 0.02
/// with its 0-wide mane, a head tilted 0.8727 (10 x 4 x 12 with two tusks and
/// two ears; since M367 turning with the look, its pitch held, as the
/// adult's), and four 3 x 6 x 3 legs on the adult's walk. The ears' `PartPose` angle is not what draws:
/// `HoglinModel.setupAnim` sets their `zRot` to ∓2π/9 every frame, so they
/// rest there as the adult's do. The zoglin's baby is the same layer.
fn hoglin_baby() -> Model {
    let mut b = ModelBuilder::new();
    let body = [Fold::at([0.0, 24.0, 0.0])];
    b.cube_f(
        STATIC_PART,
        0,
        (0.0, 16.0),
        [-4.0, -14.0, -7.0],
        [8.0, 8.0, 14.0],
        0.02,
        false,
        &body,
    );
    b.cube_f(
        STATIC_PART,
        0,
        (24.0, 39.0),
        [0.0, -18.0, -8.0],
        [0.0, 6.0, 11.0],
        0.02,
        false,
        &body,
    );
    // M367 — `HoglinModel.setupAnim` assigns the head's `xRot` (0.87266463
    // at rest, the layer's 0.8727 never drawn) and the ears' `zRot`.
    let head = b.part_ext(
        [0.0, 13.0, -7.0],
        [0.87266463, 0.0, 0.0],
        Anim::HoglinHead { baby: true },
        1.0,
        Option::None,
    );
    for (uv, min, dims) in [
        ((0.0, 0.0), [-5.0, -2.2605, -10.547], [10.0, 4.0, 12.0]),
        ((44.0, 29.0), [-7.0, -4.0981, -8.4879], [2.0, 5.0, 2.0]),
        ((52.0, 29.0), [5.0, -4.0981, -8.4879], [2.0, 5.0, 2.0]),
    ] {
        b.cube(head, 0, uv, min, dims, NONE);
    }
    for (x, z_rot, uv, min_x, mirror, left) in [
        (-5.0, -PI * 2.0 / 9.0, (32.0, 5.0), -5.1, false, false),
        (5.0, PI * 2.0 / 9.0, (32.0, 0.0), -0.9, true, true),
    ] {
        let ear = b.part_ext(
            [x, -1.0, -1.5],
            [0.0, 0.0, z_rot],
            Anim::HoglinEar { left },
            1.0,
            Some(head),
        );
        b.cube_f(
            ear,
            0,
            uv,
            [min_x, -0.5, -2.0],
            [6.0, 1.0, 4.0],
            0.0,
            mirror,
            NONE,
        );
    }
    for (pivot, front, left, uv) in [
        ([-2.5, 18.0, 4.5], false, false, (0.0, 47.0)),
        ([2.5, 18.0, 4.5], false, true, (12.0, 47.0)),
        ([-2.5, 18.0, -4.5], true, false, (0.0, 38.0)),
        ([2.5, 18.0, -4.5], true, true, (12.0, 38.0)),
    ] {
        let leg = b.part(pivot, Anim::HoglinLeg { front, left }, 1.0);
        b.cube(leg, 0, uv, [-1.5, 0.0, -1.5], [3.0, 6.0, 3.0], NONE);
    }
    b.finish(1.0)
}

/// M328 — `BabyStriderModel.createBodyLayer()` (32 x 32): a 7 x 7 x 8 body
/// with three 7 x 3 x 0 bristles across its top, and two 2 x 4 x 2 legs;
/// since M366 all of them move as `StriderModel` moves them. The body is placed where
/// `customAnimations` puts it, not where the layer does: it sets `body.y` to
/// 17.25 every frame (the layer says 16.75), so at rest the body sits 0.5 px
/// into the legs.
fn strider_baby() -> Model {
    let mut b = ModelBuilder::new();
    let body = b.part([0.0, 17.25, 0.0], Anim::StriderBody { adult: false }, 1.0);
    b.cube(
        body,
        0,
        (0.0, 0.0),
        [-3.5, -3.75, -4.0],
        [7.0, 7.0, 8.0],
        NONE,
    );
    // `bristle2` (front) is `animateBristle`'s first, `bristle0` its third.
    for (z, uv, order) in [
        (2.0, (0.0, 21.0), 2u8),
        (0.0, (0.0, 18.0), 1),
        (-2.0, (0.0, 15.0), 0),
    ] {
        let p = b.part_ext(
            [0.0, -4.25, z],
            [0.0; 3],
            Anim::StriderBristle { order, baby: true },
            1.0,
            Some(body),
        );
        b.cube(p, 0, uv, [-3.5, -2.5, 0.0], [7.0, 3.0, 0.0], NONE);
    }
    for (x, left, uv) in [(-1.5, false, (0.0, 24.0)), (1.5, true, (8.0, 24.0))] {
        let leg = b.part([x, 20.0, 0.0], Anim::StriderLeg { left }, 1.0);
        b.cube(leg, 0, uv, [-1.0, 0.0, -1.0], [2.0, 4.0, 2.0], NONE);
    }
    b.finish(1.0)
}

/// M330 — `BabyFoxModel.createBodyLayer()` (32 x 32): a 6 x 5 x 5 head
/// (looking) with its snout and two ears, a 5 x 4 x 6 body with its tail,
/// and four 2 x 2 x 2 legs. The kit has no procedural gait:
/// `FoxModel.setWalkingPose` moves no leg, and `BabyFoxModel` adds
/// `FOX_BABY_WALK` through `applyWalk(pos, speed, 1.0, 2.5)`, so every part
/// is named for that rig. Its four leg `SCALE` channels (a 1.15 stretch) are
/// dropped, as the generator drops every scale channel; sitting and sleeping
/// are not posed, as Rewo's adult fox's are not.
fn fox_baby() -> Model {
    let mut b = ModelBuilder::new();
    let head = b.part_named(
        "head",
        [0.0, 18.125, 0.125],
        [0.0; 3],
        Anim::Head,
        1.0,
        None,
    );
    for (uv, min, dims) in [
        ((0.0, 0.0), [-3.0, -2.125, -5.125], [6.0, 5.0, 5.0]),
        ((18.0, 20.0), [-1.0, 0.875, -7.125], [2.0, 2.0, 2.0]),
        ((22.0, 8.0), [-3.0, -4.125, -4.125], [2.0, 2.0, 1.0]),
        ((22.0, 11.0), [1.0, -4.125, -4.125], [2.0, 2.0, 1.0]),
    ] {
        b.cube(head, 0, uv, min, dims, NONE);
    }
    for (name, pivot, uv) in [
        ("right_hind_leg", [-1.5, 22.0, 4.0], (22.0, 4.0)),
        ("left_hind_leg", [1.5, 22.0, 4.0], (22.0, 0.0)),
        ("right_front_leg", [-1.5, 22.0, 0.0], (22.0, 4.0)),
        ("left_front_leg", [1.5, 22.0, 0.0], (22.0, 0.0)),
    ] {
        let leg = b.part_named(name, pivot, [0.0; 3], Anim::None, 1.0, None);
        b.cube(leg, 0, uv, [-1.0, 0.0, -1.0], [2.0, 2.0, 2.0], NONE);
    }
    let body = b.part_named("body", [0.0, 20.0, 2.0], [0.0; 3], Anim::None, 1.0, None);
    b.cube(
        body,
        0,
        (0.0, 10.0),
        [-2.5, -2.0, -3.0],
        [5.0, 4.0, 6.0],
        NONE,
    );
    let tail = b.part_named(
        "tail",
        [0.0, -0.5, 3.0],
        [0.0; 3],
        Anim::None,
        1.0,
        Some(body),
    );
    b.cube(
        tail,
        0,
        (0.0, 20.0),
        [-1.5, -1.48, -1.0],
        [3.0, 3.0, 6.0],
        NONE,
    );
    b.keyframe(
        &crate::anim_defs::FOX_BABY_WALK,
        KfDriver::Walk {
            speed_f: 1.0,
            scale_f: 2.5,
        },
    );
    b.finish(1.0)
}

/// M331 — `BabyPandaModel.createBodyLayer()` (64 x 64): a 9 x 7 x 11 body
/// authored level, a 7 x 6 x 5 head (looking) with its muzzle and two ears,
/// and four 3 x 2 x 3 legs on the quadruped walk. Sitting, rolling and the
/// other panda poses are not drawn, as Rewo's adult panda's are not.
fn panda_baby() -> Model {
    let mut b = ModelBuilder::new();
    b.cube_f(
        STATIC_PART,
        0,
        (0.0, 11.0),
        [-4.5, -3.5, -5.5],
        [9.0, 7.0, 11.0],
        0.0,
        false,
        &[Fold::at([0.0, 18.5, 2.5])],
    );
    let head = b.part([0.0, 19.0, -3.0], Anim::Head, 1.0);
    for (uv, min, dims) in [
        ((0.0, 0.0), [-3.5, -3.0, -5.0], [7.0, 6.0, 5.0]),
        ((24.0, 6.0), [-2.0, 1.0, -6.0], [4.0, 2.0, 1.0]),
        ((24.0, 0.0), [-4.5, -4.0, -3.5], [3.0, 3.0, 1.0]),
        ((33.0, 0.0), [1.5, -4.0, -3.5], [3.0, 3.0, 1.0]),
    ] {
        b.cube(head, 0, uv, min, dims, NONE);
    }
    for (pivot, anim, uv) in [
        ([-3.0, 22.0, 6.5], Anim::QuadHindRight, (0.0, 34.0)),
        ([3.0, 22.0, 6.5], Anim::QuadHindLeft, (12.0, 34.0)),
        ([-3.0, 22.0, -1.5], Anim::QuadFrontRight, (0.0, 29.0)),
        ([3.0, 22.0, -1.5], Anim::QuadFrontLeft, (12.0, 29.0)),
    ] {
        let leg = b.part(pivot, anim, 1.0);
        b.cube(leg, 0, uv, [-1.5, 0.0, -1.5], [3.0, 2.0, 3.0], NONE);
    }
    b.finish(1.0)
}

/// M332 — `BabyFelineModel.createBabyLayer()` (32 x 32), the ocelot
/// kitten's (and, once Rewo draws cat variants for kittens, the cat's): a
/// 5 x 4 x 4 head (looking) with its ears and nose, a 4 x 3 x 7 body, a tail
/// tilted -0.5672 (its second segment has no box), and four 1 x 2 x 2 legs.
///
/// The legs follow `BabyFelineModel.setupAnim`'s walk exactly: amplitude 1.0,
/// the left hind and right front leg in phase on `cos(f)`, the other two on
/// `cos(f + π)` (M357: `Anim::FelineLeg`, which says so by name). M357 also
/// poses it sitting; crouching, sprinting and lying down are not posed.
fn feline_baby() -> Model {
    let mut b = ModelBuilder::new();
    let head = b.part([0.0, 20.0, -3.125], Anim::FelineHead, 1.0);
    for (uv, min, dims) in [
        ((0.0, 0.0), [-2.5, -3.0, -2.875], [5.0, 4.0, 4.0]),
        ((18.0, 0.0), [-2.0, -4.0, -0.875], [1.0, 1.0, 2.0]),
        ((24.0, 0.0), [1.0, -4.0, -0.875], [1.0, 1.0, 2.0]),
        ((18.0, 3.0), [-1.5, -1.0, -3.875], [3.0, 2.0, 1.0]),
    ] {
        b.cube(head, 0, uv, min, dims, NONE);
    }
    // M357 — the body and the tail are parts: sitting moves them.
    let body = b.part_ext(
        [0.0, 20.5, 0.5],
        [0.0; 3],
        Anim::FelineBody,
        1.0,
        Option::None,
    );
    b.cube(
        body,
        0,
        (0.0, 8.0),
        [-2.0, -1.5, -3.5],
        [4.0, 3.0, 7.0],
        NONE,
    );
    let tail1 = b.part_ext(
        [0.0, 19.107, 3.9151],
        [-0.567232, 0.0, 0.0],
        Anim::FelineTail1,
        1.0,
        Option::None,
    );
    b.cube(
        tail1,
        0,
        (0.0, 18.0),
        [-0.5, -0.107, 0.0849],
        [1.0, 1.0, 5.0],
        NONE,
    );
    for (pivot, hind, left, uv) in [
        ([1.0, 22.0, -1.5], false, true, (18.0, 18.0)),
        ([-1.0, 22.0, -1.5], false, false, (12.0, 18.0)),
        ([1.0, 22.0, 2.5], true, true, (18.0, 22.0)),
        ([-1.0, 22.0, 2.5], true, false, (12.0, 22.0)),
    ] {
        let leg = b.part(pivot, Anim::FelineLeg { hind, left }, 1.0);
        b.cube(leg, 0, uv, [-0.5, 0.0, -1.0], [1.0, 2.0, 2.0], NONE);
    }
    b.finish(1.0)
}

/// M333 — `BabyBeeModel.createBodyLayer()` (32 x 32), in the airborne pose
/// Rewo's adult bee always takes: on the hovering `bone`, two antennae, a
/// 4 x 4 x 5 body with its 0-wide stinger, two 3 x 0 x 3 wings and three
/// 3 x 1 x 0 leg plates. In flight `BeeModel.setupAnim` resets the wings'
/// `yRot` splay to 0 and flutters them about z, keeping the right wing's
/// 0.2182 tilt on both, and sets every leg plate's `xRot` to π/4. The left
/// wing's `texOffs(-3, 9)` only reaches its zero-area faces, which are not
/// drawn.
fn bee_baby() -> Model {
    let mut b = ModelBuilder::new();
    let bone = b.part_named(
        "bone",
        [0.0, 19.6667, -1.8567],
        [0.0; 3],
        Anim::BeeBone,
        1.0,
        Option::None,
    );
    for (uv, min) in [
        ((6.0, 12.0), [1.0, -1.6667, -2.1633]),
        ((0.0, 12.0), [-2.0, -1.6667, -2.1933]),
    ] {
        b.cube(bone, 0, uv, min, [1.0, 2.0, 2.0], NONE);
    }
    let body = Fold::at([0.0, 1.3333, 2.3567]);
    b.cube_f(
        bone,
        0,
        (0.0, 0.0),
        [-2.0, -2.0, -2.5],
        [4.0, 4.0, 5.0],
        0.0,
        false,
        &[body],
    );
    // The stinger is the body's child: the body's offset plus its own.
    let stinger = b.part_named(
        "stinger",
        [0.0, 1.3333 + 0.5, 2.3567 + 2.5],
        [0.0; 3],
        Anim::None,
        1.0,
        Some(bone),
    );
    b.show(stinger, Show::HasStinger);
    b.cube(
        stinger,
        0,
        (13.0, 2.0),
        [0.0, -0.5, 0.0],
        [0.0, 1.0, 1.0],
        NONE,
    );
    for (name, x, left, uv, min_x) in [
        ("right_wing", -1.0, false, (3.0, 9.0), -3.0),
        ("left_wing", 1.0, true, (-3.0, 9.0), 0.0),
    ] {
        let wing = b.part_named(
            name,
            [x, -0.6667, 0.8567],
            [0.2182, 0.0, 0.0],
            Anim::BeeWing { left },
            1.0,
            Some(bone),
        );
        b.cube_f(
            wing,
            0,
            uv,
            [min_x, 0.0, 0.0],
            [3.0, 0.0, 3.0],
            0.0,
            left,
            NONE,
        );
    }
    for (name, z, uv, anim) in [
        ("front_legs", 1.8567, (13.0, 0.0), Anim::BeeFrontLegs),
        ("middle_legs", 2.8567, (13.0, 1.0), Anim::None),
        ("back_legs", 3.8567, (13.0, 2.0), Anim::BeeBackLegs),
    ] {
        let leg = b.part_named(
            name,
            [0.0, 3.3333, z],
            [PI / 4.0, 0.0, 0.0],
            anim,
            1.0,
            Some(bone),
        );
        b.cube(leg, 0, uv, [-1.5, 0.0, 0.0], [3.0, 1.0, 0.0], NONE);
    }
    b.finish(1.0)
}

/// M334 — `BabyDonkeyModel.createBabyLayer()` (64 x 64), the donkey's and
/// the mule's: an 8 x 6 x 14 body, a tail tilted -0.7418, four 3 x 8 x 3 legs
/// (children of the body; it never moves, so their pivots are the body's
/// offset plus theirs) on the quadruped walk as Rewo's adult equine's, and
/// the neck, head and ears. `BabyDonkeyModel.setupAnim` forces the look pitch
/// to -30 degrees, so `head_parts`' π/6 base cancels and the neck rests at its
/// own 0.3927 tilt; the head does not look, as Rewo's adult's does not. The
/// chest parts carry no box (a foal carries no chest). M339: the box-less
/// `tail` part rests where `AbstractEquineModel.setupAnim` puts it,
/// `getTailXRotOffset()` (-π/4 for this model) + π/6 = -π/12, under its
/// box's own -0.7418; M334 had left that term out.
fn donkey_baby() -> Model {
    let mut b = ModelBuilder::new();
    let body = Fold::at([1.0, 14.0, 0.0]);
    b.cube_f(
        STATIC_PART,
        0,
        (0.0, 13.0),
        [-5.0, -3.0, -7.0],
        [8.0, 6.0, 14.0],
        0.0,
        false,
        &[body],
    );
    b.cube_f(
        STATIC_PART,
        0,
        (24.0, 33.0),
        [-2.5, -1.0, -0.5],
        [3.0, 3.0, 8.0],
        0.0,
        false,
        &[
            Fold::rot([-0.7418, 0.0, 0.0], [0.0, 0.0, 0.0]),
            Fold::rot([-PI / 12.0, 0.0, 0.0], [0.0, -1.5, 6.5]),
            body,
        ],
    );
    let head_parts = Fold::at([0.0, -3.0, -5.0]);
    b.cube_f(
        STATIC_PART,
        0,
        (30.0, 9.0),
        [-3.0, -6.0, -3.0],
        [4.0, 8.0, 4.0],
        0.0,
        false,
        &[
            Fold::rot([0.3927, 0.0, 0.0], [0.0, 0.0, 0.0]),
            head_parts,
            body,
        ],
    );
    let head = Fold::at([0.0, -5.0, -3.0]);
    b.cube_f(
        STATIC_PART,
        0,
        (0.0, 0.0),
        [-4.0, -3.6, -8.4],
        [6.0, 4.0, 9.0],
        0.0,
        false,
        &[
            Fold::rot([0.3927, 0.0, 0.0], [0.0, -1.0, 1.0]),
            head,
            head_parts,
            body,
        ],
    );
    for (x, z_rot, uv, mirror) in [
        (2.0, 0.48, (0.0, 0.0), false),
        (-2.0, -0.48, (22.0, 0.0), true),
    ] {
        b.cube_f(
            STATIC_PART,
            0,
            uv,
            [-2.0, -6.5, -0.3],
            [2.0, 7.0, 1.0],
            0.0,
            mirror,
            &[
                Fold::rot([0.48, 0.0, z_rot], [x, -3.5, -1.0]),
                head,
                head_parts,
                body,
            ],
        );
    }
    for (pivot, anim, uv) in [
        ([3.25, 17.5, 5.25], Anim::QuadHindLeft, (12.0, 44.0)),
        ([-1.4, 17.5, 5.4], Anim::QuadHindRight, (0.0, 44.0)),
        ([3.4, 17.5, -5.3], Anim::QuadFrontLeft, (12.0, 33.0)),
        ([-1.4, 17.5, -5.4], Anim::QuadFrontRight, (0.0, 33.0)),
    ] {
        let leg = b.part(pivot, anim, 1.0);
        b.cube(leg, 0, uv, [-2.5, -1.5, -1.5], [3.0, 8.0, 3.0], NONE);
    }
    b.finish(1.0)
}

/// M335 — `BabyCamelModel.createBodyLayer()` (64 x 64): a 9 x 8 x 16 body
/// carrying its 3 x 9 x 0 tail and the head (a 5 x 5 x 7 neck base, a
/// 5 x 9 x 5 neck and a 5 x 4 x 3 muzzle; looking) with two ears, and four
/// 3 x 13 x 3 legs, all on `CamelBabyAnimation.CAMEL_BABY_WALK` through
/// `CamelModel`'s `applyWalk(pos, speed, 2.0, 2.5)`. Its channels name the
/// body, head, ears, tail, legs and `root` (which, as for Rewo's adult camel,
/// is a part at the origin parenting the rest). Scale channels are dropped,
/// as everywhere; sitting, standing up and dashing are not drawn, as Rewo's
/// adult camel's are not.
fn camel_baby() -> Model {
    let mut b = ModelBuilder::new();
    let root = b.part_named("root", [0.0; 3], [0.0; 3], Anim::None, 1.0, None);
    let body = b.part_named(
        "body",
        [0.0, 7.0, 0.0],
        [0.0; 3],
        Anim::None,
        1.0,
        Some(root),
    );
    b.cube(
        body,
        0,
        (0.0, 14.0),
        [-4.5, -4.0, -8.0],
        [9.0, 8.0, 16.0],
        NONE,
    );
    let tail = b.part_named(
        "tail",
        [0.0, -1.5, 8.05],
        [0.0; 3],
        Anim::None,
        1.0,
        Some(body),
    );
    b.cube(
        tail,
        0,
        (50.0, 38.0),
        [-1.5, -0.5, 0.0],
        [3.0, 9.0, 0.0],
        NONE,
    );
    let head = b.part_named(
        "head",
        [0.0, 1.0, -7.5],
        [0.0; 3],
        Anim::CamelHead,
        1.0,
        Some(body),
    );
    for (uv, min, dims) in [
        ((20.0, 0.0), [-2.5, -3.0, -7.5], [5.0, 5.0, 7.0]),
        ((0.0, 0.0), [-2.5, -12.0, -7.5], [5.0, 9.0, 5.0]),
        ((0.0, 14.0), [-2.5, -12.0, -10.5], [5.0, 4.0, 3.0]),
    ] {
        b.cube(head, 0, uv, min, dims, NONE);
    }
    for (name, x, uv, min_x) in [
        ("right_ear", -2.5, (37.0, 0.0), -3.0),
        ("left_ear", 2.5, (47.0, 0.0), 0.0),
    ] {
        let ear = b.part_named(
            name,
            [x, -11.0, -4.0],
            [0.0; 3],
            Anim::None,
            1.0,
            Some(head),
        );
        b.cube(ear, 0, uv, [min_x, -0.5, -1.0], [3.0, 1.0, 2.0], NONE);
    }
    for (name, pivot, uv) in [
        ("right_front_leg", [-3.0, 11.5, -5.5], (36.0, 14.0)),
        ("left_front_leg", [3.0, 11.5, -5.5], (48.0, 14.0)),
        ("left_hind_leg", [3.0, 11.5, 5.5], (12.0, 38.0)),
        ("right_hind_leg", [-3.0, 11.5, 5.5], (0.0, 38.0)),
    ] {
        let leg = b.part_named(name, pivot, [0.0; 3], Anim::None, 1.0, Some(root));
        b.cube(leg, 0, uv, [-1.5, -0.5, -1.5], [3.0, 13.0, 3.0], NONE);
    }
    b.keyframe(
        &crate::anim_defs::CAMEL_BABY_WALK,
        KfDriver::Walk {
            speed_f: 2.0,
            scale_f: 2.5,
        },
    );
    b.finish(1.0)
}

/// M336 — `BabyRabbitModel.createBodyLayer()` (32 x 32): a body (its box on
/// `body_r1`, tilted -0.5236) carrying the tail, the head (5 x 4 x 4, looking:
/// `RabbitModel.setupAnim` turns it unless the idle tilt plays, which Rewo
/// does not) with two ears, and the front legs (each part tilted 0.3927 over
/// a box tilted back -0.3927); the hind legs hang from `backlegs`, each turned
/// π about y over a haunch turned ∓0.7854. Every part `BabyRabbitAnimation
/// .HOP` names is a real part; the `_r1` and haunch children, which it never
/// names, are folds. The hop plays as the adult's does
/// (`AgeGatedByWalk { gate: 5.0 }`).
fn rabbit_baby() -> Model {
    let mut b = ModelBuilder::new();
    let body = b.part_named("body", [0.0, 23.0, 1.6], [0.0; 3], Anim::None, 1.0, None);
    b.cube_f(
        body,
        0,
        (0.0, 8.0),
        [-2.0, -2.0, -3.0],
        [4.0, 3.0, 6.0],
        0.0,
        false,
        &[Fold::rot([-0.5236, 0.0, 0.0], [0.0, -2.0, -1.6])],
    );
    let tail = b.part_named(
        "tail",
        [0.0, -2.2, 2.0],
        [0.0; 3],
        Anim::None,
        1.0,
        Some(body),
    );
    b.cube_f(
        tail,
        0,
        (0.0, 21.0),
        [-1.4, -2.0268, -1.0177],
        [3.0, 3.0, 3.0],
        0.0,
        false,
        &[Fold::rot([-0.5236, 0.0, 0.0], [-0.1, 0.0, 0.0])],
    );
    let head = b.part_named(
        "head",
        [0.0, -5.0, -2.6],
        [0.0; 3],
        Anim::Head,
        1.0,
        Some(body),
    );
    b.cube(
        head,
        0,
        (0.0, 0.0),
        [-2.5, -3.0, -3.0],
        [5.0, 4.0, 4.0],
        NONE,
    );
    for (name, x, uv) in [
        ("right_ear", -1.5, (18.0, 0.0)),
        ("left_ear", 1.5, (24.0, 0.0)),
    ] {
        let ear = b.part_named(name, [x, -3.5, -0.5], [0.0; 3], Anim::None, 1.0, Some(head));
        b.cube(ear, 0, uv, [-1.0, -3.5, -0.5], [2.0, 4.0, 1.0], NONE);
    }
    let front = b.part_named(
        "frontlegs",
        [0.0, -2.5, -2.6],
        [0.0; 3],
        Anim::None,
        1.0,
        Some(body),
    );
    for (name, x, uv) in [
        ("left_front_leg", 1.0, (18.0, 8.0)),
        ("right_front_leg", -1.0, (14.0, 8.0)),
    ] {
        let leg = b.part_named(
            name,
            [x, 1.0, -0.5],
            [0.3927, 0.0, 0.0],
            Anim::None,
            1.0,
            Some(front),
        );
        b.cube_f(
            leg,
            0,
            uv,
            [-0.5, -1.5, -0.5],
            [1.0, 3.0, 1.0],
            0.0,
            false,
            &[Fold::rot([-0.3927, 0.0, 0.0], [0.0, 1.0, 0.0])],
        );
    }
    let back = b.part_named(
        "backlegs",
        [0.0, 23.0, 2.0],
        [0.0; 3],
        Anim::None,
        1.0,
        None,
    );
    for (name, x, uv, haunch_y, haunch_off) in [
        ("left_hind_leg", 1.5, (10.0, 17.0), -0.7854, [1.0, 0.0, 0.5]),
        (
            "right_hind_leg",
            -1.5,
            (0.0, 17.0),
            0.7854,
            [0.5, 0.0, -0.9],
        ),
    ] {
        let leg = b.part_named(
            name,
            [x, 0.5, 0.5],
            [0.0, 3.1416, 0.0],
            Anim::None,
            1.0,
            Some(back),
        );
        b.cube_f(
            leg,
            0,
            uv,
            [-2.0, -0.5, 0.0],
            [2.0, 1.0, 3.0],
            0.0,
            false,
            &[Fold::rot([0.0, haunch_y, 0.0], haunch_off)],
        );
    }
    b.keyframe(
        &crate::anim_defs::BABY_RABBIT_HOP,
        KfDriver::AgeGatedByWalk { gate: 5.0 },
    );
    b.finish(1.0)
}

/// M337 — `BabyGoatModel.createBodyLayer()` (64 x 64): four 2 x 5 x 2 legs on
/// the quadruped walk (named as Rewo's adult goat's), a two-box body, and the
/// head: a 4 x 4 x 6 skull, two horns tilted -π/8, two ears turned ±0.5236
/// about y, and `HeadMain`, the skull's own box again (vanilla authors it
/// twice, coplanar). The layer tilts the head 0.4363, but `setupAnim` sets its
/// `xRot` to π/8 every frame after the look's pitch, so the head rests at π/8
/// and only its yaw follows the look (`Anim::HeadYaw`). Ramming and the horns'
/// visibility are not drawn, as Rewo's adult goat's are not.
fn goat_baby() -> Model {
    let mut b = ModelBuilder::new();
    for (pivot, anim, uv) in [
        ([1.5, 19.5, 3.0], Anim::QuadHindLeft, (29.0, 12.0)),
        ([-1.5, 19.5, 3.0], Anim::QuadHindRight, (21.0, 12.0)),
        ([-1.5, 19.5, -2.0], Anim::QuadFrontRight, (21.0, 5.0)),
        ([1.5, 19.5, -2.0], Anim::QuadFrontLeft, (29.0, 5.0)),
    ] {
        let leg = b.part(pivot, anim, 1.0);
        b.cube(leg, 0, uv, [-1.0, -0.5, -1.0], [2.0, 5.0, 2.0], NONE);
    }
    let body = [Fold::at([0.0, 17.8, 0.0])];
    b.cube_f(
        STATIC_PART,
        0,
        (0.0, 10.0),
        [-3.0, -2.3, -4.5],
        [6.0, 5.0, 9.0],
        0.0,
        false,
        &body,
    );
    b.cube_f(
        STATIC_PART,
        0,
        (0.0, 24.0),
        [-2.5, -2.2, -4.0],
        [5.0, 4.0, 8.0],
        0.0,
        false,
        &body,
    );
    let head = b.part_ext(
        [0.0, 15.5, -3.0],
        [PI / 8.0, 0.0, 0.0],
        Anim::HeadYaw,
        1.0,
        Option::None,
    );
    b.cube(
        head,
        0,
        (0.0, 0.0),
        [-2.0, -3.8126, -5.1548],
        [4.0, 4.0, 6.0],
        NONE,
    );
    let horn = Fold::rot([-PI / 8.0, 0.0, 0.0], [-1.5, -1.5, -1.0]);
    for min_x in [0.0, 2.0] {
        b.cube_m(
            head,
            0,
            (24.0, 0.0),
            [min_x, -4.5, 0.0],
            [1.0, 2.0, 1.0],
            &[horn],
        );
    }
    for (x, y_rot, min_x, mirror) in [(-1.7, -0.5236, -2.0, true), (1.7, 0.5236, 0.0, false)] {
        b.cube_f(
            head,
            0,
            (0.0, 12.0),
            [min_x, -0.5, -0.5],
            [2.0, 1.0, 1.0],
            0.0,
            mirror,
            &[Fold::rot([0.0, y_rot, 0.0], [x, -2.3126, 0.1452])],
        );
    }
    b.cube_f(
        head,
        0,
        (0.0, 0.0),
        [-2.0, -2.5, -4.0],
        [4.0, 4.0, 6.0],
        0.0,
        false,
        &[Fold::at([0.0, -1.3126, -1.1548])],
    );
    b.finish(1.0)
}

/// M338 — `BabyLlamaModel.createBodyLayer(CubeDeformation.NONE)` (64 x 64):
/// the head (an 6 x 11 x 4 neck-and-skull, its muzzle and two ears; looking),
/// an 8 x 6 x 13 body authored level and four 3 x 8 x 3 legs on
/// `LlamaModel`'s walk, which is the quadruped's leg for leg. The two chest
/// boxes are left out: `LlamaRenderer` sets `hasChest` only for an adult, so
/// a baby never shows them.
fn llama_baby() -> Model {
    let mut b = ModelBuilder::new();
    let head = b.part([0.0, 12.0, -4.0], Anim::Head, 1.0);
    for (uv, min, dims) in [
        ((0.0, 0.0), [-3.0, -9.0, -4.0], [6.0, 11.0, 4.0]),
        ((0.0, 15.0), [-1.5, -7.0, -7.0], [3.0, 3.0, 3.0]),
        ((20.0, 4.0), [0.5, -11.0, -3.0], [2.0, 2.0, 2.0]),
        ((20.0, 0.0), [-2.5, -11.0, -3.0], [2.0, 2.0, 2.0]),
    ] {
        b.cube(head, 0, uv, min, dims, NONE);
    }
    b.cube_f(
        STATIC_PART,
        0,
        (0.0, 15.0),
        [-4.0, -3.0, -8.5],
        [8.0, 6.0, 13.0],
        0.0,
        false,
        &[Fold::at([0.0, 14.0, 2.5])],
    );
    for (pivot, anim, uv, min_x) in [
        ([-2.5, 16.5, 4.5], Anim::QuadHindRight, (0.0, 45.0), -1.4),
        ([2.5, 16.5, 4.5], Anim::QuadHindLeft, (12.0, 45.0), -1.6),
        ([-2.5, 16.5, -3.5], Anim::QuadFrontRight, (0.0, 34.0), -1.4),
        ([2.5, 16.5, -3.5], Anim::QuadFrontLeft, (12.0, 34.0), -1.6),
    ] {
        let leg = b.part(pivot, anim, 1.0);
        b.cube(leg, 0, uv, [min_x, -0.5, -1.5], [3.0, 8.0, 3.0], NONE);
    }
    b.finish(1.0)
}

/// M339 — `BabyHorseModel.createBabyMesh(CubeDeformation.NONE)` (64 x 64):
/// an 8 x 7 x 14 body with its tail, four 3 x 9 x 3 legs on the quadruped
/// walk as Rewo's adult equine's, and the neck, head and two ears. Two rest
/// angles are `AbstractEquineModel.setupAnim`'s, not the layer's: the neck
/// (`head_parts`) at π/6 (the layer says 0.6109), and the tail at
/// `getTailXRotOffset()` (-π/2 here) + π/6 = -π/3 (the layer says -0.7418).
/// The head does not look, as the adult's does not; markings are not drawn,
/// as the adult's are not; a foal wears no saddle or armour.
fn horse_baby() -> Model {
    let mut b = ModelBuilder::new();
    let body = Fold::at([0.0, 12.5, 0.0]);
    b.cube_f(
        STATIC_PART,
        0,
        (0.0, 13.0),
        [-4.0, -3.5, -7.0],
        [8.0, 7.0, 14.0],
        0.0,
        false,
        &[body],
    );
    b.cube_f(
        STATIC_PART,
        0,
        (24.0, 34.0),
        [-1.5, -1.5, -1.0],
        [3.0, 3.0, 8.0],
        0.0,
        false,
        &[Fold::rot([-PI / 3.0, 0.0, 0.0], [0.0, -1.0, 7.0]), body],
    );
    let neck = Fold::rot([PI / 6.0, 0.0, 0.0], [0.0, 10.0, -6.0]);
    b.cube_f(
        STATIC_PART,
        0,
        (30.0, 0.0),
        [-2.0, -6.0, -2.0],
        [4.0, 8.0, 4.0],
        0.0,
        false,
        &[neck],
    );
    let head = Fold::at([0.0, -6.0516, -0.2951]);
    b.cube_f(
        STATIC_PART,
        0,
        (0.0, 0.0),
        [-3.0, -3.9484, -6.705],
        [6.0, 4.0, 9.0],
        0.0,
        false,
        &[head, neck],
    );
    for (x, z, z_rot, uv, min_z) in [
        (2.0, 1.9451, 0.2618, (0.0, 4.0), -0.8),
        (-2.0, 1.645, -0.2618, (0.0, 0.0), -0.5),
    ] {
        b.cube_f(
            STATIC_PART,
            0,
            uv,
            [-1.0, -2.5, min_z],
            [2.0, 3.0, 1.0],
            0.0,
            false,
            &[Fold::rot([0.0, 0.0, z_rot], [x, -4.2484, z]), head, neck],
        );
    }
    for (pivot, anim, uv) in [
        ([2.4, 16.0, 5.4], Anim::QuadHindLeft, (12.0, 46.0)),
        ([-2.4, 16.0, 5.4], Anim::QuadHindRight, (0.0, 46.0)),
        ([2.4, 16.0, -5.4], Anim::QuadFrontLeft, (12.0, 34.0)),
        ([-2.4, 16.0, -5.4], Anim::QuadFrontRight, (0.0, 34.0)),
    ] {
        let leg = b.part(pivot, anim, 1.0);
        b.cube(leg, 0, uv, [-1.5, -1.0, -1.5], [3.0, 9.0, 3.0], NONE);
    }
    b.finish(1.0)
}

/// M341 — `BabyWolfModel.createBodyLayer()` (32 x 32): a 6 x 5 x 5 head
/// grown 0.025 with its snout and two ears (looking), a 6 x 4 x 8 body,
/// four 2 x 3 x 2 legs on `WolfModel`'s walk (the quadruped's, leg for leg),
/// and the tail. `WolfModel.setupAnim` sets the tail's `xRot` to
/// `state.tailAngle` every frame, π/5 for a wild wolf, not the layer's
/// -0.5236; it rests there and wags about y, as Rewo's adult wolf's does
/// (a tame wolf's tail rises with its health in vanilla, which neither
/// models). Under it `tail_r1` turns its box -3.1.
/// M342 — the puppy with its collar (`WOLF_BABY_COLLAR_LOCATION` on the
/// puppy's own mesh), as [`wolf`].
fn wolf_baby() -> Model {
    with_collar_copy(wolf_baby_mesh(), wolf_baby_mesh())
}

fn wolf_baby_mesh() -> Model {
    let mut b = ModelBuilder::new();
    let head = b.part([0.0, 18.25, -4.0], Anim::Head, 1.0);
    b.cube_g(
        head,
        0,
        (0.0, 12.0),
        [-2.99, -3.25, -3.0],
        [6.0, 5.0, 5.0],
        0.025,
        NONE,
    );
    b.cube(
        head,
        0,
        (17.0, 12.0),
        [-1.5, -0.24, -5.0],
        [3.0, 2.0, 2.0],
        NONE,
    );
    for (x, uv) in [(-2.0, (0.0, 5.0)), (2.0, (20.0, 5.0))] {
        b.cube_f(
            head,
            0,
            uv,
            [-1.0, -1.0, -0.5],
            [2.0, 2.0, 1.0],
            0.0,
            false,
            &[Fold::at([x, -4.25, -0.5])],
        );
    }
    // M356 — the body is a part: the sitting pose moves it.
    let body = b.part_ext(
        [0.0, 19.0, 0.0],
        [0.0; 3],
        Anim::WolfBody,
        1.0,
        Option::None,
    );
    b.cube(
        body,
        0,
        (0.0, 0.0),
        [-3.0, -2.0, -4.0],
        [6.0, 4.0, 8.0],
        NONE,
    );
    for (pivot, anim, uv) in [
        (
            [-1.5, 21.0, 3.0],
            Anim::WolfHind { left: false },
            (0.0, 22.0),
        ),
        ([1.5, 21.0, 3.0], Anim::WolfHind { left: true }, (8.0, 22.0)),
        (
            [-1.5, 21.0, -3.0],
            Anim::WolfFront { left: false },
            (0.0, 0.0),
        ),
        (
            [1.5, 21.0, -3.0],
            Anim::WolfFront { left: true },
            (20.0, 0.0),
        ),
    ] {
        let leg = b.part(pivot, anim, 1.0);
        b.cube(leg, 0, uv, [-1.0, 0.0, -1.0], [2.0, 3.0, 2.0], NONE);
    }
    let tail = b.part_ext(
        [0.0, 19.0, 3.0],
        [PI / 5.0, 0.0, 0.0],
        Anim::TailWagY,
        1.0,
        Option::None,
    );
    b.cube_f(
        tail,
        0,
        (22.0, 16.0),
        [-1.0, -5.7, -1.0],
        [2.0, 6.0, 2.0],
        0.0,
        false,
        &[Fold::rot([-3.1, 0.0, 0.0], [0.0, -0.6, 0.2])],
    );
    b.finish(1.0)
}

fn piglin_normal() -> Model {
    piglin_like(false)
}

fn piglin_zombified() -> Model {
    piglin_like(true)
}

/// `ZombieVillagerModel`: villager-style head (with integral nose) + hat +
/// rim + robed body, zombie forward arms (44,22 rects), villager legs.
fn zombie_villager() -> Model {
    let mut b = ModelBuilder::new();
    let head = b.part([0.0, 0.0, 0.0], Anim::Head, 1.0);
    b.cube(
        head,
        0,
        (0.0, 0.0),
        [-4.0, -10.0, -4.0],
        [8.0, 10.0, 8.0],
        NONE,
    );
    b.cube(
        head,
        0,
        (24.0, 0.0),
        [-1.0, -3.0, -6.0],
        [2.0, 4.0, 2.0],
        NONE,
    );
    b.cube_g(
        head,
        0,
        (32.0, 0.0),
        [-4.0, -10.0, -4.0],
        [8.0, 10.0, 8.0],
        0.5,
        NONE,
    );
    b.cube_f(
        head,
        0,
        (30.0, 47.0),
        [-8.0, -8.0, -6.0],
        [16.0, 16.0, 1.0],
        0.0,
        false,
        &[Fold::rot([-FRAC_PI_2, 0.0, 0.0], [0.0, 0.0, 0.0])],
    );
    b.cube(
        STATIC_PART,
        0,
        (16.0, 20.0),
        [-4.0, 0.0, -3.0],
        [8.0, 12.0, 6.0],
        NONE,
    );
    b.cube_g(
        STATIC_PART,
        0,
        (0.0, 38.0),
        [-4.0, 0.0, -3.0],
        [8.0, 20.0, 6.0],
        0.05,
        NONE,
    );
    // M20: real parts, driven by `animateZombieArms` (was a baked −π/2 fold).
    let arm_r = b.part_named(
        "right_arm",
        [-5.0, 2.0, 0.0],
        [0.0; 3],
        Anim::UndeadArmRight,
        1.0,
        None,
    );
    b.cube(
        arm_r,
        0,
        (44.0, 22.0),
        [-3.0, -2.0, -2.0],
        [4.0, 12.0, 4.0],
        NONE,
    );
    let arm_l = b.part_named(
        "left_arm",
        [5.0, 2.0, 0.0],
        [0.0; 3],
        Anim::UndeadArmLeft,
        1.0,
        None,
    );
    b.cube_m(
        arm_l,
        0,
        (44.0, 22.0),
        [-1.0, -2.0, -2.0],
        [4.0, 12.0, 4.0],
        NONE,
    );
    let leg_r = b.part([-2.0, 12.0, 0.0], Anim::LegRight, 1.0);
    b.cube(
        leg_r,
        0,
        (0.0, 22.0),
        [-2.0, 0.0, -2.0],
        [4.0, 12.0, 4.0],
        NONE,
    );
    let leg_l = b.part([2.0, 12.0, 0.0], Anim::LegLeft, 1.0);
    b.cube_m(
        leg_l,
        0,
        (0.0, 22.0),
        [-2.0, 0.0, -2.0],
        [4.0, 12.0, 4.0],
        NONE,
    );
    b.finish(1.0)
}

/// `BoggedModel`: the skeleton (+ stray-style overlay on texture 1) with
/// mushroom plates sprouting from the head.
fn bogged() -> Model {
    let mut b = ModelBuilder::new();
    let head = skeleton_build(&mut b, Some(0.2));
    let shrooms: [((f32, f32), [f32; 3], [f32; 3], f32); 6] = [
        ((50.0, 16.0), [-3.0, -3.0, 0.0], [3.0, -8.0, 3.0], PI / 4.0),
        (
            (50.0, 16.0),
            [-3.0, -3.0, 0.0],
            [3.0, -8.0, 3.0],
            PI * 3.0 / 4.0,
        ),
        (
            (50.0, 22.0),
            [-3.0, -3.0, 0.0],
            [-3.0, -8.0, -3.0],
            PI / 4.0,
        ),
        (
            (50.0, 22.0),
            [-3.0, -3.0, 0.0],
            [-3.0, -8.0, -3.0],
            PI * 3.0 / 4.0,
        ),
        ((50.0, 28.0), [-3.0, -4.0, 0.0], [-2.0, -1.0, 4.0], 0.0),
        ((50.0, 28.0), [-3.0, -4.0, 0.0], [-2.0, -1.0, 4.0], 0.0),
    ];
    for (i, ((u, v), min, off, yrot)) in shrooms.into_iter().enumerate() {
        // The last two lie flat on the skull (x −π/2 + z ±π/4).
        let rot = if i >= 4 {
            [
                -FRAC_PI_2,
                0.0,
                if i == 4 { PI / 4.0 } else { PI * 3.0 / 4.0 },
            ]
        } else {
            [0.0, yrot, 0.0]
        };
        b.cube_f(
            head,
            0,
            (u, v),
            min,
            [6.0, 4.0, 0.0],
            0.0,
            false,
            &[Fold::rot(rot, off)],
        );
    }
    b.finish(1.0)
}

/// `IllagerModel` (pillager/vindicator/evoker/illusioner share the mesh):
/// big head + hat + nose, robed body, and either crossed arms (idle
/// vindicator/evoker) or hanging arms (crossbow-carrying pillager).
fn illager(crossed: bool) -> Model {
    let mut b = ModelBuilder::new();
    let head = b.part([0.0, 0.0, 0.0], Anim::Head, 1.0);
    b.cube(
        head,
        0,
        (0.0, 0.0),
        [-4.0, -10.0, -4.0],
        [8.0, 10.0, 8.0],
        NONE,
    );
    b.cube_g(
        head,
        0,
        (32.0, 0.0),
        [-4.0, -10.0, -4.0],
        [8.0, 12.0, 8.0],
        0.45,
        NONE,
    );
    b.cube_f(
        head,
        0,
        (24.0, 0.0),
        [-1.0, -1.0, -6.0],
        [2.0, 4.0, 2.0],
        0.0,
        false,
        &[Fold::at([0.0, -2.0, 0.0])],
    );
    b.cube(
        STATIC_PART,
        0,
        (16.0, 20.0),
        [-4.0, 0.0, -3.0],
        [8.0, 12.0, 6.0],
        NONE,
    );
    b.cube_g(
        STATIC_PART,
        0,
        (0.0, 38.0),
        [-4.0, 0.0, -3.0],
        [8.0, 20.0, 6.0],
        0.5,
        NONE,
    );
    // M20: vanilla carries BOTH arm sets on one model and switches them with
    // `arms.visible = crossedArms; left/rightArm.visible = !crossedArms`, so
    // the crossed-arms block is a visibility rule rather than a second model.
    // `crossed` now only picks which set the *neutral* (mobshot) pose shows.
    let folded = Fold::rot([-0.75, 0.0, 0.0], [0.0, 3.0, -1.0]);
    let arms_part = b.part_named("arms", [0.0, 0.0, 0.0], [0.0; 3], Anim::None, 1.0, None);
    b.cube_f(
        arms_part,
        0,
        (44.0, 22.0),
        [-8.0, -2.0, -2.0],
        [4.0, 8.0, 4.0],
        0.0,
        false,
        &[folded],
    );
    b.cube_f(
        arms_part,
        0,
        (44.0, 22.0),
        [4.0, -2.0, -2.0],
        [4.0, 8.0, 4.0],
        0.0,
        true,
        &[folded],
    );
    b.cube_f(
        arms_part,
        0,
        (40.0, 38.0),
        [-4.0, 2.0, -2.0],
        [8.0, 4.0, 4.0],
        0.0,
        false,
        &[folded],
    );
    let arm_r = b.part_named(
        "right_arm",
        [-5.0, 2.0, 0.0],
        [0.0; 3],
        Anim::IllagerArmRight,
        1.0,
        None,
    );
    b.cube(
        arm_r,
        0,
        (40.0, 46.0),
        [-3.0, -2.0, -2.0],
        [4.0, 12.0, 4.0],
        NONE,
    );
    let arm_l = b.part_named(
        "left_arm",
        [5.0, 2.0, 0.0],
        [0.0; 3],
        Anim::IllagerArmLeft,
        1.0,
        None,
    );
    b.cube_m(
        arm_l,
        0,
        (40.0, 46.0),
        [-1.0, -2.0, -2.0],
        [4.0, 12.0, 4.0],
        NONE,
    );
    if crossed {
        b.show(arms_part, Show::IllagerCrossedOnly);
        b.show(arm_r, Show::IllagerNotCrossed);
        b.show(arm_l, Show::IllagerNotCrossed);
    } else {
        // A pillager never folds its arms (its pose is CROSSBOW_* / ATTACKING /
        // NEUTRAL), so the folded set never shows.
        b.show(arms_part, Show::IllagerCrossedOnly);
        b.show(arm_r, Show::Always);
        b.show(arm_l, Show::Always);
    }
    let leg_r = b.part([-2.0, 12.0, 0.0], Anim::LegRight, 0.5);
    b.cube(
        leg_r,
        0,
        (0.0, 22.0),
        [-2.0, 0.0, -2.0],
        [4.0, 12.0, 4.0],
        NONE,
    );
    let leg_l = b.part([2.0, 12.0, 0.0], Anim::LegLeft, 0.5);
    b.cube_m(
        leg_l,
        0,
        (0.0, 22.0),
        [-2.0, 0.0, -2.0],
        [4.0, 12.0, 4.0],
        NONE,
    );
    b.finish(VILLAGER_LIKE_SCALE)
}

fn illager_crossed() -> Model {
    illager(true)
}

fn illager_arms() -> Model {
    illager(false)
}

/// `WitchModel` = the villager mesh (children preserved — vanilla's
/// `addOrReplaceChild` keeps the nose/hat) + the 4-segment crooked hat and
/// the nose mole. Texture 64×128.
fn witch() -> Model {
    let mut b = ModelBuilder::new();
    let (head, nose) = villager_build(&mut b, true);
    let nose = nose.expect("the witch's nose part");
    let hat = Fold::at([-5.0, -10.03125, -5.0]);
    b.cube_f(
        head,
        0,
        (0.0, 64.0),
        [0.0, 0.0, 0.0],
        [10.0, 2.0, 10.0],
        0.0,
        false,
        &[hat],
    );
    let hat2 = Fold::rot([-0.05235988, 0.0, 0.02617994], [1.75, -4.0, 2.0]);
    b.cube_f(
        head,
        0,
        (0.0, 76.0),
        [0.0, 0.0, 0.0],
        [7.0, 4.0, 7.0],
        0.0,
        false,
        &[hat2, hat],
    );
    let hat3 = Fold::rot([-0.10471976, 0.0, 0.05235988], [1.75, -4.0, 2.0]);
    b.cube_f(
        head,
        0,
        (0.0, 87.0),
        [0.0, 0.0, 0.0],
        [4.0, 4.0, 4.0],
        0.0,
        false,
        &[hat3, hat2, hat],
    );
    let hat4 = Fold::rot([-PI / 15.0, 0.0, 0.10471976], [1.75, -2.0, 2.0]);
    b.cube_f(
        head,
        0,
        (0.0, 95.0),
        [0.0, 0.0, 0.0],
        [1.0, 2.0, 1.0],
        0.25,
        false,
        &[hat4, hat3, hat2, hat],
    );
    // Mole on the nose (mole pose (0,−2,0) under the nose part's own).
    b.cube_f(
        nose,
        0,
        (0.0, 0.0),
        [0.0, 3.0, -6.75],
        [1.0, 1.0, 1.0],
        -0.25,
        false,
        &[Fold::at([0.0, -2.0, 0.0])],
    );
    b.finish(VILLAGER_LIKE_SCALE)
}

// ---------------------------------------------------------------------------
// Monsters II
// ---------------------------------------------------------------------------

/// `MagmaCubeModel`: 8 stacked 8×1×8 slabs + the inner core cube.
fn magma_cube() -> Model {
    let mut b = ModelBuilder::new();
    for i in 0..8 {
        let (mut u, mut v) = (0.0, 0.0);
        if (1..4).contains(&i) {
            v = 9.0 * i as f32;
        } else if i > 3 {
            u = 32.0;
            v = 9.0 * i as f32 - 36.0;
        }
        b.cube(
            STATIC_PART,
            0,
            (u, v),
            [-4.0, 16.0 + i as f32, -4.0],
            [8.0, 1.0, 8.0],
            NONE,
        );
    }
    b.cube(
        STATIC_PART,
        0,
        (24.0, 40.0),
        [-2.0, 18.0, -2.0],
        [4.0, 4.0, 4.0],
        NONE,
    );
    // Same fixed medium size as the slime.
    b.finish(2.0)
}

/// `VexModel`: floating imp — head + tapered body (tilted π/20) + bobbing
/// arms + fluttering 0-thick wings, all lifted by the root's −2.5 offset.
fn vex() -> Model {
    let mut b = ModelBuilder::new();
    let head = b.part([0.0, 17.5, 0.0], Anim::Head, 1.0);
    b.cube(
        head,
        0,
        (0.0, 0.0),
        [-2.5, -5.0, -2.5],
        [5.0, 5.0, 5.0],
        NONE,
    );
    // Body pose (0,20,0) under the root's (0,−2.5,0); setupAnim tilts it
    // π/20 when not charging.
    let body = b.part_ext(
        [0.0, 17.5, 0.0],
        [PI / 20.0, 0.0, 0.0],
        Anim::None,
        1.0,
        Option::None,
    );
    b.cube(
        body,
        0,
        (0.0, 10.0),
        [-1.5, 0.0, -1.0],
        [3.0, 4.0, 2.0],
        NONE,
    );
    b.cube_g(
        body,
        0,
        (0.0, 16.0),
        [-1.5, 1.0, -1.0],
        [3.0, 5.0, 2.0],
        -0.2,
        NONE,
    );
    let arm_r = b.part_ext(
        [-1.75, 0.25, 0.0],
        [0.0; 3],
        Anim::VexArm { left: false },
        1.0,
        Some(body),
    );
    b.cube_g(
        arm_r,
        0,
        (23.0, 0.0),
        [-1.25, -0.5, -1.0],
        [2.0, 4.0, 2.0],
        -0.1,
        NONE,
    );
    let arm_l = b.part_ext(
        [1.75, 0.25, 0.0],
        [0.0; 3],
        Anim::VexArm { left: true },
        1.0,
        Some(body),
    );
    b.cube_g(
        arm_l,
        0,
        (23.0, 6.0),
        [-0.75, -0.5, -1.0],
        [2.0, 4.0, 2.0],
        -0.1,
        NONE,
    );
    let wing_l = b.part_ext(
        [0.5, 1.0, 1.0],
        [0.0; 3],
        Anim::VexWing { left: true },
        1.0,
        Some(body),
    );
    b.cube_m(
        wing_l,
        0,
        (16.0, 14.0),
        [0.0, 0.0, 0.0],
        [0.0, 5.0, 8.0],
        NONE,
    );
    let wing_r = b.part_ext(
        [-0.5, 1.0, 1.0],
        [0.0; 3],
        Anim::VexWing { left: false },
        1.0,
        Some(body),
    );
    b.cube(
        wing_r,
        0,
        (16.0, 14.0),
        [0.0, 0.0, 0.0],
        [0.0, 5.0, 8.0],
        NONE,
    );
    b.finish(1.0)
}

/// `PhantomModel`: swept body + two-segment flapping wings + swaying tail,
/// tilted head. The tips hang off the bases (runtime hierarchy) exactly as
/// vanilla bends them.
fn phantom() -> Model {
    let mut b = ModelBuilder::new();
    let body = b.part_ext(
        [0.0, 0.0, 0.0],
        [-0.1, 0.0, 0.0],
        Anim::None,
        1.0,
        Option::None,
    );
    b.cube(
        body,
        0,
        (0.0, 8.0),
        [-3.0, -2.0, -8.0],
        [5.0, 3.0, 9.0],
        NONE,
    );
    b.cube_f(
        body,
        0,
        (0.0, 0.0),
        [-4.0, -2.0, -5.0],
        [7.0, 3.0, 5.0],
        0.0,
        false,
        &[Fold::rot([0.2, 0.0, 0.0], [0.0, 1.0, -7.0])],
    );
    let tail_base = b.part_ext(
        [0.0, -2.0, 1.0],
        [0.0; 3],
        Anim::PhantomTail,
        1.0,
        Some(body),
    );
    b.cube(
        tail_base,
        0,
        (3.0, 20.0),
        [-2.0, 0.0, 0.0],
        [3.0, 2.0, 6.0],
        NONE,
    );
    let tail_tip = b.part_ext(
        [0.0, 0.5, 6.0],
        [0.0; 3],
        Anim::PhantomTail,
        1.0,
        Some(tail_base),
    );
    b.cube(
        tail_tip,
        0,
        (4.0, 29.0),
        [-1.0, 0.0, 0.0],
        [1.0, 1.0, 6.0],
        NONE,
    );
    let wl = b.part_ext(
        [2.0, -2.0, -8.0],
        [0.0; 3],
        Anim::PhantomWing { left: true },
        1.0,
        Some(body),
    );
    b.cube(wl, 0, (23.0, 12.0), [0.0, 0.0, 0.0], [6.0, 2.0, 9.0], NONE);
    let wlt = b.part_ext(
        [6.0, 0.0, 0.0],
        [0.0; 3],
        Anim::PhantomWing { left: true },
        1.0,
        Some(wl),
    );
    b.cube(
        wlt,
        0,
        (16.0, 24.0),
        [0.0, 0.0, 0.0],
        [13.0, 1.0, 9.0],
        NONE,
    );
    let wr = b.part_ext(
        [-3.0, -2.0, -8.0],
        [0.0; 3],
        Anim::PhantomWing { left: false },
        1.0,
        Some(body),
    );
    b.cube_m(wr, 0, (23.0, 12.0), [-6.0, 0.0, 0.0], [6.0, 2.0, 9.0], NONE);
    let wrt = b.part_ext(
        [-6.0, 0.0, 0.0],
        [0.0; 3],
        Anim::PhantomWing { left: false },
        1.0,
        Some(wr),
    );
    b.cube_m(
        wrt,
        0,
        (16.0, 24.0),
        [-13.0, 0.0, 0.0],
        [13.0, 1.0, 9.0],
        NONE,
    );
    b.finish(1.0)
}

/// `GuardianModel`'s spike table (`SPIKE_X_ROT` .. `SPIKE_Z`): each spike's
/// rotation in half turns and its direction from the body's centre.
pub const GUARDIAN_SPIKE_X_ROT: [f32; 12] = [
    1.75, 0.25, 0.0, 0.0, 0.5, 0.5, 0.5, 0.5, 1.25, 0.75, 0.0, 0.0,
];
pub const GUARDIAN_SPIKE_Y_ROT: [f32; 12] = [
    0.0, 0.0, 0.0, 0.0, 0.25, 1.75, 1.25, 0.75, 0.0, 0.0, 0.0, 0.0,
];
pub const GUARDIAN_SPIKE_Z_ROT: [f32; 12] = [
    0.0, 0.0, 0.25, 1.75, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.75, 1.25,
];
pub const GUARDIAN_SPIKE_X: [f32; 12] = [
    0.0, 0.0, 8.0, -8.0, -8.0, 8.0, 8.0, -8.0, 0.0, 0.0, 8.0, -8.0,
];
pub const GUARDIAN_SPIKE_Y: [f32; 12] = [
    -8.0, -8.0, -8.0, -8.0, 0.0, 0.0, 0.0, 0.0, 8.0, 8.0, 8.0, 8.0,
];
pub const GUARDIAN_SPIKE_Z: [f32; 12] = [
    8.0, -8.0, 0.0, 0.0, -8.0, -8.0, 8.0, 8.0, 8.0, -8.0, 0.0, 0.0,
];

/// `GuardianModel`: the boxy body with side/top/bottom fins, 12 radial
/// spikes, the eye, and the 3-segment tail. M370 — the body (vanilla's
/// `head`) looks, the spikes wobble and the tail sways.
fn guardian_like(scale: f32) -> Model {
    let mut b = ModelBuilder::new();
    // M370 — vanilla's whole body is the `head` part (`PartPose.ZERO`), which
    // looks; the spikes, eye and tail are its children.
    let head = b.part_named("head", [0.0; 3], [0.0; 3], Anim::Head, 1.0, Option::None);
    b.cube(
        head,
        0,
        (0.0, 0.0),
        [-6.0, 10.0, -8.0],
        [12.0, 12.0, 16.0],
        NONE,
    );
    b.cube(
        head,
        0,
        (0.0, 28.0),
        [-8.0, 10.0, -6.0],
        [2.0, 12.0, 12.0],
        NONE,
    );
    b.cube_m(
        head,
        0,
        (0.0, 28.0),
        [6.0, 10.0, -6.0],
        [2.0, 12.0, 12.0],
        NONE,
    );
    b.cube(
        head,
        0,
        (16.0, 40.0),
        [-6.0, 8.0, -6.0],
        [12.0, 2.0, 12.0],
        NONE,
    );
    b.cube(
        head,
        0,
        (16.0, 40.0),
        [-6.0, 22.0, -6.0],
        [12.0, 2.0, 12.0],
        NONE,
    );
    for i in 0..12 {
        // Rest pose: getSpike*(i, 0, 0) with offset 1 + 0.01·cos(i); the
        // wobble about it is `Anim::GuardianSpike`.
        let k = 1.0 + 0.01 * (i as f32).cos();
        let pivot = [
            GUARDIAN_SPIKE_X[i] * k,
            16.0 + GUARDIAN_SPIKE_Y[i] * k,
            GUARDIAN_SPIKE_Z[i] * k,
        ];
        let rot = [
            PI * GUARDIAN_SPIKE_X_ROT[i],
            PI * GUARDIAN_SPIKE_Y_ROT[i],
            PI * GUARDIAN_SPIKE_Z_ROT[i],
        ];
        let spike = b.part_ext(
            pivot,
            rot,
            Anim::GuardianSpike { i: i as u8 },
            1.0,
            Some(head),
        );
        b.cube(
            spike,
            0,
            (0.0, 0.0),
            [-1.0, -4.5, -1.0],
            [2.0, 9.0, 2.0],
            NONE,
        );
    }
    b.cube_f(
        head,
        0,
        (8.0, 0.0),
        [-1.0, 15.0, 0.0],
        [2.0, 2.0, 1.0],
        0.0,
        false,
        &[Fold::at([0.0, 0.0, -8.25])],
    );
    let t0 = b.part_ext(
        [0.0; 3],
        [0.0; 3],
        Anim::GuardianTail { seg: 0 },
        1.0,
        Some(head),
    );
    b.cube(t0, 0, (40.0, 0.0), [-2.0, 14.0, 7.0], [4.0, 4.0, 8.0], NONE);
    let t1 = b.part_ext(
        [-1.5, 0.5, 14.0],
        [0.0; 3],
        Anim::GuardianTail { seg: 1 },
        1.0,
        Some(t0),
    );
    b.cube(t1, 0, (0.0, 54.0), [0.0, 14.0, 0.0], [3.0, 3.0, 7.0], NONE);
    let t2 = b.part_ext(
        [0.5, 0.5, 6.0],
        [0.0; 3],
        Anim::GuardianTail { seg: 2 },
        1.0,
        Some(t1),
    );
    b.cube(t2, 0, (41.0, 32.0), [0.0, 14.0, 0.0], [2.0, 2.0, 6.0], NONE);
    b.cube(t2, 0, (25.0, 19.0), [1.0, 10.5, 3.0], [1.0, 9.0, 9.0], NONE);
    b.finish(scale)
}

fn guardian() -> Model {
    guardian_like(1.0)
}

fn elder_guardian() -> Model {
    // Vanilla `ELDER_GUARDIAN_SCALE` mesh transform.
    guardian_like(2.35)
}

/// `ShulkerModel`: closed box (lid resting on base) + the head hidden
/// inside.
fn shulker() -> Model {
    let mut b = ModelBuilder::new();
    b.cube_f(
        STATIC_PART,
        0,
        (0.0, 0.0),
        [-8.0, -16.0, -8.0],
        [16.0, 12.0, 16.0],
        0.0,
        false,
        &[Fold::at([0.0, 24.0, 0.0])],
    );
    b.cube_f(
        STATIC_PART,
        0,
        (0.0, 28.0),
        [-8.0, -8.0, -8.0],
        [16.0, 8.0, 16.0],
        0.0,
        false,
        &[Fold::at([0.0, 24.0, 0.0])],
    );
    b.cube_f(
        STATIC_PART,
        0,
        (0.0, 52.0),
        [-3.0, 0.0, -3.0],
        [6.0, 6.0, 6.0],
        0.0,
        false,
        &[Fold::at([0.0, 12.0, 0.0])],
    );
    b.finish(1.0)
}

/// Segment-chain crawlers (`SilverfishModel` / `EndermiteModel`): boxes
/// centered on a marching z placement, each segment a part on the vanilla
/// wiggle (`ry`/`tx` amplitudes differ between the two species).
fn segmented(
    sizes: &[[f32; 3]],
    texs: &[(f32, f32)],
    ry: f32,
    tx: f32,
) -> (ModelBuilder, Vec<f32>) {
    let mut b = ModelBuilder::new();
    let mut z = Vec::with_capacity(sizes.len());
    let mut placement = -3.5f32;
    for (i, s) in sizes.iter().enumerate() {
        let p = b.part(
            [0.0, 24.0 - s[1], placement],
            Anim::Crawl {
                i: i as u8,
                ry,
                tx,
                with_x: true,
            },
            1.0,
        );
        b.cube(p, 0, texs[i], [s[0] * -0.5, 0.0, s[2] * -0.5], *s, NONE);
        z.push(placement);
        if i + 1 < sizes.len() {
            placement += (s[2] + sizes[i + 1][2]) * 0.5;
        }
    }
    (b, z)
}

fn silverfish() -> Model {
    let sizes: [[f32; 3]; 7] = [
        [3.0, 2.0, 2.0],
        [4.0, 3.0, 2.0],
        [6.0, 4.0, 3.0],
        [3.0, 3.0, 3.0],
        [2.0, 2.0, 3.0],
        [2.0, 1.0, 2.0],
        [1.0, 1.0, 2.0],
    ];
    let texs = [
        (0.0, 0.0),
        (0.0, 4.0),
        (0.0, 9.0),
        (0.0, 16.0),
        (0.0, 22.0),
        (11.0, 0.0),
        (13.0, 4.0),
    ];
    let (mut b, z) = segmented(&sizes, &texs, 0.05, 0.2);
    // Wing layers copy their source segment's wiggle (layer 0 yRot-only —
    // vanilla doesn't move its x).
    let l0 = b.part(
        [0.0, 16.0, z[2]],
        Anim::Crawl {
            i: 2,
            ry: 0.05,
            tx: 0.2,
            with_x: false,
        },
        1.0,
    );
    b.cube(
        l0,
        0,
        (20.0, 0.0),
        [-5.0, 0.0, -1.5],
        [10.0, 8.0, 3.0],
        NONE,
    );
    let l1 = b.part(
        [0.0, 20.0, z[4]],
        Anim::Crawl {
            i: 4,
            ry: 0.05,
            tx: 0.2,
            with_x: true,
        },
        1.0,
    );
    b.cube(
        l1,
        0,
        (20.0, 11.0),
        [-3.0, 0.0, -1.5],
        [6.0, 4.0, 3.0],
        NONE,
    );
    let l2 = b.part(
        [0.0, 19.0, z[1]],
        Anim::Crawl {
            i: 1,
            ry: 0.05,
            tx: 0.2,
            with_x: true,
        },
        1.0,
    );
    b.cube(
        l2,
        0,
        (20.0, 18.0),
        [-3.0, 0.0, -1.5],
        [6.0, 5.0, 2.0],
        NONE,
    );
    b.finish(1.0)
}

fn endermite() -> Model {
    let sizes: [[f32; 3]; 4] = [
        [4.0, 3.0, 2.0],
        [6.0, 4.0, 5.0],
        [3.0, 3.0, 1.0],
        [1.0, 2.0, 1.0],
    ];
    let texs = [(0.0, 0.0), (0.0, 5.0), (0.0, 14.0), (0.0, 18.0)];
    let (b, _) = segmented(&sizes, &texs, 0.01, 0.1);
    b.finish(1.0)
}

/// `BlazeModel`: head + 12 rods in three counter-rotating rings — the rod
/// positions are fully animated (vanilla repositions them every frame).
fn blaze() -> Model {
    let mut b = ModelBuilder::new();
    let head = b.part([0.0, 0.0, 0.0], Anim::Head, 1.0);
    b.cube(
        head,
        0,
        (0.0, 0.0),
        [-4.0, -4.0, -4.0],
        [8.0, 8.0, 8.0],
        NONE,
    );
    for ring in 0..3u8 {
        for idx in 0..4u8 {
            let p = b.part([0.0; 3], Anim::BlazeRod { ring, idx }, 1.0);
            b.cube(p, 0, (0.0, 16.0), [0.0, 0.0, 0.0], [2.0, 8.0, 2.0], NONE);
        }
    }
    b.finish(1.0)
}

/// Java's `Random` LCG — the ghast's tentacle lengths come from a fixed
/// seed (1660), so reproducing them exactly needs the exact generator.
struct JavaRandom {
    seed: u64,
}

impl JavaRandom {
    fn new(seed: u64) -> Self {
        JavaRandom {
            seed: (seed ^ 0x5DEECE66D) & ((1 << 48) - 1),
        }
    }
    fn next(&mut self, bits: u32) -> i32 {
        self.seed = self.seed.wrapping_mul(0x5DEECE66D).wrapping_add(0xB) & ((1 << 48) - 1);
        (self.seed >> (48 - bits)) as i32
    }
    fn next_int(&mut self, bound: i32) -> i32 {
        loop {
            let bits = self.next(31);
            let val = bits % bound;
            if bits - val + (bound - 1) >= 0 {
                return val;
            }
        }
    }
}

/// `GhastModel`: the 16³ body + 9 tentacles with seeded random lengths;
/// vanilla applies `MeshTransformer.scaling(4.5)` (ground-anchored — equal
/// to our `scale`).
fn ghast() -> Model {
    let mut b = ModelBuilder::new();
    b.cube(
        STATIC_PART,
        0,
        (0.0, 0.0),
        [-8.0, -8.0, -8.0],
        [16.0, 16.0, 16.0],
        &[Fold::at([0.0, 17.6, 0.0])],
    );
    let mut rng = JavaRandom::new(1660);
    for i in 0..9u8 {
        let xo = ((i % 3) as f32 - (i / 3 % 2) as f32 * 0.5 + 0.25 - 1.0) * 5.0;
        let yo = ((i / 3) as f32 - 1.0) * 5.0;
        let len = (rng.next_int(7) + 8) as f32;
        let p = b.part([xo, 24.6, yo], Anim::GhastTentacle { i }, 1.0);
        b.cube(p, 0, (0.0, 0.0), [-1.0, 0.0, -1.0], [2.0, len, 2.0], NONE);
    }
    b.finish(4.5)
}

/// `HoglinModel`: huge body + mane bristle, tilted head with flapped ears
/// and tusks, chunky asymmetric legs. Zoglin shares it.
///
/// M367 — the head turns (its pitch held at 0.87266463 but for the
/// headbutt), the ears flap and the legs walk as `HoglinModel.setupAnim`
/// moves them.
fn hoglin() -> Model {
    let mut b = ModelBuilder::new();
    let body = Fold::at([0.0, 7.0, 0.0]);
    b.cube_f(
        STATIC_PART,
        0,
        (1.0, 1.0),
        [-8.0, -7.0, -13.0],
        [16.0, 14.0, 26.0],
        0.0,
        false,
        &[body],
    );
    b.cube_f(
        STATIC_PART,
        0,
        (90.0, 33.0),
        [0.0, 0.0, -9.0],
        [0.0, 10.0, 19.0],
        0.001,
        false,
        &[Fold::at([0.0, -14.0, -7.0]), body],
    );
    // M367 — the head's 0.87266463 is the part's own rest (`setupAnim`
    // assigns it, the headbutt lerps it), the ears are parts that flap.
    let head = b.part_ext(
        [0.0, 2.0, -12.0],
        [0.87266463, 0.0, 0.0],
        Anim::HoglinHead { baby: false },
        1.0,
        Option::None,
    );
    b.cube(
        head,
        0,
        (61.0, 1.0),
        [-7.0, -3.0, -19.0],
        [14.0, 6.0, 19.0],
        NONE,
    );
    for (uv, min, pivot, z_rot, left) in [
        (
            (1.0, 1.0),
            [-6.0, -1.0, -2.0],
            [-6.0, -2.0, -3.0],
            -PI * 2.0 / 9.0,
            false,
        ),
        (
            (1.0, 6.0),
            [0.0, -1.0, -2.0],
            [6.0, -2.0, -3.0],
            PI * 2.0 / 9.0,
            true,
        ),
    ] {
        let ear = b.part_ext(
            pivot,
            [0.0, 0.0, z_rot],
            Anim::HoglinEar { left },
            1.0,
            Some(head),
        );
        b.cube(ear, 0, uv, min, [6.0, 1.0, 4.0], NONE);
    }
    for (uv, x) in [((10.0, 13.0), -7.0), ((1.0, 13.0), 7.0)] {
        b.cube_f(
            head,
            0,
            uv,
            [-1.0, -11.0, -1.0],
            [2.0, 11.0, 2.0],
            0.0,
            false,
            &[Fold::at([x, 2.0, -12.0])],
        );
    }
    for (uv, min, dims, pivot, front, left) in [
        (
            (66.0, 42.0),
            [-3.0, 0.0, -3.0],
            [6.0, 14.0, 6.0],
            [-4.0, 10.0, -8.5],
            true,
            false,
        ),
        (
            (41.0, 42.0),
            [-3.0, 0.0, -3.0],
            [6.0, 14.0, 6.0],
            [4.0, 10.0, -8.5],
            true,
            true,
        ),
        (
            (21.0, 45.0),
            [-2.5, 0.0, -2.5],
            [5.0, 11.0, 5.0],
            [-5.0, 13.0, 10.0],
            false,
            false,
        ),
        (
            (0.0, 45.0),
            [-2.5, 0.0, -2.5],
            [5.0, 11.0, 5.0],
            [5.0, 13.0, 10.0],
            false,
            true,
        ),
    ] {
        let p = b.part(pivot, Anim::HoglinLeg { front, left }, 1.0);
        b.cube(p, 0, uv, min, dims, NONE);
    }
    b.finish(1.0)
}

/// `AdultStriderModel`: tall legs, boxy body, six 0-thick hair bristles
/// fanning out the sides; M366 — every part moves as `StriderModel` and
/// `AdultStriderModel.customAnimations` move it.
fn strider() -> Model {
    let mut b = ModelBuilder::new();
    let leg_r = b.part([-4.0, 8.0, 0.0], Anim::StriderLeg { left: false }, 1.0);
    b.cube(
        leg_r,
        0,
        (0.0, 32.0),
        [-2.0, 0.0, -2.0],
        [4.0, 16.0, 4.0],
        NONE,
    );
    let leg_l = b.part([4.0, 8.0, 0.0], Anim::StriderLeg { left: true }, 1.0);
    b.cube(
        leg_l,
        0,
        (0.0, 55.0),
        [-2.0, 0.0, -2.0],
        [4.0, 16.0, 4.0],
        NONE,
    );
    // M366 — `customAnimations` assigns `body.y = 2` every frame, so the
    // layer's 1 is never drawn (Rewo drew it there until M366, 1 px high).
    let body = b.part([0.0, 2.0, 0.0], Anim::StriderBody { adult: true }, 1.0);
    b.cube(
        body,
        0,
        (0.0, 0.0),
        [-8.0, -6.0, -8.0],
        [16.0, 14.0, 16.0],
        NONE,
    );
    // (uv, box min, rest zRot, pivot, mirror, animateBristle order).
    let bristles: [((f32, f32), [f32; 3], f32, [f32; 3], bool, u8); 6] = [
        (
            (16.0, 65.0),
            [-12.0, 0.0, 0.0],
            -1.2217305,
            [-8.0, 4.0, -8.0],
            true,
            2,
        ),
        (
            (16.0, 49.0),
            [-12.0, 0.0, 0.0],
            -1.134464,
            [-8.0, -1.0, -8.0],
            true,
            1,
        ),
        (
            (16.0, 33.0),
            [-12.0, 0.0, 0.0],
            -0.87266463,
            [-8.0, -5.0, -8.0],
            true,
            0,
        ),
        (
            (16.0, 33.0),
            [0.0, 0.0, 0.0],
            0.87266463,
            [8.0, -6.0, -8.0],
            false,
            0,
        ),
        (
            (16.0, 49.0),
            [0.0, 0.0, 0.0],
            1.134464,
            [8.0, -2.0, -8.0],
            false,
            1,
        ),
        (
            (16.0, 65.0),
            [0.0, 0.0, 0.0],
            1.2217305,
            [8.0, 3.0, -8.0],
            false,
            2,
        ),
    ];
    for (uv, min, zrot, pivot, mirror, order) in bristles {
        let p = b.part_ext(
            pivot,
            [0.0, 0.0, zrot],
            Anim::StriderBristle { order, baby: false },
            1.0,
            Some(body),
        );
        b.cube_f(p, 0, uv, min, [12.0, 0.0, 16.0], 0.0, mirror, NONE);
    }
    b.finish(1.0)
}

// ---------------------------------------------------------------------------
// Passives II — the overworld menagerie
// ---------------------------------------------------------------------------

/// `BatModel`: small body, big ears, membrane wings — flying on the
/// vanilla BAT_FLYING keyframe rig (our bats always render airborne).
fn bat() -> Model {
    let mut b = ModelBuilder::new();
    let body = b.part_named(
        "body",
        [0.0, 17.0, 0.0],
        [0.0; 3],
        Anim::None,
        1.0,
        Option::None,
    );
    b.cube(
        body,
        0,
        (0.0, 0.0),
        [-1.5, 0.0, -1.0],
        [3.0, 5.0, 2.0],
        NONE,
    );
    let head = b.part_named(
        "head",
        [0.0, 17.0, 0.0],
        [0.0; 3],
        Anim::None,
        1.0,
        Option::None,
    );
    b.cube(
        head,
        0,
        (0.0, 7.0),
        [-2.0, -3.0, -1.0],
        [4.0, 3.0, 2.0],
        NONE,
    );
    b.cube_f(
        head,
        0,
        (1.0, 15.0),
        [-2.5, -4.0, 0.0],
        [3.0, 5.0, 0.0],
        0.0,
        false,
        &[Fold::at([-1.5, -2.0, 0.0])],
    );
    b.cube_f(
        head,
        0,
        (8.0, 15.0),
        [-0.1, -3.0, 0.0],
        [3.0, 5.0, 0.0],
        0.0,
        false,
        &[Fold::at([1.1, -3.0, 0.0])],
    );
    let wr = b.part_named(
        "right_wing",
        [-1.5, 0.0, 0.0],
        [0.0; 3],
        Anim::None,
        1.0,
        Some(body),
    );
    b.cube(wr, 0, (12.0, 0.0), [-2.0, -2.0, 0.0], [2.0, 7.0, 0.0], NONE);
    let wrt = b.part_named(
        "right_wing_tip",
        [-2.0, 0.0, 0.0],
        [0.0; 3],
        Anim::None,
        1.0,
        Some(wr),
    );
    b.cube(
        wrt,
        0,
        (16.0, 0.0),
        [-6.0, -2.0, 0.0],
        [6.0, 8.0, 0.0],
        NONE,
    );
    let wl = b.part_named(
        "left_wing",
        [1.5, 0.0, 0.0],
        [0.0; 3],
        Anim::None,
        1.0,
        Some(body),
    );
    b.cube(wl, 0, (12.0, 7.0), [0.0, -2.0, 0.0], [2.0, 7.0, 0.0], NONE);
    let wlt = b.part_named(
        "left_wing_tip",
        [2.0, 0.0, 0.0],
        [0.0; 3],
        Anim::None,
        1.0,
        Some(wl),
    );
    b.cube(wlt, 0, (16.0, 8.0), [0.0, -2.0, 0.0], [6.0, 8.0, 0.0], NONE);
    let feet = b.part_named(
        "feet",
        [0.0, 5.0, 0.0],
        [0.0; 3],
        Anim::None,
        1.0,
        Some(body),
    );
    b.cube(
        feet,
        0,
        (16.0, 16.0),
        [-1.5, 0.0, 0.0],
        [3.0, 2.0, 0.0],
        NONE,
    );
    b.keyframe(&crate::anim_defs::BAT_FLYING, KfDriver::Age);
    b.finish(1.0)
}

/// `AdultFelineModel` (cat + ocelot): slinky body, two-segment tail,
/// tall front legs.
fn feline() -> Model {
    feline_with(0.0)
}

/// M343 — `AdultFelineModel.createBodyMesh(g)`: `g` on every box but the
/// tail's second segment, which keeps its own -0.02 (`tail_g`).
fn feline_with(g: f32) -> Model {
    let mut b = ModelBuilder::new();
    let head = b.part([0.0, 15.0, -9.0], Anim::FelineHead, 1.0);
    b.cube_g(
        head,
        0,
        (0.0, 0.0),
        [-2.5, -2.0, -3.0],
        [5.0, 4.0, 5.0],
        g,
        NONE,
    );
    b.cube_g(
        head,
        0,
        (0.0, 24.0),
        [-1.5, -0.001, -4.0],
        [3.0, 2.0, 2.0],
        g,
        NONE,
    );
    b.cube_g(
        head,
        0,
        (0.0, 10.0),
        [-2.0, -3.0, 0.0],
        [1.0, 1.0, 2.0],
        g,
        NONE,
    );
    b.cube_g(
        head,
        0,
        (6.0, 10.0),
        [1.0, -3.0, 0.0],
        [1.0, 1.0, 2.0],
        g,
        NONE,
    );
    // M357 — the body and both tail segments are parts: sitting moves
    // them, and the second segment rests where `setupAnim` puts it
    // (1.7278761, not the layer's 0: until M357 it hung straight down).
    let body = b.part_ext(
        [0.0, 12.0, -10.0],
        [FRAC_PI_2, 0.0, 0.0],
        Anim::FelineBody,
        1.0,
        Option::None,
    );
    b.cube_g(
        body,
        0,
        (20.0, 0.0),
        [-2.0, 3.0, -8.0],
        [4.0, 16.0, 6.0],
        g,
        NONE,
    );
    let tail1 = b.part_ext(
        [0.0, 15.0, 8.0],
        [0.9, 0.0, 0.0],
        Anim::FelineTail1,
        1.0,
        Option::None,
    );
    b.cube_g(
        tail1,
        0,
        (0.0, 15.0),
        [-0.5, 0.0, 0.0],
        [1.0, 8.0, 1.0],
        g,
        NONE,
    );
    let tail2 = b.part_ext(
        [0.0, 20.0, 14.0],
        [1.727_876_1, 0.0, 0.0],
        Anim::FelineTail2,
        1.0,
        Option::None,
    );
    b.cube_g(
        tail2,
        0,
        (4.0, 15.0),
        [-0.5, 0.0, 0.0],
        [1.0, 8.0, 1.0],
        -0.02,
        NONE,
    );
    let legs = [
        (
            (8.0, 13.0),
            [-1.0, 0.0, 1.0],
            [2.0, 6.0, 2.0],
            [1.1, 18.0, 5.0],
            Anim::FelineLeg {
                hind: true,
                left: true,
            },
        ),
        (
            (8.0, 13.0),
            [-1.0, 0.0, 1.0],
            [2.0, 6.0, 2.0],
            [-1.1, 18.0, 5.0],
            Anim::FelineLeg {
                hind: true,
                left: false,
            },
        ),
        (
            (40.0, 0.0),
            [-1.0, 0.0, 0.0],
            [2.0, 10.0, 2.0],
            [1.2, 14.1, -5.0],
            Anim::FelineLeg {
                hind: false,
                left: true,
            },
        ),
        (
            (40.0, 0.0),
            [-1.0, 0.0, 0.0],
            [2.0, 10.0, 2.0],
            [-1.2, 14.1, -5.0],
            Anim::FelineLeg {
                hind: false,
                left: false,
            },
        ),
    ];
    for (uv, min, dims, pivot, anim) in legs {
        let p = b.part(pivot, anim, 1.0);
        b.cube_g(p, 0, uv, min, dims, g, NONE);
    }
    b.finish(1.0)
}

/// `AdultFoxModel`: big-eared head, rotated body with the tail riding it.
fn fox() -> Model {
    let mut b = ModelBuilder::new();
    let head = b.part([-1.0, 16.5, -3.0], Anim::Head, 1.0);
    b.cube(
        head,
        0,
        (1.0, 5.0),
        [-3.0, -2.0, -5.0],
        [8.0, 6.0, 6.0],
        NONE,
    );
    b.cube(
        head,
        0,
        (8.0, 1.0),
        [-3.0, -4.0, -4.0],
        [2.0, 2.0, 1.0],
        NONE,
    );
    b.cube(
        head,
        0,
        (15.0, 1.0),
        [3.0, -4.0, -4.0],
        [2.0, 2.0, 1.0],
        NONE,
    );
    b.cube(
        head,
        0,
        (6.0, 18.0),
        [-1.0, 2.01, -8.0],
        [4.0, 2.0, 3.0],
        NONE,
    );
    let body = Fold::rot([FRAC_PI_2, 0.0, 0.0], [0.0, 16.0, -6.0]);
    b.cube_f(
        STATIC_PART,
        0,
        (24.0, 15.0),
        [-3.0, 3.999, -3.5],
        [6.0, 11.0, 6.0],
        0.0,
        false,
        &[body],
    );
    let legs = [
        ((13.0, 24.0), [-5.0, 17.5, 7.0], Anim::QuadHindRight),
        ((4.0, 24.0), [-1.0, 17.5, 7.0], Anim::QuadHindLeft),
        ((13.0, 24.0), [-5.0, 17.5, 0.0], Anim::QuadFrontRight),
        ((4.0, 24.0), [-1.0, 17.5, 0.0], Anim::QuadFrontLeft),
    ];
    for (uv, pivot, anim) in legs {
        let p = b.part(pivot, anim, 1.0);
        b.cube_g(p, 0, uv, [2.0, 0.5, -1.0], [2.0, 6.0, 2.0], 0.001, NONE);
    }
    b.cube_f(
        STATIC_PART,
        0,
        (30.0, 0.0),
        [2.0, 0.0, -1.0],
        [4.0, 9.0, 5.0],
        0.0,
        false,
        &[Fold::rot([-0.05235988, 0.0, 0.0], [-4.0, 15.0, -1.0]), body],
    );
    b.finish(1.0)
}

/// `GoatModel`: horned head with rotated skull box + goatee, double-box
/// body, asymmetric legs.
fn goat() -> Model {
    let mut b = ModelBuilder::new();
    let head = b.part([1.0, 14.0, 0.0], Anim::Head, 1.0);
    b.cube(
        head,
        0,
        (2.0, 61.0),
        [-6.0, -11.0, -10.0],
        [3.0, 2.0, 1.0],
        NONE,
    );
    b.cube_m(
        head,
        0,
        (2.0, 61.0),
        [2.0, -11.0, -10.0],
        [3.0, 2.0, 1.0],
        NONE,
    );
    b.cube(
        head,
        0,
        (23.0, 52.0),
        [-0.5, -3.0, -14.0],
        [0.0, 7.0, 5.0],
        NONE,
    );
    b.cube(
        head,
        0,
        (12.0, 55.0),
        [-0.01, -16.0, -10.0],
        [2.0, 7.0, 2.0],
        NONE,
    );
    b.cube(
        head,
        0,
        (12.0, 55.0),
        [-2.99, -16.0, -10.0],
        [2.0, 7.0, 2.0],
        NONE,
    );
    b.cube_f(
        head,
        0,
        (34.0, 46.0),
        [-3.0, -4.0, -8.0],
        [5.0, 7.0, 10.0],
        0.0,
        false,
        &[Fold::rot([0.9599, 0.0, 0.0], [0.0, -8.0, -8.0])],
    );
    let body = Fold::at([0.0, 24.0, 0.0]);
    b.cube_f(
        STATIC_PART,
        0,
        (1.0, 1.0),
        [-4.0, -17.0, -7.0],
        [9.0, 11.0, 16.0],
        0.0,
        false,
        &[body],
    );
    b.cube_f(
        STATIC_PART,
        0,
        (0.0, 28.0),
        [-5.0, -18.0, -8.0],
        [11.0, 14.0, 11.0],
        0.0,
        false,
        &[body],
    );
    let legs = [
        (
            (36.0, 29.0),
            [0.0, 4.0, 0.0],
            [3.0, 6.0, 3.0],
            [1.0, 14.0, 4.0],
            Anim::QuadHindLeft,
        ),
        (
            (49.0, 29.0),
            [0.0, 4.0, 0.0],
            [3.0, 6.0, 3.0],
            [-3.0, 14.0, 4.0],
            Anim::QuadHindRight,
        ),
        (
            (49.0, 2.0),
            [0.0, 0.0, 0.0],
            [3.0, 10.0, 3.0],
            [1.0, 14.0, -6.0],
            Anim::QuadFrontLeft,
        ),
        (
            (35.0, 2.0),
            [0.0, 0.0, 0.0],
            [3.0, 10.0, 3.0],
            [-3.0, 14.0, -6.0],
            Anim::QuadFrontRight,
        ),
    ];
    for (uv, min, dims, pivot, anim) in legs {
        let p = b.part(pivot, anim, 1.0);
        b.cube(p, 0, uv, min, dims, NONE);
    }
    b.finish(1.0)
}

/// `AdultBeeModel`: fat body with antennae + stinger, spread 0-thick
/// wings, dangling leg plates. Everything hangs off the hovering bone.
/// M354 — the bone is a part (`bobUpAndDown` pitches and lifts it), the
/// wings and the three leg plates its children, the stinger a part of its
/// own (hidden once the bee has stung).
fn bee() -> Model {
    let mut b = ModelBuilder::new();
    let bone = b.part_named(
        "bone",
        [0.0, 19.0, 0.0],
        [0.0; 3],
        Anim::BeeBone,
        1.0,
        Option::None,
    );
    b.cube(
        bone,
        0,
        (0.0, 0.0),
        [-3.5, -4.0, -5.0],
        [7.0, 7.0, 10.0],
        NONE,
    );
    let stinger = b.part_named("stinger", [0.0; 3], [0.0; 3], Anim::None, 1.0, Some(bone));
    b.show(stinger, Show::HasStinger);
    b.cube(
        stinger,
        0,
        (26.0, 7.0),
        [0.0, -1.0, 5.0],
        [0.0, 1.0, 2.0],
        NONE,
    );
    // M377 — the antennae are parts: `AdultBeeModel.bobUpAndDown` tips
    // them `speed·π·0.03` with the hovering bob.
    for (uv, min) in [
        ((2.0, 0.0), [1.5, -2.0, -3.0]),
        ((2.0, 3.0), [-2.5, -2.0, -3.0]),
    ] {
        let antenna = b.part_ext(
            [0.0, -2.0, -5.0],
            [0.0; 3],
            Anim::BeeAntenna,
            1.0,
            Some(bone),
        );
        b.cube(antenna, 0, uv, min, [1.0, 2.0, 3.0], NONE);
    }
    // Airborne pose (our bees always hover): wings flutter about Z with
    // yRot reset to 0 (vanilla overrides the ±0.2618 rest splay in
    // flight), legs tucked at π/4.
    let wing_r = b.part_named(
        "right_wing",
        [-1.5, -4.0, -3.0],
        [0.0; 3],
        Anim::BeeWing { left: false },
        1.0,
        Some(bone),
    );
    b.cube_g(
        wing_r,
        0,
        (0.0, 18.0),
        [-9.0, 0.0, 0.0],
        [9.0, 0.0, 6.0],
        0.001,
        NONE,
    );
    let wing_l = b.part_named(
        "left_wing",
        [1.5, -4.0, -3.0],
        [0.0; 3],
        Anim::BeeWing { left: true },
        1.0,
        Some(bone),
    );
    b.cube_f(
        wing_l,
        0,
        (0.0, 18.0),
        [0.0, 0.0, 0.0],
        [9.0, 0.0, 6.0],
        0.001,
        true,
        NONE,
    );
    for (name, z, uv, anim) in [
        ("front_legs", -2.0, (26.0, 1.0), Anim::BeeFrontLegs),
        ("middle_legs", 0.0, (26.0, 3.0), Anim::None),
        ("back_legs", 2.0, (26.0, 5.0), Anim::BeeBackLegs),
    ] {
        let leg = b.part_named(
            name,
            [1.5, 3.0, z],
            [PI / 4.0, 0.0, 0.0],
            anim,
            1.0,
            Some(bone),
        );
        b.cube(leg, 0, uv, [-5.0, 0.0, 0.0], [7.0, 2.0, 0.0], NONE);
    }
    b.finish(1.0)
}

/// `FrogModel`: flat body/head plates, pop eyes, folded arms with hand
/// plates, squat legs with feet — walking on the vanilla FROG_WALK
/// keyframe rig.
fn frog() -> Model {
    let mut b = ModelBuilder::new();
    // root(0,24,0) → body(0,−2,4); head/eyes/croak/tongue ride the body.
    let body = b.part_named(
        "body",
        [0.0, 22.0, 4.0],
        [0.0; 3],
        Anim::None,
        1.0,
        Option::None,
    );
    b.cube(
        body,
        0,
        (3.0, 1.0),
        [-3.5, -2.0, -8.0],
        [7.0, 3.0, 9.0],
        NONE,
    );
    b.cube(
        body,
        0,
        (23.0, 22.0),
        [-3.5, -1.0, -8.0],
        [7.0, 0.0, 9.0],
        NONE,
    );
    // Real part (not a fold): FROG_TONGUE tilts the head open.
    let head = b.part_named(
        "head",
        [0.0, -2.0, -1.0],
        [0.0; 3],
        Anim::None,
        1.0,
        Some(body),
    );
    b.cube(
        head,
        0,
        (23.0, 13.0),
        [-3.5, -1.0, -7.0],
        [7.0, 0.0, 9.0],
        NONE,
    );
    b.cube(
        head,
        0,
        (0.0, 13.0),
        [-3.5, -2.0, -7.0],
        [7.0, 3.0, 9.0],
        NONE,
    );
    let eyes = Fold::at([-0.5, 0.0, 2.0]);
    b.cube_f(
        head,
        0,
        (0.0, 0.0),
        [-1.5, -1.0, -1.5],
        [3.0, 2.0, 3.0],
        0.0,
        false,
        &[Fold::at([-1.5, -3.0, -6.5]), eyes],
    );
    b.cube_f(
        head,
        0,
        (0.0, 5.0),
        [-1.5, -1.0, -1.5],
        [3.0, 2.0, 3.0],
        0.0,
        false,
        &[Fold::at([2.5, -3.0, -6.5]), eyes],
    );
    let croak = b.part_named(
        "croaking_body",
        [0.0, -1.0, -5.0],
        [0.0; 3],
        Anim::None,
        1.0,
        Some(body),
    );
    b.cube_g(
        croak,
        0,
        (26.0, 5.0),
        [-3.5, -0.1, -2.9],
        [7.0, 2.0, 3.0],
        -0.1,
        NONE,
    );
    // Vanilla: `croakingBody.visible = croakAnimationState.isStarted()`.
    b.show(croak, Show::During(Gesture::FrogCroak));
    let tongue = b.part_named(
        "tongue",
        [0.0, -1.01, 1.0],
        [0.0; 3],
        Anim::None,
        1.0,
        Some(body),
    );
    b.cube(
        tongue,
        0,
        (17.0, 13.0),
        [-2.0, 0.0, -7.1],
        [4.0, 0.0, 7.0],
        NONE,
    );
    let arm_l = b.part_named(
        "left_arm",
        [4.0, -1.0, -6.5],
        [0.0; 3],
        Anim::None,
        1.0,
        Some(body),
    );
    b.cube(
        arm_l,
        0,
        (0.0, 32.0),
        [-1.0, 0.0, -1.0],
        [2.0, 3.0, 3.0],
        NONE,
    );
    b.cube_f(
        arm_l,
        0,
        (18.0, 40.0),
        [-4.0, 0.01, -4.0],
        [8.0, 0.0, 8.0],
        0.0,
        false,
        &[Fold::at([0.0, 3.0, -1.0])],
    );
    let arm_r = b.part_named(
        "right_arm",
        [-4.0, -1.0, -6.5],
        [0.0; 3],
        Anim::None,
        1.0,
        Some(body),
    );
    b.cube(
        arm_r,
        0,
        (0.0, 38.0),
        [-1.0, 0.0, -1.0],
        [2.0, 3.0, 3.0],
        NONE,
    );
    b.cube_f(
        arm_r,
        0,
        (2.0, 40.0),
        [-4.0, 0.01, -5.0],
        [8.0, 0.0, 8.0],
        0.0,
        false,
        &[Fold::at([0.0, 3.0, 0.0])],
    );
    let leg_l = b.part_named(
        "left_leg",
        [3.5, 21.0, 4.0],
        [0.0; 3],
        Anim::None,
        1.0,
        Option::None,
    );
    b.cube(
        leg_l,
        0,
        (14.0, 25.0),
        [-1.0, 0.0, -2.0],
        [3.0, 3.0, 4.0],
        NONE,
    );
    b.cube_f(
        leg_l,
        0,
        (2.0, 32.0),
        [-4.0, 0.01, -4.0],
        [8.0, 0.0, 8.0],
        0.0,
        false,
        &[Fold::at([2.0, 3.0, 0.0])],
    );
    let leg_r = b.part_named(
        "right_leg",
        [-3.5, 21.0, 4.0],
        [0.0; 3],
        Anim::None,
        1.0,
        Option::None,
    );
    b.cube(
        leg_r,
        0,
        (0.0, 25.0),
        [-2.0, 0.0, -2.0],
        [3.0, 3.0, 4.0],
        NONE,
    );
    b.cube_f(
        leg_r,
        0,
        (18.0, 32.0),
        [-4.0, 0.01, -4.0],
        [8.0, 0.0, 8.0],
        0.0,
        false,
        &[Fold::at([-2.0, 3.0, 0.0])],
    );
    b.keyframe(
        &crate::anim_defs::FROG_WALK,
        KfDriver::Walk {
            speed_f: 1.5,
            scale_f: 2.5,
        },
    );
    // Croak/tongue gestures (their throat-puff scale channels are dropped
    // — the remaining pos/rot movement plays).
    b.keyframe_gesture(&crate::anim_defs::FROG_CROAK, Gesture::FrogCroak);
    b.keyframe_gesture(&crate::anim_defs::FROG_TONGUE, Gesture::FrogTongue);
    b.finish(1.0)
}

/// `TadpoleModel`: a nubbin + 0-thick tail.
fn tadpole() -> Model {
    let mut b = ModelBuilder::new();
    b.cube_f(
        STATIC_PART,
        0,
        (0.0, 0.0),
        [-1.5, -1.0, 0.0],
        [3.0, 2.0, 3.0],
        0.0,
        false,
        &[Fold::at([0.0, 22.0, -3.0])],
    );
    // M377 — `TadpoleModel.setupAnim`: `tail.yRot = −0.25·sin(0.3·age)` in
    // water (1.5 times that out of it, which Rewo does not tell apart).
    let tail = b.part(
        [0.0, 22.0, 0.0],
        Anim::FishTail {
            amp: 0.25,
            speed: 0.3,
        },
        1.0,
    );
    b.cube(tail, 0, (0.0, 0.0), [0.0, -1.0, 0.0], [0.0, 2.0, 7.0], NONE);
    b.finish(1.0)
}

/// `AdultArmadilloModel`: double-shelled body + angled tail + tiny head
/// with wispy ears (the rolled-up ball cube is visibility-gated in vanilla
/// and skipped here).
fn armadillo() -> Model {
    let mut b = ModelBuilder::new();
    let body = b.part_named(
        "body",
        [0.0, 21.0, 4.0],
        [0.0; 3],
        Anim::None,
        1.0,
        Option::None,
    );
    b.cube_g(
        body,
        0,
        (0.0, 20.0),
        [-4.0, -7.0, -10.0],
        [8.0, 8.0, 12.0],
        0.3,
        NONE,
    );
    b.cube(
        body,
        0,
        (0.0, 40.0),
        [-4.0, -7.0, -10.0],
        [8.0, 8.0, 12.0],
        NONE,
    );
    let tail = b.part_named(
        "tail",
        [0.0, -3.0, 1.0],
        [0.5061, 0.0, 0.0],
        Anim::None,
        1.0,
        Some(body),
    );
    b.cube(
        tail,
        0,
        (44.0, 53.0),
        [-0.5, -0.0865, 0.0933],
        [1.0, 6.0, 1.0],
        NONE,
    );
    // Head: body child at (0, −2, −11).
    let head = b.part_named(
        "head",
        [0.0, -2.0, -11.0],
        [0.0; 3],
        Anim::ArmadilloHead,
        1.0,
        Some(body),
    );
    b.cube_f(
        head,
        0,
        (43.0, 15.0),
        [-1.5, -1.0, -1.0],
        [3.0, 5.0, 2.0],
        0.0,
        false,
        &[Fold::rot([-0.3927, 0.0, 0.0], [0.0, 0.0, 0.0])],
    );
    b.cube_f(
        head,
        0,
        (43.0, 10.0),
        [-2.0, -3.0, 0.0],
        [2.0, 5.0, 0.0],
        0.0,
        false,
        &[
            Fold::rot([0.1886, -0.3864, -0.0718], [-0.5, 0.0, -0.6]),
            Fold::at([-1.0, -1.0, 0.0]),
        ],
    );
    b.cube_f(
        head,
        0,
        (47.0, 10.0),
        [0.0, -3.0, 0.0],
        [2.0, 5.0, 0.0],
        0.0,
        false,
        &[
            Fold::rot([0.1886, 0.3864, 0.0718], [0.5, 1.0, -0.6]),
            Fold::at([1.0, -2.0, 0.0]),
        ],
    );
    let legs = [
        ("right_hind_leg", (51.0, 31.0), [-2.0, 21.0, 4.0]),
        ("left_hind_leg", (42.0, 31.0), [2.0, 21.0, 4.0]),
        ("right_front_leg", (51.0, 43.0), [-2.0, 21.0, -4.0]),
        ("left_front_leg", (42.0, 43.0), [2.0, 21.0, -4.0]),
    ];
    let mut hind = Vec::new();
    for (name, uv, pivot) in legs {
        let p = b.part_named(name, pivot, [0.0; 3], Anim::None, 1.0, Option::None);
        b.cube(p, 0, uv, [-1.0, 0.0, -1.0], [2.0, 3.0, 2.0], NONE);
        if name.contains("hind") {
            hind.push(p);
        }
    }
    // The rolled-up ball — vanilla's visibility swap: while hiding in the
    // shell the ball shows and the body cubes / hind legs / tail hide
    // (head + front legs tuck inside via the roll rig).
    let ball = b.part_named(
        "cube",
        [0.0, 24.0, 0.0],
        [0.0; 3],
        Anim::None,
        1.0,
        Option::None,
    );
    b.cube(
        ball,
        0,
        (0.0, 0.0),
        [-5.0, -10.0, -6.0],
        [10.0, 10.0, 10.0],
        NONE,
    );
    b.show(ball, Show::ShellOnly);
    b.show(body, Show::NotShell);
    b.show(tail, Show::NotShell);
    for p in hind {
        b.show(p, Show::NotShell);
    }
    // Walk yields while balled: `if (!isHidingInShell) applyWalk(…)`.
    b.keyframe_gated(
        &crate::anim_defs::ARMADILLO_WALK,
        KfDriver::Walk {
            speed_f: 16.5,
            scale_f: 2.5,
        },
        KfGate::NotShell,
    );
    b.keyframe_gesture(&crate::anim_defs::ARMADILLO_ROLL_UP, Gesture::ArmadilloRoll);
    b.keyframe_gesture(
        &crate::anim_defs::ARMADILLO_ROLL_OUT,
        Gesture::ArmadilloUnroll,
    );
    // SCARED holds the peek rig's end pose (the tucked ball) — the caller
    // enters the gesture with +2.5 s on the clock, vanilla's fastForward.
    b.keyframe_gesture(&crate::anim_defs::ARMADILLO_PEEK, Gesture::ArmadilloScared);
    b.finish(1.0)
}

/// `AdultAxolotlModel`: flat body with a top fin, gilled head, stubby leg
/// plates, long tail fin.
fn axolotl() -> Model {
    let mut b = ModelBuilder::new();
    // M387 — vanilla's tree (`body` → `head` → gills, `body` → legs and
    // tail), every part on `AdultAxolotlModel.setupAnim`'s blended poses
    // (`Anim::Axolotl { part }`, `axolotl_pose`'s slots).
    let ax = |part: u8| Anim::Axolotl { part };
    let body = b.part_named("body", [0.0, 19.5, 5.0], [0.0; 3], ax(0), 1.0, Option::None);
    b.cube(
        body,
        0,
        (0.0, 11.0),
        [-4.0, -2.0, -9.0],
        [8.0, 4.0, 10.0],
        NONE,
    );
    b.cube(
        body,
        0,
        (2.0, 17.0),
        [0.0, -3.0, -8.0],
        [0.0, 5.0, 9.0],
        NONE,
    );
    let head = b.part_named("head", [0.0, 0.0, -9.0], [0.0; 3], ax(1), 1.0, Some(body));
    b.cube_g(
        head,
        0,
        (0.0, 1.0),
        [-4.0, -3.0, -5.0],
        [8.0, 5.0, 5.0],
        0.001,
        NONE,
    );
    for (name, pivot, part, uv, min, dims) in [
        (
            "top_gills",
            [0.0, -3.0, -1.0],
            2u8,
            (3.0, 37.0),
            [-4.0, -3.0, 0.0],
            [8.0, 3.0, 0.0],
        ),
        (
            "left_gills",
            [-4.0, 0.0, -1.0],
            3,
            (0.0, 40.0),
            [-3.0, -5.0, 0.0],
            [3.0, 7.0, 0.0],
        ),
        (
            "right_gills",
            [4.0, 0.0, -1.0],
            4,
            (11.0, 40.0),
            [0.0, -5.0, 0.0],
            [3.0, 7.0, 0.0],
        ),
    ] {
        let g = b.part_named(name, pivot, [0.0; 3], ax(part), 1.0, Some(head));
        b.cube_g(g, 0, uv, min, dims, 0.001, NONE);
    }
    for (name, pivot, part, min_x) in [
        ("right_hind_leg", [-3.5, 1.0, -1.0], 5u8, -2.0),
        ("left_hind_leg", [3.5, 1.0, -1.0], 6, -1.0),
        ("right_front_leg", [-3.5, 1.0, -8.0], 7, -2.0),
        ("left_front_leg", [3.5, 1.0, -8.0], 8, -1.0),
    ] {
        let leg = b.part_named(name, pivot, [0.0; 3], ax(part), 1.0, Some(body));
        b.cube_g(
            leg,
            0,
            (2.0, 13.0),
            [min_x, 0.0, 0.0],
            [3.0, 5.0, 0.0],
            0.001,
            NONE,
        );
    }
    let tail = b.part_named("tail", [0.0, 0.0, 1.0], [0.0; 3], ax(9), 1.0, Some(body));
    b.cube(
        tail,
        0,
        (2.0, 19.0),
        [0.0, -3.0, 0.0],
        [0.0, 5.0, 12.0],
        NONE,
    );
    b.finish(1.0)
}

/// `DolphinModel`: streamlined body, angled fins, two-segment tail, nosed
/// head.
fn dolphin() -> Model {
    let mut b = ModelBuilder::new();
    // M371 — the body is a part: `DolphinModel.setupAnim` turns it with the
    // look (and bobs it while swimming), and everything rides on it.
    let bp = b.part_named(
        "body",
        [0.0, 22.0, -5.0],
        [0.0; 3],
        Anim::DolphinBody,
        1.0,
        Option::None,
    );
    b.cube_f(
        bp,
        0,
        (22.0, 0.0),
        [-4.0, -7.0, 0.0],
        [8.0, 7.0, 13.0],
        0.0,
        false,
        NONE,
    );
    b.cube_f(
        bp,
        0,
        (51.0, 0.0),
        [-0.5, 0.0, 8.0],
        [1.0, 4.0, 5.0],
        0.0,
        false,
        &[Fold::rot([PI / 3.0, 0.0, 0.0], [0.0, 0.0, 0.0])],
    );
    b.cube_f(
        bp,
        0,
        (48.0, 20.0),
        [-0.5, -4.0, 0.0],
        [1.0, 4.0, 7.0],
        0.0,
        true,
        &[Fold::rot([PI / 3.0, 0.0, PI * 2.0 / 3.0], [2.0, -2.0, 4.0])],
    );
    b.cube_f(
        bp,
        0,
        (48.0, 20.0),
        [-0.5, -4.0, 0.0],
        [1.0, 4.0, 7.0],
        0.0,
        false,
        &[Fold::rot(
            [PI / 3.0, 0.0, -PI * 2.0 / 3.0],
            [-2.0, -2.0, 4.0],
        )],
    );
    // Two-segment tail: both sway with the swim stroke, fin harder.
    let tail = b.part_ext(
        [0.0, -2.5, 11.0],
        [-0.10471976, 0.0, 0.0],
        Anim::DolphinTail {
            k: 0.1,
            rest: -0.10471976,
        },
        1.0,
        Some(bp),
    );
    b.cube(
        tail,
        0,
        (0.0, 19.0),
        [-2.0, -2.5, 0.0],
        [4.0, 5.0, 11.0],
        NONE,
    );
    let tail_fin = b.part_ext(
        [0.0, 0.0, 9.0],
        [0.0; 3],
        Anim::DolphinTail { k: 0.2, rest: 0.0 },
        1.0,
        Some(tail),
    );
    b.cube(
        tail_fin,
        0,
        (19.0, 20.0),
        [-5.0, -0.5, 0.0],
        [10.0, 1.0, 6.0],
        NONE,
    );
    let head = Fold::at([0.0, -4.0, -3.0]);
    b.cube_f(
        bp,
        0,
        (0.0, 0.0),
        [-4.0, -3.0, -3.0],
        [8.0, 7.0, 6.0],
        0.0,
        false,
        &[head],
    );
    b.cube_f(
        bp,
        0,
        (0.0, 13.0),
        [-1.0, 2.0, -7.0],
        [2.0, 2.0, 4.0],
        0.0,
        false,
        &[head],
    );
    b.finish(1.0)
}

/// `AdultTurtleModel`: shell + belly plates, flipper legs, headed front.
/// M315 — `BabyTurtleModel.createBodyLayer()` (16 x 16): the shell box, the
/// head (which looks, as the adult's does), and four flat legs, which paddle
/// as the adult's flippers do since M365. Drawn at its own size: `AgeableMobRenderer`
/// adds no scale (`getAgeScale`'s 0.3 is the bounds').
fn turtle_baby() -> Model {
    let mut b = ModelBuilder::new();
    b.cube_f(
        STATIC_PART,
        0,
        (0.0, 0.0),
        [-2.0, -1.0, -2.0],
        [4.0, 2.0, 4.0],
        0.0,
        false,
        &[Fold::at([0.0, 22.9, 1.0])],
    );
    let head = b.part([0.0, 22.9, -1.0], Anim::Head, 1.0);
    b.cube(
        head,
        0,
        (0.0, 6.0),
        [-1.5, -2.0, -3.0],
        [3.0, 3.0, 3.0],
        NONE,
    );
    // M365 — the four flat legs paddle as the adult's flippers do (the
    // baby has no egg belly, so its egg flag is never set).
    for (uv, min, pivot, hind, left) in [
        (
            (-1.0, 0.0),
            [-2.0, 0.0, -0.5],
            [-2.0, 23.9, 2.5],
            true,
            false,
        ),
        ((-1.0, 1.0), [0.0, 0.0, -0.5], [2.0, 23.9, 2.5], true, true),
        (
            (8.0, 6.0),
            [-2.0, 0.0, -0.5],
            [-2.0, 23.9, -0.5],
            false,
            false,
        ),
        ((8.0, 7.0), [0.0, 0.0, -0.5], [2.0, 23.9, -0.5], false, true),
    ] {
        let leg = b.part(pivot, Anim::TurtleLeg { hind, left }, 1.0);
        b.cube(leg, 0, uv, min, [2.0, 0.0, 1.0], NONE);
    }
    b.finish(1.0)
}

/// M316 — `BabyPolarBearModel.createBodyLayer()` (64 x 64): the body, the
/// head with its snout and ears (looking, as the adult's does), and four
/// 3-px legs on the quadruped walk. At its own size (`AgeableMobRenderer`
/// adds none). Like Rewo's adult bear it does not rear.
fn polar_bear_baby() -> Model {
    let mut b = ModelBuilder::new();
    b.cube_f(
        STATIC_PART,
        0,
        (0.0, 9.0),
        [-4.0, -3.5, -6.0],
        [8.0, 7.0, 12.0],
        0.0,
        false,
        &[Fold::at([0.0, 17.5, 0.0])],
    );
    let head = b.part([0.0, 18.625, -5.75], Anim::Head, 1.0);
    for (uv, min, dims) in [
        ((0.0, 0.0), [-3.0, -2.625, -4.25], [6.0, 5.0, 4.0]),
        ((20.0, 3.0), [-2.0, 0.375, -6.25], [4.0, 2.0, 2.0]),
        ((20.0, 0.0), [-4.0, -3.625, -2.75], [2.0, 2.0, 1.0]),
        ((26.0, 0.0), [2.0, -3.625, -2.75], [2.0, 2.0, 1.0]),
    ] {
        b.cube(head, 0, uv, min, dims, NONE);
    }
    for (uv, pivot, anim) in [
        ((0.0, 34.0), [-2.5, 21.5, 4.5], Anim::QuadHindRight),
        ((12.0, 34.0), [2.5, 21.5, 4.5], Anim::QuadHindLeft),
        ((0.0, 28.0), [-2.5, 21.5, -4.5], Anim::QuadFrontRight),
        ((12.0, 28.0), [2.5, 21.5, -4.5], Anim::QuadFrontLeft),
    ] {
        let leg = b.part(pivot, anim, 1.0);
        b.cube(leg, 0, uv, [-1.5, -0.5, -1.5], [3.0, 3.0, 3.0], NONE);
    }
    b.finish(1.0)
}

fn turtle() -> Model {
    let mut b = ModelBuilder::new();
    let head = b.part([0.0, 19.0, -10.0], Anim::TurtleHead, 1.0);
    b.cube(
        head,
        0,
        (3.0, 0.0),
        [-3.0, -1.0, -3.0],
        [6.0, 5.0, 6.0],
        NONE,
    );
    // M365 — the body and the egg belly are parts (`body`, `egg_belly`), so
    // the egg lifts them and the belly can hide.
    let body = b.part_ext(
        [0.0, 11.0, -10.0],
        [FRAC_PI_2, 0.0, 0.0],
        Anim::TurtleBody,
        1.0,
        Option::None,
    );
    b.cube(
        body,
        0,
        (7.0, 37.0),
        [-9.5, 3.0, -10.0],
        [19.0, 20.0, 6.0],
        NONE,
    );
    b.cube(
        body,
        0,
        (31.0, 1.0),
        [-5.5, 3.0, -13.0],
        [11.0, 18.0, 3.0],
        NONE,
    );
    let egg = b.part_ext(
        [0.0, 11.0, -10.0],
        [FRAC_PI_2, 0.0, 0.0],
        Anim::TurtleBody,
        1.0,
        Option::None,
    );
    b.show(egg, Show::TurtleEgg);
    b.cube(
        egg,
        0,
        (70.0, 33.0),
        [-4.5, 3.0, -14.0],
        [9.0, 18.0, 1.0],
        NONE,
    );
    // M365 — the four flippers as parts on `TurtleModel`'s paddle.
    for (uv, min, dims, pivot, hind, left) in [
        (
            (1.0, 23.0),
            [-2.0, 0.0, 0.0],
            [4.0, 1.0, 10.0],
            [-3.5, 22.0, 11.0],
            true,
            false,
        ),
        (
            (1.0, 12.0),
            [-2.0, 0.0, 0.0],
            [4.0, 1.0, 10.0],
            [3.5, 22.0, 11.0],
            true,
            true,
        ),
        (
            (27.0, 30.0),
            [-13.0, 0.0, -2.0],
            [13.0, 1.0, 5.0],
            [-5.0, 21.0, -4.0],
            false,
            false,
        ),
        (
            (27.0, 24.0),
            [0.0, 0.0, -2.0],
            [13.0, 1.0, 5.0],
            [5.0, 21.0, -4.0],
            false,
            true,
        ),
    ] {
        let leg = b.part(pivot, Anim::TurtleLeg { hind, left }, 1.0);
        b.cube(leg, 0, uv, min, dims, NONE);
    }
    b.finish(1.0)
}

// ---------------------------------------------------------------------------
// Fish
// ---------------------------------------------------------------------------

fn cod() -> Model {
    let mut b = ModelBuilder::new();
    b.cube_f(
        STATIC_PART,
        0,
        (0.0, 0.0),
        [-1.0, -2.0, 0.0],
        [2.0, 4.0, 7.0],
        0.0,
        false,
        &[Fold::at([0.0, 22.0, 0.0])],
    );
    b.cube_f(
        STATIC_PART,
        0,
        (11.0, 0.0),
        [-1.0, -2.0, -3.0],
        [2.0, 4.0, 3.0],
        0.0,
        false,
        &[Fold::at([0.0, 22.0, 0.0])],
    );
    b.cube_f(
        STATIC_PART,
        0,
        (0.0, 0.0),
        [-1.0, -2.0, -1.0],
        [2.0, 3.0, 1.0],
        0.0,
        false,
        &[Fold::at([0.0, 22.0, -3.0])],
    );
    b.cube_f(
        STATIC_PART,
        0,
        (22.0, 1.0),
        [-2.0, 0.0, -1.0],
        [2.0, 0.0, 2.0],
        0.0,
        false,
        &[Fold::rot([0.0, 0.0, -PI / 4.0], [-1.0, 23.0, 0.0])],
    );
    b.cube_f(
        STATIC_PART,
        0,
        (22.0, 4.0),
        [0.0, 0.0, -1.0],
        [2.0, 0.0, 2.0],
        0.0,
        false,
        &[Fold::rot([0.0, 0.0, PI / 4.0], [1.0, 23.0, 0.0])],
    );
    let tail = b.part(
        [0.0, 22.0, 7.0],
        Anim::FishTail {
            amp: 0.45,
            speed: 0.6,
        },
        1.0,
    );
    b.cube(
        tail,
        0,
        (22.0, 3.0),
        [0.0, -2.0, 0.0],
        [0.0, 4.0, 4.0],
        NONE,
    );
    b.cube_f(
        STATIC_PART,
        0,
        (20.0, -6.0),
        [0.0, -1.0, -1.0],
        [0.0, 1.0, 6.0],
        0.0,
        false,
        &[Fold::at([0.0, 20.0, 0.0])],
    );
    b.finish(1.0)
}

fn salmon() -> Model {
    let mut b = ModelBuilder::new();
    let front = Fold::at([0.0, 20.0, -7.2]);
    b.cube_f(
        STATIC_PART,
        0,
        (0.0, 0.0),
        [-1.5, -2.5, 0.0],
        [3.0, 5.0, 8.0],
        0.0,
        false,
        &[front],
    );
    // The whole back half (+ its fins) sways while swimming.
    let back = b.part(
        [0.0, 20.0, 0.8],
        Anim::FishTail {
            amp: 0.25,
            speed: 0.6,
        },
        1.0,
    );
    b.cube(
        back,
        0,
        (0.0, 13.0),
        [-1.5, -2.5, 0.0],
        [3.0, 5.0, 8.0],
        NONE,
    );
    b.cube_f(
        back,
        0,
        (20.0, 10.0),
        [0.0, -2.5, 0.0],
        [0.0, 5.0, 6.0],
        0.0,
        false,
        &[Fold::at([0.0, 0.0, 8.0])],
    );
    b.cube_f(
        back,
        0,
        (0.0, 2.0),
        [0.0, 0.0, 0.0],
        [0.0, 2.0, 4.0],
        0.0,
        false,
        &[Fold::at([0.0, -4.5, -1.0])],
    );
    b.cube_f(
        STATIC_PART,
        0,
        (22.0, 0.0),
        [-1.0, -2.0, -3.0],
        [2.0, 4.0, 3.0],
        0.0,
        false,
        &[front],
    );
    b.cube_f(
        STATIC_PART,
        0,
        (2.0, 1.0),
        [0.0, 0.0, 0.0],
        [0.0, 2.0, 3.0],
        0.0,
        false,
        &[Fold::at([0.0, -4.5, 5.0]), front],
    );
    b.cube_f(
        STATIC_PART,
        0,
        (-4.0, 0.0),
        [-2.0, 0.0, 0.0],
        [2.0, 0.0, 2.0],
        0.0,
        false,
        &[Fold::rot([0.0, 0.0, -PI / 4.0], [-1.5, 21.5, -7.2])],
    );
    b.cube_f(
        STATIC_PART,
        0,
        (0.0, 0.0),
        [0.0, 0.0, 0.0],
        [2.0, 0.0, 2.0],
        0.0,
        false,
        &[Fold::rot([0.0, 0.0, PI / 4.0], [1.5, 21.5, -7.2])],
    );
    b.finish(1.0)
}

/// `PufferfishBigModel` — the fully inflated form, spikes and all.
fn pufferfish() -> Model {
    let mut b = ModelBuilder::new();
    b.cube_f(
        STATIC_PART,
        0,
        (0.0, 0.0),
        [-4.0, -8.0, -4.0],
        [8.0, 8.0, 8.0],
        0.0,
        false,
        &[Fold::at([0.0, 22.0, 0.0])],
    );
    let fin_r = b.part([-4.0, 15.0, -2.0], Anim::PufferFin { left: false }, 1.0);
    b.cube(
        fin_r,
        0,
        (24.0, 0.0),
        [-2.0, 0.0, -1.0],
        [2.0, 1.0, 2.0],
        NONE,
    );
    let fin_l = b.part([4.0, 15.0, -2.0], Anim::PufferFin { left: true }, 1.0);
    b.cube(
        fin_l,
        0,
        (24.0, 3.0),
        [0.0, 0.0, -1.0],
        [2.0, 1.0, 2.0],
        NONE,
    );
    let spikes: [((f32, f32), [f32; 3], [f32; 3], [f32; 3], [f32; 3]); 10] = [
        (
            (15.0, 17.0),
            [-4.0, -1.0, 0.0],
            [8.0, 1.0, 0.0],
            [PI / 4.0, 0.0, 0.0],
            [0.0, 14.0, -4.0],
        ),
        (
            (14.0, 16.0),
            [-4.0, -1.0, 0.0],
            [8.0, 1.0, 1.0],
            [0.0, 0.0, 0.0],
            [0.0, 14.0, 0.0],
        ),
        (
            (23.0, 18.0),
            [-4.0, -1.0, 0.0],
            [8.0, 1.0, 0.0],
            [-PI / 4.0, 0.0, 0.0],
            [0.0, 14.0, 4.0],
        ),
        (
            (5.0, 17.0),
            [-1.0, -8.0, 0.0],
            [1.0, 8.0, 0.0],
            [0.0, -PI / 4.0, 0.0],
            [-4.0, 22.0, -4.0],
        ),
        (
            (1.0, 17.0),
            [0.0, -8.0, 0.0],
            [1.0, 8.0, 0.0],
            [0.0, PI / 4.0, 0.0],
            [4.0, 22.0, -4.0],
        ),
        (
            (15.0, 20.0),
            [-4.0, 0.0, 0.0],
            [8.0, 1.0, 0.0],
            [-PI / 4.0, 0.0, 0.0],
            [0.0, 22.0, -4.0],
        ),
        (
            (15.0, 20.0),
            [-4.0, 0.0, 0.0],
            [8.0, 1.0, 0.0],
            [0.0, 0.0, 0.0],
            [0.0, 22.0, 0.0],
        ),
        (
            (15.0, 20.0),
            [-4.0, 0.0, 0.0],
            [8.0, 1.0, 0.0],
            [PI / 4.0, 0.0, 0.0],
            [0.0, 22.0, 4.0],
        ),
        (
            (9.0, 17.0),
            [-1.0, -8.0, 0.0],
            [1.0, 8.0, 0.0],
            [0.0, PI / 4.0, 0.0],
            [-4.0, 22.0, 4.0],
        ),
        (
            (9.0, 17.0),
            [0.0, -8.0, 0.0],
            [1.0, 8.0, 0.0],
            [0.0, -PI / 4.0, 0.0],
            [4.0, 22.0, 4.0],
        ),
    ];
    for (uv, min, dims, rot, off) in spikes {
        b.cube_f(
            STATIC_PART,
            0,
            uv,
            min,
            dims,
            0.0,
            false,
            &[Fold::rot(rot, off)],
        );
    }
    b.finish(1.0)
}

/// `CubeDeformation FISH_PATTERN_DEFORMATION` — the inflation
/// `LayerDefinitions` bakes `TROPICAL_FISH_{SMALL,LARGE}_PATTERN` with, and
/// the only difference between the pattern mesh and the body mesh.
const FISH_PATTERN_GROW: f32 = 0.008;

/// `TropicalFishSmallModel` (shape A) + `TropicalFishPatternLayer`'s copy of
/// it on texture 1, inflated by [`FISH_PATTERN_GROW`] (M68).
///
/// Which of the six `tropical_a_pattern_*` sheets that slot samples is a
/// per-draw variant id, not a second model — see
/// `rewo_data::mob_variants::fish_pattern_variant`.
fn tropical_fish() -> Model {
    let mut b = ModelBuilder::new();
    let fin_r = Fold::rot([0.0, PI / 4.0, 0.0], [-1.0, 22.5, 0.0]);
    let fin_l = Fold::rot([0.0, -PI / 4.0, 0.0], [1.0, 22.5, 0.0]);
    let tail = b.part(
        [0.0, 22.0, 3.0],
        Anim::FishTail {
            amp: 0.45,
            speed: 0.6,
        },
        1.0,
    );
    for (tex, g) in [(0usize, 0.0f32), (1, FISH_PATTERN_GROW)] {
        b.cube_f(
            STATIC_PART,
            tex,
            (0.0, 0.0),
            [-1.0, -1.5, -3.0],
            [2.0, 3.0, 6.0],
            g,
            false,
            &[Fold::at([0.0, 22.0, 0.0])],
        );
        b.cube_f(
            tail,
            tex,
            (22.0, -6.0),
            [0.0, -1.5, 0.0],
            [0.0, 3.0, 6.0],
            g,
            false,
            NONE,
        );
        b.cube_f(
            STATIC_PART,
            tex,
            (2.0, 16.0),
            [-2.0, -1.0, 0.0],
            [2.0, 2.0, 0.0],
            g,
            false,
            &[fin_r],
        );
        b.cube_f(
            STATIC_PART,
            tex,
            (2.0, 12.0),
            [0.0, -1.0, 0.0],
            [2.0, 2.0, 0.0],
            g,
            false,
            &[fin_l],
        );
        b.cube_f(
            STATIC_PART,
            tex,
            (10.0, -5.0),
            [0.0, -3.0, 0.0],
            [0.0, 3.0, 6.0],
            g,
            false,
            &[Fold::at([0.0, 20.5, -3.0])],
        );
    }
    b.finish(1.0)
}

/// `TropicalFishLargeModel` (shape B) — the *other* body plan, and the one
/// `tropical_b.png` belongs to (M68).
///
/// Not a rescale of the small one: a 2x6x6 body against 2x3x6, a 5-deep tail
/// against 6, fins pivoted a block higher, and a **bottom fin** the small
/// plan has no part for at all.
fn tropical_fish_large() -> Model {
    let mut b = ModelBuilder::new();
    let fin_r = Fold::rot([0.0, PI / 4.0, 0.0], [-1.0, 20.0, 0.0]);
    let fin_l = Fold::rot([0.0, -PI / 4.0, 0.0], [1.0, 20.0, 0.0]);
    let tail = b.part(
        [0.0, 19.0, 3.0],
        Anim::FishTail {
            amp: 0.45,
            speed: 0.6,
        },
        1.0,
    );
    for (tex, g) in [(0usize, 0.0f32), (1, FISH_PATTERN_GROW)] {
        b.cube_f(
            STATIC_PART,
            tex,
            (0.0, 20.0),
            [-1.0, -3.0, -3.0],
            [2.0, 6.0, 6.0],
            g,
            false,
            &[Fold::at([0.0, 19.0, 0.0])],
        );
        b.cube_f(
            tail,
            tex,
            (21.0, 16.0),
            [0.0, -3.0, 0.0],
            [0.0, 6.0, 5.0],
            g,
            false,
            NONE,
        );
        b.cube_f(
            STATIC_PART,
            tex,
            (2.0, 16.0),
            [-2.0, 0.0, 0.0],
            [2.0, 2.0, 0.0],
            g,
            false,
            &[fin_r],
        );
        b.cube_f(
            STATIC_PART,
            tex,
            (2.0, 12.0),
            [0.0, 0.0, 0.0],
            [2.0, 2.0, 0.0],
            g,
            false,
            &[fin_l],
        );
        b.cube_f(
            STATIC_PART,
            tex,
            (20.0, 11.0),
            [0.0, -4.0, 0.0],
            [0.0, 4.0, 6.0],
            g,
            false,
            &[Fold::at([0.0, 16.0, -3.0])],
        );
        b.cube_f(
            STATIC_PART,
            tex,
            (20.0, 21.0),
            [0.0, 0.0, 0.0],
            [0.0, 4.0, 6.0],
            g,
            false,
            &[Fold::at([0.0, 22.0, -3.0])],
        );
    }
    b.finish(1.0)
}

// ---------------------------------------------------------------------------
// Big passives
// ---------------------------------------------------------------------------

/// `PandaModel`: wide head with ears + nose, huge rotated body, thick legs.
fn panda() -> Model {
    let mut b = ModelBuilder::new();
    let head = b.part([0.0, 11.5, -17.0], Anim::Head, 1.0);
    b.cube(
        head,
        0,
        (0.0, 6.0),
        [-6.5, -5.0, -4.0],
        [13.0, 10.0, 9.0],
        NONE,
    );
    b.cube(
        head,
        0,
        (45.0, 16.0),
        [-3.5, 0.0, -6.0],
        [7.0, 5.0, 2.0],
        NONE,
    );
    b.cube(
        head,
        0,
        (52.0, 25.0),
        [3.5, -8.0, -1.0],
        [5.0, 4.0, 1.0],
        NONE,
    );
    b.cube(
        head,
        0,
        (52.0, 25.0),
        [-8.5, -8.0, -1.0],
        [5.0, 4.0, 1.0],
        NONE,
    );
    let body = Fold::rot([FRAC_PI_2, 0.0, 0.0], [0.0, 10.0, 0.0]);
    b.cube_f(
        STATIC_PART,
        0,
        (0.0, 25.0),
        [-9.5, -13.0, -6.5],
        [19.0, 26.0, 13.0],
        0.0,
        false,
        &[body],
    );
    let legs = [
        ([-5.5, 15.0, 9.0], Anim::QuadHindRight),
        ([5.5, 15.0, 9.0], Anim::QuadHindLeft),
        ([-5.5, 15.0, -9.0], Anim::QuadFrontRight),
        ([5.5, 15.0, -9.0], Anim::QuadFrontLeft),
    ];
    for (pivot, anim) in legs {
        let p = b.part(pivot, anim, 1.0);
        b.cube(p, 0, (40.0, 0.0), [-3.0, 0.0, -3.0], [6.0, 9.0, 6.0], NONE);
    }
    b.finish(1.0)
}

/// `PolarBearModel` (mesh-scaled 1.2 in vanilla — ground-anchored, equal
/// to our scale).
fn polar_bear() -> Model {
    let mut b = ModelBuilder::new();
    let head = b.part([0.0, 10.0, -16.0], Anim::Head, 1.0);
    b.cube(
        head,
        0,
        (0.0, 0.0),
        [-3.5, -3.0, -3.0],
        [7.0, 7.0, 7.0],
        NONE,
    );
    b.cube(
        head,
        0,
        (0.0, 44.0),
        [-2.5, 1.0, -6.0],
        [5.0, 3.0, 3.0],
        NONE,
    );
    b.cube(
        head,
        0,
        (26.0, 0.0),
        [-4.5, -4.0, -1.0],
        [2.0, 2.0, 1.0],
        NONE,
    );
    b.cube_m(
        head,
        0,
        (26.0, 0.0),
        [2.5, -4.0, -1.0],
        [2.0, 2.0, 1.0],
        NONE,
    );
    let body = Fold::rot([FRAC_PI_2, 0.0, 0.0], [-2.0, 9.0, 12.0]);
    b.cube_f(
        STATIC_PART,
        0,
        (0.0, 19.0),
        [-5.0, -13.0, -7.0],
        [14.0, 14.0, 11.0],
        0.0,
        false,
        &[body],
    );
    b.cube_f(
        STATIC_PART,
        0,
        (39.0, 0.0),
        [-4.0, -25.0, -7.0],
        [12.0, 12.0, 10.0],
        0.0,
        false,
        &[body],
    );
    let legs = [
        (
            (50.0, 22.0),
            [4.0, 10.0, 8.0],
            [-4.5, 14.0, 6.0],
            Anim::QuadHindRight,
        ),
        (
            (50.0, 22.0),
            [4.0, 10.0, 8.0],
            [4.5, 14.0, 6.0],
            Anim::QuadHindLeft,
        ),
        (
            (50.0, 40.0),
            [4.0, 10.0, 6.0],
            [-3.5, 14.0, -8.0],
            Anim::QuadFrontRight,
        ),
        (
            (50.0, 40.0),
            [4.0, 10.0, 6.0],
            [3.5, 14.0, -8.0],
            Anim::QuadFrontLeft,
        ),
    ];
    for (uv, dims, pivot, anim) in legs {
        let p = b.part(pivot, anim, 1.0);
        b.cube(p, 0, uv, [-2.0, 0.0, -2.0], dims, NONE);
    }
    b.finish(1.2)
}

/// `AdultCamelModel`: long body with hump + tail, towering neck/head,
/// stilt legs — walking on the vanilla CAMEL_WALK keyframe rig (which also
/// bobs the whole "root" and flicks the ears).
fn camel() -> Model {
    camel_with(CamelSaddle::None)
}

/// M300 — which of `CamelSaddleModel`'s boxes [`camel_with`] adds.
#[derive(Clone, Copy, PartialEq, Eq)]
enum CamelSaddle {
    None,
    Saddle,
    WithReins,
}

/// M300 — `CamelRenderer`'s `CAMEL_SADDLE` layer: `CamelSaddleModel` is the
/// camel's mesh (`createBodyMesh()`, drawn in the saddle's sheet,
/// transparent there) plus the saddle and bridle (inflated 0.05), and the
/// reins shown only while ridden, on a 128 x 128 sheet. Returns the saddle
/// model and the reins alone; both keep the camel's parts.
pub fn camel_saddle() -> (Model, Model) {
    let saddle = camel_with(CamelSaddle::Saddle);
    let mut reins = camel_with(CamelSaddle::WithReins);
    reins.quads.drain(..saddle.quads.len());
    (saddle, reins)
}

fn camel_with(saddle: CamelSaddle) -> Model {
    let mut b = ModelBuilder::new();
    let root = b.part_named(
        "root",
        [0.0, 0.0, 0.0],
        [0.0; 3],
        Anim::None,
        1.0,
        Option::None,
    );
    let body = Fold::at([0.0, 4.0, 9.5]);
    b.cube_f(
        root,
        0,
        (0.0, 25.0),
        [-7.5, -12.0, -23.5],
        [15.0, 12.0, 27.0],
        0.0,
        false,
        &[body],
    );
    b.cube_f(
        root,
        0,
        (74.0, 0.0),
        [-4.5, -5.0, -5.5],
        [9.0, 5.0, 11.0],
        0.0,
        false,
        &[Fold::at([0.0, -12.0, -10.0]), body],
    );
    let tail = b.part_named(
        "tail",
        [0.0, -5.0, 13.0],
        [0.0; 3],
        Anim::None,
        1.0,
        Some(root),
    );
    b.cube(
        tail,
        0,
        (122.0, 0.0),
        [-1.5, 0.0, 0.0],
        [3.0, 14.0, 0.0],
        NONE,
    );
    let head = b.part_named(
        "head",
        [0.0, 1.0, -10.0],
        [0.0; 3],
        Anim::CamelHead,
        1.0,
        Some(root),
    );
    b.cube(
        head,
        0,
        (60.0, 24.0),
        [-3.5, -7.0, -15.0],
        [7.0, 8.0, 19.0],
        NONE,
    );
    b.cube(
        head,
        0,
        (21.0, 0.0),
        [-3.5, -21.0, -15.0],
        [7.0, 14.0, 7.0],
        NONE,
    );
    b.cube(
        head,
        0,
        (50.0, 0.0),
        [-2.5, -21.0, -21.0],
        [5.0, 5.0, 6.0],
        NONE,
    );
    let ear_l = b.part_named(
        "left_ear",
        [2.5, -21.0, -9.5],
        [0.0; 3],
        Anim::None,
        1.0,
        Some(head),
    );
    b.cube(
        ear_l,
        0,
        (45.0, 0.0),
        [-0.5, 0.5, -1.0],
        [3.0, 1.0, 2.0],
        NONE,
    );
    let ear_r = b.part_named(
        "right_ear",
        [-2.5, -21.0, -9.5],
        [0.0; 3],
        Anim::None,
        1.0,
        Some(head),
    );
    b.cube(
        ear_r,
        0,
        (67.0, 0.0),
        [-2.5, 0.5, -1.0],
        [3.0, 1.0, 2.0],
        NONE,
    );
    let legs = [
        ("left_hind_leg", (58.0, 16.0), [4.9, 1.0, 9.5]),
        ("right_hind_leg", (94.0, 16.0), [-4.9, 1.0, 9.5]),
        ("left_front_leg", (0.0, 0.0), [4.9, 1.0, -10.5]),
        ("right_front_leg", (0.0, 26.0), [-4.9, 1.0, -10.5]),
    ];
    for (name, uv, pivot) in legs {
        let p = b.part_named(name, pivot, [0.0; 3], Anim::None, 1.0, Some(root));
        b.cube(p, 0, uv, [-2.5, 2.0, -2.5], [5.0, 21.0, 5.0], NONE);
    }
    b.keyframe(
        &crate::anim_defs::CAMEL_WALK,
        KfDriver::Walk {
            speed_f: 2.0,
            scale_f: 2.5,
        },
    );
    // M300 — `CamelSaddleModel.createSaddleLayer()`'s boxes (the saddle on
    // the body, the bridle on the head, then the reins, `visible =
    // isRidden`, last so `camel_saddle` can split them off).
    if saddle != CamelSaddle::None {
        let g = 0.05;
        for (uv, min, dims) in [
            ((74.0, 64.0), [-4.5, -17.0, -15.5], [9.0, 5.0, 11.0]),
            ((92.0, 114.0), [-3.5, -20.0, -15.5], [7.0, 3.0, 11.0]),
            ((0.0, 89.0), [-7.5, -12.0, -23.5], [15.0, 12.0, 27.0]),
        ] {
            b.cube_f(root, 0, uv, min, dims, g, false, &[body]);
        }
        for (uv, min, dims, g, mirror) in [
            (
                (60.0, 87.0),
                [-3.5, -7.0, -15.0],
                [7.0, 8.0, 19.0],
                g,
                false,
            ),
            (
                (21.0, 64.0),
                [-3.5, -21.0, -15.0],
                [7.0, 14.0, 7.0],
                g,
                false,
            ),
            (
                (50.0, 64.0),
                [-2.5, -21.0, -21.0],
                [5.0, 5.0, 6.0],
                g,
                false,
            ),
            (
                (74.0, 70.0),
                [2.5, -19.0, -18.0],
                [1.0, 2.0, 2.0],
                0.0,
                false,
            ),
            (
                (74.0, 70.0),
                [-3.5, -19.0, -18.0],
                [1.0, 2.0, 2.0],
                0.0,
                true,
            ),
        ] {
            b.cube_f(head, 0, uv, min, dims, g, mirror, NONE);
        }
        if saddle == CamelSaddle::WithReins {
            for (uv, min, dims) in [
                ((98.0, 42.0), [3.51, -18.0, -17.0], [0.0, 7.0, 15.0]),
                ((84.0, 57.0), [-3.5, -18.0, -2.0], [7.0, 7.0, 0.0]),
                ((98.0, 42.0), [-3.51, -18.0, -17.0], [0.0, 7.0, 15.0]),
            ] {
                b.cube_f(head, 0, uv, min, dims, 0.0, false, NONE);
            }
        }
    }
    b.finish(1.0)
}

/// `LlamaModel`: tall neck + eared head, rotated body, side chest boxes
/// (M297: shown only with `hasChest`, as `setupAnim` does), straight legs.
fn llama() -> Model {
    llama_with(0.0)
}

/// M297 — `ModelLayers.LLAMA_DECOR`: `LlamaModel.createBodyLayer(new
/// CubeDeformation(0.5F))`, the carpet's (and the trader llama's blanket's)
/// shell, `LLAMA_BODY`'s equipment layer. The same parts as [`llama`].
pub fn llama_decor() -> Model {
    llama_with(0.5)
}

/// `LlamaModel.createBodyLayer(g)`: `g` on every box.
fn llama_with(g: f32) -> Model {
    let mut b = ModelBuilder::new();
    let head = b.part([0.0, 7.0, -6.0], Anim::Head, 1.0);
    b.cube_f(
        head,
        0,
        (0.0, 0.0),
        [-2.0, -14.0, -10.0],
        [4.0, 4.0, 9.0],
        g,
        false,
        NONE,
    );
    b.cube_f(
        head,
        0,
        (0.0, 14.0),
        [-4.0, -16.0, -6.0],
        [8.0, 18.0, 6.0],
        g,
        false,
        NONE,
    );
    b.cube_f(
        head,
        0,
        (17.0, 0.0),
        [-4.0, -19.0, -4.0],
        [3.0, 3.0, 2.0],
        g,
        false,
        NONE,
    );
    b.cube_f(
        head,
        0,
        (17.0, 0.0),
        [1.0, -19.0, -4.0],
        [3.0, 3.0, 2.0],
        g,
        false,
        NONE,
    );
    let body = Fold::rot([FRAC_PI_2, 0.0, 0.0], [0.0, 5.0, 2.0]);
    b.cube_f(
        STATIC_PART,
        0,
        (29.0, 0.0),
        [-6.0, -10.0, -7.0],
        [12.0, 18.0, 10.0],
        g,
        false,
        &[body],
    );
    // M297 — `rightChest.visible = leftChest.visible = state.hasChest`.
    let chest = b.part([0.0, 0.0, 0.0], Anim::None, 1.0);
    b.show(chest, Show::Chested);
    for (uv, x) in [((45.0, 28.0), -8.5), ((45.0, 41.0), 5.5)] {
        b.cube_f(
            chest,
            0,
            uv,
            [-3.0, 0.0, 0.0],
            [8.0, 8.0, 3.0],
            g,
            false,
            &[Fold::rot([0.0, FRAC_PI_2, 0.0], [x, 3.0, 3.0])],
        );
    }
    let legs = [
        ([-3.5, 10.0, 6.0], Anim::QuadHindRight),
        ([3.5, 10.0, 6.0], Anim::QuadHindLeft),
        ([-3.5, 10.0, -5.0], Anim::QuadFrontRight),
        ([3.5, 10.0, -5.0], Anim::QuadFrontLeft),
    ];
    for (pivot, anim) in legs {
        let p = b.part(pivot, anim, 1.0);
        b.cube_f(
            p,
            0,
            (29.0, 29.0),
            [-2.0, 0.0, -2.0],
            [4.0, 14.0, 4.0],
            g,
            false,
            NONE,
        );
    }
    b.finish(1.0)
}

/// `ParrotModel`: leaned body, folded wings, crest feather, twin beak boxes.
/// M378 — every piece is a part on `setupAnim`: standing, the wings rest
/// splayed ∓0.0873 (`zRot`), the legs walk on the quadruped swing and the
/// tail swings 0.3·speed; sitting (`prepare(SITTING)`) everything drops
/// 1.9 px, the tail tips π/6 and the legs fold π/2. The flying (legs
/// +2π/9, the flap) and party poses are not modelled.
fn parrot() -> Model {
    let mut b = ModelBuilder::new();
    let body = b.part_ext(
        [0.0, 16.5, -3.0],
        [0.4937, 0.0, 0.0],
        Anim::ParrotSit,
        1.0,
        Option::None,
    );
    b.cube(
        body,
        0,
        (2.0, 8.0),
        [-1.5, 0.0, -1.5],
        [3.0, 6.0, 3.0],
        NONE,
    );
    let tail = b.part_ext(
        [0.0, 21.07, 1.16],
        [1.015, 0.0, 0.0],
        Anim::ParrotTail,
        1.0,
        Option::None,
    );
    b.cube(
        tail,
        0,
        (22.0, 1.0),
        [-1.5, -1.0, -1.0],
        [3.0, 4.0, 1.0],
        NONE,
    );
    // The left wing (x 1.5) rests at zRot −0.0873, the right at +0.0873.
    for (x, z) in [(1.5, -0.0873), (-1.5, 0.0873)] {
        let wing = b.part_ext(
            [x, 16.94, -2.76],
            [-0.6981, -PI, z],
            Anim::ParrotSit,
            1.0,
            Option::None,
        );
        b.cube(
            wing,
            0,
            (19.0, 8.0),
            [-0.5, 0.0, -1.5],
            [1.0, 5.0, 3.0],
            NONE,
        );
    }
    let head = b.part([0.0, 15.69, -2.76], Anim::ParrotHead, 1.0);
    b.cube(
        head,
        0,
        (2.0, 2.0),
        [-1.0, -1.5, -1.0],
        [2.0, 3.0, 2.0],
        NONE,
    );
    b.cube_f(
        head,
        0,
        (10.0, 0.0),
        [-1.0, -0.5, -2.0],
        [2.0, 1.0, 4.0],
        0.0,
        false,
        &[Fold::at([0.0, -2.0, -1.0])],
    );
    b.cube_f(
        head,
        0,
        (11.0, 7.0),
        [-0.5, -1.0, -0.5],
        [1.0, 2.0, 1.0],
        0.0,
        false,
        &[Fold::at([0.0, -0.5, -1.5])],
    );
    b.cube_f(
        head,
        0,
        (16.0, 7.0),
        [-0.5, 0.0, -0.5],
        [1.0, 2.0, 1.0],
        0.0,
        false,
        &[Fold::at([0.0, -1.75, -2.45])],
    );
    b.cube_f(
        head,
        0,
        (2.0, 18.0),
        [0.0, -4.0, -2.0],
        [0.0, 5.0, 4.0],
        0.0,
        false,
        &[Fold::rot([-0.2214, 0.0, 0.0], [0.0, -2.15, 0.15])],
    );
    for (x, left) in [(1.0, true), (-1.0, false)] {
        let leg = b.part_ext(
            [x, 22.0, -1.05],
            [-0.0299, 0.0, 0.0],
            Anim::ParrotLeg { left },
            1.0,
            Option::None,
        );
        b.cube(
            leg,
            0,
            (14.0, 18.0),
            [-0.5, 0.0, -0.5],
            [1.0, 2.0, 1.0],
            NONE,
        );
    }
    b.finish(1.0)
}

/// M293 — `LayerDefinitions`' equine scales: `ModelLayers.HORSE` is
/// `horseBodyLayer.apply(livingHorseScale)` with `MeshTransformer.scaling(
/// 1.1F)`, `DONKEY` and `MULE` are `DonkeyModel.createBodyLayer(0.87F)` and
/// `(0.92F)`, and the skeleton and zombie horses take the bare
/// `horseBodyLayer`. `MeshTransformer.scaling(s)` scales every root pose by
/// `s` and lifts it `24.016 · (1 - s)`, i.e. scales about the feet, which is
/// what a model's `scale` does here.
pub const HORSE_SCALE: f32 = 1.1;
pub const DONKEY_SCALE: f32 = 0.87;
pub const MULE_SCALE: f32 = 0.92;

fn horse() -> Model {
    let mut m = equine(false);
    m.scale *= HORSE_SCALE;
    m
}

fn donkey() -> Model {
    let mut m = equine(true);
    m.scale *= DONKEY_SCALE;
    m
}

fn mule() -> Model {
    let mut m = equine(true);
    m.scale *= MULE_SCALE;
    m
}

/// `AbstractEquineModel` (horse family). `donkey_ears` swaps the horse
/// ears for the long donkey pair (vanilla `DonkeyModel`).
fn equine(donkey_ears: bool) -> Model {
    equine_with(donkey_ears, 0.0)
}

/// M294 — `ModelLayers.HORSE_ARMOR`: `AbstractEquineModel.createBodyMesh(new
/// CubeDeformation(0.1F))` with `livingHorseScale`, the horse's body-armour
/// shell (`HorseRenderer`'s `HORSE_BODY` `SimpleEquipmentLayer`). The body
/// keeps its own 0.05, the neck and the ears theirs, as in vanilla.
pub fn horse_armor() -> Model {
    let mut m = equine_with(false, 0.1);
    m.scale *= HORSE_SCALE;
    m
}

/// M294 — `ModelLayers.UNDEAD_HORSE_ARMOR`: the same mesh without the
/// living scale (`UndeadHorseRenderer`'s `HORSE_BODY` layer).
pub fn undead_horse_armor() -> Model {
    equine_with(false, 0.1)
}

/// M295 — `EquineSaddleModel.createSaddleLayer()`: the equine body mesh
/// (`createBodyMesh(NONE)`, drawn in the saddle's sheet, transparent there)
/// plus the saddle's own boxes; with `DonkeyModel`'s transformer (the long
/// ears; its chests are not modelled) for the donkey and the mule, and each
/// kind's layer scale (`HORSE_SADDLE` with `livingHorseScale`,
/// `DonkeyModel.createSaddleLayer(0.87F / 0.92F)`, the undead horses bare).
/// The two reins (`left/right_saddle_line`, `visible = isRidden`) are a
/// second model, drawn only while ridden. Both keep the base model's parts.
pub fn equine_saddle(donkey_ears: bool, scale: f32) -> (Model, Model) {
    let mut saddle = equine_with(donkey_ears, 0.0);
    {
        let mut b = ModelBuilder::new();
        saddle_boxes(&mut b);
        let extra = b.finish(1.0);
        saddle.quads.extend(extra.quads);
    }
    saddle.scale *= scale;
    let mut b = ModelBuilder::new();
    let neck = equine_head_part(&mut b);
    let line = Fold::rot([-PI / 6.0, 0.0, 0.0], [0.0, 0.0, 0.0]);
    for x in [3.1, -3.1] {
        b.cube_f(
            neck,
            0,
            (32.0, 2.0),
            [x, -6.0, -8.0],
            [0.0, 3.0, 16.0],
            0.0,
            false,
            &[line],
        );
    }
    let mut reins = b.finish(1.0);
    reins.scale = saddle.scale;
    (saddle, reins)
}

/// M359 — `AbstractEquineModel`'s `head_parts` group (pivot (0, 4, −12),
/// π/6 at rest), created first in every equine builder so the saddle's
/// bridle and the reins (separate builders whose quads join the body's
/// parts) share its part index.
fn equine_head_part(b: &mut ModelBuilder) -> usize {
    b.part_ext(
        [0.0, 4.0, -12.0],
        [PI / 6.0, 0.0, 0.0],
        Anim::EquineHead,
        1.0,
        Option::None,
    )
}

/// `createSaddleLayer`'s boxes: the saddle on the static part with the
/// body's fold, the bridle on the head group (M359: a part, which looks).
fn saddle_boxes(b: &mut ModelBuilder) {
    let neck = equine_head_part(b);
    let body = Fold::at([0.0, 11.0, 5.0]);
    b.cube_f(
        STATIC_PART,
        0,
        (26.0, 0.0),
        [-5.0, -8.0, -9.0],
        [10.0, 9.0, 9.0],
        0.5,
        false,
        &[body],
    );
    for x in [2.0, -3.0] {
        b.cube_f(
            neck,
            0,
            (29.0, 5.0),
            [x, -9.0, -6.0],
            [1.0, 2.0, 2.0],
            0.0,
            false,
            NONE,
        );
    }
    b.cube_f(
        neck,
        0,
        (1.0, 1.0),
        [-3.0, -11.0, -1.9],
        [6.0, 5.0, 6.0],
        0.22,
        false,
        NONE,
    );
    b.cube_f(
        neck,
        0,
        (19.0, 0.0),
        [-2.0, -11.0, -4.0],
        [4.0, 5.0, 2.0],
        0.2,
        false,
        NONE,
    );
}

/// `LayerDefinition.create(..., 64, 64)` for the horse armour layers.
pub const HORSE_ARMOR_TEXTURE: [f32; 2] = [64.0, 64.0];

/// `AbstractEquineModel.createBodyMesh(g)`: `g` on the head, mane, upper
/// mouth, legs and tail; the body is always 0.05, the neck 0 and the ears
/// -0.001.
fn equine_with(donkey_ears: bool, g: f32) -> Model {
    let mut b = ModelBuilder::new();
    // M359 — the head group is a part, created first (see
    // [`equine_head_part`]).
    let neck = equine_head_part(&mut b);
    let body = Fold::at([0.0, 11.0, 5.0]);
    b.cube_f(
        STATIC_PART,
        0,
        (0.0, 32.0),
        [-5.0, -8.0, -17.0],
        [10.0, 10.0, 22.0],
        0.05,
        false,
        &[body],
    );
    b.cube_f(
        neck,
        0,
        (0.0, 35.0),
        [-2.05, -6.0, -2.0],
        [4.0, 12.0, 7.0],
        0.0,
        false,
        NONE,
    );
    b.cube_f(
        neck,
        0,
        (0.0, 13.0),
        [-3.0, -11.0, -2.0],
        [6.0, 5.0, 7.0],
        g,
        false,
        NONE,
    );
    b.cube_f(
        neck,
        0,
        (56.0, 36.0),
        [-1.0, -11.0, 5.01],
        [2.0, 16.0, 2.0],
        g,
        false,
        NONE,
    );
    b.cube_f(
        neck,
        0,
        (0.0, 25.0),
        [-2.0, -11.0, -7.0],
        [4.0, 5.0, 5.0],
        g,
        false,
        NONE,
    );
    if donkey_ears {
        b.cube_f(
            neck,
            0,
            (0.0, 12.0),
            [-1.0, -7.0, 0.0],
            [2.0, 7.0, 1.0],
            0.0,
            false,
            &[Fold::rot([PI / 12.0, 0.0, PI / 12.0], [1.25, -10.0, 4.0])],
        );
        b.cube_f(
            neck,
            0,
            (0.0, 12.0),
            [-1.0, -7.0, 0.0],
            [2.0, 7.0, 1.0],
            0.0,
            false,
            &[Fold::rot([PI / 12.0, 0.0, -PI / 12.0], [-1.25, -10.0, 4.0])],
        );
    } else {
        b.cube_f(
            neck,
            0,
            (19.0, 16.0),
            [0.55, -13.0, 4.0],
            [2.0, 3.0, 1.0],
            -0.001,
            false,
            NONE,
        );
        b.cube_f(
            neck,
            0,
            (19.0, 16.0),
            [-2.55, -13.0, 4.0],
            [2.0, 3.0, 1.0],
            -0.001,
            false,
            NONE,
        );
    }
    let legs: [([f32; 3], [f32; 3], bool, Anim); 4] = [
        (
            [-3.0, -1.01, -1.0],
            [4.0, 14.0, 7.0],
            true,
            Anim::EquineLeg {
                hind: true,
                left: true,
            },
        ),
        (
            [-1.0, -1.01, -1.0],
            [-4.0, 14.0, 7.0],
            false,
            Anim::EquineLeg {
                hind: true,
                left: false,
            },
        ),
        (
            [-3.0, -1.01, -1.9],
            [4.0, 14.0, -10.0],
            true,
            Anim::EquineLeg {
                hind: false,
                left: true,
            },
        ),
        (
            [-1.0, -1.01, -1.9],
            [-4.0, 14.0, -10.0],
            false,
            Anim::EquineLeg {
                hind: false,
                left: false,
            },
        ),
    ];
    for (min, pivot, mirror, anim) in legs {
        let p = b.part(pivot, anim, 1.0);
        b.cube_f(p, 0, (48.0, 21.0), min, [4.0, 11.0, 4.0], g, mirror, NONE);
    }
    // M359 — the tail is a part: it lifts with the walk speed. Its pivot
    // is the body's (0, 11, 5) plus its own (0, −5, 2).
    let tail = b.part_ext(
        [0.0, 6.0, 7.0],
        [PI / 6.0, 0.0, 0.0],
        Anim::EquineTail,
        1.0,
        Option::None,
    );
    b.cube_f(
        tail,
        0,
        (42.0, 36.0),
        [-1.5, 0.0, 0.0],
        [3.0, 14.0, 4.0],
        g,
        false,
        NONE,
    );
    // M296 — `DonkeyModel.modifyMesh`'s chests: `texOffs(26, 21)
    // .addBox(-4, 0, -2, 8, 8, 3)` on the body at `(6, -8, 0)` turned `-π/2`
    // (left) and `(-6, -8, 0)` turned `π/2` (right), shown with `hasChest`.
    if donkey_ears {
        let chest = b.part([0.0, 0.0, 0.0], Anim::None, 1.0);
        b.show(chest, Show::Chested);
        for (x, yaw) in [(6.0, -PI / 2.0), (-6.0, PI / 2.0)] {
            b.cube_f(
                chest,
                0,
                (26.0, 21.0),
                [-4.0, 0.0, -2.0],
                [8.0, 8.0, 3.0],
                0.0,
                false,
                &[Fold::rot([0.0, yaw, 0.0], [x, -8.0, 0.0]), body],
            );
        }
    }
    b.finish(1.0)
}

fn equine_plain() -> Model {
    equine(false)
}

// ---------------------------------------------------------------------------
// Boats (M223): `BoatModel` / `RaftModel`, placed by `AbstractBoatRenderer`
// ---------------------------------------------------------------------------

/// `AbstractBoatRenderer.submit` is `translate(0, 0.375, 0) · rotY(180 − yaw)
/// · scale(−1, −1, 1) · rotY(90°) · model`. The entity pass applies the yaw
/// and the flip with the living renderer's 1.501-block (24.016 px) offset, so
/// every boat cube carries this root fold: the `rotY(90°)` that comes after
/// the flip, and the offset that turns 24.016 px into the boat's 6 (0.375
/// block): `y_entity = 24.016 − (v.y + 18.016) = 6 − v.y`.
///
/// M393 — the fold is now two parts: the renderer's hurt wobble turns the
/// boat between the yaw and the flip, outside this `rotY(90°)`, so it needs
/// a part of its own around the root's. Returns the inner (root) part.
///
/// M394 — and a third between them, the bubble column's tilt (which the
/// renderer applies after the wobble, before the flip).
fn boat_root(b: &mut ModelBuilder) -> usize {
    let wobble = b.part([0.0, MODEL_EYE_Y - 6.0, 0.0], Anim::BoatHurt, 1.0);
    let bubble = b.part_ext([0.0; 3], [0.0; 3], Anim::BoatBubble, 1.0, Some(wobble));
    b.part_ext(
        [0.0; 3],
        [0.0, FRAC_PI_2, 0.0],
        Anim::None,
        1.0,
        Some(bubble),
    )
}

/// `AbstractBoatModel.animatePaddle` at a rowing time of 0, which is where a
/// boat nobody paddles keeps its paddles: `xRot = lerp(0.5, −π/3, −π/12)`,
/// `yRot = lerp((sin(1) + 1) / 2, −π/4, π/4)`, the right paddle's yaw
/// mirrored to `π − yRot`; the `PartPose`'s `zRot` of π/16 stays.
pub(crate) fn paddle_rest(right: bool) -> [f32; 3] {
    use std::f32::consts::PI;
    let lerp = |t: f32, a: f32, b: f32| a + t.clamp(0.0, 1.0) * (b - a);
    let x = lerp((0.0f32.sin() + 1.0) / 2.0, -PI / 3.0, -PI / 12.0);
    let y = lerp((1.0f32.sin() + 1.0) / 2.0, -PI / 4.0, PI / 4.0);
    [x, if right { PI - y } else { y }, PI / 16.0]
}

/// The two paddles (`BoatModel` / `RaftModel` share the boxes and differ in
/// the texture offsets and the pivot height). M393 — parts under the boat's
/// root (`Anim::BoatPaddle`: `AbstractBoatModel.animatePaddle` on the
/// rowing time), resting where a paddle nobody rows rests.
fn boat_paddles(
    b: &mut ModelBuilder,
    root: usize,
    uv_left: (f32, f32),
    uv_right: (f32, f32),
    pivot_y: f32,
) {
    for (right, uv, z, blade_x) in [(false, uv_left, 9.0, -1.001), (true, uv_right, -9.0, 0.001)] {
        let paddle = b.part_ext(
            [3.0, pivot_y, z],
            paddle_rest(right),
            Anim::BoatPaddle { right },
            1.0,
            Some(root),
        );
        b.cube_f(
            paddle,
            0,
            uv,
            [-1.0, 0.0, -5.0],
            [2.0, 2.0, 18.0],
            0.0,
            false,
            NONE,
        );
        b.cube_f(
            paddle,
            0,
            uv,
            [blade_x, -3.0, 8.0],
            [1.0, 6.0, 7.0],
            0.0,
            false,
            NONE,
        );
    }
}

/// `BoatModel.addCommonParts`: the bottom, the four sides, the paddles.
fn boat_hull(b: &mut ModelBuilder, root: usize) {
    use std::f32::consts::PI;
    let part = |b: &mut ModelBuilder,
                uv: (f32, f32),
                min: [f32; 3],
                dims: [f32; 3],
                rot: [f32; 3],
                off: [f32; 3]| {
        b.cube_f(root, 0, uv, min, dims, 0.0, false, &[Fold::rot(rot, off)]);
    };
    part(
        b,
        (0.0, 0.0),
        [-14.0, -9.0, -3.0],
        [28.0, 16.0, 3.0],
        [FRAC_PI_2, 0.0, 0.0],
        [0.0, 3.0, 1.0],
    );
    part(
        b,
        (0.0, 19.0),
        [-13.0, -7.0, -1.0],
        [18.0, 6.0, 2.0],
        [0.0, PI * 3.0 / 2.0, 0.0],
        [-15.0, 4.0, 4.0],
    );
    part(
        b,
        (0.0, 27.0),
        [-8.0, -7.0, -1.0],
        [16.0, 6.0, 2.0],
        [0.0, FRAC_PI_2, 0.0],
        [15.0, 4.0, 0.0],
    );
    part(
        b,
        (0.0, 35.0),
        [-14.0, -7.0, -1.0],
        [28.0, 6.0, 2.0],
        [0.0, PI, 0.0],
        [0.0, 4.0, -9.0],
    );
    part(
        b,
        (0.0, 43.0),
        [-14.0, -7.0, -1.0],
        [28.0, 6.0, 2.0],
        [0.0, 0.0, 0.0],
        [0.0, 4.0, 9.0],
    );
    boat_paddles(b, root, (62.0, 0.0), (62.0, 20.0), -5.0);
}

/// `RaftModel.addCommonParts`: two stacked bottom boxes, the paddles.
fn raft_hull(b: &mut ModelBuilder, root: usize) {
    let pose = Fold::rot([1.5708, 0.0, 0.0], [0.0, -2.1, 1.0]);
    b.cube_f(
        root,
        0,
        (0.0, 0.0),
        [-14.0, -11.0, -4.0],
        [28.0, 20.0, 4.0],
        0.0,
        false,
        &[pose],
    );
    b.cube_f(
        root,
        0,
        (0.0, 0.0),
        [-14.0, -9.0, -8.0],
        [28.0, 16.0, 4.0],
        0.0,
        false,
        &[pose],
    );
    boat_paddles(b, root, (0.0, 24.0), (40.0, 24.0), -4.0);
}

/// `createChestBoatModel` / `createChestRaftModel`'s chest: bottom, lid and
/// lock at the given heights (the raft's sit 5.1 px higher).
fn boat_chest(b: &mut ModelBuilder, root: usize, bottom_y: f32, lid_y: f32, lock_y: f32) {
    let rot = [0.0, -FRAC_PI_2, 0.0];
    for (uv, min, dims, off) in [
        (
            (0.0, 76.0),
            [0.0, 0.0, 0.0],
            [12.0, 8.0, 12.0],
            [-2.0, bottom_y, -6.0],
        ),
        (
            (0.0, 59.0),
            [0.0, 0.0, 0.0],
            [12.0, 4.0, 12.0],
            [-2.0, lid_y, -6.0],
        ),
        (
            (0.0, 59.0),
            [0.0, 0.0, 0.0],
            [2.0, 4.0, 1.0],
            [-1.0, lock_y, -1.0],
        ),
    ] {
        b.cube_f(root, 0, uv, min, dims, 0.0, false, &[Fold::rot(rot, off)]);
    }
}

fn boat() -> Model {
    let mut b = ModelBuilder::new();
    let root = boat_root(&mut b);
    boat_hull(&mut b, root);
    b.finish(1.0)
}

/// M363 — `BoatModel.createWaterPatch()`: one 28 x 16 x 3 box turned π/2
/// about x at (0, −3, 1), under the boat's root, which `BoatRenderer` draws
/// with `RenderTypes.waterMask()` (depth only) unless the boat is under
/// water: it fills the hull at the waterline in the depth buffer, so the
/// translucent water drawn after the entities is hidden inside the boat.
/// A raft has none (`RaftRenderer`).
pub fn boat_water_patch() -> Model {
    let mut b = ModelBuilder::new();
    // M393 — on the boat's root parts (the same indices: the patch's quads
    // are drawn through the boat's part list), so it wobbles with the hull.
    let root = boat_root(&mut b);
    b.cube_f(
        root,
        0,
        (0.0, 0.0),
        [-14.0, -9.0, -3.0],
        [28.0, 16.0, 3.0],
        0.0,
        false,
        &[Fold::rot([FRAC_PI_2, 0.0, 0.0], [0.0, -3.0, 1.0])],
    );
    b.finish(1.0)
}

// ---------------------------------------------------------------------------
// Minecarts (M252): `MinecartModel`, placed by `AbstractMinecartRenderer`
// ---------------------------------------------------------------------------

/// `AbstractMinecartRenderer.oldRender`'s fallback is `translate(0, 0.375, 0)
/// · rotY(180 − yRot) · rotZ(−xRot)` then `scale(−1, −1, 1) · model`. The
/// entity pass applies the yaw and the flip with the living renderer's
/// 24.016 px offset, so every cube carries the difference: 6 px instead,
/// `MODEL_EYE_Y − 6` (the boat's fold without its quarter turn). Recorded:
/// no `rotZ(−xRot)` (a cart on a slope is drawn level), no rail-derived
/// position or yaw (`posOnRail`), no hurt wobble and no contents block.
const MINECART_ROOT: Fold = Fold::rot([0.0; 3], [0.0, MODEL_EYE_Y - 6.0, 0.0]);

/// `MinecartModel.createBodyLayer`, 64×32: the bottom and four sides.
fn minecart() -> Model {
    use std::f32::consts::{FRAC_PI_2, PI};
    let mut b = ModelBuilder::new();
    let side = |b: &mut ModelBuilder,
                uv: (f32, f32),
                min: [f32; 3],
                dims: [f32; 3],
                rot: [f32; 3],
                off: [f32; 3]| {
        b.cube_f(
            STATIC_PART,
            0,
            uv,
            min,
            dims,
            0.0,
            false,
            &[Fold::rot(rot, off), MINECART_ROOT],
        );
    };
    side(
        &mut b,
        (0.0, 10.0),
        [-10.0, -8.0, -1.0],
        [20.0, 16.0, 2.0],
        [FRAC_PI_2, 0.0, 0.0],
        [0.0, 4.0, 0.0],
    );
    side(
        &mut b,
        (0.0, 0.0),
        [-8.0, -9.0, -1.0],
        [16.0, 8.0, 2.0],
        [0.0, 3.0 * FRAC_PI_2, 0.0],
        [-9.0, 4.0, 0.0],
    );
    side(
        &mut b,
        (0.0, 0.0),
        [-8.0, -9.0, -1.0],
        [16.0, 8.0, 2.0],
        [0.0, FRAC_PI_2, 0.0],
        [9.0, 4.0, 0.0],
    );
    side(
        &mut b,
        (0.0, 0.0),
        [-8.0, -9.0, -1.0],
        [16.0, 8.0, 2.0],
        [0.0, PI, 0.0],
        [0.0, 4.0, -7.0],
    );
    side(
        &mut b,
        (0.0, 0.0),
        [-8.0, -9.0, -1.0],
        [16.0, 8.0, 2.0],
        [0.0; 3],
        [0.0, 4.0, 7.0],
    );
    b.finish(1.0)
}

fn chest_boat() -> Model {
    let mut b = ModelBuilder::new();
    let root = boat_root(&mut b);
    boat_hull(&mut b, root);
    boat_chest(&mut b, root, -5.0, -9.0, -6.0);
    b.finish(1.0)
}

fn raft() -> Model {
    let mut b = ModelBuilder::new();
    let root = boat_root(&mut b);
    raft_hull(&mut b, root);
    b.finish(1.0)
}

fn chest_raft() -> Model {
    let mut b = ModelBuilder::new();
    let root = boat_root(&mut b);
    raft_hull(&mut b, root);
    boat_chest(&mut b, root, -10.1, -14.1, -11.1);
    b.finish(1.0)
}

// ---------------------------------------------------------------------------
// The armour stand (M238): `ArmorStandModel.createBodyLayer`
// ---------------------------------------------------------------------------

/// `ArmorStandModel.createBodyLayer` with `ArmorStand`'s default poses as the
/// parts' rotations (`DEFAULT_HEAD_POSE` .. `DEFAULT_RIGHT_LEG_POSE`, degrees
/// as `Rotations`). The named head, body, arms and legs are what the armour
/// layer hangs its pieces on. Recorded: the six synched poses
/// (`DATA_*_POSE`) are not read, so a posed stand shows the default pose,
/// and the hit wiggle is not modelled.
fn armor_stand() -> Model {
    let d = |x: f32, y: f32, z: f32| [x.to_radians(), y.to_radians(), z.to_radians()];
    let mut b = ModelBuilder::new();
    let head = b.part_named(
        "head",
        [0.0, 1.0, 0.0],
        d(0.0, 0.0, 0.0),
        Anim::StandPose(0),
        1.0,
        None,
    );
    b.cube(
        head,
        0,
        (0.0, 0.0),
        [-1.0, -7.0, -1.0],
        [2.0, 7.0, 2.0],
        NONE,
    );
    let body = b.part_named(
        "body",
        [0.0; 3],
        d(0.0, 0.0, 0.0),
        Anim::StandPose(1),
        1.0,
        None,
    );
    b.cube(
        body,
        0,
        (0.0, 26.0),
        [-6.0, 0.0, -1.5],
        [12.0, 3.0, 3.0],
        NONE,
    );
    let right_arm = b.part_named(
        "right_arm",
        [-5.0, 2.0, 0.0],
        d(-15.0, 0.0, 10.0),
        Anim::StandPose(3),
        1.0,
        None,
    );
    b.cube(
        right_arm,
        0,
        (24.0, 0.0),
        [-2.0, -2.0, -1.0],
        [2.0, 12.0, 2.0],
        NONE,
    );
    b.show(right_arm, Show::StandArms);
    let left_arm = b.part_named(
        "left_arm",
        [5.0, 2.0, 0.0],
        d(-10.0, 0.0, -10.0),
        Anim::StandPose(2),
        1.0,
        None,
    );
    b.cube_m(
        left_arm,
        0,
        (32.0, 16.0),
        [0.0, -2.0, -1.0],
        [2.0, 12.0, 2.0],
        NONE,
    );
    b.show(left_arm, Show::StandArms);
    let right_leg = b.part_named(
        "right_leg",
        [-1.9, 12.0, 0.0],
        d(1.0, 0.0, 1.0),
        Anim::StandPose(5),
        1.0,
        None,
    );
    b.cube(
        right_leg,
        0,
        (8.0, 0.0),
        [-1.0, 0.0, -1.0],
        [2.0, 11.0, 2.0],
        NONE,
    );
    let left_leg = b.part_named(
        "left_leg",
        [1.9, 12.0, 0.0],
        d(-1.0, 0.0, -1.0),
        Anim::StandPose(4),
        1.0,
        None,
    );
    b.cube_m(
        left_leg,
        0,
        (40.0, 16.0),
        [-1.0, 0.0, -1.0],
        [2.0, 11.0, 2.0],
        NONE,
    );
    // The three sticks take the BODY pose about the origin
    // (`ArmorStandModel.setupAnim` copies it to each), the body's own pivot,
    // so they ride the body part (M270).
    b.cube(
        body,
        0,
        (16.0, 0.0),
        [-3.0, 3.0, -1.0],
        [2.0, 7.0, 2.0],
        NONE,
    );
    b.cube(
        body,
        0,
        (48.0, 16.0),
        [1.0, 3.0, -1.0],
        [2.0, 7.0, 2.0],
        NONE,
    );
    b.cube(
        body,
        0,
        (0.0, 48.0),
        [-4.0, 10.0, -1.0],
        [8.0, 2.0, 2.0],
        NONE,
    );
    let plate = b.part_named(
        "base_plate",
        [0.0, 12.0, 0.0],
        [0.0; 3],
        Anim::CounterBodyYaw,
        1.0,
        None,
    );
    b.cube(
        plate,
        0,
        (0.0, 32.0),
        [-6.0, 11.0, -6.0],
        [12.0, 1.0, 12.0],
        NONE,
    );
    b.show(plate, Show::StandBasePlate);
    b.finish(1.0)
}

// ---------------------------------------------------------------------------
// Golems + allay
// ---------------------------------------------------------------------------

/// `SnowGolemModel`: three deflated snowballs + pumpkin head + stick arms;
/// M368 — the upper body and arms follow a quarter of the look.
fn snow_golem() -> Model {
    let mut b = ModelBuilder::new();
    let g = -0.5;
    let head = b.part([0.0, 4.0, 0.0], Anim::Head, 1.0);
    b.cube_g(
        head,
        0,
        (0.0, 0.0),
        [-4.0, -8.0, -4.0],
        [8.0, 8.0, 8.0],
        g,
        NONE,
    );
    // M368 — the arms are parts whose pivots `setupAnim` moves (the layer's
    // z ±1 is never drawn: at rest `z = ∓sin(0)·5 = 0`), the upper body a
    // part that turns.
    for (pivot, rot, left) in [
        ([5.0, 6.0, 0.0], [0.0, 0.0, 1.0], true),
        ([-5.0, 6.0, 0.0], [0.0, PI, -1.0], false),
    ] {
        let arm = b.part_ext(pivot, rot, Anim::SnowGolemArm { left }, 1.0, Option::None);
        b.cube_g(
            arm,
            0,
            (32.0, 0.0),
            [-1.0, 0.0, -1.0],
            [12.0, 2.0, 2.0],
            g,
            NONE,
        );
    }
    let upper = b.part([0.0, 13.0, 0.0], Anim::SnowGolemBody, 1.0);
    b.cube_g(
        upper,
        0,
        (0.0, 16.0),
        [-5.0, -10.0, -5.0],
        [10.0, 10.0, 10.0],
        g,
        NONE,
    );
    b.cube_f(
        STATIC_PART,
        0,
        (0.0, 36.0),
        [-6.0, -12.0, -6.0],
        [12.0, 12.0, 12.0],
        g,
        false,
        &[Fold::at([0.0, 24.0, 0.0])],
    );
    b.finish(1.0)
}

/// `IronGolemModel`: nosed head, massive body + skirt, 30-px arms, mirrored
/// legs.
fn iron_golem() -> Model {
    let mut b = ModelBuilder::new();
    let head = b.part([0.0, -7.0, -2.0], Anim::Head, 1.0);
    b.cube(
        head,
        0,
        (0.0, 0.0),
        [-4.0, -12.0, -5.5],
        [8.0, 10.0, 8.0],
        NONE,
    );
    b.cube(
        head,
        0,
        (24.0, 0.0),
        [-1.0, -5.0, -7.5],
        [2.0, 4.0, 2.0],
        NONE,
    );
    let body = Fold::at([0.0, -7.0, 0.0]);
    b.cube_f(
        STATIC_PART,
        0,
        (0.0, 40.0),
        [-9.0, -2.0, -6.0],
        [18.0, 12.0, 11.0],
        0.0,
        false,
        &[body],
    );
    b.cube_f(
        STATIC_PART,
        0,
        (0.0, 70.0),
        [-4.5, 10.0, -3.0],
        [9.0, 5.0, 6.0],
        0.5,
        false,
        &[body],
    );
    let arm_r = b.part([0.0, -7.0, 0.0], Anim::GolemArmRight, 1.0);
    b.cube(
        arm_r,
        0,
        (60.0, 21.0),
        [-13.0, -2.5, -3.0],
        [4.0, 30.0, 6.0],
        NONE,
    );
    let arm_l = b.part([0.0, -7.0, 0.0], Anim::GolemArmLeft, 1.0);
    b.cube(
        arm_l,
        0,
        (60.0, 58.0),
        [9.0, -2.5, -3.0],
        [4.0, 30.0, 6.0],
        NONE,
    );
    let leg_r = b.part([-4.0, 11.0, 0.0], Anim::GolemLegRight, 1.0);
    b.cube(
        leg_r,
        0,
        (37.0, 0.0),
        [-3.5, -3.0, -3.0],
        [6.0, 16.0, 5.0],
        NONE,
    );
    let leg_l = b.part([5.0, 11.0, 0.0], Anim::GolemLegLeft, 1.0);
    b.cube_m(
        leg_l,
        0,
        (60.0, 0.0),
        [-3.5, -3.0, -3.0],
        [6.0, 16.0, 5.0],
        NONE,
    );
    b.finish(1.0)
}

/// `AllayModel`: the vex's friendly cousin — same body plan, upright wings.
/// M369 — its root bobs while it hovers, its body pitches into flight and
/// its arms splay with an idle bob (`Anim::AllayBody` / `AllayArm`).
///
/// A real `root → {head, body → {arms, wings}}` hierarchy (vanilla's exact
/// `PartDefinition` tree) so the dance's whole-body root rotation
/// (`Anim::AllayRoot`) propagates to every part and the head's dance/look
/// (`Anim::AllayHead`) rotates about its own pivot on top. With no dance and a
/// neutral look this is byte-identical at rest to the old folded model — every
/// part's composed pivot is the same, so the facelabel gate is unchanged.
fn allay() -> Model {
    let mut b = ModelBuilder::new();
    // root (vanilla PartPose.offset(0, 23.5, 0)) — the dance pivot.
    let root = b.part_named(
        "root",
        [0.0, 23.5, 0.0],
        [0.0; 3],
        Anim::AllayRoot,
        1.0,
        None,
    );
    // head child (offset(0, -3.99, 0)) — dance tilt while dancing, look otherwise.
    let head = b.part_named(
        "head",
        [0.0, -3.99, 0.0],
        [0.0; 3],
        Anim::AllayHead,
        1.0,
        Some(root),
    );
    b.cube(
        head,
        0,
        (0.0, 0.0),
        [-2.5, -5.0, -2.5],
        [5.0, 5.0, 5.0],
        NONE,
    );
    // body child (offset(0, -4, 0)) — the arm/wing parent (vanilla's body).
    let body = b.part_named(
        "body",
        [0.0, -4.0, 0.0],
        [0.0; 3],
        Anim::AllayBody,
        1.0,
        Some(root),
    );
    b.cube(
        body,
        0,
        (0.0, 10.0),
        [-1.5, 0.0, -1.0],
        [3.0, 4.0, 2.0],
        NONE,
    );
    b.cube_g(
        body,
        0,
        (0.0, 16.0),
        [-1.5, 0.0, -1.0],
        [3.0, 5.0, 2.0],
        -0.2,
        NONE,
    );
    let arm_r = b.part_named(
        "right_arm",
        [-1.75, 0.5, 0.0],
        [0.0; 3],
        Anim::AllayArm { left: false },
        1.0,
        Some(body),
    );
    b.cube_g(
        arm_r,
        0,
        (23.0, 0.0),
        [-0.75, -0.5, -1.0],
        [1.0, 4.0, 2.0],
        -0.01,
        NONE,
    );
    let arm_l = b.part_named(
        "left_arm",
        [1.75, 0.5, 0.0],
        [0.0; 3],
        Anim::AllayArm { left: true },
        1.0,
        Some(body),
    );
    b.cube_g(
        arm_l,
        0,
        (23.0, 6.0),
        [-0.25, -0.5, -1.0],
        [1.0, 4.0, 2.0],
        -0.01,
        NONE,
    );
    // Wings hover-flap, children of body (vanilla offset(±0.5, 0, 0.6)).
    let wing_r = b.part_named(
        "right_wing",
        [-0.5, 0.0, 0.6],
        [0.0; 3],
        Anim::AllayWing { left: false },
        1.0,
        Some(body),
    );
    b.cube(
        wing_r,
        0,
        (16.0, 14.0),
        [0.0, 1.0, 0.0],
        [0.0, 5.0, 8.0],
        NONE,
    );
    let wing_l = b.part_named(
        "left_wing",
        [0.5, 0.0, 0.6],
        [0.0; 3],
        Anim::AllayWing { left: true },
        1.0,
        Some(body),
    );
    b.cube(
        wing_l,
        0,
        (16.0, 14.0),
        [0.0, 1.0, 0.0],
        [0.0, 5.0, 8.0],
        NONE,
    );
    b.finish(1.0)
}

// ---------------------------------------------------------------------------
// The last holdouts — bosses, deep-dark, 26.x newcomers
// ---------------------------------------------------------------------------

/// `WardenModel` (128²): huge ribbed body, tendril plates on the skull,
/// 28-px arms, stubby legs. M374 — on `animateWalk` and `animateIdlePose`
/// (`Anim::WardenBody` / `WardenHead` / `WardenArm` / `WardenLeg`).
fn warden() -> Model {
    let mut b = ModelBuilder::new();
    // bone(0,24,0) → body(0,−21,0), targeted by the gesture rigs.
    let body = b.part_named(
        "body",
        [0.0, 3.0, 0.0],
        [0.0; 3],
        Anim::WardenBody,
        1.0,
        Option::None,
    );
    b.cube(
        body,
        0,
        (0.0, 0.0),
        [-9.0, -13.0, -4.0],
        [18.0, 21.0, 11.0],
        NONE,
    );
    // Ribcage plates: promoted from static folded cubes on `body` to real
    // named child parts so `WARDEN_SONIC_BOOM` can swing them open (the rig
    // targets "right_ribcage"/"left_ribcage" by name). The part pivot equals
    // the former `Fold::at` offset, so with no rig active the neutral geometry
    // is byte-identical to the folded version (vanilla `PartPose.offset`).
    let ribcage_r = b.part_named(
        "right_ribcage",
        [-7.0, -2.0, -4.0],
        [0.0; 3],
        Anim::None,
        1.0,
        Some(body),
    );
    b.cube(
        ribcage_r,
        0,
        (90.0, 11.0),
        [-2.0, -11.0, -0.1],
        [9.0, 21.0, 0.0],
        NONE,
    );
    let ribcage_l = b.part_named(
        "left_ribcage",
        [7.0, -2.0, -4.0],
        [0.0; 3],
        Anim::None,
        1.0,
        Some(body),
    );
    b.cube_f(
        ribcage_l,
        0,
        (90.0, 11.0),
        [-7.0, -11.0, -0.1],
        [9.0, 21.0, 0.0],
        0.0,
        true,
        NONE,
    );
    let head = b.part_named(
        "head",
        [0.0, -13.0, 0.0],
        [0.0; 3],
        Anim::WardenHead,
        1.0,
        Some(body),
    );
    b.cube(
        head,
        0,
        (0.0, 32.0),
        [-8.0, -16.0, -5.0],
        [16.0, 16.0, 10.0],
        NONE,
    );
    // The tendrils are real head children, not folded plates: they carry their
    // own `animateTendrils` rotation and are the parts the tendril emissive
    // layer retains (M57). Pivot = vanilla's `PartPose.offset`, which is exactly
    // the static fold this replaced — rest geometry is unchanged, so the
    // facelabel gate is untouched. (The same promotion M17 did for the
    // ribcages, for the same reason.)
    let tendril_r = b.part_named(
        "right_tendril",
        [-8.0, -12.0, 0.0],
        [0.0; 3],
        Anim::WardenTendril { left: false },
        1.0,
        Some(head),
    );
    b.cube(
        tendril_r,
        0,
        (52.0, 32.0),
        [-16.0, -13.0, 0.0],
        [16.0, 16.0, 0.0],
        NONE,
    );
    let tendril_l = b.part_named(
        "left_tendril",
        [8.0, -12.0, 0.0],
        [0.0; 3],
        Anim::WardenTendril { left: true },
        1.0,
        Some(head),
    );
    b.cube(
        tendril_l,
        0,
        (58.0, 0.0),
        [0.0, -13.0, 0.0],
        [16.0, 16.0, 0.0],
        NONE,
    );
    let arm_r = b.part_named(
        "right_arm",
        [-13.0, -13.0, 1.0],
        [0.0; 3],
        Anim::WardenArm { left: false },
        1.0,
        Some(body),
    );
    b.cube(
        arm_r,
        0,
        (44.0, 50.0),
        [-4.0, 0.0, -4.0],
        [8.0, 28.0, 8.0],
        NONE,
    );
    let arm_l = b.part_named(
        "left_arm",
        [13.0, -13.0, 1.0],
        [0.0; 3],
        Anim::WardenArm { left: true },
        1.0,
        Some(body),
    );
    b.cube(
        arm_l,
        0,
        (0.0, 58.0),
        [-4.0, 0.0, -4.0],
        [8.0, 28.0, 8.0],
        NONE,
    );
    let leg_r = b.part_named(
        "right_leg",
        [-5.9, 11.0, 0.0],
        [0.0; 3],
        Anim::WardenLeg { left: false },
        1.0,
        Option::None,
    );
    b.cube(
        leg_r,
        0,
        (76.0, 48.0),
        [-3.1, 0.0, -3.0],
        [6.0, 13.0, 6.0],
        NONE,
    );
    let leg_l = b.part_named(
        "left_leg",
        [5.9, 11.0, 0.0],
        [0.0; 3],
        Anim::WardenLeg { left: true },
        1.0,
        Option::None,
    );
    b.cube(
        leg_l,
        0,
        (76.0, 76.0),
        [-2.9, 0.0, -3.0],
        [6.0, 13.0, 6.0],
        NONE,
    );
    b.keyframe_gesture(&crate::anim_defs::WARDEN_ROAR, Gesture::WardenRoar);
    b.keyframe_gesture(&crate::anim_defs::WARDEN_SNIFF, Gesture::WardenSniff);
    b.keyframe_gesture(&crate::anim_defs::WARDEN_EMERGE, Gesture::WardenEmerge);
    b.keyframe_gesture(&crate::anim_defs::WARDEN_DIG, Gesture::WardenDig);
    // Wire-event one-shots (ClientboundEntityEventPacket): id 4 → attack,
    // id 62 → sonic boom (swings the ribcage plates open).
    b.keyframe_event(&crate::anim_defs::WARDEN_ATTACK, ModelEvent::WardenAttack);
    b.keyframe_event(
        &crate::anim_defs::WARDEN_SONIC_BOOM,
        ModelEvent::WardenSonicBoom,
    );
    b.finish(1.0)
}

/// `SnifferModel` (192²): moss-backed barrel body on six legs, droopy head
/// — walking on the vanilla SNIFFER_WALK keyframe rig (body sway, ear
/// flops, six-leg gait).
fn sniffer() -> Model {
    let mut b = ModelBuilder::new();
    // bone(0,5,0) → body(0,0,0).
    let body = b.part_named(
        "body",
        [0.0, 5.0, 0.0],
        [0.0; 3],
        Anim::None,
        1.0,
        Option::None,
    );
    b.cube(
        body,
        0,
        (62.0, 68.0),
        [-12.5, -14.0, -20.0],
        [25.0, 29.0, 40.0],
        NONE,
    );
    b.cube_g(
        body,
        0,
        (62.0, 0.0),
        [-12.5, -14.0, -20.0],
        [25.0, 24.0, 40.0],
        0.5,
        NONE,
    );
    b.cube(
        body,
        0,
        (87.0, 68.0),
        [-12.5, 12.0, -20.0],
        [25.0, 0.0, 40.0],
        NONE,
    );
    let legs = [
        ("right_front_leg", (32.0, 87.0), [-7.5, 15.0, -15.0]),
        ("right_mid_leg", (32.0, 105.0), [-7.5, 15.0, 0.0]),
        ("right_hind_leg", (32.0, 123.0), [-7.5, 15.0, 15.0]),
        ("left_front_leg", (0.0, 87.0), [7.5, 15.0, -15.0]),
        ("left_mid_leg", (0.0, 105.0), [7.5, 15.0, 0.0]),
        ("left_hind_leg", (0.0, 123.0), [7.5, 15.0, 15.0]),
    ];
    for (name, uv, pivot) in legs {
        let p = b.part_named(name, pivot, [0.0; 3], Anim::None, 1.0, Option::None);
        b.cube(p, 0, uv, [-3.5, -1.0, -4.0], [7.0, 10.0, 8.0], NONE);
    }
    // Head: body child at (0, 6.5, −19.48) — local pivot under `body`.
    let head = b.part_named(
        "head",
        [0.0, 6.5, -19.48],
        [0.0; 3],
        Anim::Head,
        1.0,
        Some(body),
    );
    b.cube(
        head,
        0,
        (8.0, 15.0),
        [-6.5, -7.5, -11.5],
        [13.0, 18.0, 11.0],
        NONE,
    );
    b.cube(
        head,
        0,
        (8.0, 4.0),
        [-6.5, 7.5, -11.5],
        [13.0, 0.0, 11.0],
        NONE,
    );
    let ear_l = b.part_named(
        "left_ear",
        [6.51, -7.5, -4.51],
        [0.0; 3],
        Anim::None,
        1.0,
        Some(head),
    );
    b.cube(
        ear_l,
        0,
        (2.0, 0.0),
        [0.0, 0.0, -3.0],
        [1.0, 19.0, 7.0],
        NONE,
    );
    let ear_r = b.part_named(
        "right_ear",
        [-6.51, -7.5, -4.51],
        [0.0; 3],
        Anim::None,
        1.0,
        Some(head),
    );
    b.cube(
        ear_r,
        0,
        (48.0, 0.0),
        [-1.0, 0.0, -3.0],
        [1.0, 19.0, 7.0],
        NONE,
    );
    b.cube_f(
        head,
        0,
        (10.0, 45.0),
        [-6.5, -2.0, -9.0],
        [13.0, 2.0, 9.0],
        0.0,
        false,
        &[Fold::at([0.0, -4.5, -11.5])],
    );
    // Vanilla swaps walks: `if (isSearching) sniffSearch.applyWalk(…) else
    // walk.applyWalk(…)` — both walk-driven, gated on the SEARCHING state.
    let walk = KfDriver::Walk {
        speed_f: 9.0,
        scale_f: 100.0,
    };
    b.keyframe_gated(
        &crate::anim_defs::SNIFFER_WALK,
        walk,
        KfGate::Unless(Gesture::SnifferSearching),
    );
    b.keyframe_gated(
        &crate::anim_defs::SNIFFER_SNIFF_SEARCH,
        walk,
        KfGate::During(Gesture::SnifferSearching),
    );
    b.keyframe_gesture(&crate::anim_defs::SNIFFER_DIG, Gesture::SnifferDigging);
    b.keyframe_gesture(
        &crate::anim_defs::SNIFFER_LONGSNIFF,
        Gesture::SnifferSniffing,
    );
    b.keyframe_gesture(
        &crate::anim_defs::SNIFFER_HAPPY,
        Gesture::SnifferFeelingHappy,
    );
    b.keyframe_gesture(&crate::anim_defs::SNIFFER_STAND_UP, Gesture::SnifferRising);
    // SCENTING's SNIFFSNIFF rig is scale-channel-only (dropped) — no rig.
    b.finish(1.0)
}

/// `BreezeModel`: head + three orbiting rods on breeze.png, the swirling
/// wind funnel on breeze_wind.png (texture 1).
fn breeze() -> Model {
    let mut b = ModelBuilder::new();
    let body = b.part_named(
        "body",
        [0.0, 0.0, 0.0],
        [0.0; 3],
        Anim::None,
        1.0,
        Option::None,
    );
    let rods = b.part_named(
        "rods",
        [0.0, 8.0, 0.0],
        [0.0; 3],
        Anim::None,
        1.0,
        Some(body),
    );
    let rod_poses: [([f32; 3], [f32; 3]); 3] = [
        ([2.5981, -3.0, 1.5], [-2.7489, -1.0472, 3.1416]),
        ([-2.5981, -3.0, 1.5], [-2.7489, 1.0472, 3.1416]),
        ([0.0, -3.0, -3.0], [0.3927, 0.0, 0.0]),
    ];
    for (off, rot) in rod_poses {
        b.cube_f(
            rods,
            0,
            (0.0, 17.0),
            [-1.0, 0.0, -3.0],
            [2.0, 8.0, 2.0],
            0.0,
            false,
            &[Fold::rot(rot, off)],
        );
    }
    let head = b.part_named(
        "head",
        [0.0, 4.0, 0.0],
        [0.0; 3],
        Anim::None,
        1.0,
        Some(body),
    );
    b.cube(
        head,
        0,
        (4.0, 24.0),
        [-5.0, -5.0, -4.2],
        [10.0, 3.0, 4.0],
        NONE,
    );
    b.cube(
        head,
        0,
        (0.0, 0.0),
        [-4.0, -8.0, -4.0],
        [8.0, 8.0, 8.0],
        NONE,
    );
    // Wind funnel (three stacked swirl tiers, widest at the top) — the
    // idle rig spins wind_mid/wind_top and bobs the head/rods; the shoot/
    // slide/inhale gestures whip the whole funnel.
    let wb = b.part_named(
        "wind_bottom",
        [0.0, 24.0, 0.0],
        [0.0; 3],
        Anim::None,
        1.0,
        Option::None,
    );
    b.cube(
        wb,
        1,
        (1.0, 83.0),
        [-2.5, -7.0, -2.5],
        [5.0, 7.0, 5.0],
        NONE,
    );
    let wm = b.part_named(
        "wind_mid",
        [0.0, -7.0, 0.0],
        [0.0; 3],
        Anim::None,
        1.0,
        Some(wb),
    );
    b.cube(
        wm,
        1,
        (74.0, 28.0),
        [-6.0, -6.0, -6.0],
        [12.0, 6.0, 12.0],
        NONE,
    );
    b.cube(
        wm,
        1,
        (78.0, 32.0),
        [-4.0, -6.0, -4.0],
        [8.0, 6.0, 8.0],
        NONE,
    );
    b.cube(
        wm,
        1,
        (49.0, 71.0),
        [-2.5, -6.0, -2.5],
        [5.0, 6.0, 5.0],
        NONE,
    );
    let wt = b.part_named(
        "wind_top",
        [0.0, -6.0, 0.0],
        [0.0; 3],
        Anim::None,
        1.0,
        Some(wm),
    );
    b.cube(
        wt,
        1,
        (0.0, 0.0),
        [-9.0, -8.0, -9.0],
        [18.0, 8.0, 18.0],
        NONE,
    );
    b.cube(
        wt,
        1,
        (6.0, 6.0),
        [-6.0, -8.0, -6.0],
        [12.0, 8.0, 12.0],
        NONE,
    );
    b.cube(
        wt,
        1,
        (105.0, 57.0),
        [-2.5, -8.0, -2.5],
        [5.0, 8.0, 5.0],
        NONE,
    );
    b.keyframe(&crate::anim_defs::BREEZE_IDLE, KfDriver::Age);
    b.keyframe_gesture(&crate::anim_defs::BREEZE_SHOOT, Gesture::BreezeShoot);
    b.keyframe_gesture(&crate::anim_defs::BREEZE_SLIDE, Gesture::BreezeSlide);
    b.keyframe_gesture(&crate::anim_defs::BREEZE_INHALE, Gesture::BreezeInhale);
    b.keyframe_gesture(&crate::anim_defs::BREEZE_JUMP, Gesture::BreezeJump);
    b.finish(1.0)
}

/// `CreakingModel`: gnarled tree humanoid — twig-crowned head, split-log
/// body, branch arms, root legs with 0-thick foot fans.
fn creaking() -> Model {
    let mut b = ModelBuilder::new();
    // root(0,24,0) → upper_body(−1,−19,0), driven by CREAKING_WALK.
    let upper = b.part_named(
        "upper_body",
        [-1.0, 5.0, 0.0],
        [0.0; 3],
        Anim::None,
        1.0,
        Option::None,
    );
    let head = b.part_named(
        "head",
        [-3.0, -11.0, 0.0],
        [0.0; 3],
        Anim::Head,
        1.0,
        Some(upper),
    );
    b.cube(
        head,
        0,
        (0.0, 0.0),
        [-3.0, -10.0, -3.0],
        [6.0, 10.0, 6.0],
        NONE,
    );
    b.cube(
        head,
        0,
        (28.0, 31.0),
        [-3.0, -13.0, -3.0],
        [6.0, 3.0, 6.0],
        NONE,
    );
    b.cube(
        head,
        0,
        (12.0, 40.0),
        [3.0, -13.0, 0.0],
        [9.0, 14.0, 0.0],
        NONE,
    );
    b.cube(
        head,
        0,
        (34.0, 12.0),
        [-12.0, -14.0, 0.0],
        [9.0, 14.0, 0.0],
        NONE,
    );
    let body = Fold::at([0.0, -7.0, 1.0]);
    b.cube_f(
        upper,
        0,
        (0.0, 16.0),
        [0.0, -3.0, -3.0],
        [6.0, 13.0, 5.0],
        0.0,
        false,
        &[body],
    );
    b.cube_f(
        upper,
        0,
        (24.0, 0.0),
        [-6.0, -4.0, -3.0],
        [6.0, 7.0, 5.0],
        0.0,
        false,
        &[body],
    );
    let arm_r = b.part_named(
        "right_arm",
        [-7.0, -9.5, 1.5],
        [0.0; 3],
        Anim::None,
        1.0,
        Some(upper),
    );
    b.cube(
        arm_r,
        0,
        (22.0, 13.0),
        [-2.0, -1.5, -1.5],
        [3.0, 21.0, 3.0],
        NONE,
    );
    b.cube(
        arm_r,
        0,
        (46.0, 0.0),
        [-2.0, 19.5, -1.5],
        [3.0, 4.0, 3.0],
        NONE,
    );
    let arm_l = b.part_named(
        "left_arm",
        [6.0, -9.0, 0.5],
        [0.0; 3],
        Anim::None,
        1.0,
        Some(upper),
    );
    b.cube(
        arm_l,
        0,
        (30.0, 40.0),
        [0.0, -1.0, -1.5],
        [3.0, 16.0, 3.0],
        NONE,
    );
    b.cube(
        arm_l,
        0,
        (52.0, 12.0),
        [0.0, -5.0, -1.5],
        [3.0, 4.0, 3.0],
        NONE,
    );
    b.cube(
        arm_l,
        0,
        (52.0, 19.0),
        [0.0, 15.0, -1.5],
        [3.0, 4.0, 3.0],
        NONE,
    );
    let leg_l = b.part_named(
        "left_leg",
        [1.5, 8.0, 0.5],
        [0.0; 3],
        Anim::None,
        1.0,
        Option::None,
    );
    b.cube(
        leg_l,
        0,
        (42.0, 40.0),
        [-1.5, 0.0, -1.5],
        [3.0, 16.0, 3.0],
        NONE,
    );
    b.cube(
        leg_l,
        0,
        (45.0, 55.0),
        [-1.5, 15.7, -4.5],
        [5.0, 0.0, 9.0],
        NONE,
    );
    let leg_r = b.part_named(
        "right_leg",
        [-1.0, 6.5, 0.5],
        [0.0; 3],
        Anim::None,
        1.0,
        Option::None,
    );
    b.cube(
        leg_r,
        0,
        (0.0, 34.0),
        [-3.0, -1.5, -1.5],
        [3.0, 19.0, 3.0],
        NONE,
    );
    b.cube(
        leg_r,
        0,
        (45.0, 46.0),
        [-5.0, 17.2, -4.5],
        [5.0, 0.0, 9.0],
        NONE,
    );
    b.cube(
        leg_r,
        0,
        (12.0, 34.0),
        [-3.0, -4.5, -1.5],
        [3.0, 3.0, 3.0],
        NONE,
    );
    b.keyframe(
        &crate::anim_defs::CREAKING_WALK,
        KfDriver::Walk {
            speed_f: 1.0,
            scale_f: 1.0,
        },
    );
    b.finish(1.0)
}

/// M375 — `RavagerModel.setupAnim`'s `legRot = 0.4·speed`, as the
/// quadruped legs' amplitude multiplier (they swing at 1.4·speed·amp).
pub const RAVAGER_LEG_AMP: f32 = 0.4 / 1.4;

/// `RavagerModel` (128²): armored bull — horned head on a thick neck,
/// rotated slab body, 37-px legs; M375 — the mouth rests a sliver open and
/// the legs swing at vanilla's 0.4.
fn ravager() -> Model {
    let mut b = ModelBuilder::new();
    let neck = Fold::at([0.0, -7.0, 5.5]);
    b.cube_f(
        STATIC_PART,
        0,
        (68.0, 73.0),
        [-5.0, -1.0, -18.0],
        [10.0, 10.0, 18.0],
        0.0,
        false,
        &[neck],
    );
    // Head: neck child at (0, 16, −17) ⇒ absolute pivot (0, 9, −11.5).
    let head = b.part([0.0, 9.0, -11.5], Anim::Head, 1.0);
    b.cube(
        head,
        0,
        (0.0, 0.0),
        [-8.0, -20.0, -14.0],
        [16.0, 20.0, 16.0],
        NONE,
    );
    b.cube(
        head,
        0,
        (0.0, 0.0),
        [-2.0, -6.0, -18.0],
        [4.0, 8.0, 4.0],
        NONE,
    );
    b.cube_f(
        head,
        0,
        (74.0, 55.0),
        [0.0, -14.0, -2.0],
        [2.0, 14.0, 4.0],
        0.0,
        false,
        &[Fold::rot([1.0995574, 0.0, 0.0], [-10.0, -14.0, -8.0])],
    );
    b.cube_f(
        head,
        0,
        (74.0, 55.0),
        [0.0, -14.0, -2.0],
        [2.0, 14.0, 4.0],
        0.0,
        true,
        &[Fold::rot([1.0995574, 0.0, 0.0], [8.0, -14.0, -8.0])],
    );
    // M375 — the mouth is a part resting where `setupAnim` puts it when the
    // ravager is neither attacking nor stunned: `xRot = π·0.01`, a sliver
    // open (Rewo drew it shut).
    let mouth = b.part_named(
        "mouth",
        [0.0, -2.0, 2.0],
        [PI * 0.01, 0.0, 0.0],
        Anim::None,
        1.0,
        Some(head),
    );
    b.cube(
        mouth,
        0,
        (0.0, 36.0),
        [-8.0, 0.0, -16.0],
        [16.0, 3.0, 16.0],
        NONE,
    );
    let body = Fold::rot([FRAC_PI_2, 0.0, 0.0], [0.0, 1.0, 2.0]);
    b.cube_f(
        STATIC_PART,
        0,
        (0.0, 55.0),
        [-7.0, -10.0, -7.0],
        [14.0, 16.0, 20.0],
        0.0,
        false,
        &[body],
    );
    b.cube_f(
        STATIC_PART,
        0,
        (0.0, 91.0),
        [-6.0, 6.0, -7.0],
        [12.0, 13.0, 18.0],
        0.0,
        false,
        &[body],
    );
    let legs = [
        ((96.0, 0.0), [-8.0, -13.0, 18.0], false, Anim::QuadHindRight),
        ((96.0, 0.0), [8.0, -13.0, 18.0], true, Anim::QuadHindLeft),
        (
            (64.0, 0.0),
            [-8.0, -13.0, -5.0],
            false,
            Anim::QuadFrontRight,
        ),
        ((64.0, 0.0), [8.0, -13.0, -5.0], true, Anim::QuadFrontLeft),
    ];
    for (uv, pivot, mirror, anim) in legs {
        // M375 — `legRot = 0.4·speed`: the quadruped swing's 1.4 scaled to
        // 0.4 (Rewo swung them 3.5 times as far).
        let p = b.part(pivot, anim, RAVAGER_LEG_AMP);
        b.cube_f(
            p,
            0,
            uv,
            [-4.0, 0.0, -4.0],
            [8.0, 37.0, 8.0],
            0.0,
            mirror,
            NONE,
        );
    }
    b.finish(1.0)
}

/// M376 — `WitherBossModel`'s ribcage rest, `0.065·π` (the breathing's
/// mean).
pub const WITHER_RIBCAGE_MEAN: f32 = 0.20420352;

/// `WitherBossModel`: three skulls over a floating spine + ribs + tail.
fn wither() -> Model {
    wither_with(0.0)
}

/// M287 — `ModelLayers.WITHER_ARMOR`: `WitherBossModel.createBodyLayer(
/// INNER_ARMOR_DEFORMATION)`, `CubeDeformation(0.5F)`: the powered wither's
/// swirl shell, the same parts in the same order as [`wither`]. Its texture
/// is `wither_armor.png` ([`WITHER_ARMOR_TEXTURE`]).
pub fn wither_armor() -> Model {
    wither_with(0.5)
}

/// `LayerDefinition.create(mesh, 64, 64)` for the wither layers.
pub const WITHER_ARMOR_TEXTURE: [f32; 2] = [64.0, 64.0];

/// `WitherBossModel.createBodyLayer(g)`.
fn wither_with(g: f32) -> Model {
    let mut b = ModelBuilder::new();
    b.cube_f(
        STATIC_PART,
        0,
        (0.0, 16.0),
        [-10.0, 3.9, -0.5],
        [20.0, 3.0, 3.0],
        g,
        false,
        NONE,
    );
    // M376 — the ribcage (spine and ribs) and the tail are parts that
    // breathe: `setupAnim` sets `ribcage.xRot = (0.065 + 0.05·cos(age·0.1))·π`,
    // slides the tail along the ribcage's 10 px arc and sets its `xRot` to
    // `(0.265 + 0.1·cos)·π`. They rest at the means (0.065π, 0.265π).
    let ribcage = b.part_ext(
        [-2.0, 6.9, -0.5],
        [WITHER_RIBCAGE_MEAN, 0.0, 0.0],
        Anim::WitherRibcage,
        1.0,
        Option::None,
    );
    for (uv, min, dims) in [
        ((0.0, 22.0), [0.0, 0.0, 0.0], [3.0, 10.0, 3.0]),
        ((24.0, 22.0), [-4.0, 1.5, 0.5], [11.0, 2.0, 2.0]),
        ((24.0, 22.0), [-4.0, 4.0, 0.5], [11.0, 2.0, 2.0]),
        ((24.0, 22.0), [-4.0, 6.5, 0.5], [11.0, 2.0, 2.0]),
    ] {
        b.cube_f(ribcage, 0, uv, min, dims, g, false, NONE);
    }
    let a = WITHER_RIBCAGE_MEAN;
    let tail = b.part_ext(
        [-2.0, 6.9 + a.cos() * 10.0, -0.5 + a.sin() * 10.0],
        [0.83252203, 0.0, 0.0],
        Anim::WitherTail,
        1.0,
        Option::None,
    );
    b.cube_f(
        tail,
        0,
        (12.0, 22.0),
        [0.0, 0.0, 0.0],
        [3.0, 6.0, 3.0],
        g,
        false,
        NONE,
    );
    let head = b.part([0.0, 0.0, 0.0], Anim::Head, 1.0);
    b.cube_f(
        head,
        0,
        (0.0, 0.0),
        [-4.0, -4.0, -4.0],
        [8.0, 8.0, 8.0],
        g,
        false,
        NONE,
    );
    // M376 — the side heads aim at their own targets in vanilla (client
    // rotations eased in `WitherBoss.aiStep`), and without one ease back to
    // the body's facing at pitch 0, which is where they rest here: Rewo keeps
    // no per-head target state. Until M376 they copied the centre head.
    let head_r = b.part([-8.0, 4.0, 0.0], Anim::None, 1.0);
    b.cube_f(
        head_r,
        0,
        (32.0, 0.0),
        [-4.0, -4.0, -4.0],
        [6.0, 6.0, 6.0],
        g,
        false,
        NONE,
    );
    let head_l = b.part([10.0, 4.0, 0.0], Anim::None, 1.0);
    b.cube_f(
        head_l,
        0,
        (32.0, 0.0),
        [-4.0, -4.0, -4.0],
        [6.0, 6.0, 6.0],
        g,
        false,
        NONE,
    );
    b.finish(1.0)
}

/// `EnderDragonModel` (256²): the full dragon — head + jaw, 5 neck and 12
/// tail spine segments, 24×24×64 body, two-segment wings with 0-thick
/// membrane skins, three-segment legs. Static pose (vanilla's animation is
/// procedural flight).
fn ender_dragon() -> Model {
    let mut b = ModelBuilder::new();
    // M388 — vanilla's tree: the model root (which `setupAnim` lifts and
    // moves forward every frame), the head and its jaw, five neck and twelve
    // tail segments placed along their chains, and the body carrying the
    // wings and legs; every part on `Anim::Dragon { part }`, a slot of
    // [`dragon_pose`].
    let dr = |part: u8| Anim::Dragon { part };
    let root = b.part_named(
        "root",
        [0.0; 3],
        [0.0; 3],
        dr(DRAGON_ROOT),
        1.0,
        Option::None,
    );
    let head = b.part_named(
        "head",
        [0.0, 20.0, -62.0],
        [0.0; 3],
        dr(DRAGON_HEAD),
        1.0,
        Some(root),
    );
    b.cube(
        head,
        0,
        (176.0, 44.0),
        [-6.0, -1.0, -24.0],
        [12.0, 5.0, 16.0],
        NONE,
    );
    b.cube(
        head,
        0,
        (112.0, 30.0),
        [-8.0, -8.0, -10.0],
        [16.0, 16.0, 16.0],
        NONE,
    );
    b.cube_m(
        head,
        0,
        (0.0, 0.0),
        [-5.0, -12.0, -4.0],
        [2.0, 4.0, 6.0],
        NONE,
    );
    b.cube_m(
        head,
        0,
        (112.0, 0.0),
        [-5.0, -3.0, -22.0],
        [2.0, 2.0, 4.0],
        NONE,
    );
    b.cube_m(
        head,
        0,
        (0.0, 0.0),
        [3.0, -12.0, -4.0],
        [2.0, 4.0, 6.0],
        NONE,
    );
    b.cube_m(
        head,
        0,
        (112.0, 0.0),
        [3.0, -3.0, -22.0],
        [2.0, 2.0, 4.0],
        NONE,
    );
    let jaw = b.part_named(
        "jaw",
        [0.0, 4.0, -8.0],
        [0.0; 3],
        dr(DRAGON_JAW),
        1.0,
        Some(head),
    );
    b.cube(
        jaw,
        0,
        (176.0, 65.0),
        [-6.0, 0.0, -16.0],
        [12.0, 4.0, 16.0],
        NONE,
    );
    let spine = |b: &mut ModelBuilder, pivot: [f32; 3], part: u8| {
        let seg = b.part_named("", pivot, [0.0; 3], dr(part), 1.0, Some(root));
        b.cube(
            seg,
            0,
            (192.0, 104.0),
            [-5.0, -5.0, -5.0],
            [10.0, 10.0, 10.0],
            NONE,
        );
        b.cube(
            seg,
            0,
            (48.0, 0.0),
            [-1.0, -9.0, -3.0],
            [2.0, 4.0, 6.0],
            NONE,
        );
    };
    for i in 0..5u8 {
        spine(
            &mut b,
            [0.0, 20.0, -12.0 - f32::from(i) * 10.0],
            DRAGON_NECK + i,
        );
    }
    for i in 0..12u8 {
        spine(
            &mut b,
            [0.0, 10.0, 60.0 + f32::from(i) * 10.0],
            DRAGON_TAIL + i,
        );
    }
    let body = b.part_named(
        "body",
        [0.0, 3.0, 8.0],
        [0.0; 3],
        dr(DRAGON_BODY),
        1.0,
        Some(root),
    );
    b.cube(
        body,
        0,
        (0.0, 0.0),
        [-12.0, 1.0, -16.0],
        [24.0, 24.0, 64.0],
        NONE,
    );
    for z in [-10.0, 10.0, 30.0] {
        b.cube(
            body,
            0,
            (220.0, 53.0),
            [-1.0, -5.0, z],
            [2.0, 6.0, 12.0],
            NONE,
        );
    }
    // Wings: bone + membrane, tip child; the left side mirrors its boxes.
    for (sx, wing_part, tip_part) in [
        (1.0f32, DRAGON_LWING, DRAGON_LWING + 1),
        (-1.0, DRAGON_RWING, DRAGON_RWING + 1),
    ] {
        let left = sx > 0.0;
        let x0 = if left { 0.0 } else { -56.0 };
        let wing = b.part_named(
            "",
            [12.0 * sx, 2.0, -6.0],
            [0.0; 3],
            dr(wing_part),
            1.0,
            Some(body),
        );
        b.cube_f(
            wing,
            0,
            (112.0, 88.0),
            [x0, -4.0, -4.0],
            [56.0, 8.0, 8.0],
            0.0,
            left,
            NONE,
        );
        b.cube_f(
            wing,
            0,
            (-56.0, 88.0),
            [x0, 0.0, 2.0],
            [56.0, 0.0, 56.0],
            0.0,
            left,
            NONE,
        );
        let tip = b.part_named(
            "",
            [56.0 * sx, 0.0, 0.0],
            [0.0; 3],
            dr(tip_part),
            1.0,
            Some(wing),
        );
        b.cube_f(
            tip,
            0,
            (112.0, 136.0),
            [x0, -2.0, -2.0],
            [56.0, 4.0, 4.0],
            0.0,
            left,
            NONE,
        );
        b.cube_f(
            tip,
            0,
            (-56.0, 144.0),
            [x0, 0.0, 2.0],
            [56.0, 0.0, 56.0],
            0.0,
            left,
            NONE,
        );
    }
    // Legs: (upper, tip, foot) chains per side, left then right.
    for (sx, first) in [(1.0f32, DRAGON_LLEGS), (-1.0, DRAGON_RLEGS)] {
        let front = b.part_named(
            "",
            [12.0 * sx, 17.0, -6.0],
            [1.3, 0.0, 0.0],
            dr(first),
            1.0,
            Some(body),
        );
        b.cube(
            front,
            0,
            (112.0, 104.0),
            [-4.0, -4.0, -4.0],
            [8.0, 24.0, 8.0],
            NONE,
        );
        let ftip = b.part_named(
            "",
            [0.0, 20.0, -1.0],
            [-0.5, 0.0, 0.0],
            dr(first + 1),
            1.0,
            Some(front),
        );
        b.cube(
            ftip,
            0,
            (226.0, 138.0),
            [-3.0, -1.0, -3.0],
            [6.0, 24.0, 6.0],
            NONE,
        );
        let ffoot = b.part_named(
            "",
            [0.0, 23.0, 0.0],
            [0.75, 0.0, 0.0],
            dr(first + 2),
            1.0,
            Some(ftip),
        );
        b.cube(
            ffoot,
            0,
            (144.0, 104.0),
            [-4.0, 0.0, -12.0],
            [8.0, 4.0, 16.0],
            NONE,
        );
        let rear = b.part_named(
            "",
            [16.0 * sx, 13.0, 34.0],
            [1.0, 0.0, 0.0],
            dr(first + 3),
            1.0,
            Some(body),
        );
        b.cube(
            rear,
            0,
            (0.0, 0.0),
            [-8.0, -4.0, -8.0],
            [16.0, 32.0, 16.0],
            NONE,
        );
        let rtip = b.part_named(
            "",
            [0.0, 32.0, -4.0],
            [0.5, 0.0, 0.0],
            dr(first + 4),
            1.0,
            Some(rear),
        );
        b.cube(
            rtip,
            0,
            (196.0, 0.0),
            [-6.0, -2.0, 0.0],
            [12.0, 32.0, 12.0],
            NONE,
        );
        let rfoot = b.part_named(
            "",
            [0.0, 31.0, 4.0],
            [0.75, 0.0, 0.0],
            dr(first + 5),
            1.0,
            Some(rtip),
        );
        b.cube(
            rfoot,
            0,
            (112.0, 0.0),
            [-9.0, 0.0, -20.0],
            [18.0, 6.0, 24.0],
            NONE,
        );
    }
    b.finish(1.0)
}

/// M388 — [`dragon_pose`]'s slots.
pub const DRAGON_ROOT: u8 = 0;
pub const DRAGON_HEAD: u8 = 1;
pub const DRAGON_JAW: u8 = 2;
/// The five neck segments, 3..=7.
pub const DRAGON_NECK: u8 = 3;
/// The twelve tail segments, 8..=19.
pub const DRAGON_TAIL: u8 = 8;
pub const DRAGON_BODY: u8 = 20;
/// The left wing (21) and its tip (22); the right wing (23) and its tip
/// (24).
pub const DRAGON_LWING: u8 = 21;
pub const DRAGON_RWING: u8 = 23;
/// A side's six leg parts: front leg, its tip and foot, then the hind leg,
/// its tip and foot — the left from 25, the right from 31.
pub const DRAGON_LLEGS: u8 = 25;
pub const DRAGON_RLEGS: u8 = 31;
/// M388 — the slot count.
pub const DRAGON_SLOTS: usize = 37;
/// M388 — `EnderDragon.flapTime`'s advance per tick, taken constant: vanilla
/// adds `0.2 / (horizontalSpeed·10 + 1)·2^dy` while flying (about 0.03 at
/// cruise) and 0.1 perched, from per-entity state Rewo does not keep;
/// switching rates without it would make the wingbeat jump.
pub const DRAGON_FLAP_PER_TICK: f32 = 0.05;

/// M388 — `EnderDragonModel.setupAnim` for level, straight flight (every
/// sample of the dragon's 64-tick flight history alike, which Rewo does not
/// keep): the jaw, the root's bounce (lifted `(bounce − 2)·16` px and moved
/// 48 px forward — Rewo drew it 2 blocks low and 3 back until M388), the
/// neck chain waving `cos(i·0.45 + flap)·0.15` with each segment placed at
/// the end of the last (10 px along its pitch) and the head at the chain's
/// end, the wings flapping, the legs bouncing, and the tail chain turned π
/// with its accumulating `sin(i·0.45 + flap)·0.05` wave. Returns each slot's
/// rotation delta (against its rest) and pivot offset.
pub fn dragon_pose(age: f32) -> ([[f32; 3]; DRAGON_SLOTS], [[f32; 3]; DRAGON_SLOTS]) {
    use std::f32::consts::PI;
    let mut rot = [[0.0f32; 3]; DRAGON_SLOTS];
    let mut off = [[0.0f32; 3]; DRAGON_SLOTS];
    let flap = age * DRAGON_FLAP_PER_TICK * PI * 2.0;
    rot[usize::from(DRAGON_JAW)][0] = (flap.sin() + 1.0) * 0.2;
    let mut bounce = (flap - 1.0).sin() + 1.0;
    bounce = (bounce * bounce + bounce * 2.0) * 0.05;
    let r = usize::from(DRAGON_ROOT);
    off[r] = [0.0, (bounce - 2.0) * 16.0, -48.0];
    rot[r][0] = bounce * 2.0 * PI / 180.0;
    // The neck: each segment at the chain's running end, then the end moves
    // 10 px along the segment's pitch (straight flight: no yaw, no roll).
    let (mut y, mut z) = (20.0f32, -12.0f32);
    for i in 0..5usize {
        let s = usize::from(DRAGON_NECK) + i;
        let x_rot = (i as f32 * 0.45 + flap).cos() * 0.15;
        rot[s][0] = x_rot;
        off[s] = [0.0, y - 20.0, z - (-12.0 - i as f32 * 10.0)];
        y += x_rot.sin() * 10.0;
        z -= x_rot.cos() * 10.0;
    }
    let h = usize::from(DRAGON_HEAD);
    off[h] = [0.0, y - 20.0, z - (-62.0)];
    // The wings.
    let lw = usize::from(DRAGON_LWING);
    let rw = usize::from(DRAGON_RWING);
    rot[lw] = [0.125 - flap.cos() * 0.2, -0.25, -(flap.sin() + 0.125) * 0.8];
    rot[lw + 1][2] = ((flap + 2.0).sin() + 0.5) * 0.75;
    rot[rw] = [rot[lw][0], 0.25, -rot[lw][2]];
    rot[rw + 1][2] = -rot[lw + 1][2];
    // The legs (`poseLimbs`): each angle `rest ± bounce·0.1`.
    for first in [usize::from(DRAGON_LLEGS), usize::from(DRAGON_RLEGS)] {
        let k = bounce * 0.1;
        rot[first][0] = k;
        rot[first + 1][0] = -k;
        rot[first + 2][0] = k;
        rot[first + 3][0] = k;
        rot[first + 4][0] = k;
        rot[first + 5][0] = k;
    }
    // The tail: turned π (`+180°`), its wave accumulating down the chain.
    let (mut y, mut z) = (10.0f32, 60.0f32);
    let mut wave = 0.0f32;
    for i in 0..12usize {
        let s = usize::from(DRAGON_TAIL) + i;
        wave += (i as f32 * 0.45 + flap).sin() * 0.05;
        rot[s] = [wave, PI, 0.0];
        off[s] = [0.0, y - 10.0, z - (60.0 + i as f32 * 10.0)];
        y += wave.sin() * 10.0;
        z -= PI.cos() * wave.cos() * 10.0;
    }
    (rot, off)
}

/// `HappyGhastModel`: the tame ghast — body + big inner cube, nine short
/// dangling legs. Mesh-scaled 4.0 (ground-anchored ≡ our scale).
/// The happy ghast's tentacles: `(offset, length)` per `PartNames.tentacle(i)`.
const HAPPY_GHAST_LEGS: [([f32; 3], f32); 9] = [
    ([-3.75, 7.0, -5.0], 5.0),
    ([1.25, 7.0, -5.0], 7.0),
    ([6.25, 7.0, -5.0], 4.0),
    ([-6.25, 7.0, 0.0], 5.0),
    ([-1.25, 7.0, 0.0], 5.0),
    ([3.75, 7.0, 0.0], 7.0),
    ([-3.75, 7.0, 5.0], 8.0),
    ([1.25, 7.0, 5.0], 8.0),
    ([6.25, 7.0, 5.0], 5.0),
];

/// M373 — the nine tentacle parts, `GhastModel.animateTentacles`'
/// `xRot = 0.2·sin(age·0.3 + i) + 0.4` (`Anim::GhastTentacle`), each at the
/// body's offset plus its own. Made once per model, so the ropes layer's
/// tentacles (the same mesh at 0.2) swing with the base's, and the harness
/// carries the same nine (without boxes) to keep its part list the ghast's.
///
/// M391 — under the body part (`Anim::HappyGhastBody` at (0, 16, 0)), as
/// vanilla's tentacles are the body's children: the harness squeeze scales
/// them too. Returns `(body, tentacles)`.
fn happy_ghast_tentacle_parts(b: &mut ModelBuilder) -> (usize, [usize; 9]) {
    let body = b.part([0.0, 16.0, 0.0], Anim::HappyGhastBody, 1.0);
    let legs = std::array::from_fn(|i| {
        let (off, _) = HAPPY_GHAST_LEGS[i];
        b.part_ext(
            off,
            [0.0; 3],
            Anim::GhastTentacle { i: i as u8 },
            1.0,
            Some(body),
        )
    });
    (body, legs)
}

/// `HappyGhastModel.createBodyLayer(isBaby, g)` before its scaling: the body,
/// the inner body (the baby's only, M314) on the static part, and the nine
/// tentacles on `legs` (M373; static until then), at deformation `g` (the
/// inner body `g - 0.5`).
fn happy_ghast_mesh(
    b: &mut ModelBuilder,
    tex: usize,
    g: f32,
    baby: bool,
    (body, legs): (usize, &[usize; 9]),
) {
    b.cube_f(
        body,
        tex,
        (0.0, 0.0),
        [-8.0, -8.0, -8.0],
        [16.0, 16.0, 16.0],
        g,
        false,
        NONE,
    );
    if baby {
        b.cube_f(
            body,
            tex,
            (0.0, 32.0),
            [-8.0, -16.0, -8.0],
            [16.0, 16.0, 16.0],
            g - 0.5,
            false,
            &[Fold::at([0.0, 8.0, 0.0])],
        );
    }
    for (i, (_, len)) in HAPPY_GHAST_LEGS.into_iter().enumerate() {
        b.cube_f(
            legs[i],
            tex,
            (0.0, 0.0),
            [-1.0, 0.0, -1.0],
            [2.0, len, 2.0],
            g,
            false,
            NONE,
        );
    }
}

/// `HAPPY_GHAST`: the adult body layer, scaled 4.0. (Until M314 it carried the
/// inner body too, which vanilla gives only the baby; it lay wholly inside the
/// body box and never showed.)
fn happy_ghast() -> Model {
    let mut b = ModelBuilder::new();
    let (body, legs) = happy_ghast_tentacle_parts(&mut b);
    happy_ghast_mesh(&mut b, 0, 0.0, false, (body, &legs));
    // M312 — `RopesLayer`: `HAPPY_GHAST_ROPES` is the adult body layer at
    // `CubeDeformation(0.2F)` (body and tentacles, no inner body) on its own
    // sheet, texture slot 1, which the renderer shows only while the ghast
    // holds a leash and wears a harness (`MobModel::ropes_slot`).
    happy_ghast_mesh(&mut b, 1, 0.2, false, (body, &legs));
    b.finish(4.0)
}

/// M314 — `HAPPY_GHAST_BABY`: `createBodyLayer(true, NONE)` (the inner body
/// included) scaled 4.0 and then by `BABY_TRANSFORMER`'s 0.2375, on its own
/// 64 x 64 sheet. `AgeableMobRenderer` swaps it in for a baby and scales it no
/// further (`getAgeScale`'s 0.2375 shrinks only the bounds).
fn happy_ghast_baby() -> Model {
    let mut b = ModelBuilder::new();
    let (body, legs) = happy_ghast_tentacle_parts(&mut b);
    happy_ghast_mesh(&mut b, 0, 0.0, true, (body, &legs));
    b.finish(4.0 * 0.2375)
}

/// M391 — `HappyGhastModel.BODY_SQUEEZE`: the body's x/y/z scale while
/// `state.bodyItem` is not empty (the ghast wears a harness), applied about
/// the body's pivot so its tentacles shrink with it and the harness (its
/// own model, unscaled) does not z-fight the body.
pub const HAPPY_GHAST_BODY_SQUEEZE: f32 = 0.9375;

/// M302 — `HappyGhastHarnessModel.createHarnessLayer(false)` (64 x 64,
/// `MeshTransformer.scaling(4.0F)`, which the happy ghast's own model scale
/// carries): the harness cube on the body's box, and the goggles (inflated
/// 0.15), which `setupAnim` lowers to `y = 14` level while ridden and raises
/// to `y = 9` tilted `-0.7854` otherwise. The harness cube coincides with the
/// ghast's body box, but the harnessed body is squeezed inside it (M391,
/// [`HAPPY_GHAST_BODY_SQUEEZE`]), so it needs no lift (M302-M390 lifted it
/// 0.002 px over the unsqueezed body). On the static part: the squeeze
/// scales the body part only.
pub fn happy_ghast_harness(ridden: bool) -> Model {
    let mut b = ModelBuilder::new();
    b.cube_f(
        STATIC_PART,
        0,
        (0.0, 0.0),
        [-8.0, -16.0, -8.0],
        [16.0, 16.0, 16.0],
        0.0,
        false,
        &[Fold::at([0.0, 24.0, 0.0])],
    );
    let goggles = if ridden {
        Fold::at([0.0, 14.0, -5.5])
    } else {
        Fold::rot([-0.7854, 0.0, 0.0], [0.0, 9.0, -5.5])
    };
    b.cube_f(
        STATIC_PART,
        0,
        (0.0, 32.0),
        [-8.0, -2.5, -2.5],
        [16.0, 5.0, 5.0],
        0.15,
        false,
        &[goggles],
    );
    // M373 — the ghast's nine tentacle parts, no boxes: a shell keeps its
    // mob's part list. M391 — and its body part: the harness itself is on
    // the static part, so the squeeze leaves it (`HappyGhastHarnessModel`
    // has no scale).
    happy_ghast_tentacle_parts(&mut b);
    let mut m = b.finish(1.0);
    m.scale = happy_ghast().scale;
    m
}

/// `CopperGolemModel`: the little statue — antennaed head, slab body,
/// mitten arms, stub legs. The whole mesh rides a +24 root translate.
fn copper_golem() -> Model {
    let mut b = ModelBuilder::new();
    // root translate(0,24,0) → body(0,−5,0); walk + idle rigs.
    let body = b.part_named(
        "body",
        [0.0, 19.0, 0.0],
        [0.0; 3],
        Anim::None,
        1.0,
        Option::None,
    );
    b.cube(
        body,
        0,
        (0.0, 15.0),
        [-4.0, -6.0, -3.0],
        [8.0, 6.0, 6.0],
        NONE,
    );
    let head = b.part_named(
        "head",
        [0.0, -6.0, 0.0],
        [0.0; 3],
        Anim::Head,
        1.0,
        Some(body),
    );
    b.cube_g(
        head,
        0,
        (0.0, 0.0),
        [-4.0, -5.0, -5.0],
        [8.0, 5.0, 10.0],
        0.015,
        NONE,
    );
    b.cube(
        head,
        0,
        (56.0, 0.0),
        [-1.0, -2.0, -6.0],
        [2.0, 3.0, 2.0],
        NONE,
    );
    b.cube_g(
        head,
        0,
        (37.0, 8.0),
        [-1.0, -9.0, -1.0],
        [2.0, 4.0, 2.0],
        -0.015,
        NONE,
    );
    b.cube_g(
        head,
        0,
        (37.0, 0.0),
        [-2.0, -13.0, -2.0],
        [4.0, 4.0, 4.0],
        -0.015,
        NONE,
    );
    let arm_r = b.part_named(
        "right_arm",
        [-4.0, -6.0, 0.0],
        [0.0; 3],
        Anim::None,
        1.0,
        Some(body),
    );
    b.cube(
        arm_r,
        0,
        (36.0, 16.0),
        [-3.0, -1.0, -2.0],
        [3.0, 10.0, 4.0],
        NONE,
    );
    let arm_l = b.part_named(
        "left_arm",
        [4.0, -6.0, 0.0],
        [0.0; 3],
        Anim::None,
        1.0,
        Some(body),
    );
    b.cube(
        arm_l,
        0,
        (50.0, 16.0),
        [0.0, -1.0, -2.0],
        [3.0, 10.0, 4.0],
        NONE,
    );
    let leg_r = b.part_named(
        "right_leg",
        [0.0, 19.0, 0.0],
        [0.0; 3],
        Anim::None,
        1.0,
        Option::None,
    );
    b.cube(
        leg_r,
        0,
        (0.0, 27.0),
        [-4.0, 0.0, -2.0],
        [4.0, 5.0, 4.0],
        NONE,
    );
    let leg_l = b.part_named(
        "left_leg",
        [0.0, 19.0, 0.0],
        [0.0; 3],
        Anim::None,
        1.0,
        Option::None,
    );
    b.cube(
        leg_l,
        0,
        (16.0, 27.0),
        [0.0, 0.0, -2.0],
        [4.0, 5.0, 4.0],
        NONE,
    );
    b.keyframe(
        &crate::anim_defs::COPPER_GOLEM_WALK,
        KfDriver::Walk {
            speed_f: 2.0,
            scale_f: 2.5,
        },
    );
    b.keyframe(&crate::anim_defs::COPPER_GOLEM_IDLE, KfDriver::Age);
    b.finish(1.0)
}

/// `NautilusModel` (128²): spiral shell + soft body with layered tentacle
/// slabs. The zombie variant shares the mesh.
fn nautilus() -> Model {
    nautilus_with(NautilusLayer::None)
}

/// M301 — which of the nautilus layers' boxes [`nautilus_with`] adds.
#[derive(Clone, Copy, PartialEq, Eq)]
enum NautilusLayer {
    None,
    Armor,
    Saddle,
}

/// M301 — `ModelLayers.NAUTILUS_ARMOR`: the shell boxes at 0.01 (and its
/// panel), `NAUTILUS_BODY`'s equipment layer shell on a 128 x 128 sheet; the
/// base model's parts, its boxes alone.
pub fn nautilus_armor() -> Model {
    let base = nautilus().quads.len();
    let mut m = nautilus_with(NautilusLayer::Armor);
    m.quads.drain(..base);
    m
}

/// M350 — `ZombieNautilusCoralModel.createBodyLayer()`: the nautilus mesh
/// with a `corals` part under the shell (offset (8, 4.5, -8)) holding four
/// corals of two zero-thickness planes each (yellow, pink, blue, red, each
/// plane turned about y or, the pink's second, z), on a 128 x 128 sheet.
/// The corals hide under body armour ([`Show::Unarmoured`]).
fn nautilus_coral() -> Model {
    nautilus_of(NautilusLayer::None, true)
}

/// M350 — the armour and saddle shells over the coral mesh: the same boxes
/// as [`nautilus_armor`] / [`nautilus_saddle`], on a part list that matches
/// the coral mesh's (its extra, empty corals part).
pub fn nautilus_coral_armor() -> Model {
    let base = nautilus().quads.len();
    let mut m = nautilus_of(NautilusLayer::Armor, true);
    m.quads.drain(..base);
    m
}

pub fn nautilus_coral_saddle() -> Model {
    let base = nautilus().quads.len();
    let mut m = nautilus_of(NautilusLayer::Saddle, true);
    m.quads.drain(..base);
    m
}

/// M301 — `ModelLayers.NAUTILUS_SADDLE`: the shell's top box at 0.2,
/// `NAUTILUS_SADDLE`'s layer shell.
pub fn nautilus_saddle() -> Model {
    let base = nautilus().quads.len();
    let mut m = nautilus_with(NautilusLayer::Saddle);
    m.quads.drain(..base);
    m
}

fn nautilus_with(layer: NautilusLayer) -> Model {
    nautilus_of(layer, false)
}

fn nautilus_of(layer: NautilusLayer, coral: bool) -> Model {
    let mut b = ModelBuilder::new();
    let root = Fold::at([0.0, 29.0, -6.0]);
    let shell = Fold::at([0.0, -13.0, 5.0]);
    b.cube_f(
        STATIC_PART,
        0,
        (0.0, 0.0),
        [-7.0, -10.0, -7.0],
        [14.0, 10.0, 16.0],
        0.0,
        false,
        &[shell, root],
    );
    b.cube_f(
        STATIC_PART,
        0,
        (0.0, 26.0),
        [-7.0, 0.0, -7.0],
        [14.0, 8.0, 20.0],
        0.0,
        false,
        &[shell, root],
    );
    b.cube_f(
        STATIC_PART,
        0,
        (48.0, 26.0),
        [-7.0, 0.0, 6.0],
        [14.0, 8.0, 0.0],
        0.0,
        false,
        &[shell, root],
    );
    // M372 — the soft body is a part (root + body, (0, 20.5, 6.3)) that
    // follows the look within ±10°; the beak rides on it.
    let body = b.part_named(
        "body",
        [0.0, 20.5, 6.3],
        [0.0; 3],
        Anim::NautilusBody,
        1.0,
        Option::None,
    );
    b.cube(
        body,
        0,
        (0.0, 54.0),
        [-5.0, -4.51, -3.0],
        [10.0, 8.0, 14.0],
        NONE,
    );
    b.cube(
        body,
        0,
        (0.0, 76.0),
        [-5.0, -4.51, 7.0],
        [10.0, 8.0, 0.0],
        NONE,
    );
    // The beak: upper/lower mouth slabs open and close on the swim rig,
    // children of the body (their pivots body-local).
    let upper = b.part_named(
        "upper_mouth",
        [0.0, -2.51, 7.0],
        [0.0; 3],
        Anim::None,
        1.0,
        Some(body),
    );
    b.cube_g(
        upper,
        0,
        (54.0, 54.0),
        [-5.0, -2.0, 0.0],
        [10.0, 4.0, 4.0],
        -0.001,
        NONE,
    );
    b.cube_f(
        body,
        0,
        (54.0, 70.0),
        [-3.0, -2.0, -0.5],
        [6.0, 4.0, 4.0],
        0.0,
        false,
        &[Fold::at([0.0, -0.51, 7.5])],
    );
    let lower = b.part_named(
        "lower_mouth",
        [0.0, 1.49, 7.0],
        [0.0; 3],
        Anim::None,
        1.0,
        Some(body),
    );
    b.cube_g(
        lower,
        0,
        (54.0, 62.0),
        [-5.0, -1.98, 0.0],
        [10.0, 4.0, 4.0],
        -0.001,
        NONE,
    );
    b.keyframe(
        &crate::anim_defs::NAUTILUS_SWIMMING,
        KfDriver::WalkPlusAge {
            age_div: 5.0,
            amt_add: 0.2,
            speed_f: 2.0,
            scale_f: 3.0,
        },
    );
    // M350 — the coral mesh's `corals` part; its planes only on the mesh
    // itself, so the layer shells keep the base boxes' count ahead of theirs.
    if coral {
        let c = b.part([0.0; 3], Anim::None, 1.0);
        b.show(c, Show::Unarmoured);
        if layer == NautilusLayer::None {
            let corals = Fold::at([8.0, 4.5, -8.0]);
            let (yellow, pink, blue) = (
                Fold::at([0.0, -11.0, 11.0]),
                Fold::at([-12.5, -18.0, 11.0]),
                Fold::at([-14.0, 0.0, 5.5]),
            );
            let y = |a: f32, p: [f32; 3]| Fold::rot([0.0, a, 0.0], p);
            for (uv, min, dims, folds) in [
                (
                    (0.0, 85.0),
                    [-4.5, -3.5, 0.0],
                    [6.0, 8.0, 0.0],
                    vec![y(-0.7854, [0.0, 0.0, 2.0]), yellow],
                ),
                (
                    (0.0, 85.0),
                    [-4.5, -3.5, 0.0],
                    [6.0, 8.0, 0.0],
                    vec![y(0.7854, [0.0; 3]), yellow],
                ),
                ((-8.0, 94.0), [-4.5, 4.5, 0.0], [6.0, 0.0, 8.0], vec![pink]),
                (
                    (-8.0, 94.0),
                    [-3.0, 0.0, -4.0],
                    [6.0, 0.0, 8.0],
                    vec![Fold::rot([0.0, 0.0, 1.5708], [-1.5, 4.5, 4.0]), pink],
                ),
                (
                    (0.0, 102.0),
                    [-3.5, -5.5, 0.0],
                    [5.0, 10.0, 0.0],
                    vec![y(0.7854, [0.0, 0.0, -2.0]), blue],
                ),
                (
                    (0.0, 102.0),
                    [-3.5, -5.5, 0.0],
                    [5.0, 10.0, 0.0],
                    vec![y(-0.7854, [0.0; 3]), blue],
                ),
                // The red coral's own part sits at the corals' origin.
                (
                    (0.0, 112.0),
                    [-2.5, -5.5, 0.0],
                    [4.0, 10.0, 0.0],
                    vec![y(-0.829, [-0.5, -1.0, 1.5])],
                ),
                (
                    (0.0, 112.0),
                    [-4.5, -5.5, 0.0],
                    [6.0, 10.0, 0.0],
                    vec![y(0.7854, [0.0; 3])],
                ),
            ] {
                let mut chain = folds;
                chain.extend([corals, shell, root]);
                b.cube_f(c, 0, uv, min, dims, 0.0, false, &chain);
            }
        }
    }
    // M301 — `NautilusArmorModel` / `NautilusSaddleModel`: the root replaced
    // by one holding only the shell, i.e. boxes on the shell's pose alone,
    // appended last so `nautilus_armor` / `nautilus_saddle` split them off.
    match layer {
        NautilusLayer::None => {}
        NautilusLayer::Armor => {
            for (uv, min, dims, g) in [
                ((0.0, 0.0), [-7.0, -10.0, -7.0], [14.0, 10.0, 16.0], 0.01),
                ((0.0, 26.0), [-7.0, 0.0, -7.0], [14.0, 8.0, 20.0], 0.01),
                // `CubeDeformation(0.0F)`: coplanar with the shell's own
                // panel, which vanilla's `LESS_THAN_OR_EQUAL` draws over;
                // Rewo's solid range is strictly `GREATER`, so the panel is
                // lifted two thousandths of a pixel instead.
                ((48.0, 26.0), [-7.0, 0.0, 6.0], [14.0, 8.0, 0.0], 0.002),
            ] {
                b.cube_f(STATIC_PART, 0, uv, min, dims, g, false, &[shell, root]);
            }
        }
        NautilusLayer::Saddle => {
            b.cube_f(
                STATIC_PART,
                0,
                (0.0, 0.0),
                [-7.0, -10.0, -7.0],
                [14.0, 10.0, 16.0],
                0.2,
                false,
                &[shell, root],
            );
        }
    }
    b.finish(1.0)
}

// ---------------------------------------------------------------------------
// Tests — the hand-computed vanilla ground truth from the handoff §3
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    /// `setupAnim`'s pose dispatch across the whole modelled truth table.
    /// The subtle case is `SPEAR`: `affectsOffhandPose` makes it suppress the
    /// *other* arm, and which arm that is flips with handedness.
    #[test]
    fn the_pose_dispatch_matches_vanillas_branch() {
        use super::{ArmPose::*, ArmPoses};
        let p = |right, left, right_handed| ArmPoses {
            right,
            left,
            right_handed,
            known: true,
            ..ArmPoses::EMPTY
        };
        // Nothing two-handed and nothing claiming the offhand: both posed.
        for (r, l) in [(Empty, Empty), (Item, Item), (Empty, Item), (Item, Empty)] {
            for rh in [true, false] {
                let a = p(r, l, rh);
                assert!(
                    a.poses_arm(false) && a.poses_arm(true),
                    "{r:?}/{l:?} rh={rh}"
                );
            }
        }
        // Right-handed: poseLeftArm runs first, so a SPEAR in the LEFT arm
        // (the off hand) stops the right arm being posed at all.
        let a = p(Item, Spear, true);
        assert!(!a.poses_arm(false) && a.poses_arm(true));
        // ...while a SPEAR in the right arm does not stop the left.
        let a = p(Spear, Item, true);
        assert!(a.poses_arm(false) && a.poses_arm(true));
        // Left-handed mirrors it exactly.
        let a = p(Spear, Item, false);
        assert!(a.poses_arm(false) && !a.poses_arm(true));
        let a = p(Item, Spear, false);
        assert!(a.poses_arm(false) && a.poses_arm(true));
        // An unresolvable hand suppresses both, whatever the poses say.
        let mut a = p(Item, Item, true);
        a.known = false;
        assert!(!a.poses_arm(false) && !a.poses_arm(true));
    }

    use super::*;

    /// The humanoid head at texOffs(0,0), box(−4,−8,−4, 8,8,8): every face's
    /// vertex array + UV corners hand-computed from vanilla `ModelPart.Cube`.
    #[test]
    fn cube_matches_vanilla_head_unwrap() {
        let faces = cube_faces((0.0, 0.0), [-4.0, -8.0, -4.0], [8.0, 8.0, 8.0], 0.0, false);

        // NORTH (front): verts {t1,t0,t3,t2}, rect (8,8,16,16).
        let (f, pos, uv) = &faces[3];
        assert_eq!(*f, Facing::North);
        assert_eq!(
            *pos,
            [
                [4.0, -8.0, -4.0],
                [-4.0, -8.0, -4.0],
                [-4.0, 0.0, -4.0],
                [4.0, 0.0, -4.0]
            ]
        );
        assert_eq!(*uv, [[16.0, 8.0], [8.0, 8.0], [8.0, 16.0], [16.0, 16.0]]);

        // UP (maxY plane): verts {t2,t3,l3,l2}, rect (16,8,24,0) — the
        // v1-before-v0 vertical flip.
        let (f, pos, uv) = &faces[1];
        assert_eq!(*f, Facing::Up);
        assert_eq!(
            *pos,
            [
                [4.0, 0.0, -4.0],
                [-4.0, 0.0, -4.0],
                [-4.0, 0.0, 4.0],
                [4.0, 0.0, 4.0]
            ]
        );
        assert_eq!(*uv, [[24.0, 8.0], [16.0, 8.0], [16.0, 0.0], [24.0, 0.0]]);

        // DOWN (minY plane, world top): verts {l1,l0,t0,t1}, rect (8,0,16,8).
        let (f, pos, uv) = &faces[0];
        assert_eq!(*f, Facing::Down);
        assert_eq!(
            *pos,
            [
                [4.0, -8.0, 4.0],
                [-4.0, -8.0, 4.0],
                [-4.0, -8.0, -4.0],
                [4.0, -8.0, -4.0]
            ]
        );
        assert_eq!(*uv, [[16.0, 0.0], [8.0, 0.0], [8.0, 8.0], [16.0, 8.0]]);

        // WEST rect (0,8,8,16); EAST rect (16,8,24,16); SOUTH rect (24,8,32,16).
        assert_eq!(
            faces[2].2,
            [[8.0, 8.0], [0.0, 8.0], [0.0, 16.0], [8.0, 16.0]]
        );
        assert_eq!(
            faces[4].2,
            [[24.0, 8.0], [16.0, 8.0], [16.0, 16.0], [24.0, 16.0]]
        );
        assert_eq!(
            faces[5].2,
            [[32.0, 8.0], [24.0, 8.0], [24.0, 16.0], [32.0, 16.0]]
        );
    }

    /// Mirror: X extremes swap and the vertex array reverses (each vertex
    /// keeps its UV corner).
    #[test]
    fn cube_mirror_swaps_x_and_reverses() {
        let plain = cube_faces((0.0, 16.0), [-2.0, 0.0, -2.0], [4.0, 12.0, 4.0], 0.0, false);
        let mirrored = cube_faces((0.0, 16.0), [-2.0, 0.0, -2.0], [4.0, 12.0, 4.0], 0.0, true);
        let (_, ppos, puv) = &plain[3];
        let (_, mpos, muv) = &mirrored[3];
        // Same rect, reversed orders; each mirrored vertex mirrors its
        // partner's UV assignment.
        for i in 0..4 {
            assert_eq!(muv[i], puv[3 - i]);
            let want = ppos[3 - i];
            assert_eq!(mpos[i][1], want[1]);
            assert_eq!(mpos[i][2], want[2]);
            // x negates around the box center (center x = 0 here).
            assert_eq!(mpos[i][0], -want[0]);
        }
    }

    /// Grow inflates the geometry but never the UV rect.
    #[test]
    fn grow_inflates_geometry_only() {
        let base = cube_faces(
            (16.0, 16.0),
            [-4.0, 0.0, -2.0],
            [8.0, 12.0, 4.0],
            0.0,
            false,
        );
        let grown = cube_faces(
            (16.0, 16.0),
            [-4.0, 0.0, -2.0],
            [8.0, 12.0, 4.0],
            0.25,
            false,
        );
        assert_eq!(base[3].2, grown[3].2);
        assert_eq!(grown[3].1[0], [4.25, -0.25, -2.25]);
    }

    /// Every registered mob builds with valid part indices and non-empty
    /// geometry, and every animated part's amp is sane.
    #[test]
    fn registry_builds_clean() {
        for def in MOBS {
            let m = (def.build)();
            assert!(!m.quads.is_empty(), "{:?} empty", def.kind);
            for q in &m.quads {
                assert!(q.part < m.parts.len(), "{:?} bad part", def.kind);
                assert!(q.tex < def.textures.len(), "{:?} bad tex index", def.kind);
                let n = q.normal;
                let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
                assert!((len - 1.0).abs() < 1e-4, "{:?} normal not unit", def.kind);
            }
            assert!(m.scale > 0.0);
        }
    }

    /// The model→world convention at yaw 0: front (model −Z) faces world +Z,
    /// the right arm (model −X) lands at world −X, feet at y≈0. This is the
    /// transform whose X sign the old code had wrong.
    #[test]
    fn player_world_orientation_at_yaw_zero() {
        let m = player();
        // M310 — the outer layers are child parts at their parent's pivot
        // (no rotation), so a part's pivot is summed up its parent chain.
        let pivot_of = |mut i: usize| -> [f32; 3] {
            let mut acc = [0.0f32; 3];
            loop {
                let part = &m.parts[i];
                for (a, p) in acc.iter_mut().zip(part.pivot) {
                    *a += p;
                }
                match part.parent {
                    Some(par) => i = par as usize,
                    None => return acc,
                }
            }
        };
        let to_world = |p: [f32; 3], part: usize| -> [f32; 3] {
            let pivot = pivot_of(part);
            let v = [p[0] + pivot[0], p[1] + pivot[1], p[2] + pivot[2]];
            // model → entity local: (−x, 24.016−y, z); yaw 0 ⇒ rotY(180°).
            let e = [-v[0], MODEL_EYE_Y - v[1], v[2]];
            [-e[0], e[1], -e[2]]
        };
        // Head front face (first cube's NORTH quad) must sit at +Z.
        let head_north = m
            .quads
            .iter()
            .find(|q| q.facing == Facing::North && m.parts[q.part].anim == Anim::Head)
            .unwrap();
        for corner in head_north.pos {
            let w = to_world(corner, head_north.part);
            assert!(w[2] > 0.0, "front face must face +Z, got {w:?}");
            // Head spans model-px y 24.016..32.016 above the feet.
            assert!(w[1] >= MODEL_EYE_Y - 0.1 && w[1] <= MODEL_EYE_Y + 8.1);
        }
        // Right arm reaches to x −8.25 px (west side; −8 base + 0.25 sleeve,
        // the sleeve on the arm's child part since M310).
        let on_right_arm = |i: usize| {
            m.parts[i].anim == Anim::HumanoidArmRight
                || m.parts[i]
                    .parent
                    .is_some_and(|p| m.parts[p as usize].anim == Anim::HumanoidArmRight)
        };
        let arm = m
            .quads
            .iter()
            .filter(|q| on_right_arm(q.part))
            .flat_map(|q| q.pos.iter().map(|p| to_world(*p, q.part)[0]))
            .fold(f32::MAX, f32::min);
        assert_eq!(arm, -8.25);
    }

    /// The player is the one model M19 poses from `attackTime`, so its body and
    /// arms must be real named parts at `HumanoidModel.createMesh`'s poses —
    /// and, crucially, at *zero* base rotation with the body pivot at the
    /// origin, which is what makes promoting them rest-geometry-neutral.
    #[test]
    fn the_player_humanoid_carries_the_attack_parts_at_the_vanilla_rest_pose() {
        for slim in [false, true] {
            let m = player_model(slim);
            let part = |name: &str| {
                m.parts
                    .iter()
                    .find(|p| p.name == name)
                    .unwrap_or_else(|| panic!("{name} missing (slim={slim})"))
            };
            let body = part("body");
            assert_eq!(body.pivot, [0.0; 3], "body PartPose.offset(0,0,0)");
            assert_eq!(body.rot, [0.0; 3], "no base rotation → rest pose unchanged");
            assert_eq!(body.parent, None);
            assert_eq!(body.anim, Anim::HumanoidBody);
            let right = part("right_arm");
            assert_eq!(right.pivot, [-HUMANOID_ARM_PIVOT_X, 2.0, 0.0]);
            assert_eq!(right.rot, [0.0; 3]);
            assert_eq!(right.anim, Anim::HumanoidArmRight);
            let left = part("left_arm");
            assert_eq!(left.pivot, [HUMANOID_ARM_PIVOT_X, 2.0, 0.0]);
            assert_eq!(left.rot, [0.0; 3]);
            assert_eq!(left.anim, Anim::HumanoidArmLeft);
        }
        // No other mob may carry the humanoid attack anims: they bake the ±5
        // arm pivot, and every other vanilla `HumanoidModel` subclass overrides
        // the arms in ways this client does not model.
        for def in MOBS {
            if matches!(
                def.kind,
                EntityModelKind::Player | EntityModelKind::PlayerSlim
            ) {
                continue;
            }
            let m = (def.build)();
            assert!(
                !m.parts.iter().any(|p| matches!(
                    p.anim,
                    Anim::HumanoidBody | Anim::HumanoidArmRight | Anim::HumanoidArmLeft
                )),
                "{} must not use the humanoid attack anims",
                def.kind.name()
            );
        }
    }

    /// The `MobDef::textures` pin (the last item of the M162–M165
    /// "known-deletable code" list): every declared key must exist in
    /// rewo-data's baked mob-texture spec table. A key with no spec row
    /// bakes no `MobTexture`, so `emit_model`'s slot lookup misses and
    /// the mob silently renders with missing sheets — the bake only
    /// *warns* ("{key} renders as a capsule") and `mobtexshot`'s oracle
    /// reads this same declaration, so nothing else would catch a typo
    /// or a renamed key. Asserting a non-empty table keeps the all-match
    /// form honest; `baby_swap_skips` rows are allowed to be orphans
    /// (they are keyed for M175's UV-offset swaps, not listed here).
    #[test]
    fn every_mob_texture_key_exists_in_the_baked_specs() {
        let specs: std::collections::HashSet<&str> = rewo_data::assets::mob_texture_specs()
            .map(|(k, _, _, _)| k)
            .collect();
        assert!(!specs.is_empty());
        for def in MOBS {
            assert!(
                !def.textures.is_empty(),
                "{:?} declares no textures",
                def.kind
            );
            for t in def.textures {
                assert!(
                    specs.contains(t),
                    "{:?} declares texture key {t:?} with no baked spec row",
                    def.kind
                );
            }
        }
    }

    #[test]
    fn entity_names_map_to_kinds() {
        assert_eq!(kind_for_entity_name("minecraft:cow"), EntityModelKind::Cow);
        assert_eq!(
            kind_for_entity_name("minecraft:glow_squid"),
            EntityModelKind::GlowSquid
        );
        assert_eq!(
            kind_for_entity_name("minecraft:magma_cube"),
            EntityModelKind::MagmaCube
        );
        assert_eq!(
            kind_for_entity_name("minecraft:wither_skeleton"),
            EntityModelKind::WitherSkeleton
        );
        assert_eq!(
            kind_for_entity_name("minecraft:warden"),
            EntityModelKind::Warden
        );
        // M238 — the armour stand has its model now (it was the capsule).
        assert_eq!(
            kind_for_entity_name("minecraft:armor_stand"),
            EntityModelKind::ArmorStand
        );
        // M68: the wire name resolves to the *small* plan, and the large one
        // is unreachable by name — its own `name()` included, which is not a
        // registered entity type and must not become a back door to it.
        assert_eq!(
            kind_for_entity_name("minecraft:tropical_fish"),
            EntityModelKind::TropicalFish
        );
        assert_eq!(
            kind_for_entity_name("minecraft:tropical_fish_large"),
            EntityModelKind::Capsule
        );
        // Every registry def's kind is reachable from its own wire name —
        // except the ones the caller picks from something the name cannot
        // carry: the player variants (type id + skin model) and the tropical
        // fish's large body plan (the packed variant's low bit, M68). Both
        // share a wire name with their sibling kind.
        for def in MOBS {
            if matches!(
                def.kind,
                EntityModelKind::Player
                    | EntityModelKind::PlayerSlim
                    | EntityModelKind::TropicalFishLarge
                    // M307 — the baby sulfur cube's model, the same shape.
                    | EntityModelKind::SulfurCubeSmall
                    // M314 — and the ghastling's, M315 the baby turtle's.
                    | EntityModelKind::HappyGhastBaby
                    | EntityModelKind::TurtleBaby
                    | EntityModelKind::PolarBearBaby
                    | EntityModelKind::ZombieBaby
                    | EntityModelKind::HuskBaby
                    | EntityModelKind::VillagerBaby
                    | EntityModelKind::DrownedBaby
                    | EntityModelKind::PiglinBaby
                    | EntityModelKind::ZombifiedPiglinBaby
                    | EntityModelKind::CowBaby
                    | EntityModelKind::MooshroomBaby
                    | EntityModelKind::PigBaby
                    | EntityModelKind::ChickenBaby
                    | EntityModelKind::SheepBaby
                    | EntityModelKind::DolphinBaby
                    | EntityModelKind::ZombieVillagerBaby
                    | EntityModelKind::SquidBaby
                    | EntityModelKind::GlowSquidBaby
                    | EntityModelKind::HoglinBaby
                    | EntityModelKind::ZoglinBaby
                    | EntityModelKind::StriderBaby
                    | EntityModelKind::FoxBaby
                    | EntityModelKind::PandaBaby
                    | EntityModelKind::OcelotBaby
                    | EntityModelKind::BeeBaby
                    | EntityModelKind::DonkeyBaby
                    | EntityModelKind::MuleBaby
                    | EntityModelKind::CamelBaby
                    | EntityModelKind::RabbitBaby
                    | EntityModelKind::GoatBaby
                    | EntityModelKind::LlamaBaby
                    | EntityModelKind::HorseBaby
                    | EntityModelKind::CatBaby
                    | EntityModelKind::WolfBaby
                    | EntityModelKind::ColdCow
                    | EntityModelKind::WarmCow
                    | EntityModelKind::ColdPig
                    | EntityModelKind::ColdChicken
                    | EntityModelKind::ZombieNautilusCoral
                    // M223 — a boat kind is a model shared by many wire
                    // types, none of them named after it; asserted below.
                    | EntityModelKind::Boat
                    | EntityModelKind::ChestBoat
                    | EntityModelKind::Raft
                    | EntityModelKind::ChestRaft
            ) {
                continue;
            }
            assert_eq!(
                kind_for_entity_name(&format!("minecraft:{}", def.kind.name())),
                def.kind,
                "wire mapping for {:?}",
                def.kind
            );
        }
        // The boat types (M223): every wood's boat and chest boat, and the
        // bamboo pair, which are rafts.
        for wood in [
            "oak", "spruce", "birch", "jungle", "acacia", "cherry", "dark_oak", "mangrove",
            "pale_oak",
        ] {
            assert_eq!(
                kind_for_entity_name(&format!("minecraft:{wood}_boat")),
                EntityModelKind::Boat
            );
            assert_eq!(
                kind_for_entity_name(&format!("minecraft:{wood}_chest_boat")),
                EntityModelKind::ChestBoat
            );
        }
        assert_eq!(
            kind_for_entity_name("minecraft:bamboo_raft"),
            EntityModelKind::Raft
        );
        assert_eq!(
            kind_for_entity_name("minecraft:bamboo_chest_raft"),
            EntityModelKind::ChestRaft
        );
    }

    /// Every layer table names a texture the mob actually lists, and no mob
    /// lists more slots than the per-slot tint scratch in `entities` sizes
    /// for. A table naming a key its `MobDef` does not carry would silently
    /// resolve to `None` and the layer would simply never draw.
    #[test]
    fn every_layer_table_names_a_texture_its_mob_carries() {
        const MAX_SCRATCH: usize = 4; // `entities::MAX_MOB_TEXTURES`
        for def in MOBS {
            assert!(
                def.textures.len() <= MAX_SCRATCH,
                "{:?} lists {} textures, past the tint scratch",
                def.kind,
                def.textures.len()
            );
            let has = |k: &str| def.textures.contains(&k);
            for (what, key) in [
                ("tinted", tinted_texture(def.kind)),
                ("shearable", shearable_texture(def.kind)),
                ("coplanar", coplanar_layer_texture(def.kind)),
                ("collar", collar_texture(def.kind)),
            ] {
                if let Some(k) = key {
                    assert!(
                        has(k),
                        "{:?}'s {what} texture {k} is not in its MobDef",
                        def.kind
                    );
                }
            }
            if let Some((b, p)) = fish_tint_textures(def.kind) {
                assert!(
                    has(b) && has(p),
                    "{:?}'s fish tint slots are not in its MobDef",
                    def.kind
                );
                assert_ne!(b, p, "the body and the pattern are different slots");
            }
        }
    }

    /// The undercoat is the **body** mesh, not the fleece: every quad it adds
    /// has a texture-0 twin at the same position, and none has a texture-1
    /// one. That is the whole `sheepBodyLayer`-not-`sheepWoolLayer` finding,
    /// stated as a property of the built mesh — a mesh copied from the fur
    /// layer would be inflated and share no position with the body at all.
    #[test]
    fn the_sheep_undercoat_is_the_body_mesh_repeated() {
        let m = sheep();
        let at = |tex: usize| {
            m.quads
                .iter()
                .filter(|q| q.tex == tex)
                .map(|q| format!("{:?}{:?}", q.facing, q.pos))
                .collect::<std::collections::HashSet<_>>()
        };
        let (base, fleece, under) = (at(0), at(1), at(2));
        assert!(!under.is_empty(), "the undercoat contributes quads");
        assert_eq!(
            under, base,
            "every undercoat quad is a body quad, and vice versa"
        );
        assert!(
            fleece.is_disjoint(&under),
            "the fleece is inflated 0.6/1.75/0.5, so it shares no position with either"
        );
    }

    /// The two fish plans are different meshes, and each carries its pattern
    /// layer as a second slot inflated by exactly `FISH_PATTERN_GROW`.
    #[test]
    fn each_fish_plan_carries_its_own_pattern_layer() {
        for (name, m) in [("small", tropical_fish()), ("large", tropical_fish_large())] {
            let body: Vec<_> = m.quads.iter().filter(|q| q.tex == 0).collect();
            let pattern: Vec<_> = m.quads.iter().filter(|q| q.tex == 1).collect();
            assert!(!body.is_empty(), "{name} has a body");
            assert_eq!(
                body.len(),
                pattern.len(),
                "{name}: the pattern is the same mesh"
            );
            // Same faces, strictly outside — an un-inflated pattern would be
            // coplanar and rejected by the solid pass's `GREATER` test.
            for (b, p) in body.iter().zip(pattern.iter()) {
                assert_eq!(b.facing, p.facing, "{name}");
                let d: f32 = (0..3).map(|i| (p.pos[0][i] - b.pos[0][i]).abs()).sum();
                assert!(
                    d > 0.0,
                    "{name}: the pattern layer is not coplanar with the body"
                );
            }
        }
        // …and the plans are not the same mesh at a different scale: the large
        // one has a bottom fin the small one has no part for.
        assert_ne!(
            tropical_fish().quads.len(),
            tropical_fish_large().quads.len()
        );
    }
}

// The player model, for the first-person hand (M38).
//
// Built once per variant and shared: the hand needs one named part out of it
// every frame, and rebuilding a whole humanoid mesh to read two cubes would
// be the same per-frame allocation the 2026-05-31 leak pass went looking for.
// -- worn armour (M46) --------------------------------------------------------
//
// `HumanoidModel.createArmorMeshSet` builds four models from the same humanoid
// mesh, differing only in how far it is inflated and which parts survive:
//
// ```java
// OUTER_ARMOR_DEFORMATION = new CubeDeformation(1.0F);
// INNER_ARMOR_DEFORMATION = new CubeDeformation(0.5F);
// ADULT_ARMOR_PARTS_PER_SLOT = {
//     HEAD:  {head},                        // retainPartsAndChildren
//     CHEST: {body, left_arm, right_arm},
//     LEGS:  {left_leg, right_leg, body},   // inner deformation
//     FEET:  {left_leg, right_leg},
// };
// ```
//
// So a chestplate is the body and both arms grown by 1.0, and leggings are the
// body and both legs grown by 0.5 — **the body appears in both**, which is
// exactly why the leggings need the thinner inflation: they sit inside the
// chestplate rather than z-fighting it.
//
// The legs are not the humanoid's own leg boxes. `createBaseArmorMesh`
// *replaces* them with `texOffs(0, 16)`, box `(-2, 0, -2)` 4x12x4 at
// `g.extend(-0.1)` — a tenth thinner than the rest of the piece, so the boot
// and the legging do not fight each other where they overlap.

/// Find the body part an armour box hangs off (M46).
///
/// **Not by name alone.** Only `body`, `left_arm` and `right_arm` carry names
/// in Rewo's humanoid builders — a name exists where a keyframe channel or a
/// CEM bone needs to target the part, and the head and legs never did. Naming
/// them to suit the armour would newly bind them to any CEM pack that declares
/// those bones, which is a behaviour change nothing here asked for. So the
/// lookup falls back to the part's **animation kind**, which is what actually
/// identifies a humanoid limb.
pub fn armor_part(parts: &[Part], want: &str) -> Option<usize> {
    if let Some(i) = parts.iter().position(|p| p.name == want) {
        return Some(i);
    }
    // M380 — the piglin's arms are `PiglinArm { left }`: matched exactly (a
    // discriminant would take the right arm for both).
    let piglin_side = match want {
        "right_arm" => Some(false),
        "left_arm" => Some(true),
        _ => Option::None,
    };
    if let Some(left) = piglin_side {
        if let Some(i) = parts
            .iter()
            .position(|p| p.anim == Anim::PiglinArm { left })
        {
            return Some(i);
        }
    }
    let kinds: &[Anim] = match want {
        "head" => &[
            Anim::Head,
            Anim::FelineHead,
            Anim::SheepHead,
            Anim::PiglinHead,
        ],
        "body" => &[Anim::HumanoidBody, Anim::PiglinBody],
        // `ArmRight`/`ArmLeft` (the plain piglin's until M380, matched above
        // since) stay for any generic-armed wearer. Widening the list is safe
        // because the caller has already filtered to the fourteen kinds
        // vanilla gives an armour layer.
        "right_arm" => &[
            Anim::HumanoidArmRight,
            Anim::UndeadArmRight,
            Anim::ArmRight,
            Anim::EndermanArmRight,
        ],
        "left_arm" => &[
            Anim::HumanoidArmLeft,
            Anim::UndeadArmLeft,
            Anim::ArmLeft,
            Anim::EndermanArmLeft,
        ],
        "right_leg" => &[Anim::LegRight, Anim::EndermanLegRight],
        "left_leg" => &[Anim::LegLeft, Anim::EndermanLegLeft],
        _ => return None,
    };
    if let Some(i) = parts.iter().position(|p| {
        kinds
            .iter()
            .any(|k| std::mem::discriminant(&p.anim) == std::mem::discriminant(k))
    }) {
        return Some(i);
    }
    // A humanoid **mob** has no `body` part at all: `humanoid_head_body` puts
    // its torso cube straight on the static root, because only the player's
    // torso animates (M19 gave it one for `setupAttackAnimation`). The armour
    // belongs on the root, in exactly the space that cube occupies.
    //
    // Safe as an unconditional fallback because the caller has already asked
    // [`wears_humanoid_armor`] — nothing reaches here that vanilla would not
    // hang a humanoid armour model off.
    if want == "body" {
        return Some(STATIC_PART);
    }
    None
}

/// Whether vanilla gives this entity a `HumanoidArmorLayer`.
///
/// **Transcribed from the renderers, not inferred from the model.** Every
/// mention of `HumanoidArmorLayer` in 26.2 is one of:
///
/// ```text
/// AvatarRenderer                       player
/// AbstractZombieRenderer               zombie, husk (extends ZombieRenderer), drowned
/// AbstractSkeletonRenderer             skeleton, stray, bogged, wither_skeleton, parched
/// ZombieVillagerRenderer               zombie_villager
/// ZombifiedPiglinRenderer              zombified_piglin
/// PiglinRenderer                       piglin, piglin_brute
/// ArmorStandRenderer                   armor_stand (M238)
/// GiantMobRenderer                     giant (M304; `GIANT_ARMOR` is the armour scaled 6.0, as the mob)
/// ```
///
/// Deliberately not a geometric test. An earlier form asked "does the model
/// have humanoid arms", which is *nearly* the same set and wrong at both
/// edges: an **allay** has arms and no legs, and an **illager** or a
/// **creaking** has the full humanoid limb set — yet none of the three has an
/// armour layer, so equipping one renders nothing in vanilla. The renderers
/// are the rule; the mesh only looks like it is.
pub fn wears_humanoid_armor(kind: EntityModelKind) -> bool {
    matches!(
        kind,
        EntityModelKind::Player
            | EntityModelKind::PlayerSlim
            | EntityModelKind::ArmorStand
            | EntityModelKind::Zombie
            | EntityModelKind::Husk
            | EntityModelKind::Drowned
            | EntityModelKind::ZombieVillager
            | EntityModelKind::ZombifiedPiglin
            | EntityModelKind::Skeleton
            | EntityModelKind::Stray
            | EntityModelKind::Bogged
            | EntityModelKind::WitherSkeleton
            | EntityModelKind::Parched
            | EntityModelKind::Piglin
            | EntityModelKind::PiglinBrute
            | EntityModelKind::Giant
    )
}

/// Whether a model kind wears a cape — `CapeLayer` is added by
/// `AvatarRenderer` alone (M60).
///
/// The same rule as [`wears_humanoid_armor`] and for the same reason: the
/// *renderer* decides, not the mesh. Every humanoid here has a torso to hang
/// a cape from, and vanilla gives one to none of them.
pub fn wears_cape(kind: EntityModelKind) -> bool {
    matches!(kind, EntityModelKind::Player | EntityModelKind::PlayerSlim)
}

/// `PlayerCapeModel.createCapeLayer`'s `PartPose` **offset** — the cape hangs
/// two pixels behind the body's origin.
///
/// The pose's *rotation* is deliberately absent: it is `Ry(π)`, and
/// `setupAnim`'s quaternion opens with `rotateY(-PI)`, which
/// `ModelPart.rotateBy` post-multiplies onto it — so the two cancel exactly
/// and the net rotation is [`crate::entities::cape_rotation`] alone. Applying
/// both would rotate the cape twice.
pub const CAPE_PIVOT: [f32; 3] = [0.0, 0.0, 2.0];

/// `PlayerCapeModel.createCapeLayer`'s single cube:
/// `texOffs(0, 0).addBox(-5, 0, -1, 10, 16, 1, CubeDeformation.NONE, 1.0F, 0.5F)`.
///
/// The trailing `1.0F, 0.5F` are `xTexScale`/`yTexScale`, not a deformation —
/// `CubeDefinition.bake` multiplies them into the 64x64 `LayerDefinition`
/// size, so the UVs this returns normalize against **64x32**. They reach
/// `u 22, v 17`, which fits either sheet; only the divisor tells them apart.
pub fn cape_faces() -> [(Facing, [[f32; 3]; 4], [[f32; 2]; 4]); 6] {
    cube_faces((0.0, 0.0), [-5.0, 0.0, -1.0], [10.0, 16.0, 1.0], 0.0, false)
}

/// One face of one slab of the wavy cape (M61), addressed by joint rather
/// than by position.
///
/// The simulation hands the renderer a spine — a polyline of joints — and
/// nothing else. Each corner of each face is then "joint `j`, offset `off`
/// in that joint's own frame", so the geometry follows whatever shape the
/// chain has taken without the generator knowing anything about it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CapeSlabQuad {
    /// Which spine joint each corner hangs from.
    pub joint: [usize; 4],
    /// Each corner's offset from the spine, `[along the width, along the
    /// thickness]`, in model units. The cube is x −5..5 and z −1..0, so the
    /// spine is at z −0.5 and the thickness offsets are ±0.5.
    pub off: [[f32; 2]; 4],
    /// Sheet UVs in the cape's own 64×32 space, ready for
    /// `entities::cape_face_uv`.
    pub uv: [[f32; 2]; 4],
}

/// The wavy cape's geometry: `n` stacked slabs of `10 × 16/n × 1` model
/// units, with the box UV subdivided straight down the same 64×32 sheet the
/// single cube uses (M61).
///
/// # Why the internal caps are not emitted
///
/// Consecutive slabs share their boundary quad exactly — corner positions
/// are derived from the shared **joint's** frame, not from each slab's own —
/// so the surface is already watertight when the chain bends. Emitting the
/// caps as well would put two coincident quads at every interior joint,
/// which under reversed-Z depth is a z-fight, for geometry that can never be
/// seen. Only the cape's two real ends get one.
///
/// # It reduces
///
/// At `n == 1` this is `cape_faces()` exactly: one slab spanning y 0..16,
/// all six faces, and a UV shift of zero. That is a *structural* property
/// and not the spec's reduction rule — the rule is about bypassing the
/// simulation, and the renderer does that with its own branch, because the
/// two paths reach world space through different arithmetic and only the
/// bypass can be bit-for-bit.
pub fn cape_slab_quads(n: usize) -> Vec<CapeSlabQuad> {
    let n = n.max(1);
    let h = 16.0 / n as f32;
    let mut out = Vec::with_capacity(n * 4 + 2);
    for i in 0..n {
        let ya = i as f32 * h;
        for (facing, pos, uv) in
            cube_faces((0.0, 0.0), [-5.0, ya, -1.0], [10.0, h, 1.0], 0.0, false)
        {
            let cap = matches!(facing, Facing::Down | Facing::Up);
            if cap && !((facing == Facing::Down && i == 0) || (facing == Facing::Up && i + 1 == n))
            {
                continue;
            }
            let mut q = CapeSlabQuad {
                joint: [0; 4],
                off: [[0.0; 2]; 4],
                uv: [[0.0; 2]; 4],
            };
            for k in 0..4 {
                let p = pos[k];
                // `cube_faces` places every corner at exactly one of the two
                // slab boundaries, so this is a selection and not a rounding.
                q.joint[k] = if (p[1] - ya).abs() <= (p[1] - (ya + h)).abs() {
                    i
                } else {
                    i + 1
                };
                q.off[k] = [p[0], p[2] + 0.5];
                // The caps keep the sheet's own 1-pixel strip (v 0..1); only
                // the four side faces subdivide, and they do it by sliding
                // down the sheet exactly as far as the slab sits down the
                // cape.
                q.uv[k] = if cap {
                    uv[k]
                } else {
                    [uv[k][0], uv[k][1] + ya]
                };
            }
            out.push(q);
        }
    }
    out
}

/// Which humanoid parts a slot's armour covers, and how far the mesh is grown.
///
/// The names are the *body* model's part names: the armour is posed by the
/// same transforms the body was, which is what makes a chestplate swing with
/// the arm it is on rather than floating beside it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArmorSlot {
    Head,
    Chest,
    Legs,
    Feet,
}

impl ArmorSlot {
    /// `usesInnerModel` — only the leggings.
    pub fn inner(self) -> bool {
        matches!(self, ArmorSlot::Legs)
    }

    /// `CubeDeformation` for this slot: 0.5 inside a chestplate, 1.0 outside.
    pub fn grow(self) -> f32 {
        if self.inner() {
            0.5
        } else {
            1.0
        }
    }

    /// `ADULT_ARMOR_PARTS_PER_SLOT`.
    pub fn parts(self) -> &'static [&'static str] {
        match self {
            ArmorSlot::Head => &["head"],
            ArmorSlot::Chest => &["body", "left_arm", "right_arm"],
            ArmorSlot::Legs => &["left_leg", "right_leg", "body"],
            ArmorSlot::Feet => &["left_leg", "right_leg"],
        }
    }
}

/// One armour box, in the part's own local space.
///
/// `part` names the body part whose transform poses it — `emit_model` already
/// computes those, and reusing them is the only way the armour can follow a
/// walk cycle or a combat swing without a second animation implementation.
pub struct ArmorBox {
    pub part: &'static str,
    /// Texture offset into the 64x32 armour sheet.
    pub uv: (f32, f32),
    pub min: [f32; 3],
    pub dims: [f32; 3],
    /// Extra inflation beyond the slot's, for the legs' `extend(-0.1)`.
    pub extend: f32,
    pub mirror: bool,
}

/// The armour boxes for one slot, from `HumanoidModel.createMesh` with
/// `createBaseArmorMesh`'s leg replacement.
///
/// The head's `retainPartsAndChildren` keeps the hat too, but the hat is the
/// *skin's* second layer — the armour sheet has nothing at those coordinates,
/// so only the head box is emitted.
pub fn armor_boxes(slot: ArmorSlot) -> Vec<ArmorBox> {
    let leg = |mirror: bool| ArmorBox {
        part: if mirror { "left_leg" } else { "right_leg" },
        uv: (0.0, 16.0),
        min: [-2.0, 0.0, -2.0],
        dims: [4.0, 12.0, 4.0],
        // `g.extend(-0.1)` — the legs are a tenth thinner than the rest.
        extend: -0.1,
        mirror,
    };
    match slot {
        ArmorSlot::Head => vec![ArmorBox {
            part: "head",
            uv: (0.0, 0.0),
            min: [-4.0, -8.0, -4.0],
            dims: [8.0, 8.0, 8.0],
            extend: 0.0,
            mirror: false,
        }],
        ArmorSlot::Chest => vec![
            ArmorBox {
                part: "body",
                uv: (16.0, 16.0),
                min: [-4.0, 0.0, -2.0],
                dims: [8.0, 12.0, 4.0],
                extend: 0.0,
                mirror: false,
            },
            ArmorBox {
                part: "right_arm",
                uv: (40.0, 16.0),
                min: [-3.0, -2.0, -2.0],
                dims: [4.0, 12.0, 4.0],
                extend: 0.0,
                mirror: false,
            },
            // The left arm is the **mirrored right rect**, not its own — the
            // armour sheet is 64x32 and has no second arm on it.
            ArmorBox {
                part: "left_arm",
                uv: (40.0, 16.0),
                min: [-1.0, -2.0, -2.0],
                dims: [4.0, 12.0, 4.0],
                extend: 0.0,
                mirror: true,
            },
        ],
        ArmorSlot::Legs => vec![
            ArmorBox {
                part: "body",
                uv: (16.0, 16.0),
                min: [-4.0, 0.0, -2.0],
                dims: [8.0, 12.0, 4.0],
                extend: 0.0,
                mirror: false,
            },
            leg(false),
            leg(true),
        ],
        ArmorSlot::Feet => vec![leg(false), leg(true)],
    }
}

pub fn player_model_for_hand(slim: bool) -> &'static Model {
    static WIDE: std::sync::OnceLock<Model> = std::sync::OnceLock::new();
    static SLIM: std::sync::OnceLock<Model> = std::sync::OnceLock::new();
    if slim {
        SLIM.get_or_init(|| player_model(true))
    } else {
        WIDE.get_or_init(|| player_model(false))
    }
}

#[cfg(test)]
mod creeper_armor_tests {
    use super::*;

    /// M285 — `CREEPER_ARMOR` is the creeper at `CubeDeformation(2.0F)`: the
    /// head cube spans `-6..6` (8 px, 2 more each side), and the shell keeps
    /// the base model's parts in order (its transforms index them) and its
    /// UVs (vanilla's box UVs ignore the deformation).
    #[test]
    fn the_creeper_armour_is_the_creeper_inflated_two() {
        let (base, shell) = (creeper(), creeper_armor());
        assert_eq!(base.parts.len(), shell.parts.len());
        assert_eq!(base.quads.len(), shell.quads.len());
        let span = |m: &Model| {
            let xs = m.quads.iter().flat_map(|q| q.pos.iter().map(|p| p[0]));
            xs.fold((f32::MAX, f32::MIN), |(lo, hi), x| (lo.min(x), hi.max(x)))
        };
        assert_eq!(span(&base), (-4.0, 4.0));
        assert_eq!(span(&shell), (-6.0, 6.0));
        for (a, b) in base.quads.iter().zip(&shell.quads) {
            assert_eq!((a.part, a.uv), (b.part, b.uv), "same parts, same UVs");
        }
    }
}

#[cfg(test)]
mod breeze_wind_tests {
    use super::*;

    /// M286 — the breeze's wind slot is `breeze_wind.png`, and the model has
    /// quads on it (the funnel the wind layer draws) as well as off it.
    #[test]
    fn the_breeze_wind_slot_is_its_wind_sheet() {
        let def = MOBS
            .iter()
            .find(|d| d.kind == EntityModelKind::Breeze)
            .unwrap();
        assert_eq!(def.textures[BREEZE_WIND_SLOT], "breeze_wind");
        let m = breeze();
        assert!(m.quads.iter().any(|q| q.tex == BREEZE_WIND_SLOT));
        assert!(m.quads.iter().any(|q| q.tex != BREEZE_WIND_SLOT));
    }
}

#[cfg(test)]
mod wither_armor_tests {
    use super::*;

    /// M287 — `WITHER_ARMOR` is the wither at `CubeDeformation(0.5F)`: the
    /// centre head's cube spans `-4.5..4.5` against `-4..4`, the parts and
    /// UVs are the base's.
    #[test]
    fn the_wither_armour_is_the_wither_inflated_half() {
        let (base, shell) = (wither(), wither_armor());
        assert_eq!(base.parts.len(), shell.parts.len());
        assert_eq!(base.quads.len(), shell.quads.len());
        for (a, b) in base.quads.iter().zip(&shell.quads) {
            assert_eq!((a.part, a.uv), (b.part, b.uv), "same parts, same UVs");
        }
        // The centre head is the seventh cube (after the shoulders, the
        // ribcage's four and the tail): quads 36..42, pivot at the origin.
        let span = |m: &Model| {
            m.quads[36..42]
                .iter()
                .flat_map(|q| q.pos.iter().map(|p| p[1]))
                .fold((f32::MAX, f32::MIN), |(lo, hi), y| (lo.min(y), hi.max(y)))
        };
        assert_eq!(span(&base), (-4.0, 4.0));
        assert_eq!(span(&shell), (-4.5, 4.5));
    }
}

#[cfg(test)]
mod equine_scale_tests {
    use super::*;

    /// M293 — the horse family's model scales, from `LayerDefinitions`
    /// (`livingHorseScale` 1.1, `createBodyLayer(0.87F)` / `(0.92F)`); the
    /// undead horses keep the bare body layer.
    #[test]
    fn the_equines_carry_vanillas_layer_scales() {
        let scale = |k: EntityModelKind| {
            let def = MOBS.iter().find(|d| d.kind == k).unwrap();
            (def.build)().scale
        };
        let base = scale(EntityModelKind::SkeletonHorse);
        assert_eq!(base, scale(EntityModelKind::ZombieHorse));
        assert!((scale(EntityModelKind::Horse) - base * 1.1).abs() < 1e-6);
        assert!((scale(EntityModelKind::Donkey) - base * 0.87).abs() < 1e-6);
        assert!((scale(EntityModelKind::Mule) - base * 0.92).abs() < 1e-6);
    }
}

#[cfg(test)]
mod horse_armor_tests {
    use super::*;

    /// M294 — `HORSE_ARMOR` is the horse at 0.1 (the body still 0.05): the
    /// same parts and UVs, and the legs' boxes 0.2 wider.
    #[test]
    fn the_horse_armour_is_the_horse_inflated_a_tenth() {
        let (base, shell) = (horse(), horse_armor());
        assert_eq!(base.parts.len(), shell.parts.len());
        assert_eq!(base.quads.len(), shell.quads.len());
        assert_eq!(base.scale, shell.scale);
        for (a, b) in base.quads.iter().zip(&shell.quads) {
            assert_eq!((a.part, a.uv), (b.part, b.uv));
        }
        let width = |m: &Model, part: usize| {
            let xs = m
                .quads
                .iter()
                .filter(|q| q.part == part)
                .flat_map(|q| q.pos.iter().map(|p| p[0]));
            let (lo, hi) = xs.fold((f32::MAX, f32::MIN), |(lo, hi), x| (lo.min(x), hi.max(x)));
            hi - lo
        };
        let leg = base.quads.iter().map(|q| q.part).max().unwrap();
        assert!((width(&shell, leg) - width(&base, leg) - 0.2).abs() < 1e-4);
        assert_eq!(undead_horse_armor().scale, equine(false).scale);
    }
}

#[cfg(test)]
mod saddle_tests {
    use super::*;

    /// M295 — the saddle mesh is the body mesh plus six boxes (the saddle,
    /// the two bit pieces, the head strap and the mouth wrap — five quads
    /// lists of six), at the kind's scale; the reins are two zero-thickness
    /// boxes on the static part, two faces each.
    #[test]
    fn the_saddle_is_the_body_plus_its_boxes() {
        let base = horse();
        let (saddle, reins) = equine_saddle(false, HORSE_SCALE);
        assert_eq!(saddle.parts.len(), base.parts.len());
        assert_eq!(saddle.quads.len(), base.quads.len() + 5 * 6);
        assert_eq!(saddle.scale, base.scale);
        assert_eq!(reins.scale, base.scale);
        // Two faces each: a zero-thickness box's other four have no area
        // (vanilla emits them, and they draw nothing).
        assert_eq!(reins.quads.len(), 2 * 2);
        // M359 — the reins ride the head group (it looks).
        assert!(reins
            .quads
            .iter()
            .all(|q| base.parts[q.part].anim == Anim::EquineHead));
    }
}

#[cfg(test)]
mod donkey_chest_tests {
    use super::*;

    /// M296 — the donkey and mule meshes carry two chest boxes on a part
    /// shown only with `hasChest`; the horse's does not.
    #[test]
    fn the_donkey_carries_its_chests_hidden_until_chested() {
        let chested = |m: &Model| {
            m.parts
                .iter()
                .enumerate()
                .filter(|(_, p)| p.show == Show::Chested)
                .map(|(i, _)| m.quads.iter().filter(|q| q.part == i).count())
                .sum::<usize>()
        };
        assert_eq!(chested(&donkey()), 2 * 6);
        assert_eq!(chested(&mule()), 2 * 6);
        assert_eq!(chested(&horse()), 0);
    }
}

#[cfg(test)]
mod llama_decor_tests {
    use super::*;

    /// M297 — the llama's chests sit on a part shown only with `hasChest`,
    /// and the decor shell is the llama at 0.5 (same parts and UVs, the legs
    /// one wider).
    #[test]
    fn the_llama_hides_its_chests_and_its_decor_is_inflated_half() {
        let (base, decor) = (llama(), llama_decor());
        let chested: usize = base
            .parts
            .iter()
            .enumerate()
            .filter(|(_, p)| p.show == Show::Chested)
            .map(|(i, _)| base.quads.iter().filter(|q| q.part == i).count())
            .sum();
        assert_eq!(chested, 2 * 6);
        assert_eq!(base.parts.len(), decor.parts.len());
        assert_eq!(base.quads.len(), decor.quads.len());
        for (a, b) in base.quads.iter().zip(&decor.quads) {
            assert_eq!((a.part, a.uv), (b.part, b.uv));
        }
        let leg = base.quads.iter().map(|q| q.part).max().unwrap();
        let width = |m: &Model| {
            let xs = m
                .quads
                .iter()
                .filter(|q| q.part == leg)
                .flat_map(|q| q.pos.iter().map(|p| p[0]));
            let (lo, hi) = xs.fold((f32::MAX, f32::MIN), |(lo, hi), x| (lo.min(x), hi.max(x)));
            hi - lo
        };
        assert!((width(&decor) - width(&base) - 1.0).abs() < 1e-4);
    }
}

#[cfg(test)]
mod pig_saddle_tests {
    use super::*;

    /// M298 — the pig's saddle shell is the pig at 0.5 (same parts and UVs,
    /// the body box one wider); the strider's is the strider.
    #[test]
    fn the_pig_saddle_is_the_pig_inflated_half() {
        let (base, shell) = (pig(), pig_saddle());
        assert_eq!(base.parts.len(), shell.parts.len());
        assert_eq!(base.quads.len(), shell.quads.len());
        for (a, b) in base.quads.iter().zip(&shell.quads) {
            assert_eq!((a.part, a.uv), (b.part, b.uv));
        }
        let leg = base.quads.iter().map(|q| q.part).max().unwrap();
        let width = |m: &Model| {
            let xs = m
                .quads
                .iter()
                .filter(|q| q.part == leg)
                .flat_map(|q| q.pos.iter().map(|p| p[0]));
            let (lo, hi) = xs.fold((f32::MAX, f32::MIN), |(lo, hi), x| (lo.min(x), hi.max(x)));
            hi - lo
        };
        assert!((width(&shell) - width(&base) - 1.0).abs() < 1e-4);
        assert_eq!(strider_saddle().quads.len(), strider().quads.len());
    }
}

#[cfg(test)]
mod wolf_armor_tests {
    use super::*;

    /// M299 — the wolf's armour shell is the wolf at 0.2: same parts and
    /// UVs, the legs 0.4 wider.
    #[test]
    fn the_wolf_armour_is_the_wolf_inflated_a_fifth() {
        // M342: the bare wolf (its collar copy is not the armour's).
        let (base, shell) = (wolf_with(0.0), wolf_armor());
        assert_eq!(base.parts.len(), shell.parts.len());
        assert_eq!(base.quads.len(), shell.quads.len());
        for (a, b) in base.quads.iter().zip(&shell.quads) {
            assert_eq!((a.part, a.uv), (b.part, b.uv));
        }
        let leg = base
            .parts
            .iter()
            .position(|p| p.anim == Anim::WolfFront { left: true })
            .unwrap();
        let width = |m: &Model| {
            let xs = m
                .quads
                .iter()
                .filter(|q| q.part == leg)
                .flat_map(|q| q.pos.iter().map(|p| p[0]));
            let (lo, hi) = xs.fold((f32::MAX, f32::MIN), |(lo, hi), x| (lo.min(x), hi.max(x)));
            hi - lo
        };
        assert!((width(&shell) - width(&base) - 0.4).abs() < 1e-4);
    }
}

#[cfg(test)]
mod camel_saddle_tests {
    use super::*;

    /// M300 — the camel saddle is the camel plus eight boxes (three saddle,
    /// five bridle); the reins are three zero-thickness boxes (two faces
    /// each), all on the camel's own parts.
    #[test]
    fn the_camel_saddle_is_the_camel_plus_its_boxes() {
        let base = camel();
        let (saddle, reins) = camel_saddle();
        assert_eq!(saddle.parts.len(), base.parts.len());
        assert_eq!(saddle.quads.len(), base.quads.len() + 8 * 6);
        assert_eq!(reins.quads.len(), 3 * 2);
        let head = base.parts.iter().position(|p| p.name == "head").unwrap();
        assert!(reins.quads.iter().all(|q| q.part == head));
    }
}

#[cfg(test)]
mod nautilus_layer_tests {
    use super::*;

    /// M301 — the armour is the shell's three boxes (the panel's two faces
    /// only), the saddle one box, on the static part with the base's parts.
    #[test]
    fn the_nautilus_layers_are_boxes_on_the_shell() {
        let base = nautilus();
        let (armor, saddle) = (nautilus_armor(), nautilus_saddle());
        assert_eq!(armor.parts.len(), base.parts.len());
        assert_eq!(saddle.parts.len(), base.parts.len());
        // The panel is zero-thick: its two broad faces only (vanilla's other
        // four have no area).
        assert_eq!(armor.quads.len(), 6 + 6 + 2);
        assert_eq!(saddle.quads.len(), 6);
        assert!(armor
            .quads
            .iter()
            .chain(&saddle.quads)
            .all(|q| q.part == STATIC_PART));
    }
}

#[cfg(test)]
mod harness_tests {
    use super::*;

    /// M302 — the harness is two boxes on the static part at the ghast's
    /// scale, and only the goggles move between ridden and not.
    #[test]
    fn the_harness_goggles_drop_when_ridden() {
        let (up, down) = (happy_ghast_harness(false), happy_ghast_harness(true));
        assert_eq!(up.quads.len(), 12);
        assert_eq!(up.scale, happy_ghast().scale);
        assert!(up.quads.iter().all(|q| q.part == STATIC_PART));
        assert_eq!(
            up.quads[..6].iter().map(|q| q.pos).collect::<Vec<_>>(),
            down.quads[..6].iter().map(|q| q.pos).collect::<Vec<_>>()
        );
        let low = |m: &Model| {
            m.quads[6..]
                .iter()
                .flat_map(|q| q.pos.iter().map(|p| p[1]))
                .fold(f32::MAX, f32::min)
        };
        assert!(
            low(&down) > low(&up),
            "ridden goggles sit lower on screen (larger model y)"
        );
    }
}

#[cfg(test)]
mod giant_tests {
    use super::*;

    /// M304 — the giant is the zombie six times over: the same quads and
    /// parts, the model scale six times the zombie's.
    #[test]
    fn the_giant_is_the_zombie_at_six_times_the_size() {
        let (z, g) = (zombie(), giant());
        assert_eq!(g.scale, z.scale * 6.0);
        assert_eq!(g.parts.len(), z.parts.len());
        assert_eq!(
            g.quads.iter().map(|q| (q.pos, q.uv)).collect::<Vec<_>>(),
            z.quads.iter().map(|q| (q.pos, q.uv)).collect::<Vec<_>>()
        );
        assert!(wears_humanoid_armor(EntityModelKind::Giant));
        assert_eq!(
            kind_for_entity_name("minecraft:giant"),
            EntityModelKind::Giant
        );
    }
}

#[cfg(test)]
mod translucent_tests {
    use super::*;

    /// M306 — the slime is its inner body (core, eyes, mouth: 24 quads),
    /// then the translucent shell (6).
    #[test]
    fn the_slime_is_its_inner_body_under_a_translucent_shell() {
        let m = slime();
        assert_eq!(m.quads.len(), 30);
        assert_eq!(m.translucent_from, Some(24));
        assert_eq!(zombie().translucent_from, None);
    }
}

#[cfg(test)]
mod sulfur_cube_tests {
    use super::*;

    /// M307 — each sulfur cube is its opaque inner box, then its
    /// translucent outer box, standing on the ground (model y 24) once the
    /// renderer's `vOffset` is folded in.
    #[test]
    fn the_sulfur_cubes_stand_on_the_ground_inside_translucent_shells() {
        for (m, scale, side) in [(sulfur_cube(), 0.5, 18.0), (sulfur_cube_small(), 1.0, 10.0)] {
            assert_eq!(m.quads.len(), 12);
            assert_eq!(m.translucent_from, Some(6));
            assert_eq!(m.scale, scale);
            assert!(m.quads[..6].iter().all(|q| q.tex == 1));
            assert!(m.quads[6..].iter().all(|q| q.tex == 0));
            let ys = m.quads[6..].iter().flat_map(|q| q.pos.iter().map(|p| p[1]));
            let (lo, hi) = ys.fold((f32::MAX, f32::MIN), |(a, b), y| (a.min(y), b.max(y)));
            assert!((hi - lo - side).abs() < 1e-4, "outer box is {side} px tall");
            // Vanilla's `vOffset` leaves the adult's bottom a third of a model
            // pixel up (0.98 rather than 1 - 1/16 + 1/16 x 9/9).
            assert!((hi - 24.0).abs() < 0.5, "its bottom is on the ground: {hi}");
        }
        assert_eq!(
            kind_for_entity_name("minecraft:sulfur_cube"),
            EntityModelKind::SulfurCube
        );
    }
}

#[cfg(test)]
mod avatar_layer_tests {
    use super::*;

    /// M310 — each of the six outer layers is one inflated box on its own
    /// part, a child at its parent's pivot shown by its mask bit; the base
    /// boxes stay on parts shown always.
    #[test]
    fn the_players_outer_layers_each_ride_a_part_shown_by_their_bit() {
        for m in [player(), player_slim()] {
            let mut bits = Vec::new();
            for (i, part) in m.parts.iter().enumerate() {
                if let Show::AvatarPart(bit) = part.show {
                    assert_eq!(part.pivot, [0.0; 3]);
                    assert!(part.parent.is_some());
                    assert_eq!(m.quads.iter().filter(|q| q.part == i).count(), 6);
                    bits.push(bit);
                }
            }
            bits.sort_unstable();
            assert_eq!(
                bits,
                [
                    AVATAR_JACKET,
                    AVATAR_LEFT_SLEEVE,
                    AVATAR_RIGHT_SLEEVE,
                    AVATAR_LEFT_PANTS,
                    AVATAR_RIGHT_PANTS,
                    AVATAR_HAT
                ]
            );
            assert_eq!(m.quads.len(), 72, "twelve boxes, as before");
        }
    }
}

#[cfg(test)]
mod boat_root_tests {
    use super::*;

    /// M393 — every boat kind (and the water patch) hangs off the same root
    /// chain (the wobble part, then the `rotY(90°)` root), and the paddles
    /// are two parts under the root on `BoatPaddle`, their boxes on them;
    /// nothing is left on the static part.
    #[test]
    fn the_boats_ride_a_wobble_root_and_their_paddles_are_parts() {
        for m in [
            boat(),
            chest_boat(),
            raft(),
            chest_raft(),
            boat_water_patch(),
        ] {
            assert_eq!(m.parts[1].anim, Anim::BoatHurt);
            // M394 — the bubble tilt between the wobble and the root.
            assert_eq!(m.parts[2].anim, Anim::BoatBubble);
            assert_eq!(m.parts[2].parent, Some(1));
            assert_eq!(m.parts[3].parent, Some(2));
            assert_eq!(m.parts[3].rot, [0.0, FRAC_PI_2, 0.0]);
            assert!(m.quads.iter().all(|q| q.part != STATIC_PART));
        }
        for m in [boat(), chest_boat(), raft(), chest_raft()] {
            let paddles: Vec<usize> = (0..m.parts.len())
                .filter(|&i| matches!(m.parts[i].anim, Anim::BoatPaddle { .. }))
                .collect();
            assert_eq!(paddles.len(), 2);
            for &p in &paddles {
                assert_eq!(m.parts[p].parent, Some(3));
                assert_eq!(m.quads.iter().filter(|q| q.part == p).count(), 12);
            }
        }
    }
}

#[cfg(test)]
mod ghast_ropes_tests {
    use super::*;

    /// M312 — the ropes are the body and nine tentacles again, 0.2 larger,
    /// on texture slot 1, after the body's own boxes; since M373 they share
    /// the base mesh's nine tentacle parts, and since M391 its body part
    /// (eleven parts in all).
    #[test]
    fn the_happy_ghasts_ropes_are_its_body_inflated_on_their_own_sheet() {
        let m = happy_ghast();
        let ropes: Vec<_> = m.quads.iter().filter(|q| q.tex == 1).collect();
        assert_eq!(ropes.len(), 60, "ten boxes");
        assert_eq!(m.parts.len(), 11);
        let span = |qs: &[&RawQuad]| {
            qs.iter()
                .flat_map(|q| q.pos.iter().map(|p| p[0]))
                .fold((f32::MAX, f32::MIN), |(a, b), x| (a.min(x), b.max(x)))
        };
        let body: Vec<_> = m.quads.iter().filter(|q| q.tex == 0).take(6).collect();
        let (lo, hi) = span(&ropes);
        let (blo, bhi) = span(&body);
        assert!((blo - lo - 0.2).abs() < 1e-4 && (hi - bhi - 0.2).abs() < 1e-4);
    }
}

#[cfg(test)]
mod ghastling_tests {
    use super::*;

    /// M314 — the ghastling is the body, the inner body and nine tentacles
    /// at 4 x 0.2375; the adult has no inner body (vanilla's is the baby's
    /// only).
    #[test]
    fn the_ghastling_is_the_baby_body_layer_at_its_own_scale() {
        let (baby, adult) = (happy_ghast_baby(), happy_ghast());
        assert_eq!(baby.quads.len(), 11 * 6);
        assert!((baby.scale - 0.95).abs() < 1e-6);
        assert_eq!(adult.quads.iter().filter(|q| q.tex == 0).count(), 10 * 6);
        assert_eq!(adult.scale, 4.0);
    }
}

#[cfg(test)]
mod baby_turtle_tests {
    use super::*;

    /// M315 — the baby turtle's shell is 4 px wide, its head looks.
    #[test]
    fn the_baby_turtle_is_its_own_small_model() {
        let m = turtle_baby();
        assert_eq!(m.scale, 1.0);
        // Part-local corners plus the part's pivot (M365 made the legs
        // parts; every part here is a root part unrotated at rest).
        let xs = m.quads.iter().flat_map(|q| {
            let px = m.parts[q.part].pivot[0];
            q.pos.iter().map(move |p| p[0] + px)
        });
        let (lo, hi) = xs.fold((f32::MAX, f32::MIN), |(a, b), x| (a.min(x), b.max(x)));
        assert_eq!(
            (lo, hi),
            (-4.0, 4.0),
            "the legs reach 2 px beyond the 4-px shell"
        );
        assert!(m.parts.iter().any(|p| p.anim == Anim::Head));
    }
}

#[cfg(test)]
mod polar_cub_tests {
    use super::*;

    /// M316 — the cub: a static body, a looking head of four boxes, four
    /// walking legs.
    #[test]
    fn the_polar_bear_cub_is_its_own_model() {
        let m = polar_bear_baby();
        assert_eq!(m.quads.len(), 9 * 6);
        let anims: Vec<Anim> = m.parts.iter().map(|p| p.anim).collect();
        for a in [
            Anim::Head,
            Anim::QuadHindRight,
            Anim::QuadHindLeft,
            Anim::QuadFrontRight,
            Anim::QuadFrontLeft,
        ] {
            assert!(anims.contains(&a), "{a:?}");
        }
    }
}

#[cfg(test)]
mod baby_zombie_tests {
    use super::*;

    /// M317 — `BabyZombieModel`'s seven boxes at their own texture offsets:
    /// the head's front face samples (9, 9)..(15, 15) of the 64-px sheet,
    /// not the adult's (8, 8)..(16, 16).
    #[test]
    fn the_baby_zombie_is_its_own_model() {
        let m = zombie_baby();
        assert_eq!(m.quads.len(), 7 * 6);
        let head = m.parts.iter().position(|p| p.anim == Anim::Head).unwrap();
        let front = m
            .quads
            .iter()
            .find(|q| q.part == head && q.facing == Facing::North)
            .unwrap();
        let us: Vec<f32> = front.uv.iter().map(|u| u[0]).collect();
        let vs: Vec<f32> = front.uv.iter().map(|u| u[1]).collect();
        let span = |v: &[f32]| {
            v.iter()
                .fold((f32::MAX, f32::MIN), |(a, b), x| (a.min(*x), b.max(*x)))
        };
        assert_eq!(span(&us), (9.0, 15.0));
        assert_eq!(span(&vs), (9.0, 15.0));
        assert_eq!(husk_baby().scale, 1.0625);
        assert!(!wears_humanoid_armor(EntityModelKind::ZombieBaby));
    }
}

#[cfg(test)]
mod baby_villager_tests {
    use super::*;

    /// M318 — `BabyVillagerModel`'s eleven boxes; the head's front face
    /// samples (7, 7)..(15, 15) of its sheet (an 8 x 8 face behind a 7-deep
    /// side), not the adult's (8, 8)..(16, 18).
    #[test]
    fn the_baby_villager_is_its_own_model() {
        let m = villager_baby();
        assert_eq!(m.quads.len(), 11 * 6);
        let head = m.parts.iter().position(|p| p.anim == Anim::Head).unwrap();
        let front = m
            .quads
            .iter()
            .find(|q| q.part == head && q.facing == Facing::North)
            .unwrap();
        let span = |i: usize| {
            front
                .uv
                .iter()
                .map(|u| u[i])
                .fold((f32::MAX, f32::MIN), |(a, b), x| (a.min(x), b.max(x)))
        };
        assert_eq!((span(0), span(1)), ((7.0, 15.0), (7.0, 15.0)));
    }
}

#[cfg(test)]
mod baby_drowned_tests {
    use super::*;

    /// M319 — the baby drowned is the baby zombie's seven boxes and their
    /// outer layer on slot 1, the zombie's boxes unchanged.
    #[test]
    fn the_baby_drowned_is_the_baby_zombie_with_its_outer_layer() {
        let (d, z) = (drowned_baby(), zombie_baby());
        assert_eq!(d.quads.len(), 14 * 6);
        assert_eq!(d.quads.iter().filter(|q| q.tex == 1).count(), 7 * 6);
        let base = |m: &Model| {
            m.quads
                .iter()
                .filter(|q| q.tex == 0)
                .map(|q| (q.pos, q.uv))
                .collect::<Vec<_>>()
        };
        assert_eq!(base(&d).len(), base(&z).len());
        assert!(base(&d).iter().all(|q| base(&z).contains(q)));
    }
}

#[cfg(test)]
mod baby_piglin_tests {
    use super::*;

    /// M320 — `BabyPiglinModel`'s nine boxes; the head box's front samples
    /// (7, 7)..(16, 13) of its sheet.
    #[test]
    fn the_baby_piglin_is_its_own_model() {
        let m = piglin_baby();
        assert_eq!(m.quads.len(), 9 * 6);
        // M390 — the posing head (`PiglinHead`).
        let head = m
            .parts
            .iter()
            .position(|p| p.anim == Anim::PiglinHead)
            .unwrap();
        let fronts: Vec<_> = m
            .quads
            .iter()
            .filter(|q| q.part == head && q.facing == Facing::North)
            .collect();
        // The head box's front is the widest north face (the snout's is 3 px).
        let width = |q: &&&RawQuad| {
            let us = q.uv.iter().map(|u| u[0]);
            us.clone().fold(f32::MIN, f32::max) - us.fold(f32::MAX, f32::min)
        };
        let big = fronts
            .iter()
            .max_by(|a, b| width(a).total_cmp(&width(b)))
            .expect("the head box's front");
        let span = |i: usize| {
            big.uv
                .iter()
                .map(|u| u[i])
                .fold((f32::MAX, f32::MIN), |(a, b), x| (a.min(x), b.max(x)))
        };
        assert_eq!((span(0), span(1)), ((7.0, 16.0), (7.0, 13.0)));
        assert_eq!(zombified_piglin_baby().quads.len(), 9 * 6);
    }
}

#[cfg(test)]
mod calf_tests {
    use super::*;

    /// M321 — `BabyCowModel`'s nine boxes and the head box's front at
    /// (5, 23)..(11, 29) of its sheet.
    #[test]
    fn the_calf_is_its_own_model() {
        let m = cow_baby();
        assert_eq!(m.quads.len(), 9 * 6);
        let head = m.parts.iter().position(|p| p.anim == Anim::Head).unwrap();
        let span = |q: &RawQuad, i: usize| {
            q.uv.iter()
                .map(|u| u[i])
                .fold((f32::MAX, f32::MIN), |(a, b), x| (a.min(x), b.max(x)))
        };
        let front = m
            .quads
            .iter()
            .filter(|q| q.part == head && q.facing == Facing::North)
            .max_by(|a, b| {
                let w = |q: &RawQuad| span(q, 0).1 - span(q, 0).0;
                w(a).total_cmp(&w(b))
            })
            .unwrap();
        assert_eq!(
            (span(front, 0), span(front, 1)),
            ((5.0, 11.0), (23.0, 29.0))
        );
    }
}

#[cfg(test)]
mod piglet_chick_tests {
    use super::*;

    fn span(q: &RawQuad, i: usize) -> (f32, f32) {
        q.uv.iter()
            .map(|u| u[i])
            .fold((f32::MAX, f32::MIN), |(a, b), x| (a.min(x), b.max(x)))
    }

    fn widest_north(m: &Model, part: usize) -> ((f32, f32), (f32, f32)) {
        let q = m
            .quads
            .iter()
            .filter(|q| q.part == part && q.facing == Facing::North)
            .max_by(|a, b| {
                let w = |q: &RawQuad| span(q, 0).1 - span(q, 0).0;
                w(a).total_cmp(&w(b))
            })
            .unwrap();
        (span(q, 0), span(q, 1))
    }

    /// M322 — `BabyPigModel`'s seven boxes, the head box's front at
    /// (6, 21)..(13, 27) of its sheet.
    #[test]
    fn the_piglet_is_its_own_model() {
        let m = pig_baby();
        assert_eq!(m.quads.len(), 7 * 6);
        let head = m.parts.iter().position(|p| p.anim == Anim::Head).unwrap();
        assert_eq!(widest_north(&m, head), ((6.0, 13.0), (21.0, 27.0)));
    }

    /// M322 — `BabyChickenModel`: the body and beak (6 faces each), four
    /// plates of 2 faces (the legs and feet) and two wings of 2; no head
    /// part; the body's front at (4, 4)..(8, 8).
    #[test]
    fn the_chick_is_its_own_model() {
        let m = chicken_baby();
        assert_eq!(m.quads.len(), 6 + 6 + 4 * 2 + 2 * 2);
        assert!(m.parts.iter().all(|p| p.anim != Anim::Head));
        assert_eq!(widest_north(&m, STATIC_PART), ((4.0, 8.0), (4.0, 8.0)));
    }
}

#[cfg(test)]
mod lamb_tests {
    use super::*;

    /// M323 — `BabySheepModel`'s six boxes twice (the body, then the fleece
    /// on the same mesh, quad for quad), and the head box's front at
    /// (5, 5)..(10, 10) of its sheet.
    #[test]
    fn the_lamb_is_its_own_model_and_its_fleece_is_coplanar() {
        let m = sheep_baby();
        assert_eq!(m.quads.len(), 2 * 6 * 6);
        let (body, fleece) = m.quads.split_at(36);
        assert!(body.iter().all(|q| q.tex == 0) && fleece.iter().all(|q| q.tex == 1));
        for (a, f) in body.iter().zip(fleece) {
            assert_eq!((a.pos, a.uv, a.part), (f.pos, f.uv, f.part));
        }
        let head = m
            .parts
            .iter()
            .position(|p| p.anim == Anim::SheepHead)
            .unwrap();
        let front = body
            .iter()
            .find(|q| q.part == head && q.facing == Facing::North)
            .unwrap();
        let span = |i: usize| {
            front
                .uv
                .iter()
                .map(|u| u[i])
                .fold((f32::MAX, f32::MIN), |(a, b), x| (a.min(x), b.max(x)))
        };
        assert_eq!((span(0), span(1)), ((5.0, 10.0), (5.0, 10.0)));
    }
}

#[cfg(test)]
mod dolphin_calf_tests {
    use super::*;

    /// M324 — `BabyDolphinModel`'s eight boxes; its tail and tail fin on the
    /// stroke (k 0.1 and 0.2, the fin under the tail); the head box's front
    /// at (4, 4)..(10, 9) of its sheet and in front of the body's.
    #[test]
    fn the_dolphin_calf_is_its_own_model() {
        let m = dolphin_baby();
        assert_eq!(m.quads.len(), 8 * 6);
        let tail = m
            .parts
            .iter()
            .position(|p| p.anim == Anim::DolphinTail { k: 0.1, rest: 0.0 })
            .unwrap();
        let fin = m
            .parts
            .iter()
            .position(|p| p.anim == Anim::DolphinTail { k: 0.2, rest: 0.0 })
            .unwrap();
        assert_eq!(m.parts[fin].parent, Some(tail as u16));
        let span = |q: &RawQuad, i: usize| {
            q.uv.iter()
                .map(|u| u[i])
                .fold((f32::MAX, f32::MIN), |(a, b), x| (a.min(x), b.max(x)))
        };
        let front = m
            .quads
            .iter()
            .find(|q| q.facing == Facing::North && span(q, 0) == (4.0, 10.0))
            .expect("the head box's front");
        assert_eq!(span(front, 1), (4.0, 9.0));
        // The body's front is (28, 8)..(34, 13); the head's sits 4 px ahead
        // of it (body z -4, head z -4 under its -4 offset), whichever way
        // the model's z runs.
        let body_front = m
            .quads
            .iter()
            .find(|q| q.facing == Facing::North && span(q, 0) == (28.0, 34.0))
            .expect("the body box's front");
        let z = |q: &RawQuad| q.pos[0][2];
        assert!((z(front) - z(body_front)).abs() > 1e-4);
        assert!(front.pos.iter().all(|p| (p[2] - z(front)).abs() < 1e-5));
    }
}

#[cfg(test)]
mod zombie_villager_baby_tests {
    use super::*;

    /// M325 — `BabyZombieVillagerModel`'s ten boxes, its arms on the undead
    /// pose, and the head box's front at (7, 7)..(15, 15) of its sheet.
    #[test]
    fn the_baby_zombie_villager_is_its_own_model() {
        let m = zombie_villager_baby();
        assert_eq!(m.quads.len(), 10 * 6);
        for a in [Anim::UndeadArmRight, Anim::UndeadArmLeft] {
            assert_eq!(m.parts.iter().filter(|p| p.anim == a).count(), 1);
        }
        let head = m.parts.iter().position(|p| p.anim == Anim::Head).unwrap();
        let span = |q: &RawQuad, i: usize| {
            q.uv.iter()
                .map(|u| u[i])
                .fold((f32::MAX, f32::MIN), |(a, b), x| (a.min(x), b.max(x)))
        };
        let front = m
            .quads
            .iter()
            .find(|q| q.part == head && q.facing == Facing::North && span(q, 1) == (7.0, 15.0))
            .expect("the head box's front");
        assert_eq!(span(front, 0), (7.0, 15.0));
    }
}

#[cfg(test)]
mod squid_baby_tests {
    use super::*;

    /// M326 — `BabySquidModel`: the body and eight tentacles on the wave,
    /// the body's front at (8, 8)..(16, 18) of its sheet and a tentacle's
    /// at (2, 20)..(4, 26).
    #[test]
    fn the_baby_squid_is_its_own_model() {
        let m = squid_baby();
        assert_eq!(m.quads.len(), 9 * 6);
        assert_eq!(
            m.parts
                .iter()
                .filter(|p| p.anim == Anim::SquidTentacle)
                .count(),
            8
        );
        let span = |q: &RawQuad, i: usize| {
            q.uv.iter()
                .map(|u| u[i])
                .fold((f32::MAX, f32::MIN), |(a, b), x| (a.min(x), b.max(x)))
        };
        let north: Vec<_> = m
            .quads
            .iter()
            .filter(|q| q.facing == Facing::North)
            .map(|q| (span(q, 0), span(q, 1)))
            .collect();
        assert!(north.contains(&((8.0, 16.0), (8.0, 18.0))));
        assert_eq!(
            north
                .iter()
                .filter(|r| **r == ((2.0, 4.0), (20.0, 26.0)))
                .count(),
            8
        );
    }
}

#[cfg(test)]
mod hoglet_tests {
    use super::*;

    /// M327 — `BabyHoglinModel`: eleven boxes, the mane a 0-wide plate (two
    /// faces), so 10 x 6 + 2 faces; the head box's front at (12, 12)..
    /// (22, 16) of its sheet; the right ear at the -2π/9 `setupAnim` rests
    /// it at, not its `PartPose` -0.8727.
    #[test]
    fn the_hoglet_is_its_own_model() {
        let m = hoglin_baby();
        assert_eq!(m.quads.len(), 10 * 6 + 2);
        let head = m
            .parts
            .iter()
            .position(|p| p.anim == Anim::HoglinHead { baby: true })
            .unwrap();
        let span = |q: &RawQuad, i: usize| {
            q.uv.iter()
                .map(|u| u[i])
                .fold((f32::MAX, f32::MIN), |(a, b), x| (a.min(x), b.max(x)))
        };
        assert!(m.quads.iter().any(|q| q.part == head
            && q.facing == Facing::North
            && (span(q, 0), span(q, 1)) == ((12.0, 22.0), (12.0, 16.0))));
        // M367 — the ears are parts: the right ear's −2π/9 is its part's
        // rest `zRot` (not the `PartPose` −0.8727), and its down face is
        // still the (36, 5)..(42, 9) rect.
        // The right ear's (32+4, 5)..(32+4+6, 5+4) face (vanilla's box-UV
        // `Down` rect): its normal is the head's vertical turned -2π/9 about
        // z (then tilted about x), so its x component is sin(2π/9), where
        // the `PartPose` -0.8727 would make it sin(0.8727).
        let ear_top = m
            .quads
            .iter()
            .find(|q| {
                q.facing == Facing::Down && (span(q, 0), span(q, 1)) == ((36.0, 42.0), (5.0, 9.0))
            })
            .expect("the right ear's down face");
        assert_eq!(m.parts[ear_top.part].anim, Anim::HoglinEar { left: false });
        assert!((m.parts[ear_top.part].rot[2] + 2.0 * PI / 9.0).abs() < 1e-6);
    }
}

#[cfg(test)]
mod baby_strider_tests {
    use super::*;

    /// M328 — `BabyStriderModel`: the body (6 faces), three bristle plates
    /// (2 each) and two legs; the body's front at (8, 8)..(15, 15); and the
    /// body resting where `customAnimations` puts it (y 17.25), which sinks
    /// it 0.5 px into the 4 px legs (an eighth of a leg).
    #[test]
    fn the_baby_strider_is_its_own_model() {
        let m = strider_baby();
        assert_eq!(m.quads.len(), 6 + 3 * 2 + 2 * 6);
        let span = |q: &RawQuad, i: usize| {
            q.uv.iter()
                .map(|u| u[i])
                .fold((f32::MAX, f32::MIN), |(a, b), x| (a.min(x), b.max(x)))
        };
        assert!(m
            .quads
            .iter()
            .any(|q| q.facing == Facing::North
                && (span(q, 0), span(q, 1)) == ((8.0, 15.0), (8.0, 15.0))));
        let y_extent = |part: usize| {
            // Quads are part-local: a part's extent adds its pivot.
            let y0 = m.parts.get(part).map_or(0.0, |p| p.pivot[1]);
            m.quads
                .iter()
                .filter(|q| q.part == part)
                .flat_map(|q| q.pos.iter().map(move |p| y0 + p[1]))
                .fold((f32::MAX, f32::MIN), |(a, b), y| (a.min(y), b.max(y)))
        };
        let leg = m
            .parts
            .iter()
            .position(|p| p.anim == Anim::StriderLeg { left: false })
            .unwrap();
        // M366 — the body is its own part (it bobs and rolls).
        let body = m
            .parts
            .iter()
            .position(|p| p.anim == Anim::StriderBody { adult: false })
            .unwrap();
        let (b0, b1) = y_extent(body);
        let (l0, l1) = y_extent(leg);
        let overlap = b1.min(l1) - b0.max(l0);
        let ratio = overlap / (l1 - l0);
        assert!(
            (ratio - 0.125).abs() < 1e-4,
            "body/leg overlap {ratio} of a leg"
        );
    }
}

#[cfg(test)]
mod kit_tests {
    use super::*;

    /// M330 — `BabyFoxModel`: ten boxes, the head box's front at (5, 5)..
    /// (11, 10) of its sheet, the tail under the body, and `FOX_BABY_WALK`
    /// on the walk with every one of its channels bound to a part (the
    /// build panics on a name it cannot find), the legs among them.
    #[test]
    fn the_kit_is_its_own_model_on_its_own_walk() {
        let m = fox_baby();
        assert_eq!(m.quads.len(), 10 * 6);
        let head = m.parts.iter().position(|p| p.anim == Anim::Head).unwrap();
        let span = |q: &RawQuad, i: usize| {
            q.uv.iter()
                .map(|u| u[i])
                .fold((f32::MAX, f32::MIN), |(a, b), x| (a.min(x), b.max(x)))
        };
        assert!(m.quads.iter().any(|q| q.part == head
            && q.facing == Facing::North
            && (span(q, 0), span(q, 1)) == ((5.0, 11.0), (5.0, 10.0))));
        let named = |n: &str| m.parts.iter().position(|p| p.name == n).unwrap();
        assert_eq!(m.parts[named("tail")].parent, Some(named("body") as u16));
        assert_eq!(m.keyframes.len(), 1);
        let kf = &m.keyframes[0];
        assert!(std::ptr::eq(kf.def, &crate::anim_defs::FOX_BABY_WALK));
        assert!(
            matches!(kf.driver, KfDriver::Walk { speed_f, scale_f } if speed_f == 1.0 && scale_f == 2.5)
        );
        assert_eq!(kf.parts.len(), kf.def.channels.len());
        for leg in [
            "right_hind_leg",
            "left_hind_leg",
            "right_front_leg",
            "left_front_leg",
        ] {
            assert!(
                kf.parts.contains(&(named(leg) as u16)),
                "{leg} is not on the walk"
            );
        }
    }
}

#[cfg(test)]
mod panda_cub_tests {
    use super::*;

    /// M331 — `BabyPandaModel`: nine boxes, the head box's front at (5, 5)..
    /// (12, 11) of its sheet, the body's top at (11, 11)..(20, 22), and the
    /// four legs on the quadruped walk.
    #[test]
    fn the_panda_cub_is_its_own_model() {
        let m = panda_baby();
        assert_eq!(m.quads.len(), 9 * 6);
        let head = m.parts.iter().position(|p| p.anim == Anim::Head).unwrap();
        let span = |q: &RawQuad, i: usize| {
            q.uv.iter()
                .map(|u| u[i])
                .fold((f32::MAX, f32::MIN), |(a, b), x| (a.min(x), b.max(x)))
        };
        let rect = |q: &RawQuad| (span(q, 0), span(q, 1));
        assert!(m.quads.iter().any(|q| q.part == head
            && q.facing == Facing::North
            && rect(q) == ((5.0, 12.0), (5.0, 11.0))));
        assert!(m.quads.iter().any(|q| q.part == STATIC_PART
            && q.facing == Facing::Down
            && rect(q) == ((11.0, 20.0), (11.0, 22.0))));
        for a in [
            Anim::QuadHindRight,
            Anim::QuadHindLeft,
            Anim::QuadFrontRight,
            Anim::QuadFrontLeft,
        ] {
            assert_eq!(m.parts.iter().filter(|p| p.anim == a).count(), 1);
        }
    }
}

#[cfg(test)]
mod kitten_tests {
    use super::*;

    /// M332 — `BabyFelineModel`'s layer: ten boxes, the head box's front at
    /// (4, 4)..(9, 8) of its sheet; and the walk vanilla's: the left hind
    /// (+x, +z) and right front (-x, -z) legs share `cos(f)`'s phase, at
    /// amplitude 1.0 (`Anim`'s 1.4 times `amp`).
    #[test]
    fn the_kitten_is_its_own_model_on_vanilla_s_walk() {
        let m = feline_baby();
        assert_eq!(m.quads.len(), 10 * 6);
        let head = m
            .parts
            .iter()
            .position(|p| p.anim == Anim::FelineHead)
            .unwrap();
        let span = |q: &RawQuad, i: usize| {
            q.uv.iter()
                .map(|u| u[i])
                .fold((f32::MAX, f32::MIN), |(a, b), x| (a.min(x), b.max(x)))
        };
        assert!(m.quads.iter().any(|q| q.part == head
            && q.facing == Facing::North
            && (span(q, 0), span(q, 1)) == ((4.0, 9.0), (4.0, 8.0))));
        let leg = |x: f32, z: f32| {
            m.parts
                .iter()
                .find(|p| p.pivot[0] == x && p.pivot[2] == z)
                .unwrap()
        };
        let (lh, rf) = (leg(1.0, 2.5), leg(-1.0, -1.5));
        let (rh, lf) = (leg(-1.0, 2.5), leg(1.0, -1.5));
        // M357 — the feline's own legs (`FelineLeg`, amplitude 1 by itself;
        // its phases are graded in `entities::feline_sit_tests`).
        let is = |p: &Part, hind: bool, left: bool| p.anim == Anim::FelineLeg { hind, left };
        assert!(is(lh, true, true) && is(rf, false, false));
        assert!(is(rh, true, false) && is(lf, false, true));
        for p in [lh, rf, rh, lf] {
            assert_eq!(p.amp, 1.0);
        }
    }
}

#[cfg(test)]
mod baby_bee_tests {
    use super::*;

    /// M333 — `BabyBeeModel` airborne: two antennae and the body (6 faces
    /// each), the stinger, two wings and three leg plates (2 each); the
    /// body's front at (5, 5)..(9, 9) of its sheet; both wings fluttering
    /// from the bone's offset plus their own, tilted 0.2182 with no splay.
    #[test]
    fn the_baby_bee_is_its_own_model() {
        let m = bee_baby();
        assert_eq!(m.quads.len(), 3 * 6 + 6 * 2);
        let span = |q: &RawQuad, i: usize| {
            q.uv.iter()
                .map(|u| u[i])
                .fold((f32::MAX, f32::MIN), |(a, b), x| (a.min(x), b.max(x)))
        };
        assert!(m
            .quads
            .iter()
            .any(|q| q.facing == Facing::North
                && (span(q, 0), span(q, 1)) == ((5.0, 9.0), (5.0, 9.0))));
        let wings: Vec<_> = m
            .parts
            .iter()
            .filter(|p| matches!(p.anim, Anim::BeeWing { .. }))
            .collect();
        assert_eq!(wings.len(), 2);
        // M354 — the wings are the bone's children, at its offset plus their
        // own (the bone at (0, 19.6667, -1.8567)).
        let bone = m
            .parts
            .iter()
            .position(|p| p.anim == Anim::BeeBone)
            .unwrap();
        assert_eq!(m.parts[bone].pivot, [0.0, 19.6667, -1.8567]);
        for w in wings {
            assert_eq!(w.rot, [0.2182, 0.0, 0.0]);
            assert_eq!(w.parent, Some(bone as u16));
            assert!((w.pivot[1] - -0.6667).abs() < 1e-4);
            assert!((w.pivot[2] - 0.8567).abs() < 1e-4);
        }
        assert!(m.quads.iter().all(|q| q.uv.iter().all(|u| u[0] >= 0.0)));
    }
}

#[cfg(test)]
mod foal_tests {
    use super::*;

    /// M334 — `BabyDonkeyModel`: ten boxes, the body's top at (14, 13)..
    /// (22, 27) of its sheet, and the legs' pivots the body's (1, 14, 0)
    /// plus their own offsets (they are its children in vanilla).
    #[test]
    fn the_foal_is_its_own_model() {
        let m = donkey_baby();
        assert_eq!(m.quads.len(), 10 * 6);
        let span = |q: &RawQuad, i: usize| {
            q.uv.iter()
                .map(|u| u[i])
                .fold((f32::MAX, f32::MIN), |(a, b), x| (a.min(x), b.max(x)))
        };
        assert!(m.quads.iter().any(|q| q.part == STATIC_PART
            && q.facing == Facing::Down
            && (span(q, 0), span(q, 1)) == ((14.0, 22.0), (13.0, 27.0))));
        let pivot = |a: Anim| m.parts.iter().find(|p| p.anim == a).unwrap().pivot;
        let near = |p: [f32; 3], q: [f32; 3]| (0..3).all(|i| (p[i] - q[i]).abs() < 1e-5);
        assert!(near(
            pivot(Anim::QuadHindLeft),
            [1.0 + 2.25, 14.0 + 3.5, 5.25]
        ));
        assert!(near(
            pivot(Anim::QuadFrontRight),
            [1.0 - 2.4, 14.0 + 3.5, -5.4]
        ));
    }
}

#[cfg(test)]
mod camel_calf_tests {
    use super::*;

    /// M335 — `BabyCamelModel`: the body, three head boxes, two ears and four
    /// legs (6 faces each) and the tail plate (2); the neck base's front at
    /// (27, 7)..(32, 12) of its sheet; the head under the body; and
    /// `CAMEL_BABY_WALK` on `Walk { 2.0, 2.5 }` with every channel bound.
    #[test]
    fn the_camel_calf_is_its_own_model_on_its_own_walk() {
        let m = camel_baby();
        assert_eq!(m.quads.len(), 10 * 6 + 2);
        let span = |q: &RawQuad, i: usize| {
            q.uv.iter()
                .map(|u| u[i])
                .fold((f32::MAX, f32::MIN), |(a, b), x| (a.min(x), b.max(x)))
        };
        let named = |n: &str| m.parts.iter().position(|p| p.name == n).unwrap();
        assert!(m.quads.iter().any(|q| q.part == named("head")
            && q.facing == Facing::North
            && (span(q, 0), span(q, 1)) == ((27.0, 32.0), (7.0, 12.0))));
        assert_eq!(m.parts[named("head")].parent, Some(named("body") as u16));
        assert_eq!(m.parts[named("body")].parent, Some(named("root") as u16));
        let kf = &m.keyframes[0];
        assert!(std::ptr::eq(kf.def, &crate::anim_defs::CAMEL_BABY_WALK));
        assert!(
            matches!(kf.driver, KfDriver::Walk { speed_f, scale_f } if speed_f == 2.0 && scale_f == 2.5)
        );
        assert_eq!(kf.parts.len(), kf.def.channels.len());
    }
}

#[cfg(test)]
mod rabbit_kit_tests {
    use super::*;

    /// M336 — `BabyRabbitModel`: nine boxes; the head box's front at (4, 4)..
    /// (9, 8) of its sheet; the hind legs turned π under `backlegs`; and
    /// `BABY_RABBIT_HOP` with every channel bound to a named part.
    #[test]
    fn the_rabbit_kit_is_its_own_model_on_its_own_hop() {
        let m = rabbit_baby();
        assert_eq!(m.quads.len(), 9 * 6);
        let named = |n: &str| m.parts.iter().position(|p| p.name == n).unwrap();
        let span = |q: &RawQuad, i: usize| {
            q.uv.iter()
                .map(|u| u[i])
                .fold((f32::MAX, f32::MIN), |(a, b), x| (a.min(x), b.max(x)))
        };
        assert!(m.quads.iter().any(|q| q.part == named("head")
            && q.facing == Facing::North
            && (span(q, 0), span(q, 1)) == ((4.0, 9.0), (4.0, 8.0))));
        for leg in ["left_hind_leg", "right_hind_leg"] {
            let p = &m.parts[named(leg)];
            assert_eq!(p.parent, Some(named("backlegs") as u16));
            assert_eq!(p.rot, [0.0, 3.1416, 0.0]);
        }
        assert_eq!(m.parts[named("head")].anim, Anim::Head);
        let kf = &m.keyframes[0];
        assert!(std::ptr::eq(kf.def, &crate::anim_defs::BABY_RABBIT_HOP));
        assert_eq!(kf.parts.len(), kf.def.channels.len());
    }
}

#[cfg(test)]
mod goat_kid_tests {
    use super::*;

    /// M337 — `BabyGoatModel`: twelve boxes; the head at `setupAnim`'s π/8
    /// (not the layer's 0.4363) on the yaw-only look; the skull's front at
    /// (6, 6)..(10, 10) of its sheet, twice (`HeadMain` repeats it, at the
    /// same place).
    #[test]
    fn the_kid_is_its_own_model() {
        let m = goat_baby();
        assert_eq!(m.quads.len(), 12 * 6);
        let head = m
            .parts
            .iter()
            .position(|p| p.anim == Anim::HeadYaw)
            .unwrap();
        assert!((m.parts[head].rot[0] - PI / 8.0).abs() < 1e-6);
        let span = |q: &RawQuad, i: usize| {
            q.uv.iter()
                .map(|u| u[i])
                .fold((f32::MAX, f32::MIN), |(a, b), x| (a.min(x), b.max(x)))
        };
        let fronts: Vec<_> = m
            .quads
            .iter()
            .filter(|q| {
                q.part == head
                    && q.facing == Facing::North
                    && (span(q, 0), span(q, 1)) == ((6.0, 10.0), (6.0, 10.0))
            })
            .collect();
        assert_eq!(fronts.len(), 2);
        for (a, b) in fronts[0].pos.iter().zip(fronts[1].pos.iter()) {
            assert!((0..3).all(|i| (a[i] - b[i]).abs() < 1e-4));
        }
    }
}

#[cfg(test)]
mod llama_kid_tests {
    use super::*;

    /// M338 — `BabyLlamaModel`: nine boxes (no chests), the head's front at
    /// (4, 4)..(10, 15) of its sheet, and no part shown only when chested.
    #[test]
    fn the_baby_llama_is_its_own_model_without_chests() {
        let m = llama_baby();
        assert_eq!(m.quads.len(), 9 * 6);
        let head = m.parts.iter().position(|p| p.anim == Anim::Head).unwrap();
        let span = |q: &RawQuad, i: usize| {
            q.uv.iter()
                .map(|u| u[i])
                .fold((f32::MAX, f32::MIN), |(a, b), x| (a.min(x), b.max(x)))
        };
        assert!(m.quads.iter().any(|q| q.part == head
            && q.facing == Facing::North
            && (span(q, 0), span(q, 1)) == ((4.0, 10.0), (4.0, 15.0))));
        assert!(m.parts.iter().all(|p| !matches!(p.show, Show::Chested)));
    }
}

#[cfg(test)]
mod equine_foal_rest_tests {
    use super::*;

    fn span(q: &RawQuad, i: usize) -> (f32, f32) {
        q.uv.iter()
            .map(|u| u[i])
            .fold((f32::MAX, f32::MIN), |(a, b), x| (a.min(x), b.max(x)))
    }

    /// The tilt about x of the static north face with this UV rect: its
    /// normal is (0, sin θ, -cos θ) after `rotateZYX(0, 0, θ)`.
    fn north_tilt(m: &Model, rect: ((f32, f32), (f32, f32))) -> f32 {
        let q = m
            .quads
            .iter()
            .find(|q| q.facing == Facing::North && (span(q, 0), span(q, 1)) == rect)
            .expect("the face");
        (-q.normal[2]).clamp(-1.0, 1.0).acos()
    }

    /// M339 — `BabyHorseModel`: ten boxes; the neck at `setupAnim`'s π/6
    /// (not the layer's 0.6109) and the tail at -π/2 + π/6 (not -0.7418).
    #[test]
    fn the_horse_foal_rests_where_setup_anim_puts_it() {
        let m = horse_baby();
        assert_eq!(m.quads.len(), 10 * 6);
        assert!((north_tilt(&m, ((34.0, 38.0), (4.0, 12.0))) - PI / 6.0).abs() < 1e-4);
        assert!((north_tilt(&m, ((32.0, 35.0), (42.0, 45.0))) - PI / 3.0).abs() < 1e-4);
    }

    /// M339 — the donkey foal's tail: -π/12 on the box-less part
    /// (`getTailXRotOffset` -π/4 + π/6) under the box's -0.7418.
    #[test]
    fn the_donkey_foal_s_tail_rests_where_setup_anim_puts_it() {
        let m = donkey_baby();
        let tilt = north_tilt(&m, ((32.0, 35.0), (41.0, 44.0)));
        assert!(
            (tilt - (0.7418 + PI / 12.0)).abs() < 1e-4,
            "tail tilt {tilt}"
        );
    }
}

#[cfg(test)]
mod puppy_tests {
    use super::*;

    /// M341 — `BabyWolfModel`: ten boxes, the head box's front at (5, 17)..
    /// (11, 22) of its sheet, and the tail resting at a wild wolf's
    /// `tailAngle` π/5 (not the layer's -0.5236), wagging about y.
    #[test]
    fn the_puppy_is_its_own_model_with_a_wild_wolf_s_tail() {
        // M342: the bare mesh (the collar copy is `collar_tests`').
        let m = wolf_baby_mesh();
        assert_eq!(m.quads.len(), 10 * 6);
        let head = m.parts.iter().position(|p| p.anim == Anim::Head).unwrap();
        let span = |q: &RawQuad, i: usize| {
            q.uv.iter()
                .map(|u| u[i])
                .fold((f32::MAX, f32::MIN), |(a, b), x| (a.min(x), b.max(x)))
        };
        assert!(m.quads.iter().any(|q| q.part == head
            && q.facing == Facing::North
            && (span(q, 0), span(q, 1)) == ((5.0, 11.0), (17.0, 22.0))));
        let tail = m.parts.iter().find(|p| p.anim == Anim::TailWagY).unwrap();
        assert!((tail.rot[0] - PI / 5.0).abs() < 1e-6);
    }
}

#[cfg(test)]
mod collar_tests {
    use super::*;

    /// M342 — the wolf's and the puppy's collar is their own mesh again on
    /// texture 1, quad for quad, so it is coplanar with them.
    #[test]
    fn the_wolf_s_collar_is_its_own_mesh_again() {
        for (m, bare) in [(wolf(), wolf_with(0.0)), (wolf_baby(), wolf_baby_mesh())] {
            let collar: Vec<_> = m.quads.iter().filter(|q| q.tex == 1).collect();
            assert_eq!(collar.len(), bare.quads.len());
            for (c, b) in collar.iter().zip(&bare.quads) {
                assert_eq!((c.pos, c.uv, c.part), (b.pos, b.uv, b.part));
            }
        }
    }
}

#[cfg(test)]
mod cat_collar_tests {
    use super::*;

    /// M343 — the adult cat is the feline at 0.8 (the ocelot stays at 1),
    /// its collar the same parts grown 0.01; the kitten's collar is the
    /// kitten scaled 1.01 about the feet on parts of its own.
    #[test]
    fn the_cat_is_four_fifths_of_an_ocelot_and_wears_its_collar() {
        let (cat, ocelot) = (cat(), feline());
        assert_eq!((cat.scale, ocelot.scale), (0.8, 1.0));
        assert_eq!(cat.parts.len(), ocelot.parts.len());
        assert_eq!(
            cat.quads.iter().filter(|q| q.tex == 1).count(),
            ocelot.quads.len()
        );
        let kit = cat_baby();
        let bare = feline_baby();
        assert_eq!(kit.parts.len(), 2 * bare.parts.len());
        let collar: Vec<_> = kit.quads.iter().filter(|q| q.tex == 1).collect();
        assert_eq!(collar.len(), bare.quads.len());
        for (c, b) in collar.iter().zip(&bare.quads) {
            for (cp, bp) in c.pos.iter().zip(&b.pos) {
                if b.part == STATIC_PART {
                    assert!((cp[1] - (24.016 + (bp[1] - 24.016) * 1.01)).abs() < 1e-4);
                } else {
                    assert!((cp[0] - bp[0] * 1.01).abs() < 1e-4);
                }
            }
        }
    }
}

#[cfg(test)]
mod cow_variant_tests {
    use super::*;

    fn has_face(m: &Model, u: (f32, f32), v: (f32, f32)) -> bool {
        let span = |q: &RawQuad, i: usize| {
            q.uv.iter()
                .map(|t| t[i])
                .fold((f32::MAX, f32::MIN), |(a, b), x| (a.min(x), b.max(x)))
        };
        m.quads.iter().any(|q| span(q, 0) == u && span(q, 1) == v)
    }

    /// M348 — the base cow's ten boxes; the cold cow's eleven (the coat, and
    /// two child horns for the head's two); the warm cow's twelve (two boxes
    /// a horn). Each new box's north face sits where its `texOffs` puts it.
    #[test]
    fn the_cold_and_warm_cows_are_their_own_models() {
        let (base, cold, warm) = (cow(), cold_cow(), warm_cow());
        assert_eq!(base.quads.len(), 10 * 6);
        assert_eq!(cold.quads.len(), 11 * 6);
        assert_eq!(warm.quads.len(), 12 * 6);
        // The cold coat (20, 32) 12 x 18 x 10, its horns (0, 40) and (0, 32)
        // 2 x 6 x 2, and its muzzle at (9, 33).
        assert!(has_face(&cold, (30.0, 42.0), (42.0, 60.0)));
        assert!(has_face(&cold, (2.0, 4.0), (42.0, 48.0)));
        assert!(has_face(&cold, (2.0, 4.0), (34.0, 40.0)));
        assert!(has_face(&cold, (10.0, 16.0), (34.0, 37.0)));
        assert!(!has_face(&cold, (23.0, 24.0), (1.0, 4.0)));
        // The warm horns (27, 0) 4 x 2 x 2 and tips (39, 0) 2 x 2 x 2.
        assert!(has_face(&warm, (29.0, 33.0), (2.0, 4.0)));
        assert!(has_face(&warm, (41.0, 43.0), (2.0, 4.0)));
        assert!(has_face(&base, (23.0, 24.0), (1.0, 4.0)));
    }
}

#[cfg(test)]
mod cold_pig_chicken_tests {
    use super::*;

    fn has_face(m: &Model, u: (f32, f32), v: (f32, f32)) -> bool {
        let span = |q: &RawQuad, i: usize| {
            q.uv.iter()
                .map(|t| t[i])
                .fold((f32::MAX, f32::MIN), |(a, b), x| (a.min(x), b.max(x)))
        };
        m.quads.iter().any(|q| span(q, 0) == u && span(q, 1) == v)
    }

    /// M349 — the cold pig is the pig plus its coat (28, 32) 10 x 16 x 8;
    /// the cold chicken is the chicken plus its crest (44, 0) 6 x 3 x 4 and
    /// its tail plane (38, 9) 0 x 3 x 5, which has two faces with area.
    #[test]
    fn the_cold_pig_and_chicken_are_their_own_models() {
        let (pig, cold_pig) = (pig(), cold_pig());
        assert_eq!(cold_pig.quads.len(), pig.quads.len() + 6);
        assert_eq!(cold_pig.parts.len(), pig.parts.len());
        assert!(has_face(&cold_pig, (36.0, 46.0), (40.0, 56.0)));
        assert!(!has_face(&pig, (36.0, 46.0), (40.0, 56.0)));
        let (chicken, cold) = (chicken(), cold_chicken());
        assert_eq!(cold.quads.len(), chicken.quads.len() + 6 + 2);
        assert_eq!(cold.parts.len(), chicken.parts.len());
        assert!(has_face(&cold, (48.0, 54.0), (4.0, 7.0)));
        assert!(has_face(&cold, (38.0, 43.0), (14.0, 17.0)));
        assert!(has_face(&cold, (43.0, 48.0), (14.0, 17.0)));
        // The beak (14, 0) 4 x 2 x 2 survives the head's replacement.
        assert!(has_face(&cold, (16.0, 20.0), (2.0, 4.0)));
    }
}

#[cfg(test)]
mod coral_nautilus_tests {
    use super::*;

    fn has_face(m: &Model, u: (f32, f32), v: (f32, f32)) -> bool {
        let span = |q: &RawQuad, i: usize| {
            q.uv.iter()
                .map(|t| t[i])
                .fold((f32::MAX, f32::MIN), |(a, b), x| (a.min(x), b.max(x)))
        };
        m.quads.iter().any(|q| span(q, 0) == u && span(q, 1) == v)
    }

    /// M350 — the coral mesh is the nautilus plus one part holding eight
    /// zero-thickness planes (two faces each), shown only unarmoured; its
    /// armour and saddle shells have its part list and the base shells' boxes.
    #[test]
    fn the_coral_nautilus_is_the_nautilus_plus_its_corals() {
        let (base, coral) = (nautilus(), nautilus_coral());
        assert_eq!(coral.parts.len(), base.parts.len() + 1);
        assert_eq!(coral.quads.len(), base.quads.len() + 8 * 2);
        let c = coral.parts.len() - 1;
        assert_eq!(coral.parts[c].show, Show::Unarmoured);
        assert!(coral.quads[base.quads.len()..].iter().all(|q| q.part == c));
        for (u, v) in [
            ((0.0, 6.0), (85.0, 93.0)),
            ((0.0, 6.0), (94.0, 102.0)),
            ((0.0, 5.0), (102.0, 112.0)),
            ((0.0, 4.0), (112.0, 122.0)),
            ((0.0, 6.0), (112.0, 122.0)),
        ] {
            assert!(has_face(&coral, u, v), "{u:?} {v:?}");
        }
        for (shell, plain) in [
            (nautilus_coral_armor(), nautilus_armor()),
            (nautilus_coral_saddle(), nautilus_saddle()),
        ] {
            assert_eq!(shell.parts.len(), coral.parts.len());
            assert_eq!(shell.quads.len(), plain.quads.len());
        }
    }
}

#[cfg(test)]
mod feline_rest_tests {
    use super::*;

    /// M357 — the adult feline's second tail segment rests at 1.7278761
    /// (`AdultFelineModel.setupAnim` sets it every frame); until M357 it was
    /// the layer's 0 and hung its eight pixels straight down past the feet.
    #[test]
    fn the_cats_tail_tip_rests_where_setup_anim_puts_it() {
        for m in [feline(), cat()] {
            let tail2 = m
                .parts
                .iter()
                .find(|p| p.anim == Anim::FelineTail2)
                .unwrap();
            assert_eq!(tail2.rot, [1.727_876_1, 0.0, 0.0]);
        }
    }
}

#[cfg(test)]
mod equine_head_tests {
    use super::*;

    /// M359 — the head group is one part (created first, so index 1 in
    /// every equine builder); its boxes (neck, head, mane, mouth, ears) and
    /// the saddle's bridle and the reins ride it; the armour shells keep the
    /// body's part list.
    #[test]
    fn the_horses_head_group_is_a_part_the_bridle_shares() {
        let base = equine_with(false, 0.0);
        let head = base
            .parts
            .iter()
            .position(|p| p.anim == Anim::EquineHead)
            .unwrap();
        assert_eq!(head, 1);
        assert_eq!(base.parts[head].rot, [PI / 6.0, 0.0, 0.0]);
        assert_eq!(base.quads.iter().filter(|q| q.part == head).count(), 6 * 6);
        let (saddle, reins) = equine_saddle(false, 1.0);
        assert_eq!(saddle.parts.len(), base.parts.len());
        // The bridle: two bit pieces, the head strap and the mouth wrap.
        let extra = &saddle.quads[base.quads.len()..];
        assert_eq!(extra.iter().filter(|q| q.part == head).count(), 4 * 6);
        assert!(reins.quads.iter().all(|q| q.part == head));
        assert_eq!(horse_armor().parts.len(), base.parts.len());
        let donkey = equine_with(true, 0.0);
        assert_eq!(
            donkey.quads.iter().filter(|q| q.part == head).count(),
            6 * 6
        );
    }
}

#[cfg(test)]
mod boat_patch_tests {
    use super::*;

    /// M363 — the water patch is one box (six faces) lying flat in the hull:
    /// its 28 px run along the boat's length and 16 across, 3 px thick, at
    /// the hull's waterline (model y 21..24 under the root's 18 px lift).
    #[test]
    fn the_boat_water_patch_lies_flat_in_the_hull() {
        let m = boat_water_patch();
        assert_eq!(m.quads.len(), 6);
        let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
        for q in &m.quads {
            for p in &q.pos {
                for k in 0..3 {
                    lo[k] = lo[k].min(p[k]);
                    hi[k] = hi[k].max(p[k]);
                }
            }
        }
        let span = [hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]];
        let mut horizontal = [span[0], span[2]];
        horizontal.sort_by(f32::total_cmp);
        assert!((horizontal[0] - 16.0).abs() < 1e-3 && (horizontal[1] - 28.0).abs() < 1e-3);
        assert!((span[1] - 3.0).abs() < 1e-3, "{span:?}");
    }
}

#[cfg(test)]
mod villager_scale_tests {
    use super::*;

    /// M360 — the villager-like layers at `villagerLikeScale`; their babies
    /// (`BabyVillagerModel`'s own layer) and the zombie villager are not.
    #[test]
    fn the_villager_like_mobs_are_drawn_at_fifteen_sixteenths() {
        for m in [villager(), witch(), illager_crossed(), illager_arms()] {
            assert_eq!(m.scale, 0.9375);
        }
        let kinds = [
            EntityModelKind::Villager,
            EntityModelKind::WanderingTrader,
            EntityModelKind::Witch,
            EntityModelKind::Pillager,
            EntityModelKind::Vindicator,
            EntityModelKind::Evoker,
            EntityModelKind::Illusioner,
        ];
        for def in MOBS.iter().filter(|d| kinds.contains(&d.kind)) {
            assert_eq!((def.build)().scale, 0.9375, "{:?}", def.kind);
        }
        assert_eq!(MOBS.iter().filter(|d| kinds.contains(&d.kind)).count(), 7);
        for def in MOBS.iter().filter(|d| {
            matches!(
                d.kind,
                EntityModelKind::VillagerBaby | EntityModelKind::ZombieVillager
            )
        }) {
            assert_eq!((def.build)().scale, 1.0, "{:?}", def.kind);
        }
    }
}

#[cfg(test)]
mod rabbit_head_tests {
    use super::*;

    /// M364 — both rabbits' heads look (`RabbitModel.setupAnim`): no rest
    /// tilt of their own, the look assigned, the ears riding on them.
    #[test]
    fn the_rabbit_heads_look() {
        for m in [rabbit(), rabbit_baby()] {
            let head = m
                .parts
                .iter()
                .position(|p| p.name == "head")
                .expect("a head part");
            assert_eq!(m.parts[head].rot, [0.0; 3]);
            assert_eq!(m.parts[head].anim, Anim::Head);
            let ears = m
                .parts
                .iter()
                .filter(|p| p.parent == Some(head as u16) && p.name.ends_with("ear"))
                .count();
            assert_eq!(ears, 2);
        }
    }
}

#[cfg(test)]
mod turtle_model_tests {
    use super::*;

    /// M365 — both turtles' four legs are paddling parts (two hind, two
    /// left); the adult's egg belly is its own part, shown only with an egg.
    #[test]
    fn the_turtles_paddle_and_the_belly_waits_for_an_egg() {
        for m in [turtle(), turtle_baby()] {
            let legs: Vec<_> = m
                .parts
                .iter()
                .filter_map(|p| match p.anim {
                    Anim::TurtleLeg { hind, left } => Some((hind, left)),
                    _ => Option::None,
                })
                .collect();
            assert_eq!(legs.len(), 4);
            assert_eq!(legs.iter().filter(|l| l.0).count(), 2);
            assert_eq!(legs.iter().filter(|l| l.1).count(), 2);
        }
        let m = turtle();
        let eggs: Vec<usize> = (0..m.parts.len())
            .filter(|&i| m.parts[i].show == Show::TurtleEgg)
            .collect();
        assert_eq!(eggs.len(), 1);
        assert_eq!(m.quads.iter().filter(|q| q.part == eggs[0]).count(), 6);
        assert!(turtle_baby()
            .parts
            .iter()
            .all(|p| p.show != Show::TurtleEgg));
    }
}

#[cfg(test)]
mod strider_model_tests {
    use super::*;

    /// M366 — both striders' bodies, legs and bristles are parts: the
    /// adult's body at `customAnimations`' y 2 with six bristles on it (two
    /// of each `animateBristle` order), the baby's at 17.25 with three.
    #[test]
    fn the_striders_parts_move() {
        for (m, adult, body_y, bristles) in [
            (strider(), true, 2.0, 6usize),
            (strider_baby(), false, 17.25, 3),
        ] {
            let body = m
                .parts
                .iter()
                .position(|p| p.anim == Anim::StriderBody { adult })
                .expect("a body part");
            assert_eq!(m.parts[body].pivot[1], body_y);
            let orders: Vec<u8> = m
                .parts
                .iter()
                .filter(|p| p.parent == Some(body as u16))
                .filter_map(|p| match p.anim {
                    Anim::StriderBristle { order, baby } if baby != adult => Some(order),
                    _ => Option::None,
                })
                .collect();
            assert_eq!(orders.len(), bristles);
            for o in 0..3u8 {
                assert_eq!(orders.iter().filter(|&&x| x == o).count(), bristles / 3);
            }
            let legs = m
                .parts
                .iter()
                .filter(|p| matches!(p.anim, Anim::StriderLeg { .. }))
                .count();
            assert_eq!(legs, 2);
        }
        assert_eq!(strider_saddle().parts.len(), strider().parts.len());
    }
}

#[cfg(test)]
mod hoglin_model_tests {
    use super::*;

    /// M367 — both hoglins: the head rests at `setupAnim`'s 0.87266463 on
    /// its own animation, two flapping ears ride it (resting at ∓2π/9), and
    /// four legs walk in diagonal pairs.
    #[test]
    fn the_hoglins_parts_move_as_vanilla_moves_them() {
        for (m, baby) in [(hoglin(), false), (hoglin_baby(), true)] {
            let head = m
                .parts
                .iter()
                .position(|p| p.anim == Anim::HoglinHead { baby })
                .expect("a head");
            assert_eq!(m.parts[head].rot, [0.87266463, 0.0, 0.0]);
            let ears: Vec<f32> = m
                .parts
                .iter()
                .filter(|p| p.parent == Some(head as u16))
                .filter(|p| matches!(p.anim, Anim::HoglinEar { .. }))
                .map(|p| p.rot[2])
                .collect();
            assert_eq!(ears, vec![-PI * 2.0 / 9.0, PI * 2.0 / 9.0]);
            let legs = m
                .parts
                .iter()
                .filter(|p| matches!(p.anim, Anim::HoglinLeg { .. }))
                .count();
            assert_eq!(legs, 4);
        }
    }
}

#[cfg(test)]
mod snow_golem_tests {
    use super::*;

    /// M368 — the snow golem's arms rest at z 0 on their own animation, the
    /// upper body is a turning part.
    #[test]
    fn the_snow_golems_arms_and_upper_body_turn() {
        let m = snow_golem();
        let arms: Vec<[f32; 3]> = m
            .parts
            .iter()
            .filter(|p| matches!(p.anim, Anim::SnowGolemArm { .. }))
            .map(|p| p.pivot)
            .collect();
        assert_eq!(arms, vec![[5.0, 6.0, 0.0], [-5.0, 6.0, 0.0]]);
        assert_eq!(
            m.parts
                .iter()
                .filter(|p| p.anim == Anim::SnowGolemBody)
                .count(),
            1
        );
    }
}

#[cfg(test)]
mod allay_pose_tests {
    use super::*;

    /// M369 — the allay's body and both arms carry `setupAnim`'s motion.
    #[test]
    fn the_allays_body_and_arms_move() {
        let m = allay();
        let anim = |name: &str| m.parts.iter().find(|p| p.name == name).map(|p| p.anim);
        assert_eq!(anim("body"), Some(Anim::AllayBody));
        assert_eq!(anim("right_arm"), Some(Anim::AllayArm { left: false }));
        assert_eq!(anim("left_arm"), Some(Anim::AllayArm { left: true }));
    }
}

#[cfg(test)]
mod guardian_pose_tests {
    use super::*;

    /// M370 — the guardian (and the elder) is one looking `head` part at
    /// the origin carrying twelve spikes and a three-segment tail chain.
    #[test]
    fn the_guardian_looks_wobbles_and_sways() {
        for m in [guardian(), elder_guardian()] {
            let head = m.parts.iter().position(|p| p.name == "head").unwrap();
            assert_eq!(m.parts[head].anim, Anim::Head);
            assert_eq!(m.parts[head].pivot, [0.0; 3]);
            let spikes = m
                .parts
                .iter()
                .filter(|p| {
                    matches!(p.anim, Anim::GuardianSpike { .. }) && p.parent == Some(head as u16)
                })
                .count();
            assert_eq!(spikes, 12);
            let tail: Vec<usize> = (0..m.parts.len())
                .filter(|&i| matches!(m.parts[i].anim, Anim::GuardianTail { .. }))
                .collect();
            assert_eq!(tail.len(), 3);
            assert_eq!(m.parts[tail[0]].parent, Some(head as u16));
            assert_eq!(m.parts[tail[1]].parent, Some(tail[0] as u16));
            assert_eq!(m.parts[tail[2]].parent, Some(tail[1] as u16));
        }
    }
}

#[cfg(test)]
mod dolphin_pose_tests {
    use super::*;

    /// M371 — both dolphins: one looking `body` part carries the head, the
    /// fins and the tail chain; the adult's tail rests at −0.1047.
    #[test]
    fn the_dolphins_body_looks_and_carries_the_rest() {
        for (m, rest) in [(dolphin(), -0.10471976_f32), (dolphin_baby(), 0.0)] {
            let body = m.parts.iter().position(|p| p.name == "body").unwrap();
            assert_eq!(m.parts[body].anim, Anim::DolphinBody);
            let tail = m
                .parts
                .iter()
                .position(|p| p.anim == Anim::DolphinTail { k: 0.1, rest })
                .unwrap();
            assert_eq!(m.parts[tail].parent, Some(body as u16));
            assert_eq!(m.parts[tail].rot[0], rest);
            assert!(m.quads.iter().all(|q| q.part != STATIC_PART));
        }
    }
}

#[cfg(test)]
mod nautilus_body_tests {
    use super::*;

    /// M372 — every nautilus (and its shells) has the looking soft body at
    /// (0, 20.5, 6.3) with both mouths on it, at the pivots the old absolute
    /// ones were (body + (0, −2.51, 7) and (0, 1.49, 7)).
    #[test]
    fn the_nautilus_body_carries_its_beak() {
        for m in [
            nautilus(),
            nautilus_coral(),
            nautilus_armor(),
            nautilus_saddle(),
        ] {
            let body = m.parts.iter().position(|p| p.name == "body").unwrap();
            assert_eq!(m.parts[body].anim, Anim::NautilusBody);
            assert_eq!(m.parts[body].pivot, [0.0, 20.5, 6.3]);
            for (name, y) in [("upper_mouth", -2.51), ("lower_mouth", 1.49)] {
                let p = m.parts.iter().find(|p| p.name == name).unwrap();
                assert_eq!(p.parent, Some(body as u16));
                assert_eq!(p.pivot, [0.0, y, 7.0]);
            }
        }
    }
}

#[cfg(test)]
mod happy_ghast_tentacle_tests {
    use super::*;

    /// M373 — the happy ghast's nine tentacles swing on their own parts,
    /// shared by its base and ropes meshes (two boxes each); the baby has
    /// the nine too; the harness keeps the adult's part list.
    #[test]
    fn the_happy_ghasts_tentacles_swing() {
        let m = happy_ghast();
        let legs: Vec<usize> = (0..m.parts.len())
            .filter(|&i| matches!(m.parts[i].anim, Anim::GhastTentacle { .. }))
            .collect();
        assert_eq!(legs.len(), 9);
        for &l in &legs {
            assert_eq!(m.quads.iter().filter(|q| q.part == l).count(), 12);
        }
        // M391 — under the body part at (0, 16, 0), as vanilla's are.
        assert_eq!(m.parts[legs[0]].pivot, [-3.75, 7.0, -5.0]);
        let body = m
            .parts
            .iter()
            .position(|p| p.anim == Anim::HappyGhastBody)
            .unwrap();
        assert_eq!(m.parts[body].pivot, [0.0, 16.0, 0.0]);
        assert!(legs.iter().all(|&l| m.parts[l].parent == Some(body as u16)));
        // The body boxes (base and ropes) ride the body part; the harness's
        // cube and goggles stay on the static part, which the squeeze
        // leaves alone.
        assert_eq!(m.quads.iter().filter(|q| q.part == body).count(), 12);
        assert!(happy_ghast_harness(false)
            .quads
            .iter()
            .all(|q| q.part == STATIC_PART));
        let baby = happy_ghast_baby();
        assert_eq!(
            baby.parts
                .iter()
                .filter(|p| matches!(p.anim, Anim::GhastTentacle { .. }))
                .count(),
            9
        );
        assert_eq!(happy_ghast_harness(false).parts.len(), m.parts.len());
        assert_eq!(happy_ghast_harness(true).parts.len(), m.parts.len());
    }
}

#[cfg(test)]
mod warden_pose_tests {
    use super::*;

    /// M374 — the warden's body, head, arms and legs carry its own gait.
    #[test]
    fn the_warden_walks_on_its_own_gait() {
        let m = warden();
        let anim = |name: &str| m.parts.iter().find(|p| p.name == name).map(|p| p.anim);
        assert_eq!(anim("body"), Some(Anim::WardenBody));
        assert_eq!(anim("head"), Some(Anim::WardenHead));
        assert_eq!(anim("left_arm"), Some(Anim::WardenArm { left: true }));
        assert_eq!(anim("right_leg"), Some(Anim::WardenLeg { left: false }));
    }
}

#[cfg(test)]
mod ravager_pose_tests {
    use super::*;

    /// M375 — the ravager's mouth rests at π·0.01 on the head, and its four
    /// legs swing at 0.4·speed (the quadruped's 1.4 times 0.4 / 1.4).
    #[test]
    fn the_ravager_rests_its_mouth_open_and_walks_at_0_4() {
        let m = ravager();
        let head = m.parts.iter().position(|p| p.anim == Anim::Head).unwrap();
        let mouth = m.parts.iter().find(|p| p.name == "mouth").unwrap();
        assert_eq!(mouth.parent, Some(head as u16));
        assert_eq!(mouth.rot, [PI * 0.01, 0.0, 0.0]);
        let legs: Vec<f32> = m
            .parts
            .iter()
            .filter(|p| {
                matches!(
                    p.anim,
                    Anim::QuadHindRight
                        | Anim::QuadHindLeft
                        | Anim::QuadFrontRight
                        | Anim::QuadFrontLeft
                )
            })
            .map(|p| p.amp)
            .collect();
        assert_eq!(legs.len(), 4);
        assert!(legs.iter().all(|&a| (1.4 * a - 0.4).abs() < 1e-6));
    }
}

#[cfg(test)]
mod wither_breath_tests {
    use super::*;

    /// M376 — the wither (and its armour shell) breathes on its ribcage and
    /// tail parts; only the centre head looks.
    #[test]
    fn the_wither_breathes_and_its_side_heads_rest() {
        for m in [wither(), wither_with(0.5)] {
            let n = |a: Anim| m.parts.iter().filter(|p| p.anim == a).count();
            assert_eq!(n(Anim::WitherRibcage), 1);
            assert_eq!(n(Anim::WitherTail), 1);
            assert_eq!(n(Anim::Head), 1);
        }
        assert_eq!(wither().parts.len(), wither_with(0.5).parts.len());
    }
}

#[cfg(test)]
mod small_pose_tests {
    use super::*;

    /// M377 — the breeze's and the airborne bat's heads do not look; the
    /// camel's (both) and the armadillo's look within their clamps; the
    /// bee's two antennae and the tadpole's tail are moving parts.
    #[test]
    fn the_small_rest_pose_fixes_hold() {
        let anims = |m: &Model| m.parts.iter().map(|p| p.anim).collect::<Vec<_>>();
        for m in [breeze(), bat()] {
            assert!(!anims(&m).contains(&Anim::Head));
        }
        for m in [camel(), camel_baby()] {
            assert!(anims(&m).contains(&Anim::CamelHead));
        }
        assert!(anims(&armadillo()).contains(&Anim::ArmadilloHead));
        assert_eq!(
            anims(&bee())
                .iter()
                .filter(|a| **a == Anim::BeeAntenna)
                .count(),
            2
        );
        assert!(anims(&tadpole()).contains(&Anim::FishTail {
            amp: 0.25,
            speed: 0.3
        }));
    }
}

#[cfg(test)]
mod parrot_pose_tests {
    use super::*;

    /// M378 — the parrot's wings rest splayed ∓0.0873 on their own parts,
    /// and its legs and tail are walking parts.
    #[test]
    fn the_parrot_splays_its_wings_and_walks() {
        let m = parrot();
        let wings: Vec<f32> = m
            .parts
            .iter()
            .filter(|p| p.anim == Anim::ParrotSit && p.rot[1] == -PI)
            .map(|p| p.rot[2])
            .collect();
        assert_eq!(wings, vec![-0.0873, 0.0873]);
        let n = |f: &dyn Fn(Anim) -> bool| m.parts.iter().filter(|p| f(p.anim)).count();
        assert_eq!(n(&|a| matches!(a, Anim::ParrotLeg { .. })), 2);
        assert_eq!(n(&|a| a == Anim::ParrotTail), 1);
        assert_eq!(n(&|a| a == Anim::ParrotHead), 1);
        assert!(m.quads.iter().all(|q| q.part != STATIC_PART));
    }
}

#[cfg(test)]
mod witch_nose_tests {
    use super::*;

    /// M379 — the witch's nose is a part on her head carrying the mole; the
    /// villager's is still a box on the head (no extra part).
    #[test]
    fn the_witchs_nose_wobbles_and_the_villagers_does_not() {
        let w = witch();
        let nose = w
            .parts
            .iter()
            .position(|p| p.anim == Anim::WitchNose)
            .unwrap();
        assert_eq!(w.parts[nose].pivot, [0.0, -2.0, 0.0]);
        assert_eq!(w.quads.iter().filter(|q| q.part == nose).count(), 12);
        assert!(villager().parts.iter().all(|p| p.anim != Anim::WitchNose));
    }
}

#[cfg(test)]
mod piglin_creeper_tests {
    use super::*;

    /// M380 — every piglin (and its baby) has two flapping ear parts on its
    /// head; the non-zombified adult's and baby's arms bob; the
    /// creeper's right hind leg swings on the left's phase.
    #[test]
    fn piglin_ears_flap_arms_bob_and_creeper_legs_swap() {
        for m in [piglin_normal(), piglin_zombified(), piglin_baby()] {
            // M390 — a `PiglinModel`'s head poses (`PiglinHead`); the
            // zombified piglin's only looks.
            let head = m
                .parts
                .iter()
                .position(|p| matches!(p.anim, Anim::Head | Anim::PiglinHead))
                .unwrap();
            let ears = m
                .parts
                .iter()
                .filter(|p| {
                    matches!(p.anim, Anim::PiglinEar { .. }) && p.parent == Some(head as u16)
                })
                .count();
            assert_eq!(ears, 2);
        }
        for m in [piglin_normal(), piglin_baby()] {
            assert!(m
                .parts
                .iter()
                .any(|p| p.anim == Anim::PiglinArm { left: false }));
            assert!(m.parts.iter().all(|p| p.anim != Anim::ArmRight));
        }
        let c = creeper();
        let right_hind = c
            .parts
            .iter()
            .find(|p| p.pivot == [-2.0, 18.0, 4.0])
            .unwrap();
        assert_eq!(right_hind.anim, Anim::QuadHindLeft);
    }
}

#[cfg(test)]
mod riding_pose_kind_tests {
    use super::*;

    /// M381 — the humanoids and illagers sit when riding; the villager, the
    /// witch, a horse and a pig do not.
    #[test]
    fn humanoids_and_illagers_sit_when_riding() {
        for k in [
            EntityModelKind::Player,
            EntityModelKind::Zombie,
            EntityModelKind::Skeleton,
            EntityModelKind::Piglin,
            EntityModelKind::Pillager,
        ] {
            assert!(sits_when_riding(k), "{k:?}");
        }
        for k in [
            EntityModelKind::Villager,
            EntityModelKind::Witch,
            EntityModelKind::Horse,
            EntityModelKind::Pig,
        ] {
            assert!(!sits_when_riding(k), "{k:?}");
        }
    }
}

#[cfg(test)]
mod drowned_mesh_tests {
    use super::*;

    /// M382 — the drowned's left arm and leg sample their own (32, 48) and
    /// (16, 48) rects, in the base and the outer layer; the zombie's still
    /// mirror the right limbs; the drowned's arms are drowned arms.
    #[test]
    fn the_drowned_has_true_left_limbs() {
        let uv_min = |m: &Model, part: usize, tex: usize| {
            m.quads
                .iter()
                .filter(|q| q.part == part && q.tex == tex)
                .flat_map(|q| q.uv.iter().copied())
                .fold([f32::MAX; 2], |a, u| [a[0].min(u[0]), a[1].min(u[1])])
        };
        let d = drowned();
        let arm_l = d.parts.iter().position(|p| p.name == "left_arm").unwrap();
        let leg_l = d
            .parts
            .iter()
            .position(|p| p.anim == Anim::LegLeft)
            .unwrap();
        for tex in [0, 1] {
            assert_eq!(uv_min(&d, arm_l, tex), [32.0, 48.0], "arm tex {tex}");
            assert_eq!(uv_min(&d, leg_l, tex), [16.0, 48.0], "leg tex {tex}");
        }
        assert_eq!(d.parts[arm_l].anim, Anim::DrownedArmLeft);
        let z = zombie();
        let zl = z.parts.iter().position(|p| p.name == "left_arm").unwrap();
        assert_eq!(uv_min(&z, zl, 0), [40.0, 16.0]);
        assert_eq!(z.parts[zl].anim, Anim::UndeadArmLeft);
    }
}

#[cfg(test)]
mod skeleton_clothing_tests {
    use super::*;

    /// M383 — the stray's and the bogged's clothing is the humanoid mesh at
    /// 0.25 / 0.2: 4-wide arms and legs (the legs on their own ±1.9 parts),
    /// the hat at g + 0.5. The plain skeleton has none.
    #[test]
    fn the_stray_and_bogged_wear_humanoid_clothing() {
        for (m, g) in [(stray(), 0.25_f32), (bogged(), 0.2)] {
            let cloth: Vec<&RawQuad> = m.quads.iter().filter(|q| q.tex == 1).collect();
            let legs: Vec<usize> = (0..m.parts.len())
                .filter(|&i| {
                    m.parts[i].pivot == [-1.9, 12.0, 0.0] || m.parts[i].pivot == [1.9, 12.0, 0.0]
                })
                .collect();
            assert_eq!(legs.len(), 2);
            for &l in &legs {
                let xs = cloth
                    .iter()
                    .filter(|q| q.part == l)
                    .flat_map(|q| q.pos.iter().map(|p| p[0]));
                let (lo, hi) = xs.fold((f32::MAX, f32::MIN), |(a, b), x| (a.min(x), b.max(x)));
                assert!(
                    (hi - lo - (4.0 + 2.0 * g)).abs() < 1e-4,
                    "leg width {}",
                    hi - lo
                );
            }
            let arm_r = m.parts.iter().position(|p| p.name == "right_arm").unwrap();
            let xs = cloth
                .iter()
                .filter(|q| q.part == arm_r)
                .flat_map(|q| q.pos.iter().map(|p| p[0]));
            let (lo, hi) = xs.fold((f32::MAX, f32::MIN), |(a, b), x| (a.min(x), b.max(x)));
            assert!(
                (hi - lo - (4.0 + 2.0 * g)).abs() < 1e-4,
                "arm width {}",
                hi - lo
            );
        }
        assert!(skeleton().quads.iter().all(|q| q.tex == 0));
    }
}

#[cfg(test)]
mod parched_mesh_tests {
    use super::*;

    /// M384 — the parched is its own dual-body layer: thirteen boxes, arms
    /// on ±5.5 pivots with 3-wide sleeves at (42, 33) / (40, 48), a belt at
    /// (28, 0), a head overlay at (0, 32).
    #[test]
    fn the_parched_is_its_own_dual_body_layer() {
        let m = parched();
        assert_eq!(m.quads.len(), 13 * 6);
        let arm_r = m.parts.iter().find(|p| p.name == "right_arm").unwrap();
        let arm_l = m.parts.iter().find(|p| p.name == "left_arm").unwrap();
        assert_eq!((arm_r.pivot[0], arm_l.pivot[0]), (-5.5, 5.5));
        let def = MOBS
            .iter()
            .find(|d| d.kind == EntityModelKind::Parched)
            .unwrap();
        assert_eq!((def.build)().quads.len(), m.quads.len());
        let uv_min = |q: &RawQuad| {
            q.uv.iter()
                .fold([f32::MAX; 2], |a, u| [a[0].min(u[0]), a[1].min(u[1])])
        };
        // Each box's faces lie inside its box-UV rect: 2·(w + d) wide and
        // d + h tall from its texture offset (no face starts at the offset
        // itself).
        for (at, w, h, d) in [
            ([42.0, 33.0], 3.0, 12.0, 3.0),
            ([40.0, 48.0], 3.0, 12.0, 3.0),
            ([28.0, 0.0], 8.0, 1.0, 4.0),
            ([0.0, 32.0], 8.0, 8.0, 8.0),
        ] {
            let inside = |q: &RawQuad| {
                let u = uv_min(q);
                u[0] >= at[0]
                    && u[1] >= at[1]
                    && u[0] < at[0] + 2.0 * (w + d)
                    && u[1] < at[1] + d + h
            };
            assert_eq!(
                m.quads.iter().filter(|q| inside(q)).count(),
                6,
                "the box at {at:?}"
            );
        }
    }
}

#[cfg(test)]
mod piglin_mesh_tests {
    use super::*;

    /// M385 — the piglin, the brute's layer and the zombified piglin are the
    /// player mesh: the left arm and leg at their own (32, 48) / (16, 48)
    /// rects, and 0.25 sleeves, pants and jacket (seven boxes the humanoid
    /// mesh did not have, or had mirrored).
    #[test]
    fn the_piglin_family_wears_the_player_mesh() {
        let uv_min = |q: &RawQuad| {
            q.uv.iter()
                .fold([f32::MAX; 2], |a, u| [a[0].min(u[0]), a[1].min(u[1])])
        };
        for m in [piglin_normal(), piglin_zombified()] {
            let in_rect = |at: [f32; 2], w: f32, h: f32, d: f32| {
                m.quads
                    .iter()
                    .filter(|q| {
                        let u = uv_min(q);
                        u[0] >= at[0]
                            && u[1] >= at[1]
                            && u[0] < at[0] + 2.0 * (w + d)
                            && u[1] < at[1] + d + h
                    })
                    .count()
            };
            for at in [
                [32.0, 48.0],
                [16.0, 48.0],
                [48.0, 48.0],
                [0.0, 48.0],
                [40.0, 32.0],
                [0.0, 32.0],
            ] {
                assert_eq!(in_rect(at, 4.0, 12.0, 4.0), 6, "the box at {at:?}");
            }
            assert_eq!(in_rect([16.0, 32.0], 8.0, 12.0, 4.0), 6, "the jacket");
        }
    }
}

#[cfg(test)]
mod dragon_head_tests {
    use super::*;

    /// M386 — the ender dragon's head does not look.
    #[test]
    fn the_dragon_head_does_not_look() {
        assert!(ender_dragon().parts.iter().all(|p| p.anim != Anim::Head));
    }
}

#[cfg(test)]
mod axolotl_pose_tests {
    use super::*;

    /// M387 — the axolotl is vanilla's tree (the gills on the head, the
    /// legs and tail on the body), and its poses: swimming tucks the legs
    /// back (π/2 roll), lying still on the ground splays the left legs and
    /// mirrors them right, crawling does not mirror, playing dead rolls the
    /// body 0.35, and the look turns the body.
    #[test]
    fn the_axolotl_blends_its_poses() {
        let m = axolotl();
        let idx = |n: &str| m.parts.iter().position(|p| p.name == n).unwrap();
        assert_eq!(m.parts[idx("top_gills")].parent, Some(idx("head") as u16));
        assert_eq!(m.parts[idx("tail")].parent, Some(idx("body") as u16));
        let (swim, _) = axolotl_pose(10.0, 0.0, 0.0, true, true, false);
        assert!((swim[8][2] - std::f32::consts::FRAC_PI_2).abs() < 1e-6);
        assert!((swim[7][2] + swim[8][2]).abs() < 1e-6, "mirrored in water");
        let (lay, _) = axolotl_pose(10.0, 0.0, 0.0, false, false, false);
        assert!((lay[6][0] - 1.1).abs() < 1e-6 && (lay[5][1] + lay[6][1]).abs() < 1e-6);
        let (crawl, _) = axolotl_pose(10.0, 0.0, 0.0, false, true, false);
        let cs = (10.0f32 * 0.11).cos();
        assert!(
            (crawl[5][1] - (-1.0 - (cs * cs - 2.0 * cs) / 5.0)).abs() < 1e-5,
            "no mirror"
        );
        let (dead, _) = axolotl_pose(0.0, 0.0, 0.4, false, false, true);
        assert!((dead[0][2] - 0.35).abs() < 1e-6 && (dead[0][1] - 0.4).abs() < 1e-6);
    }
}

#[cfg(test)]
mod dragon_pose_tests {
    use super::*;

    /// M388 — the dragon is vanilla's tree (every part under the root, the
    /// jaw on the head, wing tips on wings, feet on leg tips), its boxes
    /// all kept; its pose lifts the root 2 blocks less the bounce and moves
    /// it 48 px forward, chains the neck so the head sits at its end, and
    /// turns the tail π.
    #[test]
    fn the_dragon_flies_on_its_procedural_rig() {
        let m = ender_dragon();
        assert_eq!(m.parts[1].name, "root");
        assert!(m.parts.iter().skip(2).all(|p| p.parent.is_some()));
        let jaw = m.parts.iter().position(|p| p.name == "jaw").unwrap();
        let head = m.parts.iter().position(|p| p.name == "head").unwrap();
        assert_eq!(m.parts[jaw].parent, Some(head as u16));
        // 7 head boxes, 17 spine segments of 2, 4 body, 4 wing bones,
        // 12 leg boxes (six faces each) and 4 zero-thickness wing skins
        // (two faces each).
        assert_eq!(m.quads.len(), (7 + 34 + 4 + 4 + 12) * 6 + 4 * 2);
        let (rot, off) = dragon_pose(0.0);
        let r = usize::from(DRAGON_ROOT);
        assert_eq!(off[r][2], -48.0);
        assert!(off[r][1] < -30.0 && off[r][1] > -32.5);
        // The head sits where the neck chain ends: the last segment's
        // position plus 10 px along its pitch.
        let n4 = usize::from(DRAGON_NECK) + 4;
        let end_z = -52.0 + off[n4][2] - rot[n4][0].cos() * 10.0;
        let h = usize::from(DRAGON_HEAD);
        assert!((-62.0 + off[h][2] - end_z).abs() < 1e-4);
        assert!((rot[usize::from(DRAGON_TAIL)][1] - std::f32::consts::PI).abs() < 1e-6);
        let (lw, rw) = (usize::from(DRAGON_LWING), usize::from(DRAGON_RWING));
        assert_eq!((rot[lw][2], rot[lw][1]), (-rot[rw][2], -rot[rw][1]));
    }
}
