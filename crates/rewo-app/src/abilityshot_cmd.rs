//! `rewo abilityshot` — M75's permanent **serverless** abilities oracle.
//!
//! The `player_abilities` packets, the movement modes they unlock, and the
//! gamemode binding. No socket, no GPU device, no jar — only the pinned
//! version's `packets.json` (to resolve the two real packet ids) and CPU.
//!
//! The production path it drives, end to end:
//!
//! ```text
//! nine raw bytes                       (built here, from the decompiled layout)
//!   -> rewo_net::abilities::PlayerAbilities::parse
//!   -> ::apply_to                      (`handlePlayerAbilities`, verbatim)
//!   -> rewo_world::abilities::FlightControl::before_travel   (`LocalPlayer.aiStep`)
//!   -> rewo_world::physics::tick_with                        (`Player.travel`)
//!   -> FlightControl::after_travel                           (the landing clause)
//! ```
//!
//! and, for the gamemode half:
//!
//! ```text
//! a CommonPlayerSpawnInfo body
//!   -> rewo_net::spawn_info::CommonPlayerSpawnInfo::read     (the M16 decoder)
//!   -> rewo_net::play::apply_spawn_game_mode                 (what the session calls)
//!   -> ClientGameState::set_local_mode_with_previous + GameMode::update_player_abilities
//! ```
//!
//! **Fail-closed by construction**, the same shape as `eventshot`/`danceshot`: a
//! fixed [`EXPECTED_WITNESSES`] count, every property *observed* rather than
//! assumed, failures gathered so all of them report, and a run that observes a
//! different number of properties errors even when none failed.
//!
//! **The expectations are independent.** Nothing here reads
//! `rewo_world::abilities`' constants as its own target: the decay is graded
//! against a literal `0.6`, the impulse against a separately written
//! `0.05f32 * 3.0`, the flag masks against literal `1/2/4/8`, and the
//! gamemode table against hand-written booleans transcribed from
//! `GameType.updatePlayerAbilities`.
//!
//! # Every witness names a mutation partner
//!
//! Each `record` call's name is followed in the source by the mutation that
//! must break it. The ones worth stating up front, because they are the
//! properties a plausible implementation gets wrong:
//!
//! - **`w3.creative_does_not_start_you_flying`** — mutation: make CREATIVE's
//!   `flying` `Some(true)`. This is the asymmetry that would *look* like it
//!   worked.
//! - **`w3.leaving_creative_clears_flight`** — mutation: make survival's
//!   `flying` `None`. Merely ceasing to permit flight is not what vanilla does,
//!   and the leaked state is what the live gate sees as corrections.
//! - **`w5.walking_speed_does_not_drive_the_walk`** — mutation: feed
//!   `walking_speed` into the move speed. At defaults the two agree, so only a
//!   non-default value exposes it.
//! - **`w5.flight_has_no_gravity`** — mutation: keep `travelInAir`'s gravity
//!   and 0.98 drag instead of overwriting Y.
//! - **`w2.serverbound_is_one_byte`** — mutation: write the clientbound body.

use clap::Args as ClapArgs;
use rewo_data::{packets::Packets, DataPaths};
use rewo_net::abilities::{
    serverbound, PlayerAbilities, FLAG_CAN_FLY, FLAG_FLYING, FLAG_INSTABUILD, FLAG_INVULNERABLE,
};
use rewo_net::game_event::ClientGameState;
use rewo_net::ids::Ids;
use rewo_net::play::{apply_spawn_game_mode, GameMode};
use rewo_world::abilities::{Abilities, FlightControl};
use rewo_world::physics::{tick_with, PlayerState, TickInput};

/// Total number of named properties this gate asserts. Locked so a skipped
/// property (fewer observations) fails the run even if nothing mismatched.
const EXPECTED_WITNESSES: usize = 47;

#[derive(ClapArgs, Debug)]
pub struct AbilityshotArgs {
    /// Assert every owned property. The oracle asserts unconditionally and a
    /// failure exits nonzero with or without this flag, so it only labels the
    /// run — the same convention `eventshot`/`danceshot` use.
    #[arg(long, default_value_t = false)]
    check: bool,
    /// Version whose `packets.json` datagen report resolves the two real
    /// `player_abilities` packet ids.
    #[arg(long, default_value = "26.2")]
    version: String,
}

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
        println!("[abilityshot] {status}  {name}: {detail}");
        if pass {
            self.witnessed += 1;
        } else {
            self.failures.push(name.to_string());
        }
    }
}

// -------------------------------------------------------------------- helpers

/// A `ClientboundPlayerAbilitiesPacket` body, built here from the decompiled
/// layout — one flags byte then two big-endian floats, flying speed first.
fn body(bits: u8, flying_speed: f32, walking_speed: f32) -> Vec<u8> {
    let mut b = vec![bits];
    b.extend_from_slice(&flying_speed.to_be_bytes());
    b.extend_from_slice(&walking_speed.to_be_bytes());
    b
}

fn near(a: f64, b: f64, eps: f64) -> bool {
    (a - b).abs() <= eps
}

/// Empty world — flight and no-clip both need somewhere with nothing in it.
fn air(_x: i32, _y: i32, _z: i32) -> &'static [[f32; 6]] {
    &[]
}

/// Solid below y = 0.
fn ground(_x: i32, y: i32, _z: i32) -> &'static [[f32; 6]] {
    const FULL: &[[f32; 6]] = &[[0.0, 0.0, 0.0, 1.0, 1.0, 1.0]];
    if y < 0 {
        FULL
    } else {
        &[]
    }
}

/// Default abilities plus `mayfly` — the only field the toggle needs. Written
/// as a mutation rather than a struct literal because the two speeds are
/// private (and deliberately so: `walking_speed` is a trap worth naming).
fn may_fly() -> Abilities {
    let mut a = Abilities::default();
    a.mayfly = true;
    a
}

