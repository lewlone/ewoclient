//! `rewo play` — the M3 headless bot harness.
//!
//! Connects, runs a scripted survival session on a fixed 20 Hz clock
//! (spawn → walk → sprint → jump → look → dig → place → chat), and reports
//! the physics-parity meter: **server position corrections**. A world that
//! matches vanilla's simulation gets few/no corrections; drift shows up as
//! rubber-banding the server sends back. This is the DoD verification —
//! no human, no window.

use std::time::{Duration, Instant};

use clap::Args as ClapArgs;
use rewo_data::{assets, DataPaths, GameData};
use rewo_net::play::PlaySession;
use rewo_net::Connection;
use rewo_world::physics::TickInput;

const TICK: Duration = Duration::from_millis(50); // 20 Hz

#[derive(ClapArgs)]
pub struct PlayArgs {
    #[arg(long, default_value = "127.0.0.1")]
    host: String,
    #[arg(long, default_value_t = 25599)]
    port: u16,
    /// Player name. Defaults to the launcher handoff's profile name when
    /// an account is present (online-mode servers verify the name against
    /// the session join), else "RewoBot".
    #[arg(long)]
    username: Option<String>,
    #[arg(long, default_value = "26.2")]
    version: String,
    /// Total session length in seconds.
    #[arg(long, default_value_t = 40.0)]
    seconds: f32,
    /// Server command (without the leading `/`) to run once, right after
    /// spawn — world setup for a parity run, e.g. paving the floor with slabs
    /// to exercise partial collision shapes against the server's own physics.
    /// Multiple commands may be separated by `;` — enough to build a test
    /// structure (walls + roof + a torch) in one run.
    #[arg(long)]
    setup: Option<String>,
    /// Suppress the movement script. The light gate centres on the bot's
    /// final position, so it must stay inside whatever `--setup` built.
    #[arg(long, default_value_t = false)]
    still: bool,
    /// Report light at a fixed world coordinate ("x,y,z") in the summary,
    /// instead of only at the bot. Removes the bot's wandering from the
    /// measurement — the deterministic way to check the light decode, and to
    /// see whether a runtime change (a torch placed by --setup) reaches us.
    #[arg(long)]
    light_at: Option<String>,
    /// Recompute the loaded chunks' light from scratch and diff against the
    /// server's authoritative values — the lighting equivalent of the
    /// `CORRECTIONS` physics meter. Any mismatch is a bug in our light
    /// tables or flood fill, measured against vanilla's own engine.
    #[arg(long, default_value_t = false)]
    light_check: bool,
    /// Do not relight our own edits. `--light-check` compares the recomputed
    /// light against whatever is stored, and incremental relighting *writes*
    /// that store — so a world built during the session would have the gate
    /// grading our engine against itself. This flag keeps the stored light
    /// purely server-authoritative, which is the only comparison that proves
    /// anything.
    #[arg(long, default_value_t = false)]
    no_relight: bool,
    /// Send a chat line partway through.
    #[arg(long, default_value = "rewo bot online")]
    chat: String,
    /// Skip the block dig/place actions (movement-only run).
    #[arg(long, default_value_t = false)]
    no_build: bool,
    /// M16's authoritative live dimension gate: drive Overworld → Nether → End
    /// → Overworld from inside this one session, issuing the server commands
    /// itself, and validate the dimension property at each checkpoint. Forces a
    /// deterministic still/no-build/no-chat path (see `dimension_check`) and
    /// refuses `--setup`, whose paced command stream would race this one for
    /// the server's chat rate limiter. Needs the op account (`--username
    /// RewoOp`): a non-op's commands are silently rejected, which this gate
    /// turns into a timeout rather than a skipped check.
    #[arg(long, default_value_t = false)]
    dimension_check: bool,
    /// M19's live equipment gate: summon a mob, arm it from the server, and
    /// prove the `ClientboundSetEquipmentPacket` + `ItemStack` wire decode
    /// against a real vanilla server.
    ///
    /// This is the one M19 wire format built by hand from the decompile that no
    /// serverless oracle can validate — `swingshot` grades the decoder against
    /// bodies this repo also wrote. Here the *server* writes them. Drives its
    /// own paced command stream (so it refuses `--setup`, whose stream would
    /// race this one for the chat rate limiter) and forces a still, no-build
    /// path. Needs the op account (`--username RewoOp`).
    ///
    /// Deliberately does **not** cover the haste / mining-fatigue term of
    /// `getCurrentSwingDuration`: 26.2 sends `ClientboundUpdateMobEffectPacket`
    /// only to the affected player itself and to players *riding* the affected
    /// entity (`ServerPlayer.onEffectAdded`, `LivingEntity.sendEffectToPassengers`)
    /// — never to ordinary trackers — so a mob's effects are unobservable here
    /// by design, not by omission.
    #[arg(long, default_value_t = false)]
    swing_check: bool,
    /// M68's live gate for the four packets that move the local player from
    /// outside its own input: `explode`, `set_entity_motion`, `move_vehicle`
    /// and `set_passengers`.
    ///
    /// **This exists because `CORRECTIONS 0` was proving less than it was
    /// being read as proving.** The ordinary run walks, sprints, jumps and
    /// builds on flat ground — it is never knocked back and never mounted, so
    /// all four of these packets are outside what it can exercise
    /// (`REWO_PACKET_COVERAGE.md` §3.1). This gate drives its own paced
    /// command stream to get the bot seated in a boat, dismounted, blown up
    /// and hit, then grades what actually arrived and what the physics did
    /// with it.
    ///
    /// Fail-closed on **observation**, not just on decode: a command that the
    /// server silently ignored leaves its packet count at zero and turns the
    /// gate red, rather than passing a run that tested nothing. Needs the op
    /// account (`--username RewoOp`) and forces a still, no-build path.
    ///
    /// `move_vehicle` is deliberately **not** required — see
    /// `motion_acceptance`.
    #[arg(long, default_value_t = false)]
    motion_check: bool,
}

/// The scoreboard tag scoping every fixture `--motion-check` creates, so a
/// repeat run cannot mount a boat left by an earlier one and `kill` can never
/// reach an unrelated entity.
const MOTION_CHECK_TAG: &str = "rewo_motion_check";
/// The vehicle the mount phase uses. A boat sits still on land, carries a
/// player, and — unlike a minecart — needs no rail, so the fixture works on
/// the flat test world with no world preparation.
const MOTION_CHECK_VEHICLE: &str = "minecraft:oak_boat";
/// The attacker the damage phase names. `dealDefaultKnockback` reads
/// `source.getSourcePosition()`, so a damage source with no entity behind it
/// produces a hit with **no** knockback — and a zero-velocity
/// `set_entity_motion` would exercise only the one-byte sentinel path.
const MOTION_CHECK_ATTACKER: &str = "minecraft:zombie";

/// How many of `motion_check_commands`' entries belong to the mount phase.
///
/// **Derived, not guessed.** The first version of this gate hard-coded the
/// phase split at a flat 9 s, which happened to be *after the whole knockback
/// phase had already run* — so the "knockback phase corrections" window was
/// empty and the assertion over it was vacuous. A deliberate mutation (decode
/// the knockback, never apply it) sailed through a green gate; that is what
/// found it. Keying the split to the command list means adding a mount command
/// moves the boundary automatically instead of silently emptying the window
/// again.
const MOTION_MOUNT_PHASE_COMMANDS: usize = 12;

/// Spawn-relative second at which the mount phase is over and the knockback
/// phase begins. The two are graded separately because the server treats them
/// completely differently — see `motion_acceptance`.
///
/// `drive` issues one command per 250 ms starting at t=1 s, so command `i`
/// fires at `1.0 + 0.25 * i`. The split sits on the boundary command itself
/// (`gamemode survival`), which is the first of the knockback phase: every
/// mount-phase command has gone out by then, and nothing that shoves the
/// player has.
const MOTION_PHASE_SPLIT: f32 = 1.0 + 0.25 * MOTION_MOUNT_PHASE_COMMANDS as f32;

