//! The three sound packets, decoded (M63).
//!
//! **Decode only.** Nothing here opens an audio device, mixes, or resamples;
//! the crate takes no audio dependency. Playing a sound needs a human to
//! listen before it can be called correct, and decoding a packet does not —
//! so the two halves ship separately, and this is the half a machine can
//! grade. What this produces is a queue of "the server asked for sound X
//! here, at this volume, with this seed", which a later playback layer drains.
//!
//! ## Where the model lives, and why here rather than `rewo-world`
//!
//! `rewo_world::particles::ParticleEvent` is the obvious analogue and it sits
//! in `rewo-world` — because `rewo-world` *simulates* particles: it ticks
//! them, moves them, ages them out. A sound has no such state on this side of
//! the wire. `ClientboundSoundPacket` is inert wire data: a reference, a
//! category, a place, and three numbers. There is nothing for `rewo-world` to
//! own, so putting the type there would add a hop through a crate that only
//! forwards it. It lives beside its decoder instead, and
//! [`crate::play::PlaySession::sound_events`] is the seam a playback layer
//! reads — exactly the shape `particle_events` already has.
//!
//! ## Ground truth (bundled 26.2 decompile, `%APPDATA%/EwoClient/rewo/26.2/
//! decompiled/`)
//!
//! - `net/minecraft/network/protocol/game/ClientboundSoundPacket.java`
//! - `net/minecraft/network/protocol/game/ClientboundSoundEntityPacket.java`
//! - `net/minecraft/network/protocol/game/ClientboundStopSoundPacket.java`
//! - `net/minecraft/sounds/SoundEvent.java` — `STREAM_CODEC =
//!   ByteBufCodecs.holder(Registries.SOUND_EVENT, DIRECT_STREAM_CODEC)`
//! - `net/minecraft/sounds/SoundSource.java` — the eleven categories, in the
//!   declaration order that *is* the wire ordinal
//! - `net/minecraft/network/codec/ByteBufCodecs.java` — `holder` and `optional`
//! - `net/minecraft/network/FriendlyByteBuf.java` — `readEnum` is
//!   `getEnumConstants()[readVarInt()]`, `readIdentifier` is a UTF string
//!
//! There is **no `custom_sound` packet in 26.2** — the datagen report's
//! clientbound-play table lists `sound` (117), `sound_entity` (116) and
//! `stop_sound` (119) and nothing else sound-shaped. `level_event` exists and
//! carries most of vanilla's block/world sounds in its id table, but it is
//! already resolved and handled by M37's particle path
//! (`crate::route_level_event`); the sound half of that table is a playback
//! concern, not a decode gap, so this module leaves it alone.

use rewo_proto::reader::PacketReader;
use rewo_proto::{ProtoError, Result};

/// `ClientboundSoundPacket.LOCATION_ACCURACY` — the fixed-point scale.
pub const LOCATION_ACCURACY: f32 = 8.0;

/// `net.minecraft.sounds.SoundSource`, in declaration order.
///
/// `FriendlyByteBuf.readEnum` indexes `getEnumConstants()` with a VarInt, so
/// the ordinal is the wire value and the order below is load-bearing. An
/// out-of-range ordinal is an `ArrayIndexOutOfBoundsException` in vanilla, so
/// it is an error here rather than a silent `MASTER`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SoundSource {
    Master,
    Music,
    Records,
    Weather,
    Blocks,
    Hostile,
    Neutral,
    Players,
    Ambient,
    Voice,
    Ui,
}

impl SoundSource {
    pub const ALL: [SoundSource; 11] = [
        SoundSource::Master,
        SoundSource::Music,
        SoundSource::Records,
        SoundSource::Weather,
        SoundSource::Blocks,
        SoundSource::Hostile,
        SoundSource::Neutral,
        SoundSource::Players,
        SoundSource::Ambient,
        SoundSource::Voice,
        SoundSource::Ui,
    ];

    pub fn from_ordinal(id: i32) -> Option<SoundSource> {
        usize::try_from(id).ok().and_then(|i| Self::ALL.get(i)).copied()
    }

