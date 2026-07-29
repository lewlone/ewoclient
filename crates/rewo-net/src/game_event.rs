//! `ClientboundGameEventPacket` — all fourteen types (M71).
//!
//! M33 consumed four of them (the weather levels) and the other ten were
//! matched and thrown away, including `CHANGE_GAME_MODE` — the local player's
//! own gamemode change. `REWO_PACKET_COVERAGE.md` §4 named this as the sharpest
//! example of "handled" not meaning "complete": every grep called `game_event`
//! handled while it decoded 4 of 14.
//!
//! ## The body
//!
//! Fixed **five bytes** — `readUnsignedByte()` then `readFloat()`, neither a
//! var-int. There is no length prefix and no trailing field, so a decoder that
//! miscounts here corrupts nothing else (the packet ends), but it does silently
//! mis-read the event.
//!
//! ## There are two params, not one
//!
//! `handleGameEvent` computes `int param = Mth.floor(paramFloat + 0.5F)` — a
//! *rounded* int — and then uses it in exactly **two** branches:
//! `CHANGE_GAME_MODE` and `GUARDIAN_ELDER_EFFECT`. Every other branch reads the
//! **raw float**, and three of them compare it for exact equality
//! (`DEMO_EVENT` against `0/101/102/103/104`, `IMMEDIATE_RESPAWN` against `0`,
//! `LIMITED_CRAFTING` against `1`). Using one where the other belongs is
//! invisible for an integral param and wrong for anything else — a server
//! sending `0.4` means "not limited crafting" to vanilla and would mean
//! "limited crafting" to a rounding implementation.
//!
//! [`GameEvent`] resolves that at decode time: each variant already carries the
//! form its branch actually reads, so a caller cannot pick the wrong one.
//!
//! ## An unknown type id is a silent no-op, not an error
//!
//! `Type.TYPES.get(id)` returns `null` for an unregistered id, and every
//! `event == CONSTANT` comparison against `null` is `false`, so vanilla falls
//! out of the whole `if`/`else if` chain having done nothing. That is a
//! *successful* decode of a packet with no effect — [`decode`] returns
//! `Ok(None)`, never an error, or a future protocol addition would kill a
//! connection that vanilla shrugs off.
//!
//! ## Ground truth (bundled 26.2 decompile)
//!
//! - `net/minecraft/network/protocol/game/ClientboundGameEventPacket.java`
//! - `net/minecraft/client/multiplayer/ClientPacketListener.java`
//!   (`handleGameEvent`)
//! - `net/minecraft/client/multiplayer/MultiPlayerGameMode.java`
//!   (`setLocalMode`)
//! - `net/minecraft/client/multiplayer/ClientLevel.java` (`playSeededSound`)
//! - `net/minecraft/client/player/LocalPlayer.java` (the two flag defaults)
//! - `net/minecraft/world/level/GameType.java` (`byId`, `updatePlayerAbilities`)

use crate::play::GameMode;
use crate::sounds::SoundSource;
use rewo_world::physics::PlayerState;
use rewo_proto::reader::PacketReader;
use rewo_proto::Result;

/// The fourteen `ClientboundGameEventPacket.Type` ids, 0..=13.
///
/// The four weather ids are duplicated by [`rewo_world::weather::WeatherState`]
/// because that type owns the weather implementation; a test below asserts the
/// two tables agree, so they cannot drift.
pub mod ids {
    pub const NO_RESPAWN_BLOCK_AVAILABLE: u8 = 0;
    pub const START_RAINING: u8 = 1;
    pub const STOP_RAINING: u8 = 2;
    pub const CHANGE_GAME_MODE: u8 = 3;
    pub const WIN_GAME: u8 = 4;
    pub const DEMO_EVENT: u8 = 5;
    pub const PLAY_ARROW_HIT_SOUND: u8 = 6;
    pub const RAIN_LEVEL_CHANGE: u8 = 7;
    pub const THUNDER_LEVEL_CHANGE: u8 = 8;
    pub const PUFFER_FISH_STING: u8 = 9;
    pub const GUARDIAN_ELDER_EFFECT: u8 = 10;
    pub const IMMEDIATE_RESPAWN: u8 = 11;
    pub const LIMITED_CRAFTING: u8 = 12;
    pub const LEVEL_CHUNKS_LOAD_START: u8 = 13;

    /// One past the last registered id. Anything at or above this is the
    /// silent no-op described in the module docs.
    pub const COUNT: u8 = 14;
}

/// The packet body is exactly this many bytes: one unsigned byte, one f32.
pub const BODY_LEN: usize = 5;

/// `Mth.floor(paramFloat + 0.5F)` — the *rounded* param.
///
/// `Mth.floor(float)` is `(int)Math.floor(value)`, and the `+ 0.5F` is a float
/// add, so the rounding happens in `f32` before the narrowing cast. Rust's
/// `as i32` matches Java's `(int)` cast on both edges that matter here: NaN
/// becomes 0 and out-of-range values saturate.
fn rounded(param: f32) -> i32 {
    (param + 0.5f32).floor() as i32
}

/// Which demo hint `DEMO_EVENT`'s param selects.
///
/// The five constants are `DEMO_PARAM_INTRO` and `DEMO_PARAM_HINT_1..4` on the
/// packet class. Vanilla compares the **raw float** for exact equality, so a
/// param matching none of them leaves `message` null and does nothing at all —
/// which is why [`DemoHint::from_param`] returns `Option`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DemoHint {
    /// Param `0.0` — opens the demo intro popup rather than sending a message.
    Intro,
    /// Param `101.0` — `demo.help.movement`.
    Movement,
    /// Param `102.0` — `demo.help.jump`.
    Jump,
    /// Param `103.0` — `demo.help.inventory`.
    Inventory,
    /// Param `104.0` — `demo.day.6`.
    Day6,
}