/// The paced command stream `--motion-check` issues after spawn, one per
/// 250 ms from t=1s (the same rate limiter budget every other gate here
/// respects). `say` lines are deliberate no-ops that keep the pacing honest
/// while the server applies the previous command and the tracker broadcasts.
///
/// Two phases, in this order for a reason: the mount phase runs first, while
/// the ground under the bot is still undisturbed, because the knockback phase
/// craters it.
fn motion_check_commands() -> Vec<String> {
    let boat = format!("@e[type={MOTION_CHECK_VEHICLE},tag={MOTION_CHECK_TAG},limit=1,sort=nearest]");
    let zombie =
        format!("@e[type={MOTION_CHECK_ATTACKER},tag={MOTION_CHECK_TAG},limit=1,sort=nearest]");
    vec![
        // ── phase 1: mount ──────────────────────────────────────────────
        format!("kill @e[type={MOTION_CHECK_VEHICLE},tag={MOTION_CHECK_TAG}]"),
        "say motion-check w1".into(),
        format!("summon {MOTION_CHECK_VEHICLE} ~ ~ ~1 {{Tags:[\"{MOTION_CHECK_TAG}\"]}}"),
        "say motion-check w2".into(),
        // `/ride` is gamemode-independent and does not depend on damage rules,
        // which is why the mount phase is the reliable half of this gate.
        format!("ride @s mount {boat}"),
        "say motion-check w3".into(),
        "say motion-check w4".into(),
        "say motion-check w5".into(),
        // A dismount is not a removal on the wire: the server re-sends the
        // boat's rider list without us. That asymmetry is rule 5 in
        // `rewo_net::motion`, and this is where it is exercised live.
        "ride @s dismount".into(),
        "say motion-check w6".into(),
        format!("kill @e[type={MOTION_CHECK_VEHICLE},tag={MOTION_CHECK_TAG}]"),
        "say motion-check w7".into(),
        // ── phase 2: knockback + damage ─────────────────────────────────
        // Everything from here on must stay below the mount-phase count in
        // `MOTION_MOUNT_PHASE_COMMANDS`, which is asserted against this list.
        // Survival, because `ServerExplosion.hurtEntities` records a player in
        // `hitPlayers` only when it is neither a spectator nor a *flying*
        // creative player, and because `markHurt` — the thing that makes the
        // server send us our own `set_entity_motion` — needs real damage.
        "gamemode survival @s".into(),
        // Resistance IV (amplifier 3) blunts 80% of the blast so the bot
        // survives it, while leaving `damage > 0` so `markHurt` still fires.
        // Amplifier 4 would zero the damage and silently remove the trigger.
        "effect give @s minecraft:resistance 120 3 true".into(),
        // **Rebuild the ground before using it.** `ServerExplosion` scales
        // knockback by `getSeenPercent`, a line-of-sight sample — so a TNT
        // that lands in a crater left by an *earlier run of this same gate*
        // is shielded by the crater wall and delivers a knockback that is
        // present-but-zero. That is not hypothetical: it happened, and the
        // gate initially misreported it as "the knockback is being read and
        // dropped" because a zero vector and a dropped vector look identical
        // downstream. Same class as M20.1's build gate, which assumed
        // undisturbed ground and went red one run in four.
        //
        // A flat floor and clear air make the geometry deterministic
        // regardless of what previous runs blew up.
        "fill ~-7 ~-1 ~-7 ~7 ~-1 ~7 minecraft:stone".into(),
        "fill ~-7 ~ ~-7 ~7 ~3 ~7 minecraft:air".into(),
        "say motion-check w8".into(),
        // Four blocks out on that clean floor: far enough that the crater does
        // not swallow the bot, close enough to be well inside the falloff.
        "summon minecraft:tnt ~ ~ ~4 {fuse:20s}".into(),
        "say motion-check w9".into(),
        "say motion-check w10".into(),
        "say motion-check w11".into(),
        "say motion-check w12".into(),
        // A second, independent trigger for `set_entity_motion`. The explosion
        // above should already produce one (its damage sets `hurtMarked`, and
        // `ServerEntity` then sends the motion to tracking players *and self*),
        // but that path runs through the damage calculator and the resistance
        // effect; a named attacker is the direct route and keeps the gate from
        // depending on one server-side chain.
        format!(
            "summon {MOTION_CHECK_ATTACKER} ~ ~ ~5 {{Tags:[\"{MOTION_CHECK_TAG}\"],\
             NoAI:1b,Silent:1b,Invulnerable:1b,PersistenceRequired:1b}}"
        ),
        "say motion-check w13".into(),
        format!("damage @s 2 minecraft:mob_attack by {zombie}"),
        "say motion-check w14".into(),
        format!("damage @s 2 minecraft:mob_attack by {zombie}"),
        "say motion-check w15".into(),
    ]
}

/// Restore the world and the bot after grading, whatever the verdict.
///
/// **The crater repair is not tidiness, it is a correctness requirement for
/// the *other* gates.** This gate detonates TNT, which blows a hole in the
/// flat world. Left behind, that hole makes the bot spawn a block low on the
/// next run, and the standard `rewo play` build gate then reports
/// "no air-over-solid column … the bot is somewhere this gate cannot place"
/// and exits 1 — a red gate caused entirely by this one's litter. That was
/// observed, not predicted: the plain gate went red immediately after these
/// runs.
///
/// The layers are `minecraft:flat`'s defaults (bedrock at y=-64, dirt at -63
/// and -62, grass at -61), written at **absolute** y with `~` only on x/z, so
/// the repair is correct wherever the bot happens to have ended up. The ±16
/// extent covers both the stone platform the knockback phase lays down and the
/// blast crater around it.
fn motion_check_cleanup() -> Vec<String> {
    vec![
        format!("kill @e[type={MOTION_CHECK_VEHICLE},tag={MOTION_CHECK_TAG}]"),
        format!("kill @e[type={MOTION_CHECK_ATTACKER},tag={MOTION_CHECK_TAG}]"),
        "effect clear @s".into(),
        "gamemode creative @s".into(),
        "fill ~-16 -64 ~-16 ~16 -64 ~16 minecraft:bedrock".into(),
        "fill ~-16 -63 ~-16 ~16 -62 ~16 minecraft:dirt".into(),
        "fill ~-16 -61 ~-16 ~16 -61 ~16 minecraft:grass_block".into(),
        "fill ~-16 -60 ~-16 ~16 -50 ~16 minecraft:air".into(),
        // Stand the bot back on the restored surface. Without this the repair
        // can *encase* it: a bot blown into its own crater sits at y=-63, and
        // the dirt layer above fills straight over it — so the next run's
        // stored spawn is inside solid ground, which is the same "bot is
        // somewhere this gate cannot place" failure the repair exists to
        // prevent, just moved one step later. Creative mode is set above, so
        // the moment spent inside the fill cannot suffocate it.
        "tp @s ~ -60 ~".into(),
    ]
}

/// The shortest session that can issue the stream above (one command per
/// 250 ms from t=1s) and still leave the TNT's one-second fuse, the damage
/// broadcasts and a settle window inside the run.
const MOTION_CHECK_MIN_SECONDS: f32 = 24.0;

/// The mob the swing gate arms, and what it arms it with. The two items are
/// chosen so the *prototype* table is load-bearing: the spear is one of the
/// seven non-default `minecraft:swing_animation` items (STAB / 19 ticks) and
/// the sword is an ordinary WHACK / 6. A decoder that mixed up the slot
/// ordinals, mis-read the component patch, or lost the item id would land on
/// the wrong pair.
const SWING_CHECK_MOB: &str = "minecraft:zombie";
const SWING_CHECK_MAIN: &str = "minecraft:iron_spear";
const SWING_CHECK_OFF: &str = "minecraft:stone_sword";
/// The scoreboard tag *and* custom name this gate's own fixture carries.
///
/// Both, deliberately. The tag scopes every server command (`kill`, `item
/// replace`) to this fixture so a repeat run cannot touch a zombie that was
/// already there; the custom name is what the *client* grades on, resolved
/// through the production metadata path (`EntityTable::custom_name`), so the
/// gate is looking at the same entity the server armed rather than "whatever
/// zombie happens to be nearest".
const SWING_CHECK_TAG: &str = "rewo_swing_check";

/// `minecraft:air`'s block-state id. Air is state 0 in every vanilla build —
/// `Blocks.AIR` is the first registration, and the mesher already relies on it.
const AIR_STATE: u32 = 0;

/// The paced command stream `--swing-check` issues after spawn. `say` lines are
/// deliberate no-ops that keep the 250 ms pacing honest while the server
/// applies the previous command and the tracker broadcasts its equipment
/// update (see the AGENT_LOOP_BRIEF pacing trap).
fn swing_check_commands() -> Vec<String> {
    // Every selector is tag-scoped: `kill` removes only this gate's own
    // fixture, never "all zombies", so an unrelated mob standing nearby (or a
    // fixture from a previous run) is neither destroyed nor mistaken for ours.
    let sel = format!("@e[type={SWING_CHECK_MOB},tag={SWING_CHECK_TAG},limit=1,sort=nearest]");
    let kill = format!("kill @e[type={SWING_CHECK_MOB},tag={SWING_CHECK_TAG}]");
    vec![
        kill.clone(),
        "say swing-check w1".into(),
        // `CustomName` is an SNBT *component compound* in 26.x. Quoting it
        // as a JSON string instead makes the literal `{"text":...}` the
        // entity's name, which is what the client would have to match.
        format!(
            "summon {SWING_CHECK_MOB} ~ ~ ~3 {{Tags:[\"{SWING_CHECK_TAG}\"],\
             CustomName:{{text:\"{SWING_CHECK_TAG}\"}},CustomNameVisible:1b,\
             NoAI:1b,Silent:1b,Invulnerable:1b,PersistenceRequired:1b}}"
        ),
        "say swing-check w2".into(),
        format!("item replace entity {sel} weapon.mainhand with {SWING_CHECK_MAIN}"),
        "say swing-check w3".into(),
        format!("item replace entity {sel} weapon.offhand with {SWING_CHECK_OFF}"),
        "say swing-check w4".into(),
        "say swing-check w5".into(),
        "say swing-check w6".into(),
    ]
}

/// The cleanup command, run after grading so a passing run leaves the world as
/// it found it. Tag-scoped for the same reason as the setup stream.
fn swing_check_cleanup() -> String {
    format!("kill @e[type={SWING_CHECK_MOB},tag={SWING_CHECK_TAG}]")
}

/// The shortest session that can issue the stream above (one command per
/// 250 ms from t=1s) and still leave a settle window for the equipment
/// broadcast to arrive.
const SWING_CHECK_MIN_SECONDS: f32 = 14.0;

/// The shortest session the M16 live gate can complete in: four settle windows
/// plus three transitions, at the bounds `dimension_check` uses. A run that
/// cannot finish would report a partial matrix, which is exactly the "skipped
/// check that looks green" this gate exists to prevent.
const DIMENSION_CHECK_MIN_SECONDS: f32 = 90.0;

