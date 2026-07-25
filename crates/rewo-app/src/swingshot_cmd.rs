//! `rewo swingshot` — M19's permanent **serverless** combat-swing oracle.
//!
//! `ClientboundAnimatePacket` is the generic "this entity swung" signal. Unlike
//! M17's one-shot entity events and M18's metadata counters, the swing is a
//! small *state machine* on `LivingEntity` whose length depends on the item in
//! the swinging hand, and whose pose is `HumanoidModel.setupAttackAnimation`.
//! This gate proves the whole continuous production path with no socket and no
//! GPU device:
//!
//! ```text
//! raw animate body (VarInt id + unsigned byte action, built here)
//!   -> rewo_net::route_animate            (real packet-id selection seam)
//!   -> apply_animate                      (missing-entity drop + action mapping)
//!   -> EntityTable::swing / tick_lerp     (the exact LivingEntity swing clock)
//!   -> live_cmd::resolve_attack_anim      (the SAME app resolver collect_entities uses)
//!   -> rewo_gpu::entities::oracle_part_deltas  (the exact setupAttackAnimation math)
//! ```
//!
//! plus the two inputs that decide *how* it swings, through their own
//! production seams: `rewo_net::route_set_equipment` (item → duration + swing
//! type, via the real item registry and the datagen-derived prototype table)
//! and `rewo_net::route_set_entity_data` (main arm, metadata index 15 with the
//! HUMANOID_ARM serializer).
//!
//! **Fail-closed by construction.** A fixed [`EXPECTED_WITNESSES`] count equals
//! the number of named properties. Each property is *observed* (the real value
//! read and printed) and increments the counter only on a real pass; the run
//! errors if any property failed **or** the observed count differs — the latter
//! catches a property silently skipped by a missing part or a `None` from the
//! oracle.
//!
//! **The expected values are independent transcriptions.** [`expect_attack`] is
//! a private hand-port of `HumanoidModel.setupAttackAnimation` +
//! `SpearAnimations.thirdPersonAttackHand` + `AnimationUtils.bobModelPart` +
//! the `Ease` functions they call, written from the decompile — nothing reads
//! the production `anim_delta` as its expectation. Even vanilla's `Mth` sine
//! table is rebuilt here independently (its own array, its own lookup, std's
//! `sin` rather than the renderer's `libm`) so the quantization witness
//! compares two constructions rather than one against itself. The lifecycle
//! expectations are literal tick tables derived from `LivingEntity.swing` /
//! `updateSwingTime` / `getAttackAnim`.
//!
//! **Nothing unknowable is guessed.** An item id outside the registry, or a
//! component patch holding a codec this client cannot walk, marks that hand
//! `HandItem::Unknown`; the pose and CEM's `swing_progress` are then suppressed
//! rather than filled in from the item's prototype. Three witnesses pin that
//! (`d6`, `d9`, `d11`), including that an exact update lifts it again.
//!
//! **Every property carries a mutation/sensitivity partner.** The wrong-packet
//! id proves id selection is load-bearing; the non-swing actions prove 2/4/5 do
//! not touch the swing; the boat proves the living gate; the cow-vs-zombie pair
//! proves the machine-extracted swing-ticking set; the zombie-kind pose proves
//! the rig belongs to the humanoid player model alone while its
//! `swing_progress` still reaches CEM; the head-pitch case proves the WHACK
//! `bb` term; the STAB cases prove the spear delegate really undoes the shared
//! yaw; and the `Mth` witness fails on a plain-trig port by construction.

use clap::Args as ClapArgs;
use rewo_data::components::DataComponentIds;
use rewo_data::entity_types::EntityClasses;
use rewo_data::items::Items;
use rewo_data::swing_anim::{SwingAnimation, SwingAnimationType, SwingAnimations};
use rewo_data::{entity_types::EntityTypes, packets::Packets, DataPaths};
use rewo_gpu::entities::{oracle_part_deltas, CemFrameInputs, EntityDraw, OracleInputs};
use rewo_data::item_tags::ItemTag;
use rewo_gpu::mobs::{ArmPose, ArmPoses, EntityModelKind, SwingKind, SwingPose};
use rewo_net::ids::Ids;
use rewo_net::item_stack::SwingWireData;
use rewo_net::{route_animate, route_set_entity_data, route_set_equipment, SwingEffectIds};
use rewo_world::entities::HandItem;
use rewo_world::entities::{EntityState, EntityTable, InteractionHand};

/// Total named properties this gate asserts. Locked so a skipped property fails
/// the run even when nothing mismatched.
const EXPECTED_WITNESSES: usize = 97;

/// Degrees → radians, the factor `SpearAnimations` writes inline as `Math.PI/180`.
const DEG: f32 = std::f32::consts::PI / 180.0;
/// `HumanoidModel.createMesh` arm pivot x — transcribed here independently of
/// the production constant.
const ARM_PIVOT_X: f32 = 5.0;

#[derive(ClapArgs)]
pub struct SwingshotArgs {
    /// Assert every owned property. The oracle asserts unconditionally and a
    /// failure exits nonzero with or without this flag, so it only labels the
    /// run — the same convention `eventshot`/`danceshot` use.
    #[arg(long, default_value_t = false)]
    check: bool,
    /// Version whose `packets.json` / `registries.json` resolve the real
    /// `animate` / `set_equipment` ids, the player type id, the item registry
    /// and the data-component ids.
    #[arg(long, default_value = "26.2")]
    version: String,
}

pub fn run(args: SwingshotArgs) -> Result<(), String> {
    let mode = if args.check { "check" } else { "report" };
    println!(
        "[swingshot] mode: {mode} (serverless; the oracle asserts unconditionally — a \
         failure exits nonzero with or without --check)"
    );

    let paths = DataPaths::for_version(&args.version)
        .ok_or_else(|| "no config dir for version data".to_string())?;
    let packets = Packets::load(&paths.packets_json())?;
    let ids = Ids::resolve(&packets)?;
    let animate_id = ids.cb_play_animate;
    let equip_id = ids.cb_play_set_equipment;
    let sed_id = ids.cb_play_set_entity_data;
    // `entity_event` is a *different real* clientbound-play id — the control
    // that proves `route_animate` selects on the id, not on the body shape
    // (both bodies start with an int and end with a byte).
    let wrong_id = ids.cb_play_entity_event;

    let entity_types = EntityTypes::load(&paths.registries_json())?;
    let items = Items::load(&paths.registries_json())?;
    let components = DataComponentIds::load(&paths.registries_json())?;
    let wire = SwingWireData {
        prototypes: SwingAnimations::resolve(&items)?,
        components,
        use_profiles: rewo_data::use_item::UseProfiles::resolve(&items)?,
    };
    let classes = EntityClasses::resolve(&entity_types)?;
    let player_tid = entity_types.player_id;
    let tid = |name: &str| -> Result<i32, String> {
        entity_types
            .id_of(name)
            .ok_or_else(|| format!("registries.json: no {name} entity type"))
    };
    let zombie_tid = tid("minecraft:zombie")?;
    let cow_tid = tid("minecraft:cow")?;
    let mannequin_tid = tid("minecraft:mannequin")?;
    let boat_tid = tid("minecraft:oak_boat")?;
    let skeleton_tid = tid("minecraft:skeleton")?;
    let pillager_tid = tid("minecraft:pillager")?;
    let vindicator_tid = tid("minecraft:vindicator")?;
    let evoker_tid = tid("minecraft:evoker")?;
    let illusioner_tid = tid("minecraft:illusioner")?;
    // An item id that is deliberately NOT in the registry, for the
    // unresolvable-item witness. Picked past the end rather than invented.
    let unregistered_item = (0..).find(|i| !items.has_id(*i)).unwrap_or(i32::MAX);
    let spear = items
        .id("minecraft:iron_spear")
        .ok_or("registries.json: no minecraft:iron_spear")?;
    let sword = items
        .id("minecraft:stone_sword")
        .ok_or("registries.json: no minecraft:stone_sword")?;
    let effect_ids = load_effect_ids(&paths)?;
    // `ItemTags.SPEARS` from the real client jar — the same production loader
    // the live client uses. Fails closed if the jar is absent: the tag decides
    // the SPEAR hold pose and nothing may stand in for it.
    let jar = client_jar(&args.version)
        .ok_or("client jar not found — swingshot needs it for ItemTags.SPEARS")?;
    let spears = ItemTag::load_spears(&jar, &items)?;
    let bow = items
        .id("minecraft:bow")
        .ok_or("registries.json: no minecraft:bow")?;
    let crossbow = items
        .id("minecraft:crossbow")
        .ok_or("registries.json: no minecraft:crossbow")?;
    let _ = crate::live_cmd::CROSSBOW_ITEM.set(Some(crossbow));
    // M23: the items whose getUseAnimation reaches each use-driven arm pose.
    let use_item = |name: &str| -> Result<i32, String> {
        items
            .id(name)
            .ok_or_else(|| format!("registries.json: no {name}"))
    };

    println!(
        "[swingshot] ids: animate={animate_id} set_equipment={equip_id} \
         set_entity_data={sed_id} control(entity_event)={wrong_id}"
    );
    println!(
        "[swingshot] types: player={player_tid} zombie={zombie_tid} cow={cow_tid} \
         mannequin={mannequin_tid} boat={boat_tid}; items: iron_spear={spear} \
         stone_sword={sword} unregistered={unregistered_item}; components: \
         swing_animation={} damage={}",
        components.swing_animation, components.damage
    );
    println!(
        "[swingshot] mob effects: haste={:?} conduit_power={:?} mining_fatigue={:?}",
        effect_ids.haste, effect_ids.conduit_power, effect_ids.mining_fatigue
    );

    let ctx = Ctx {
        ids,
        animate_id,
        equip_id,
        sed_id,
        shield: use_item("minecraft:shield")?,
        spyglass: use_item("minecraft:spyglass")?,
        goat_horn: use_item("minecraft:goat_horn")?,
        brush: use_item("minecraft:brush")?,
        trident: use_item("minecraft:trident")?,
        apple: use_item("minecraft:apple")?,
        wrong_id,
        player_tid,
        zombie_tid,
        cow_tid,
        mannequin_tid,
        boat_tid,
        spear,
        sword,
        unregistered_item,
        effect_ids,
        wire,
        classes,
        spears,
        bow,
        crossbow,
        skeleton_tid,
        pillager_tid,
        vindicator_tid,
        evoker_tid,
        illusioner_tid,
    };
    let mut c = Checker::new();

    check_ids(&mut c, &ctx);
    check_routing(&mut c, &ctx);
    check_lifecycle(&mut c, &ctx);
    check_equipment(&mut c, &ctx);
    check_arms(&mut c, &ctx);
    check_pose(&mut c, &ctx);
    check_arm_poses(&mut c, &ctx);
    check_mob_rigs(&mut c, &ctx);
    check_item_use(&mut c, &ctx);
    check_integration(&mut c, &ctx);

    println!(
        "[swingshot] witnesses observed: {} / {}",
        c.witnessed, EXPECTED_WITNESSES
    );
    if !c.failures.is_empty() {
        return Err(format!(
            "{} propert{} failed: {}",
            c.failures.len(),
            if c.failures.len() == 1 { "y" } else { "ies" },
            c.failures.join(", ")
        ));
    }
    if c.witnessed != EXPECTED_WITNESSES {
        return Err(format!(
            "witness count {} != expected {} — a named property was skipped (fail-closed)",
            c.witnessed, EXPECTED_WITNESSES
        ));
    }
    println!("[swingshot] PASS — {} witnesses", c.witnessed);
    Ok(())
}

/// Everything resolved once from the reports, handed to every check.
struct Ctx {
    ids: Ids,
    animate_id: i32,
    equip_id: i32,
    sed_id: i32,
    wrong_id: i32,
    /// A `Player`, a `Monster` descendant, a living non-`Monster`, a
    /// `Mannequin` and a non-living entity — one of each class the gates split
    /// on, all resolved from the real registry.
    player_tid: i32,
    zombie_tid: i32,
    cow_tid: i32,
    mannequin_tid: i32,
    boat_tid: i32,
    spear: i32,
    sword: i32,
    unregistered_item: i32,
    effect_ids: SwingEffectIds,
    wire: SwingWireData,
    classes: EntityClasses,
    /// `minecraft:spears`, loaded from the real client jar. Fails closed if the
    /// jar is absent: the tag decides the SPEAR hold pose and there is nothing
    /// honest to substitute for it.
    spears: ItemTag,
    /// `Items.BOW` / `Items.CROSSBOW` — identity tests in the skeleton and
    /// pillager rigs (M20).
    bow: i32,
    crossbow: i32,
    /// The items whose `getUseAnimation` reaches each of the eight
    /// use-driven arm poses (M23), resolved from the real registry.
    shield: i32,
    spyglass: i32,
    goat_horn: i32,
    brush: i32,
    trident: i32,
    apple: i32,
    skeleton_tid: i32,
    pillager_tid: i32,
    vindicator_tid: i32,
    evoker_tid: i32,
    illusioner_tid: i32,
}

/// The three `minecraft:mob_effect` ids `getCurrentSwingDuration` consults.
///
/// Production captures these from the Configuration `registry_data` packet
/// (`parse_registry_data`, exactly like the M13 lightmap's night-vision /
/// darkness ids). This gate is serverless, so it reads the same registry from
/// the datagen report — the *ids* are the test input; what is under test is
/// `apply_swing_effect`'s id → effect → duration mapping.
fn load_effect_ids(paths: &DataPaths) -> Result<SwingEffectIds, String> {
    let text = std::fs::read_to_string(paths.registries_json())
        .map_err(|e| format!("read registries.json: {e}"))?;
    let json: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("parse registries.json: {e}"))?;
    let entries = json
        .get("minecraft:mob_effect")
        .and_then(|r| r.get("entries"))
        .and_then(|e| e.as_object())
        .ok_or("registries.json: no minecraft:mob_effect registry")?;
    let id = |name: &str| -> Option<i32> {
        entries
            .get(name)
            .and_then(|e| e.get("protocol_id"))
            .and_then(|i| i.as_i64())
            .map(|i| i as i32)
    };
    let ids = SwingEffectIds {
        haste: id("minecraft:haste"),
        conduit_power: id("minecraft:conduit_power"),
        mining_fatigue: id("minecraft:mining_fatigue"),
    };
    if ids.haste.is_none() || ids.conduit_power.is_none() || ids.mining_fatigue.is_none() {
        return Err(format!("registries.json: missing a swing mob effect ({ids:?})"));
    }
    Ok(ids)
}

// --------------------------------------------------------------- accumulator

struct Checker {
    witnessed: usize,
    failures: Vec<String>,
}

impl Checker {
    fn new() -> Self {
        Self {
            witnessed: 0,
            failures: Vec::new(),
        }
    }

    fn record(&mut self, name: &str, pass: bool, detail: impl std::fmt::Display) {
        let status = if pass { " ok " } else { "FAIL" };
        println!("[swingshot] {status}  {name}: {detail}");
        if pass {
            self.witnessed += 1;
        } else {
            self.failures.push(name.to_string());
        }
    }
}

// -------------------------------------------------------- independent oracle

/// Independent transcription of `Ease` (the four functions the attack uses).
mod ease {
    pub fn out_quart(x: f32) -> f32 {
        let s = (1.0 - x) * (1.0 - x);
        1.0 - s * s
    }
    pub fn in_out_sine(x: f32) -> f32 {
        // `Ease.inOutSine` calls `Mth.cos`, not the platform cosine.
        -(super::mth_cos((std::f32::consts::PI * x) as f64) - 1.0) / 2.0
    }
    pub fn in_quad(x: f32) -> f32 {
        x * x
    }
    pub fn in_out_expo(x: f32) -> f32 {
        if x < 0.5 {
            if x == 0.0 {
                0.0
            } else {
                (2.0f64.powf(20.0 * x as f64 - 10.0) / 2.0) as f32
            }
        } else if x == 1.0 {
            1.0
        } else {
            ((2.0 - 2.0f64.powf(-20.0 * x as f64 + 10.0)) / 2.0) as f32
        }
    }
    /// `SpearAnimations.progress`.
    pub fn progress(t: f32, start: f32, end: f32) -> f32 {
        ((t - start) / (end - start)).clamp(0.0, 1.0)
    }
}

/// The expected per-part deltas for the player humanoid, hand-transcribed from
/// `HumanoidModel.setupAnim`'s arm walk assignment plus
/// `setupAttackAnimation` (and its STAB delegate).
#[derive(Clone, Copy, Debug, Default)]
struct Expect {
    /// The poses this expectation was built from — surfaced so a witness can
    /// name the pose it is asserting without recomputing the derivation.
    right: ArmPose,
    left: ArmPose,
    body: [f32; 3],
    right_rot: [f32; 3],
    right_off: [f32; 3],
    left_rot: [f32; 3],
    left_off: [f32; 3],
}