/// Abilities as a creative player has them the instant flight is toggled on.
fn creative_flying() -> Abilities {
    let mut a = Abilities::default();
    a.apply_mode(GameMode::Creative.update_player_abilities());
    a.flying = true;
    a
}

// ------------------------------------------------- 1. the clientbound wire

fn check_clientbound_wire(c: &mut Checker) {
    // w1.each_flag_bit_is_isolated.
    // MUTATION: swap FLAG_FLYING and FLAG_CAN_FLY in `rewo_net::abilities`.
    // Targets are literal 1/2/4/8 from ClientboundPlayerAbilitiesPacket's own
    // FLAG_* constants, not the crate's.
    let quad = |bits: u8| {
        let p = PlayerAbilities::parse(&body(bits, 0.05, 0.1)).expect("parse");
        (p.invulnerable, p.flying, p.can_fly, p.instabuild)
    };
    let isolated = quad(1) == (true, false, false, false)
        && quad(2) == (false, true, false, false)
        && quad(4) == (false, false, true, false)
        && quad(8) == (false, false, false, true);
    c.record(
        "w1.each_flag_bit_is_isolated",
        isolated,
        format!(
            "1->{:?} 2->{:?} 4->{:?} 8->{:?}",
            quad(1),
            quad(2),
            quad(4),
            quad(8)
        ),
    );

    // w1.crate_masks_match_the_decompiled_literals.
    // MUTATION: change any FLAG_* constant.
    let masks = (FLAG_INVULNERABLE, FLAG_FLYING, FLAG_CAN_FLY, FLAG_INSTABUILD);
    c.record(
        "w1.crate_masks_match_the_decompiled_literals",
        masks == (1, 2, 4, 8),
        format!("{masks:?} (want (1, 2, 4, 8))"),
    );

    // w1.floats_are_flying_then_walking.
    // MUTATION: swap the two `r.f32()?` reads. The fixture uses two values that
    // cannot be confused for one another.
    let p = PlayerAbilities::parse(&body(0, 0.25, 0.7)).expect("parse");
    c.record(
        "w1.floats_are_flying_then_walking",
        p.flying_speed == 0.25 && p.walking_speed == 0.7,
        format!("flying={} walking={}", p.flying_speed, p.walking_speed),
    );

    // w1.body_is_exactly_nine_bytes.
    // MUTATION: add or drop a field. A short body must error rather than
    // silently read zeros, because this packet has no length prefix.
    let full = body(0xF, 0.05, 0.1);
    let short_ok = PlayerAbilities::parse(&full[..full.len() - 1]).is_err()
        && PlayerAbilities::parse(&[]).is_err();
    c.record(
        "w1.body_is_exactly_nine_bytes",
        full.len() == 9 && PlayerAbilities::parse(&full).is_ok() && short_ok,
        format!("len={} short_errors={short_ok}", full.len()),
    );

    // w1.unused_bits_are_ignored_not_rejected.
    // MUTATION: reject a body whose high nibble is set. Vanilla tests four
    // masks and never looks at the rest, so a protocol addition must not kill
    // the connection.
    let hi = PlayerAbilities::parse(&body(0xF0 | FLAG_FLYING, 0.05, 0.1));
    c.record(
        "w1.unused_bits_are_ignored_not_rejected",
        hi.as_ref().is_ok_and(|p| p.flying && !p.can_fly),
        format!("{hi:?}"),
    );

    // w1.apply_to_leaves_may_build_alone.
    // MUTATION: assign `a.may_build = …` in `apply_to`. `mayBuild` is the fifth
    // Abilities boolean and the packet carries four — it reaches the client only
    // through `updatePlayerAbilities`, so a decode that clears it silently takes
    // away block placing.
    let mut a = Abilities::default();
    a.may_build = false;
    PlayerAbilities::parse(&body(0xF, 0.2, 0.3))
        .expect("parse")
        .apply_to(&mut a);
    c.record(
        "w1.apply_to_leaves_may_build_alone",
        !a.may_build && a.flying && a.mayfly && a.instabuild && a.invulnerable,
        format!(
            "may_build={} (untouched) flying={} mayfly={} instabuild={} invulnerable={}",
            a.may_build, a.flying, a.mayfly, a.instabuild, a.invulnerable
        ),
    );

    // w1.apply_to_clears_as_well_as_sets.
    // MUTATION: `a.flying |= self.flying` instead of `=`. An or-assign passes
    // every "does it set the flag" check and never lets the server take flight
    // away.
    PlayerAbilities::parse(&body(0, 0.05, 0.1))
        .expect("parse")
        .apply_to(&mut a);
    c.record(
        "w1.apply_to_clears_as_well_as_sets",
        !a.flying && !a.mayfly && !a.instabuild && !a.invulnerable,
        format!("all four cleared by a zero-flags packet: flying={}", a.flying),
    );
}

// ------------------------------------------------- 2. the serverbound wire

fn check_serverbound_wire(c: &mut Checker, sb_id: i32, cb_id: i32) {
    // w2.serverbound_is_one_byte.
    // MUTATION: write the clientbound nine-byte body. That would desync the
    // stream by eight bytes on every flight toggle.
    let on = serverbound(sb_id, true).into_bytes();
    let off = serverbound(sb_id, false).into_bytes();
    // The id var-int is one byte for anything under 128; assert on the payload.
    let payload_on = &on[on.len() - 1..];
    let payload_off = &off[off.len() - 1..];
    let one_byte = on.len() == off.len() && payload_on == [FLAG_FLYING] && payload_off == [0];
    c.record(
        "w2.serverbound_is_one_byte",
        one_byte && on.len() == 2,
        format!("on={on:?} off={off:?} (want id + exactly one payload byte)"),
    );

    // w2.serverbound_declares_only_the_flying_bit.
    // MUTATION: also set FLAG_CAN_FLY or FLAG_INSTABUILD. The client's only
    // ability claim is "I am flying now"; the server AND-gates even that with
    // its own `mayfly`.
    let claims_nothing_else = payload_on[0] & !FLAG_FLYING == 0;
    c.record(
        "w2.serverbound_declares_only_the_flying_bit",
        claims_nothing_else,
        format!("payload=0b{:04b}, other bits clear", payload_on[0]),
    );

    // w2.the_two_ids_are_distinct_and_direction_resolved.
    // MUTATION: resolve both from the same direction. The two packets share the
    // *name* `player_abilities`; only the direction tells them apart, so a
    // resolver that ignored direction would send the clientbound id.
    c.record(
        "w2.the_two_ids_are_distinct_and_direction_resolved",
        sb_id != cb_id,
        format!("clientbound={cb_id} serverbound={sb_id} (same name, different direction)"),
    );
}