/// The op account. Only this name is in the test server's `ops.json`, and a
/// non-op's commands are *silently* rejected.
const OP_USERNAME: &str = "RewoOp";

pub fn run(mut args: PlayArgs) -> Result<(), String> {
    if args.motion_check {
        // Same reason `--dimension-check` and `--swing-check` refuse it: both
        // pace server commands through one rate limiter, and the loser's tail
        // is dropped silently.
        if args.setup.is_some() {
            return Err(
                "--motion-check and --setup cannot be combined: both pace server \
                 commands, and the loser's tail is silently dropped. Run them separately."
                    .into(),
            );
        }
        if args.dimension_check || args.swing_check {
            return Err(
                "--motion-check cannot be combined with another live gate: they would \
                 share the server's chat rate limiter."
                    .into(),
            );
        }
        if args.seconds < MOTION_CHECK_MIN_SECONDS {
            return Err(format!(
                "--motion-check needs --seconds >= {MOTION_CHECK_MIN_SECONDS:.0} (given \
                 {:.0}): the mount phase, the TNT fuse, the damage broadcasts and a \
                 settle window do not fit in a shorter session, and a truncated run \
                 would grade a stream that never finished sending.",
                args.seconds
            ));
        }
        // The bot must not walk: the knockback phase measures what the
        // *server's* shove did to a stationary player, and a walk input would
        // mask it. Building is off for the same reason the dimension gate
        // turns it off — there is nothing to prove here and the crater would
        // make the targets unreliable.
        args.still = true;
        args.no_build = true;
    }
    if args.dimension_check {
        // Reject rather than reconcile: `--setup`'s paced stream and this
        // gate's commands would share the server's chat rate limiter, and a
        // dropped command here is invisible.
        if args.setup.is_some() {
            return Err(
                "--dimension-check and --setup cannot be combined: both pace server \
                 commands, and the loser's tail is silently dropped. Run them separately."
                    .into(),
            );
        }
        if args.seconds < DIMENSION_CHECK_MIN_SECONDS {
            return Err(format!(
                "--dimension-check needs --seconds >= {DIMENSION_CHECK_MIN_SECONDS:.0} \
                 (given {:.0}): four settle windows and three bounded transitions do not \
                 fit in a shorter session, and a truncated run would print a partial \
                 matrix.",
                args.seconds
            ));
        }
    }
    if args.swing_check {
        if args.setup.is_some() {
            return Err(
                "--swing-check and --setup cannot be combined: both pace server commands, \
                 and the loser's tail is silently dropped. Run them separately."
                    .into(),
            );
        }
        if args.dimension_check {
            return Err(
                "--swing-check and --dimension-check cannot be combined: both drive their \
                 own command stream, and the mob would be left behind in another dimension."
                    .into(),
            );
        }
        if args.seconds < SWING_CHECK_MIN_SECONDS {
            return Err(format!(
                "--swing-check needs --seconds >= {SWING_CHECK_MIN_SECONDS:.0} (given {:.0}): \
                 the paced command stream plus the equipment broadcast do not fit in a \
                 shorter session, and a truncated run would grade an unarmed mob.",
                args.seconds
            ));
        }
        // Deterministic path: the mob is summoned relative to the bot, so the
        // bot must not wander off, and the build actions would only add noise
        // (and contend for the same paced-command budget).
        args.still = true;
        args.no_build = true;
    }
    let data = GameData::load_for_version(&args.version)?;

    // Build the state→solid table from the asset bake: any block that
    // resolved to a full cube collides. Non-baked (partial/plants) fall
    // through to "non-air is solid" in the session, which is right for the
    // flat-world test.
    let baked = match client_jar_path(&args.version) {
        Some(jar) => {
            let paths = DataPaths::for_version(&args.version).ok_or("no config dir")?;
            match assets::bake(&jar, &paths.blocks_json()) {
                Ok(b) => Some(b),
                Err(e) => {
                    log::warn!("play: asset bake failed ({e}); using non-air-is-solid");
                    None
                }
            }
        }
        None => None,
    };
    let collide = baked
        .as_ref()
        .map(|b| b.collide.clone())
        .unwrap_or_default();

    let dirt_item = data.items.id("dirt");
    // Launcher account handoff (REWO_ACCESS_TOKEN/UUID/USERNAME) — lets the
    // bot harness join online-mode servers; offline servers ignore it. An
    // online-mode server verifies the hello name against the session join,
    // so the profile name wins unless --username overrides it.
    let auth = rewo_net::crypt::OnlineAuth::from_env();
    let username = args
        .username
        .clone()
        .or_else(|| auth.as_ref().map(|a| a.username.clone()))
        .unwrap_or_else(|| "RewoBot".into());
    let colormaps = baked
        .as_ref()
        .map(|b| {
            rewo_world::biome::Colormaps::from_pixels(
                b.grass_colormap.clone(),
                b.foliage_colormap.clone(),
                b.dry_foliage_colormap.clone(),
            )
        })
        .unwrap_or_else(rewo_world::biome::Colormaps::neutral);
    let conn = Connection::connect(&args.host, args.port, &data)?;
    let mut session = conn.into_play(
        &args.host,
        args.port,
        &username,
        auth.as_ref(),
        collide,
        data.blocks.global_palette_bits,
        colormaps,
    )?;
    // Entity collision: per-type footprint + whether it shoves (living only).
    session.entity_push = crate::live_cmd::entity_push_table(&data.entity_types);
    // Kinds whose polymorphic entity events drive model rigs (warden attack/
    // sonic boom, armadillo peek).
    session.warden_type_id = data.entity_types.id_of("minecraft:warden");
    session.armadillo_type_id = data.entity_types.id_of("minecraft:armadillo");
    // The Allay's type id disambiguates its index-16 `DATA_DANCING` from the
    // modeled baby path at the same slot — every production PlaySession consumer
    // needs it, not just `live_cmd`.
    session.allay_type_id = data.entity_types.id_of("minecraft:allay");
    // M20: the index-17 BOOLEAN is `Pillager.IS_CHARGING_CROSSBOW`.
    session.pillager_type_id = data.entity_types.id_of("minecraft:pillager");
    // M52: the two kinds that disambiguate an otherwise-shared metadata slot —
    // the sheep's wool byte at 18 and the creaking's `IS_ACTIVE` at 17.
    session.sheep_type_id = data.entity_types.id_of("minecraft:sheep");
    session.creaking_type_id = data.entity_types.id_of("minecraft:creaking");
    // M60: the player, for the index-16 skin-customisation byte (cape bit).
    session.player_type_id = Some(data.entity_types.player_id);
    // M19 combat swings — every production `PlaySession` consumer interprets
    // them, not just `live_cmd`: the player type id gates the swing clock and
    // the equipment tables supply each swing's duration + animation type.
    session.entity_classes = Some(std::sync::Arc::new(
        rewo_data::entity_types::EntityClasses::resolve(&data.entity_types)?,
    ));
    // M52 attributes — see the same pair in `live_cmd`.
    session.entity_types = Some(std::sync::Arc::new(data.entity_types.clone()));
    session.attribute_registry = Some(data.attributes.clone());
    // The component walker is keyed by name and the wire by id, so the table
    // is installed once the registry is known. Without this every component is
    // unwalkable and the first enchanted sword in a packet costs every stack
    // after it — so the count is logged rather than assumed.
    {
        let n = rewo_net::component_wire::install_shapes(data.component_registry.ids());
        log::info!(
            "rewo-net: {n}/{} data component codec(s) transcribed of {} registered",
            rewo_net::component_wire::CODECS.len(),
            data.component_registry.len()
        );
    }
    session.swing_data = Some(rewo_net::item_stack::SwingWireData {
        prototypes: rewo_data::swing_anim::SwingAnimations::resolve(&data.items)?,
        components: data.components,
        use_profiles: rewo_data::use_item::UseProfiles::resolve(&data.items)?,
    });
    // Client-side relighting of our own edits — the server only sends light
    // on chunk load, never for a placed torch or a broken roof.
    if let (Some(b), false) = (baked.as_ref(), args.no_relight) {
        session.set_light_tables(
            b.emission.clone(),
            b.dampening.clone(),
            b.face_occludes.clone(),
        );
    }
    log::info!("play: entered live session, waiting for spawn…");

    // M16 live gate. Built after login so it addresses its commands to the name
    // the session actually joined with — never a hardcoded account.
    let mut dim_check = args.dimension_check.then(|| {
        if username != OP_USERNAME {
            log::warn!(
                "play --dimension-check: joined as {username:?}, not the op account \
                 {OP_USERNAME:?}. A non-op's commands are silently rejected by the \
                 server; this gate will time out rather than skip a check."
            );
        }
        log::info!(
            "play --dimension-check: deterministic path — movement, build actions, the \
             scripted chat and --setup are all off for this run; commands target \
             {username:?}"
        );
        crate::dimension_check::DimensionCheck::new(&username)
    });

    if args.motion_check && username != OP_USERNAME {
        // Not a warning like the dimension gate's: every observation this gate
        // grades comes from a command, so a non-op run cannot produce a single
        // one and would fail with a confusing "no explosion arrived" instead of
        // the real cause.
        return Err(format!(
            "--motion-check joined as {username:?}, not the op account {OP_USERNAME:?}. \
             A non-op's commands are silently rejected by the server, so every \
             observation this gate needs would be missing and the failure would \
             misreport itself as a protocol bug."
        ));
    }

    // The paced command stream this run issues: the swing gate's own list, or
    // whatever `--setup` asked for. One source, so the two can never interleave.
    let scripted: Vec<String> = if args.swing_check {
        swing_check_commands()
    } else if args.motion_check {
        motion_check_commands()
    } else {
        args.setup
            .as_deref()
            .map(|c| {
                c.split(';')
                    .map(|one| one.trim().to_string())
                    .filter(|one| !one.is_empty())
                    .collect()
            })
            .unwrap_or_default()
    };

    let start = Instant::now();
    let total_ticks = (args.seconds / 0.05) as u64;
    let mut acted = Actions::default();
    // Tick index at which the server first spawned us — the action clock is
    // relative to spawn, not connect (chunk streaming can take a second).
    let mut spawn_tick: Option<u64> = None;
    // Earliest world-clock value observed, so the summary can show the day/night
    // clock actually advanced over the session (start → end → advance). This is
    // the headless proof of the frozen-clock fix — an empty `set_time` every 20
    // ticks used to hold the value; a live session now shows a non-zero advance.
    let mut clock_start: Option<i64> = None;

    for tick_n in 0..total_ticks {
        let deadline = start + TICK * (tick_n as u32 + 1);
        if session.spawned && spawn_tick.is_none() {
            spawn_tick = Some(tick_n);
            log::info!("play: spawned at tick {tick_n}, running the action script");
        }

        // Movement input + one-shot actions, on a spawn-relative clock. The
        // dimension gate owns the whole session instead: no movement, no build,
        // no scripted chat, so the only thing that moves the player is the
        // server's own teleport.
        let input = match (&dim_check, spawn_tick) {
            (Some(_), _) => TickInput::default(),
            (None, Some(st)) => {
                let secs = (tick_n - st) as f32 * 0.05;
                drive(&mut session, &args, secs, dirt_item, &mut acted, &scripted)?
            }
            (None, None) => TickInput::default(),
        };

        session.tick(&input)?;
        if let Some(reason) = &session.disconnect {
            return Err(format!("disconnected: {reason}"));
        }
        if let Some(dc) = dim_check.as_mut() {
            dc.tick(&mut session, tick_n)?;
            if dc.finished() {
                log::info!("play --dimension-check: all checkpoints reached at tick {tick_n}");
                break;
            }
        }
        if clock_start.is_none() {
            clock_start = session.day_ticks;
        }
        // Real-time pacing so the server sees a genuine 20 Hz client.
        let now = Instant::now();
        if now < deadline {
            std::thread::sleep(deadline - now);
        }
    }

    report(
        &mut session,
        &acted,
        &args,
        baked.as_ref(),
        &data,
        clock_start,
    );
    if !session.spawned {
        return Err("never spawned (no initial position from server)".into());
    }
    if let Some(dc) = dim_check.as_ref() {
        // Fails closed: an incomplete matrix is a failure, and `report` says so
        // in both the summary line and the exit code.
        dc.report(&session)?;
    }
    // Build actions fail closed. A standard build-enabled play gate must prove
    // the requested placement and dig properties from the server's own world —
    // not from the fact that a packet went out — and exit nonzero otherwise.
    // Runs that deliberately suppress building are exempt: `--no-build` (the
    // lighting gate) never touches these targets, and `--dimension-check` drives
    // its own still/no-build path, so neither placement nor dig is ever
    // attempted there and there is nothing to prove.
    if !args.no_build && !args.dimension_check {
        build_acceptance(&session, &acted, &data)?;
    }
    if args.motion_check {
        // Same shape as the swing gate: grade first, then tidy up whatever the
        // verdict, because a red run should not leave a boat, a zombie, a
        // resistance effect and a survival-mode bot behind for the next gate.
        let verdict = motion_acceptance(&session, &acted);
        for one in motion_check_cleanup() {
            match session.send_command(&one) {
                Ok(()) => println!("[motion-check] cleaned up /{one}"),
                Err(e) => log::warn!("play --motion-check: cleanup `/{one}` failed: {e}"),
            }
            // The cleanup shares the rate limiter the command stream just
            // used; pace it the same way rather than firing four at once.
            std::thread::sleep(Duration::from_millis(250));
        }
        verdict?;
    }
    if args.swing_check {
        // Grade the client's snapshot first, then tidy up regardless of the
        // verdict: the fixture is tag-scoped, so removing it touches nothing
        // else, and a failed run should not leave a mob behind either.
        let verdict = swing_acceptance(&session, &data);
        match session.send_command(&swing_check_cleanup()) {
            Ok(()) => println!("[swing-check] cleaned up /{}", swing_check_cleanup()),
            Err(e) => log::warn!("play --swing-check: cleanup failed: {e}"),
        }
        verdict?;
    }
    Ok(())
}