#[allow(clippy::too_many_arguments)]
fn expect_attack(
    attack_time: f32,
    left_attack_arm: bool,
    kind: SwingKind,
    age_scale: f32,
    head_x_rot: f32,
    walk_pos: f32,
    walk_amount: f32,
    age_in_ticks: f32,
    poses: ArmPoses,
    head_y_rot: f32,
) -> Expect {
    use std::f32::consts::PI;
    let mut e = Expect::default();
    e.right = poses.right;
    e.left = poses.left;
    // `setupAnim`: the walk swing, assigned before the attack runs.
    // rightArm.xRot = cos(pos·0.6662 + π)·2·amount·0.5 / speedValue (1.0)
    let f = walk_pos * 0.6662;
    e.right_rot[0] = (f + PI).cos() * 2.0 * walk_amount * 0.5;
    e.left_rot[0] = f.cos() * 2.0 * walk_amount * 0.5;
    // Then the hold pose, exactly where `setupAnim` runs its dispatch: after
    // the walk assignment and before `setupAttackAnimation`.
    expect_pose_stage(&mut e.right_rot, poses, false, head_y_rot, head_x_rot);
    expect_pose_stage(&mut e.left_rot, poses, true, head_y_rot, head_x_rot);
    if attack_time <= 0.0 {
        // `if (!(attackTime <= 0.0F))` — the attack is skipped, but the arm
        // bob at the end of `setupAnim` is not.
        expect_bob(&mut e, age_in_ticks);
        return e;
    }
    // body.yRot = Mth.sin(Mth.sqrt(attackTime)·2π)·0.2, negated for a LEFT arm.
    let mut by = mth_sin((mth_sqrt(attack_time) * std::f32::consts::TAU) as f64) * 0.2;
    if left_attack_arm {
        by = -by;
    }
    e.body[1] = by;
    // Arm pivots are *assigned*; the rest pose is (∓5, 2, 0), so the delta is
    // target − base.
    let (sin_by, cos_by) = (mth_sin(by as f64), mth_cos(by as f64));
    e.right_off[0] = -cos_by * 5.0 * age_scale + ARM_PIVOT_X;
    e.right_off[2] = sin_by * 5.0 * age_scale;
    e.left_off[0] = cos_by * 5.0 * age_scale - ARM_PIVOT_X;
    e.left_off[2] = -sin_by * 5.0 * age_scale;
    e.right_rot[1] += by;
    e.left_rot[1] += by;
    e.left_rot[0] += by;
    match kind {
        SwingKind::Whack => {
            let arm = if left_attack_arm {
                &mut e.left_rot
            } else {
                &mut e.right_rot
            };
            let swing = ease::out_quart(attack_time);
            let aa = mth_sin((swing * PI) as f64);
            let bb = mth_sin((attack_time * PI) as f64) * -(head_x_rot - 0.7) * 0.75;
            arm[0] -= aa * 1.2 + bb;
            arm[1] += by * 2.0;
            arm[2] += mth_sin((attack_time * PI) as f64) * -0.4;
        }
        SwingKind::Stab => {
            // `SpearAnimations.thirdPersonAttackHand`.
            e.right_rot[1] -= by;
            e.left_rot[1] -= by;
            e.left_rot[0] -= by;
            let prepare = ease::in_out_sine(ease::progress(attack_time, 0.0, 0.05));
            let attack = ease::in_quad(ease::progress(attack_time, 0.05, 0.2));
            let retract = ease::in_out_expo(ease::progress(attack_time, 0.4, 1.0));
            let arm = if left_attack_arm {
                &mut e.left_rot
            } else {
                &mut e.right_rot
            };
            arm[0] += (90.0 * prepare - 120.0 * attack + 30.0 * retract) * DEG;
        }
        SwingKind::None => {}
    }
    expect_bob(&mut e, age_in_ticks);
    e
}

/// `AnimationUtils.bobModelPart(arm, ageInTicks, ±1)` — the last thing
/// `HumanoidModel.setupAnim` does to the arms, transcribed independently.
fn expect_bob(e: &mut Expect, age_in_ticks: f32) {
    for (rot, scale) in [(&mut e.right_rot, 1.0f32), (&mut e.left_rot, -1.0f32)] {
        rot[2] += scale * (mth_cos((age_in_ticks * 0.09) as f64) * 0.05 + 0.05);
        rot[0] += scale * (mth_sin((age_in_ticks * 0.067) as f64) * 0.05);
    }
}

/// Vanilla's `Mth` sine table, rebuilt here **independently** of the
/// production one: same definition, its own array, its own lookup, and
/// `std`'s `sin` rather than the `libm` the renderer uses. Two independent
/// constructions agreeing bit-for-bit is the witness; sharing the renderer's
/// table would only prove the renderer equals itself.
fn mth_table() -> &'static [f32; 65536] {
    static T: std::sync::OnceLock<Box<[f32; 65536]>> = std::sync::OnceLock::new();
    T.get_or_init(|| {
        let mut t = Box::new([0.0f32; 65536]);
        for (i, v) in t.iter_mut().enumerate() {
            *v = (i as f64 / 10430.378350470453).sin() as f32;
        }
        t
    })
}

/// `Mth.sin(double)` — `SIN[(int)((long)(x · scale) & 65535L)]`.
fn mth_sin(x: f64) -> f32 {
    mth_table()[(((x * 10430.378350470453) as i64) & 65535) as usize]
}

/// `Mth.cos(double)` — the table read a quarter-turn along.
fn mth_cos(x: f64) -> f32 {
    mth_table()[(((x * 10430.378350470453 + 16384.0) as i64) & 65535) as usize]
}

/// `Mth.sqrt(float)` = `(float)Math.sqrt(x)`.
fn mth_sqrt(x: f32) -> f32 {
    (x as f64).sqrt() as f32
}

// ------------------------------------------------------------------- helpers

/// `<config>/EwoClient/shared/versions/<v>/<v>.jar` — the same layout
/// `mobshot`/`demo` resolve the client jar from.
fn client_jar(version: &str) -> Option<std::path::PathBuf> {
    let mut p = dirs::config_dir()?;
    p.push("EwoClient");
    p.push("shared/versions");
    p.push(version);
    p.push(format!("{version}.jar"));
    p.exists().then_some(p)
}

/// A `ClientboundAnimatePacket` body: VarInt entity id + unsigned byte action.
fn animate_body(eid: i32, action: u8) -> Vec<u8> {
    let mut b = Vec::new();
    varint(eid, &mut b);
    b.push(action);
    b
}

fn varint(v: i32, out: &mut Vec<u8>) {
    let mut n = v as u32;
    loop {
        let byte = (n & 0x7f) as u8;
        n >>= 7;
        if n == 0 {
            out.push(byte);
            return;
        }
        out.push(byte | 0x80);
    }
}

/// One equipment slot's payload: `None` = `ItemStack.EMPTY`.
#[derive(Clone)]
enum Stack {
    Empty,
    /// Plain item, empty component patch.
    Plain(i32),
    /// Item with an explicit `swing_animation` override in its patch.
    Override(i32, u8, i32),
    /// Item whose patch *removes* `swing_animation`.
    RemoveSwing(i32),
    /// Item whose patch sets `swing_animation` and *then* carries a damage
    /// entry — the alignment case: the walk must consume both.
    OverrideThenDamage(i32, u8, i32, i32),
    /// Item whose patch leads with a component this decoder cannot walk.
    Unwalkable(i32),
    /// A crossbow whose patch sets `minecraft:charged_projectiles` to a
    /// one-entry list — `CrossbowItem.isCharged` true (M23). The nested value
    /// is an `ItemStackTemplate`: item id, count, then its own patch.
    Charged(i32, i32),
    /// The same component set to an **empty** list, which `isCharged` reads as
    /// not charged. The distinguishing case: present but empty must not read
    /// as charged just because the component was mentioned.
    ChargedEmpty(i32),
}

fn push_stack(b: &mut Vec<u8>, s: &Stack, comp: DataComponentIds) {
    match s {
        Stack::Empty => varint(0, b),
        Stack::Charged(item, projectile) => {
            varint(1, b);
            varint(*item, b);
            varint(1, b); // one added component
            varint(0, b);
            varint(comp.charged_projectiles, b);
            varint(1, b); // ByteBufCodecs.list length
            varint(*projectile, b); // Item.STREAM_CODEC
            varint(1, b); // VAR_INT count
            varint(0, b); // nested patch: 0 added
            varint(0, b); // nested patch: 0 removed
        }
        Stack::ChargedEmpty(item) => {
            varint(1, b);
            varint(*item, b);
            varint(1, b);
            varint(0, b);
            varint(comp.charged_projectiles, b);
            varint(0, b); // empty list
        }
        Stack::Plain(item) => {
            varint(1, b);
            varint(*item, b);
            varint(0, b);
            varint(0, b);
        }
        Stack::Override(item, kind, dur) => {
            varint(1, b);
            varint(*item, b);
            varint(1, b); // one added component
            varint(0, b);
            varint(comp.swing_animation, b);
            varint(*kind as i32, b);
            varint(*dur, b);
        }
        Stack::RemoveSwing(item) => {
            varint(1, b);
            varint(*item, b);
            varint(0, b);
            varint(1, b); // one removed component
            varint(comp.swing_animation, b);
        }
        Stack::OverrideThenDamage(item, kind, dur, damage) => {
            varint(1, b);
            varint(*item, b);
            varint(2, b); // two added components
            varint(0, b);
            varint(comp.swing_animation, b);
            varint(*kind as i32, b);
            varint(*dur, b);
            varint(comp.damage, b);
            varint(*damage, b);
        }
        Stack::Unwalkable(item) => {
            varint(1, b);
            varint(*item, b);
            varint(1, b);
            varint(0, b);
            // `minecraft:enchantments` (id 13 in 26.2) — a codec this decoder
            // does not transcribe, so the walk must stop right here.
            varint(13, b);
            b.extend_from_slice(&[0x01, 0x02, 0x03]);
        }
    }
}

/// A `ClientboundSetEquipmentPacket` body. `slots` are `(ordinal, stack)` pairs
/// in wire order; the continuation bit is set on all but the last.
fn equipment_body(eid: i32, slots: &[(u8, Stack)], comp: DataComponentIds) -> Vec<u8> {
    let mut b = Vec::new();
    varint(eid, &mut b);
    for (i, (ordinal, stack)) in slots.iter().enumerate() {
        let last = i == slots.len() - 1;
        b.push(if last { *ordinal } else { *ordinal | 0x80 });
        push_stack(&mut b, stack, comp);
    }
    b
}

/// A `set_entity_data` body carrying `Avatar.DATA_PLAYER_MAIN_HAND`
/// (index 15, HUMANOID_ARM serializer 42). `arm`: 0 = LEFT, 1 = RIGHT.
fn main_arm_body(eid: u8, arm: u8) -> Vec<u8> {
    vec![eid, 15, 42, arm, 0xFF]
}

fn add(t: &mut EntityTable, id: i32, type_id: i32) {
    t.add(id, EntityState::new(0, type_id, 0.0, 0.0, 0.0, 0.0, 0.0));
}

/// One entity of every class the gates split on: 1 player, 2 cow (living, not
/// swing-ticking), 3 zombie (a `Monster`), 4 mannequin, 5 boat (not living).
fn table(ctx: &Ctx) -> EntityTable {
    let mut t = EntityTable::default();
    add(&mut t, 1, ctx.player_tid);
    add(&mut t, 2, ctx.cow_tid);
    add(&mut t, 3, ctx.zombie_tid);
    add(&mut t, 4, ctx.mannequin_tid);
    add(&mut t, 5, ctx.boat_tid);
    t
}

fn swing(ctx: &Ctx, t: &mut EntityTable, eid: i32, action: u8) -> bool {
    route_animate(
        ctx.animate_id,
        &animate_body(eid, action),
        &ctx.ids,
        t,
        Some(&ctx.classes),
    )
}

fn equip(ctx: &Ctx, t: &mut EntityTable, eid: i32, slots: &[(u8, Stack)]) -> bool {
    route_set_equipment(
        ctx.equip_id,
        &equipment_body(eid, slots, ctx.wire.components),
        &ctx.ids,
        t,
        Some(&ctx.wire),
        Some(&ctx.classes),
    )
}

fn near(a: f32, b: f32, tol: f32) -> bool {
    (a - b).abs() <= tol
}

fn near3(a: [f32; 3], b: [f32; 3], tol: f32) -> bool {
    near(a[0], b[0], tol) && near(a[1], b[1], tol) && near(a[2], b[2], tol)
}

fn fmt3(v: [f32; 3]) -> String {
    format!("[{:+.6}, {:+.6}, {:+.6}]", v[0], v[1], v[2])
}

/// The zRot `AnimationUtils.bobModelPart` contributes on its own — the
/// baseline a "no strike" assertion has to compare against now that the bob is
/// modelled.
fn bob_zrot(left: bool, age_in_ticks: f32) -> f32 {
    let scale = if left { -1.0f32 } else { 1.0 };
    scale * (mth_cos((age_in_ticks * 0.09) as f64) * 0.05 + 0.05)
}