    pub fn ordinal(self) -> i32 {
        Self::ALL.iter().position(|&s| s == self).unwrap_or(0) as i32
    }

    /// `SoundSource.getName()` — the string form used in `options.txt` and by
    /// the `/stopsound` command. Not on the wire; here so a future mixer can
    /// key its per-category volumes off the same names the launcher uses.
    pub fn name(self) -> &'static str {
        match self {
            SoundSource::Master => "master",
            SoundSource::Music => "music",
            SoundSource::Records => "record",
            SoundSource::Weather => "weather",
            SoundSource::Blocks => "block",
            SoundSource::Hostile => "hostile",
            SoundSource::Neutral => "neutral",
            SoundSource::Players => "player",
            SoundSource::Ambient => "ambient",
            SoundSource::Voice => "voice",
            SoundSource::Ui => "ui",
        }
    }

    /// The inverse of [`Self::name`].
    ///
    /// `rewo_data::level_event_sounds` carries its category as one of these
    /// strings rather than as this enum, because that table lives in
    /// `rewo-data` and the dependency runs `rewo-net` → `rewo-data`. This is
    /// the join, so the two never drift into two spellings of "blocks".
    pub fn from_name(name: &str) -> Option<SoundSource> {
        SoundSource::ALL.into_iter().find(|s| s.name() == name)
    }

    fn read(r: &mut PacketReader<'_>) -> Result<SoundSource> {
        let ordinal = r.varint()?;
        SoundSource::from_ordinal(ordinal).ok_or(ProtoError::LengthOutOfRange {
            what: "sound source ordinal",
            len: ordinal as i64,
            max: SoundSource::ALL.len() - 1,
        })
    }
}

/// `Holder<SoundEvent>` as `ByteBufCodecs.holder` writes it.
///
/// **The var-int is `id + 1`, and a literal `0` means an inline definition
/// follows** — not "registry id 0", which would be
/// `minecraft:entity.allay.ambient_with_item`. This is the same convention
/// `component_wire.rs` documents for `Shape::Holder`, and it is the opposite
/// of `holderRegistry`'s raw id (`spawn_info.rs`). Reading it as raw shifts
/// every sound by one *and* then reads the inline body as the next field, so
/// the rest of the packet is garbage — the failure is loud but a long way
/// from its cause.
#[derive(Clone, Debug, PartialEq)]
pub enum SoundRef {
    /// A 0-based `minecraft:sound_event` registry id (the wire value minus
    /// one). Left unresolved by the decode on purpose: `sound_event` is a
    /// built-in registry, so turning this into a name is a report lookup, not
    /// something reading the packet needs. [`SoundRef::resolve`] does it.
    Registry(i32),
    /// `SoundEvent.DIRECT_STREAM_CODEC` — a datapack sound the server defines
    /// inline, e.g. a custom `/playsound` on a resource-pack event.
    Inline {
        /// `Identifier location` — "minecraft:block.stone.break".
        name: String,
        /// `Optional<Float> fixedRange`, boolean-prefixed
        /// (`ByteBufCodecs::optional`). `None` means
        /// `SoundEvent.getRange` falls back to `16 * max(volume, 1)`.
        fixed_range: Option<f32>,
    },
}

impl SoundRef {
    pub fn read(r: &mut PacketReader<'_>) -> Result<SoundRef> {
        let raw = r.varint()?;
        if raw == 0 {
            let name = r.identifier()?;
            let fixed_range = r.option(|r| r.f32())?;
            Ok(SoundRef::Inline { name, fixed_range })
        } else {
            Ok(SoundRef::Registry(raw - 1))
        }
    }