/// M68's live gate for the four packets that move the local player.
///
/// Fail-closed on **observation**. Every requirement below is "the server
/// actually sent this and we decoded it", because the failure this gate exists
/// to prevent is not a wrong number — it is a run that exercised nothing and
/// reported success. A `/ride` the server rejected, a TNT that fell outside
/// the blast radius, a gamemode switch that did not take: each of those leaves
/// a counter at zero, and each turns the gate red.
///
/// ## What the two phases prove, and why they are graded differently
///
/// **The knockback phase is the real physics claim.** The bot is stationary,
/// the server shoves it, and the server *does* validate an unmounted player's
/// movement — so if Rewo's `addDeltaMovement` / `lerpMotion` handling were
/// wrong (dropped, added instead of replaced, or scaled by a wrong constant),
/// the client's position would diverge from the server's simulation and the
/// server would correct it. Zero corrections across this phase is therefore a
/// statement about Rewo.
///
/// **The mount phase is not, and cannot be made into one.**
/// `ServerGamePacketListenerImpl` has `if (this.player.isPassenger())` snap the
/// rotation and return, skipping the whole move-check — so a mounted client
/// could believe it was anywhere at all and the server would never object.
/// This phase therefore grades *packet handling* (did the seat arrive, did the
/// dismount arrive, did the client's mount state follow) and deliberately does
/// **not** assert a correction count. At least one correction is expected here
/// and is not a fault: `ServerPlayer.startRiding` teleports the rider as part
/// of seating it, and that teleport arrives *before* the `set_passengers` that
/// would have told us we were mounted.
///
/// ## Why `move_vehicle` is not required
///
/// Both of its send sites are inside `ServerGamePacketListenerImpl.handleMoveVehicle`
/// — the server rejecting a *serverbound* `ServerboundMoveVehiclePacket`. A
/// client that never claims to drive a vehicle never receives one, and Rewo
/// rides as a passenger by design. Requiring it would make the gate
/// permanently red for a reason that is not a bug; asserting it arrived when
/// it structurally cannot would be worse. It is decoded and unit-tested in
/// `rewo_net::motion` and its live count is reported, not required.
fn motion_acceptance(session: &PlaySession, acted: &Actions) -> Result<(), String> {
    let s = session.motion_stats;
    let mut problems: Vec<String> = Vec::new();

    // ── phase 1: mount ──────────────────────────────────────────────────
    if s.passenger_updates == 0 {
        problems.push(
            "no `set_passengers` arrived at all — the `/ride mount` was rejected or the \
             boat never spawned (is the account op'd?)"
                .into(),
        );
    }
    if s.local_mounts == 0 {
        problems.push(
            "the local player never became a passenger — a `set_passengers` arrived but \
             never named us, so either the ride targeted another entity or the local \
             entity id is wrong"
                .into(),
        );
    }
    if s.local_dismounts == 0 {
        problems.push(
            "the local player never stopped being a passenger — `/ride dismount` \
             produces a rider list that no longer names us, so a missing dismount means \
             the replace-not-merge rule is broken (rewo_net::motion rule 5)"
                .into(),
        );
    }
    if session.is_mounted() {
        problems.push(
            "still mounted at the end of the run — the dismount was not applied, which \
             would leave the client's physics suppressed forever".into(),
        );
    }

    // ── phase 2: knockback + damage ─────────────────────────────────────
    if s.explosions == 0 {
        problems.push(
            "no `explode` arrived — the TNT never detonated within 64 blocks of the bot"
                .into(),
        );
    } else if s.explosion_knockbacks == 0 {
        // Distinguished from the above on purpose: an explosion that reached us
        // but carried no knockback means the bot was not in `hitPlayers`
        // (spectator, flying creative, or simply out of the blast), which is a
        // *fixture* fault and not a decode fault.
        problems.push(format!(
            "{} `explode` packet(s) arrived but none carried a `playerKnockback` — the \
             bot was outside the blast or still in creative-flying, so the physics half \
             of this phase tested nothing",
            s.explosions
        ));
    }
    // The direct witness: did the knockback reach the player's velocity?
    //
    // Deliberately independent of the correction count below, because the
    // correction count turned out to be the weaker of the two. The server's
    // move check flags a client that moves *too much*; a client that ignores a
    // shove moves too little, which vanilla does not report. This measures the
    // client's own state instead, so it cannot be satisfied by a server that
    // chose to say nothing.
    //
    // The two branches are separated because a zero knockback and a dropped
    // knockback are indistinguishable in the velocity alone, and blaming the
    // decoder for a shielded blast would send the next reader hunting a bug
    // that is not there.
    if s.explosion_knockbacks > 0 && s.explosion_knockbacks_nonzero == 0 {
        problems.push(format!(
            "{} `explode` packet(s) carried a `playerKnockback` but every one of them \
             was the zero vector — `getSeenPercent` found no line of sight, so the blast \
             was shielded (a crater wall, a block between). This is a FIXTURE fault, not \
             a decode fault: the packet arrived and parsed, it simply carried no shove.",
            s.explosion_knockbacks
        ));
    } else if s.explosion_knockbacks_nonzero > 0 && s.knockback_velocity_delta <= 0.0 {
        problems.push(
            "a non-zero `playerKnockback` was decoded but the local player's velocity \
             never changed — the knockback is being read and dropped. This is the exact \
             failure the pre-M68 client had, and the one a correction count alone does \
             not catch."
                .into(),
        );
    }
    if s.local_motions == 0 {
        problems.push(format!(
            "no `set_entity_motion` addressed the local player ({} arrived for other \
             entities) — `markHurt` never fired, so the damage was fully resisted or \
             the bot was still invulnerable",
            s.entity_motions
        ));
    }

    // The correction split. Only the knockback phase is asserted; see above.
    let split = acted.motion_corrections_at_split;
    match split {
        None => problems.push(format!(
            "the run never reached the phase split at t={MOTION_PHASE_SPLIT:.0}s, so the \
             knockback phase's corrections cannot be separated from the mount phase's"
        )),
        Some(at_split) => {
            let knockback_corrections = session.corrections.saturating_sub(at_split);
            if knockback_corrections > 0 {
                problems.push(format!(
                    "{knockback_corrections} server correction(s) during the knockback \
                     phase — the client's velocity diverged from the server's after a \
                     shove it was told about. THIS is the failure the ordinary \
                     `CORRECTIONS 0` run is structurally unable to see."
                ));
            }
        }
    }

    let mount_corrections = split.unwrap_or(session.corrections);
    println!(
        "[motion-check] knockback reached the player's velocity: max |Δv| = {:.4} \
         blocks/tick (0 would mean decoded-and-dropped)",
        s.knockback_velocity_delta
    );
    println!(
        "[motion-check] explode {} ({} with knockback, {} of them non-zero) · \
         set_entity_motion {} ({} local, {} stops) · set_passengers {} ({} mounts, \
         {} dismounts) · move_vehicle {} (structurally unreachable — passenger-only \
         client)",
        s.explosions,
        s.explosion_knockbacks,
        s.explosion_knockbacks_nonzero,
        s.entity_motions,
        s.local_motions,
        s.local_motion_stops,
        s.passenger_updates,
        s.local_mounts,
        s.local_dismounts,
        s.vehicle_moves,
    );
    println!(
        "[motion-check] corrections — mount phase {mount_corrections} (not graded: the \
         server does not validate a passenger's movement, and seating teleports the \
         rider) · knockback phase {} (graded: must be 0) · while mounted {}",
        split
            .map(|at| session.corrections.saturating_sub(at))
            .unwrap_or(0),
        s.corrections_while_mounted,
    );

    if problems.is_empty() {
        println!("[motion-check] PASS — all four packets accounted for");
        return Ok(());
    }
    Err(format!(
        "play --motion-check: {} problem(s):\n  - {}",
        problems.len(),
        problems.join("\n  - ")
    ))
}

