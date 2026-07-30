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
//! - **`w8.rotation_is_float_bool_float_bool_not_a_mask`** — mutation: read the
//!   body as `Relative.SET_STREAM_CODEC`'s packed int, the shape its positional
//!   twin uses and the shape `REWO_PACKET_COVERAGE.md` §3 described.
//! - **`w9.look_at_evaluates_vanillas_atan2_not_the_platforms`** — mutation:
//!   `y.atan2(x)`. Two-sided, because the failure is *agreement*.
//! - **`w10.entering_a_level_discards_the_previous_spawn`** — mutation: carry
//!   the world spawn across a dimension change, as the difficulty beside it is
//!   carried. They behave oppositely and share one vanilla object.

use clap::Args as ClapArgs;
use rewo_data::{packets::Packets, DataPaths};
use rewo_net::abilities::{
    serverbound, PlayerAbilities, FLAG_CAN_FLY, FLAG_FLYING, FLAG_INSTABUILD, FLAG_INVULNERABLE,
};
use rewo_net::bundle::{BundleAssembler, BundleIds, Feed, BUNDLE_SIZE_LIMIT};
use rewo_net::game_event::ClientGameState;
use rewo_net::ids::Ids;
use rewo_net::play::{apply_spawn_game_mode, GameMode};
use rewo_net::route_session;
use rewo_net::session::{
    read_chat_type_bound, read_custom_payload, read_player_combat_end, read_player_combat_enter,
    read_server_data, read_store_cookie, write_cookie_response, CustomPayload, SessionState,
    BRAND_PAYLOAD_ID, MAX_COOKIE_PAYLOAD_SIZE,
};
use rewo_net::client_state::{read_set_default_spawn_position, ClientState, Difficulty};
use rewo_net::player_rotation::{
    route_player_rotation, Anchor, LocalRotation, PlayerLookAt, PlayerRotation, RotationRoute,
};
use rewo_proto::reader::PacketReader;
use rewo_world::abilities::{Abilities, FlightControl};
use rewo_world::physics::{tick_with, PlayerState, TickInput};

/// Total number of named properties this gate asserts. Locked so a skipped
/// property (fewer observations) fails the run even if nothing mismatched.
///
/// Total number of named properties this gate asserts. Locked so a skipped
/// property (fewer observations) fails the run even if nothing mismatched.
///
/// **47 through M75**, then **+20 for M78's** session / metadata / chat layer
/// (7 for the bundle machine, 13 for the seven decodes), then **+29 for M76's**
/// rotation and world spawn. Both extended this gate rather than minting a new
/// `*shot` command: it is already the serverless CPU-only oracle that resolves
/// real packet ids through `Ids::resolve`, which is what all eleven need on top
/// of pure decode.
const EXPECTED_WITNESSES: usize = 96;

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

/// A VarInt, built here rather than through `PacketWriter` so the byte-level
/// witnesses below are not graded against the writer they are grading.
fn wire_varint(mut v: i32) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let byte = (v & 0x7f) as u8;
        v = ((v as u32) >> 7) as i32;
        if v == 0 {
            out.push(byte);
            return out;
        }
        out.push(byte | 0x80);
    }
}

/// A VarInt-length-prefixed UTF-8 string — a wire `Identifier` or `String`.
fn wire_string(s: &str) -> Vec<u8> {
    let mut out = wire_varint(s.len() as i32);
    out.extend_from_slice(s.as_bytes());
    out
}

/// A network-NBT string tag: the shape `ComponentSerialization`'s
/// `fromCodecTrusted` produces for a plain-text `Component`.
fn wire_component(text: &str) -> Vec<u8> {
    let mut out = vec![0x08]; // TAG_String, unnamed root
    out.extend_from_slice(&(text.len() as u16).to_be_bytes());
    out.extend_from_slice(text.as_bytes());
    out
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
        tick_with(&mut p, &TickInput::default(), &ab, false, None, &air);
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
            tick_with(&mut q, &TickInput::default(), ab, false, None, &air);
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
        tick_with(&mut p, &up, &ab, false, None, &air);
    }
    let carried = p.vy;
    let before = p.y;
    fc.before_travel(&mut ab, &mut p, &up, false, false);
    tick_with(&mut p, &up, &ab, false, None, &air);
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
            tick_with(&mut p, &input, &ab, false, None, &air);
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
            tick_with(&mut p, &input, &ab, false, None, &air);
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
            tick_with(&mut p, &input, &Abilities::default(), false, None, &ground);
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
            tick_with(&mut p, &input, &ab, false, None, &ground);
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
            tick_with(&mut p, &input, &ab, false, None, &air);
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
        tick_with(&mut p, &TickInput::default(), &Abilities::default(), false, None, &air);
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
            tick_with(&mut p, &down, &ab, no_clip, None, &ground);
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
        tick_with(&mut p, &input, &ab, false, None, &air);
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
        tick_with(&mut q, &TickInput::default(), &ab, false, None, &air);
    }
    c.record(
        "w7.a_packet_can_take_flight_away_mid_air",
        before && !ab.flying && q.vy < -0.3,
        format!("was flying={before}, now flying={}, falling vy={:.4}", ab.flying, q.vy),
    );
}

// ------------------------------- 8. M78: the bundle reassembler (packet 0)