// ------------------------------- 3. GameType.updatePlayerAbilities, all four

fn check_mode_table(c: &mut Checker) {
    // Hand-written from the decompiled `GameType.updatePlayerAbilities`, as
    // (mayfly, instabuild, invulnerable, flying, may_build). `flying` is
    // `Option` because CREATIVE genuinely does not mention it.
    let want = [
        (GameMode::Survival, (false, false, false, Some(false), true)),
        (GameMode::Creative, (true, true, true, None, true)),
        (GameMode::Adventure, (false, false, false, Some(false), false)),
        (GameMode::Spectator, (true, false, true, Some(true), false)),
    ];
    for (mode, w) in want {
        let m = mode.update_player_abilities();
        let got = (m.mayfly, m.instabuild, m.invulnerable, m.flying, m.may_build);
        // w3.mode_table.<mode>
        // MUTATION: any single field of that arm.
        c.record(
            &format!("w3.mode_table.{mode:?}"),
            got == w,
            format!("{got:?} (want {w:?})"),
        );
    }

    // w3.creative_does_not_start_you_flying.
    // MUTATION: make CREATIVE's `flying` `Some(true)`. Deriving `flying` from
    // `mayfly` is right for three modes and wrong for the one a tester is most
    // likely to be in — and it would look like it worked.
    let mut a = Abilities::default();
    a.apply_mode(GameMode::Creative.update_player_abilities());
    let grants_without_flying = a.mayfly && a.instabuild && a.invulnerable && !a.flying;
    c.record(
        "w3.creative_does_not_start_you_flying",
        grants_without_flying,
        format!("mayfly={} flying={} (want true/false)", a.mayfly, a.flying),
    );

    // w3.creative_preserves_flight_it_did_not_grant.
    // MUTATION: `Some(false)` for CREATIVE. A re-announcement of the current
    // mode (which servers send freely) would then drop a flying player out of
    // the sky.
    a.flying = true;
    a.apply_mode(GameMode::Creative.update_player_abilities());
    c.record(
        "w3.creative_preserves_flight_it_did_not_grant",
        a.flying,
        format!("still flying after a creative re-announce: {}", a.flying),
    );

    // w3.leaving_creative_clears_flight.
    // MUTATION: make survival's `flying` `None`. Merely ceasing to permit
    // flight leaves the client applying flight physics in survival — which is
    // exactly what the live gate's survival walk would then see as corrections.
    a.apply_mode(GameMode::Survival.update_player_abilities());
    c.record(
        "w3.leaving_creative_clears_flight",
        !a.flying && !a.mayfly && !a.instabuild && !a.invulnerable,
        format!("flying={} mayfly={} (both must be false)", a.flying, a.mayfly),
    );

    // w3.spectator_is_the_only_mode_that_sets_flying.
    // MUTATION: set `flying: Some(true)` on another arm, or `None` here.
    let sets: Vec<GameMode> = [
        GameMode::Survival,
        GameMode::Creative,
        GameMode::Adventure,
        GameMode::Spectator,
    ]
    .into_iter()
    .filter(|m| m.update_player_abilities().flying == Some(true))
    .collect();
    c.record(
        "w3.spectator_is_the_only_mode_that_sets_flying",
        sets == [GameMode::Spectator],
        format!("modes assigning flying=true: {sets:?}"),
    );

    // w3.may_build_tracks_block_placing_restriction.
    // MUTATION: drop the `may_build` assignment after the match (it sits
    // *outside* the if/else in vanilla, so it applies to every arm).
    let restricted: Vec<GameMode> = [
        GameMode::Survival,
        GameMode::Creative,
        GameMode::Adventure,
        GameMode::Spectator,
    ]
    .into_iter()
    .filter(|m| !m.update_player_abilities().may_build)
    .collect();
    c.record(
        "w3.may_build_tracks_block_placing_restriction",
        restricted == [GameMode::Adventure, GameMode::Spectator],
        format!("modes with may_build=false: {restricted:?}"),
    );
}

// ------------------------------------------- 4. the three gamemode sources

/// A `CommonPlayerSpawnInfo` body, built here to the M16 layout, ending in the
/// two gamemode bytes this milestone consumes.
fn spawn_body(game_type: i8, previous: i8) -> Vec<u8> {
    let name = b"minecraft:overworld";
    let mut b = Vec::new();
    b.push(0); // dimension type holder — a VarInt (raw 0-based registry id)
    b.push(name.len() as u8); // identifier: VarInt length then UTF-8
    b.extend_from_slice(name);
    b.extend_from_slice(&1234i64.to_be_bytes()); // biomeZoomSeed
    b.push(game_type as u8); // gameType
    b.push(previous as u8); // previousGameType, -1 = absent
    b.push(0); // isDebug
    b.push(0); // isFlat
    b.push(0); // Optional<GlobalPos> lastDeathLocation: absent
    b.push(0); // portalCooldown (VarInt)
    b.push(63); // seaLevel (VarInt)
    b
}