    /// The sound's registry name — `"minecraft:block.stone.break"` — for
    /// either variant (M64).
    ///
    /// The two variants reach a name by different routes and that asymmetry is
    /// the point: a registry id is only meaningful against the built-in table
    /// this jar was generated from, while an inline definition **carries** its
    /// own identifier and needs no table at all. An inline sound may well name
    /// a resource-pack event that has no registry id anywhere, so resolving it
    /// through the table would lose it.
    ///
    /// `None` means a registry id this jar does not have — a newer server
    /// naming a newer sound. That is a real state, not a decode failure, and
    /// substituting anything for it would produce a *wrong* sound rather than
    /// a missing one.
    pub fn resolve<'a>(
        &'a self,
        registry: &'a rewo_data::sound_events::SoundEvents,
    ) -> Option<&'a str> {
        match self {
            SoundRef::Registry(id) => registry.name(*id),
            SoundRef::Inline { name, .. } => Some(name.as_str()),
        }
    }
}

/// `ClientboundSoundPacket` — a sound at a fixed world position.
#[derive(Clone, Debug, PartialEq)]
pub struct PositionedSound {
    pub sound: SoundRef,
    pub source: SoundSource,
    /// Already divided out of the packet's fixed-point form — see
    /// [`PositionedSound::read`].
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub volume: f32,
    pub pitch: f32,
    /// Seeds the client's per-sound variant pick and pitch jitter, so two
    /// clients hearing the same event hear the same one.
    pub seed: i64,
}

/// Undo the packet's fixed-point encoding, bit-for-bit as vanilla does it.
///
/// The wire carries `(int)(coord * 8.0)` and the accessor is `this.x / 8.0F`
/// — an **`int` divided by a `float`**, so Java promotes to `float`, divides,
/// and only then widens the `float` result to the `double` the getter
/// returns. Doing the division in `f64` is the plausible-but-wrong version:
/// it agrees for every coordinate near spawn and silently disagrees past
/// ±2^21 blocks, where `f32`'s 24-bit mantissa can no longer hold `coord * 8`
/// exactly. Dividing by 16 instead of 8 is the other plausible-but-wrong
/// version, and it puts every sound at half its true distance — audible as
/// wrong attenuation rather than as an error.
fn unpack_fixed(v: i32) -> f64 {
    (v as f32 / LOCATION_ACCURACY) as f64
}

impl PositionedSound {
    pub fn read(r: &mut PacketReader<'_>) -> Result<Self> {
        let sound = SoundRef::read(r)?;
        let source = SoundSource::read(r)?;
        // `readInt`, not `readVarInt` — fixed big-endian i32s, and negative
        // for anything west/below/north of the origin, which is exactly where
        // a var-int reading would blow up into a huge coordinate.
        let x = r.i32()?;
        let y = r.i32()?;
        let z = r.i32()?;
        let volume = r.f32()?;
        let pitch = r.f32()?;
        let seed = r.i64()?;
        Ok(Self {
            sound,
            source,
            x: unpack_fixed(x),
            y: unpack_fixed(y),
            z: unpack_fixed(z),
            volume,
            pitch,
            seed,
        })
    }
}

/// `ClientboundSoundEntityPacket` — a sound that follows an entity.
///
/// Identical to [`PositionedSound`] except that the position is replaced by
/// one **VarInt** entity id. The two packets are otherwise the same shape,
/// which is why they are easy to conflate; the id's encoding is the tell.
#[derive(Clone, Debug, PartialEq)]
pub struct EntitySound {
    pub sound: SoundRef,
    pub source: SoundSource,
    pub entity_id: i32,
    pub volume: f32,
    pub pitch: f32,
    pub seed: i64,
}

impl EntitySound {
    pub fn read(r: &mut PacketReader<'_>) -> Result<Self> {
        let sound = SoundRef::read(r)?;
        let source = SoundSource::read(r)?;
        let entity_id = r.varint()?;
        let volume = r.f32()?;
        let pitch = r.f32()?;
        let seed = r.i64()?;
        Ok(Self {
            sound,
            source,
            entity_id,
            volume,
            pitch,
            seed,
        })
    }
}

/// `ClientboundStopSoundPacket.HAS_SOURCE`.
pub const STOP_HAS_SOURCE: u8 = 1;
/// `ClientboundStopSoundPacket.HAS_SOUND`.
pub const STOP_HAS_SOUND: u8 = 2;