impl DemoHint {
    /// Exact float equality, as vanilla writes it. `101.5` is not a hint.
    pub fn from_param(param: f32) -> Option<DemoHint> {
        Some(match param {
            p if p == 0.0 => DemoHint::Intro,
            p if p == 101.0 => DemoHint::Movement,
            p if p == 102.0 => DemoHint::Jump,
            p if p == 103.0 => DemoHint::Inventory,
            p if p == 104.0 => DemoHint::Day6,
            _ => return None,
        })
    }
}

/// One sound `handleGameEvent` plays locally, with everything except its
/// position — which is the local player's, and so belongs to the session.
///
/// Vanilla routes these through `ClientLevel.playSeededSound`, whose body is
/// `if (except == this.minecraft.player)`. **On the client that reads the
/// opposite way round to the server**: the `except` argument names the player
/// the *server* would skip, and the client plays the sound only when it is the
/// local player. `handleGameEvent` passes the local player, so all three of
/// these are audible; passing anything else would be silence.
///
/// The seed is deliberately absent. A wire sound carries one
/// ([`crate::sounds::PositionedSound::seed`]) so every client picks the same
/// variant; a client-local sound draws `level.random.nextLong()` at play time,
/// so there is no number here to record and a playback layer supplies its own.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LocalSoundSpec {
    /// The registry name, from `SoundEvents`.
    pub name: &'static str,
    pub source: SoundSource,
    pub volume: f32,
    pub pitch: f32,
    /// `PLAY_ARROW_HIT_SOUND` plays at `player.getEyeY()`; the other two play
    /// at `player.getY()`. A one-field difference that is easy to flatten and
    /// would put the arrow sound at the player's feet.
    pub at_eye: bool,
}

const ARROW_HIT_PLAYER: LocalSoundSpec = LocalSoundSpec {
    name: "entity.arrow.hit_player",
    source: SoundSource::Players,
    volume: 0.18,
    pitch: 0.45,
    at_eye: true,
};

const PUFFER_FISH_STING: LocalSoundSpec = LocalSoundSpec {
    name: "entity.puffer_fish.sting",
    source: SoundSource::Neutral,
    volume: 1.0,
    pitch: 1.0,
    at_eye: false,
};

const ELDER_GUARDIAN_CURSE: LocalSoundSpec = LocalSoundSpec {
    name: "entity.elder_guardian.curse",
    source: SoundSource::Hostile,
    volume: 1.0,
    pitch: 1.0,
    at_eye: false,
};

/// The translation key `NO_RESPAWN_BLOCK_AVAILABLE` sends to chat.
pub const NO_RESPAWN_BLOCK_KEY: &str = "block.minecraft.spawn.not_valid";

/// A `game_event` whose type id was recognised, with its param already
/// resolved into the form its own branch reads.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GameEvent {
    /// Sends the translatable system message [`NO_RESPAWN_BLOCK_KEY`].
    NoRespawnBlockAvailable,
    /// Weather. Applied by [`rewo_world::weather::WeatherState`], not here —
    /// and note it sets the rain level to **0**, not 1. See that type's docs.
    StartRaining,
    /// Weather; sets the rain level to **1**. See [`Self::StartRaining`].
    StopRaining,
    /// `gameMode.setLocalMode(GameType.byId(param))` — the **rounded** param.
    /// `GameType.byId` is `ByIdMap.continuous(..., ZERO)`, so an out-of-range
    /// id is `SURVIVAL` rather than an error (the convention M62 recorded).
    ChangeGameMode(GameMode),
    /// Opens the credits/win screen.
    WinGame,
    /// `None` when the param matched none of the five demo constants, which in
    /// vanilla means the whole branch does nothing.
    DemoEvent(Option<DemoHint>),
    /// A local sound only.
    PlayArrowHitSound,
    /// Weather. The raw float, applied by `WeatherState`.
    RainLevelChange(f32),
    /// Weather. The raw float, applied by `WeatherState`.
    ThunderLevelChange(f32),
    /// A local sound only.
    PufferFishSting,
    /// Adds the `minecraft:elder_guardian` particle **unconditionally**, and
    /// plays the curse sound only when the *rounded* param is exactly 1.
    GuardianElderEffect { curse: bool },
    /// **Inverted**: `setShowDeathScreen(paramFloat == 0.0F)`, so a param of
    /// zero means *show* the death screen and the immediate-respawn gamerule
    /// being on arrives as a non-zero param.
    ImmediateRespawn { show_death_screen: bool },
    /// `setDoLimitedCrafting(paramFloat == 1.0F)`.
    LimitedCrafting(bool),
    /// Vanilla guards this on `levelLoadTracker != null`.
    LevelChunksLoadStart,
}

impl GameEvent {
    /// Resolve a raw `(id, param)` pair. `None` is an unregistered id — the
    /// silent no-op, not a failure.
    pub fn classify(id: u8, param: f32) -> Option<GameEvent> {
        Some(match id {
            ids::NO_RESPAWN_BLOCK_AVAILABLE => GameEvent::NoRespawnBlockAvailable,
            ids::START_RAINING => GameEvent::StartRaining,
            ids::STOP_RAINING => GameEvent::StopRaining,
            ids::CHANGE_GAME_MODE => GameEvent::ChangeGameMode(GameMode::by_id(rounded(param))),
            ids::WIN_GAME => GameEvent::WinGame,
            ids::DEMO_EVENT => GameEvent::DemoEvent(DemoHint::from_param(param)),
            ids::PLAY_ARROW_HIT_SOUND => GameEvent::PlayArrowHitSound,
            ids::RAIN_LEVEL_CHANGE => GameEvent::RainLevelChange(param),
            ids::THUNDER_LEVEL_CHANGE => GameEvent::ThunderLevelChange(param),
            ids::PUFFER_FISH_STING => GameEvent::PufferFishSting,
            ids::GUARDIAN_ELDER_EFFECT => GameEvent::GuardianElderEffect {
                curse: rounded(param) == 1,
            },
            ids::IMMEDIATE_RESPAWN => GameEvent::ImmediateRespawn {
                show_death_screen: param == 0.0,
            },
            ids::LIMITED_CRAFTING => GameEvent::LimitedCrafting(param == 1.0),
            ids::LEVEL_CHUNKS_LOAD_START => GameEvent::LevelChunksLoadStart,
            _ => return None,
        })
    }