/// M19's live equipment gate, fail-closed.
///
/// Everything `swingshot` proves about the equipment path it proves against
/// bodies this repo wrote. Here a real 26.2 server writes them: it broadcasts
/// `ClientboundSetEquipmentPacket` to trackers from `ServerEntity` (the initial
/// tracking snapshot) and `LivingEntity.handleEquipmentChanges`
/// (`sendToTrackingPlayers`), so the slot ordinals, the continuation bit, the
/// `ItemStack.OPTIONAL_STREAM_CODEC` layout and the item registry ids are all
/// the server's, not ours.
///
/// The two items make the datagen-derived prototype table load-bearing:
/// `iron_spear` is one of the seven non-default `minecraft:swing_animation`
/// entries (STAB / 19) and `stone_sword` is an ordinary WHACK / 6. Reading the
/// same value for both would be a fail.
fn swing_acceptance(session: &PlaySession, data: &GameData) -> Result<(), String> {
    use rewo_world::entities::{HandItem, InteractionHand};

    let mob_tid = data
        .entity_types
        .id_of(SWING_CHECK_MOB)
        .ok_or_else(|| format!("registries.json: no {SWING_CHECK_MOB} entity type"))?;
    let want_main = data
        .items
        .id(SWING_CHECK_MAIN)
        .ok_or_else(|| format!("registries.json: no {SWING_CHECK_MAIN}"))?;
    let want_off = data
        .items
        .id(SWING_CHECK_OFF)
        .ok_or_else(|| format!("registries.json: no {SWING_CHECK_OFF}"))?;
    // `of` is `None` for an id outside the registry; these two came *from* the
    // registry, so a `None` here means the pin itself is broken.
    let proto_main = data
        .swing_animations
        .of(want_main)
        .ok_or_else(|| format!("{SWING_CHECK_MAIN} has no prototype swing animation"))?;
    let proto_off = data
        .swing_animations
        .of(want_off)
        .ok_or_else(|| format!("{SWING_CHECK_OFF} has no prototype swing animation"))?;
    if proto_main == proto_off {
        return Err(format!(
            "swing-check is not discriminating: {SWING_CHECK_MAIN} and {SWING_CHECK_OFF} \
             share the prototype {proto_main:?}"
        ));
    }

    // Grade only *our* fixture: the custom name arrives over the production
    // metadata path (index 2, OPTIONAL_COMPONENT), so an unrelated zombie in the
    // world — or one left by an earlier run — is neither counted nor graded.
    let mobs: Vec<i32> = session
        .world
        .entities
        .iter()
        .filter(|(id, e)| {
            e.type_id == mob_tid && session.world.entities.custom_name(*id) == Some(SWING_CHECK_TAG)
        })
        .map(|(id, _)| id)
        .collect();
    let all_of_kind = session
        .world
        .entities
        .iter()
        .filter(|(_, e)| e.type_id == mob_tid)
        .count();
    if mobs.len() != 1 {
        // Print what the client actually sees, so a mismatch names itself
        // instead of turning into a hunt.
        let seen: Vec<(i32, Option<&str>)> = session
            .world
            .entities
            .iter()
            .filter(|(_, e)| e.type_id == mob_tid)
            .map(|(id, _)| (id, session.world.entities.custom_name(id)))
            .take(8)
            .collect();
        return Err(format!(
            "swing-check: expected exactly one tracked {SWING_CHECK_MOB} named \
             {SWING_CHECK_TAG:?}, found {} (ids {mobs:?}; {all_of_kind} of that type tracked \
             in total; first few (id, custom_name) = {seen:?}) - the paced summon did not \
             land, so nothing was graded",
            mobs.len()
        ));
    }
    let eid = mobs[0];
    println!(
        "[swing-check] {all_of_kind} {SWING_CHECK_MOB}(s) tracked; grading the one named \
         {SWING_CHECK_TAG:?}"
    );

    let ents = &session.world.entities;
    let main = ents.hand_item(eid, InteractionHand::MainHand);
    let off = ents.hand_item(eid, InteractionHand::OffHand);
    let duration = ents.current_swing_duration(eid);
    let kind = ents.swing_animation_type(eid);
    let known = ents.swing_inputs_known(eid);
    println!("[swing-check] tracked {SWING_CHECK_MOB} entity {eid}");
    println!("[swing-check]   mainhand {main:?}  (want item {want_main} swing {proto_main:?})");
    println!("[swing-check]   offhand  {off:?}  (want item {want_off} swing {proto_off:?})");
    println!(
        "[swing-check]   getCurrentSwingDuration={duration:?} swingAnimationType={kind:?} \
         inputs_known={known} (want {} / {:?} / true)",
        proto_main.duration, proto_main.kind
    );

    let mut bad = Vec::new();
    if main.held().map(|i| (i.item_id, i.swing)) != Some((want_main, proto_main)) {
        bad.push("mainhand");
    }
    if off.held().map(|i| (i.item_id, i.swing)) != Some((want_off, proto_off)) {
        bad.push("offhand");
    }
    // No swing has been received, so `swingingArm` is null - MAIN_HAND - and
    // the attack arm is the default RIGHT main arm: both read the main hand.
    if duration != Some(proto_main.duration) {
        bad.push("duration");
    }
    if kind != Some(proto_main.kind) {
        bad.push("type");
    }
    // Both stacks were fully resolved: a real server's equipment must never
    // land in the suppressed state.
    if !known || main == HandItem::Unknown || off == HandItem::Unknown {
        bad.push("inputs-known");
    }
    if !bad.is_empty() {
        return Err(format!(
            "SWING-CHECK FAILED: {} did not match the server-armed mob",
            bad.join(", ")
        ));
    }
    println!(
        "SWING-CHECK: OK - the server-sent equipment decoded to {SWING_CHECK_MAIN} \
         (STAB/{}) main hand + {SWING_CHECK_OFF} (WHACK/{}) off hand, and \
         getCurrentSwingDuration/swingAnimationType read the main hand",
        proto_main.duration, proto_off.duration
    );
    Ok(())
}