fn check_gamemode_sources(c: &mut Checker) {
    use rewo_net::spawn_info::CommonPlayerSpawnInfo;
    use rewo_proto::reader::PacketReader;

    let decode = |gt: i8, prev: i8| {
        let bytes = spawn_body(gt, prev);
        let mut r = PacketReader::new(&bytes);
        CommonPlayerSpawnInfo::read(&mut r).expect("spawn info decodes")
    };

    // w4.a_creative_login_knows_it_is_creative.
    // MUTATION: remove the `apply_spawn_game_mode` call from `apply_login_shape`.
    // `game_event`'s CHANGE_GAME_MODE is only the *mid-session* change, so
    // without this a client that joins in creative and never switches has no
    // idea it is in creative — and can never take off.
    let spawn = decode(1, -1);
    let mut state = ClientGameState::default();
    let mut ab = Abilities::default();
    apply_spawn_game_mode(&mut state, &mut ab, &spawn);
    c.record(
        "w4.a_creative_login_knows_it_is_creative",
        state.game_mode() == Some(GameMode::Creative) && ab.mayfly && !ab.flying,
        format!(
            "mode={:?} mayfly={} flying={}",
            state.game_mode(),
            ab.mayfly,
            ab.flying
        ),
    );

    // w4.a_spectator_login_arrives_flying.
    // MUTATION: drop SPECTATOR's `flying: Some(true)`. A spectator that joins
    // not-flying would fall out of the world on its first tick.
    let spawn = decode(3, 1);
    let mut state = ClientGameState::default();
    let mut ab = Abilities::default();
    apply_spawn_game_mode(&mut state, &mut ab, &spawn);
    c.record(
        "w4.a_spectator_login_arrives_flying",
        ab.flying && ab.mayfly && state.game_mode() == Some(GameMode::Spectator),
        format!("flying={} mode={:?}", ab.flying, state.game_mode()),
    );

    // w4.previous_game_type_minus_one_is_absent.
    // MUTATION: read the sentinel as a mode. `-1` would become SPECTATOR under
    // a naive `by_id`, or SURVIVAL under `by_id`'s out-of-range rule — either
    // way a mode the player was never in.
    c.record(
        "w4.previous_game_type_minus_one_is_absent",
        state.previous_game_mode() == Some(GameMode::Creative)
            && {
                let s = decode(1, -1);
                let mut st = ClientGameState::default();
                apply_spawn_game_mode(&mut st, &mut Abilities::default(), &s);
                st.previous_game_mode().is_none()
            },
        format!(
            "prev(from 1)={:?}, prev(from -1)=None",
            state.previous_game_mode()
        ),
    );

    // w4.the_two_set_local_mode_forms_differ_on_a_repeat.
    // MUTATION: give the two-argument form the one-argument form's change
    // guard. `handleRespawn` assigns both fields directly; the guard is the
    // *only* difference between the overloads and it runs one way.
    let mut one = ClientGameState::default();
    one.set_local_mode(GameMode::Survival, &mut Abilities::default());
    one.set_local_mode(GameMode::Creative, &mut Abilities::default());
    one.set_local_mode(GameMode::Creative, &mut Abilities::default());
    let guarded = one.previous_game_mode() == Some(GameMode::Survival);

    let mut two = ClientGameState::default();
    two.set_local_mode_with_previous(
        GameMode::Creative,
        Some(GameMode::Survival),
        &mut Abilities::default(),
    );
    two.set_local_mode_with_previous(GameMode::Creative, None, &mut Abilities::default());
    let unguarded = two.previous_game_mode().is_none();
    c.record(
        "w4.the_two_set_local_mode_forms_differ_on_a_repeat",
        guarded && unguarded,
        format!(
            "one-arg repeat keeps prev={:?}; two-arg repeat overwrites to {:?}",
            one.previous_game_mode(),
            two.previous_game_mode()
        ),
    );

    // w4.a_respawn_into_survival_drops_flight.
    // MUTATION: skip `apply_spawn_game_mode` in `apply_respawn`. A player who
    // dies while flying in creative and respawns in survival would keep flying.
    let mut ab = creative_flying();
    let spawn = decode(0, 1);
    let mut state = ClientGameState::default();
    apply_spawn_game_mode(&mut state, &mut ab, &spawn);
    c.record(
        "w4.a_respawn_into_survival_drops_flight",
        !ab.flying && !ab.mayfly && state.game_mode() == Some(GameMode::Survival),
        format!("flying={} mayfly={}", ab.flying, ab.mayfly),
    );
}

// ------------------------------------------------------- 5. flight physics