    /// Whether this event is one of the four the weather state owns.
    ///
    /// Here so a caller can route without duplicating the id list; the
    /// *application* stays in `WeatherState::apply_game_event`.
    pub fn is_weather(self) -> bool {
        matches!(
            self,
            GameEvent::StartRaining
                | GameEvent::StopRaining
                | GameEvent::RainLevelChange(_)
                | GameEvent::ThunderLevelChange(_)
        )
    }

    /// The client-local sounds this event plays, in vanilla's order.
    pub fn sounds(self) -> &'static [LocalSoundSpec] {
        match self {
            GameEvent::PlayArrowHitSound => std::slice::from_ref(&ARROW_HIT_PLAYER),
            GameEvent::PufferFishSting => std::slice::from_ref(&PUFFER_FISH_STING),
            GameEvent::GuardianElderEffect { curse: true } => {
                std::slice::from_ref(&ELDER_GUARDIAN_CURSE)
            }
            _ => &[],
        }
    }
}

/// A decoded `game_event`, keeping the raw pair beside the classified form.
///
/// The raw `id`/`param` survive so the weather path can call
/// `WeatherState::apply_game_event(id, param)` — the single implementation of
/// the weather rules — instead of a second copy that could drift from it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DecodedGameEvent {
    pub id: u8,
    pub param: f32,
    pub kind: GameEvent,
}

/// Decode one `ClientboundGameEventPacket` body.
///
/// `Err` is a short body (vanilla's reader would throw); `Ok(None)` is a
/// well-formed packet naming an unregistered type, which vanilla accepts and
/// ignores.
pub fn decode(body: &[u8]) -> Result<Option<DecodedGameEvent>> {
    let mut r = PacketReader::new(body);
    let id = r.u8()?;
    let param = r.f32()?;
    Ok(GameEvent::classify(id, param).map(|kind| DecodedGameEvent { id, param, kind }))
}

/// Apply one `ClientboundGameEventPacket` body: weather, client game state,
/// and the client-local sounds it asks for.
///
/// **All of the routing lives here rather than in
/// [`crate::play::PlaySession`]**, which owns a socket and so has no unit
/// tests. A mutation battery found exactly that: with the weather/state/sound
/// fan-out written at the call site, dropping any one of the three branches
/// survived the whole suite. The session is now a six-line adapter with no
/// logic to get wrong, and everything below is witnessed. Same instinct as
/// M18's shared live resolver and M45's `install_shapes` gap.
///
/// It takes the whole [`PlayerState`] rather than loose coordinates on
/// purpose: with an `(x, feet_y, z)` list, a caller could pass `eye_y()` into
/// the `feet_y` slot or transpose two axes, and the mutation battery confirmed
/// both survived. Handing over the player makes those unrepresentable — the
/// adapter has no coordinate arithmetic left to get wrong.
///
/// Returns what it decoded and the sounds it produced, from **one** decode —
/// the caller queues the sounds (the queue is capped, which is the session's
/// business) and [`crate::apply_game_event`] reads the event back to report
/// whether it was a weather one.
pub fn apply(
    body: &[u8],
    weather: &mut rewo_world::weather::WeatherState,
    state: &mut ClientGameState,
    player: &PlayerState,
) -> Applied {
    let Ok(Some(ev)) = decode(body) else {
        return Applied::default();
    };
    if ev.kind.is_weather() {
        // The raw pair, so `WeatherState` stays the single implementation of
        // the weather rules rather than gaining a second copy here.
        weather.apply_game_event(ev.id, ev.param);
    }
    state.apply(ev.kind);
    let sounds = ev
        .kind
        .sounds()
        .iter()
        .map(|spec| crate::sounds::LocalSound {
            name: spec.name.to_string(),
            source: spec.source,
            x: player.x,
            // `getEyeY()` for the arrow, `getY()` for the other two.
            y: if spec.at_eye {
                player.eye_y()
            } else {
                player.y
            },
            z: player.z,
            volume: spec.volume,
            pitch: spec.pitch,
        })
        .collect();
    Applied {
        event: Some(ev),
        sounds,
    }
}

/// What [`apply`] did, from one decode.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Applied {
    /// `None` for a short body or an unregistered type id — both of which
    /// leave everything untouched.
    pub event: Option<DecodedGameEvent>,
    /// Client-local sounds, in vanilla's order, for the caller to queue.
    pub sounds: Vec<crate::sounds::LocalSound>,
}

impl Applied {
    /// Whether the packet was one of the four weather events.
    pub fn was_weather(&self) -> bool {
        self.event.is_some_and(|ev| ev.kind.is_weather())
    }
}