#[derive(Default)]
struct Actions {
    /// How many `--setup` commands have gone out (they are paced).
    setup_sent: usize,
    walked: bool,
    sprinted: bool,
    jumped: bool,
    looked: bool,
    dug: bool,
    placed: bool,
    chatted: bool,
    gave_block: bool,
    /// Anchor feet block captured when the build phase starts (bot is idle
    /// by then, so place/dig targets stay fixed).
    anchor: Option<(i32, i32, i32)>,
    /// Where dirt was placed / grass was dug — queried in the report to
    /// prove the actions mutated the server world end-to-end.
    placed_at: Option<(i32, i32, i32)>,
    dug_at: Option<(i32, i32, i32)>,
    /// M68: `session.corrections` sampled at [`MOTION_PHASE_SPLIT`], so the
    /// mount phase's corrections can be told apart from the knockback
    /// phase's. They are graded differently and the reason is not cosmetic —
    /// see `motion_acceptance`.
    motion_corrections_at_split: Option<u32>,
}

/// Movement input for this spawn-relative second, firing one-shot gameplay
/// actions on their scheduled tick. Timeline:
///   0-2s settle · 2-6s walk · 6-9s sprint · 9-12s jump · 12s look ·
///   14s give+place · 18s dig · 22s chat · rest settle.
fn drive(
    session: &mut PlaySession,
    args: &PlayArgs,
    secs: f32,
    dirt_item: Option<i32>,
    acted: &mut Actions,
    scripted: &[String],
) -> Result<TickInput, String> {
    let mut input = TickInput::default();
    if args.still {
        // fall through to the one-shot actions with no movement
    } else if (2.0..6.0).contains(&secs) {
        input.forward = 1.0;
        acted.walked = true;
    } else if (6.0..9.0).contains(&secs) {
        input.forward = 1.0;
        input.sprint = true;
        acted.sprinted = true;
    } else if (9.0..12.0).contains(&secs) {
        input.forward = 1.0;
        input.jump = true;
        acted.jumped = true;
    }

    // One-shot actions fire on the first tick of their scheduled second.
    if secs >= 12.0 && !acted.looked {
        session.player.yaw = 90.0;
        session.player.pitch = 20.0;
        acted.looked = true;
    }
    if !args.no_build {
        if secs >= 14.0 && !acted.gave_block {
            // Anchor the build targets to the (now-stationary) feet block.
            acted.anchor = Some(feet_block(session));
            // Give + hold a stack of dirt (creative server), then select it.
            if let Some(dirt) = dirt_item {
                let _ = session.creative_set_hotbar(0, dirt, 64);
                let _ = session.select_hotbar(0);
            }
            acted.gave_block = true;
        }
        if let Some((fx, fy, fz)) = acted.anchor {
            if secs >= 15.0 && !acted.placed {
                // Place against the TOP face of the grass block *two* to the
                // east (fx+2, fy-1) → dirt lands at (fx+2, fy), which is air.
                //
                // Two, not one. The server's `BlockItem.canPlace` rejects a
                // placement whose cell overlaps any entity —
                // `isUnobstructed(state, clickedPos, placementContext(player))`
                // in 26.2's `BlockItem`. The player's 0.6-wide AABB, centred on
                // a sub-block x anywhere in `[fx, fx+1)`, reaches east to at most
                // `fx+1.3`, so the cell one to the east (`fx+1`) is inside the
                // body whenever the bot's fractional x ≥ 0.7 — an intermittent
                // rejection that left the target as air on roughly one run in
                // four. Column `fx+2` starts at `fx+2 > fx+1.3`, so the footprint
                // can never touch it: the placement is always geometrically
                // valid, and a resulting air state now means a real bug rather
                // than the bot standing on its own target.
                //
                // M20.1: `fx + 2` is necessary but not sufficient. It assumed
                // the bot stands on *undisturbed* ground — but an earlier run
                // of this same gate digs a hole, and if the bot walks into it
                // its feet sit a block low, making `(fx+2, fy)` the grass
                // SURFACE rather than air. The server then correctly rejects
                // the placement and the gate went red for a world-state
                // reason, roughly one run in four. The premise is now checked
                // instead of assumed: scan east for the first column whose
                // target cell is air and whose support is solid.
                let support_ok = |x: i32| {
                    session.world.block_state_at(x, fy, fz) == AIR_STATE
                        && session.world.block_state_at(x, fy - 1, fz) != AIR_STATE
                };
                match (fx + 2..fx + 8).find(|x| support_ok(*x)) {
                    Some(x) => {
                        let target = (x, fy, fz);
                        match session.use_item_on(x, fy - 1, fz, 1) {
                            Ok(()) => log::info!(
                                "play: place → {target:?} (on top of {:?}; scanned east                                  from {} for air-over-solid)",
                                (x, fy - 1, fz),
                                fx + 2
                            ),
                            Err(e) => log::warn!("play: place failed: {e}"),
                        }
                        acted.placed_at = Some(target);
                    }
                    None => {
                        // Leaving `placed_at` unset makes `build_acceptance`
                        // report the action as never-run, which is exit 1 —
                        // an unplaceable world is a red gate, not a silent skip.
                        log::warn!(
                            "play: no air-over-solid column in {}..{} at y={fy} z={fz} —                              the bot is somewhere this gate cannot place",
                            fx + 2,
                            fx + 8
                        );
                    }
                }
                acted.placed = true;
            }
            if secs >= 18.0 && !acted.dug {
                // Break the grass block one to the WEST (a different block,
                // so removal is observable). Creative breaks on dig start.
                let target = (fx - 1, fy - 1, fz);
                match session.start_dig(fx - 1, fy - 1, fz, 1) {
                    Ok(()) => log::info!("play: dig → {target:?}"),
                    Err(e) => log::warn!("play: dig failed: {e}"),
                }
                acted.dug_at = Some(target);
                acted.dug = true;
            }
        }
    }
    // M68: close the mount phase's correction window. Sampled here rather than
    // derived at the end because the two phases must be attributed separately
    // and there is no way to recover the split from a single total.
    if args.motion_check && secs >= MOTION_PHASE_SPLIT && acted.motion_corrections_at_split.is_none()
    {
        acted.motion_corrections_at_split = Some(session.corrections);
    }
    // Setup commands go out one per 250 ms. Firing them all in one tick trips
    // the server's chat rate limit and the tail is silently dropped — which
    // looks exactly like a light bug, because the structure never appears.
    {
        let due = ((secs - 1.0) / 0.25).floor();
        if secs >= 1.0
            && due >= 0.0
            && (due as usize) < scripted.len()
            && acted.setup_sent <= due as usize
        {
            let one = &scripted[due as usize];
            match session.send_command(one) {
                Ok(()) => log::info!("play: setup → /{one}"),
                Err(e) => log::warn!("play: setup failed: {e}"),
            }
            acted.setup_sent = due as usize + 1;
        }
    }
    // The swing gate's command budget is the whole rate-limit allowance; a
    // scripted chat line on top of it would push the tail out.
    if secs >= 22.0 && !acted.chatted && !args.swing_check && !args.motion_check {
        let _ = session.send_chat(&args.chat);
        acted.chatted = true;
    }

    // After the scripted phase, wander continuously so long runs keep
    // stressing physics + collision parity (the "survival session" proxy).
    // Walk forward, curving the yaw slowly, with a periodic sprint-jump.
    //
    // **`--still` must suppress this too.** It did not, which was a latent
    // hole in the flag rather than a deliberate exception: a `--still` run
    // longer than 24 s started walking here, contradicting the flag's whole
    // purpose ("the light gate centres on the bot's final position, so it must
    // stay inside whatever `--setup` built"). Nothing had noticed because the
    // light runs are short. M68 needs it closed because its knockback phase
    // measures what the server's shove did to a *stationary* player, and a
    // walk input arriving mid-measurement would mask it.
    if secs >= 24.0 && !args.still {
        let t = secs - 24.0;
        input.forward = 1.0;
        input.sprint = (t as u32 / 3) % 2 == 0;
        input.jump = (t * 20.0) as u32 % 40 == 0;
        session.player.yaw += 1.3; // slow left curve
    }
    Ok(input)
}

/// Integer block coords of the bot's feet.
fn feet_block(session: &PlaySession) -> (i32, i32, i32) {
    (
        session.player.x.floor() as i32,
        session.player.y.floor() as i32,
        session.player.z.floor() as i32,
    )
}