fn check_flight_physics(c: &mut Checker) {
    // w5.flight_has_no_gravity.
    // MUTATION: keep `travelInAir`'s gravity subtraction and 0.98 vertical drag
    // instead of overwriting Y with `originalMovementY * 0.6`. Targets are the
    // literal 0.6 powers, written here rather than read from the crate.
    let mut p = PlayerState::at(0.5, 80.0, 0.5);
    p.vy = 1.0;
    let ab = creative_flying();
    let mut seen = Vec::new();
    for _ in 0..5 {
        tick_with(&mut p, &TickInput::default(), &ab, false, &air);
        seen.push(p.vy);
    }
    let want: Vec<f64> = (1..=5).map(|n| 0.6f64.powi(n)).collect();
    let decays = seen
        .iter()
        .zip(&want)
        .all(|(g, w)| near(*g, *w, 1e-12));
    c.record(
        "w5.flight_has_no_gravity",
        decays && p.vy > 0.0,
        format!("vy={seen:?} (want {want:?}; still rising, not falling)"),
    );

    // w5.walking_in_the_same_state_falls_and_flight_never_does.
    // This IS the sensitivity partner for the row above: if the flight branch
    // were never taken, those five decay numbers could not hold.
    //
    // The discriminator is the **sign**, not the magnitude. Both start at
    // vy = 1.0 and both slow down, so at five ticks the walking player is still
    // rising and is the *faster* of the two — an early version of this witness
    // compared magnitudes and failed for that reason. Given enough ticks the
    // walking one crosses zero and accelerates downward, while a pure ×0.6
    // decay asymptotes to zero **from above** and can never change sign.
    // Tracking the *minimum* vy over the run rather than its final value keeps
    // this independent of where the min-movement clamp sits: a pure decay never
    // goes negative at any point, whatever the tail does.
    let min_vy = |ab: &Abilities, ticks: usize| {
        let mut q = PlayerState::at(0.5, 80.0, 0.5);
        q.vy = 1.0;
        let mut lo = q.vy;
        for _ in 0..ticks {
            tick_with(&mut q, &TickInput::default(), ab, false, &air);
            lo = lo.min(q.vy);
        }
        lo
    };
    let walk_vy = min_vy(&Abilities::default(), 40);
    let fly_vy = min_vy(&creative_flying(), 40);
    c.record(
        "w5.walking_in_the_same_state_falls_and_flight_never_does",
        walk_vy < -0.5 && fly_vy >= 0.0,
        format!(
            "over 40 ticks: walking min vy={walk_vy:.4} (goes negative), \
             flying min vy={fly_vy:.3e} (never does)"
        ),
    );

    // w5.the_vertical_impulse_is_computed_in_f32.
    // MUTATION: `f64::from(input_ya) * flying_speed as f64 * 3.0` — widen first.
    // Vanilla's `inputYa * getFlyingSpeed() * 3.0F` is an int-times-float-times-
    // float product, so it is a float, and 0.05 is not representable.
    let impulse = Abilities::default().vertical_flight_impulse(true, false);
    let f32_path = f64::from(0.05f32 * 3.0f32);
    let f64_path = 0.05f64 * 3.0;
    c.record(
        "w5.the_vertical_impulse_is_computed_in_f32",
        impulse == f32_path && f32_path != f64_path,
        format!("impulse={impulse:.17} f32-path={f32_path:.17} f64-path={f64_path:.17}"),
    );

    // w5.ascent_reaches_its_closed_form_terminal.
    // MUTATION: change the 0.6 decay, or apply the impulse after the move.
    // Fixed point of v ← (v + I)·0.6 is v = 1.5·I; the distance travelled in a
    // tick is v + I = 2.5·I, because the impulse lands before `travel`.
    let mut fc = FlightControl::default();
    let mut ab = creative_flying();
    let mut p = PlayerState::at(0.5, 80.0, 0.5);
    let up = TickInput {
        jump: true,
        ..Default::default()
    };
    for _ in 0..300 {
        fc.before_travel(&mut ab, &mut p, &up, false, false);
        tick_with(&mut p, &up, &ab, false, &air);
    }
    let carried = p.vy;
    let before = p.y;
    fc.before_travel(&mut ab, &mut p, &up, false, false);
    tick_with(&mut p, &up, &ab, false, &air);
    let per_tick = p.y - before;
    c.record(
        "w5.ascent_reaches_its_closed_form_terminal",
        near(carried, 1.5 * impulse, 1e-12) && near(per_tick, 2.5 * impulse, 1e-12),
        format!(
            "carried={carried:.6} (want {:.6}) per-tick={per_tick:.6} (want {:.6}) = {:.2} blocks/s",
            1.5 * impulse,
            2.5 * impulse,
            per_tick * 20.0
        ),
    );

    // w5.horizontal_terminal_matches_its_closed_form.
    // MUTATION: use the walking air constants (0.02) instead of the flying
    // speed. Note the fixed point of v ← (v + a)·0.91 is 0.91a/(1 − 0.91), NOT
    // a/(1 − 0.91) — the drag applies to the accelerated velocity, a ~10%
    // difference that a "looks about right" eyeball would pass.
    let terminal = |sprint: bool| {
        let mut p = PlayerState::at(0.5, 80.0, 0.5);
        let ab = creative_flying();
        let input = TickInput {
            forward: 1.0,
            sprint,
            ..Default::default()
        };
        for _ in 0..600 {
            tick_with(&mut p, &input, &ab, false, &air);
        }
        p.vz
    };
    let closed = |accel: f64| accel * 0.98 * 0.91 / (1.0 - 0.91);
    let base = f64::from(0.05f32);
    let (t0, t1) = (terminal(false), terminal(true));
    c.record(
        "w5.horizontal_terminal_matches_its_closed_form",
        near(t0, closed(base), 1e-6) && near(t1, closed(base * 2.0), 1e-6),
        format!(
            "walk-fly={t0:.5} (want {:.5}) sprint-fly={t1:.5} (want {:.5}) = {:.1}/{:.1} blocks/s",
            closed(base),
            closed(base * 2.0),
            t0 * 20.0,
            t1 * 20.0
        ),
    );

    // w5.sprinting_doubles_flight_but_not_the_air_constants.
    // MUTATION: apply the 1.3 walking sprint multiplier, or drop the ×2.
    let flying_ratio = t1 / t0;
    let ab = Abilities::default();
    let air_ratio = ab.air_move_speed(true, false) / ab.air_move_speed(false, false);
    c.record(
        "w5.sprinting_doubles_flight_but_not_the_air_constants",
        near(flying_ratio, 2.0, 1e-6) && near(air_ratio, 1.3, 1e-3) && air_ratio < 2.0,
        format!("flight ×{flying_ratio:.4}, walking-air ×{air_ratio:.4}"),
    );

    // w5.sneaking_does_not_slow_a_flying_player.
    // MUTATION: drop the `&& !flying` from the sneak guard in `tick_with`.
    // `crouching = !abilities.flying && …`, so `isMovingSlowly()` is false
    // while flying and the 0.3 SNEAKING_SPEED factor never applies.
    let fly_with_sneak = |sneak: bool| {
        let mut p = PlayerState::at(0.5, 80.0, 0.5);
        let ab = creative_flying();
        let input = TickInput {
            forward: 1.0,
            sneak,
            ..Default::default()
        };
        for _ in 0..300 {
            tick_with(&mut p, &input, &ab, false, &air);
        }
        p.vz
    };
    let walk_with_sneak = |sneak: bool| {
        let mut p = PlayerState::at(0.5, 0.0, 0.5);
        let input = TickInput {
            forward: 1.0,
            sneak,
            ..Default::default()
        };
        for _ in 0..80 {
            tick_with(&mut p, &input, &Abilities::default(), false, &ground);
        }
        p.vz
    };
    let (fs, fn_) = (fly_with_sneak(true), fly_with_sneak(false));
    let (ws, wn) = (walk_with_sneak(true), walk_with_sneak(false));
    c.record(
        "w5.sneaking_does_not_slow_a_flying_player",
        near(fs, fn_, 1e-12) && ws < wn * 0.5,
        format!("flying {fs:.5} vs {fn_:.5} (equal); walking {ws:.5} vs {wn:.5} (control: slower)"),
    );

    // w5.walking_speed_does_not_drive_the_walk.
    // MUTATION: feed `abilities.walking_speed()` into the move speed. On the
    // client `getWalkingSpeed()` has exactly one consumer — the FOV modifier's
    // divisor; the walk speed is `Attributes.MOVEMENT_SPEED`, synced separately.
    // At the defaults the two agree (0.1 == 0.1), so only a non-default value
    // exposes the mistake.
    let walked = |walking_speed: f32| {
        let mut ab = Abilities::default();
        ab.set_walking_speed(walking_speed);
        let mut p = PlayerState::at(0.5, 0.0, 0.5);
        let input = TickInput {
            forward: 1.0,
            ..Default::default()
        };
        for _ in 0..80 {
            tick_with(&mut p, &input, &ab, false, &ground);
        }
        p.z
    };
    let (slow, fast) = (walked(0.02), walked(0.5));
    c.record(
        "w5.walking_speed_does_not_drive_the_walk",
        slow == fast,
        format!("walkingSpeed 0.02 -> z={slow:.5}, 0.5 -> z={fast:.5} (must be identical)"),
    );

    // w5.flying_speed_does_drive_the_flight.
    // The other half of the pair: `getFlyingSpeed()` IS read every tick, so the
    // packet's *first* float must reach the movement. Together these two rows
    // pin which float goes where.
    // MUTATION: ignore `flying_speed` and use a constant 0.05.
    let flown = |flying_speed: f32| {
        let mut ab = creative_flying();
        ab.set_flying_speed(flying_speed);
        let mut p = PlayerState::at(0.5, 80.0, 0.5);
        let input = TickInput {
            forward: 1.0,
            ..Default::default()
        };
        for _ in 0..600 {
            tick_with(&mut p, &input, &ab, false, &air);
        }
        p.vz
    };
    let (f_slow, f_fast) = (flown(0.02), flown(0.2));
    c.record(
        "w5.flying_speed_does_drive_the_flight",
        near(f_slow, closed(f64::from(0.02f32)), 1e-6)
            && near(f_fast, closed(f64::from(0.2f32)), 1e-6),
        format!("flyingSpeed 0.02 -> {f_slow:.5}, 0.2 -> {f_fast:.5} (both closed-form)"),
    );

    // w5.the_horizontal_min_movement_clamp_is_joint_for_a_player.
    // MUTATION: two independent per-axis `< 0.003` tests.
    //
    // Added *because the mutation battery found it missing*: the per-axis
    // reversion was the one mutation of thirty that left this gate green, and
    // the property was covered only by a `rewo-world` unit test. `aiStep`'s
    // clamp has two arms and `EntityTypes.PLAYER` takes
    // `horizontalDistanceSqr() < 9.0E-6` — a joint test on the pair. They
    // disagree exactly where each axis is under 0.003 but the magnitude is not:
    // 0.0025 on both axes has magnitude 0.00354, which vanilla keeps.
    let survives = |vx: f64, vz: f64| {
        let mut p = PlayerState::at(0.5, 80.0, 0.5);
        p.vx = vx;
        p.vz = vz;
        tick_with(&mut p, &TickInput::default(), &Abilities::default(), false, &air);
        (p.vx != 0.0, p.vz != 0.0)
    };
    let joint = survives(0.0025, 0.0025);
    let both_under = survives(0.002, 0.002);
    c.record(
        "w5.the_horizontal_min_movement_clamp_is_joint_for_a_player",
        joint == (true, true) && both_under == (false, false),
        format!(
            "vx=vz=0.0025 (|h|=0.00354, above) survives {joint:?}; \
             vx=vz=0.002 (|h|=0.00283, below) survives {both_under:?}"
        ),
    );

    // w5.flight_still_collides.
    // MUTATION: route flight through the no-clip arm. Flight is not no-clip —
    // they are two different flags and only spectator has both.
    let descend = |no_clip: bool| {
        let mut fc = FlightControl::default();
        let mut ab = creative_flying();
        let mut p = PlayerState::at(0.5, 4.0, 0.5);
        let down = TickInput {
            sneak: true,
            ..Default::default()
        };
        for _ in 0..80 {
            fc.before_travel(&mut ab, &mut p, &down, true, false);
            tick_with(&mut p, &down, &ab, no_clip, &ground);
        }
        p.y
    };
    let (solid, ghost) = (descend(false), descend(true));
    c.record(
        "w5.flight_still_collides",
        solid >= -1e-9 && solid < 0.5,
        format!("flying down lands at y={solid:.5} (want 0)"),
    );

    // w5.no_clip_passes_through_the_floor.
    // MUTATION: run `collide_move` regardless of `no_clip`. `Entity.move`'s
    // noPhysics arm sets the position and clears every collision flag.
    c.record(
        "w5.no_clip_passes_through_the_floor",
        ghost < -5.0,
        format!("no-clip descends to y={ghost:.5} (want well below 0)"),
    );
}