/// `bundle_delimiter` is the one M78 packet that changes how packets are
/// *applied*, so its properties are about the state machine rather than a body.
///
/// Driven through the production [`BundleAssembler`] with the **real** resolved
/// delimiter and terminal ids, so a renumber or a mis-resolved name shows up
/// here rather than as a silent no-op.
fn check_bundle(c: &mut Checker, delimiter: i32, terminal: Option<i32>) {
    let fresh = || BundleAssembler::new(BundleIds {
        delimiter,
        terminal,
    });

    // w8.only_the_resolved_delimiter_opens_a_bundle.
    // MUTATION: hard-code `0` as the delimiter instead of taking it from `Ids`.
    // It happens to *be* 0 in 26.2, which is why the witness feeds a
    // non-delimiter id too: a machine that opened on everything would pass a
    // delimiter-only check.
    let mut a = fresh();
    let opened = a.feed(delimiter, &[]) == Feed::Buffered && a.is_bundling();
    let mut b = fresh();
    let ignored = b.feed(delimiter + 1, &[1]) == Feed::Apply && !b.is_bundling();
    c.record(
        "w8.only_the_resolved_delimiter_opens_a_bundle",
        opened && ignored,
        format!("delimiter={delimiter} opens={opened}, id {} passes through={ignored}", delimiter + 1),
    );

    // w8.a_bundle_is_withheld_until_it_closes.
    // MUTATION: returning `Apply` for a packet inside a bundle — which is
    // exactly the pre-M78 behaviour, and whose failure mode is a mob rendered
    // for one frame with default metadata rather than a protocol error.
    let mut a = fresh();
    a.feed(delimiter, &[]);
    let held = a.feed(11, &[0xaa]) == Feed::Buffered && a.feed(22, &[0xbb]) == Feed::Buffered;
    let flushed = a.feed(delimiter, &[]) == Feed::Flush;
    let run = a.take();
    c.record(
        "w8.a_bundle_is_withheld_until_it_closes_then_released_in_order",
        held && flushed && run == vec![(11, vec![0xaa]), (22, vec![0xbb])],
        format!("held={held} flushed={flushed} run={run:?} (neither delimiter is in it)"),
    );

    // w8.an_unterminated_bundle_survives_a_drained_queue.
    // MUTATION: clearing the buffer when the caller's `try_recv` runs dry, or
    // applying the partial run. This is the case bundling exists for in Rewo —
    // a socket that hands over a bundle in two reads — and the sample re-enters
    // the machine after a simulated gap and asserts the earlier packet is
    // *still* buffered.
    let mut a = fresh();
    a.feed(delimiter, &[]);
    a.feed(11, &[0x01]);
    let survived = a.is_bundling() && a.buffered() == 1;
    a.feed(22, &[0x02]);
    let closed = a.feed(delimiter, &[]) == Feed::Flush;
    let run = a.take();
    c.record(
        "w8.an_unterminated_bundle_survives_a_drained_queue",
        survived && closed && run.len() == 2,
        format!("still buffering across the gap={survived}, resumed run={} packets", run.len()),
    );

    // w8.a_second_delimiter_closes_rather_than_nesting.
    // MUTATION: a depth counter. That reading never closes the outer bundle, so
    // everything after the first "nested" delimiter is withheld for the rest of
    // the session. Three delimiters, because with two the two readings agree on
    // everything but the final state.
    let mut a = fresh();
    a.feed(delimiter, &[]);
    a.feed(11, &[]);
    a.feed(delimiter, &[]);
    a.take();
    let reopened = a.feed(delimiter, &[]) == Feed::Buffered && a.buffered() == 0;
    a.feed(22, &[]);
    let second_run = a.feed(delimiter, &[]) == Feed::Flush && a.take() == vec![(22, vec![])];
    c.record(
        "w8.a_second_delimiter_closes_rather_than_nesting",
        reopened && second_run,
        format!("third delimiter opens a fresh empty run={reopened}, and it closes={second_run}"),
    );

    // w8.the_size_limit_admits_exactly_4096_and_no_more.
    // MUTATION: `>` instead of `>=`, or checking after the push — both admit
    // 4097. The sample sits exactly on the bound in both directions, because a
    // ten-packet bundle leaves every off-by-one reading green.
    let mut a = fresh();
    a.feed(delimiter, &[]);
    let mut all_buffered = true;
    for _ in 0..BUNDLE_SIZE_LIMIT {
        all_buffered &= a.feed(11, &[]) == Feed::Buffered;
    }
    let at_limit = all_buffered && a.buffered() == BUNDLE_SIZE_LIMIT;
    let over = matches!(a.feed(11, &[]), Feed::Fatal(_));
    c.record(
        "w8.the_size_limit_admits_exactly_4096_and_no_more",
        at_limit && over && BUNDLE_SIZE_LIMIT == 4096,
        format!("{BUNDLE_SIZE_LIMIT} buffered ok={at_limit}, the next is fatal={over}"),
    );

    // w8.a_full_bundle_still_closes.
    // MUTATION: moving the size check above the delimiter test. A full bundle
    // would then be fatal at the moment it correctly closed — the worst
    // possible reading, because it fires only on servers that legitimately send
    // large bundles. The witness above cannot see it.
    let mut a = fresh();
    a.feed(delimiter, &[]);
    for _ in 0..BUNDLE_SIZE_LIMIT {
        a.feed(11, &[]);
    }
    let closed = a.feed(delimiter, &[]) == Feed::Flush;
    let n = a.take().len();
    c.record(
        "w8.a_full_bundle_still_closes",
        closed && n == BUNDLE_SIZE_LIMIT,
        format!("closed={closed} with {n} sub-packets"),
    );

    // w8.a_terminal_packet_is_fatal_only_inside_a_bundle.
    // MUTATION: dropping `verifyNonTerminalPacket`, or applying it
    // unconditionally. The outside-a-bundle half is what makes this sharp: an
    // unconditional rejection would break `start_configuration` on every server
    // that reloads a datapack, and an inside-only check would not see it.
    let Some(term) = terminal else {
        c.record(
            "w8.a_terminal_packet_is_fatal_only_inside_a_bundle",
            false,
            "start_configuration did not resolve, so the rule cannot be graded",
        );
        return;
    };
    let mut a = fresh();
    let outside = a.feed(term, &[]) == Feed::Apply;
    a.feed(delimiter, &[]);
    a.feed(11, &[]);
    let inside = matches!(a.feed(term, &[]), Feed::Fatal(_));
    c.record(
        "w8.a_terminal_packet_is_fatal_only_inside_a_bundle",
        outside && inside && !a.is_bundling(),
        format!("start_configuration id={term}: outside is ordinary={outside}, inside is fatal={inside}"),
    );
}

// ---------------------- 9. M78: session, server metadata and chat (7 packets)