/// `ClientboundStopSoundPacket` — stop everything, one category, one sound,
/// or one sound within one category.
///
/// The body is a flags byte followed by **only the fields the flags claim**,
/// so an unconditional read of both is a desync, not a wrong value: with
/// flags `0` the packet is one byte long and reading a source would consume
/// whatever framing follows. Two further details are easy to invert:
///
/// - **The source comes first.** `HAS_SOURCE` is bit 0 and is tested first;
///   the name is bit 1. Reading name-then-source happens to work for flags
///   `1` and `2` (only one field is present) and corrupts flags `3`, which is
///   the `/stopsound <player> <source> <sound>` form.
/// - **`name` is a plain `Identifier`, not a `Holder<SoundEvent>`.** This
///   packet takes `FriendlyByteBuf`, not `RegistryFriendlyByteBuf`, so there
///   is no registry to hold against and no `id + 1` here.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StopSound {
    /// `null` in vanilla — stop across every category.
    pub source: Option<SoundSource>,
    /// `null` in vanilla — stop every sound (within `source`, if given).
    pub name: Option<String>,
}

impl StopSound {
    pub fn read(r: &mut PacketReader<'_>) -> Result<Self> {
        // `readByte()` is signed in vanilla, but every test is `& mask`, so
        // the sign never reaches a decision. Read unsigned and mask.
        let flags = r.u8()?;
        let source = if flags & STOP_HAS_SOURCE != 0 {
            Some(SoundSource::read(r)?)
        } else {
            None
        };
        let name = if flags & STOP_HAS_SOUND != 0 {
            Some(r.identifier()?)
        } else {
            None
        };
        Ok(Self { source, name })
    }

    /// True when the packet names neither a category nor a sound — vanilla's
    /// "stop absolutely everything". Distinct from a decode failure, and the
    /// one case a playback layer must not treat as a no-op.
    pub fn stops_everything(&self) -> bool {
        self.source.is_none() && self.name.is_none()
    }
}

/// What a decoded sound packet asks the client to do.
///
/// One queue for all three so the order the server sent them in survives —
/// a `stop_sound` that overtook the `sound` it was meant to cancel would
/// leave the sound playing forever.
#[derive(Clone, Debug, PartialEq)]
pub enum SoundEvent {
    At(PositionedSound),
    OnEntity(EntitySound),
    Stop(StopSound),
}

#[cfg(test)]
mod tests {
    use super::*;
    use rewo_proto::writer::PacketWriter;

    /// A byte no decoder in this module can consume: every `read` here ends
    /// on a known field, so if a body is followed by this and the reader is
    /// not sitting exactly on it, the decode read the wrong number of bytes.
    const SENTINEL: u8 = 0xA7;

    fn with_sentinel(body: &[u8]) -> Vec<u8> {
        let mut v = body.to_vec();
        v.push(SENTINEL);
        v
    }

    /// Assert the reader consumed the whole body and stopped on the sentinel.
    fn assert_consumed_exactly(r: &mut PacketReader<'_>) {
        assert_eq!(r.remaining(), 1, "decoder must stop on the sentinel byte");
        assert_eq!(r.u8().unwrap(), SENTINEL, "trailing byte is the sentinel");
        assert_eq!(r.remaining(), 0);
    }

    fn body(f: impl FnOnce(&mut PacketWriter)) -> Vec<u8> {
        let mut w = PacketWriter::default();
        f(&mut w);
        w.into_bytes()
    }

    #[test]
    fn sound_source_ordinals_are_the_declaration_order_of_the_java_enum() {
        // SoundSource.java, top to bottom. Vanilla's wire value is the
        // ordinal, so this table is the whole contract.
        let expected = [
            (0, SoundSource::Master, "master"),
            (1, SoundSource::Music, "music"),
            (2, SoundSource::Records, "record"),
            (3, SoundSource::Weather, "weather"),
            (4, SoundSource::Blocks, "block"),
            (5, SoundSource::Hostile, "hostile"),
            (6, SoundSource::Neutral, "neutral"),
            (7, SoundSource::Players, "player"),
            (8, SoundSource::Ambient, "ambient"),
            (9, SoundSource::Voice, "voice"),
            (10, SoundSource::Ui, "ui"),
        ];
        for (ordinal, src, name) in expected {
            assert_eq!(SoundSource::from_ordinal(ordinal), Some(src));
            assert_eq!(src.ordinal(), ordinal);
            assert_eq!(src.name(), name);
        }
    }