// ---------------------------------------------------- 6. the toggle machine

fn check_toggle(c: &mut Checker) {
    let press = TickInput {
        jump: true,
        ..Default::default()
    };
    let idle = TickInput::default();

    /// Arm on tick 0, release for `gap` ticks, press again. Returns whether
    /// flight toggled.
    fn toggles_after(gap: usize, mayfly: bool) -> bool {
        let press = TickInput {
            jump: true,
            ..Default::default()
        };
        let idle = TickInput::default();
        let mut fc = FlightControl::default();
        let mut ab = may_fly();
        ab.mayfly = mayfly;
        let mut st = PlayerState::at(0.5, 80.0, 0.5);
        fc.before_travel(&mut ab, &mut st, &press, false, false);
        for _ in 0..gap {
            fc.before_travel(&mut ab, &mut st, &idle, false, false);
        }
        fc.before_travel(&mut ab, &mut st, &press, false, false);
        ab.flying
    }

    // w6.double_tap_window_is_five_ticks_of_separation.
    // MUTATION: change the literal 7, or move the decrement to after the
    // toggle check. The counter is already 6 when the next tick starts, so
    // the last gap that still toggles is 5 — the usable window is not 7.
    let opens: Vec<usize> = (1..=10).filter(|g| toggles_after(*g, true)).collect();
    c.record(
        "w6.double_tap_window_is_five_ticks_of_separation",
        opens == vec![1, 2, 3, 4, 5],
        format!("gaps that toggle: {opens:?} (want 1..=5; ~250 ms)"),
    );

    // w6.mayfly_gates_the_whole_toggle.
    // MUTATION: drop the `if ab.mayfly` guard. A survival player would fly.
    let any_without = (1..=10).any(|g| toggles_after(g, false));
    c.record(
        "w6.mayfly_gates_the_whole_toggle",
        !any_without,
        format!("without mayfly, any gap toggles: {any_without}"),
    );

    // w6.holding_jump_never_toggles.
    // MUTATION: drop the `!was_jumping` rising-edge test.
    //
    // Asserted **every tick**, not just at the end. Dropping the rising-edge
    // test makes a held key toggle on a repeating cycle, so the final state is
    // a coin flip — an earlier version of this witness sampled only the last
    // tick and the mutation battery watched it pass while the mutation was
    // live. That is the "passed by coincidence" failure, caught by the battery
    // rather than by reading.
    let mut fc = FlightControl::default();
    let mut ab = may_fly();
    let mut st = PlayerState::at(0.5, 80.0, 0.5);
    let mut ever_flew = false;
    for _ in 0..60 {
        fc.before_travel(&mut ab, &mut st, &press, false, false);
        ever_flew |= ab.flying;
    }
    c.record(
        "w6.holding_jump_never_toggles",
        !ever_flew,
        format!("flight engaged at any point during a 60-tick hold: {ever_flew}"),
    );

    // w6.a_toggle_resets_the_window.
    // MUTATION: leave `jump_trigger_time` alone on toggle. The next press would
    // then ride the old window and toggle straight back.
    let mut fc = FlightControl::default();
    let mut ab = may_fly();
    let mut st = PlayerState::at(0.5, 80.0, 0.5);
    fc.before_travel(&mut ab, &mut st, &press, false, false);
    fc.before_travel(&mut ab, &mut st, &idle, false, false);
    fc.before_travel(&mut ab, &mut st, &press, false, false);
    let on = ab.flying;
    fc.before_travel(&mut ab, &mut st, &idle, false, false);
    fc.before_travel(&mut ab, &mut st, &press, false, false);
    c.record(
        "w6.a_toggle_resets_the_window",
        on && ab.flying,
        format!("toggled on={on}, still flying after the next press={}", ab.flying),
    );

    // w6.toggling_on_while_standing_jumps.
    // MUTATION: drop the `jumpFromGround()` call. Without it the toggle leaves
    // you on the ground, where the landing clause immediately ends flight —
    // flight would appear not to work at all from a standing start.
    let mut fc = FlightControl::default();
    let mut ab = may_fly();
    let mut st = PlayerState::at(0.5, 0.0, 0.5);
    st.on_ground = true;
    fc.before_travel(&mut ab, &mut st, &press, false, false);
    fc.before_travel(&mut ab, &mut st, &idle, false, false);
    let step = fc.before_travel(&mut ab, &mut st, &press, false, false);
    c.record(
        "w6.toggling_on_while_standing_jumps",
        ab.flying && step.jump_from_ground && step.abilities_changed,
        format!(
            "flying={} jump_from_ground={} owes_packet={}",
            ab.flying, step.jump_from_ground, step.abilities_changed
        ),
    );

    // w6.creative_flight_ends_on_landing.
    // MUTATION: drop the landing clause, or stop excluding spectator from it.
    let mut fc = FlightControl::default();
    let mut ab = creative_flying();
    let mut st = PlayerState::at(0.5, 0.0, 0.5);
    st.on_ground = false;
    let airborne = fc.after_travel(&mut ab, &st, false);
    st.on_ground = true;
    let landed = fc.after_travel(&mut ab, &st, false);
    c.record(
        "w6.creative_flight_ends_on_landing",
        !airborne && landed && !ab.flying,
        format!("airborne changed={airborne} landed changed={landed} flying={}", ab.flying),
    );

    // w6.a_spectator_is_forced_flying_and_never_lands.
    // MUTATION: remove the spectator arm, or include spectator in the landing
    // clause. Either leaves a spectator walking on the ground.
    let mut fc = FlightControl::default();
    let mut ab = may_fly();
    let mut st = PlayerState::at(0.5, 0.0, 0.5);
    let forced = fc.before_travel(&mut ab, &mut st, &idle, true, false);
    st.on_ground = true;
    let ends = fc.after_travel(&mut ab, &st, true);
    c.record(
        "w6.a_spectator_is_forced_flying_and_never_lands",
        ab.flying && forced.abilities_changed && !ends,
        format!("flying={} announced={} landing_ends_it={ends}", ab.flying, forced.abilities_changed),
    );

    // w6.a_client_side_change_owes_the_server_a_packet.
    // MUTATION: stop returning `abilities_changed`. The server's own
    // `Abilities.flying` would stay false, and it AND-gates our claim with
    // `mayfly` — so silently we would be flying only client-side.
    let mut fc = FlightControl::default();
    let mut ab = may_fly();
    let mut st = PlayerState::at(0.5, 80.0, 0.5);
    let a = fc.before_travel(&mut ab, &mut st, &press, false, false);
    let b = fc.before_travel(&mut ab, &mut st, &idle, false, false);
    let d = fc.before_travel(&mut ab, &mut st, &press, false, false);
    c.record(
        "w6.a_client_side_change_owes_the_server_a_packet",
        !a.abilities_changed && !b.abilities_changed && d.abilities_changed,
        format!(
            "arm={} idle={} toggle={} (only the toggle announces)",
            a.abilities_changed, b.abilities_changed, d.abilities_changed
        ),
    );

    // w6.a_mounted_player_cannot_toggle.
    // Recorded as a *scoped exclusion*, not a parity claim: vanilla's guard is
    // `getVehicle() == null || jumpableVehicle() != null`, so a rider on a
    // horse may toggle. Rewo models no rideable-jumping, so it takes the boat
    // arm for every vehicle — the safe subset.
    // MUTATION: drop the `!mounted` guard.
    let mut fc = FlightControl::default();
    let mut ab = may_fly();
    let mut st = PlayerState::at(0.5, 80.0, 0.5);
    fc.before_travel(&mut ab, &mut st, &press, false, true);
    fc.before_travel(&mut ab, &mut st, &idle, false, true);
    fc.before_travel(&mut ab, &mut st, &press, false, true);
    c.record(
        "w6.a_mounted_player_cannot_toggle",
        !ab.flying,
        format!("mounted toggle attempt left flying={}", ab.flying),
    );
}