/// The client-side state the ten non-weather events write.
///
/// Weather is deliberately **not** here — it lives in
/// [`rewo_world::weather::WeatherState`], which owns the four levels and their
/// counter-intuitive start/stop rule. [`Self::apply`] ignores those four rather
/// than dropping them; the session applies both.
///
/// ## `game_event` is not the only writer of any of this
///
/// Three of these fields have an authoritative source *other* than
/// `game_event`, and this type is fed by `game_event` alone — so it is
/// complete for the packet and not yet complete for the state. Precisely:
///
/// - **The two flags arrive at join on the login packet.**
///   `PlayerList.placeNewPlayer` puts `!immediateRespawn` and
///   `doLimitedCrafting` into `ClientboundLoginPacket`, and `handleLogin`
///   calls both setters from it. The `game_event` ids 11/12 are only the
///   *mid-session gamerule change* (`MinecraftServer` sends them when
///   `IMMEDIATE_RESPAWN` or `LIMITED_CRAFTING` is edited). So the defaults
///   below are vanilla's field initialisers, correct until the server speaks,
///   but the join-time truth rides a packet whose flags Rewo does not read.
/// - **Gamemode also arrives on the login and respawn packets**, via
///   `spawnInfo.gameType()`. `handleRespawn` ends with the **two-argument**
///   `setLocalMode(gameType, previousGameType)`, which assigns both directly
///   with none of the change-guard the one-argument form has.
///   [`crate::spawn_info`] already decodes `game_type` and
///   `previous_game_type`; nothing feeds them here.
///
/// ## Why nothing here is cleared on a dimension change
///
/// `MultiPlayerGameMode` outlives the `ClientLevel`, so the gamemode persists.
/// `handleRespawn` builds a new `LocalPlayer` and explicitly copies
/// `showDeathScreen` across — **but not `doLimitedCrafting`**, which therefore
/// resets to `false` in vanilla until the server re-announces it. That
/// asymmetry is vanilla's, so a blanket [`Self::clear`] on a dimension change
/// would be wrong for two of the three; `clear` exists for a reconnect, where
/// the whole session is new.
#[derive(Clone, Debug, PartialEq)]
pub struct ClientGameState {
    game_mode: Option<GameMode>,
    previous_game_mode: Option<GameMode>,
    show_death_screen: bool,
    do_limited_crafting: bool,
    won_game: bool,
    demo_hint: Option<DemoHint>,
    chunks_load_started: bool,
    pending_system_messages: Vec<&'static str>,
}

impl Default for ClientGameState {
    /// `LocalPlayer` declares `showDeathScreen = true` and
    /// `doLimitedCrafting = false` as field initialisers, so those are the
    /// values in force before any `game_event` arrives — not `false`/`false`.
    fn default() -> Self {
        Self {
            game_mode: None,
            previous_game_mode: None,
            show_death_screen: true,
            do_limited_crafting: false,
            won_game: false,
            demo_hint: None,
            chunks_load_started: false,
            pending_system_messages: Vec::new(),
        }
    }
}

impl ClientGameState {
    /// Cap on the queued system messages.
    ///
    /// The windowed client drains this every tick, but the headless harnesses
    /// never do, and a server can send `NO_RESPAWN_BLOCK_AVAILABLE` on every
    /// respawn attempt. Same reasoning as
    /// [`crate::play::PlaySession::MAX_PENDING_SOUNDS`]: a queue with no
    /// guaranteed consumer is bounded, dropping the oldest.
    pub const MAX_PENDING_SYSTEM_MESSAGES: usize = 64;

    /// Apply one decoded event. The four weather ids are no-ops here.
    pub fn apply(&mut self, event: GameEvent) {
        match event {
            GameEvent::NoRespawnBlockAvailable => {
                if self.pending_system_messages.len() >= Self::MAX_PENDING_SYSTEM_MESSAGES {
                    self.pending_system_messages.remove(0);
                }
                self.pending_system_messages.push(NO_RESPAWN_BLOCK_KEY);
            }
            GameEvent::ChangeGameMode(mode) => {
                // `setLocalMode(GameType)` guards the previous-mode write on
                // the mode actually changing, so a server re-announcing the
                // current mode does NOT clobber the previous one.
                if self.game_mode != Some(mode) {
                    self.previous_game_mode = self.game_mode;
                }
                self.game_mode = Some(mode);
            }
            GameEvent::WinGame => self.won_game = true,
            GameEvent::DemoEvent(hint) => {
                // A param matching no constant leaves vanilla's `message`
                // null and opens no screen, so it must not clear a hint.
                if let Some(hint) = hint {
                    self.demo_hint = Some(hint);
                }
            }
            GameEvent::LevelChunksLoadStart => self.chunks_load_started = true,
            GameEvent::ImmediateRespawn { show_death_screen } => {
                self.show_death_screen = show_death_screen;
            }
            GameEvent::LimitedCrafting(v) => self.do_limited_crafting = v,
            // Sound- and particle-only; no state.
            GameEvent::PlayArrowHitSound
            | GameEvent::PufferFishSting
            | GameEvent::GuardianElderEffect { .. } => {}
            // Weather — `WeatherState` owns these.
            GameEvent::StartRaining
            | GameEvent::StopRaining
            | GameEvent::RainLevelChange(_)
            | GameEvent::ThunderLevelChange(_) => {}
        }
    }

    /// The local player's gamemode, or `None` before the server announces one.
    ///
    /// `None` is not `Some(Survival)`: vanilla's `MultiPlayerGameMode` is
    /// constructed with the mode from the login packet, which Rewo does not
    /// decode, so "not yet told" is a real third state here.
    pub fn game_mode(&self) -> Option<GameMode> {
        self.game_mode
    }

    /// The mode held before the most recent *change* — `setLocalMode`'s
    /// `previousLocalPlayerMode`.
    pub fn previous_game_mode(&self) -> Option<GameMode> {
        self.previous_game_mode
    }

    /// `LocalPlayer.shouldShowDeathScreen()`. Defaults to `true`.
    pub fn show_death_screen(&self) -> bool {
        self.show_death_screen
    }

    /// `LocalPlayer.getDoLimitedCrafting()`. Defaults to `false`.
    pub fn do_limited_crafting(&self) -> bool {
        self.do_limited_crafting
    }

    /// Whether `WIN_GAME` has been received. Vanilla opens the credits screen;
    /// Rewo has no screen system, so this is the whole effect.
    pub fn won_game(&self) -> bool {
        self.won_game
    }

    /// The most recent demo hint. Vanilla renders four of the five as chat
    /// messages formatted with the player's own keybind names, which Rewo has
    /// no source for, so the hint is recorded rather than turned into text.
    pub fn demo_hint(&self) -> Option<DemoHint> {
        self.demo_hint
    }