/// Everything but the delimiter, driven through the **production
/// `rewo_net::route_session` seam** with a real [`Ids`] rather than a
/// reimplemented id table — the M45/M47 rule that a gate reimplementing a slice
/// of the app's setup misses whatever the app adds to it.
fn check_session(c: &mut Checker, ids: &Ids) {
    // w9.every_session_id_routes_to_its_own_effect.
    // MUTATION: swapping any two arms of `session::kind_for_id`. Several of
    // these bodies are mutually decodable — `player_combat_enter` accepts any
    // body at all and `custom_payload`/`store_cookie` both open with an
    // identifier — so a swap is silent rather than an error. Each id is fed a
    // body that only *its* packet turns into an observable, and the whole thing
    // runs through the real router, so a broken `SessionIds` table fails here.
    let mut s = SessionState::default();
    let mut brand = wire_string(BRAND_PAYLOAD_ID);
    brand.extend(wire_string("Paper"));
    let routed_brand = route_session(ids.cb_play_custom_payload, &brand, ids, &mut s);

    let mut cookie = wire_string("mynet:session");
    cookie.push(0x02);
    cookie.extend_from_slice(&[0xde, 0xad]);
    let routed_cookie = route_session(ids.cb_play_store_cookie, &cookie, ids, &mut s);

    let mut rules = vec![0x01];
    rules.extend(wire_string("minecraft:keep_inventory"));
    rules.extend(wire_string("true"));
    let routed_rules = route_session(ids.cb_play_game_rule_values, &rules, ids, &mut s);

    let mut data = wire_component("A Minecraft Server");
    data.push(0x00);
    let routed_data = route_session(ids.cb_play_server_data, &data, ids, &mut s);

    let routed_end = route_session(ids.cb_play_player_combat_end, &[0x05], ids, &mut s);
    let routed_enter = route_session(ids.cb_play_player_combat_enter, &[], ids, &mut s);

    let mut chat = wire_component("psst");
    chat.push(0x02);
    chat.extend(wire_component("Notch"));
    chat.push(0x00);
    let routed_chat = route_session(ids.cb_play_disguised_chat, &chat, ids, &mut s);

    let all_routed = routed_brand
        && routed_cookie
        && routed_rules
        && routed_data
        && routed_end
        && routed_enter
        && routed_chat;
    let landed = s.server_brand.as_deref() == Some("Paper")
        && s.cookie("mynet:session") == Some(&[0xdeu8, 0xad][..])
        && s.game_rules.get("minecraft:keep_inventory").map(String::as_str) == Some("true")
        && s.server_data.as_ref().map(|d| d.motd.as_str()) == Some("A Minecraft Server")
        && s.take_chat() == vec!["psst".to_string()];
    c.record(
        "w9.every_session_id_routes_to_its_own_effect",
        all_routed && landed,
        format!(
            "routed={all_routed} brand={:?} cookies={} rules={} motd={:?}",
            s.server_brand,
            s.cookie_count(),
            s.game_rules.len(),
            s.server_data.as_ref().map(|d| d.motd.clone())
        ),
    );

    // w9.an_unrelated_id_is_not_claimed.
    // MUTATION: `route_session` returning `true` unconditionally. It sits in
    // an `else if` ladder and, in `Connection::run_play`, in the fallthrough
    // arm — a router that claimed every id would swallow every packet after it.
    let mut s2 = SessionState::default();
    let claimed = route_session(ids.cb_play_keep_alive, &[0; 8], ids, &mut s2);
    c.record(
        "w9.an_unrelated_id_is_not_claimed",
        !claimed && s2 == SessionState::default(),
        format!("keep_alive (id {}) claimed={claimed}", ids.cb_play_keep_alive),
    );

    // w9.the_brand_identifier_is_namespaced.
    // MUTATION: comparing against the bare `"brand"`. `createType` runs the
    // name through `Identifier.withDefaultNamespace`, so a bare comparison
    // sends every real brand down the discard path and leaves `server_brand`
    // permanently `None` — with no error anywhere.
    let mut bare = wire_string("brand");
    bare.extend(wire_string("Paper"));
    let bare_is_discarded = matches!(
        read_custom_payload(&bare),
        Ok(CustomPayload::Discarded { .. })
    );
    let namespaced =
        read_custom_payload(&brand).is_ok_and(|p| p == CustomPayload::Brand("Paper".into()));
    c.record(
        "w9.the_brand_identifier_is_namespaced",
        namespaced && bare_is_discarded && BRAND_PAYLOAD_ID == "minecraft:brand",
        format!("{BRAND_PAYLOAD_ID:?} is the brand={namespaced}, bare \"brand\" is not={bare_is_discarded}"),
    );

    // w9.an_unknown_payload_is_discarded_and_fully_consumed.
    // MUTATION: erroring on an unrecognised identifier. That is M41's rule for
    // a `DataComponentPatch` member and it is wrong here, because this union
    // *has* a fallback codec — rejecting would kill the connection to any
    // modded server. The tail is five bytes that decode as nothing else, so a
    // reader that stopped at the identifier leaves them unread.
    let mut unknown = wire_string("mymod:hello");
    unknown.extend_from_slice(&[0xff, 0x00, 0x13, 0x37, 0x42]);
    let discarded = read_custom_payload(&unknown);
    c.record(
        "w9.an_unknown_payload_is_discarded_and_fully_consumed",
        discarded.as_ref().is_ok_and(|p| {
            *p == CustomPayload::Discarded {
                id: "mymod:hello".into(),
                len: 5,
            }
        }),
        format!("{discarded:?}"),
    );

    // w9.the_brand_is_also_resolved_in_configuration.
    // MUTATION: resolving only the play id. The vanilla server sends
    // `minecraft:brand` from `ServerConfigurationPacketListenerImpl`'s opening
    // burst and never repeats it in play, so a play-only client reads no brand
    // from any server that exists — M69's `update_tags` finding one packet
    // over. The two ids are *different numbers*, which is the whole point.
    let cfg = ids.cb_config_custom_payload;
    let play = ids.cb_play_custom_payload;
    c.record(
        "w9.the_brand_is_also_resolved_in_configuration",
        cfg != play && cfg >= 0 && play >= 0,
        format!("configuration custom_payload={cfg}, play custom_payload={play} (both resolved, and distinct)"),
    );

    // w9.a_stored_cookie_changes_what_the_request_is_answered_with.
    // MUTATION: the pre-M78 `resp.string(&key).bool(false)` — an
    // unconditional miss. This is the *headline* property of `store_cookie`:
    // the jar is only observable through the reply, so a jar that filled
    // correctly behind a reply that still wrote `false` would be
    // indistinguishable from the old client. Graded on the produced bytes,
    // through the same writer `Connection::answer_cookie_request` calls.
    let mut jar = SessionState::default();
    let empty = write_cookie_response(7, "mynet:session", jar.cookie("mynet:session"));
    jar.store_cookie("mynet:session".into(), vec![0xde, 0xad, 0xbe]);
    let filled = write_cookie_response(7, "mynet:session", jar.cookie("mynet:session"));
    // packet id 7, the key, then the nullable payload.
    let mut want_empty = vec![0x07];
    want_empty.extend(wire_string("mynet:session"));
    want_empty.push(0x00);
    let mut want_filled = vec![0x07];
    want_filled.extend(wire_string("mynet:session"));
    want_filled.extend_from_slice(&[0x01, 0x03, 0xde, 0xad, 0xbe]);
    c.record(
        "w9.a_stored_cookie_changes_what_the_request_is_answered_with",
        empty.buf == want_empty && filled.buf == want_filled && empty.buf != filled.buf,
        format!(
            "empty jar -> {:02x?}, after store_cookie -> {:02x?}",
            empty.buf, filled.buf
        ),
    );

    // w9.a_cookie_for_another_key_is_still_a_miss.
    // MUTATION: a store that ignores its key (a single `Option<Vec<u8>>`
    // instead of a map). Handing back one backend's session token under
    // another's key is worse than handing back nothing.
    let other = write_cookie_response(7, "mynet:other", jar.cookie("mynet:other"));
    c.record(
        "w9.a_cookie_for_another_key_is_still_a_miss",
        other.buf == {
            let mut w = vec![0x07];
            w.extend(wire_string("mynet:other"));
            w.push(0x00);
            w
        },
        format!("{:02x?}", other.buf),
    );

    // w9.the_cookie_payload_limit_is_an_error_on_the_bound.
    // MUTATION: `>=` instead of `>` (rejecting a legal 5120-byte cookie), or
    // clamping instead of erroring (storing a truncated cookie to hand back to
    // a server that never issued it). The samples sit exactly on 5120 and 5121.
    let cookie_body = |len: usize| {
        let mut b = wire_string("mynet:session");
        b.extend(wire_varint(len as i32));
        b.extend(std::iter::repeat_n(0xabu8, len));
        b
    };
    let at = read_store_cookie(&cookie_body(MAX_COOKIE_PAYLOAD_SIZE));
    let over = read_store_cookie(&cookie_body(MAX_COOKIE_PAYLOAD_SIZE + 1));
    c.record(
        "w9.the_cookie_payload_limit_is_an_error_on_the_bound",
        at.as_ref().is_ok_and(|(_, p)| p.len() == MAX_COOKIE_PAYLOAD_SIZE)
            && over.is_err()
            && MAX_COOKIE_PAYLOAD_SIZE == 5120,
        format!(
            "{MAX_COOKIE_PAYLOAD_SIZE} bytes ok={}, {} bytes errors={}",
            at.is_ok(),
            MAX_COOKIE_PAYLOAD_SIZE + 1,
            over.is_err()
        ),
    );

    // w9.the_chat_type_holder_is_id_plus_one_with_zero_meaning_inline.
    // MUTATION: reading the VarInt as a raw registry id, the convention the
    // *dimension* holder uses one packet over. The inline case is what bites: a
    // raw reading takes 0 as chat type 0 and then reads the first decoration's
    // translation key as the sender's name, so the sender comes out as
    // "chat.type.text" and every field after it is garbage.
    let mut referenced = vec![0x04];
    referenced.extend(wire_component("Notch"));
    referenced.push(0x00);
    let mut r = PacketReader::new(&referenced);
    let ref_bound = read_chat_type_bound(&mut r);
    let ref_ok = ref_bound.as_ref().is_ok_and(|b| {
        b.chat_type == Some(3) && b.name == "Notch" && b.target_name.is_none()
    }) && r.remaining() == 0;

    let decoration = |key: &str| {
        let mut d = wire_string(key);
        d.push(0x02); // two parameters
        d.extend_from_slice(&[0x00, 0x02]);
        d.extend_from_slice(&[0x0a, 0x00]); // an empty NBT compound Style
        d
    };
    let mut inline = vec![0x00];
    inline.extend(decoration("chat.type.text"));
    inline.extend(decoration("chat.type.text.narrate"));
    inline.extend(wire_component("Notch"));
    inline.push(0x01);
    inline.extend(wire_component("Herobrine"));
    let mut r = PacketReader::new(&inline);
    let inline_bound = read_chat_type_bound(&mut r);
    let inline_ok = inline_bound.as_ref().is_ok_and(|b| {
        b.chat_type.is_none()
            && b.name == "Notch"
            && b.target_name.as_deref() == Some("Herobrine")
    }) && r.remaining() == 0;
    c.record(
        "w9.the_chat_type_holder_is_id_plus_one_with_zero_meaning_inline",
        ref_ok && inline_ok,
        format!("wire 4 -> {ref_bound:?}; wire 0 (inline) -> {inline_bound:?}"),
    );

    // w9.the_vestigial_packets_consume_exactly_their_bodies.
    // MUTATION: reading one byte too many or too few. Both handlers are empty
    // methods in vanilla, so nothing is stored and consumption is the *only*
    // property either packet has. The duration is written as a two-byte VarInt
    // precisely so a one-byte reader and a fixed-i32 reader both disagree with
    // it; `player_combat_enter` reads nothing, so any read is one too many.
    let end = read_player_combat_end(&[0xac, 0x02]);
    let enter = read_player_combat_enter(&[]);
    c.record(
        "w9.the_vestigial_packets_consume_exactly_their_bodies",
        end.as_ref().is_ok_and(|&(d, n)| d == 300 && n == 2)
            && enter == 0
            && read_player_combat_end(&[]).is_err(),
        format!("player_combat_end -> {end:?} (2 bytes), player_combat_enter -> {enter} bytes"),
    );

    // w9.game_rules_replace_rather_than_merge.
    // MUTATION: `extend` instead of assignment. A merge passes every "did the
    // new rule arrive" check and leaves a rule the server stopped sending
    // permanently visible. The second packet drops the first's only key, which
    // is where the two readings differ.
    let mut s3 = SessionState::default();
    route_session(ids.cb_play_game_rule_values, &rules, ids, &mut s3);
    let mut second = vec![0x01];
    second.extend(wire_string("minecraft:do_fire_tick"));
    second.extend(wire_string("false"));
    route_session(ids.cb_play_game_rule_values, &second, ids, &mut s3);
    c.record(
        "w9.game_rules_replace_rather_than_merge",
        s3.game_rules.len() == 1 && !s3.game_rules.contains_key("minecraft:keep_inventory"),
        format!("after two packets: {:?}", s3.game_rules),
    );

    // w9.server_data_reads_an_optional_icon_not_an_unconditional_one.
    // MUTATION: dropping the present-flag and reading the byte array always.
    // The absent case here is followed by nothing, so an unconditional read
    // errors; the present case's icon is a PNG magic prefix so a flag consumed
    // as part of the length gives a visibly different array.
    let mut with_icon = wire_component("hi");
    with_icon.extend_from_slice(&[0x01, 0x03, 0x89, 0x50, 0x4e]);
    let a = read_server_data(&data);
    let b = read_server_data(&with_icon);
    c.record(
        "w9.server_data_reads_an_optional_icon_not_an_unconditional_one",
        a.as_ref().is_ok_and(|d| d.icon.is_none())
            && b.as_ref().is_ok_and(|d| d.icon.as_deref() == Some(&[0x89, 0x50, 0x4e][..]))
            && read_server_data(&wire_component("hi")).is_err(),
        format!("absent -> {:?}, present -> {:?}", a.map(|d| d.icon), b.map(|d| d.icon)),
    );

    // w9.a_malformed_session_body_changes_nothing.
    // MUTATION: `apply` assigning as it reads — e.g. clearing `game_rules`
    // before the decode instead of after it. Every reader builds its value
    // before anything is written, which is what makes "changed nothing"
    // assertable at all.
    //
    // The state is **pre-populated on purpose**: the first version of this
    // witness started from `SessionState::default()`, and an `apply` that wiped
    // the game rules before decoding left an already-empty map empty, so the
    // mutation passed the gate (the unit test, which seeds a rule, caught it).
    // A sample has to sit where the mutation bites.
    let mut s4 = SessionState::default();
    s4.store_cookie("mynet:session".into(), vec![1]);
    route_session(ids.cb_play_game_rule_values, &rules, ids, &mut s4);
    route_session(ids.cb_play_custom_payload, &brand, ids, &mut s4);
    route_session(ids.cb_play_server_data, &data, ids, &mut s4);
    let before = s4.clone();
    for (id, body) in [
        (ids.cb_play_custom_payload, &[0x7f][..]),
        (ids.cb_play_disguised_chat, &[][..]),
        (ids.cb_play_game_rule_values, &[0x7f, 0x00][..]),
        (ids.cb_play_server_data, &[][..]),
        (ids.cb_play_store_cookie, &[][..]),
    ] {
        route_session(id, body, ids, &mut s4);
    }
    c.record(
        "w9.a_malformed_session_body_changes_nothing",
        s4 == before && !before.game_rules.is_empty() && before.server_brand.is_some(),
        format!(
            "five malformed bodies later: brand={:?} cookies={} rules={} (all pre-populated)",
            s4.server_brand,
            s4.cookie_count(),
            s4.game_rules.len()
        ),
    );
}