    #[test]
    fn an_out_of_range_sound_source_ordinal_is_an_error_not_a_silent_master() {
        assert_eq!(SoundSource::from_ordinal(11), None);
        assert_eq!(SoundSource::from_ordinal(-1), None);
        let bytes = with_sentinel(&body(|w| {
            w.varint(1); // holder: registry id 0
            w.varint(11); // one past UI
        }));
        let mut r = PacketReader::new(&bytes);
        assert!(PositionedSound::read(&mut r).is_err());
    }

    #[test]
    fn a_holder_var_int_of_one_is_registry_id_zero_not_one() {
        // `ByteBufCodecs.holder` decodes `byIdOrThrow(id - 1)`. Reading the
        // wire value raw would name a different sound for every event.
        let bytes = with_sentinel(&body(|w| {
            w.varint(1);
        }));
        let mut r = PacketReader::new(&bytes);
        assert_eq!(SoundRef::read(&mut r).unwrap(), SoundRef::Registry(0));
        assert_consumed_exactly(&mut r);
    }

    #[test]
    fn a_holder_var_int_of_zero_means_an_inline_definition_follows() {
        let bytes = with_sentinel(&body(|w| {
            w.varint(0); // DIRECT_HOLDER_ID
            w.string("rewo:test.sound");
            w.bool(true);
            w.f32(24.5);
        }));
        let mut r = PacketReader::new(&bytes);
        assert_eq!(
            SoundRef::read(&mut r).unwrap(),
            SoundRef::Inline {
                name: "rewo:test.sound".into(),
                fixed_range: Some(24.5),
            }
        );
        assert_consumed_exactly(&mut r);
    }

    #[test]
    fn an_inline_sound_without_a_fixed_range_reads_only_the_false_boolean() {
        let bytes = with_sentinel(&body(|w| {
            w.varint(0);
            w.string("rewo:no.range");
            w.bool(false);
        }));
        let mut r = PacketReader::new(&bytes);
        assert_eq!(
            SoundRef::read(&mut r).unwrap(),
            SoundRef::Inline {
                name: "rewo:no.range".into(),
                fixed_range: None,
            }
        );
        assert_consumed_exactly(&mut r);
    }

    /// Build a `ClientboundSoundPacket` body for a registry sound.
    fn positioned_body(reg_id: i32, src: SoundSource, x: i32, y: i32, z: i32) -> Vec<u8> {
        body(|w| {
            w.varint(reg_id + 1);
            w.varint(src.ordinal());
            w.i32(x);
            w.i32(y);
            w.i32(z);
            w.f32(0.75);
            w.f32(1.25);
            w.i64(0x0123_4567_89AB_CDEF);
        })
    }

    #[test]
    fn a_positioned_sound_decodes_every_field_and_consumes_exactly_its_body() {
        let bytes = with_sentinel(&positioned_body(345, SoundSource::Blocks, 8, 520, -12));
        let mut r = PacketReader::new(&bytes);
        let s = PositionedSound::read(&mut r).unwrap();
        assert_eq!(s.sound, SoundRef::Registry(345));
        assert_eq!(s.source, SoundSource::Blocks);
        assert_eq!(s.volume, 0.75);
        assert_eq!(s.pitch, 1.25);
        assert_eq!(s.seed, 0x0123_4567_89AB_CDEF);
        assert_consumed_exactly(&mut r);
    }

    #[test]
    fn a_positioned_sounds_coordinates_are_the_wire_ints_divided_by_eight() {
        // The server writes `(int)(coord * 8.0)`; 8 -> 1.0, 520 -> 65.0,
        // -12 -> -1.5. Dividing by 16 (the other plausible scale) would give
        // 0.5 / 32.5 / -0.75 and put every sound at half its true distance.
        let bytes = positioned_body(0, SoundSource::Master, 8, 520, -12);
        let mut r = PacketReader::new(&bytes);
        let s = PositionedSound::read(&mut r).unwrap();
        assert_eq!((s.x, s.y, s.z), (1.0, 65.0, -1.5));
    }