    /// Whether `LEVEL_CHUNKS_LOAD_START` has arrived. Vanilla forwards it to a
    /// `levelLoadTracker` when one exists (a loading-screen progress hook).
    pub fn chunks_load_started(&self) -> bool {
        self.chunks_load_started
    }

    /// Drain the translation keys of system messages the client generated.
    ///
    /// Keys rather than text: vanilla builds a `Component.translatable` and
    /// resolves it against the loaded language at render time, and the
    /// language lives in the app, not here.
    pub fn take_system_messages(&mut self) -> Vec<&'static str> {
        std::mem::take(&mut self.pending_system_messages)
    }

    /// Reset to the join-time defaults, for a dimension change or a reconnect.
    pub fn clear(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rewo_proto::writer::PacketWriter;

    /// A byte the decoder cannot consume: the body is a fixed five, so if the
    /// reader is not sitting exactly on this afterwards it read the wrong
    /// number of bytes.
    const SENTINEL: u8 = 0xA7;

    fn body(id: u8, param: f32) -> Vec<u8> {
        let mut w = PacketWriter::default();
        w.u8(id);
        w.f32(param);
        w.into_bytes()
    }

    fn body_with_sentinel(id: u8, param: f32) -> Vec<u8> {
        let mut v = body(id, param);
        v.push(SENTINEL);
        v
    }

    /// The body is fixed-length, so "consumes exactly five" is provable
    /// through the real [`decode`] with no second copy of the walk: five bytes
    /// must succeed (so it reads no more than five) and four must fail (so it
    /// reads no fewer). Re-reading `u8` + `f32` here instead would test a
    /// reimplementation of the decoder rather than the decoder — the drift
    /// M62 found, in miniature.
    #[test]
    fn the_decoder_consumes_exactly_the_five_byte_body() {
        for id in 0..ids::COUNT {
            let full = body(id, 1.0);
            assert_eq!(full.len(), BODY_LEN);
            assert!(decode(&full).is_ok(), "id {id}: five bytes must suffice");
            assert!(
                decode(&full[..BODY_LEN - 1]).is_err(),
                "id {id}: four bytes must not, or the decoder reads too few"
            );
            // A trailing byte must not change the answer: the decoder stops
            // at five and never looks at what follows.
            assert_eq!(
                decode(&body_with_sentinel(id, 1.0)).unwrap(),
                decode(&full).unwrap(),
                "id {id}: a trailing byte changed the decode"
            );
        }
    }

    #[test]
    fn a_short_body_is_an_error_not_a_silent_default() {
        assert!(decode(&[]).is_err());
        assert!(decode(&[ids::WIN_GAME]).is_err());
        // Four bytes: the id plus three of the float's four.
        assert!(decode(&[ids::WIN_GAME, 0, 0, 0]).is_err());
        // Five is the whole body.
        assert!(decode(&body(ids::WIN_GAME, 0.0)).is_ok());
    }

    #[test]
    fn the_param_is_a_big_endian_f32_not_a_var_int_or_a_fixed_point_int() {
        // 101.0f32 is 0x42CA0000. Reading the four bytes any other way
        // (little-endian, or an int scaled by 8 the way the sound packets
        // encode a coordinate) lands on a different demo hint or none.
        let bytes = body(ids::DEMO_EVENT, 101.0);
        assert_eq!(&bytes[1..], &[0x42, 0xCA, 0x00, 0x00]);
        assert_eq!(
            decode(&bytes).unwrap().unwrap().kind,
            GameEvent::DemoEvent(Some(DemoHint::Movement))
        );
        assert_eq!(decode(&bytes).unwrap().unwrap().param, 101.0);
    }

    #[test]
    fn the_type_id_is_an_unsigned_byte_so_high_ids_do_not_wrap_negative() {
        // readUnsignedByte: 0xFF is 255, not -1. A signed read would make it
        // -1, and a signed classify could then alias onto a real id.
        let bytes = body(0xFF, 0.0);
        assert_eq!(bytes[0], 0xFF);
        assert_eq!(decode(&bytes).unwrap(), None);
        let mut r = PacketReader::new(&bytes);
        assert_eq!(r.u8().unwrap(), 255u8);
    }

    #[test]
    fn every_registered_id_classifies_and_anything_past_the_table_is_a_silent_no_op() {
        for id in 0..ids::COUNT {
            assert!(
                GameEvent::classify(id, 0.0).is_some(),
                "id {id} is registered and must classify"
            );
        }
        // Vanilla's `TYPES.get(id)` returns null and every `==` is false, so
        // the packet is accepted and does nothing. Not an error.
        for id in [ids::COUNT, 20, 100, 255] {
            assert_eq!(GameEvent::classify(id, 0.0), None, "id {id}");
            assert_eq!(decode(&body(id, 0.0)).unwrap(), None, "id {id}");
        }
    }

    #[test]
    fn the_weather_ids_agree_with_the_weather_states_own_table() {
        use rewo_world::weather::WeatherState;
        // Two tables, one contract. If either moves, this fails.
        assert_eq!(ids::START_RAINING, WeatherState::START_RAINING);
        assert_eq!(ids::STOP_RAINING, WeatherState::STOP_RAINING);
        assert_eq!(ids::RAIN_LEVEL_CHANGE, WeatherState::RAIN_LEVEL_CHANGE);
        assert_eq!(ids::THUNDER_LEVEL_CHANGE, WeatherState::THUNDER_LEVEL_CHANGE);
    }

    #[test]
    fn exactly_the_four_weather_events_report_as_weather() {
        let mut weather = 0;
        for id in 0..ids::COUNT {
            if GameEvent::classify(id, 0.5).unwrap().is_weather() {
                weather += 1;
            }
        }
        assert_eq!(weather, 4, "only the four level events are weather");
    }

    // ---------------------------------------------------------------
    // The rounded param — used by exactly two branches.
    // ---------------------------------------------------------------

    #[test]
    fn change_game_mode_rounds_its_param_and_is_survival_out_of_range() {
        // Mth.floor(p + 0.5F): 2.6 -> 3 is SPECTATOR, 2.4 -> 2 is ADVENTURE.
        // A truncating implementation would call both ADVENTURE.
        let m = |p: f32| match GameEvent::classify(ids::CHANGE_GAME_MODE, p).unwrap() {
            GameEvent::ChangeGameMode(m) => m,
            other => panic!("{other:?}"),
        };
        assert_eq!(m(0.0), GameMode::Survival);
        assert_eq!(m(1.0), GameMode::Creative);
        assert_eq!(m(2.0), GameMode::Adventure);
        assert_eq!(m(3.0), GameMode::Spectator);
        assert_eq!(m(2.6), GameMode::Spectator, "rounds up, does not truncate");
        assert_eq!(m(2.4), GameMode::Adventure);
        // ByIdMap.continuous(..., ZERO): out of range is SURVIVAL, never an
        // error and never a clamp to SPECTATOR.
        assert_eq!(m(4.0), GameMode::Survival);
        assert_eq!(m(99.0), GameMode::Survival);
        assert_eq!(m(-1.0), GameMode::Survival);
    }

    #[test]
    fn the_elder_guardian_curse_uses_the_rounded_param_and_only_exactly_one() {
        let curse = |p: f32| match GameEvent::classify(ids::GUARDIAN_ELDER_EFFECT, p).unwrap() {
            GameEvent::GuardianElderEffect { curse } => curse,
            other => panic!("{other:?}"),
        };
        assert!(curse(1.0));
        // Rounded, so these reach 1 too — a raw `param == 1.0` test would
        // call both of them false.
        assert!(curse(0.6));
        assert!(curse(1.4));
        assert!(!curse(0.0));
        assert!(!curse(2.0));
        assert!(!curse(1.5), "1.5 + 0.5 floors to 2, not 1");
    }

    // ---------------------------------------------------------------
    // The raw param — exact float equality, and the inversion.
    // ---------------------------------------------------------------

    #[test]
    fn immediate_respawn_is_inverted_zero_means_show_the_death_screen() {
        let show = |p: f32| match GameEvent::classify(ids::IMMEDIATE_RESPAWN, p).unwrap() {
            GameEvent::ImmediateRespawn { show_death_screen } => show_death_screen,
            other => panic!("{other:?}"),
        };
        // setShowDeathScreen(paramFloat == 0.0F). The name says "immediate
        // respawn"; the payload says "show the screen".
        assert!(show(0.0), "param 0 SHOWS the death screen");
        assert!(!show(1.0));
        // Raw, not rounded: 0.4 would round to 0 and wrongly show it.
        assert!(!show(0.4), "the raw float is compared, not the rounded int");
    }

    #[test]
    fn limited_crafting_compares_the_raw_float_against_exactly_one() {
        let on = |p: f32| match GameEvent::classify(ids::LIMITED_CRAFTING, p).unwrap() {
            GameEvent::LimitedCrafting(v) => v,
            other => panic!("{other:?}"),
        };
        assert!(on(1.0));
        assert!(!on(0.0));
        // Rounding would turn both of these on.
        assert!(!on(0.6), "the raw float is compared, not the rounded int");
        assert!(!on(1.4));
    }

    #[test]
    fn the_demo_hints_are_exact_floats_and_anything_else_does_nothing() {
        let hint = |p: f32| match GameEvent::classify(ids::DEMO_EVENT, p).unwrap() {
            GameEvent::DemoEvent(h) => h,
            other => panic!("{other:?}"),
        };
        assert_eq!(hint(0.0), Some(DemoHint::Intro));
        assert_eq!(hint(101.0), Some(DemoHint::Movement));
        assert_eq!(hint(102.0), Some(DemoHint::Jump));
        assert_eq!(hint(103.0), Some(DemoHint::Inventory));
        assert_eq!(hint(104.0), Some(DemoHint::Day6));
        // Between the constants vanilla leaves `message` null and opens no
        // screen. A nearest-match implementation would fire Movement here.
        assert_eq!(hint(100.0), None);
        assert_eq!(hint(101.5), None);
        assert_eq!(hint(105.0), None);
    }

    // ---------------------------------------------------------------
    // Sounds.
    // ---------------------------------------------------------------

    #[test]
    fn exactly_three_events_play_a_sound_and_they_carry_vanillas_constants() {
        assert_eq!(
            GameEvent::classify(ids::PLAY_ARROW_HIT_SOUND, 0.0)
                .unwrap()
                .sounds(),
            &[LocalSoundSpec {
                name: "entity.arrow.hit_player",
                source: SoundSource::Players,
                volume: 0.18,
                pitch: 0.45,
                at_eye: true,
            }]
        );
        assert_eq!(
            GameEvent::classify(ids::PUFFER_FISH_STING, 0.0)
                .unwrap()
                .sounds(),
            &[LocalSoundSpec {
                name: "entity.puffer_fish.sting",
                source: SoundSource::Neutral,
                volume: 1.0,
                pitch: 1.0,
                at_eye: false,
            }]
        );
        // Only the arrow sound plays at eye height; flattening that would
        // move it to the player's feet.
        let at_eye: Vec<u8> = (0..ids::COUNT)
            .filter(|id| {
                GameEvent::classify(*id, 1.0)
                    .unwrap()
                    .sounds()
                    .iter()
                    .any(|s| s.at_eye)
            })
            .collect();
        assert_eq!(at_eye, vec![ids::PLAY_ARROW_HIT_SOUND]);

        // Everything else is silent — including the weather four.
        let silent_at_param_1: Vec<u8> = (0..ids::COUNT)
            .filter(|id| GameEvent::classify(*id, 1.0).unwrap().sounds().is_empty())
            .collect();
        assert_eq!(silent_at_param_1.len(), 11);
        assert!(!silent_at_param_1.contains(&ids::PLAY_ARROW_HIT_SOUND));
        assert!(!silent_at_param_1.contains(&ids::PUFFER_FISH_STING));
        assert!(!silent_at_param_1.contains(&ids::GUARDIAN_ELDER_EFFECT));
    }

    #[test]
    fn the_elder_guardian_sound_is_conditional_but_its_particle_never_is() {
        // param 1 curses, so one sound; any other param is the particle alone.
        let curse = GameEvent::classify(ids::GUARDIAN_ELDER_EFFECT, 1.0).unwrap();
        let no_curse = GameEvent::classify(ids::GUARDIAN_ELDER_EFFECT, 0.0).unwrap();
        assert_eq!(curse.sounds().len(), 1);
        assert_eq!(curse.sounds()[0].name, "entity.elder_guardian.curse");
        assert_eq!(no_curse.sounds().len(), 0);
        // Both still request the particle — `addParticle` sits outside the
        // `if (param == 1)`. The particle itself has no home in Rewo (see the
        // module docs / REWO_PACKET_COVERAGE), which is why the only thing
        // distinguishing these two is the sound.
        assert!(matches!(curse, GameEvent::GuardianElderEffect { curse: true }));
        assert!(matches!(
            no_curse,
            GameEvent::GuardianElderEffect { curse: false }
        ));
    }

    // ---------------------------------------------------------------
    // ClientGameState.
    // ---------------------------------------------------------------

    #[test]
    fn the_flag_defaults_are_local_players_field_initialisers_not_false_false() {
        let s = ClientGameState::default();
        assert!(
            s.show_death_screen(),
            "LocalPlayer declares showDeathScreen = true"
        );
        assert!(!s.do_limited_crafting());
        assert_eq!(s.game_mode(), None, "not yet told is not Survival");
        assert_eq!(s.previous_game_mode(), None);
        assert!(!s.won_game());
        assert_eq!(s.demo_hint(), None);
        assert!(!s.chunks_load_started());
    }

    fn apply(state: &mut ClientGameState, id: u8, param: f32) {
        let ev = decode(&body(id, param)).unwrap().unwrap();
        state.apply(ev.kind);
    }

    #[test]
    fn a_repeated_gamemode_does_not_clobber_the_previous_one() {
        let mut s = ClientGameState::default();
        apply(&mut s, ids::CHANGE_GAME_MODE, 0.0); // survival
        assert_eq!(s.game_mode(), Some(GameMode::Survival));
        assert_eq!(s.previous_game_mode(), None);

        apply(&mut s, ids::CHANGE_GAME_MODE, 1.0); // creative
        assert_eq!(s.game_mode(), Some(GameMode::Creative));
        assert_eq!(s.previous_game_mode(), Some(GameMode::Survival));

        // setLocalMode guards the previous-mode write on the mode CHANGING.
        // Without the guard this would report Creative as its own previous.
        apply(&mut s, ids::CHANGE_GAME_MODE, 1.0);
        assert_eq!(s.game_mode(), Some(GameMode::Creative));
        assert_eq!(
            s.previous_game_mode(),
            Some(GameMode::Survival),
            "a repeat must not overwrite the previous mode"
        );
    }

    #[test]
    fn the_two_flags_round_trip_both_ways() {
        let mut s = ClientGameState::default();
        apply(&mut s, ids::IMMEDIATE_RESPAWN, 1.0);
        assert!(!s.show_death_screen());
        apply(&mut s, ids::IMMEDIATE_RESPAWN, 0.0);
        assert!(s.show_death_screen(), "must be settable back on");

        apply(&mut s, ids::LIMITED_CRAFTING, 1.0);
        assert!(s.do_limited_crafting());
        apply(&mut s, ids::LIMITED_CRAFTING, 0.0);
        assert!(!s.do_limited_crafting(), "must be settable back off");
    }

    #[test]
    fn a_demo_param_matching_nothing_leaves_an_earlier_hint_standing() {
        let mut s = ClientGameState::default();
        apply(&mut s, ids::DEMO_EVENT, 102.0);
        assert_eq!(s.demo_hint(), Some(DemoHint::Jump));
        // Vanilla does nothing at all for an unmatched param, so this must
        // not clear the hint (an unconditional assign would).
        apply(&mut s, ids::DEMO_EVENT, 55.0);
        assert_eq!(s.demo_hint(), Some(DemoHint::Jump));
    }

    #[test]
    fn no_respawn_block_queues_its_translation_key_and_the_queue_drains() {
        let mut s = ClientGameState::default();
        assert!(s.take_system_messages().is_empty());
        apply(&mut s, ids::NO_RESPAWN_BLOCK_AVAILABLE, 0.0);
        apply(&mut s, ids::NO_RESPAWN_BLOCK_AVAILABLE, 0.0);
        // The key, not English text: the language lives in the app.
        assert_eq!(
            s.take_system_messages(),
            vec![NO_RESPAWN_BLOCK_KEY, NO_RESPAWN_BLOCK_KEY]
        );
        assert!(
            s.take_system_messages().is_empty(),
            "taking must drain, not copy"
        );
    }

    #[test]
    fn win_game_and_chunks_load_start_are_recorded() {
        let mut s = ClientGameState::default();
        apply(&mut s, ids::WIN_GAME, 0.0);
        apply(&mut s, ids::LEVEL_CHUNKS_LOAD_START, 0.0);
        assert!(s.won_game());
        assert!(s.chunks_load_started());
    }

    #[test]
    fn the_weather_events_leave_the_game_state_untouched() {
        // Weather is WeatherState's; applying it here must be a no-op rather
        // than, say, a stray gamemode write from a shared param path.
        let baseline = ClientGameState::default();
        for id in [
            ids::START_RAINING,
            ids::STOP_RAINING,
            ids::RAIN_LEVEL_CHANGE,
            ids::THUNDER_LEVEL_CHANGE,
        ] {
            let mut s = ClientGameState::default();
            apply(&mut s, id, 1.0);
            assert_eq!(s, baseline, "id {id} must not touch the game state");
        }
    }

    #[test]
    fn the_system_message_queue_is_capped_and_drops_the_oldest() {
        // Nothing drains this on the headless paths, and a server can send
        // NO_RESPAWN_BLOCK_AVAILABLE on every respawn attempt.
        let mut s = ClientGameState::default();
        for _ in 0..ClientGameState::MAX_PENDING_SYSTEM_MESSAGES + 50 {
            apply(&mut s, ids::NO_RESPAWN_BLOCK_AVAILABLE, 0.0);
        }
        assert_eq!(
            s.take_system_messages().len(),
            ClientGameState::MAX_PENDING_SYSTEM_MESSAGES,
            "the queue must be bounded, not grow for the whole session"
        );
    }

    // ---------------------------------------------------------------
    // The whole-packet seam `PlaySession` delegates to.
    //
    // These exist because a mutation battery caught the session's original
    // hand-written fan-out: `PlaySession` owns a socket and has no unit
    // tests, so dropping the weather branch, the state branch, or the eye
    // height all survived the entire suite. The logic moved here; these are
    // the witnesses that make the move worth anything.
    // ---------------------------------------------------------------

    fn apply_packet(
        id: u8,
        param: f32,
    ) -> (
        rewo_world::weather::WeatherState,
        ClientGameState,
        Vec<crate::sounds::LocalSound>,
    ) {
        let mut weather = rewo_world::weather::WeatherState::default();
        let mut state = ClientGameState::default();
        // A position with three distinct, non-zero coordinates, so a dropped
        // or transposed axis cannot coincide with a right answer.
        let player = PlayerState::at(10.0, 64.0, -30.0);
        let applied = super::apply(&body(id, param), &mut weather, &mut state, &player);
        (weather, state, applied.sounds)
    }

    #[test]
    fn the_seam_applies_the_weather_half() {
        // STOP_RAINING sets the level to 1 — see WeatherState on why the
        // names read backwards. Dropping this branch leaves it at 0.
        let (weather, _, _) = apply_packet(ids::STOP_RAINING, 0.0);
        assert_eq!(weather.rain_level(), 1.0);

        let (weather, _, _) = apply_packet(ids::RAIN_LEVEL_CHANGE, 0.25);
        assert_eq!(weather.rain_level(), 0.25);

        let (weather, _, _) = apply_packet(ids::THUNDER_LEVEL_CHANGE, 1.0);
        // getThunderLevel multiplies by the rain level, which is still 0.
        assert_eq!(weather.thunder_level(), 0.0);
    }

    #[test]
    fn the_seam_applies_the_game_state_half() {
        let (_, state, _) = apply_packet(ids::CHANGE_GAME_MODE, 3.0);
        assert_eq!(state.game_mode(), Some(GameMode::Spectator));

        let (_, state, _) = apply_packet(ids::LIMITED_CRAFTING, 1.0);
        assert!(state.do_limited_crafting());
    }

    #[test]
    fn the_seam_places_a_sound_at_the_player_and_only_the_arrow_at_eye_height() {
        let (_, _, sounds) = apply_packet(ids::PUFFER_FISH_STING, 0.0);
        assert_eq!(sounds.len(), 1);
        assert_eq!(sounds[0].name, "entity.puffer_fish.sting");
        assert_eq!((sounds[0].x, sounds[0].y, sounds[0].z), (10.0, 64.0, -30.0));

        let (_, _, sounds) = apply_packet(ids::PLAY_ARROW_HIT_SOUND, 0.0);
        assert_eq!(sounds.len(), 1);
        assert_eq!(sounds[0].x, 10.0);
        assert_eq!(sounds[0].z, -30.0);
        // getEyeY(), not getY(). Vanilla's EYE_HEIGHT is 1.62.
        assert_eq!(sounds[0].y, 64.0 + rewo_world::physics::EYE_HEIGHT);
        assert_ne!(sounds[0].y, 64.0, "the arrow sound is not at the feet");
        assert_eq!(sounds[0].volume, 0.18);
        assert_eq!(sounds[0].pitch, 0.45);
    }

    #[test]
    fn the_seam_is_silent_for_the_eleven_events_that_play_nothing() {
        for id in 0..ids::COUNT {
            let (_, _, sounds) = apply_packet(id, 1.0);
            let expect = matches!(
                id,
                ids::PLAY_ARROW_HIT_SOUND | ids::PUFFER_FISH_STING | ids::GUARDIAN_ELDER_EFFECT
            );
            assert_eq!(!sounds.is_empty(), expect, "id {id}");
        }
    }

    #[test]
    fn the_seam_ignores_a_short_body_and_an_unknown_id_without_panicking() {
        let mut weather = rewo_world::weather::WeatherState::default();
        let mut state = ClientGameState::default();
        for body in [
            vec![],
            vec![ids::STOP_RAINING],
            body(ids::COUNT, 1.0),
            body(200, 1.0),
        ] {
            let applied =
                super::apply(&body, &mut weather, &mut state, &PlayerState::at(0.0, 0.0, 0.0));
            assert!(applied.sounds.is_empty());
            assert_eq!(applied.event, None, "nothing was decoded");
            assert!(!applied.was_weather());
        }
        // Nothing was applied by any of them.
        assert_eq!(weather.rain_level(), 0.0);
        assert_eq!(state, ClientGameState::default());
    }

    #[test]
    fn clear_returns_the_join_time_defaults() {
        let mut s = ClientGameState::default();
        apply(&mut s, ids::CHANGE_GAME_MODE, 1.0);
        apply(&mut s, ids::LIMITED_CRAFTING, 1.0);
        apply(&mut s, ids::IMMEDIATE_RESPAWN, 1.0);
        apply(&mut s, ids::NO_RESPAWN_BLOCK_AVAILABLE, 0.0);
        assert_ne!(s, ClientGameState::default());
        s.clear();
        assert_eq!(s, ClientGameState::default());
        assert!(s.show_death_screen(), "back to true, not false");
    }
}