// ═══════════════════════════════════════════ M76: the rotation the server writes
//
// `player_rotation` (73), `player_look_at` (71) and `set_default_spawn_position`
// (97) join this gate rather than getting their own because they are the same
// subject it already owns: the local player's state as the server writes it.
// M75's `player_abilities` decides how the player *moves*; these decide where
// it *looks* and where it respawns.
//
// Everything below drives the production seams — `route_player_rotation` and
// `route_client_state` with a real resolved `Ids` — rather than the readers
// underneath them. M45 records why: a gate that reimplements a slice of the
// app's dispatch misses whatever the app adds to it, and both seams have a
// behaviour (`RotationRoute`'s send asymmetry, `client_state::apply`'s
// decode-failure answer) that only exists at that layer.

/// A `ClientboundPlayerRotationPacket` body — `FLOAT, BOOL, FLOAT, BOOL`,
/// written here from the decompiled `StreamCodec.composite`, **not** from
/// `PlayerRotation`'s own reader.
fn rot_body(y_rot: f32, rel_y: bool, x_rot: f32, rel_x: bool) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(&y_rot.to_be_bytes());
    b.push(rel_y as u8);
    b.extend_from_slice(&x_rot.to_be_bytes());
    b.push(rel_x as u8);
    b
}

/// A `ClientboundPlayerLookAtPacket` body. `at` is `Some((entity, to_anchor))`
/// for the `facing entity` form and `None` for the point form.
fn look_body(from_anchor: u8, pos: [f64; 3], at: Option<(i32, u8)>) -> Vec<u8> {
    let mut b = vec![from_anchor];
    for v in pos {
        b.extend_from_slice(&v.to_be_bytes());
    }
    b.push(at.is_some() as u8);
    if let Some((entity, to)) = at {
        rewo_proto::varint::write_varint(&mut b, entity);
        b.push(to);
    }
    b
}