// ------------------------------------------------- 7. the end-to-end chain

fn check_packet_to_flight(c: &mut Checker) {
    // w7.a_packet_grants_flight_that_actually_flies.
    // The whole chain in one: nine raw bytes -> parse -> apply_to -> the
    // controller -> `tick_with`. Proves the decoded `flying` bit and the
    // decoded flying speed both reach the movement, through the real functions.
    // MUTATION: any break in the chain; most sharply, `apply_to` not writing
    // `flying_speed`.
    let mut ab = Abilities::default();
    PlayerAbilities::parse(&body(FLAG_FLYING | FLAG_CAN_FLY, 0.1, 0.1))
        .expect("parse")
        .apply_to(&mut ab);
    let mut p = PlayerState::at(0.5, 80.0, 0.5);
    let input = TickInput {
        forward: 1.0,
        ..Default::default()
    };
    for _ in 0..600 {
        tick_with(&mut p, &input, &ab, false, &air);
    }
    let want = f64::from(0.1f32) * 0.98 * 0.91 / (1.0 - 0.91);
    c.record(
        "w7.a_packet_grants_flight_that_actually_flies",
        ab.flying && near(p.vz, want, 1e-6),
        format!("vz={:.5} (want {want:.5} for the packet's 0.1 flying speed)", p.vz),
    );

    // w7.a_packet_can_take_flight_away_mid_air.
    // MUTATION: `|=` in `apply_to` (see w1). The server revoking flight — which
    // is how `/gamemode survival` reaches a flying client — must land.
    let before = ab.flying;
    PlayerAbilities::parse(&body(0, 0.05, 0.1))
        .expect("parse")
        .apply_to(&mut ab);
    let mut q = PlayerState::at(0.5, 80.0, 0.5);
    q.vy = 0.0;
    for _ in 0..5 {
        tick_with(&mut q, &TickInput::default(), &ab, false, &air);
    }
    c.record(
        "w7.a_packet_can_take_flight_away_mid_air",
        before && !ab.flying && q.vy < -0.3,
        format!("was flying={before}, now flying={}, falling vy={:.4}", ab.flying, q.vy),
    );
}