    #[test]
    fn a_positioned_sound_holds_negative_coordinates_as_fixed_big_endian_ints() {
        // -8000 as a var-int would be a five-byte sequence and would be read
        // as a huge positive coordinate by a signed-var-int decoder; as a
        // fixed i32 it is exactly -1000.0 blocks.
        let bytes = with_sentinel(&positioned_body(1, SoundSource::Ambient, -8000, -512, -8));
        let mut r = PacketReader::new(&bytes);
        let s = PositionedSound::read(&mut r).unwrap();
        assert_eq!((s.x, s.y, s.z), (-1000.0, -64.0, -1.0));
        assert_consumed_exactly(&mut r);
    }

    #[test]
    fn a_positioned_sound_far_from_spawn_rounds_the_way_javas_float_divide_does() {
        // `this.x / 8.0F` is an int/float divide, so the quotient is rounded
        // to f32 before it widens to double. At x = 8 * 30_000_001 the exact
        // value 30000001.0 is not representable in f32; vanilla answers
        // 30000000.0 and an f64 divide would answer 30000001.0.
        let wire = 8 * 30_000_001;
        assert_eq!(unpack_fixed(wire), 30_000_000.0);
        assert_ne!(unpack_fixed(wire), f64::from(wire) / 8.0);
    }

    #[test]
    fn an_entity_sound_reads_its_id_as_a_var_int_not_a_fixed_int() {
        // 300 is two bytes as a var-int and four as a fixed int, so the two
        // readings disagree about where `volume` starts. The sentinel is what
        // catches that rather than the id value itself.
        let bytes = with_sentinel(&body(|w| {
            w.varint(1); // registry id 0
            w.varint(SoundSource::Hostile.ordinal());
            w.varint(300);
            w.f32(1.0);
            w.f32(0.5);
            w.i64(-1);
        }));
        let mut r = PacketReader::new(&bytes);
        let s = EntitySound::read(&mut r).unwrap();
        assert_eq!(s.sound, SoundRef::Registry(0));
        assert_eq!(s.source, SoundSource::Hostile);
        assert_eq!(s.entity_id, 300);
        assert_eq!(s.volume, 1.0);
        assert_eq!(s.pitch, 0.5);
        assert_eq!(s.seed, -1);
        assert_consumed_exactly(&mut r);
    }

    #[test]
    fn an_entity_sound_carrying_an_inline_definition_still_lands_on_the_sentinel() {
        let bytes = with_sentinel(&body(|w| {
            w.varint(0);
            w.string("rewo:inline.entity");
            w.bool(false);
            w.varint(SoundSource::Records.ordinal());
            w.varint(7);
            w.f32(2.0);
            w.f32(1.0);
            w.i64(42);
        }));
        let mut r = PacketReader::new(&bytes);
        let s = EntitySound::read(&mut r).unwrap();
        assert_eq!(
            s.sound,
            SoundRef::Inline {
                name: "rewo:inline.entity".into(),
                fixed_range: None
            }
        );
        assert_eq!(s.entity_id, 7);
        assert_consumed_exactly(&mut r);
    }

    #[test]
    fn stop_sound_with_no_flags_is_a_one_byte_body_that_stops_everything() {
        let bytes = with_sentinel(&[0u8]);
        let mut r = PacketReader::new(&bytes);
        let s = StopSound::read(&mut r).unwrap();
        assert_eq!(s, StopSound::default());
        assert!(s.stops_everything());
        assert_consumed_exactly(&mut r);
    }