/// A named part's `(rot delta, pos delta)` in the oracle output.
fn find(
    deltas: &[(&'static str, [f32; 3], [f32; 3])],
    name: &str,
) -> Option<([f32; 3], [f32; 3])> {
    deltas
        .iter()
        .find(|&&(n, _, _)| n == name)
        .map(|&(_, r, o)| (r, o))
}

/// The full production pose path for one entity at one partial tick: resolve
/// the render state through the SAME app resolver `collect_entities` uses, then
/// run the GPU rig oracle (no device).
#[allow(clippy::too_many_arguments)]
fn pose(
    kind: EntityModelKind,
    t: &EntityTable,
    eid: i32,
    alpha: f32,
    pitch: f32,
    net: f32,
    limb_swing: f32,
    limb_amount: f32,
    spears: &ItemTag,
    bow: Option<i32>,
    crossbow: Option<i32>,
) -> (
    SwingPose,
    ArmPoses,
    Vec<(&'static str, [f32; 3], [f32; 3])>,
) {
    let attack = crate::live_cmd::resolve_attack_anim(t, eid, alpha);
    // The SAME app resolver `collect_entities` uses — the gate must not build
    // its own `ArmPoses`, or it would stop proving the live mapping.
    let arm_poses = crate::live_cmd::resolve_arm_poses(t, eid, kind, spears, bow, crossbow);
    // M20: the same app resolver the collector uses for the mob rigs.
    let mob = crate::live_cmd::resolve_mob_combat(t, eid, kind, bow);
    let deltas = oracle_part_deltas(
        kind,
        &OracleInputs {
            pitch,
            net,
            limb_swing,
            limb_amount,
            attack,
            arm_poses,
            mob,
            ..Default::default()
        },
    )
    .unwrap_or_default();
    (attack, arm_poses, deltas)
}

// ---- independent transcriptions of the arm-pose stage --------------------
//
// These deliberately do NOT call `ArmPose::affects_offhand_pose` /
// `is_two_handed` / `ArmPoses::poses_arm`. Those are the production answers;
// reading them here would make the dispatch witnesses grade the renderer
// against itself, which is the exact defect `dimensioncheck` once shipped.

/// `ArmPose`'s `twoHanded` constructor argument. `EMPTY(false,…)`,
/// `ITEM(false,…)`, `SPEAR(false,…)`.
fn want_two_handed(p: ArmPose) -> bool {
    matches!(
        p,
        ArmPose::BowAndArrow | ArmPose::CrossbowCharge | ArmPose::CrossbowHold
    )
}

/// `ArmPose`'s `affectsOffhandPose` constructor argument, transcribed
/// independently from `HumanoidModel:488` — never read off the production
/// `ArmPose::affects_offhand_pose`.
fn want_affects_offhand(p: ArmPose) -> bool {
    matches!(
        p,
        ArmPose::BowAndArrow
            | ArmPose::ThrowTrident
            | ArmPose::CrossbowCharge
            | ArmPose::CrossbowHold
            | ArmPose::Spear
    )
}

/// Which arm `setupAnim` poses first, and whether the second runs — **both**
/// dispatch branches, including the `isUsingItem` one M23 made reachable.
fn want_pose_order(p: ArmPoses) -> (bool, bool) {
    let first_left = if p.using_item {
        // `mainHandUsed == rightHanded` → right first.
        p.main_hand_used != p.right_handed
    } else {
        let two_handed_offhand = if p.right_handed {
            want_two_handed(p.left)
        } else {
            want_two_handed(p.right)
        };
        p.right_handed != two_handed_offhand
    };
    let first = if first_left { p.left } else { p.right };
    (first_left, !want_affects_offhand(first))
}

/// Whether `pose{Left,Right}Arm` is called for this arm at all.
fn want_poses_arm(p: ArmPoses, left: bool) -> bool {
    if !p.known {
        return false;
    }
    let (first_left, second_runs) = want_pose_order(p);
    if first_left == left {
        true
    } else {
        second_runs
    }
}

/// The whole pose stage for one arm: replay the dispatch and apply every
/// running case's effect on this arm, in vanilla's order.
fn expect_pose_stage(rot: &mut [f32; 3], p: ArmPoses, left: bool, head_yaw: f32, head_pitch: f32) {
    if !p.known {
        return;
    }
    let (first_left, second_runs) = want_pose_order(p);
    let pose_of = |l: bool| if l { p.left } else { p.right };
    expect_pose_arm(
        rot,
        p,
        pose_of(first_left),
        first_left,
        left,
        head_yaw,
        head_pitch,
    );
    if second_runs {
        let sl = !first_left;
        expect_pose_arm(rot, p, pose_of(sl), sl, left, head_yaw, head_pitch);
    }
}

/// One `pose{Right,Left}Arm` case's effect on the arm `target_left`.
///
/// Independently transcribed from `HumanoidModel.poseRightArm` / `poseLeftArm`
/// / `poseBlockingArm` and `AnimationUtils.animateCrossbow{Hold,Charge}`.
fn expect_pose_arm(
    rot: &mut [f32; 3],
    p: ArmPoses,
    pose: ArmPose,
    pose_left: bool,
    target_left: bool,
    head_yaw: f32,
    head_pitch: f32,
) {
    use std::f32::consts::{FRAC_PI_2, PI};
    let own = pose_left == target_left;
    let both = matches!(
        pose,
        ArmPose::BowAndArrow | ArmPose::CrossbowCharge | ArmPose::CrossbowHold
    );
    if !own && !both {
        return;
    }
    // `holdingInRightArm`.
    let right = !pose_left;
    let lerp = |d: f32, a: f32, b: f32| a + d * (b - a);
    match pose {
        ArmPose::Empty => rot[1] = 0.0,
        ArmPose::Item => {
            rot[0] = rot[0] * 0.5 - PI / 10.0;
            rot[1] = 0.0;
        }
        ArmPose::Block => {
            rot[0] = rot[0] * 0.5 - 0.9424779 + head_pitch.clamp(-(PI * 4.0 / 9.0), 0.43633232);
            rot[1] =
                (if right { -30.0f32 } else { 30.0 }) * DEG + head_yaw.clamp(-PI / 6.0, PI / 6.0);
        }
        // right-called: rightArm.yRot = -0.1 + hy;       leftArm.yRot = 0.1 + hy + 0.4
        // left-called:  rightArm.yRot = -0.1 + hy - 0.4; leftArm.yRot = 0.1 + hy
        ArmPose::BowAndArrow => {
            let base = if target_left { 0.1 } else { -0.1 };
            let nudge = if own {
                0.0
            } else if target_left {
                0.4
            } else {
                -0.4
            };
            rot[1] = base + head_yaw + nudge;
            rot[0] = -PI / 2.0 + head_pitch;
        }
        ArmPose::ThrowTrident => {
            rot[0] = rot[0] * 0.5 - PI;
            rot[1] = 0.0;
        }
        ArmPose::CrossbowCharge => {
            let max = p.max_crossbow_charge;
            if !(max > 0.0) {
                return;
            }
            if own {
                rot[1] = if right { -0.8 } else { 0.8 };
                rot[0] = -0.97079635;
            } else {
                let alpha = p.ticks_using_item.clamp(0.0, max) / max;
                rot[1] = lerp(alpha, 0.4, 0.85) * if right { 1.0 } else { -1.0 };
                rot[0] = lerp(alpha, -0.97079635, -PI / 2.0);
            }
        }
        ArmPose::CrossbowHold => {
            if own {
                rot[1] = (if right { -0.3f32 } else { 0.3 }) + head_yaw;
                rot[0] = -PI / 2.0 + head_pitch + 0.1;
            } else {
                rot[1] = (if right { 0.6f32 } else { -0.6 }) + head_yaw;
                rot[0] = -1.5 + head_pitch;
            }
        }
        ArmPose::Spyglass => {
            // isCrouching is unmodelled, hence false, so the PI/12 duck is out.
            rot[0] = (head_pitch - 1.9198622).clamp(-2.4, 3.3);
            rot[1] = head_yaw + if right { -PI / 12.0 } else { PI / 12.0 };
        }
        ArmPose::TootHorn => {
            rot[0] = head_pitch.clamp(-1.2, 1.2) - 1.4835298;
            rot[1] = head_yaw + if right { -PI / 6.0 } else { PI / 6.0 };
        }
        ArmPose::Brush => {
            rot[0] = rot[0] * 0.5 - PI / 5.0;
            rot[1] = 0.0;
        }
        ArmPose::Spear => {
            let invert = if pose_left { -1.0f32 } else { 1.0 };
            rot[1] = -0.1 * invert + head_yaw;
            rot[0] = -FRAC_PI_2 + head_pitch + 0.8;
            // isFallFlying / swimAmount are both false for any state this
            // client can be in, so the -0.9599311 duck never applies.
            let r2d = 180.0f32 / PI;
            rot[1] = DEG * (rot[1] * r2d).clamp(-60.0, 60.0);
            rot[0] = DEG * (rot[0] * r2d).clamp(-120.0, 30.0);
        }
    }
}

// ------------------------------------------------------------------- checks

fn check_ids(c: &mut Checker, ctx: &Ctx) {
    c.record(
        "a1.animate_id_resolved",
        ctx.animate_id != ctx.wrong_id,
        format!(
            "animate={} != control entity_event={}",
            ctx.animate_id, ctx.wrong_id
        ),
    );
    c.record(
        "a2.set_equipment_id_resolved",
        ctx.equip_id != ctx.animate_id && ctx.equip_id != ctx.sed_id,
        format!(
            "set_equipment={} distinct from animate={} and set_entity_data={}",
            ctx.equip_id, ctx.animate_id, ctx.sed_id
        ),
    );
    c.record(
        "a3.player_type_id_resolved",
        ctx.player_tid != ctx.zombie_tid,
        format!("player={} != zombie={}", ctx.player_tid, ctx.zombie_tid),
    );
    // The generated prototype table, resolved through the real item registry.
    // The unregistered id must answer `None` — not the bare default, which is
    // what "we know nothing about this item" would otherwise be mistaken for.
    let spear = ctx.wire.prototypes.of(ctx.spear);
    let sword = ctx.wire.prototypes.of(ctx.sword);
    let unknown = ctx.wire.prototypes.of(ctx.unregistered_item);
    c.record(
        "a4.item_prototype_swing_animations",
        spear == Some(SwingAnimation::new(SwingAnimationType::Stab, 19))
            && sword == Some(SwingAnimation::DEFAULT)
            && unknown.is_none()
            && ctx.wire.prototypes.non_default_count() == 7,
        format!(
            "iron_spear={spear:?} stone_sword={sword:?} unregistered({})={unknown:?} \
             non_default={}",
            ctx.unregistered_item,
            ctx.wire.prototypes.non_default_count()
        ),
    );
    // The machine-extracted living / swing-ticking classification. The counts
    // are the generator's; the spot checks are the four decompiled call sites
    // (`Player`, `Monster`, `Mannequin`) restated as entity types.
    let k = &ctx.classes;
    let living_ok = [ctx.player_tid, ctx.zombie_tid, ctx.cow_tid, ctx.mannequin_tid]
        .iter()
        .all(|id| k.is_living(*id))
        && !k.is_living(ctx.boat_tid);
    let ticking_ok = k.ticks_swing(ctx.player_tid)
        && k.ticks_swing(ctx.zombie_tid)
        && k.ticks_swing(ctx.mannequin_tid)
        && !k.ticks_swing(ctx.cow_tid)
        && !k.ticks_swing(ctx.boat_tid);
    c.record(
        "a5.entity_classes_resolved_from_the_decompiled_hierarchy",
        living_ok && ticking_ok && k.swing_ticking_count() < k.living_count(),
        format!(
            "{} living / {} swing-ticking; player+zombie+mannequin tick, cow is living but \
             does not, boat is not living",
            k.living_count(),
            k.swing_ticking_count()
        ),
    );
}

fn check_routing(c: &mut Checker, ctx: &Ctx) {
    // Action 0 = MAIN_HAND.
    let mut t = table(ctx);
    let matched = swing(ctx, &mut t, 1, 0);
    let arm = t.swing_debug(1).and_then(|s| s.4);
    c.record(
        "b1.action_0_swings_the_main_hand",
        matched && arm == Some(InteractionHand::MainHand),
        format!("routed={matched} swingingArm={arm:?}"),
    );
    // Action 3 = OFF_HAND (accepted immediately: swingTime is still −1).
    swing(ctx, &mut t, 1, 3);
    c.record(
        "b2.action_3_swings_the_off_hand",
        t.swing_debug(1).and_then(|s| s.4) == Some(InteractionHand::OffHand),
        format!("swingingArm={:?}", t.swing_debug(1).and_then(|s| s.4)),
    );
    // 2 (wake up), 4 (crit particles), 5 (enchanted-hit particles) and any
    // other byte must not touch swing state.
    let mut inert = true;
    let mut seen = Vec::new();
    for action in [1u8, 2, 4, 5, 6, 64, 255] {
        let mut t = table(ctx);
        swing(ctx, &mut t, 1, action);
        let s = t.swing_debug(1);
        if s.is_some() {
            inert = false;
        }
        seen.push(format!("{action}:{}", if s.is_some() { "SET" } else { "-" }));
    }
    c.record(
        "b3.other_actions_never_touch_the_swing",
        inert,
        format!("actions {}", seen.join(" ")),
    );
    // The wrong packet id must not match and must not swing.
    let mut t = table(ctx);
    let matched = route_animate(
        ctx.wrong_id,
        &animate_body(1, 0),
        &ctx.ids,
        &mut t,
        Some(&ctx.classes),
    );
    c.record(
        "b4.wrong_packet_id_is_inert",
        !matched && t.swing_debug(1).is_none(),
        format!("routed={matched} swing={:?}", t.swing_debug(1)),
    );
    // Missing entity, empty body, id-without-action.
    let mut t = table(ctx);
    swing(ctx, &mut t, 99, 0);
    let missing = t.swing_debug(99).is_none();
    route_animate(ctx.animate_id, &[], &ctx.ids, &mut t, Some(&ctx.classes));
    route_animate(ctx.animate_id, &[1], &ctx.ids, &mut t, Some(&ctx.classes));
    let malformed = t.swing_debug(1).is_none();
    c.record(
        "b5.missing_entity_and_malformed_bodies_are_inert",
        missing && malformed,
        format!("missing_inert={missing} malformed_inert={malformed}"),
    );
    // The entity id is a VarInt, not the fixed BE i32 `entity_event` uses.
    let mut t = EntityTable::default();
    add(&mut t, 300, ctx.player_tid);
    add(&mut t, 0, ctx.player_tid); // what a BE-i32 misread would hit first
    let body = animate_body(300, 0);
    route_animate(ctx.animate_id, &body, &ctx.ids, &mut t, Some(&ctx.classes));
    c.record(
        "b6.entity_id_is_a_varint_not_a_fixed_i32",
        body.len() == 3 && t.swing_debug(300).is_some() && t.swing_debug(0).is_none(),
        format!(
            "body={body:?} → 300 swung={} 0 swung={}",
            t.swing_debug(300).is_some(),
            t.swing_debug(0).is_some()
        ),
    );
    // A non-living entity must be inert on all three swing inputs — vanilla
    // casts (`(LivingEntity)entity`) or tests `instanceof`, so a boat cannot
    // grow a swing, hold a weapon, or carry haste.
    let mut t = table(ctx);
    swing(ctx, &mut t, 5, 0);
    let no_swing = t.swing_debug(5).is_none();
    equip(ctx, &mut t, 5, &[(0, Stack::Plain(ctx.spear))]);
    let no_item = t.hand_item(5, InteractionHand::MainHand) == HandItem::Empty;
    effect(ctx, &mut t, 5, ctx.effect_ids.haste, 0, true);
    let no_haste = t.current_swing_duration(5) == Some(6);
    c.record(
        "b7.a_non_living_entity_is_inert_on_every_swing_input",
        no_swing && no_item && no_haste,
        format!("boat: swing={no_swing} equipment={no_item} haste={no_haste}"),
    );
}

fn check_lifecycle(c: &mut Checker, ctx: &Ctx) {
    let mut t = table(ctx);
    swing(ctx, &mut t, 1, 0);
    c.record(
        "c1.an_accepted_swing_parks_swingtime_at_minus_one",
        t.swing_debug(1) == Some((true, -1, 0.0, 0.0, Some(InteractionHand::MainHand))),
        format!("{:?}", t.swing_debug(1)),
    );
    // Default duration 6 → attackAnim 0, 1/6 … 5/6, then back to 0.
    let mut seq = Vec::new();
    let mut ok = true;
    for step in 0..7 {
        t.tick_lerp();
        let (swinging, time, anim, _, _) = t.swing_debug(1).unwrap();
        seq.push(format!("{time}:{anim:.4}"));
        let (want_time, want_anim, want_swinging) = if step < 6 {
            (step, step as f32 / 6.0, true)
        } else {
            (0, 0.0, false)
        };
        ok &= time == want_time && near(anim, want_anim, 1e-6) && swinging == want_swinging;
    }
    c.record(
        "c2.default_duration_six_tick_sequence",
        ok,
        format!("swingTime:attackAnim = {}", seq.join(" ")),
    );
    // A repeat inside the first half is ignored (integer duration/2 = 3).
    let mut t = table(ctx);
    swing(ctx, &mut t, 1, 0);
    for _ in 0..3 {
        t.tick_lerp(); // swingTime 0, 1, 2
    }
    let before = t.swing_debug(1).unwrap();
    swing(ctx, &mut t, 1, 3);
    let after = t.swing_debug(1).unwrap();
    c.record(
        "c3.a_first_half_repeat_is_ignored",
        before == after && after.1 == 2 && after.4 == Some(InteractionHand::MainHand),
        format!("swingTime={} arm={:?} unchanged={}", after.1, after.4, before == after),
    );
    // …and one at exactly duration/2 restarts.
    t.tick_lerp(); // swingTime 3 == 6/2
    swing(ctx, &mut t, 1, 3);
    let after = t.swing_debug(1).unwrap();
    c.record(
        "c4.the_half_boundary_restarts_the_swing",
        after.0 && after.1 == -1 && after.4 == Some(InteractionHand::OffHand),
        format!("swinging={} swingTime={} arm={:?}", after.0, after.1, after.4),
    );
    // Partial-tick interpolation between oAttackAnim and attackAnim.
    let mut t = table(ctx);
    swing(ctx, &mut t, 1, 0);
    for _ in 0..4 {
        t.tick_lerp(); // oAttackAnim 2/6, attackAnim 3/6
    }
    let (a0, a5, a1) = (
        t.attack_anim(1, 0.0),
        t.attack_anim(1, 0.5),
        t.attack_anim(1, 1.0),
    );
    c.record(
        "c5.partial_tick_interpolation",
        near(a0, 2.0 / 6.0, 1e-6) && near(a5, 2.5 / 6.0, 1e-6) && near(a1, 3.0 / 6.0, 1e-6),
        format!("alpha 0/0.5/1 = {a0:.6}/{a5:.6}/{a1:.6}"),
    );
    // The end of the swing steps 5/6 → 0; `getAttackAnim` wraps the negative
    // difference forward through 1.0 instead of snapping backwards.
    for _ in 4..7 {
        t.tick_lerp();
    }
    let (w0, w5, w1) = (
        t.attack_anim(1, 0.0),
        t.attack_anim(1, 0.5),
        t.attack_anim(1, 1.0),
    );
    c.record(
        "c6.the_end_of_a_swing_wraps_forward",
        near(w0, 5.0 / 6.0, 1e-6) && near(w5, 5.0 / 6.0 + 0.5 / 6.0, 1e-6) && near(w1, 1.0, 1e-6),
        format!("alpha 0/0.5/1 = {w0:.6}/{w5:.6}/{w1:.6} (a naive lerp would fall to 0)"),
    );
    // Removal and id reuse clear everything.
    let mut t = table(ctx);
    equip(ctx, &mut t, 1, &[(0, Stack::Plain(ctx.spear))]);
    swing(ctx, &mut t, 1, 0);
    t.tick_lerp();
    let live = t.swing_debug(1).is_some() && t.current_swing_duration(1) == Some(19);
    t.remove(1);
    let cleared = t.swing_debug(1).is_none() && t.current_swing_duration(1) == Some(6);
    add(&mut t, 1, ctx.player_tid);
    let reused = t.swing_debug(1).is_none() && t.attack_anim(1, 1.0) == 0.0;
    c.record(
        "c7.removal_and_id_reuse_clear_the_swing",
        live && cleared && reused,
        format!("live={live} cleared={cleared} reused_clean={reused}"),
    );
    // A living kind whose client class does not run `updateSwingTime` stores
    // the accepted swing but never advances it — the cow, not the zombie.
    let mut t = table(ctx);
    for eid in [1, 2, 3, 4] {
        swing(ctx, &mut t, eid, 0);
    }
    for _ in 0..3 {
        t.tick_lerp();
    }
    let cow = t.swing_debug(2).unwrap();
    c.record(
        "c8.a_living_non_monster_accepts_but_never_advances",
        cow.1 == -1 && t.attack_anim(2, 1.0) == 0.0,
        format!(
            "cow swingTime={} attackAnim={} (a hoglin can be sent swing() and still never \
             animate)",
            cow.1,
            t.attack_anim(2, 1.0)
        ),
    );
    // …while Player, a Monster descendant and Mannequin all do.
    let ticked: Vec<(i32, i32)> = [1, 3, 4]
        .iter()
        .map(|eid| (*eid, t.swing_debug(*eid).unwrap().1))
        .collect();
    c.record(
        "c9.player_monster_and_mannequin_all_tick",
        ticked.iter().all(|(_, time)| *time == 2),
        format!("(entity, swingTime) = {ticked:?} after 3 ticks (want 2 each)"),
    );
    // And a Monster's swing reaches CEM: `swing_progress` is published for
    // every mob, not only the model Rewo poses.
    let monster = crate::live_cmd::resolve_attack_anim(&t, 3, 1.0);
    let published = cem_swing_progress(EntityModelKind::Zombie, monster);
    c.record(
        "c10.a_monster_swing_reaches_cem_swing_progress",
        monster.attack_time > 0.0 && near(published, monster.attack_time, 0.0),
        format!(
            "zombie attackTime={:.6} → AnimContext.swing_progress={published:.6}",
            monster.attack_time
        ),
    );
}

/// Route one `update_mob_effect` / `remove_mob_effect` through production.
fn effect(ctx: &Ctx, t: &mut EntityTable, eid: i32, id: Option<i32>, amp: i32, add: bool) {
    let mut b = Vec::new();
    varint(eid, &mut b);
    varint(id.expect("effect id resolved"), &mut b);
    if add {
        varint(amp, &mut b);
        varint(100, &mut b);
        b.push(0);
    }
    rewo_net::apply_swing_effect(&b, t, ctx.effect_ids, add, Some(&ctx.classes));
}

/// The `swing_progress` the production CEM context builder publishes for a
/// draw carrying `attack` — the exact function `emit_model` calls.
fn cem_swing_progress(kind: EntityModelKind, attack: SwingPose) -> f32 {
    let draw = neutral_draw(kind, attack);
    rewo_gpu::entities::cem_anim_context(
        &draw,
        CemFrameInputs {
            frame_time: 0.05,
            frame_counter: 1.0,
            age_seconds: 0.0,
            cam_pos: [0.0; 3],
        },
        Vec::new(),
    )
    .swing_progress
}

/// A minimal `EntityDraw` carrying just the fields the CEM context reads.
fn neutral_draw(kind: EntityModelKind, attack: SwingPose) -> EntityDraw<'static> {
    EntityDraw {
        pos: [0.0; 3],
        width: 0.6,
        height: 1.8,
        color: [1.0; 3],
        name: None,
        kind,
        yaw: 0.0,
        head_yaw: 0.0,
        pitch: 0.0,
        limb_swing: 7.5,
        limb_amount: 0.8,
        gesture: None,
        events: [None; rewo_gpu::mobs::ModelEvent::COUNT],
        shell: false,
        allay_dance: None,
        attack,
        arm_poses: ArmPoses::EMPTY,
        mob: rewo_gpu::mobs::MobCombat::default(),
        hurt: false,
        held: [None, None],
        skin_uv: None,
        scale_mul: 1.0,
        anim_id: 0.0,
        light: [1.0; 3],
    }
}

fn check_equipment(c: &mut Checker, ctx: &Ctx) {
    let held = |t: &EntityTable, hand| -> Option<(i32, SwingAnimation)> {
        t.hand_item(1, hand).held().map(|i| (i.item_id, i.swing))
    };
    // A plain iron spear in the main hand: prototype STAB / 19.
    let mut t = table(ctx);
    let matched = equip(ctx, &mut t, 1, &[(0, Stack::Plain(ctx.spear))]);
    c.record(
        "d1.an_empty_patch_uses_the_item_prototype",
        matched
            && t.current_swing_duration(1) == Some(19)
            && t.swing_animation_type(1) == Some(SwingAnimationType::Stab),
        format!(
            "routed={matched} duration={:?} type={:?}",
            t.current_swing_duration(1),
            t.swing_animation_type(1)
        ),
    );
    // A stone sword is the plain default.
    let mut t = table(ctx);
    equip(ctx, &mut t, 1, &[(0, Stack::Plain(ctx.sword))]);
    c.record(
        "d2.an_ordinary_item_is_whack_six",
        t.current_swing_duration(1) == Some(6)
            && t.swing_animation_type(1) == Some(SwingAnimationType::Whack),
        format!(
            "duration={:?} type={:?}",
            t.current_swing_duration(1),
            t.swing_animation_type(1)
        ),
    );
    // Sword main hand + spear off hand: an OFF_HAND swing reads the off hand
    // for the duration and (attackArm having flipped) for the type too.
    let mut t = table(ctx);
    equip(
        ctx,
        &mut t,
        1,
        &[(0, Stack::Plain(ctx.sword)), (1, Stack::Plain(ctx.spear))],
    );
    let main = (t.current_swing_duration(1), t.swing_animation_type(1));
    swing(ctx, &mut t, 1, 3);
    let off = (t.current_swing_duration(1), t.swing_animation_type(1));
    c.record(
        "d3.an_off_hand_swing_reads_the_off_hand_item",
        main == (Some(6), Some(SwingAnimationType::Whack))
            && off == (Some(19), Some(SwingAnimationType::Stab)),
        format!("main-hand {main:?} → off-hand {off:?}"),
    );
    // An explicit override in the patch wins over the prototype.
    let mut t = table(ctx);
    equip(ctx, &mut t, 1, &[(0, Stack::Override(ctx.sword, 2, 11))]);
    c.record(
        "d4.an_explicit_patch_override_wins",
        t.current_swing_duration(1) == Some(11)
            && t.swing_animation_type(1) == Some(SwingAnimationType::Stab),
        format!(
            "stone_sword patched to STAB/11 → duration={:?} type={:?}",
            t.current_swing_duration(1),
            t.swing_animation_type(1)
        ),
    );
    // Removing the component yields `SwingAnimation.DEFAULT`, NOT the spear's
    // prototype — `getOrDefault` sees no value at all.
    let mut t = table(ctx);
    equip(ctx, &mut t, 1, &[(0, Stack::RemoveSwing(ctx.spear))]);
    c.record(
        "d5.a_patch_removal_falls_to_the_bare_default",
        t.current_swing_duration(1) == Some(6)
            && t.swing_animation_type(1) == Some(SwingAnimationType::Whack),
        format!(
            "spear with !swing_animation → duration={:?} type={:?} (prototype would be 19/Stab)",
            t.current_swing_duration(1),
            t.swing_animation_type(1)
        ),
    );
    // A patch leading with an un-transcribed codec: the hand becomes UNKNOWN —
    // not the item's prototype, which would be a guessed visual — and the
    // *other* slot in the same packet is abandoned, because the reader is
    // parked mid-component.
    let mut t = table(ctx);
    equip(
        ctx,
        &mut t,
        1,
        &[
            (0, Stack::Unwalkable(ctx.spear)),
            (1, Stack::Plain(ctx.sword)),
        ],
    );
    let main_item = t.hand_item(1, InteractionHand::MainHand);
    let off_item = t.hand_item(1, InteractionHand::OffHand);
    swing(ctx, &mut t, 1, 0);
    t.tick_lerp();
    let pose = crate::live_cmd::resolve_attack_anim(&t, 1, 1.0);
    c.record(
        "d6.an_unwalkable_patch_is_unknown_and_suppresses_the_pose",
        main_item == HandItem::Unknown
            && off_item == HandItem::Empty
            && t.current_swing_duration(1).is_none()
            && !t.swing_inputs_known(1)
            && !pose.inputs_known
            && pose.attack_time == 0.0,
        format!(
            "main={main_item:?} off={off_item:?} duration={:?} pose(known={}, t={}) — the \
             prototype (Stab/19) is NOT substituted and the packet stops at the \
             unwalkable stack",
            t.current_swing_duration(1),
            pose.inputs_known,
            pose.attack_time
        ),
    );
    // An item id outside the registry is equally unknowable — a plain patch
    // does not make an unknown item known.
    let mut t = table(ctx);
    equip(ctx, &mut t, 1, &[(0, Stack::Plain(ctx.unregistered_item))]);
    let unreg = t.hand_item(1, InteractionHand::MainHand);
    c.record(
        "d9.an_unregistered_item_is_unknown_not_the_bare_default",
        unreg == HandItem::Unknown
            && t.current_swing_duration(1).is_none()
            && t.swing_animation_type(1).is_none(),
        format!(
            "item {} → {unreg:?}, duration={:?} (WHACK/6 would be a guess)",
            ctx.unregistered_item,
            t.current_swing_duration(1)
        ),
    );
    // The patch walk must continue *past* the swing component: a swing override
    // followed by a damage entry, with a second slot after the stack, has to
    // leave the reader aligned or the off hand is parsed out of garbage.
    let mut t = table(ctx);
    equip(
        ctx,
        &mut t,
        1,
        &[
            (0, Stack::OverrideThenDamage(ctx.sword, 2, 11, 37)),
            (1, Stack::Plain(ctx.spear)),
        ],
    );
    let m = held(&t, InteractionHand::MainHand);
    let o = held(&t, InteractionHand::OffHand);
    c.record(
        "d10.the_walk_continues_past_the_swing_component_and_stays_aligned",
        m == Some((ctx.sword, SwingAnimation::new(SwingAnimationType::Stab, 11)))
            && o == Some((ctx.spear, SwingAnimation::new(SwingAnimationType::Stab, 19)))
            && t.swing_inputs_known(1),
        format!("main={m:?} off={o:?} (returning early after the swing would desync the off hand)"),
    );
    // Suppression is not permanent: an exact update repairs it.
    let mut t = table(ctx);
    equip(ctx, &mut t, 1, &[(0, Stack::Unwalkable(ctx.spear))]);
    let before = t.swing_inputs_known(1);
    equip(ctx, &mut t, 1, &[(0, Stack::Plain(ctx.sword))]);
    swing(ctx, &mut t, 1, 0);
    t.tick_lerp();
    t.tick_lerp();
    let after = crate::live_cmd::resolve_attack_anim(&t, 1, 1.0);
    c.record(
        "d11.an_exact_update_lifts_the_suppression",
        !before && t.swing_inputs_known(1) && after.inputs_known && after.attack_time > 0.0,
        format!(
            "suppressed={} → repaired, pose(known={}, t={:.6})",
            !before, after.inputs_known, after.attack_time
        ),
    );
    // An empty stack is the bare hand: `ItemStack.EMPTY`'s components are empty
    // so `getOrDefault` still hands back the default.
    let mut t = table(ctx);
    equip(ctx, &mut t, 1, &[(0, Stack::Plain(ctx.spear))]);
    let armed = t.current_swing_duration(1);
    equip(ctx, &mut t, 1, &[(0, Stack::Empty)]);
    c.record(
        "d7.an_empty_stack_is_the_bare_hand_default",
        armed == Some(19)
            && t.hand_item(1, InteractionHand::MainHand) == HandItem::Empty
            && t.current_swing_duration(1) == Some(6),
        format!("armed={armed:?} → unarmed={:?}", t.current_swing_duration(1)),
    );
    // Haste shortens, mining fatigue lengthens, dig speed wins outright.
    let mut t = table(ctx);
    let e = ctx.effect_ids;
    effect(ctx, &mut t, 1, e.haste, 0, true);
    let hasted = t.current_swing_duration(1);
    effect(ctx, &mut t, 1, e.conduit_power, 1, true);
    let conduit = t.current_swing_duration(1);
    effect(ctx, &mut t, 1, e.mining_fatigue, 0, true);
    let both = t.current_swing_duration(1);
    effect(ctx, &mut t, 1, e.haste, 0, false);
    effect(ctx, &mut t, 1, e.conduit_power, 0, false);
    let fatigued = t.current_swing_duration(1);
    effect(ctx, &mut t, 1, e.mining_fatigue, 0, false);
    let clean = t.current_swing_duration(1);
    c.record(
        "d8.haste_and_mining_fatigue_adjust_the_duration",
        hasted == Some(5)
            && conduit == Some(4)
            && both == Some(4)
            && fatigued == Some(8)
            && clean == Some(6),
        format!(
            "haste_I={hasted:?} +conduit_II={conduit:?} +fatigue_I={both:?} \
             (dig speed wins) fatigue_only={fatigued:?} none={clean:?} — reachable only for \
             the local player and a ridden vehicle (see apply_swing_effect)"
        ),
    );
}

fn check_arms(c: &mut Checker, ctx: &Ctx) {
    let mut t = table(ctx);
    swing(ctx, &mut t, 1, 0);
    let (main_pose, _, _) = pose(EntityModelKind::Player, &t, 1, 1.0, 0.0, 0.0, 0.0, 0.0, &ctx.spears, None, Some(ctx.crossbow));
    c.record(
        "e1.a_main_hand_swing_uses_the_main_arm",
        !main_pose.left_arm,
        format!("attackArm left={} (mainArm defaults to RIGHT)", main_pose.left_arm),
    );
    swing(ctx, &mut t, 1, 3);
    let (off_pose, _, _) = pose(EntityModelKind::Player, &t, 1, 1.0, 0.0, 0.0, 0.0, 0.0, &ctx.spears, None, Some(ctx.crossbow));
    c.record(
        "e2.an_off_hand_swing_uses_the_opposite_arm",
        off_pose.left_arm,
        format!("attackArm left={}", off_pose.left_arm),
    );
    // A left-handed player (metadata index 15, HUMANOID_ARM) mirrors both.
    let mut t = table(ctx);
    let routed = route_set_entity_data(
        ctx.sed_id,
        &main_arm_body(1, 0), // 0 = LEFT
        &ctx.ids,
        &mut t,
        None,
    );
    let _ = &ctx.classes;
    swing(ctx, &mut t, 1, 0);
    let (lm, _, _) = pose(EntityModelKind::Player, &t, 1, 1.0, 0.0, 0.0, 0.0, 0.0, &ctx.spears, None, Some(ctx.crossbow));
    swing(ctx, &mut t, 1, 3);
    let (lo, _, _) = pose(EntityModelKind::Player, &t, 1, 1.0, 0.0, 0.0, 0.0, 0.0, &ctx.spears, None, Some(ctx.crossbow));
    c.record(
        "e3.the_main_arm_metadata_mirrors_both_hands",
        routed && lm.left_arm && !lo.left_arm,
        format!(
            "routed={routed} left-handed: main-hand→left={} off-hand→left={}",
            lm.left_arm, lo.left_arm
        ),
    );
}

/// Drive a swing to a chosen `attackTime` and return the production pose plus
/// the independent expectation.
///
/// The clock only produces sixths, so the checks pick a tick and grade the
/// exact value the production resolver reports — no value is invented here.
#[allow(clippy::too_many_arguments)]
fn posed(
    ctx: &Ctx,
    kind: EntityModelKind,
    hand_action: u8,
    stack: Option<Stack>,
    ticks: u32,
    alpha: f32,
    pitch: f32,
    limb: (f32, f32),
) -> (
    SwingPose,
    Vec<(&'static str, [f32; 3], [f32; 3])>,
    Expect,
) {
    posed_net(ctx, kind, hand_action, stack, ticks, alpha, pitch, 0.0, limb)
}

/// [`posed`] with an explicit net head yaw — the SPEAR hold pose is the only
/// stage that reads it.
#[allow(clippy::too_many_arguments)]
fn posed_net(
    ctx: &Ctx,
    kind: EntityModelKind,
    hand_action: u8,
    stack: Option<Stack>,
    ticks: u32,
    alpha: f32,
    pitch: f32,
    net: f32,
    limb: (f32, f32),
) -> (
    SwingPose,
    Vec<(&'static str, [f32; 3], [f32; 3])>,
    Expect,
) {
    let mut t = table(ctx);
    if let Some(s) = stack {
        equip(ctx, &mut t, 1, &[(0, s.clone()), (1, s)]);
    }
    swing(ctx, &mut t, 1, hand_action);
    for _ in 0..ticks {
        t.tick_lerp();
    }
    let (attack, poses, deltas) = pose(kind, &t, 1, alpha, pitch, net, limb.0, limb.1, &ctx.spears, Some(ctx.bow), Some(ctx.crossbow));
    let expect = expect_attack(
        attack.attack_time,
        attack.left_arm,
        attack.kind,
        attack.age_scale,
        pitch,
        limb.0,
        limb.1,
        // `OracleInputs::age_seconds` defaults to 0, so `ageInTicks` is 0 for
        // every posed case; the bob's own age dependence gets its own witness.
        0.0,
        poses,
        net,
    );
    (attack, deltas, expect)
}

fn check_pose(c: &mut Checker, ctx: &Ctx) {
    const TOL: f32 = 1e-6;
    // A mid-swing WHACK on the right arm, with a real walk pose + look pitch so
    // every term is exercised at once.
    let pitch = 0.35_f32;
    let limb = (7.5_f32, 0.8_f32);
    let (a, d, e) = posed(ctx, EntityModelKind::Player, 0, None, 3, 1.0, pitch, limb);
    let body = find(&d, "body").map(|(r, _)| r);
    c.record(
        "f1.body_yrot_is_exact",
        body.is_some_and(|r| near3(r, e.body, TOL)),
        format!(
            "attackTime={:.6} body={} want {}",
            a.attack_time,
            body.map(fmt3).unwrap_or_else(|| "<missing>".into()),
            fmt3(e.body)
        ),
    );
    // The same swing on the LEFT arm negates body.yRot.
    let (al, dl, el) = posed(ctx, EntityModelKind::Player, 3, None, 3, 1.0, pitch, limb);
    let body_l = find(&dl, "body").map(|(r, _)| r);
    c.record(
        "f2.body_yrot_negates_for_a_left_attack_arm",
        body_l.is_some_and(|r| near3(r, el.body, TOL))
            && al.left_arm
            && body_l.is_some_and(|r| near(r[1], -e.body[1], TOL))
            && e.body[1].abs() > 1e-3,
        format!(
            "left body={} want {} (right was {})",
            body_l.map(fmt3).unwrap_or_else(|| "<missing>".into()),
            fmt3(el.body),
            fmt3(e.body)
        ),
    );
    // Arm pivot displacement — the part vanilla *assigns* rather than adds.
    let right = find(&d, "right_arm");
    let left = find(&d, "left_arm");
    c.record(
        "f3.right_arm_pivot_offset_is_exact",
        right.is_some_and(|(_, o)| near3(o, e.right_off, 1e-5)),
        format!(
            "off={} want {}",
            right.map(|(_, o)| fmt3(o)).unwrap_or_else(|| "<missing>".into()),
            fmt3(e.right_off)
        ),
    );
    c.record(
        "f4.left_arm_pivot_offset_is_exact",
        left.is_some_and(|(_, o)| near3(o, e.left_off, 1e-5)),
        format!(
            "off={} want {}",
            left.map(|(_, o)| fmt3(o)).unwrap_or_else(|| "<missing>".into()),
            fmt3(e.left_off)
        ),
    );
    c.record(
        "f5.the_whack_strike_on_the_attack_arm_is_exact",
        right.is_some_and(|(r, _)| near3(r, e.right_rot, TOL)),
        format!(
            "right rot={} want {}",
            right.map(|(r, _)| fmt3(r)).unwrap_or_else(|| "<missing>".into()),
            fmt3(e.right_rot)
        ),
    );
    c.record(
        "f6.the_idle_arm_gets_only_the_shared_prologue",
        left.is_some_and(|(r, _)| near3(r, e.left_rot, TOL))
            && left.is_some_and(|(r, _)| near(r[2], bob_zrot(true, 0.0), TOL)),
        format!(
            "left rot={} want {} (zRot is the idle bob alone — no strike term)",
            left.map(|(r, _)| fmt3(r)).unwrap_or_else(|| "<missing>".into()),
            fmt3(e.left_rot)
        ),
    );
    // Sensitivity: the WHACK `bb` term reads the head's xRot, so changing the
    // look pitch must move the attack arm by exactly the predicted amount.
    let (_, d2, e2) = posed(ctx, EntityModelKind::Player, 0, None, 3, 1.0, -0.9, limb);
    let r2 = find(&d2, "right_arm").map(|(r, _)| r);
    let observed_delta = r2.map(|r| r[0] - right.unwrap().0[0]);
    let want_delta = e2.right_rot[0] - e.right_rot[0];
    c.record(
        "f7.the_head_pitch_enters_the_whack_bb_term",
        r2.is_some_and(|r| near3(r, e2.right_rot, TOL))
            && observed_delta.is_some_and(|o| near(o, want_delta, 1e-5))
            && want_delta.abs() > 1e-3,
        format!(
            "pitch 0.35→-0.9 moved right.xRot by {:+.6}, want {:+.6}",
            observed_delta.unwrap_or(f32::NAN),
            want_delta
        ),
    );
    // No swing → the pure walk pose, and no pivot displacement at all.
    let (a0, d0, e0) = posed(ctx, EntityModelKind::Player, 6, None, 3, 1.0, pitch, limb);
    let _ = &e0;
    let (rb, ro) = find(&d0, "right_arm").unwrap_or_default();
    let body0 = find(&d0, "body").map(|(r, _)| r).unwrap_or_default();
    c.record(
        "f8.no_swing_is_the_pure_walk_pose",
        a0.attack_time == 0.0
            && near3(rb, e0.right_rot, TOL)
            && near3(ro, [0.0; 3], TOL)
            && near3(body0, [0.0; 3], TOL),
        format!(
            "attackTime={} right rot={} off={} body={}",
            a0.attack_time,
            fmt3(rb),
            fmt3(ro),
            fmt3(body0)
        ),
    );
    // STAB: a spear in both hands. Its delegate undoes the shared yaw.
    let (astab, dstab, estab) = posed(
        ctx,
        EntityModelKind::Player,
        0,
        Some(Stack::Plain(ctx.spear)),
        3,
        1.0,
        pitch,
        limb,
    );
    let sr = find(&dstab, "right_arm");
    let sl = find(&dstab, "left_arm");
    // What the SPEAR hold pose alone leaves on the left arm, transcribed
    // independently. `thirdPersonAttackHand` subtracts exactly the prologue yaw
    // `setupAttackAnimation` added, so whatever the hold pose set survives the
    // strike untouched — which is why this is no longer "back to 0" on both
    // arms now that the hold stage exists.
    let mut hold_left = [0.0f32; 3];
    expect_pose_arm(
        &mut hold_left,
        ArmPoses {
            right: ArmPose::Spear,
            left: ArmPose::Spear,
            right_handed: true,
            known: true,
            using_item: false,
            main_hand_used: true,
            ticks_using_item: 0.0,
            max_crossbow_charge: 0.0,
        },
        ArmPose::Spear,
        true,
        true,
        0.0,
        pitch,
    );
    c.record(
        "g1.the_stab_strike_on_the_attack_arm_is_exact",
        astab.kind == SwingKind::Stab
            && sr.is_some_and(|(r, _)| near3(r, estab.right_rot, TOL)),
        format!(
            "kind={:?} attackTime={:.6} right rot={} want {}",
            astab.kind,
            astab.attack_time,
            sr.map(|(r, _)| fmt3(r)).unwrap_or_else(|| "<missing>".into()),
            fmt3(estab.right_rot)
        ),
    );
    c.record(
        "g2.stab_cancels_the_shared_yaw_leaving_the_hold_pose",
        sl.is_some_and(|(r, _)| near3(r, estab.left_rot, TOL))
            && sr.is_some_and(|(r, _)| near(r[1], 0.0, TOL))
            && sl.is_some_and(|(r, _)| near(r[1], hold_left[1], TOL))
            && hold_left[1].abs() > 1e-3
            && estab.body[1].abs() > 1e-3
            && sl.is_some_and(|(_, o)| !near3(o, [0.0; 3], 1e-4)),
        format!(
            "left rot={} want {}; right yRot back to exactly 0 — a swinging spear in \
             both hands is SPEAR/SPEAR, and affectsOffhandPose leaves the RIGHT arm \
             unposed, so the delegate's cancellation is visible bare there — while \
             the left keeps its hold yaw {:+.6}; body.yRot={:+.6}, pivot still moved",
            sl.map(|(r, _)| fmt3(r)).unwrap_or_else(|| "<missing>".into()),
            fmt3(estab.left_rot),
            hold_left[1],
            estab.body[1]
        ),
    );
    // NONE: the prologue only — no strike on either arm.
    let (anone, dnone, enone) = posed(
        ctx,
        EntityModelKind::Player,
        0,
        Some(Stack::Override(ctx.sword, 0, 6)),
        3,
        1.0,
        pitch,
        limb,
    );
    let nr = find(&dnone, "right_arm");
    c.record(
        "g3.the_none_swing_type_has_no_strike",
        anone.kind == SwingKind::None
            && nr.is_some_and(|(r, _)| near3(r, enone.right_rot, TOL))
            && nr.is_some_and(|(r, _)| near(r[2], bob_zrot(false, 0.0), TOL))
            && nr.is_some_and(|(_, o)| !near3(o, [0.0; 3], 1e-4)),
        format!(
            "kind={:?} right rot={} want {} (zRot is the idle bob alone, but the pivot              still moved)",
            anone.kind,
            nr.map(|(r, _)| fmt3(r)).unwrap_or_else(|| "<missing>".into()),
            fmt3(enone.right_rot)
        ),
    );
}

fn check_integration(c: &mut Checker, ctx: &Ctx) {
    // A mob model must not pose from `attackTime` — M19 models the humanoid
    // player rig only, and every other vanilla `HumanoidModel` subclass
    // overrides the arms in ways this client does not implement.
    let mut t = table(ctx);
    swing(ctx, &mut t, 1, 0);
    t.tick_lerp();
    t.tick_lerp();
    t.tick_lerp();
    let attack = crate::live_cmd::resolve_attack_anim(&t, 1, 1.0);
    let zombie = oracle_part_deltas(
        EntityModelKind::Zombie,
        &OracleInputs {
            attack,
            ..Default::default()
        },
    )
    .unwrap_or_default();
    let neutral = oracle_part_deltas(EntityModelKind::Zombie, &OracleInputs::default())
        .unwrap_or_default();
    let same = zombie.len() == neutral.len()
        && zombie
            .iter()
            .zip(&neutral)
            .all(|(a, b)| near3(a.1, b.1, 0.0) && near3(a.2, b.2, 0.0));
    // M19 asserted the opposite here: with the undead arms baked as static
    // folds, a swinging zombie was inert. M20 gives it the real
    // `animateZombieArms` rig, so the claim is now that it animates and does so
    // with a *different* rig than the player's — the humanoid strike is not
    // simply reused.
    let player = oracle_part_deltas(
        EntityModelKind::Player,
        &OracleInputs {
            attack,
            ..Default::default()
        },
    )
    .unwrap_or_default();
    let z_arm = find(&zombie, "right_arm").map(|(r, _)| r);
    let p_arm = find(&player, "right_arm").map(|(r, _)| r);
    c.record(
        "h1.the_undead_rig_animates_and_differs_from_the_player_rig",
        attack.attack_time > 0.0
            && !same
            && z_arm.is_some()
            && z_arm.zip(p_arm).is_some_and(|(z, p)| !near3(z, p, 1e-3)),
        format!(
            "attackTime={:.6}: zombie right_arm={} vs player right_arm={}; any zombie              part moved from neutral: {}",
            attack.attack_time,
            z_arm.map(fmt3).unwrap_or_default(),
            p_arm.map(fmt3).unwrap_or_default(),
            !same,
        ),
    );
    // The CEM path: the production `AnimContext` builder must publish the same
    // `attackTime` as `swing_progress`, and `limb_swing_amount` must be the
    // walk amplitude (it aliased `swing_progress` until M19, when that slot
    // stopped being permanently zero).
    let draw = neutral_draw(EntityModelKind::Player, attack);
    let frame = CemFrameInputs {
        frame_time: 0.05,
        frame_counter: 1.0,
        age_seconds: 0.0,
        cam_pos: [0.0; 3],
    };
    let mut actx = rewo_gpu::entities::cem_anim_context(&draw, frame, Vec::new());
    let published = actx.swing_progress;
    // …and prove a program actually reads it, through the real interpreter.
    let jem = r#"{"models":[
        {"part":"body","model":"a.jpm","translate":[0,0,0],
         "boxes":[{"coordinates":[-4,0,-2,8,12,4],"textureOffset":[16,16]}]}]}"#;
    let mut jpms = std::collections::HashMap::new();
    jpms.insert(
        "a.jpm".to_string(),
        r#"{"animations":[{"var.s":"swing_progress","var.w":"limb_swing_amount"}]}"#.to_string(),
    );
    let read = rewo_gpu::cem::model_from_jem(jem, &jpms)
        .ok()
        .and_then(|m| m.cem.map(|prog| (prog, m.parts.len(), m.cem_translate.clone())))
        .map(|(prog, parts, tr)| {
            rewo_gpu::cem::eval_program(&prog, &mut actx, parts, &tr);
            (
                actx.user.first().copied().unwrap_or(f32::NAN),
                actx.user.get(1).copied().unwrap_or(f32::NAN),
            )
        });
    c.record(
        "h2.cem_swing_progress_is_the_live_attack_time",
        near(published, attack.attack_time, 0.0)
            && published > 0.0
            && read.is_some_and(|(s, w)| near(s, attack.attack_time, 0.0) && near(w, 0.8, 0.0)),
        format!(
            "AnimContext.swing_progress={published:.6} (attackTime={:.6}); program read \
             swing_progress/limb_swing_amount = {:?}",
            attack.attack_time, read
        ),
    );
    // The production port evaluates the attack's trigonometry with plain `f32`
    // sin/cos while vanilla samples `Mth`'s 65,536-entry table. Bound that
    // deviation rather than leave it unstated.
    // The production port must reproduce `Mth`'s *quantization*, not merely
    // come close to a true sine. Two independent table constructions (this
    // module's, built with std's `sin`; the renderer's, built with `libm`) are
    // compared bit-for-bit over the swing domain — and the same points are
    // compared against the platform sine, which must DISAGREE, or the witness
    // would pass just as well on the plain-trig implementation it replaced.
    let mut mismatches = 0usize;
    let mut differs_from_libm = 0usize;
    let mut worst_vs_plain = 0.0f32;
    for i in 0..=20_000 {
        let t = i as f32 / 20_000.0;
        for x in [
            mth_sqrt(t) * std::f32::consts::TAU,
            t * std::f32::consts::PI,
        ] {
            let ours = mth_sin(x as f64);
            let theirs = rewo_gpu::entities::mth_sin(x as f64);
            if ours.to_bits() != theirs.to_bits() {
                mismatches += 1;
            }
            let plain = x.sin();
            if plain.to_bits() != theirs.to_bits() {
                differs_from_libm += 1;
                worst_vs_plain = worst_vs_plain.max((plain - theirs).abs());
            }
            let c_ours = mth_cos(x as f64);
            if c_ours.to_bits() != rewo_gpu::entities::mth_cos(x as f64).to_bits() {
                mismatches += 1;
            }
        }
    }
    c.record(
        "h3.the_production_trig_is_mths_quantized_table_bit_for_bit",
        mismatches == 0 && differs_from_libm > 0,
        format!(
            "0 bit mismatches over 60,003 sin/cos samples ({mismatches} found); the same \
             points differ from the platform sine {differs_from_libm} times (max \
             {worst_vs_plain:.4e}), so a plain-trig port would fail this"
        ),
    );
    // `bobModelPart` is applied unconditionally to both arms, including when
    // nothing is swinging — its own witness, with an age sensitivity partner.
    let bob_expect = |age: f32, left: bool| -> [f32; 3] {
        let scale = if left { -1.0f32 } else { 1.0 };
        [
            scale * (mth_sin((age * 0.067) as f64) * 0.05),
            0.0,
            scale * (mth_cos((age * 0.09) as f64) * 0.05 + 0.05),
        ]
    };
    let bob_at = |age_seconds: f32| {
        oracle_part_deltas(
            EntityModelKind::Player,
            &OracleInputs {
                age_seconds,
                ..Default::default()
            },
        )
        .unwrap_or_default()
    };
    let at0 = bob_at(0.0);
    let (r0, l0) = (find(&at0, "right_arm"), find(&at0, "left_arm"));
    // 1.7 s = 34 ageInTicks — far enough that both terms have moved.
    let at1 = bob_at(1.7);
    let r1 = find(&at1, "right_arm");
    let (w_r0, w_l0, w_r1) = (
        bob_expect(0.0, false),
        bob_expect(0.0, true),
        bob_expect(34.0, false),
    );
    c.record(
        "h4.the_arm_bob_is_exact_and_age_driven",
        r0.is_some_and(|(r, _)| near3(r, w_r0, 0.0))
            && l0.is_some_and(|(r, _)| near3(r, w_l0, 0.0))
            && r1.is_some_and(|(r, _)| near3(r, w_r1, 0.0))
            && !near3(w_r0, w_r1, 1e-4)
            && near(w_l0[2], -w_r0[2], 0.0),
        format!(
            "idle right={} want {}; left={} want {} (scale −1); at ageInTicks 34 right={} \
             want {}",
            r0.map(|(r, _)| fmt3(r)).unwrap_or_else(|| "<missing>".into()),
            fmt3(w_r0),
            l0.map(|(r, _)| fmt3(r)).unwrap_or_else(|| "<missing>".into()),
            fmt3(w_l0),
            r1.map(|(r, _)| fmt3(r)).unwrap_or_else(|| "<missing>".into()),
            fmt3(w_r1)
        ),
    );
}

/// `HumanoidModel.ArmPose` — the *hold* baseline `pose{Right,Left}Arm` writes
/// before `setupAttackAnimation`.
///
/// This is the stage M19's first cut omitted entirely, which left every armed
/// entity posed from an unarmed baseline. `ITEM` is the fall-through for any
/// ordinary held item, so the omission hit the ordinary combat case rather than
/// an exotic one — hence the sensitivity partners here compare against the
/// *unposed* arm rather than only against a formula.
fn check_arm_poses(c: &mut Checker, ctx: &Ctx) {
    const TOL: f32 = 1e-6;
    let pitch = 0.35_f32;
    let limb = (7.5_f32, 0.8_f32);

    // --- the tag itself ---------------------------------------------------
    c.record(
        "j1.the_spears_tag_loads_from_the_client_jar",
        ctx.spears.len() == 7 && ctx.spears.contains(ctx.spear) && !ctx.spears.contains(ctx.sword),
        format!(
            "minecraft:spears = {} item(s); iron_spear in={} stone_sword in={}",
            ctx.spears.len(),
            ctx.spears.contains(ctx.spear),
            ctx.spears.contains(ctx.sword),
        ),
    );

    // --- EMPTY ------------------------------------------------------------
    let (_, d, e) = posed(ctx, EntityModelKind::Player, 255, None, 0, 1.0, pitch, limb);
    let right = find(&d, "right_arm").map(|(r, _)| r);
    c.record(
        "j2.an_empty_hand_poses_empty",
        right.is_some_and(|r| near3(r, e.right_rot, TOL)),
        format!(
            "bare hands, no swing: right={} want={}",
            right.map(fmt3).unwrap_or_default(),
            fmt3(e.right_rot)
        ),
    );

    // --- ITEM: the common case -------------------------------------------
    let (_, di, ei) = posed(
        ctx,
        EntityModelKind::Player,
        255,
        Some(Stack::Plain(ctx.sword)),
        0,
        1.0,
        pitch,
        limb,
    );
    let ri = find(&di, "right_arm").map(|(r, _)| r);
    let unposed = right.map(|r| r[0]).unwrap_or(0.0);
    let moved = ri.map(|r| r[0]).unwrap_or(0.0);
    c.record(
        "j3.an_ordinary_held_item_poses_item",
        ri.is_some_and(|r| near3(r, ei.right_rot, TOL))
            && near(moved, unposed * 0.5 - std::f32::consts::PI / 10.0, TOL)
            && ei.right == ArmPose::Item,
        format!(
            "sword: right={} want={} — xRot {:+.6}, exactly baseline/2 - pi/10 from the unarmed {:+.6}",
            ri.map(fmt3).unwrap_or_default(),
            fmt3(ei.right_rot),
            moved - unposed,
            unposed,
        ),
    );

    // --- ITEM under a live swing: ordering --------------------------------
    let (asw, dsw, esw) = posed(
        ctx,
        EntityModelKind::Player,
        0,
        Some(Stack::Plain(ctx.sword)),
        3,
        1.0,
        pitch,
        limb,
    );
    let rsw = find(&dsw, "right_arm").map(|(r, _)| r);
    let (_, dbare, _) = posed(ctx, EntityModelKind::Player, 0, None, 3, 1.0, pitch, limb);
    let rbare = find(&dbare, "right_arm").map(|(r, _)| r);
    c.record(
        "j4.the_hold_pose_is_applied_before_the_attack",
        rsw.is_some_and(|r| near3(r, esw.right_rot, TOL))
            && rsw.zip(rbare).is_some_and(|(a, b)| {
                // The whole difference is the ITEM transform on the walk term;
                // the strike itself is identical in both.
                near(a[0] - b[0], -unposed * 0.5 - std::f32::consts::PI / 10.0, TOL)
            }),
        format!(
            "mid-swing attackTime={:.6}: armed right={} want={}; unarmed right={} (the strike is added to the hold pose, not instead of it)",
            asw.attack_time,
            rsw.map(fmt3).unwrap_or_default(),
            fmt3(esw.right_rot),
            rbare.map(fmt3).unwrap_or_default(),
        ),
    );

    // --- SPEAR by tag, while NOT swinging ---------------------------------
    // Spear in the MAIN hand and an EMPTY off hand. `posed` equips both hands,
    // which would put a spear in the off hand too — and `affectsOffhandPose`
    // would then leave the right arm unposed (that is j7). To exercise the
    // SPEAR *math* the off hand has to be something that does not claim it.
    let net = 0.4_f32;
    let mut tsp = table(ctx);
    equip(
        ctx,
        &mut tsp,
        1,
        &[(0, Stack::Plain(ctx.spear)), (1, Stack::Empty)],
    );
    let (_, sposes, dsp) = pose(
        EntityModelKind::Player,
        &tsp,
        1,
        1.0,
        pitch,
        net,
        limb.0,
        limb.1,
        &ctx.spears,
        None,
        Some(ctx.crossbow),
    );
    let esp = expect_attack(
        0.0,
        false,
        SwingKind::Whack,
        1.0,
        pitch,
        limb.0,
        limb.1,
        0.0,
        sposes,
        net,
    );
    let rsp = find(&dsp, "right_arm").map(|(r, _)| r);
    c.record(
        "j5.a_held_spear_poses_spear_from_the_tag",
        sposes.right == ArmPose::Spear
            && want_poses_arm(sposes, false)
            && rsp.is_some_and(|r| near3(r, esp.right_rot, TOL))
            && rsp.zip(right).is_some_and(|(a, b)| (a[0] - b[0]).abs() > 0.1),
        format!(
            "spear main / empty off, not swinging -> {:?}: right={} want={} (head yaw {} and pitch {} both enter it); unposed baseline was {}",
            sposes.right,
            rsp.map(fmt3).unwrap_or_default(),
            fmt3(esp.right_rot),
            net,
            pitch,
            right.map(fmt3).unwrap_or_default(),
        ),
    );

    // --- the tag is the predicate, not the swing component ----------------
    // `stone_sword` with an explicit STAB/11 `swing_animation` patch
    // (kind byte 2 = `SwingAnimationType::Stab`).
    let stab_sword = Stack::Override(ctx.sword, 2, 11);
    let (_, _, est) = posed(
        ctx,
        EntityModelKind::Player,
        255,
        Some(stab_sword.clone()),
        0,
        1.0,
        pitch,
        limb,
    );
    let (_, _, esw2) = posed(
        ctx,
        EntityModelKind::Player,
        0,
        Some(stab_sword),
        1,
        1.0,
        pitch,
        limb,
    );
    c.record(
        "j6.stab_poses_spear_only_while_swinging_the_tag_covers_the_rest",
        est.right == ArmPose::Item && esw2.right == ArmPose::Spear,
        format!(
            "stone_sword patched STAB: idle -> {:?} (not in minecraft:spears), swinging -> {:?}",
            est.right, esw2.right
        ),
    );

    // --- affectsOffhandPose: a spear stops the OTHER arm being posed ------
    let mut t = table(ctx);
    equip(
        ctx,
        &mut t,
        1,
        &[(0, Stack::Plain(ctx.sword)), (1, Stack::Plain(ctx.spear))],
    );
    let (_, poses, d2) = pose(
        EntityModelKind::Player,
        &t,
        1,
        1.0,
        pitch,
        0.0,
        limb.0,
        limb.1,
        &ctx.spears,
        None,
        Some(ctx.crossbow),
    );
    let r2 = find(&d2, "right_arm").map(|(r, _)| r);
    c.record(
        "j7.a_spear_in_the_offhand_stops_the_main_arm_being_posed",
        poses.left == ArmPose::Spear
            && poses.right == ArmPose::Item
            && !want_poses_arm(poses, false)
            && want_poses_arm(poses, true)
            && r2.zip(right).is_some_and(|(a, b)| near3(a, b, TOL)),
        format!(
            "right-handed, sword main + spear off: right={:?} left={:?}; right posed={} left posed={} — right={} equals the UNPOSED baseline {}",
            poses.right,
            poses.left,
            want_poses_arm(poses, false),
            want_poses_arm(poses, true),
            r2.map(fmt3).unwrap_or_default(),
            right.map(fmt3).unwrap_or_default(),
        ),
    );

    // --- handedness flips which arm the dispatch protects -----------------
    let mut tl = table(ctx);
    tl.set_main_arm(1, rewo_world::entities::HumanoidArm::Left);
    equip(
        ctx,
        &mut tl,
        1,
        &[(0, Stack::Plain(ctx.sword)), (1, Stack::Plain(ctx.spear))],
    );
    let (_, lposes, _) = pose(
        EntityModelKind::Player,
        &tl,
        1,
        1.0,
        pitch,
        0.0,
        limb.0,
        limb.1,
        &ctx.spears,
        None,
        Some(ctx.crossbow),
    );
    c.record(
        "j8.handedness_selects_which_arm_the_dispatch_protects",
        lposes.left == ArmPose::Item
            && lposes.right == ArmPose::Spear
            && want_poses_arm(lposes, false)
            && !want_poses_arm(lposes, true),
        format!(
            "left-handed, same two stacks: right={:?} left={:?}; right posed={} left posed={} (mirror of j7)",
            lposes.right,
            lposes.left,
            want_poses_arm(lposes, false),
            want_poses_arm(lposes, true),
        ),
    );

    // --- the SPEAR clamps -------------------------------------------------
    // Extreme look angles, main-hand spear again so the pose actually applies.
    let (cl_pitch, cl_net) = (-1.5_f32, 2.0_f32);
    let (_, cposes, dcl) = pose(
        EntityModelKind::Player,
        &tsp,
        1,
        1.0,
        cl_pitch,
        cl_net,
        limb.0,
        limb.1,
        &ctx.spears,
        None,
        Some(ctx.crossbow),
    );
    let ecl = expect_attack(
        0.0,
        false,
        SwingKind::Whack,
        1.0,
        cl_pitch,
        limb.0,
        limb.1,
        0.0,
        cposes,
        cl_net,
    );
    let rcl = find(&dcl, "right_arm").map(|(r, _)| r);
    let deg = |r: f32| r / DEG;
    c.record(
        "j9.the_spear_pose_clamps_to_vanilla_degree_limits",
        rcl.is_some_and(|r| near3(r, ecl.right_rot, TOL))
            // yRot wants -0.1 + 2.0 = 1.9 rad = 108.9 deg, capped at 60.
            && rcl.is_some_and(|r| near(deg(r[1]), 60.0, 1e-3))
            // xRot wants -pi/2 - 1.5 + 0.8 = -2.27 rad = -130.1 deg, floored at -120.
            && rcl.is_some_and(|r| near(deg(r[0]), -120.0, 1e-3)),
        format!(
            "head yaw {} rad / pitch {} rad: right={} want={} -> yRot {:.3} deg (wanted 108.9, cap 60), xRot {:.3} deg (wanted -130.1, floor -120)",
            cl_net,
            cl_pitch,
            rcl.map(fmt3).unwrap_or_default(),
            fmt3(ecl.right_rot),
            rcl.map(|r| deg(r[1])).unwrap_or(0.0),
            rcl.map(|r| deg(r[0])).unwrap_or(0.0),
        ),
    );

    // --- an unknowable hand suppresses the whole baseline -----------------
    let mut tu = table(ctx);
    equip(
        ctx,
        &mut tu,
        1,
        &[(0, Stack::Plain(ctx.unregistered_item)), (1, Stack::Empty)],
    );
    let (_, uposes, du) = pose(
        EntityModelKind::Player,
        &tu,
        1,
        1.0,
        pitch,
        0.0,
        limb.0,
        limb.1,
        &ctx.spears,
        None,
        Some(ctx.crossbow),
    );
    let ru = find(&du, "right_arm").map(|(r, _)| r);
    c.record(
        "j10.an_unknowable_hand_suppresses_the_hold_pose",
        !uposes.known
            && !want_poses_arm(uposes, false)
            && !want_poses_arm(uposes, true)
            && ru.zip(right).is_some_and(|(a, b)| near3(a, b, TOL)),
        format!(
            "unregistered item {} -> known={}, neither arm posed; right={} equals the unposed baseline {} (ArmPose::Item would be a guess)",
            ctx.unregistered_item,
            uposes.known,
            ru.map(fmt3).unwrap_or_default(),
            right.map(fmt3).unwrap_or_default(),
        ),
    );
}

// ---- M20: the mob combat rigs -------------------------------------------
//
// Independent transcriptions again: `AnimationUtils.animateAttackArms`,
// `SkeletonModel.setupAnim`'s override and `AnimationUtils.swingWeaponDown`
// are hand-ported here with the oracle's own `Mth` table. Nothing reads the
// production `animate_zombie_arms` / `skeleton_attack_arm` / `swing_weapon_down`
// as its expectation.

/// `AnimationUtils.animateAttackArms` for one arm. Vanilla **assigns** all
/// three rotations, so this is the whole rotation, not a delta.
fn want_attack_arms(left: bool, t: f32, negate: bool, arm_drop: f32) -> [f32; 3] {
    use std::f32::consts::PI;
    let a_y = (if negate { 1.0f32 } else { -1.0 }) * mth_sin((t * PI) as f64);
    let inv = 1.0 - t;
    let a_x = mth_sin(((1.0 - inv * inv) * PI) as f64);
    let x = arm_drop + a_y * 1.2 - a_x * 0.4;
    let y = 0.1 - a_y * 0.6;
    [x, if negate == left { y } else { -y }, 0.0]
}

/// `animateZombieArms`'s non-STAB branch, plus its `bobArms` call.
fn want_zombie_arms(
    left: bool,
    t: f32,
    age: f32,
    aggressive: bool,
    is_baby: bool,
    main_hand_empty: bool,
) -> [f32; 3] {
    use std::f32::consts::PI;
    let raise = !is_baby || main_hand_empty;
    let arm_drop = if raise {
        -PI / if aggressive { 1.5 } else { 2.25 }
    } else {
        0.0
    };
    let mut r = want_attack_arms(left, t, raise, arm_drop);
    let scale = if left { -1.0f32 } else { 1.0 };
    r[2] += scale * (mth_cos((age * 0.09) as f64) * 0.05 + 0.05);
    r[0] += scale * (mth_sin((age * 0.067) as f64) * 0.05);
    r
}

/// `SkeletonModel.setupAnim`'s override, plus its `bobArms` call.
fn want_skeleton_arms(left: bool, t: f32, age: f32) -> [f32; 3] {
    use std::f32::consts::{FRAC_PI_2, PI};
    let attack2 = mth_sin((t * PI) as f64);
    let inv = 1.0 - t;
    let attack = mth_sin(((1.0 - inv * inv) * PI) as f64);
    let y = 0.1 - attack2 * 0.6;
    let mut r = [
        -FRAC_PI_2 - (attack2 * 1.2 - attack * 0.4),
        if left { y } else { -y },
        0.0,
    ];
    let scale = if left { -1.0f32 } else { 1.0 };
    r[2] += scale * (mth_cos((age * 0.09) as f64) * 0.05 + 0.05);
    r[0] += scale * (mth_sin((age * 0.067) as f64) * 0.05);
    r
}

/// `AnimationUtils.swingWeaponDown` for one arm.
fn want_swing_weapon_down(left: bool, main_arm_left: bool, t: f32, age: f32) -> [f32; 3] {
    use std::f32::consts::PI;
    let attack2 = mth_sin((t * PI) as f64);
    let attack = mth_sin(((1.0 - (1.0 - t) * (1.0 - t)) * PI) as f64);
    let x = if left == main_arm_left {
        -1.8849558 + mth_cos((age * 0.09) as f64) * 0.15 + (attack2 * 2.2 - attack * 0.4)
    } else {
        -0.0 + mth_cos((age * 0.19) as f64) * 0.5 + (attack2 * 1.2 - attack * 0.4)
    };
    [x, if left { -PI / 20.0 } else { PI / 20.0 }, 0.0]
}

/// A metadata delta body: entity id, then one `(index, serializer, value)`
/// entry, then the 0xFF terminator.
fn meta_body(eid: i32, index: u8, serializer: i32, value: &[u8]) -> Vec<u8> {
    let mut b = Vec::new();
    varint(eid, &mut b);
    b.push(index);
    varint(serializer, &mut b);
    b.extend_from_slice(value);
    b.push(0xFF);
    b
}

/// Drive one metadata entry through the production router with M20's kind map.
fn meta(ctx: &Ctx, t: &mut EntityTable, eid: i32, index: u8, ser: i32, value: &[u8]) -> bool {
    rewo_net::route_set_entity_data(
        ctx.sed_id,
        &meta_body(eid, index, ser, value),
        &ctx.ids,
        t,
        rewo_net::MetaKinds {
            allay: None,
            pillager: Some(ctx.pillager_tid),
            classes: Some(&ctx.classes),
        },
    )
}

/// M23 — item-use state: the derived clock, and the eight arm poses it gates.
///
/// The clock witnesses drive `set_entity_data` through the production router,
/// so they prove the index-8 decode, the kind gate and
/// `onSyncedDataUpdated`'s branch shape together. The pose witnesses drive the
/// production resolver and compare against this file's independent
/// transcription of `poseRightArm` / `poseLeftArm`.
fn check_item_use(c: &mut Checker, ctx: &Ctx) {
    const TOL: f32 = 1e-6;
    let pitch = 0.35_f32;
    let limb = (7.5_f32, 0.8_f32);

    // `DATA_LIVING_ENTITY_FLAGS` bit 1 = using, bit 2 = off-hand.
    let using_main = 1u8;
    let using_off = 3u8;

    // --- the clock --------------------------------------------------------
    let mut t = table(ctx);
    equip(ctx, &mut t, 1, &[(0, Stack::Plain(ctx.shield))]);
    meta(ctx, &mut t, 1, 8, 0, &[using_main]);
    let latched = t.use_state(1);
    c.record(
        "m1.the_using_bit_latches_the_duration_from_the_held_item",
        latched.using
            && latched.remaining == 72000
            && latched.duration == 72000
            && latched.hand == rewo_world::entities::InteractionHand::MainHand,
        format!(
            "shield + flags 0b1 -> using={} hand={:?} remaining={} duration={} \
             (Item.getUseDuration's BLOCKS_ATTACKS branch, 72000)",
            latched.using, latched.hand, latched.remaining, latched.duration
        ),
    );

    t.tick_lerp();
    t.tick_lerp();
    t.tick_lerp();
    let ticked = t.use_state(1);
    c.record(
        "m2.each_tick_decrements_the_remaining_count",
        ticked.remaining == 72000 - 3 && ticked.ticks_using_item() == 3,
        format!(
            "after 3 ticks remaining={} (want 71997), getTicksUsingItem()={} (want 3)",
            ticked.remaining,
            ticked.ticks_using_item()
        ),
    );

    // A repeated `true` must NOT restart: `useItem` is no longer empty, so
    // `onSyncedDataUpdated`'s first branch does not run.
    meta(ctx, &mut t, 1, 8, 0, &[using_main]);
    let repeated = t.use_state(1);
    c.record(
        "m3.a_repeated_using_flag_does_not_restart_the_clock",
        repeated.remaining == 72000 - 3,
        format!(
            "remaining={} after re-sending the same flags (want 71997, NOT 72000) — \
             the branch is guarded on useItem.isEmpty(), not on the flag changing",
            repeated.remaining
        ),
    );

    meta(ctx, &mut t, 1, 8, 0, &[0]);
    let cleared = t.use_state(1);
    c.record(
        "m4.clearing_the_flag_empties_the_use_item_and_zeroes_the_clock",
        !cleared.using && cleared.remaining == 0 && cleared.item_id.is_none(),
        format!(
            "using={} remaining={} item={:?}",
            cleared.using, cleared.remaining, cleared.item_id
        ),
    );

    // A hand swap mid-use: `updatingUsingItem` compares the *current* hand item
    // against `useItem` each tick and stops using when they differ.
    let mut t2 = table(ctx);
    equip(ctx, &mut t2, 1, &[(0, Stack::Plain(ctx.shield))]);
    meta(ctx, &mut t2, 1, 8, 0, &[using_main]);
    equip(ctx, &mut t2, 1, &[(0, Stack::Plain(ctx.sword))]);
    t2.tick_lerp();
    let swapped = t2.use_state(1);
    c.record(
        "m5.swapping_the_held_item_mid_use_stops_the_use",
        swapped.remaining == 0 && swapped.item_id.is_none() && swapped.using,
        format!(
            "after swapping shield->sword and ticking: remaining={} item={:?} using={} — \
             stopUsingItem clears useItem but NOT the flag on a client",
            swapped.remaining, swapped.item_id, swapped.using
        ),
    );

    // Index 8 is the first slot a direct `Entity` subclass may claim, so the
    // serializer alone cannot disambiguate it — a boat must be inert.
    let mut t3 = table(ctx);
    meta(ctx, &mut t3, 5, 8, 0, &[using_main]);
    c.record(
        "m6.a_non_living_entity_never_takes_the_index_8_byte",
        !t3.use_state(5).using,
        format!(
            "boat sent flags 0b1 -> using={} (Entity owns 0..7, so an AbstractArrow \
             puts its own byte at 8 and it does not mean 'using an item')",
            t3.use_state(5).using
        ),
    );

    // Wrong index / wrong serializer must both be inert — the mutation
    // partners for m1.
    let mut t4 = table(ctx);
    equip(ctx, &mut t4, 1, &[(0, Stack::Plain(ctx.shield))]);
    meta(ctx, &mut t4, 1, 9, 0, &[using_main]);
    let wrong_index = t4.use_state(1).using;
    meta(ctx, &mut t4, 1, 8, 8, &[1]);
    let wrong_ser = t4.use_state(1).using;
    c.record(
        "m7.the_wrong_index_or_serializer_sets_nothing",
        !wrong_index && !wrong_ser,
        format!(
            "index 9 BYTE -> using={}; index 8 BOOLEAN -> using={} (both must be false)",
            wrong_index, wrong_ser
        ),
    );

    // --- the poses --------------------------------------------------------
    // Each item reaches its pose only while the use gate is open. `used`
    // returns the resolved poses plus the rendered + expected right-arm rot.
    let used = |item: i32, flags: u8| -> (ArmPoses, Option<[f32; 3]>, Expect) {
        let mut t = table(ctx);
        equip(ctx, &mut t, 1, &[(0, Stack::Plain(item))]);
        if flags != 0 {
            meta(ctx, &mut t, 1, 8, 0, &[flags]);
        }
        let (attack, poses, deltas) =
            pose(EntityModelKind::Player, &t, 1, 1.0, pitch, 0.0, limb.0, limb.1,
                 &ctx.spears, Some(ctx.bow), Some(ctx.crossbow));
        let e = expect_attack(
            attack.attack_time, attack.left_arm, attack.kind, attack.age_scale,
            pitch, limb.0, limb.1, 0.0, poses, 0.0,
        );
        let r = find(&deltas, "right_arm").map(|(r, _)| r);
        (poses, r, e)
    };

    let cases: [(&str, i32, ArmPose); 6] = [
        ("shield", ctx.shield, ArmPose::Block),
        ("bow", ctx.bow, ArmPose::BowAndArrow),
        ("trident", ctx.trident, ArmPose::ThrowTrident),
        ("spyglass", ctx.spyglass, ArmPose::Spyglass),
        ("goat_horn", ctx.goat_horn, ArmPose::TootHorn),
        ("brush", ctx.brush, ArmPose::Brush),
    ];
    let mut all_selected = true;
    let mut all_exact = true;
    let mut detail = Vec::new();
    for (name, item, want) in cases {
        let (poses, got, e) = used(item, using_main);
        let selected = poses.right == want;
        let exact = got.is_some_and(|r| near3(r, e.right_rot, TOL));
        // The pose must also actually MOVE the arm relative to the same item
        // held with the gate shut — otherwise "selected" proves nothing.
        let (_, idle, _) = used(item, 0);
        let moved = match (got, idle) {
            (Some(a), Some(b)) => !near3(a, b, 1e-4),
            _ => false,
        };
        all_selected &= selected && moved;
        all_exact &= exact;
        detail.push(format!(
            "{name}->{:?}{} rot={} want={}",
            poses.right,
            if moved { "" } else { " (INERT!)" },
            got.map(fmt3).unwrap_or_default(),
            fmt3(e.right_rot)
        ));
    }
    c.record(
        "m8.each_use_animation_selects_and_moves_its_arm_pose",
        all_selected,
        format!("with the use gate open: {}", detail.join("; ")),
    );
    c.record(
        "m9.every_use_driven_pose_body_is_exact",
        all_exact,
        format!("rendered vs independently transcribed: {}", detail.join("; ")),
    );

    // The gate is what makes the difference: the same shield with the flag
    // clear must pose ITEM, not BLOCK.
    let (shut, _, _) = used(ctx.shield, 0);
    c.record(
        "m10.without_the_use_flag_the_same_item_poses_item",
        shut.right == ArmPose::Item,
        format!(
            "shield, flags 0 -> {:?} (want Item) — the eight poses are gated on \
             getUsedItemHand() == hand && getUseItemRemainingTicks() > 0",
            shut.right
        ),
    );

    // An EAT item is used but has no `ArmPose` case, so it falls through the
    // switch to the ITEM tail rather than being suppressed.
    let (eaten, _, _) = used(ctx.apple, using_main);
    c.record(
        "m11.an_eaten_item_falls_through_the_switch_to_item",
        eaten.right == ArmPose::Item && eaten.using_item,
        format!(
            "apple + using -> {:?}, isUsingItem={} (EAT/DRINK/BUNDLE/NONE have no \
             case in getArmPose's switch)",
            eaten.right, eaten.using_item
        ),
    );

    // The used hand selects which arm poses: an off-hand shield on a
    // right-handed player poses the LEFT arm and leaves the right alone.
    let (off, _, _) = {
        let mut t = table(ctx);
        equip(ctx, &mut t, 1, &[(1, Stack::Plain(ctx.shield))]);
        meta(ctx, &mut t, 1, 8, 0, &[using_off]);
        let (attack, poses, deltas) =
            pose(EntityModelKind::Player, &t, 1, 1.0, pitch, 0.0, limb.0, limb.1,
                 &ctx.spears, Some(ctx.bow), Some(ctx.crossbow));
        let _ = attack;
        (poses, deltas, ())
    };
    c.record(
        "m12.the_used_hand_decides_which_arm_takes_the_pose",
        off.left == ArmPose::Block && off.right == ArmPose::Empty && !off.main_hand_used,
        format!(
            "off-hand shield on a right-handed player -> right={:?} left={:?} \
             mainHandUsed={}",
            off.right, off.left, off.main_hand_used
        ),
    );

    // --- both-arm poses ---------------------------------------------------
    // BOW_AND_ARROW writes the arm it was NOT called for. Proving that needs
    // the *other* arm to differ from what it would be with the bow gate shut.
    let bow_open = {
        let mut t = table(ctx);
        equip(ctx, &mut t, 1, &[(0, Stack::Plain(ctx.bow))]);
        meta(ctx, &mut t, 1, 8, 0, &[using_main]);
        let (attack, poses, deltas) =
            pose(EntityModelKind::Player, &t, 1, 1.0, pitch, 0.0, limb.0, limb.1,
                 &ctx.spears, Some(ctx.bow), Some(ctx.crossbow));
        let e = expect_attack(
            attack.attack_time, attack.left_arm, attack.kind, attack.age_scale,
            pitch, limb.0, limb.1, 0.0, poses, 0.0,
        );
        (poses, find(&deltas, "left_arm").map(|(r, _)| r), e)
    };
    c.record(
        "m13.a_two_handed_pose_writes_the_other_arm_too",
        bow_open.0.right == ArmPose::BowAndArrow
            && bow_open.0.left == ArmPose::Empty
            && bow_open.1.is_some_and(|r| near3(r, bow_open.2.left_rot, TOL))
            && bow_open.1.is_some_and(|r| (r[1] - (0.1 + 0.4)).abs() < 1e-4),
        format!(
            "bow drawn: right={:?} left={:?}; LEFT arm rot={} want={} — the left \
             arm's yRot is 0.1 + head.yRot + 0.4 even though its own pose is EMPTY \
             and was never called (BOW_AND_ARROW.affectsOffhandPose() suppressed it)",
            bow_open.0.right,
            bow_open.0.left,
            bow_open.1.map(fmt3).unwrap_or_default(),
            fmt3(bow_open.2.left_rot)
        ),
    );

    // --- CROSSBOW_HOLD: charged vs not ------------------------------------
    let hold = |stack: Stack| -> ArmPoses {
        let mut t = table(ctx);
        equip(ctx, &mut t, 1, &[(0, stack)]);
        let (_, poses, _) = pose(
            EntityModelKind::Player, &t, 1, 1.0, pitch, 0.0, limb.0, limb.1,
            &ctx.spears, Some(ctx.bow), Some(ctx.crossbow),
        );
        poses
    };
    let charged = hold(Stack::Charged(ctx.crossbow, ctx.apple));
    let bare = hold(Stack::Plain(ctx.crossbow));
    let empty_list = hold(Stack::ChargedEmpty(ctx.crossbow));
    c.record(
        "m14.only_a_charged_crossbow_holds",
        charged.right == ArmPose::CrossbowHold
            && bare.right == ArmPose::Item
            && empty_list.right == ArmPose::Item,
        format!(
            "charged={:?} bare={:?} empty-list={:?} — isCharged is \
             !getOrDefault(CHARGED_PROJECTILES, EMPTY).isEmpty(), so a present but \
             empty list is NOT charged",
            charged.right, bare.right, empty_list.right
        ),
    );

    // Walking `charged_projectiles` is what makes that reachable at all: before
    // M23 the component was un-transcribed, so the stack was Unwalkable and the
    // whole baseline was suppressed.
    c.record(
        "m15.the_charged_projectiles_patch_is_walked_not_suppressed",
        charged.known && empty_list.known,
        format!(
            "charged known={} empty-list known={} — an unwalkable patch would \
             suppress the pose entirely (HandItem::Unknown)",
            charged.known, empty_list.known
        ),
    );

    // The two-handed override: a charged crossbow in the main hand rewrites the
    // off-hand pose to ITEM/EMPTY regardless of what the off-hand holds.
    let overridden = {
        let mut t = table(ctx);
        equip(
            ctx, &mut t, 1,
            &[(0, Stack::Charged(ctx.crossbow, ctx.apple)), (1, Stack::Plain(ctx.spear))],
        );
        let (_, poses, _) = pose(
            EntityModelKind::Player, &t, 1, 1.0, pitch, 0.0, limb.0, limb.1,
            &ctx.spears, Some(ctx.bow), Some(ctx.crossbow),
        );
        poses
    };
    c.record(
        "m16.a_two_handed_main_hand_rewrites_the_off_hand_pose",
        overridden.right == ArmPose::CrossbowHold && overridden.left == ArmPose::Item,
        format!(
            "charged crossbow + off-hand SPEAR -> right={:?} left={:?} (want Item, \
             NOT Spear) — mainHandPose.isTwoHanded() rewrites offHandPose, which is \
             why the pose must be computed per HAND and not per arm",
            overridden.right, overridden.left
        ),
    );

    // --- CROSSBOW_CHARGE lerps over the elapsed use ticks ------------------
    let charge_at = |ticks: u32| -> (ArmPoses, Option<[f32; 3]>, Expect) {
        let mut t = table(ctx);
        equip(ctx, &mut t, 1, &[(0, Stack::Plain(ctx.crossbow))]);
        meta(ctx, &mut t, 1, 8, 0, &[using_main]);
        for _ in 0..ticks {
            t.tick_lerp();
        }
        let (attack, poses, deltas) =
            pose(EntityModelKind::Player, &t, 1, 1.0, pitch, 0.0, limb.0, limb.1,
                 &ctx.spears, Some(ctx.bow), Some(ctx.crossbow));
        let e = expect_attack(
            attack.attack_time, attack.left_arm, attack.kind, attack.age_scale,
            pitch, limb.0, limb.1, 0.0, poses, 0.0,
        );
        (poses, find(&deltas, "left_arm").map(|(r, _)| r), e)
    };
    let (p0, l0, e0) = charge_at(0);
    let (_, l12, e12) = charge_at(12);
    let (_, l25, e25) = charge_at(25);
    c.record(
        "m17.the_crossbow_charge_pulling_arm_lerps_over_the_use_clock",
        p0.right == ArmPose::CrossbowCharge
            && p0.max_crossbow_charge == 25.0
            && l0.is_some_and(|r| near3(r, e0.left_rot, TOL))
            && l12.is_some_and(|r| near3(r, e12.left_rot, TOL))
            && l25.is_some_and(|r| near3(r, e25.left_rot, TOL))
            && match (l0, l12, l25) {
                (Some(a), Some(b), Some(cc)) => a[1] < b[1] && b[1] < cc[1],
                _ => false,
            },
        format!(
            "pose={:?} max={} — LEFT (pulling) arm at 0/12/25 ticks: {} / {} / {} \
             (want {} / {} / {}); yRot must increase monotonically 0.4 -> 0.85",
            p0.right,
            p0.max_crossbow_charge,
            l0.map(fmt3).unwrap_or_default(),
            l12.map(fmt3).unwrap_or_default(),
            l25.map(fmt3).unwrap_or_default(),
            fmt3(e0.left_rot),
            fmt3(e12.left_rot),
            fmt3(e25.left_rot),
        ),
    );

    // --- the avatar / mob split -------------------------------------------
    // `HumanoidMobRenderer.getArmPose` falls through to EMPTY, not ITEM. This
    // was collapsed to the avatar function through M22.
    let mob_armed = {
        let mut t = table(ctx);
        equip(ctx, &mut t, 3, &[(0, Stack::Plain(ctx.sword))]);
        let (_, poses, _) = pose(
            EntityModelKind::Zombie, &t, 3, 1.0, pitch, 0.0, limb.0, limb.1,
            &ctx.spears, Some(ctx.bow), Some(ctx.crossbow),
        );
        poses
    };
    let player_armed = {
        let mut t = table(ctx);
        equip(ctx, &mut t, 1, &[(0, Stack::Plain(ctx.sword))]);
        let (_, poses, _) = pose(
            EntityModelKind::Player, &t, 1, 1.0, pitch, 0.0, limb.0, limb.1,
            &ctx.spears, Some(ctx.bow), Some(ctx.crossbow),
        );
        poses
    };
    c.record(
        "m18.an_armed_mob_poses_empty_where_an_armed_player_poses_item",
        mob_armed.right == ArmPose::Empty && player_armed.right == ArmPose::Item,
        format!(
            "same sword: zombie->{:?} player->{:?} — HumanoidMobRenderer.getArmPose \
             checks only STAB-while-swinging and the spears tag, then returns EMPTY",
            mob_armed.right, player_armed.right
        ),
    );

    // A mob still has no use-driven pose: its getArmPose has no use branch,
    // even though its render state carries the use fields.
    let mob_shield = {
        let mut t = table(ctx);
        equip(ctx, &mut t, 3, &[(0, Stack::Plain(ctx.shield))]);
        meta(ctx, &mut t, 3, 8, 0, &[using_main]);
        let (_, poses, _) = pose(
            EntityModelKind::Zombie, &t, 3, 1.0, pitch, 0.0, limb.0, limb.1,
            &ctx.spears, Some(ctx.bow), Some(ctx.crossbow),
        );
        poses
    };
    c.record(
        "m19.a_mob_using_an_item_still_takes_no_use_pose_but_does_set_the_flag",
        mob_shield.right == ArmPose::Empty && mob_shield.using_item,
        format!(
            "zombie raising a shield -> {:?} with isUsingItem={} — the use fields are \
             extracted for mobs (extractHumanoidRenderState) but only \
             AvatarRenderer.getArmPose reads getUseAnimation",
            mob_shield.right, mob_shield.using_item
        ),
    );

    // --- the dispatch order -----------------------------------------------
    // With isUsingItem set, the dispatch picks the used arm first rather than
    // deriving the order from twoHandedOffhand.
    let mut order_ok = true;
    let mut order_detail = Vec::new();
    for (rh, main_used, want_first_left) in
        [(true, true, false), (true, false, true), (false, true, true), (false, false, false)]
    {
        let p = ArmPoses {
            right: ArmPose::Item,
            left: ArmPose::Item,
            right_handed: rh,
            known: true,
            using_item: true,
            main_hand_used: main_used,
            ticks_using_item: 0.0,
            max_crossbow_charge: 0.0,
        };
        let got = p.order();
        order_ok &= got.first_left == want_first_left && got.second_runs;
        order_detail.push(format!(
            "rh={rh} mainUsed={main_used} -> first_left={} (want {want_first_left})",
            got.first_left
        ));
    }
    c.record(
        "m20.the_is_using_item_dispatch_branch_picks_the_used_arm_first",
        order_ok,
        format!(
            "{} — vanilla's `mainHandUsed == rightHanded` equality, which M22 \
             documented as unreachable because isUsingItem was never set",
            order_detail.join("; ")
        ),
    );
}

fn check_mob_rigs(c: &mut Checker, ctx: &Ctx) {
    const TOL: f32 = 1e-6;
    let age = 0.0_f32;
    let limb = (7.5_f32, 0.8_f32);
    let pitch = 0.0_f32;

    // --- the flags byte -------------------------------------------------
    let mut t = table(ctx);
    add(&mut t, 9, ctx.zombie_tid);
    let routed = meta(ctx, &mut t, 9, 15, 0, &[0b0000_0110]);
    let st = t.mob_state(9);
    c.record(
        "k1.the_index_15_byte_is_the_mob_flags_and_its_bits_are_exact",
        routed
            && st.is_aggressive()
            && st.is_left_handed()
            && t.main_arm(9) == rewo_world::entities::HumanoidArm::Left,
        format!(
            "flags=0b110 → aggressive={} left_handed={} mainArm={:?} (bit 4 / bit 2)",
            st.is_aggressive(),
            st.is_left_handed(),
            t.main_arm(9)
        ),
    );
    // Sensitivity: bit 1 is no-AI and must move neither.
    let mut t1 = table(ctx);
    add(&mut t1, 9, ctx.zombie_tid);
    meta(ctx, &mut t1, 9, 15, 0, &[0b0000_0001]);
    let s1 = t1.mob_state(9);
    c.record(
        "k2.the_other_flag_bits_do_not_leak",
        !s1.is_aggressive()
            && !s1.is_left_handed()
            && t1.main_arm(9) == rewo_world::entities::HumanoidArm::Right,
        format!(
            "flags=0b001 (no-AI) → aggressive={} left_handed={} mainArm={:?}",
            s1.is_aggressive(),
            s1.is_left_handed(),
            t1.main_arm(9)
        ),
    );
    // A non-mob must not take the byte at all: an ArmorStand puts unrelated
    // client flags at the same index with the same serializer.
    let mut t2 = table(ctx);
    add(&mut t2, 9, ctx.boat_tid);
    meta(ctx, &mut t2, 9, 15, 0, &[0b0000_0110]);
    c.record(
        "k3.a_non_mob_never_takes_the_index_15_byte",
        t2.mob_state(9) == Default::default()
            && t2.main_arm(9) == rewo_world::entities::HumanoidArm::Right,
        format!(
            "boat flags=0b110 → state {:?}, mainArm {:?} (an armor stand's client \
             flags share the slot and mean something else)",
            t2.mob_state(9),
            t2.main_arm(9)
        ),
    );
    // Index 16 BOOLEAN on a Raider is IS_CELEBRATING, not DATA_BABY_ID.
    let mut t3 = table(ctx);
    add(&mut t3, 9, ctx.vindicator_tid);
    meta(ctx, &mut t3, 9, 16, 8, &[1]);
    let mut t4 = table(ctx);
    add(&mut t4, 9, ctx.zombie_tid);
    meta(ctx, &mut t4, 9, 16, 8, &[1]);
    c.record(
        "k4.index_16_boolean_on_a_raider_is_celebrating_not_baby",
        t3.mob_state(9).celebrating
            && !t3.is_baby(9)
            && t4.is_baby(9)
            && !t4.mob_state(9).celebrating,
        format!(
            "vindicator → celebrating={} baby={}; zombie → celebrating={} baby={}",
            t3.mob_state(9).celebrating,
            t3.is_baby(9),
            t4.mob_state(9).celebrating,
            t4.is_baby(9)
        ),
    );

    // --- the undead rig -------------------------------------------------
    let undead = |aggressive: bool, baby: bool, stack: Option<Stack>, action: u8, ticks: u32| {
        let mut t = table(ctx);
        add(&mut t, 1, ctx.zombie_tid);
        if aggressive {
            meta(ctx, &mut t, 1, 15, 0, &[0b0000_0100]);
        }
        if baby {
            meta(ctx, &mut t, 1, 16, 8, &[1]);
        }
        if let Some(s) = stack {
            equip(ctx, &mut t, 1, &[(0, s.clone()), (1, s)]);
        }
        if action != 255 {
            swing(ctx, &mut t, 1, action);
        }
        for _ in 0..ticks {
            t.tick_lerp();
        }
        let (a, _, d) = pose(
            EntityModelKind::Zombie,
            &t,
            1,
            1.0,
            pitch,
            0.0,
            limb.0,
            limb.1,
            &ctx.spears,
            Some(ctx.bow),
            Some(ctx.crossbow),
        );
        (a, d)
    };

    let (_, d_idle) = undead(false, false, None, 255, 0);
    let r_idle = find(&d_idle, "right_arm").map(|(r, _)| r);
    let w_idle = want_zombie_arms(false, 0.0, age, false, false, true);
    c.record(
        "k5.an_idle_adult_undead_rests_at_minus_pi_over_2_25",
        r_idle.is_some_and(|r| near3(r, w_idle, TOL)),
        format!(
            "right={} want={} → xRot {:.3} deg (vanilla −π/2.25 = −80°, not the −90° \
             the baked fold used to freeze it at)",
            r_idle.map(fmt3).unwrap_or_default(),
            fmt3(w_idle),
            r_idle.map(|r| r[0] / DEG).unwrap_or(0.0),
        ),
    );

    let (_, d_agg) = undead(true, false, None, 255, 0);
    let r_agg = find(&d_agg, "right_arm").map(|(r, _)| r);
    let w_agg = want_zombie_arms(false, 0.0, age, true, false, true);
    c.record(
        "k6.aggressive_deepens_the_arm_drop_to_minus_pi_over_1_5",
        r_agg.is_some_and(|r| near3(r, w_agg, TOL))
            && r_agg.zip(r_idle).is_some_and(|(a, i)| a[0] < i[0] - 0.5),
        format!(
            "aggressive right={} want={} → {:.3} deg vs idle {:.3} deg (−120° vs −80°)",
            r_agg.map(fmt3).unwrap_or_default(),
            fmt3(w_agg),
            r_agg.map(|r| r[0] / DEG).unwrap_or(0.0),
            r_idle.map(|r| r[0] / DEG).unwrap_or(0.0),
        ),
    );

    let (a_sw, d_sw) = undead(false, false, None, 0, 3);
    let r_sw = find(&d_sw, "right_arm").map(|(r, _)| r);
    let l_sw = find(&d_sw, "left_arm").map(|(r, _)| r);
    let w_sw_r = want_zombie_arms(false, a_sw.attack_time, age, false, false, true);
    let w_sw_l = want_zombie_arms(true, a_sw.attack_time, age, false, false, true);
    c.record(
        "k7.the_undead_strike_is_exact_on_both_arms",
        r_sw.is_some_and(|r| near3(r, w_sw_r, TOL)) && l_sw.is_some_and(|r| near3(r, w_sw_l, TOL)),
        format!(
            "attackTime={:.6} right={} want={}; left={} want={} (both arms swing — the \
             undead rig is not attack-arm gated)",
            a_sw.attack_time,
            r_sw.map(fmt3).unwrap_or_default(),
            fmt3(w_sw_r),
            l_sw.map(fmt3).unwrap_or_default(),
            fmt3(w_sw_l),
        ),
    );

    // The pivot still carries `setupAttackAnimation`'s arm.x/z assignment:
    // `animateZombieArms` overwrites only the rotations.
    let off_sw = find(&d_sw, "right_arm").map(|(_, o)| o);
    c.record(
        "k8.the_undead_rig_overwrites_rotations_but_not_the_pivot",
        off_sw.is_some_and(|o| !near3(o, [0.0; 3], 1e-4)),
        format!(
            "right_arm pivot delta={} — the humanoid stage's arm.x/z survives because \
             animateZombieArms assigns rotations only",
            off_sw.map(fmt3).unwrap_or_default()
        ),
    );

    // A STAB item skips the strike entirely; the humanoid pose survives and is
    // bobbed a SECOND time.
    let (a_stab, d_stab) = undead(false, false, Some(Stack::Plain(ctx.spear)), 0, 3);
    let r_stab = find(&d_stab, "right_arm").map(|(r, _)| r);
    let mut e_stab = expect_attack(
        a_stab.attack_time,
        a_stab.left_arm,
        SwingKind::Stab,
        1.0,
        pitch,
        limb.0,
        limb.1,
        age,
        ArmPoses {
            right: ArmPose::Spear,
            left: ArmPose::Spear,
            right_handed: true,
            known: true,
            using_item: false,
            main_hand_used: true,
            ticks_using_item: 0.0,
            max_crossbow_charge: 0.0,
        },
        0.0,
    );
    // `bobArms` runs unconditionally at the end of `animateZombieArms`.
    expect_bob(&mut e_stab, age);
    c.record(
        "k9.a_stab_item_skips_the_undead_strike_and_bobs_twice",
        a_stab.kind == SwingKind::Stab && r_stab.is_some_and(|r| near3(r, e_stab.right_rot, TOL)),
        format!(
            "kind={:?} right={} want={} — the humanoid STAB pose survives and takes a \
             SECOND bob (vanilla calls bobArms outside the animateAttack guard)",
            a_stab.kind,
            r_stab.map(fmt3).unwrap_or_default(),
            fmt3(e_stab.right_rot),
        ),
    );

    // Only a BABY HOLDING SOMETHING drops its arms.
    let (_, d_baby_item) = undead(false, true, Some(Stack::Plain(ctx.sword)), 255, 0);
    let (_, d_baby_bare) = undead(false, true, None, 255, 0);
    let r_bi = find(&d_baby_item, "right_arm").map(|(r, _)| r);
    let r_bb = find(&d_baby_bare, "right_arm").map(|(r, _)| r);
    let w_bi = want_zombie_arms(false, 0.0, age, false, true, false);
    let w_bb = want_zombie_arms(false, 0.0, age, true.eq(&false), true, true);
    c.record(
        "k10.only_a_baby_holding_an_item_drops_its_arms",
        r_bi.is_some_and(|r| near3(r, w_bi, TOL))
            && r_bb.is_some_and(|r| near3(r, w_bb, TOL))
            && r_bi.zip(r_bb).is_some_and(|(a, b)| (a[0] - b[0]).abs() > 1.0),
        format!(
            "baby+item right={} want={} ({:.1} deg, armDrop 0); baby+empty right={} \
             want={} ({:.1} deg, still raised)",
            r_bi.map(fmt3).unwrap_or_default(),
            fmt3(w_bi),
            r_bi.map(|r| r[0] / DEG).unwrap_or(0.0),
            r_bb.map(fmt3).unwrap_or_default(),
            fmt3(w_bb),
            r_bb.map(|r| r[0] / DEG).unwrap_or(0.0),
        ),
    );

    // --- the skeleton rig ------------------------------------------------
    let skel = |aggressive: bool, stack: Option<Stack>, ticks: u32| {
        let mut t = table(ctx);
        add(&mut t, 1, ctx.skeleton_tid);
        if aggressive {
            meta(ctx, &mut t, 1, 15, 0, &[0b0000_0100]);
        }
        if let Some(s) = stack {
            equip(ctx, &mut t, 1, &[(0, s.clone()), (1, s)]);
        }
        swing(ctx, &mut t, 1, 0);
        for _ in 0..ticks {
            t.tick_lerp();
        }
        let (a, _, d) = pose(
            EntityModelKind::Skeleton,
            &t,
            1,
            1.0,
            pitch,
            0.0,
            limb.0,
            limb.1,
            &ctx.spears,
            Some(ctx.bow),
            Some(ctx.crossbow),
        );
        (a, d)
    };
    let (a_sk, d_sk) = skel(true, None, 3);
    let r_sk = find(&d_sk, "right_arm").map(|(r, _)| r);
    let w_sk = want_skeleton_arms(false, a_sk.attack_time, age);
    c.record(
        "k11.the_skeleton_attack_rig_is_exact_when_aggressive",
        r_sk.is_some_and(|r| near3(r, w_sk, TOL)),
        format!(
            "attackTime={:.6} right={} want={} (xRot pinned to −π/2 with the strike \
             SUBTRACTED, unlike the undead assembly)",
            a_sk.attack_time,
            r_sk.map(fmt3).unwrap_or_default(),
            fmt3(w_sk),
        ),
    );
    // Gated twice: not aggressive, or holding a bow, leaves the humanoid pose.
    let (_, d_calm) = skel(false, None, 3);
    let (_, d_bow) = skel(true, Some(Stack::Plain(ctx.bow)), 3);
    let r_calm = find(&d_calm, "right_arm").map(|(r, _)| r);
    let r_bow = find(&d_bow, "right_arm").map(|(r, _)| r);
    c.record(
        "k12.the_skeleton_rig_is_gated_on_aggressive_and_no_bow",
        r_calm.is_some_and(|r| !near3(r, w_sk, 1e-3))
            && r_bow.is_some_and(|r| !near3(r, w_sk, 1e-3)),
        format!(
            "not-aggressive right={} and bow-in-hand right={} both differ from the \
             aggressive rig {} — a bow-armed skeleton keeps its aiming pose",
            r_calm.map(fmt3).unwrap_or_default(),
            r_bow.map(fmt3).unwrap_or_default(),
            fmt3(w_sk),
        ),
    );

    // --- the illager rig -------------------------------------------------
    let illager = |tid: i32, kind: EntityModelKind, setup: &dyn Fn(&Ctx, &mut EntityTable)| {
        let mut t = table(ctx);
        add(&mut t, 1, tid);
        setup(ctx, &mut t);
        swing(ctx, &mut t, 1, 0);
        for _ in 0..3 {
            t.tick_lerp();
        }
        let (a, _, d) = pose(
            kind,
            &t,
            1,
            1.0,
            pitch,
            0.0,
            limb.0,
            limb.1,
            &ctx.spears,
            Some(ctx.bow),
            Some(ctx.crossbow),
        );
        let mob = crate::live_cmd::resolve_mob_combat(&t, 1, kind, Some(ctx.bow));
        (a, d, mob)
    };

    // A vindicator with no weapon and the aggressive bit set → ATTACKING with
    // an empty main hand → `animateZombieArms(..., true, ...)`. Note the
    // literal `true`: the illager path never consults the mob's own flag.
    let (a_vi, d_vi, m_vi) = illager(
        ctx.vindicator_tid,
        EntityModelKind::Vindicator,
        &|ctx, t| {
            meta(ctx, t, 1, 15, 0, &[0b0000_0100]);
        },
    );
    let r_vi = find(&d_vi, "right_arm").map(|(r, _)| r);
    let w_vi = want_zombie_arms(false, a_vi.attack_time, age, true, false, true);
    c.record(
        "k13.an_empty_handed_attacking_illager_uses_the_zombie_arms_with_a_literal_true",
        m_vi.illager_pose == rewo_gpu::mobs::IllagerArmPose::Attacking
            && r_vi.is_some_and(|r| near3(r, w_vi, TOL)),
        format!(
            "pose={:?} right={} want={} (aggressive=true is passed literally, not read \
             from the mob's flag)",
            m_vi.illager_pose,
            r_vi.map(fmt3).unwrap_or_default(),
            fmt3(w_vi),
        ),
    );

    // The same vindicator holding a weapon takes `swingWeaponDown` instead.
    let (a_vw, d_vw, m_vw) = illager(
        ctx.vindicator_tid,
        EntityModelKind::Vindicator,
        &|ctx, t| {
            meta(ctx, t, 1, 15, 0, &[0b0000_0100]);
            equip(
                ctx,
                t,
                1,
                &[(0, Stack::Plain(ctx.sword)), (1, Stack::Empty)],
            );
        },
    );
    let r_vw = find(&d_vw, "right_arm").map(|(r, _)| r);
    let l_vw = find(&d_vw, "left_arm").map(|(r, _)| r);
    let w_vw_r = want_swing_weapon_down(false, false, a_vw.attack_time, age);
    let w_vw_l = want_swing_weapon_down(true, false, a_vw.attack_time, age);
    c.record(
        "k14.an_armed_attacking_illager_swings_the_weapon_down",
        m_vw.illager_pose == rewo_gpu::mobs::IllagerArmPose::Attacking
            && r_vw.is_some_and(|r| near3(r, w_vw_r, TOL))
            && l_vw.is_some_and(|r| near3(r, w_vw_l, TOL))
            && r_vw.is_some_and(|r| !near3(r, w_vi, 1e-3)),
        format!(
            "right={} want={}; left={} want={} — the two arms take different terms, and \
             this differs from the empty-handed rig {}",
            r_vw.map(fmt3).unwrap_or_default(),
            fmt3(w_vw_r),
            l_vw.map(fmt3).unwrap_or_default(),
            fmt3(w_vw_l),
            fmt3(w_vi),
        ),
    );

    // Per-class pose derivation, each from its own decompiled `getArmPose`.
    let pose_of = |tid: i32, kind: EntityModelKind, setup: &dyn Fn(&Ctx, &mut EntityTable)| {
        let (_, _, m) = illager(tid, kind, setup);
        m.illager_pose
    };
    use rewo_gpu::mobs::IllagerArmPose as P;
    let pil_charge = pose_of(ctx.pillager_tid, EntityModelKind::Pillager, &|ctx, t| {
        meta(ctx, t, 1, 17, 8, &[1]);
    });
    let pil_hold = pose_of(ctx.pillager_tid, EntityModelKind::Pillager, &|ctx, t| {
        equip(ctx, t, 1, &[(0, Stack::Plain(ctx.crossbow)), (1, Stack::Empty)]);
    });
    let pil_neutral = pose_of(ctx.pillager_tid, EntityModelKind::Pillager, &|_, _| {});
    let evo_spell = pose_of(ctx.evoker_tid, EntityModelKind::Evoker, &|ctx, t| {
        meta(ctx, t, 1, 17, 0, &[3]);
    });
    let evo_celeb = pose_of(ctx.evoker_tid, EntityModelKind::Evoker, &|ctx, t| {
        meta(ctx, t, 1, 16, 8, &[1]);
    });
    let ill_bow = pose_of(ctx.illusioner_tid, EntityModelKind::Illusioner, &|ctx, t| {
        meta(ctx, t, 1, 15, 0, &[0b0000_0100]);
    });
    let vin_crossed = pose_of(ctx.vindicator_tid, EntityModelKind::Vindicator, &|_, _| {});
    c.record(
        "k15.each_illager_class_derives_its_own_arm_pose",
        pil_charge == P::CrossbowCharge
            && pil_hold == P::CrossbowHold
            && pil_neutral == P::Neutral
            && evo_spell == P::Spellcasting
            && evo_celeb == P::Celebrating
            && ill_bow == P::BowAndArrow
            && vin_crossed == P::Crossed,
        format!(
            "pillager charging={pil_charge:?} holding={pil_hold:?} idle={pil_neutral:?}; \
             evoker casting={evo_spell:?} celebrating={evo_celeb:?}; illusioner \
             aggressive={ill_bow:?} (BOW_AND_ARROW, not ATTACKING); vindicator \
             idle={vin_crossed:?}"
        ),
    );

    // A spellcaster's index-17 BYTE must not be read on a non-spellcaster, and
    // a pillager's index-17 BOOLEAN must not be read on one.
    let vin_spell = pose_of(ctx.vindicator_tid, EntityModelKind::Vindicator, &|ctx, t| {
        meta(ctx, t, 1, 17, 0, &[3]);
    });
    let vin_charge = pose_of(ctx.vindicator_tid, EntityModelKind::Vindicator, &|ctx, t| {
        meta(ctx, t, 1, 17, 8, &[1]);
    });
    c.record(
        "k16.the_index_17_slots_are_kind_gated",
        vin_spell == P::Crossed && vin_charge == P::Crossed,
        format!(
            "a vindicator sent the spellcaster BYTE → {vin_spell:?} and the pillager \
             BOOLEAN → {vin_charge:?}; both stay CROSSED because neither accessor is \
             declared on its class"
        ),
    );
}