fn report(
    session: &mut PlaySession,
    acted: &Actions,
    args: &PlayArgs,
    baked: Option<&assets::BakedAssets>,
    data: &GameData,
    clock_start: Option<i64>,
) {
    let (px, py, pz) = (session.player.x, session.player.y, session.player.z);
    let ground = session
        .world
        .block_state_at(px.floor() as i32, py as i32 - 1, pz.floor() as i32);
    println!("[rewo-m3] play session summary");
    println!(
        "[rewo-m3] spawned: {}  ticks: {}",
        session.spawned, session.ticks
    );
    println!(
        "[rewo-m3] final pos: ({:.2}, {:.2}, {:.2})  on_ground: {}",
        px, py, pz, session.player.on_ground
    );
    println!("[rewo-m3] block below feet: state {ground}");
    println!(
        "[rewo-m3] teleports: {}  CORRECTIONS: {}  (physics-parity meter — lower is better)",
        session.teleports, session.corrections
    );
    println!(
        "[rewo-m3] block_updates received: {}",
        session.block_updates
    );
    // World-clock progress: an advance of ~1 per game tick elapsed proves the
    // day/night clock is running; a frozen clock reads `advance 0` here.
    match (clock_start, session.day_ticks) {
        (Some(start), Some(end)) => println!(
            "[rewo-m3] world clock: start {start} → end {end}  (advance {} over the session)",
            end - start
        ),
        _ => println!("[rewo-m3] world clock: no set_time observed"),
    }
    println!(
        "[rewo-m3] actions sent (packets attempted — NOT proof; the ACCEPT lines below are the \
         server-observed proof): walk:{} sprint:{} jump:{} look:{} dig:{} place:{} chat:{} give:{}",
        acted.walked,
        acted.sprinted,
        acted.jumped,
        acted.looked,
        acted.dug,
        acted.placed,
        acted.chatted,
        acted.gave_block,
    );
    // Prove build/dig mutated the server world: query the echoed states.
    // Light readout at the bot's final position — the headless check for the
    // light decode. In a sealed, torch-lit room sky must be 0 and block must
    // fall off by 1 per block from the torch; under open sky it's 15/0.
    {
        let p = &session.player;
        let (bx, by, bz) = (
            p.x.floor() as i32,
            p.eye_y().floor() as i32,
            p.z.floor() as i32,
        );
        let (bl, sl) = session.world.light_at(bx, by, bz);
        println!(
            "[rewo-m3] LIGHT @ ({bx},{by},{bz}) = {} (sky {sl}, block {bl})",
            bl.max(sl)
        );
        // A short horizontal profile makes block-light falloff visible.
        let profile: Vec<String> = (0..6)
            .map(|d| {
                let (b, sk) = session.world.light_at(bx + d, by, bz);
                format!("{}:{}/{}", d, b, sk)
            })
            .collect();
        println!(
            "[rewo-m3] LIGHT profile +x (block/sky): {}",
            profile.join("  ")
        );
        if let Some(spec) = args.light_at.as_deref() {
            let n: Vec<i32> = spec
                .split(',')
                .filter_map(|v| v.trim().parse().ok())
                .collect();
            if let [x, y, z] = n[..] {
                let (b, sk) = session.world.light_at(x, y, z);
                let st = session.world.block_state_at(x, y, z);
                println!(
                    "[rewo-m3] LIGHT at ({x},{y},{z}) = {} (sky {sk}, block {b})  state {st}",
                    b.max(sk)
                );
            } else {
                println!("[rewo-m3] LIGHT at: bad --light-at {spec:?} (want \"x,y,z\")");
            }
        }
    }
    if args.light_check {
        match baked {
            Some(b) => light_parity_check(session, b, data),
            None => println!("[rewo-light] LIGHT-CHECK: no asset bake — skipped"),
        }
    }
    // The server-observed placement/dig proof is printed by `build_acceptance`
    // (the ACCEPT lines), which also owns the exit code. It reads the same world
    // state the server echoed back — `use_item_on` always draws a `block_update`
    // for the placement cell whether or not the placement was accepted (26.2
    // `ServerGamePacketListenerImpl.handleUseItemOn` sends one for `pos` and
    // `pos.relative(direction)` unconditionally) — so "still air" here is a real
    // server rejection, not a missed packet. The old `state != 0` "non-air =
    // success" proxy lived here and is deliberately gone: it graded the wrong
    // property and passed on any block, air-adjacent or not.
    println!("[rewo-m3] chat received: {} lines", session.chat_log.len());
    for line in session.chat_log.iter().take(6) {
        println!("[rewo-m3]   > {line}");
    }
    println!(
        "[rewo-m3] loaded columns: {}",
        session.world.loaded_columns()
    );
}

fn client_jar_path(version: &str) -> Option<std::path::PathBuf> {
    let mut p = dirs::config_dir()?;
    p.push("EwoClient");
    p.push("shared/versions");
    p.push(version);
    p.push(format!("{version}.jar"));
    p.exists().then_some(p)
}

/// Recompute a loaded column's light from scratch and diff it against the
/// server's authoritative values.
///
/// The server runs vanilla's own light engine, so its `chunk_data` /
/// `light_update` payloads are ground truth. Zeroing a column and refilling it
/// from our tables exercises emission, dampening, sky sources and both flood
/// passes at once — a mismatch is a real bug, not a rendering opinion. This is
/// the "verify the property, not a proxy" gate for lighting.
///
/// Columns are relit twice (once to settle the 3×3 neighbourhood, once for the
/// centre) so light entering across a chunk border is accounted for; edge
/// columns of the loaded region are skipped because their neighbours are not
/// present to feed them.
fn light_parity_check(
    session: &mut rewo_net::play::PlaySession,
    baked: &assets::BakedAssets,
    data: &GameData,
) {
    use rewo_world::light::{LightEngine, LightTables};

    let tables = LightTables {
        emission: &baked.emission,
        dampening: &baked.dampening,
        face_occludes: &baked.face_occludes,
    };
    let shape = session.world.shape;
    let (y0, y1) = (shape.min_y, shape.min_y + shape.height);

    // Centre on the bot, and only check columns whose whole 3×3 is loaded.
    let (px, pz) = (
        session.player.x.floor() as i32 >> 4,
        session.player.z.floor() as i32 >> 4,
    );
    let mut targets = Vec::new();
    for dx in -1..=1i32 {
        for dz in -1..=1i32 {
            let (cx, cz) = (px + dx, pz + dz);
            if (-1..=1).all(|ax: i32| {
                (-1..=1).all(|az: i32| session.world.is_loaded((cx + ax) * 16, (cz + az) * 16))
            }) {
                targets.push((cx, cz));
            }
        }
    }
    if targets.is_empty() {
        println!("[rewo-light] LIGHT-CHECK: no fully-surrounded column loaded — skipped");
        return;
    }

    // Snapshot the server's values before we overwrite anything.
    let mut truth = Vec::new();
    for &(cx, cz) in &targets {
        let mut col = Vec::with_capacity(16 * 16 * (y1 - y0) as usize);
        for lx in 0..16 {
            for lz in 0..16 {
                for y in y0..y1 {
                    col.push(session.world.light_at(cx * 16 + lx, y, cz * 16 + lz));
                }
            }
        }
        truth.push(col);
    }

    let mut engine = LightEngine::new();
    for &(cx, cz) in &targets {
        engine.relight_column(&mut session.world, tables, cx, cz);
    }
    for &(cx, cz) in &targets {
        engine.relight_column(&mut session.world, tables, cx, cz);
    }

    // Diff. Report per-channel, and keep a few concrete examples — a bare
    // count would not say whether the gap is a source, a cost, or a border.
    let (mut cells, mut bad_block, mut bad_sky) = (0usize, 0usize, 0usize);
    let mut examples: Vec<String> = Vec::new();
    // Direction and size of the error say what kind of bug it is: uniformly
    // brighter means a source is one too high, darker means a path is blocked
    // that should not be.
    let (mut brighter, mut darker, mut max_delta) = (0usize, 0usize, 0i32);
    // The brightest disagreeing cell is nearest whatever source is wrong.
    let mut worst: Option<(u8, String)> = None;
    for (ci, &(cx, cz)) in targets.iter().enumerate() {
        let mut i = 0;
        for lx in 0..16 {
            for lz in 0..16 {
                for y in y0..y1 {
                    let (want_b, want_s) = truth[ci][i];
                    i += 1;
                    cells += 1;
                    let (x, z) = (cx * 16 + lx, cz * 16 + lz);
                    let (got_b, got_s) = session.world.light_at(x, y, z);
                    if got_b != want_b {
                        bad_block += 1;
                    }
                    if got_s != want_s {
                        bad_sky += 1;
                    }
                    if got_s != want_s {
                        let d = got_s as i32 - want_s as i32;
                        if d > 0 {
                            brighter += 1
                        } else {
                            darker += 1
                        }
                        max_delta = max_delta.max(d.abs());
                        if worst
                            .as_ref()
                            .map_or(true, |(lv, _)| want_s.max(got_s) > *lv)
                        {
                            let n = data
                                .blocks
                                .block_name(session.world.block_state_at(x, y, z))
                                .unwrap_or("?");
                            worst = Some((
                                want_s.max(got_s),
                                format!("({x},{y},{z}) {n}: want s{want_s} got s{got_s}"),
                            ));
                        }
                    }
                    if (got_b != want_b || got_s != want_s) && examples.len() < 8 {
                        let st = session.world.block_state_at(x, y, z);
                        let name = data.blocks.block_name(st).unwrap_or("?");
                        let (e, d) = (
                            baked.emission.get(st as usize).copied().unwrap_or(0),
                            baked.dampening.get(st as usize).copied().unwrap_or(0),
                        );
                        examples.push(format!(
                            "({x},{y},{z}) {name} [emit {e} damp {d}]: want b{want_b}/s{want_s} got b{got_b}/s{got_s}"
                        ));
                    }
                }
            }
        }
    }

    let pct = |n: usize| 100.0 * n as f64 / cells.max(1) as f64;
    println!(
        "[rewo-light] LIGHT-CHECK {} columns, {cells} cells: block {bad_block} ({:.3}%), sky {bad_sky} ({:.3}%) {}",
        targets.len(),
        pct(bad_block),
        pct(bad_sky),
        if bad_block == 0 && bad_sky == 0 { "✓ EXACT" } else { "✗" }
    );
    if bad_sky > 0 {
        println!(
            "[rewo-light]   sky: {brighter} too bright, {darker} too dark, max delta {max_delta}"
        );
        if let Some((_, w)) = &worst {
            println!("[rewo-light]   brightest disagreement: {w}");
        }
    }
    for e in &examples {
        println!("[rewo-light]   {e}");
    }
}