    #[test]
    fn stop_sound_flag_one_reads_a_source_and_no_name() {
        let bytes = with_sentinel(&body(|w| {
            w.u8(STOP_HAS_SOURCE);
            w.varint(SoundSource::Music.ordinal());
        }));
        let mut r = PacketReader::new(&bytes);
        let s = StopSound::read(&mut r).unwrap();
        assert_eq!(s.source, Some(SoundSource::Music));
        assert_eq!(s.name, None);
        assert!(!s.stops_everything());
        assert_consumed_exactly(&mut r);
    }

    #[test]
    fn stop_sound_flag_two_reads_a_name_and_no_source() {
        let bytes = with_sentinel(&body(|w| {
            w.u8(STOP_HAS_SOUND);
            w.string("minecraft:block.stone.break");
        }));
        let mut r = PacketReader::new(&bytes);
        let s = StopSound::read(&mut r).unwrap();
        assert_eq!(s.source, None);
        assert_eq!(s.name.as_deref(), Some("minecraft:block.stone.break"));
        assert_consumed_exactly(&mut r);
    }

    #[test]
    fn stop_sound_flag_three_reads_the_source_before_the_name() {
        // The ordering witness. `Weather` is ordinal 3, and a name-first
        // reading would take that 0x03 as a three-character string length and
        // then read the string's own length byte as a source ordinal.
        let bytes = with_sentinel(&body(|w| {
            w.u8(STOP_HAS_SOURCE | STOP_HAS_SOUND);
            w.varint(SoundSource::Weather.ordinal());
            w.string("minecraft:weather.rain");
        }));
        let mut r = PacketReader::new(&bytes);
        let s = StopSound::read(&mut r).unwrap();
        assert_eq!(s.source, Some(SoundSource::Weather));
        assert_eq!(s.name.as_deref(), Some("minecraft:weather.rain"));
        assert_consumed_exactly(&mut r);
    }

    #[test]
    fn stop_sound_ignores_flag_bits_vanilla_never_sets() {
        // `write` only ever emits 0..3, but the reader's tests are `& 1` and
        // `& 2`, so a high bit must not change what is read.
        let bytes = with_sentinel(&body(|w| {
            w.u8(0xFC | STOP_HAS_SOURCE);
            w.varint(SoundSource::Ui.ordinal());
        }));
        let mut r = PacketReader::new(&bytes);
        let s = StopSound::read(&mut r).unwrap();
        assert_eq!(s.source, Some(SoundSource::Ui));
        assert_eq!(s.name, None);
        assert_consumed_exactly(&mut r);
    }