/// A `ClientboundSetDefaultSpawnPositionPacket` body — `LevelData.RespawnData`
/// = an `Identifier` string, a packed `BlockPos` long, then two floats.
///
/// The pack is written out here (`x << 38 | z << 12 | y`) rather than taken
/// from a helper so the gate's expectation of the layout is independent of the
/// reader's.
fn world_spawn_body(dimension: &str, pos: (i32, i32, i32), yaw: f32, pitch: f32) -> Vec<u8> {
    let mut b = Vec::new();
    rewo_proto::varint::write_varint(&mut b, dimension.len() as i32);
    b.extend_from_slice(dimension.as_bytes());
    let packed = (((pos.0 as i64) & 0x3FF_FFFF) << 38)
        | (((pos.2 as i64) & 0x3FF_FFFF) << 12)
        | ((pos.1 as i64) & 0xFFF);
    b.extend_from_slice(&packed.to_be_bytes());
    b.extend_from_slice(&yaw.to_be_bytes());
    b.extend_from_slice(&pitch.to_be_bytes());
    b
}

/// Drive one rotation packet through the production seam and report the
/// resulting `(yaw, pitch)` plus which packet the seam said it was.
fn route_rot(
    ids: &Ids,
    id: i32,
    body: &[u8],
    start: (f32, f32),
) -> ((f32, f32), RotationRoute) {
    route_rot_from(ids, id, body, start, [0.0, 0.0, 0.0], |_, _| None)
}

/// As [`route_rot`], with the player's feet and a look-at entity resolver.
fn route_rot_from(
    ids: &Ids,
    id: i32,
    body: &[u8],
    start: (f32, f32),
    player_pos: [f64; 3],
    resolve: impl FnOnce(i32, Anchor) -> Option<[f64; 3]>,
) -> ((f32, f32), RotationRoute) {
    let (mut yaw, mut pitch) = start;
    let route = route_player_rotation(
        id,
        body,
        ids,
        LocalRotation {
            pos: player_pos,
            eye_height: rewo_world::physics::EYE_HEIGHT,
            yaw: &mut yaw,
            pitch: &mut pitch,
        },
        resolve,
    );
    ((yaw, pitch), route)
}

// ------------------------------------------------ 8. the two rotation wires

fn check_rotation_wire(c: &mut Checker, ids: &Ids) {
    // w8.rotation_is_float_bool_float_bool_not_a_mask.
    // MUTATION: read the body as `f32, f32, bool, bool` (the "it is like its
    // positional twin" shape), or as `f32, i32-mask`. The two flags below
    // differ, so any reordering reports at least one of them wrong.
    let p = PlayerRotation::parse(&rot_body(90.0, true, -30.0, false)).expect("parse");
    let layout = p.y_rot == 90.0 && p.relative_y && p.x_rot == -30.0 && !p.relative_x;
    let q = PlayerRotation::parse(&rot_body(1.5, false, 2.5, true)).expect("parse");
    let layout = layout && q.y_rot == 1.5 && !q.relative_y && q.x_rot == 2.5 && q.relative_x;
    c.record(
        "w8.rotation_is_float_bool_float_bool_not_a_mask",
        layout,
        format!(
            "({}, {}, {}, {}) then ({}, {}, {}, {})",
            p.y_rot, p.relative_y, p.x_rot, p.relative_x, q.y_rot, q.relative_y, q.x_rot,
            q.relative_x
        ),
    );

    // w8.rotation_body_is_exactly_ten_bytes.
    // MUTATION: a `Relative.SET_STREAM_CODEC` reading needs 4 (yaw) + 4 (mask)
    // and would accept eight; a nine-byte body must be rejected, which pins the
    // arity rather than just the field order. (M67's mutation survivor was
    // exactly an arity witness that only measured the happy case.)
    let ten = rot_body(0.0, false, 0.0, false).len() == 10;
    let mut short = rot_body(0.0, false, 0.0, false);
    short.pop();
    let rejects_short = PlayerRotation::parse(&short).is_err();
    c.record(
        "w8.rotation_body_is_exactly_ten_bytes",
        ten && rejects_short,
        format!("len==10: {ten}, a 9-byte body is rejected: {rejects_short}"),
    );

    // w8.look_at_point_form_has_no_trailing_pair.
    // MUTATION: read `entity` and `to_anchor` unconditionally. The point form
    // is the common one (`/teleport … facing <x y z>`) and ends after the flag.
    let body = look_body(1, [1.0, 2.0, 3.0], None);
    let p = PlayerLookAt::parse(&body).expect("parse");
    let point_form = body.len() == 26
        && p.from_anchor == Anchor::Eyes
        && p.pos == [1.0, 2.0, 3.0]
        && !p.at_entity
        && p.entity == 0
        && p.to_anchor.is_none();
    c.record(
        "w8.look_at_point_form_has_no_trailing_pair",
        point_form,
        format!(
            "len={} from={:?} at_entity={} entity={} to={:?}",
            body.len(),
            p.from_anchor,
            p.at_entity,
            p.entity,
            p.to_anchor
        ),
    );

    // w8.look_at_entity_form_carries_entity_then_anchor.
    // MUTATION: swap the trailing VarInt and enum, or read the anchor first.
    // Entity 77 and anchor 1 cannot be confused for one another.
    let p = PlayerLookAt::parse(&look_body(0, [4.0, 5.0, 6.0], Some((77, 1)))).expect("parse");
    c.record(
        "w8.look_at_entity_form_carries_entity_then_anchor",
        p.from_anchor == Anchor::Feet
            && p.at_entity
            && p.entity == 77
            && p.to_anchor == Some(Anchor::Eyes),
        format!(
            "from={:?} entity={} to={:?}",
            p.from_anchor, p.entity, p.to_anchor
        ),
    );

    // w8.an_out_of_range_anchor_is_an_error.
    // MUTATION: give `read_anchor` a `ByIdMap.continuous(…, ZERO)` default
    // (→ FEET) or a WRAP one (→ ordinal 2 becomes FEET as well). `readEnum` is
    // `values()[readVarInt()]` and throws, so both plausible alternatives are
    // wrong and both are silent. Checked on *both* anchor fields, since they go
    // through the same reader and a fix applied to one would not show here
    // otherwise.
    let mut leading = look_body(0, [0.0; 3], None);
    leading[0] = 2;
    let mut trailing = look_body(0, [0.0; 3], Some((1, 0)));
    let last = trailing.len() - 1;
    trailing[last] = 7;
    c.record(
        "w8.an_out_of_range_anchor_is_an_error",
        PlayerLookAt::parse(&leading).is_err() && PlayerLookAt::parse(&trailing).is_err(),
        "ordinal 2 (leading) and 7 (trailing) both rejected".to_string(),
    );

    // w8.at_entity_and_to_anchor_agree_after_parse.
    //
    // This is the invariant, and it is here because of a mutation that
    // *survived*: dropping `getPosition`'s `if (this.atEntity)` test changes
    // nothing, since `to_anchor` is `None` for the point form and the
    // `and_then` short-circuits to the same fallback. The two guards are
    // interchangeable **exactly while this holds**, so the honest thing to pin
    // is the invariant rather than either guard. `PlayerLookAt`'s fields are
    // public, so a hand-built value can violate it; nothing on the wire can.
    //
    // MUTATION: `PlayerLookAt::parse` writing `to_anchor: Some(..)` on the
    // point form (which is what reading the trailing pair unconditionally
    // does — see the `w8.look_at_point_form…` witness above, which catches
    // that one from the other side).
    let point = PlayerLookAt::parse(&look_body(0, [0.0; 3], None)).expect("parse");
    let ent = PlayerLookAt::parse(&look_body(0, [0.0; 3], Some((3, 1)))).expect("parse");
    c.record(
        "w8.at_entity_and_to_anchor_agree_after_parse",
        point.at_entity == point.to_anchor.is_some()
            && ent.at_entity == ent.to_anchor.is_some(),
        format!(
            "point: {}=={}, entity: {}=={}",
            point.at_entity,
            point.to_anchor.is_some(),
            ent.at_entity,
            ent.to_anchor.is_some()
        ),
    );

    // w8.the_two_ids_resolve_distinctly.
    // MUTATION: resolve either name to the other's, or to `player_position`.
    // The seam is keyed on them, so a collision would route one packet's body
    // into the other's reader.
    let distinct = ids.cb_play_player_rotation != ids.cb_play_player_look_at
        && ids.cb_play_player_rotation != ids.cb_play_position
        && ids.cb_play_player_look_at != ids.cb_play_position;
    c.record(
        "w8.the_two_ids_resolve_distinctly",
        distinct,
        format!(
            "rotation={} look_at={} position={}",
            ids.cb_play_player_rotation, ids.cb_play_player_look_at, ids.cb_play_position
        ),
    );

    // w8.an_unrelated_id_is_no_match.
    // MUTATION: make the seam's final `else` return `Rotation`. Without this
    // the seam could claim every packet and the dispatch ladder would stop.
    let (_, route) = route_rot(ids, ids.cb_play_position, &[], (0.0, 0.0));
    c.record(
        "w8.an_unrelated_id_is_no_match",
        route == RotationRoute::NoMatch,
        format!("player_position routed as {route:?}"),
    );
}