pub fn run(args: AbilityshotArgs) -> Result<(), String> {
    let mode = if args.check { "check" } else { "report" };
    println!(
        "[abilityshot] mode: {mode} (serverless, CPU-only; the oracle asserts \
         unconditionally — a failure exits nonzero with or without --check)"
    );

    // The two real resolved `player_abilities` ids, through the production
    // `Ids::resolve` on the pinned version's datagen report. They share a name
    // and differ only by direction, so resolving both here proves the direction
    // seam rather than a fabricated pair.
    let paths = DataPaths::for_version(&args.version)
        .ok_or_else(|| "no config dir for version data".to_string())?;
    let packets = Packets::load(&paths.packets_json())?;
    let ids = Ids::resolve(&packets)?;
    println!(
        "[abilityshot] player_abilities: clientbound={} serverbound={}",
        ids.cb_play_player_abilities, ids.sb_play_player_abilities
    );

    let mut c = Checker::new();
    check_clientbound_wire(&mut c);
    check_serverbound_wire(
        &mut c,
        ids.sb_play_player_abilities,
        ids.cb_play_player_abilities,
    );
    check_mode_table(&mut c);
    check_gamemode_sources(&mut c);
    check_flight_physics(&mut c);
    check_toggle(&mut c);
    check_packet_to_flight(&mut c);

    println!(
        "[abilityshot] {}/{} witnesses",
        c.witnessed, EXPECTED_WITNESSES
    );
    if !c.failures.is_empty() {
        return Err(format!(
            "abilityshot: {} propert(y/ies) failed: {}",
            c.failures.len(),
            c.failures.join(", ")
        ));
    }
    if c.witnessed != EXPECTED_WITNESSES {
        return Err(format!(
            "abilityshot: observed {} properties, expected {EXPECTED_WITNESSES} — a witness \
             was skipped",
            c.witnessed
        ));
    }
    println!("[abilityshot] PASS {}/{}", c.witnessed, EXPECTED_WITNESSES);
    Ok(())
}