    /// A two-entry `minecraft:sound_event` registry, written to disk and read
    /// back through the production `SoundEvents::load`, so the resolve tests
    /// exercise the real table rather than a hand-built map. Ids 0 and 1 are
    /// the real 26.2 pair, so the fixture also stands as a reminder that
    /// registry id 0 is a *real* sound and not an "absent" sentinel.
    ///
    /// **The path has to be unique per call.** Three tests call this, cargo
    /// runs them on separate threads, and a shared filename means one test's
    /// `fs::write` truncates the file another is part-way through reading —
    /// observed as `EOF while parsing a value at line 1 column 0` in roughly
    /// one run in six. The counter is what makes `cargo test -p rewo-net`
    /// deterministic; the process id keeps two concurrent cargo invocations
    /// (a mutation harness, say) from colliding with each other.
    fn sound_registry() -> rewo_data::sound_events::SoundEvents {
        use std::sync::atomic::{AtomicU32, Ordering};
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let dir = std::env::temp_dir().join("rewo-net-sound-resolve");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join(format!(
            "registries-{}-{}.json",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::write(
            &p,
            r#"{"minecraft:sound_event":{"entries":{
                 "minecraft:entity.allay.ambient_with_item":{"protocol_id":0},
                 "minecraft:block.stone.break":{"protocol_id":1596}}}}"#,
        )
        .unwrap();
        rewo_data::sound_events::SoundEvents::load(&p).unwrap()
    }

    #[test]
    fn a_registry_sound_ref_resolves_through_the_report_table() {
        let reg = sound_registry();
        // Wire value 1597 → `Registry(1596)` → the name. Getting the holder's
        // `id + 1` wrong here would resolve a neighbouring sound, which is why
        // the assertion starts from the wire bytes rather than the variant.
        let bytes = body(|w| {
            w.varint(1597);
        });
        let mut r = PacketReader::new(&bytes);
        let s = SoundRef::read(&mut r).unwrap();
        assert_eq!(s, SoundRef::Registry(1596));
        assert_eq!(s.resolve(&reg), Some("minecraft:block.stone.break"));
        assert_eq!(
            SoundRef::Registry(0).resolve(&reg),
            Some("minecraft:entity.allay.ambient_with_item")
        );
    }

    #[test]
    fn an_unknown_registry_id_resolves_to_none_not_a_substitute_sound() {
        let reg = sound_registry();
        assert_eq!(SoundRef::Registry(9999).resolve(&reg), None);
    }

    #[test]
    fn an_inline_sound_resolves_to_its_own_identifier_without_the_table() {
        // A resource-pack event has no registry id at all, so consulting the
        // table for it would lose the name entirely.
        let reg = sound_registry();
        let s = SoundRef::Inline {
            name: "rewo:custom.event".into(),
            fixed_range: Some(8.0),
        };
        assert_eq!(s.resolve(&reg), Some("rewo:custom.event"));
        assert_eq!(reg.id_of("rewo:custom.event"), None);
    }

    /// `from_name` is the exact inverse of `name` — the property, not a
    /// spot-check, because the pair exists so two crates cannot disagree.
    #[test]
    fn from_name_is_the_exact_inverse_of_name() {
        for s in SoundSource::ALL {
            assert_eq!(SoundSource::from_name(s.name()), Some(s));
        }
        // The two spellings a plausible implementation would accept and
        // vanilla does not: `getName()` is singular for these, even though
        // the constants are `BLOCKS` and `PLAYERS`.
        assert_eq!(SoundSource::from_name("blocks"), None);
        assert_eq!(SoundSource::from_name("players"), None);
        assert_eq!(SoundSource::from_name(""), None);
        // And the match is **case-sensitive**. Nothing above forces that —
        // a case-insensitive lookup passes every other assertion here — but
        // both places vanilla turns a name back into a category are exact:
        // `options.txt` keys are `"soundCategory_" + getName()` compared as
        // strings, and `/stopsound`'s categories are Brigadier literals.
        // Accepting `"BLOCK"` would read an options file real Minecraft
        // rejects.
        assert_eq!(SoundSource::from_name("BLOCK"), None);
        assert_eq!(SoundSource::from_name("Block"), None);
        assert_eq!(SoundSource::from_name("Master"), None);
    }

    /// Every category `rewo_data::level_event_sounds` names resolves here.
    /// The table stores a string because it cannot see this enum; this is the
    /// witness that the string is nonetheless a real `SoundSource`.
    #[test]
    fn every_level_event_sound_category_resolves_to_a_source() {
        use rewo_data::level_event_sounds::{DERIVED, SOUNDS};
        for s in SOUNDS {
            assert!(
                SoundSource::from_name(s.source).is_some(),
                "level event {}: {} is not a SoundSource",
                s.id,
                s.source
            );
        }
        for d in DERIVED {
            assert!(
                SoundSource::from_name(d.source).is_some(),
                "level event {}: {} is not a SoundSource",
                d.id,
                d.source
            );
        }
        // Non-vacuous: the table really does use more than one category.
        let mut seen: Vec<&str> = SOUNDS.iter().map(|s| s.source).collect();
        seen.sort_unstable();
        seen.dedup();
        assert!(seen.len() >= 4, "only {seen:?} appear");
    }

    #[test]
    fn a_truncated_body_is_an_error_rather_than_a_partial_sound() {
        // Flags claim a name; nothing follows.
        let mut r = PacketReader::new(&[STOP_HAS_SOUND]);
        assert!(StopSound::read(&mut r).is_err());
        // A positioned sound cut off mid-position.
        let short = body(|w| {
            w.varint(1);
            w.varint(0);
            w.i32(8);
        });
        let mut r = PacketReader::new(&short);
        assert!(PositionedSound::read(&mut r).is_err());
    }
}