// ------------------------------------------- 9. what the rotation packets do

fn check_rotation_semantics(c: &mut Checker, ids: &Ids) {
    let rot_id = ids.cb_play_player_rotation;
    let look_id = ids.cb_play_player_look_at;

    // w9.each_flag_composes_its_own_axis.
    // MUTATION: compose against a stored "last sent" rotation, or let one flag
    // drive both axes. The two calls below use the same numbers with the flags
    // swapped, so a single-flag implementation gives the same answer twice.
    let ((y1, p1), _) = route_rot(ids, rot_id, &rot_body(30.0, true, -5.0, false), (100.0, 20.0));
    let ((y2, p2), _) = route_rot(ids, rot_id, &rot_body(30.0, false, -5.0, true), (100.0, 20.0));
    c.record(
        "w9.each_flag_composes_its_own_axis",
        (y1, p1) == (130.0, -5.0) && (y2, p2) == (30.0, 15.0),
        format!("relY: ({y1}, {p1}) want (130, -5); relX: ({y2}, {p2}) want (30, 15)"),
    );

    // w9.the_pitch_clamps_to_ninety.
    // MUTATION: drop `Mth.clamp(…, -90, 90)` from `calculateAbsolute`. 80 + 30
    // overshoots, so the clamp is the only thing between the player and 110.
    let ((_, pitch), _) = route_rot(ids, rot_id, &rot_body(0.0, false, 30.0, true), (0.0, 80.0));
    let ((_, low), _) = route_rot(ids, rot_id, &rot_body(0.0, false, -30.0, true), (0.0, -80.0));
    c.record(
        "w9.the_pitch_clamps_to_ninety",
        pitch == 90.0 && low == -90.0,
        format!("80+30 -> {pitch} (want 90), -80-30 -> {low} (want -90)"),
    );

    // w9.the_pitch_clamp_is_on_the_sum_not_the_step.
    // MUTATION: `offset + clamp(x_rot)` instead of `clamp(offset + x_rot)`.
    //
    // This witness exists because the battery caught the previous one being
    // blind to it. Neither sample above can see the reordering: any step under
    // 90° leaves `clamp(x_rot)` an identity, and for a step *over* 90° with a
    // positive base both orders saturate to 90 anyway. Separating them needs a
    // base of the opposite sign to the step — base -80, step +400 gives
    // `clamp(320) = 90` the right way round and `-80 + 90 = 10` the wrong one.
    let ((_, sum), _) = route_rot(ids, rot_id, &rot_body(0.0, false, 400.0, true), (0.0, -80.0));
    c.record(
        "w9.the_pitch_clamp_is_on_the_sum_not_the_step",
        sum == 90.0,
        format!("-80 + 400 -> {sum} (want 90; clamping the step first gives 10)"),
    );

    // w9.yaw_is_neither_clamped_nor_wrapped.
    // MUTATION: wrap the yaw to [-180, 180) — which looks like tidying and is a
    // divergence: `setYRot` is a bare assignment, and the raw value is what
    // goes back out on the wire.
    let ((yaw, _), _) = route_rot(ids, rot_id, &rot_body(30.0, true, 0.0, true), (350.0, 0.0));
    c.record(
        "w9.yaw_is_neither_clamped_nor_wrapped",
        yaw == 380.0,
        format!("350 + 30 -> {yaw} (want 380, not 20)"),
    );

    // w9.a_nan_pitch_is_discarded_not_clamped.
    // MUTATION: make `Mth.clamp` collapse NaN to a bound, or replace the
    // setters' `Float.isFinite` guard with a default. Vanilla logs and returns
    // *without writing*, so the previous pitch stands — and the yaw half of the
    // same packet still applies, which a whole-packet reject would lose.
    let ((yaw, pitch), _) = route_rot(
        ids,
        rot_id,
        &rot_body(5.0, true, f32::NAN, false),
        (10.0, 25.0),
    );
    c.record(
        "w9.a_nan_pitch_is_discarded_not_clamped",
        yaw == 15.0 && pitch == 25.0,
        format!("({yaw}, {pitch}) want (15, 25) — not (15, ±90) and not (10, 25)"),
    );

    // w9.look_at_uses_the_minecraft_yaw_convention.
    // MUTATION: `atan2(xd, zd)` instead of `atan2(zd, xd)`, or drop the
    // `- 90.0F`. All four cardinals are checked because a single sample is
    // satisfied by several wrong conventions.
    let near_deg = |a: f32, b: f32| (a - b).abs() < 1.0e-3;
    let cardinal = |x: f64, z: f64| {
        route_rot_from(
            ids,
            look_id,
            &look_body(0, [x, 0.0, z], None),
            (0.0, 0.0),
            [0.0, 0.0, 0.0],
            |_, _| None,
        )
        .0
         .0
    };
    let (s, w, n, e) = (
        cardinal(0.0, 1.0),
        cardinal(-1.0, 0.0),
        cardinal(0.0, -1.0),
        cardinal(1.0, 0.0),
    );
    c.record(
        "w9.look_at_uses_the_minecraft_yaw_convention",
        near_deg(s, 0.0) && near_deg(w, 90.0) && near_deg(n, -180.0) && near_deg(e, -90.0),
        format!("south={s:.3} west={w:.3} north={n:.3} east={e:.3} (want 0, 90, ±180, -90)"),
    );

    // w9.look_at_pitch_is_negative_upward.
    // MUTATION: drop the leading unary minus in
    // `-(Mth.atan2(yd, sd) * 180.0F / (float)Math.PI)`. A level target cannot
    // see it, so both signs are sampled.
    let pitch_at = |y: f64, z: f64| {
        route_rot_from(
            ids,
            look_id,
            &look_body(0, [0.0, y, z], None),
            (0.0, 0.0),
            [0.0, 0.0, 0.0],
            |_, _| None,
        )
        .0
         .1
    };
    let (up, down) = (pitch_at(1.0, 1.0), pitch_at(-1.0, 1.0));
    c.record(
        "w9.look_at_pitch_is_negative_upward",
        near_deg(up, -45.0) && near_deg(down, 45.0),
        format!("up={up:.3} (want -45), down={down:.3} (want 45)"),
    );

    // w9.the_from_anchor_is_the_viewers_own.
    // MUTATION: apply `fromAnchor` to the target, or ignore it. `Entity.lookAt`
    // starts the ray at `anchor.apply(this)`, so the eye ray to a target 10
    // blocks up is 1.62 blocks less steep.
    let from_anchor = |anchor: u8| {
        route_rot_from(
            ids,
            look_id,
            &look_body(anchor, [0.0, 10.0, 10.0], None),
            (0.0, 0.0),
            [0.0, 0.0, 0.0],
            |_, _| None,
        )
        .0
    };
    let ((feet_yaw, feet_pitch), (eye_yaw, eye_pitch)) = (from_anchor(0), from_anchor(1));
    c.record(
        "w9.the_from_anchor_is_the_viewers_own",
        near_deg(feet_pitch, -45.0) && eye_pitch > feet_pitch && feet_yaw == eye_yaw,
        format!(
            "feet pitch={feet_pitch:.3} (want -45), eyes pitch={eye_pitch:.3} (want > feet), \
             yaw unchanged: {}",
            feet_yaw == eye_yaw
        ),
    );

    // w9.an_unknown_target_entity_uses_the_carried_coordinates.
    // MUTATION: treat an unresolvable entity as "do nothing". `getPosition`
    // falls back to the packet's own x/y/z, and those are the *sender's*
    // snapshot of `toAnchor.apply(entity)` — a correct point, stale by however
    // far the entity moved. Dropping the rotation loses one vanilla performs.
    let ((unknown_yaw, _), _) = route_rot_from(
        ids,
        look_id,
        &look_body(0, [0.0, 0.0, 1.0], Some((5, 0))),
        (123.0, 45.0),
        [0.0, 0.0, 0.0],
        |_, _| None,
    );
    // w9.a_resolvable_target_entity_overrides_them.
    // MUTATION: ignore the resolver. The resolved point is due *west* and the
    // carried one due south, so the two answers cannot be confused.
    let ((known_yaw, _), _) = route_rot_from(
        ids,
        look_id,
        &look_body(0, [0.0, 0.0, 1.0], Some((5, 0))),
        (123.0, 45.0),
        [0.0, 0.0, 0.0],
        |id, _| (id == 5).then_some([-1.0, 0.0, 0.0]),
    );
    c.record(
        "w9.an_unknown_target_entity_uses_the_carried_coordinates",
        near_deg(unknown_yaw, 0.0),
        format!("yaw={unknown_yaw:.3} (want 0 = the carried point, not 123 = untouched)"),
    );
    c.record(
        "w9.a_resolvable_target_entity_overrides_them",
        near_deg(known_yaw, 90.0),
        format!("yaw={known_yaw:.3} (want 90 = the resolved point, not 0 = the carried one)"),
    );

    // w9.the_point_form_never_consults_the_resolver.
    // MUTATION: drop the `at_entity` test in `getPosition`. A resolver that
    // answers would then redirect a packet that names no entity at all.
    let mut consulted = false;
    let ((point_yaw, _), _) = route_rot_from(
        ids,
        look_id,
        &look_body(0, [0.0, 0.0, 1.0], None),
        (0.0, 0.0),
        [0.0, 0.0, 0.0],
        |_, _| {
            consulted = true;
            Some([-1.0, 0.0, 0.0])
        },
    );
    c.record(
        "w9.the_point_form_never_consults_the_resolver",
        !consulted && near_deg(point_yaw, 0.0),
        format!("resolver consulted={consulted}, yaw={point_yaw:.3} (want 0)"),
    );

    // w9.look_at_evaluates_vanillas_atan2_not_the_platforms.
    // MUTATION: `rewo_world::rotation::atan2` -> `y.atan2(x)`. Two-sided on
    // purpose: `Mth.atan2` is a 257-entry table plus a Quake `fastInvSqrt`, so
    // it must stay *close* to the platform (or it is simply broken) and must
    // *not* equal it (or someone substituted the platform function, which is
    // the M12 `Mth.sin` mistake one function over). The bound is measured over
    // a sweep rather than at one point, because the two agree exactly at the
    // cardinals where a spot check would look.
    let mut worst: f64 = 0.0;
    let mut agreed_everywhere = true;
    for i in -30..=30 {
        for j in -30..=30 {
            let (y, x) = (f64::from(i) * 0.31, f64::from(j) * 0.47);
            if x == 0.0 && y == 0.0 {
                continue;
            }
            let d = (rewo_world::rotation::atan2(y, x) - y.atan2(x)).abs();
            worst = worst.max(d);
            if d != 0.0 {
                agreed_everywhere = false;
            }
        }
    }
    c.record(
        "w9.look_at_evaluates_vanillas_atan2_not_the_platforms",
        worst < 1.0e-5 && !agreed_everywhere,
        format!(
            "worst |Mth.atan2 - f64::atan2| = {worst:.3e} (want < 1e-5 and > 0; \
             identical everywhere: {agreed_everywhere})"
        ),
    );

    // w9.only_player_rotation_answers_the_server.
    // MUTATION: return `Rotation` for both, or `LookAt` for both.
    // `handleRotatePlayer` ends with an immediate
    // `ServerboundMovePlayerPacket.Rot`; `handleLookAt` sends nothing and lets
    // the next tick's movement report carry the angles. The seam's answer is
    // the only thing that carries that distinction to the session.
    let (_, r1) = route_rot(ids, rot_id, &rot_body(0.0, true, 0.0, true), (0.0, 0.0));
    let (_, r2) = route_rot_from(
        ids,
        look_id,
        &look_body(0, [0.0, 0.0, 1.0], None),
        (0.0, 0.0),
        [0.0, 0.0, 0.0],
        |_, _| None,
    );
    c.record(
        "w9.only_player_rotation_answers_the_server",
        r1 == RotationRoute::Rotation && r2 == RotationRoute::LookAt,
        format!("rotation -> {r1:?}, look_at -> {r2:?}"),
    );

    // w9.a_malformed_body_still_claims_its_id.
    // MUTATION: return `NoMatch` when the body fails to decode. The id *did*
    // match, and falling through would let the dispatch ladder test the same
    // packet against every arm below it.
    let (_, r) = route_rot(ids, rot_id, &[], (0.0, 0.0));
    c.record(
        "w9.a_malformed_body_still_claims_its_id",
        r == RotationRoute::Rotation,
        format!("an empty player_rotation body routed as {r:?}"),
    );
}