/// The server-observed state at the cell where the scripted place fired.
///
/// `Actions.placed` only records that a `use_item_on` packet was sent;
/// this records what the server's authoritative world actually holds there
/// afterwards, so the gate can prove the *property* (a dirt block appeared)
/// rather than the send.
struct PlaceObservation {
    at: (i32, i32, i32),
    /// The block state a successful placement must produce — dirt's default
    /// state, resolved from the block table. `None` if the table could not
    /// resolve `minecraft:dirt`, which is itself a failure (nothing to prove
    /// against).
    expected_state: Option<u32>,
    /// The state the server echoed at `at` (0 = air = the placement was rejected
    /// or never happened).
    observed_state: u32,
    /// The observed state's block name, for the diagnostic.
    observed_name: Option<String>,
}

/// The server-observed state at the cell where the scripted dig fired.
struct DigObservation {
    at: (i32, i32, i32),
    /// The state the server echoed at `at` (must be 0 = air for a proven break).
    observed_state: u32,
    observed_name: Option<String>,
}

/// Read the server's world at the recorded action targets and grade them.
///
/// A thin adapter over [`evaluate_build_actions`]: it turns the session's
/// authoritative block states into owned observations, then defers the pass/fail
/// decision to the pure function so the decision itself is unit-testable without
/// a socket. Called only on build-enabled, non-dimension runs.
fn build_acceptance(
    session: &rewo_net::play::PlaySession,
    acted: &Actions,
    data: &GameData,
) -> Result<(), String> {
    let expected_place = data.blocks.default_state("minecraft:dirt");
    let place = acted.placed_at.map(|at| {
        let observed = session.world.block_state_at(at.0, at.1, at.2);
        PlaceObservation {
            at,
            expected_state: expected_place,
            observed_state: observed,
            observed_name: data.blocks.block_name(observed).map(str::to_owned),
        }
    });
    let dig = acted.dug_at.map(|at| {
        let observed = session.world.block_state_at(at.0, at.1, at.2);
        DigObservation {
            at,
            observed_state: observed,
            observed_name: data.blocks.block_name(observed).map(str::to_owned),
        }
    });
    evaluate_build_actions(place, dig)
}

/// Pure acceptance logic for the build actions — the fail-closed gate.
///
/// Given only what the server's world shows at each recorded target, decide
/// whether the *exact* requested property holds: the placed cell is dirt, the
/// dug cell is air. Prints one `ACCEPT` line per sub-result (so a red placement
/// is always visible even when the other passed) and returns `Err` — a nonzero
/// process exit — if any sub-result failed.
///
/// Split from the session on purpose: it owns its inputs, so a test can hand it
/// a "dirt" observation and see green, flip that one field to air, and see red —
/// which is the regression guard that stops the check from silently going back
/// to "any non-air = success". `None` means the action never ran; for a
/// build-enabled gate that is itself a failure, because the property is unproven.
fn evaluate_build_actions(
    place: Option<PlaceObservation>,
    dig: Option<DigObservation>,
) -> Result<(), String> {
    let mut failures: Vec<String> = Vec::new();

    match place {
        Some(p) => match p.expected_state {
            Some(exp) if p.observed_state == exp => {
                println!(
                    "[rewo-m3] ACCEPT place @ {:?}: state {} (minecraft:dirt) ✓ (server-observed)",
                    p.at, p.observed_state
                );
            }
            Some(exp) => {
                let name = p.observed_name.as_deref().unwrap_or("?");
                failures.push(format!(
                    "place @ {:?}: expected state {exp} (minecraft:dirt), observed state {} ({name})",
                    p.at, p.observed_state
                ));
            }
            None => failures.push(
                "place: could not resolve minecraft:dirt default state from the block table"
                    .to_string(),
            ),
        },
        None => failures
            .push("place: action never ran (build enabled but no placement attempted)".to_string()),
    }

    match dig {
        Some(d) if d.observed_state == 0 => {
            println!(
                "[rewo-m3] ACCEPT dig @ {:?}: state 0 (air) ✓ (server-observed)",
                d.at
            );
        }
        Some(d) => {
            let name = d.observed_name.as_deref().unwrap_or("?");
            failures.push(format!(
                "dig @ {:?}: expected state 0 (air), observed state {} ({name})",
                d.at, d.observed_state
            ));
        }
        None => {
            failures.push("dig: action never ran (build enabled but no dig attempted)".to_string())
        }
    }

    if failures.is_empty() {
        Ok(())
    } else {
        for f in &failures {
            println!("[rewo-m3] ACCEPT ✗ {f}");
        }
        Err(format!(
            "build actions unproven ({}/2 failed): {}",
            failures.len(),
            failures.join("; ")
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        evaluate_build_actions, motion_check_commands, DigObservation, PlaceObservation,
        MOTION_MOUNT_PHASE_COMMANDS,
    };

    // Dirt's default state on 26.2 is 10 (observed live); the exact value does
    // not matter to the logic, only that observed == expected.
    const DIRT: u32 = 10;

    fn place(observed: u32) -> PlaceObservation {
        PlaceObservation {
            at: (0, -60, 0),
            expected_state: Some(DIRT),
            observed_state: observed,
            observed_name: Some(if observed == 0 { "air" } else { "minecraft:dirt" }.to_string()),
        }
    }

    fn dig(observed: u32) -> DigObservation {
        DigObservation {
            at: (0, -61, 0),
            observed_state: observed,
            observed_name: Some(if observed == 0 { "air" } else { "minecraft:grass_block" }.to_string()),
        }
    }

    #[test]
    fn both_proven_is_green() {
        assert!(evaluate_build_actions(Some(place(DIRT)), Some(dig(0))).is_ok());
    }

    #[test]
    fn placement_reverting_to_air_turns_the_gate_red() {
        // The exact regression this milestone exists to catch: a green placement
        // whose cell is actually air must fail the gate, not pass on "non-air".
        let err = evaluate_build_actions(Some(place(0)), Some(dig(0)))
            .expect_err("air placement must be a failure");
        assert!(err.contains("place @"), "{err}");
        assert!(err.contains("observed state 0"), "{err}");
    }

    #[test]
    fn a_wrong_but_non_air_block_is_still_red() {
        // "Any non-air = success" was the old proxy. A stone (say state 1) at the
        // dirt target proves nothing and must be red.
        let err = evaluate_build_actions(Some(place(1)), Some(dig(0)))
            .expect_err("wrong block must be a failure");
        assert!(err.contains("expected state 10"), "{err}");
    }

    #[test]
    fn a_dig_that_left_solid_turns_the_gate_red() {
        let err = evaluate_build_actions(Some(place(DIRT)), Some(dig(9)))
            .expect_err("un-broken block must be a failure");
        assert!(err.contains("dig @"), "{err}");
        assert!(err.contains("observed state 9"), "{err}");
    }

    #[test]
    fn a_missing_action_is_red_not_skipped() {
        // Build enabled but the action never fired (e.g. too-short session): the
        // property is unproven, which must fail closed rather than pass silently.
        assert!(evaluate_build_actions(None, Some(dig(0))).is_err());
        assert!(evaluate_build_actions(Some(place(DIRT)), None).is_err());
        assert!(evaluate_build_actions(None, None).is_err());
    }

    #[test]
    fn an_unresolvable_dirt_state_is_red() {
        let p = PlaceObservation {
            at: (0, -60, 0),
            expected_state: None,
            observed_state: DIRT,
            observed_name: Some("minecraft:dirt".to_string()),
        };
        assert!(evaluate_build_actions(Some(p), Some(dig(0))).is_err());
    }

    /// M68: the phase split must actually separate the two phases.
    ///
    /// A regression guard with a real history. The split was first a flat 9 s,
    /// which sat *after* every knockback command had already fired — so the
    /// window the gate graded was empty, and a mutation that decoded the
    /// explosion knockback and threw it away passed green. The constant is now
    /// derived from this list, and this test is what stops the list and the
    /// constant drifting apart again: adding a mount-phase command without
    /// bumping the count makes the boundary land on the wrong command and
    /// fails here, loudly, instead of silently emptying the window.
    #[test]
    fn the_motion_phase_split_lands_on_the_first_knockback_command() {
        let cmds = motion_check_commands();
        assert!(
            MOTION_MOUNT_PHASE_COMMANDS < cmds.len(),
            "the split index must be inside the command list"
        );
        assert!(
            cmds[MOTION_MOUNT_PHASE_COMMANDS].starts_with("gamemode survival"),
            "the boundary command should be the first of the knockback phase, got {:?}",
            cmds[MOTION_MOUNT_PHASE_COMMANDS]
        );
        // Nothing before the boundary may shove the player: if it did, its
        // effect would be attributed to the (ungraded) mount phase and the
        // graded window would miss it.
        for (i, c) in cmds.iter().take(MOTION_MOUNT_PHASE_COMMANDS).enumerate() {
            assert!(
                !c.contains("tnt") && !c.starts_with("damage"),
                "command {i} ({c:?}) shoves the player but sits in the mount phase"
            );
        }
        // And the knockback phase must contain both triggers, or the graded
        // window would be empty for the opposite reason.
        let tail = &cmds[MOTION_MOUNT_PHASE_COMMANDS..];
        assert!(
            tail.iter().any(|c| c.contains("tnt")),
            "the knockback phase must summon the TNT"
        );
        assert!(
            tail.iter().any(|c| c.starts_with("damage ")),
            "the knockback phase must issue the damage trigger"
        );
    }
}