// ------------------------------------------------- 10. the world spawn (97)

fn check_respawn_data(c: &mut Checker, ids: &Ids) {
    let entities = rewo_world::entities::EntityTable::default();

    // w10.the_dimension_is_an_identifier_string.
    // MUTATION: read the dimension as a VarInt registry id (the
    // `holderRegistry` shape M16 records for the *dimension type*). `GlobalPos`
    // uses `ResourceKey.streamCodec`, which is `Identifier.STREAM_CODEC` — a
    // length-prefixed string. A VarInt reading would eat the length prefix and
    // misread everything after it, which the position below would then show.
    let body = world_spawn_body("minecraft:the_nether", (100, 64, -200), 45.0, -12.5);
    let d = read_set_default_spawn_position(&body).expect("decode");
    c.record(
        "w10.the_dimension_is_an_identifier_string",
        d.dimension == "minecraft:the_nether",
        format!("{:?}", d.dimension),
    );

    // w10.the_block_pos_is_the_packed_26_26_12_long.
    // MUTATION: read three VarInts, or unpack in x/y/z order. The sample uses a
    // negative Z so the sign extension is exercised — a mask-without-sext
    // reading gives a large positive.
    c.record(
        "w10.the_block_pos_is_the_packed_26_26_12_long",
        d.pos == (100, 64, -200),
        format!("{:?} (want (100, 64, -200))", d.pos),
    );

    // w10.yaw_and_pitch_are_stored_verbatim.
    // MUTATION: apply `RespawnData.of`'s `Mth.wrapDegrees(yaw)` /
    // `Mth.clamp(pitch, -90, 90)`, or `MAP_CODEC`'s `floatRange` bounds. The
    // STREAM_CODEC is a bare `composite` over the record's accessors and does
    // neither. Both sample values are outside the range those would impose, so
    // either mutation moves them.
    let wild = world_spawn_body("minecraft:overworld", (0, 0, 0), 540.0, 130.0);
    let w = read_set_default_spawn_position(&wild).expect("decode");
    c.record(
        "w10.yaw_and_pitch_are_stored_verbatim",
        w.yaw == 540.0 && w.pitch == 130.0,
        format!("yaw={} pitch={} (want 540 / 130, not 180 / 90)", w.yaw, w.pitch),
    );

    // w10.the_packet_lands_through_the_client_state_seam.
    // MUTATION: leave the id out of `ClientStateIds` / `kind_for_id`, which is
    // the failure the coverage table's machine check cannot see (it proves the
    // *field* is dispatched, not that the seam's table carries it). Driven
    // through `route_client_state` with the real `Ids` for M45's reason.
    let mut state = ClientState::default();
    let matched = rewo_net::route_client_state(
        ids.cb_play_set_default_spawn_position,
        &body,
        ids,
        &mut state,
        &entities,
        Some(1),
    );
    let stored = state.respawn_data().cloned();
    c.record(
        "w10.the_packet_lands_through_the_client_state_seam",
        matched && stored.as_ref().map(|r| r.pos) == Some((100, 64, -200)),
        format!("matched={matched} stored={stored:?}"),
    );

    // w10.a_level_default_is_8_64_8_of_that_level.
    // MUTATION: seed with `LevelData.RespawnData.DEFAULT` — overworld,
    // `BlockPos.ZERO`. That constant exists but **no client ever holds it**:
    // `ClientLevelData.respawnData` has no initialiser and the `ClientLevel`
    // constructor immediately writes `RespawnData.of(dimension,
    // BlockPos(8, 64, 8), 0, 0)`. The dimension is the level being entered, so
    // the default follows you into the Nether.
    let mut fresh = ClientState::default();
    let before = fresh.respawn_data().is_none();
    fresh.enter_level("minecraft:the_end");
    let seeded = fresh.respawn_data().cloned();
    c.record(
        "w10.a_level_default_is_8_64_8_of_that_level",
        before
            && seeded.as_ref().map(|r| (r.pos, r.dimension.as_str()))
                == Some(((8, 64, 8), "minecraft:the_end")),
        format!("was None: {before}, seeded {seeded:?}"),
    );

    // w10.entering_a_level_discards_the_previous_spawn.
    // MUTATION: make `enter_level` fill only when the slot is empty. Vanilla
    // builds a *new* `ClientLevelData` on a dimension change and the field is
    // not among the three the constructor carries over, so travel resets the
    // world spawn — the opposite of the difficulty sitting beside it, which
    // `handleRespawn` copies across explicitly. (The same-dimension respawn
    // path keeps it; that clause lives in `play::apply_respawn` and is gated on
    // the transition, not here.)
    let mut travelled = ClientState::default();
    travelled.enter_level("minecraft:overworld");
    rewo_net::route_client_state(
        ids.cb_play_set_default_spawn_position,
        &body,
        ids,
        &mut travelled,
        &entities,
        Some(1),
    );
    let carried = travelled.respawn_data().cloned();
    travelled.enter_level("minecraft:the_nether");
    let after = travelled.respawn_data().cloned();
    c.record(
        "w10.entering_a_level_discards_the_previous_spawn",
        carried.as_ref().map(|r| r.pos) == Some((100, 64, -200))
            && after.as_ref().map(|r| (r.pos, r.dimension.as_str()))
                == Some(((8, 64, 8), "minecraft:the_nether")),
        format!("held {:?}, after travel {after:?}", carried.map(|r| r.pos)),
    );

    // w10.the_difficulty_beside_it_is_untouched.
    // MUTATION: reset the whole `ClientState` on `enter_level`. The two fields
    // live on the same vanilla object and behave oppositely across a dimension
    // change; conflating them is the natural simplification and is wrong in one
    // direction or the other whichever way it is done.
    let mut both = ClientState::default();
    both.apply_change_difficulty(Difficulty::Hard, true);
    both.enter_level("minecraft:the_nether");
    c.record(
        "w10.the_difficulty_beside_it_is_untouched",
        both.difficulty == Difficulty::Hard && both.difficulty_locked,
        format!("{:?} locked={}", both.difficulty, both.difficulty_locked),
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
    // M78's eight, through the same production resolver. Printed because two
    // of them are the whole point of a *pair* of ids: `custom_payload` and
    // `store_cookie` are `common` packets and exist in configuration as well.
    println!(
        "[abilityshot] M78 session ids: bundle_delimiter={} custom_payload={}/{} \
         disguised_chat={} game_rule_values={} player_combat_end={} \
         player_combat_enter={} server_data={} store_cookie={}/{}",
        ids.cb_play_bundle_delimiter,
        ids.cb_config_custom_payload,
        ids.cb_play_custom_payload,
        ids.cb_play_disguised_chat,
        ids.cb_play_game_rule_values,
        ids.cb_play_player_combat_end,
        ids.cb_play_player_combat_enter,
        ids.cb_play_server_data,
        ids.cb_config_store_cookie,
        ids.cb_play_store_cookie,
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
    check_bundle(
        &mut c,
        ids.cb_play_bundle_delimiter,
        ids.cb_play_start_configuration,
    );
    check_session(&mut c, &ids);
    // M76 — the rotation the server writes, and the world spawn.
    println!(
        "[abilityshot] player_rotation={} player_look_at={} set_default_spawn_position={}",
        ids.cb_play_player_rotation,
        ids.cb_play_player_look_at,
        ids.cb_play_set_default_spawn_position
    );
    check_rotation_wire(&mut c, &ids);
    check_rotation_semantics(&mut c, &ids);
    check_respawn_data(&mut c, &ids);

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
