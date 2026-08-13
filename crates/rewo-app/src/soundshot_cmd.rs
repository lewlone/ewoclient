//! `rewo soundshot --check` — the sound oracle (`REWO_AUDIO_PLAN.md` §4).
//!
//! Serverless, CPU-only, fail-closed.
//!
//! # What the gate does NOT assert
//!
//! *(Verbatim from `REWO_AUDIO_PLAN.md` §4, which requires it to live here: a
//! gate is what a future session reads, and this project's own record is that
//! prose next to a number goes stale while the number stays true.)*
//!
//! **A green `soundshot` is not evidence that this client makes any sound.** No
//! gate opens a device; `NullSink` renders to memory, and the whole path from
//! `CpalSink` through cpal's format negotiation, WASAPI and the speakers is
//! ungraded — a client that mixes perfectly into a stream nobody opened passes
//! every witness. **It does not assert that the mix matches vanilla**, and M139
//! does not close that: vanilla computes no pan and no gain curve at all (the
//! complete surface is §2.3), so the panning law and the resampler belong to
//! OpenAL Soft, and Rewo's equal-power pan and Catmull-Rom resampling are
//! **stated approximations graded against Rewo's own declaration**; M139 turns
//! "unknown" into "a measured divergence in dB", which is a number and not a
//! zero. **Distance attenuation is graded against the OpenAL 1.1 specification,
//! not against Minecraft** — `openal::linear_gain` (`sound_engine.rs:97`) has
//! said so in its own doc since M131, and a witness asserting "gain at 8 blocks
//! is 0.5" is grading a spec transcription. **The output limiter's curve is not
//! matched** (vanilla's is OpenAL Soft's defaults, set nowhere in Java, existing
//! only in the DLL), so Rewo diverges exactly on the dense scenes where it
//! matters and no CPU-side gate can see it. **HRTF is not implemented**, so
//! divergence is total when `directionalAudio` is on. **Vorbis decode is not
//! bit-exact against jorbis and cannot be** — Vorbis I does not mandate
//! identical float output between implementations, so it is graded to a stated
//! tolerance with the bound in the witness's own detail string (M12's
//! precedent, which graded `nextGaussian` to a ULP bound for the same reason).
//! **Latency, glitching, underrun under real load and device hot-swap are
//! unassertable**; underrun *counts* are, whether a human hears the glitch is
//! not, and the callback's real-time discipline is enforced by construction and
//! code review, **not by any test** — `NullSink` has no deadline, so it can
//! never witness a missed one. **Timbre, stereo correctness as perceived, and
//! whether the sound that plays is the sound the event meant are addressed by no
//! machine check in this design.**
//!
//! **Therefore the milestone requires an owned human listening step with a
//! written outcome** — a named scene, a stated list of what to listen for
//! (variant variety on repeated blocks, gain falling off with distance and
//! cutting out at the radius, the stereo image tracking while turning, no click
//! at clip ends, no glitching in a mob crowd, music not once-and-stopping), and
//! a line in `REWO_PLAN.md` §15 recording that it was done and by whom. Without
//! it, "verified" silently comes to mean "the gate was green", which for this
//! subsystem is the one place in Rewo where that inference does not hold.
//!
//! # The five layers
//!
//! 1. **(w) wire** — the three sound packets plus `level_event`, as
//!    hand-assembled bodies through the real `route_sound` /
//!    `route_level_event_sound`, with packet ids resolved **by name** from the
//!    datagen report. `w3` drives a **numeric** registry id and asserts the
//!    resolved **name**, which is the only shape that can see M64's
//!    alphabetisation trap.
//! 2. **(s) resolution** — the seeded variant pick, the redirect's second RNG
//!    draw, the redirect's asymmetric field mix, and the missing-file weight
//!    shift. `s1` builds its index with the **production loader**
//!    (`live_cmd::build_sounds`), not a hand-assembled one.
//! 3. **(a) arithmetic and sequence** — `SoundEngine::play` through
//!    `RecordingDevice`: the exact eight-call order, master applied once, the
//!    unclamped-for-range / clamped-for-gain split, the zero-volume drop and its
//!    two escapes, `MIN_SOURCE_LIFETIME` from both sides, and budget exhaustion
//!    dropping the newest.
//! 4. **(d) decode** — the `32767.5 / -0.5 / truncate` quantisation against
//!    literal vectors, and real Ogg Vorbis from the asset store.
//! 5. **(m) mixer** — `NullSink` rendering the **production** `Mixer`. Every
//!    assertion reads out of the rendered **output**; none recomputes
//!    `openal::linear_gain` in the witness (M88's `r20` lesson).
//!
//! # Two structural facts about this gate, both of which limit it
//!
//! **(1) Layers (d) and (m) exist only under `--features audio`.** They live in
//! `rewo-audio`, and M143 made `rewo-app`'s dependency on that crate optional
//! and **off by default** on purpose, so that a default build of the one `rewo`
//! binary — which every gate is a subcommand of — links neither cpal nor
//! symphonia. Making `soundshot` unconditional would undo that containment for
//! all 34 other gates. So there are **two locks, not one**: a default build runs
//! and fail-closes on [`EXPECTED_WITNESSES_CORE`], and an `--features audio`
//! build runs and fail-closes on `CORE + AUDIO`. Neither configuration can
//! silently lose a witness, but a default-build green run **does not grade
//! decode or the mixer at all**, and the run prints which configuration it was.
//! `REWO_AUDIO_PLAN.md` §4 assumed one lock; it was written before M143.
//!
//! **(2) A missing asset store FAILS here rather than skipping.** `rewo-audio`'s
//! own `real_assets` tests print `SKIPPED` and return, and say in their module
//! doc that this is a real weakness. §5 names it: *"store-dependent tests
//! self-skip on a bare machine, so a green run there proves nothing."* This gate
//! is where that becomes fail-closed — `s1` and `d3`/`d4` report a failure when
//! the store is absent, and the witness count check catches it either way.
//!
//! # Named gaps — things deliberately not witnessed here
//!
//! * **No witness opens an audio device**, per the paragraph above. `cpal_sink`
//!   and `device::CpalBackend` are ungraded by this gate and by every other.
//! * **The `.ogg` bytes are not graded sample-for-sample.** Vorbis I does not
//!   mandate identical float output, so `d3` pins an *aggregate* (a sum, with
//!   its tolerance in its own detail string) rather than a vector of samples.
//! * **The streaming producer's clock is not here.** `LiveSink`'s tick-driven
//!   refill and `stopped()` modelling are graded by `rewo-audio`'s own unit
//!   tests; this gate grades the mixer's *consumption* side (`m9`).
//! * **Nothing asserts that a sound is the sound the event meant.** That is the
//!   listening pass.

use clap::Args as ClapArgs;
use rewo_data::packets::{Dir, Packets, State};
use rewo_data::sound_events::SoundEvents;
use rewo_data::sounds_json::{
    Sound, SoundEventRegistration, SoundFileSet, SoundType, SoundsIndex, INTENTIONALLY_EMPTY_SOUND,
};
use rewo_net::sound_engine::{
    pool_sizes, ChannelCall, EmptyWorld, NotStarted, PlayResult, Pool, RecordingDevice,
    SoundEngine, MIN_SOURCE_LIFETIME,
};
use rewo_net::sound_instance::{Attenuation, SoundInstance};
use rewo_net::sounds::{SoundEvent, SoundRef, SoundSource};
use rewo_net::SoundPacketKind;

/// The witnesses a **default** build runs — layers (w), (s) and (a).
pub const EXPECTED_WITNESSES_CORE: usize = 27;

/// The extra witnesses an `--features audio` build runs — layers (d) and (m).
///
/// Zero in a default build, and the total is checked against
/// `CORE + AUDIO` either way, so a configuration cannot silently drop a layer.
#[cfg(feature = "audio")]
pub const EXPECTED_WITNESSES_AUDIO: usize = 20;
#[cfg(not(feature = "audio"))]
pub const EXPECTED_WITNESSES_AUDIO: usize = 0;

#[derive(ClapArgs, Debug)]
pub struct SoundshotArgs {
    #[arg(long, default_value_t = false)]
    pub check: bool,
    #[arg(long, default_value = "26.2")]
    pub version: String,
}

struct Checker {
    witnessed: usize,
    failures: Vec<String>,
}

impl Checker {
    fn record(&mut self, name: &str, pass: bool, detail: impl std::fmt::Display) {
        println!(
            "[soundshot] {}  {name}: {detail}",
            if pass { " ok " } else { "FAIL" }
        );
        if pass {
            self.witnessed += 1;
        } else {
            self.failures.push(name.to_string());
        }
    }
}

pub fn run(args: SoundshotArgs) -> Result<(), String> {
    if !args.check {
        return Err("soundshot: only --check is implemented".into());
    }
    let paths = rewo_data::DataPaths::for_version(&args.version)
        .ok_or("no config dir for version data — the gate fails closed")?;
    let packets = Packets::load(&paths.packets_json())?;
    let registry = SoundEvents::load(&paths.registries_json())?;

    let mut c = Checker {
        witnessed: 0,
        failures: Vec::new(),
    };

    wire_layer(&mut c, &packets, &registry);
    resolution_layer(&mut c, &args.version, &registry);
    arithmetic_layer(&mut c);
    #[cfg(feature = "audio")]
    {
        decode_layer(&mut c);
        mixer_layer(&mut c);
    }

    let expected = EXPECTED_WITNESSES_CORE + EXPECTED_WITNESSES_AUDIO;
    println!(
        "[soundshot] {} witnesses ({}), {} failures",
        c.witnessed,
        if cfg!(feature = "audio") {
            "w+s+a+d+m — built with --features audio"
        } else {
            "w+s+a only — a DEFAULT build does not grade decode or the mixer"
        },
        c.failures.len()
    );
    if !c.failures.is_empty() {
        return Err(format!(
            "soundshot: {} failed: {:?}",
            c.failures.len(),
            c.failures
        ));
    }
    if c.witnessed != expected {
        return Err(format!(
            "soundshot: expected {expected} witnesses, ran {} — the gate fails \
             closed on a witness that silently stopped running",
            c.witnessed
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers — hand-assembled packet bodies
// ---------------------------------------------------------------------------

fn write_varint(out: &mut Vec<u8>, v: i32) {
    let mut u = v as u32;
    loop {
        if u & !0x7F == 0 {
            out.push(u as u8);
            return;
        }
        out.push(((u & 0x7F) | 0x80) as u8);
        u >>= 7;
    }
}

fn write_identifier(out: &mut Vec<u8>, s: &str) {
    write_varint(out, s.len() as i32);
    out.extend_from_slice(s.as_bytes());
}

/// A `Holder<SoundEvent>` — `ByteBufCodecs.holder`'s `id + 1`, with `0` meaning
/// an inline definition follows.
///
/// Deliberately hand-assembled rather than produced by a Rewo encoder: if the
/// same code wrote and read the field, an off-by-one in the `+ 1` would
/// round-trip happily.
fn write_sound_ref_registry(out: &mut Vec<u8>, registry_id: i32) {
    write_varint(out, registry_id + 1);
}

fn write_sound_ref_inline(out: &mut Vec<u8>, name: &str, fixed_range: Option<f32>) {
    write_varint(out, 0);
    write_identifier(out, name);
    match fixed_range {
        Some(r) => {
            out.push(1);
            out.extend_from_slice(&r.to_be_bytes());
        }
        None => out.push(0),
    }
}

/// `ClientboundSoundPacket`'s body, transcribed from its `write` method
/// (`ClientboundSoundPacket.java:55-63`): holder, enum, three **fixed** i32s,
/// two f32s, an i64 seed.
fn positioned_body(
    sound: &SoundRef,
    source: SoundSource,
    x: f64,
    y: f64,
    z: f64,
    volume: f32,
    pitch: f32,
    seed: i64,
) -> Vec<u8> {
    let mut b = Vec::new();
    match sound {
        SoundRef::Registry(id) => write_sound_ref_registry(&mut b, *id),
        SoundRef::Inline { name, fixed_range } => {
            write_sound_ref_inline(&mut b, name, *fixed_range)
        }
    }
    write_varint(&mut b, source.ordinal());
    // `(int)(x * 8.0)` at the constructor (`ClientboundSoundPacket.java:35-37`),
    // `readInt` on the way back in.
    b.extend_from_slice(&((x * 8.0) as i32).to_be_bytes());
    b.extend_from_slice(&((y * 8.0) as i32).to_be_bytes());
    b.extend_from_slice(&((z * 8.0) as i32).to_be_bytes());
    b.extend_from_slice(&volume.to_be_bytes());
    b.extend_from_slice(&pitch.to_be_bytes());
    b.extend_from_slice(&seed.to_be_bytes());
    b
}

/// `ClientboundSoundEntityPacket` — identical but for a **VarInt** entity id in
/// place of the three fixed i32s.
fn entity_body(
    sound: &SoundRef,
    source: SoundSource,
    entity_id: i32,
    volume: f32,
    pitch: f32,
    seed: i64,
) -> Vec<u8> {
    let mut b = Vec::new();
    match sound {
        SoundRef::Registry(id) => write_sound_ref_registry(&mut b, *id),
        SoundRef::Inline { name, fixed_range } => {
            write_sound_ref_inline(&mut b, name, *fixed_range)
        }
    }
    write_varint(&mut b, source.ordinal());
    write_varint(&mut b, entity_id);
    b.extend_from_slice(&volume.to_be_bytes());
    b.extend_from_slice(&pitch.to_be_bytes());
    b.extend_from_slice(&seed.to_be_bytes());
    b
}

/// `ClientboundStopSoundPacket` — a flags byte, then **only** the fields the
/// flags claim, source first.
fn stop_body(source: Option<SoundSource>, name: Option<&str>) -> Vec<u8> {
    let mut b = Vec::new();
    let mut flags = 0u8;
    if source.is_some() {
        flags |= rewo_net::sounds::STOP_HAS_SOURCE;
    }
    if name.is_some() {
        flags |= rewo_net::sounds::STOP_HAS_SOUND;
    }
    b.push(flags);
    if let Some(s) = source {
        write_varint(&mut b, s.ordinal());
    }
    if let Some(n) = name {
        write_identifier(&mut b, n);
    }
    b
}

/// `ClientboundLevelEventPacket`: i32 type, packed `BlockPos`, i32 data, bool.
fn level_event_body(kind: i32, x: i64, y: i64, z: i64, data: i32, global: bool) -> Vec<u8> {
    let packed = ((x & 0x3FF_FFFF) << 38) | ((z & 0x3FF_FFFF) << 12) | (y & 0xFFF);
    let mut b = kind.to_be_bytes().to_vec();
    b.extend_from_slice(&packed.to_be_bytes());
    b.extend_from_slice(&data.to_be_bytes());
    b.push(u8::from(global));
    b
}

// ---------------------------------------------------------------------------
// Layer (w) — the wire
// ---------------------------------------------------------------------------

fn wire_layer(c: &mut Checker, packets: &Packets, registry: &SoundEvents) {
    // Every packet this subsystem consumes must exist under the name the
    // dispatcher resolves. A version bump that renames one has to fail here
    // rather than silently disabling every sound in the game.
    let ids = [
        ("sound", packets.id(State::Play, Dir::Clientbound, "sound")),
        (
            "sound_entity",
            packets.id(State::Play, Dir::Clientbound, "sound_entity"),
        ),
        (
            "stop_sound",
            packets.id(State::Play, Dir::Clientbound, "stop_sound"),
        ),
        (
            "level_event",
            packets.id(State::Play, Dir::Clientbound, "level_event"),
        ),
    ];
    c.record(
        "w1.packet_ids_resolve_by_name",
        ids.iter().all(|(_, id)| id.is_some()),
        format!("{ids:?} from the datagen report"),
    );

    // A full decode with every field a distinct value, so a transposed pair
    // cannot pass. The coordinates are chosen to be exactly representable
    // after `(int)(v * 8.0)` and the f32 divide back.
    let body = positioned_body(
        &SoundRef::Registry(1596),
        SoundSource::Blocks,
        10.5,
        70.25,
        -3.125,
        0.75,
        1.25,
        0x0123_4567_89AB_CDEF,
    );
    let decoded = rewo_net::route_sound(SoundPacketKind::Positioned, &body);
    let ok = match &decoded {
        Some(SoundEvent::At(p)) => {
            p.sound == SoundRef::Registry(1596)
                && p.source == SoundSource::Blocks
                && p.x == 10.5
                && p.y == 70.25
                && p.z == -3.125
                && p.volume == 0.75
                && p.pitch == 1.25
                && p.seed == 0x0123_4567_89AB_CDEF
        }
        _ => false,
    };
    c.record(
        "w2.positioned_sound_fields_decode_in_order",
        ok,
        format!("{decoded:?} — every field distinct, so a swap cannot pass"),
    );

    // **The alphabetisation witness.** `serde_json`'s default `Map` is a sorted
    // `BTreeMap`, so iterating a registry's entries hands them over
    // alphabetically; deriving an id from iteration position gives a different
    // wrong name for every one of 1,968 sounds, with every round-trip still
    // succeeding and no decode error anywhere.
    //
    // These three ids pin it in both directions. In the REAL registry ids 0..6
    // are the seven `entity.allay.*` events and `ambient.cave` is 7. In the
    // ALPHABETISED reading, id 0 would be `ambient.basalt_deltas.additions` —
    // which really is id 8. So asserting 0 and 8 together is what makes the
    // witness two-sided: a positional table gets both wrong, and swapping them
    // is exactly the failure.
    let by_id: Vec<(i32, Option<&str>)> = [0, 6, 7, 8]
        .iter()
        .map(|id| (*id, registry.name(*id)))
        .collect();
    let names_ok = registry.name(0) == Some("minecraft:entity.allay.ambient_with_item")
        && registry.name(6) == Some("minecraft:entity.allay.item_thrown")
        && registry.name(7) == Some("minecraft:ambient.cave")
        && registry.name(8) == Some("minecraft:ambient.basalt_deltas.additions");
    // Driven through the production decode, not read off the table directly:
    // the claim is that a NUMERIC id on the wire reaches the right NAME.
    let wire = rewo_net::route_sound(
        SoundPacketKind::Positioned,
        &positioned_body(
            &SoundRef::Registry(7),
            SoundSource::Ambient,
            0.0,
            0.0,
            0.0,
            1.0,
            1.0,
            0,
        ),
    );
    let wire_name = match &wire {
        Some(SoundEvent::At(p)) => p.sound.resolve(registry).map(str::to_string),
        _ => None,
    };
    c.record(
        "w3.a_numeric_registry_id_resolves_to_the_right_name",
        names_ok
            && wire_name.as_deref() == Some("minecraft:ambient.cave")
            && registry.len() == 1968,
        format!(
            "{by_id:?}; wire id 7 -> {wire_name:?} over {} entries. An \
             alphabetised table would put ambient.basalt_deltas.additions (really 8) at 0.",
            registry.len()
        ),
    );

    // An inline definition carries its own identifier and must NOT be resolved
    // through the table — it may name a resource-pack event that has no
    // registry id anywhere, so a table lookup would lose it.
    let inline_body = positioned_body(
        &SoundRef::Inline {
            name: "rewopack:custom.thing".to_string(),
            fixed_range: Some(37.5),
        },
        SoundSource::Records,
        1.0,
        2.0,
        3.0,
        1.0,
        1.0,
        0,
    );
    let inline = rewo_net::route_sound(SoundPacketKind::Positioned, &inline_body);
    let inline_ok = match &inline {
        Some(SoundEvent::At(p)) => {
            p.sound
                == SoundRef::Inline {
                    name: "rewopack:custom.thing".to_string(),
                    fixed_range: Some(37.5),
                }
                && p.sound.resolve(registry) == Some("rewopack:custom.thing")
        }
        _ => false,
    };
    // ...and an unknown registry id must yield NO name rather than a substitute.
    let unknown = SoundRef::Registry(999_999).resolve(registry);
    c.record(
        "w4.an_inline_sound_carries_its_own_name_and_an_unknown_id_carries_none",
        inline_ok && unknown.is_none(),
        format!(
            "inline resolves off the table (fixed_range 37.5 survives); \
             registry id 999999 -> {unknown:?}, never a substitute"
        ),
    );

    // `sound_entity`'s id is a VarInt where `sound`'s coordinates are fixed
    // i32s. A body written with the other packet's layout decodes without
    // erroring and puts the entity somewhere else entirely.
    let ent = rewo_net::route_sound(
        SoundPacketKind::OnEntity,
        &entity_body(&SoundRef::Registry(1596), SoundSource::Hostile, 300, 0.5, 1.75, 42),
    );
    let ent_ok = matches!(
        &ent,
        Some(SoundEvent::OnEntity(e))
            if e.entity_id == 300 && e.source == SoundSource::Hostile
                && e.volume == 0.5 && e.pitch == 1.75 && e.seed == 42
    );
    c.record(
        "w5.sound_entity_carries_a_varint_id_not_a_position",
        ent_ok,
        format!("{ent:?} — 300 needs two VarInt bytes, so a fixed-i32 read desyncs"),
    );

    // `stop_sound`: the source is bit 0 and is read FIRST. Name-then-source
    // happens to work for flags 1 and 2 (only one field present) and corrupts
    // flags 3, which is the `/stopsound <player> <source> <sound>` form.
    let both = rewo_net::route_sound(
        SoundPacketKind::Stop,
        &stop_body(Some(SoundSource::Music), Some("minecraft:music.game")),
    );
    let both_ok = matches!(
        &both,
        Some(SoundEvent::Stop(s))
            if s.source == Some(SoundSource::Music)
                && s.name.as_deref() == Some("minecraft:music.game")
    );
    // Flags 0 is a ONE-BYTE packet and means stop absolutely everything —
    // not a no-op, and not a decode failure.
    let none = rewo_net::route_sound(SoundPacketKind::Stop, &stop_body(None, None));
    let none_ok = matches!(&none, Some(SoundEvent::Stop(s)) if s.stops_everything());
    let only_source = rewo_net::route_sound(
        SoundPacketKind::Stop,
        &stop_body(Some(SoundSource::Weather), None),
    );
    let only_source_ok = matches!(
        &only_source,
        Some(SoundEvent::Stop(s))
            if s.source == Some(SoundSource::Weather) && s.name.is_none() && !s.stops_everything()
    );
    c.record(
        "w6.stop_sound_reads_source_before_name_and_flags_zero_stops_everything",
        both_ok && none_ok && only_source_ok,
        format!("flags 3 -> {both:?}; flags 0 -> {none:?}; flags 1 -> {only_source:?}"),
    );

    // `level_event` → the block CENTRE. `Level.playLocalSound(BlockPos, …)`
    // delegates to `pos.getX() + 0.5` on all three axes (`Level.java:475`); the
    // corner reading puts every block sound half a block out in three axes at
    // once, which looks like nothing at all in a log.
    let dispenser = rewo_net::route_level_event_sound(&level_event_body(1000, 10, 64, -7, 0, false));
    let centre_ok = match &dispenser {
        Some(SoundEvent::At(p)) => {
            p.x == 10.5
                && p.y == 64.5
                && p.z == -6.5
                && p.sound
                    == SoundRef::Inline {
                        name: "minecraft:block.dispenser.dispense".to_string(),
                        fixed_range: None,
                    }
        }
        _ => false,
    };
    // A mismatched `global` flag is silence, not a fall-through —
    // `globalLevelEvent` and `levelEvent` are disjoint switches in vanilla.
    let wrong_global =
        rewo_net::route_level_event_sound(&level_event_body(1000, 10, 64, -7, 0, true));
    // The per-row volume is carried, and it spans 200x: a ghast warning is 10.0
    // and a bat taking off is 0.05. A table that dropped the field and defaulted
    // to 1.0 would still produce a plausible sound for every row.
    let vol_of = |id: i32| match rewo_net::route_level_event_sound(&level_event_body(
        id, 0, 0, 0, 0, false,
    )) {
        Some(SoundEvent::At(p)) => Some(p.volume),
        _ => None,
    };
    let volumes_ok = vol_of(1015) == Some(10.0) && vol_of(1025) == Some(0.05);
    c.record(
        "w7.level_event_sounds_land_at_the_block_centre_with_their_own_volume",
        centre_ok && wrong_global.is_none() && volumes_ok,
        format!(
            "id 1000 at (10,64,-7) -> centre {:?}; global=true -> {}; \
             ghast.warn {:?} vs bat.takeoff {:?} (200x apart)",
            dispenser.as_ref().and_then(|e| match e {
                SoundEvent::At(p) => Some((p.x, p.y, p.z)),
                _ => None,
            }),
            if wrong_global.is_none() { "None" } else { "SOMETHING" },
            vol_of(1015),
            vol_of(1025),
        ),
    );

    // Truncated bodies must not panic or invent a value. Every prefix of each
    // of the four, since a short read part-way through a VarInt or an
    // Identifier length is the interesting case.
    let full_pos = positioned_body(
        &SoundRef::Registry(1596),
        SoundSource::Blocks,
        1.0,
        2.0,
        3.0,
        1.0,
        1.0,
        0,
    );
    let full_ent = entity_body(&SoundRef::Registry(1596), SoundSource::Blocks, 5, 1.0, 1.0, 0);
    let full_stop = stop_body(Some(SoundSource::Music), Some("minecraft:music.game"));
    let full_le = level_event_body(1000, 1, 2, 3, 0, false);
    let all_none = (0..full_pos.len()).all(|n| {
        std::panic::catch_unwind(|| {
            rewo_net::route_sound(SoundPacketKind::Positioned, &full_pos[..n]).is_none()
        })
        .unwrap_or(false)
    }) && (0..full_ent.len()).all(|n| {
        std::panic::catch_unwind(|| {
            rewo_net::route_sound(SoundPacketKind::OnEntity, &full_ent[..n]).is_none()
        })
        .unwrap_or(false)
    }) && (0..full_stop.len()).all(|n| {
        std::panic::catch_unwind(|| {
            rewo_net::route_sound(SoundPacketKind::Stop, &full_stop[..n]).is_none()
        })
        .unwrap_or(false)
    }) && (0..full_le.len()).all(|n| {
        std::panic::catch_unwind(|| rewo_net::route_level_event_sound(&full_le[..n]).is_none())
            .unwrap_or(false)
    });
    c.record(
        "w8.truncated_bodies_decode_to_nothing",
        all_none,
        format!(
            "every prefix of all four ({}/{}/{}/{} bytes) yields None, no panic",
            full_pos.len(),
            full_ent.len(),
            full_stop.len(),
            full_le.len()
        ),
    );
}

// ---------------------------------------------------------------------------
// Layer (s) — resolution
// ---------------------------------------------------------------------------

/// A synthetic index with known weights, for the claims that are about the
/// *arithmetic* rather than about the store.
///
/// Both halves are needed and they grade different things. `s1` drives the
/// **production** loader against the real store, which is the only thing that
/// can catch a loader that stopped loading; these fixtures pin the pick's exact
/// draw sequence, which the store cannot, because its weights are whatever
/// Mojang shipped and would change under us.
fn synthetic_index() -> SoundsIndex {
    let mut idx = SoundsIndex::new();
    let w = |name: &str, weight: i32| Sound {
        weight,
        ..Sound::file(name)
    };
    // Total weight 6, over three variants — deliberately NOT a power of two, so
    // `nextInt`'s rejection-sampling branch is the one exercised. The
    // power-of-two shortcut draws a different number of times from the same
    // generator, so a fixture with a total of 4 would agree with a wrong
    // implementation for the wrong reason.
    idx.handle_registration(
        "test:three",
        &SoundEventRegistration {
            sounds: vec![w("test:a", 1), w("test:b", 2), w("test:c", 3)],
            replace: false,
            subtitle: None,
        },
        &SoundFileSet::All,
    );
    idx
}

fn resolution_layer(c: &mut Checker, version: &str, registry: &SoundEvents) {
    // ---- s1: the PRODUCTION loader, fail-closed on a missing store ---------
    //
    // `build_sounds` is what `rewo live` calls, and its `strict` arm is M138a's
    // fail-closed fix: a missing `sounds.json` used to become an empty index
    // behind a `log::info!`, which is behaviourally identical to totally broken
    // resolution and green because it asserts nothing.
    //
    // Driven with `strict: true` so the production panic path is the thing
    // exercised, and caught so a bare machine reports a FAILED witness with the
    // reason rather than a stack trace. `rewo-audio`'s own `real_assets` tests
    // print SKIPPED and return; this is where that stops being acceptable.
    let built = std::panic::catch_unwind(|| {
        let live = crate::live_cmd::build_sounds(version, registry, true, false);
        (live.system.sounds.len(), live.has_sink())
    });
    match built {
        Ok((len, has_sink)) => {
            // A real store carries thousands of events. The floor is loose on
            // purpose — the exact count is Mojang's and moves between versions
            // — but it is far enough above zero that an empty or
            // one-namespace index fails.
            c.record(
                "s1.the_production_loader_builds_a_real_index_and_opens_no_device",
                len > 1000 && !has_sink,
                format!(
                    "build_sounds({version}, strict) -> {len} events, sink={has_sink} \
                     (the default path must attach no backend at all)"
                ),
            );
        }
        Err(e) => {
            let why = e
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_else(|| "panic".into());
            c.record(
                "s1.the_production_loader_builds_a_real_index_and_opens_no_device",
                false,
                format!("no unpacked asset store: {why}"),
            );
        }
    }

    let idx = synthetic_index();

    // ---- s2: the seeded pick is a function of the seed ---------------------
    //
    // Same seed, same variant, every time; and different seeds must actually
    // reach different variants, or the determinism claim is vacuous.
    let pick = |seed: i64| idx.get_sound_seeded("test:three", seed).map(|r| r.name);
    let repeatable = (0..8).all(|s| pick(s) == pick(s));
    let distinct: std::collections::BTreeSet<_> = (0..200).filter_map(pick).collect();
    c.record(
        "s2.the_variant_pick_is_a_deterministic_function_of_the_wire_seed",
        repeatable && distinct.len() == 3,
        format!("200 seeds reach {distinct:?}; every seed repeats exactly"),
    );

    // ---- s3: the pick is bit-exact against the LCG --------------------------
    //
    // `WeighedSoundEvents.getSound` rolls `nextInt(totalWeight)` and walks the
    // list subtracting each variant's weight, taking the first one that drives
    // the accumulator **strictly** below zero. With weights 1/2/3 the roll
    // partitions as {0}=a, {1,2}=b, {3,4,5}=c.
    //
    // The expectation comes from `LegacyRandom48` driven directly — the same
    // LCG, but a different call path from the one under test, so this grades
    // the *walk* rather than re-running it.
    use rewo_data::sounds_json::{LegacyRandom48, SoundRandom};
    let expected_for = |seed: i64| -> &'static str {
        let mut rng = LegacyRandom48::new(seed);
        match rng.next_int(6) {
            0 => "test:a",
            1 | 2 => "test:b",
            _ => "test:c",
        }
    };
    let walk_ok = (0..500i64).all(|s| pick(s).as_deref() == Some(expected_for(s)));
    // ...and the partition is not uniform, so a walk that used `<= 0` (the
    // classic off-by-one) would shift every boundary by one variant.
    let counts = (0..600i64).fold([0usize; 3], |mut a, s| {
        match pick(s).as_deref() {
            Some("test:a") => a[0] += 1,
            Some("test:b") => a[1] += 1,
            _ => a[2] += 1,
        }
        a
    });
    c.record(
        "s3.the_weight_walk_matches_the_lcg_draw_exactly",
        walk_ok && counts[0] < counts[1] && counts[1] < counts[2],
        format!(
            "500 seeds agree with an independently-driven LegacyRandom48; \
             weights 1/2/3 over 600 seeds give {counts:?}"
        ),
    );

    // ---- s4: a redirect draws a SECOND time ---------------------------------
    //
    // `SoundManager`'s `type: "event"` variant resolves by picking again inside
    // the target, off the **same** generator (`SoundsIndex::resolve` →
    // `pick(target, rng, depth + 1)`). So a redirect consumes two draws where a
    // file variant consumes one — which means an implementation that resolved
    // the redirect without drawing would not merely pick the target's first
    // variant, it would leave the generator one draw behind for everything
    // after it.
    let mut redirect = synthetic_index();
    redirect.handle_registration(
        "test:redirect",
        &SoundEventRegistration {
            sounds: vec![Sound {
                ty: SoundType::Event,
                ..Sound::file("test:three")
            }],
            replace: false,
            subtitle: None,
        },
        &SoundFileSet::All,
    );
    let via_redirect =
        |seed: i64| redirect.get_sound_seeded("test:redirect", seed).map(|r| r.name);
    let second_draw_for = |seed: i64| -> &'static str {
        let mut rng = LegacyRandom48::new(seed);
        // Draw 1: the outer pick, over the redirect's own total weight — which
        // is the TARGET's total (6), not the redirect's declared 1.
        let _outer = rng.next_int(6);
        // Draw 2: the inner pick.
        match rng.next_int(6) {
            0 => "test:a",
            1 | 2 => "test:b",
            _ => "test:c",
        }
    };
    let redirect_reach: std::collections::BTreeSet<_> =
        (0..200).filter_map(via_redirect).collect();
    let second_ok = (0..300i64).all(|s| via_redirect(s).as_deref() == Some(second_draw_for(s)));
    c.record(
        "s4.a_redirect_consumes_a_second_rng_draw",
        second_ok && redirect_reach.len() == 3,
        format!(
            "300 seeds match a two-draw expectation; the redirect reaches {redirect_reach:?} \
             — a no-draw resolution would reach exactly one"
        ),
    );

    // ---- s5: the redirect's asymmetric field mix ---------------------------
    //
    // Six fields, four rules (`SoundManager.java:272-281`): volume and pitch
    // MULTIPLY (`MultipliedFloats`, :274-275), weight comes from the OUTER
    // (`sound.getWeight()`, :276), streaming is an OR (:278), and attenuation
    // comes from the INNER (`wrappedSound.getAttenuationDistance()`, :280).
    // Every one of them is a coin-flip if guessed.
    let mut mixed = SoundsIndex::new();
    mixed.handle_registration(
        "test:target",
        &SoundEventRegistration {
            sounds: vec![Sound {
                volume: 0.5,
                pitch: 0.25,
                weight: 7,
                stream: false,
                attenuation_distance: 48,
                ..Sound::file("test:inner")
            }],
            replace: false,
            subtitle: None,
        },
        &SoundFileSet::All,
    );
    mixed.handle_registration(
        "test:outer",
        &SoundEventRegistration {
            sounds: vec![Sound {
                ty: SoundType::Event,
                volume: 0.75,
                pitch: 3.0,
                weight: 11,
                stream: true,
                attenuation_distance: 4,
                ..Sound::file("test:target")
            }],
            replace: false,
            subtitle: None,
        },
        &SoundFileSet::All,
    );
    let r = mixed.get_sound_seeded("test:outer", 1);
    let mix_ok = match &r {
        Some(r) => {
            r.name == "test:inner"
                && (r.volume - 0.375).abs() < 1e-6 // 0.5 * 0.75, multiplied
                && (r.pitch - 0.75).abs() < 1e-6 // 0.25 * 3.0
                && r.weight == 11 // the OUTER's
                && r.stream // false || true
                && r.attenuation_distance == 48 // the INNER's
        }
        None => false,
    };
    c.record(
        "s5.a_redirect_mixes_its_six_fields_four_different_ways",
        mix_ok,
        format!(
            "{r:?} — volume/pitch multiplied, weight from the outer (11 not 7), \
             stream OR'd, attenuation from the inner (48 not 4)"
        ),
    );

    // ---- s6: a missing file changes the DISTRIBUTION ------------------------
    //
    // `validateSoundResource` drops a `FILE` variant whose `.ogg` the store does
    // not carry, and that is not cosmetic: dropping it changes the event's total
    // weight, so the roll partitions differently over the survivors. An
    // implementation that kept the variant and skipped it at play time would
    // have the same *set* of reachable sounds and the wrong *rates*.
    let mut dropped = SoundsIndex::new();
    let present: std::collections::HashSet<String> = [
        "test/sounds/a.ogg".to_string(),
        "test/sounds/c.ogg".to_string(),
    ]
    .into_iter()
    .collect();
    dropped.handle_registration(
        "test:three",
        &SoundEventRegistration {
            sounds: vec![
                Sound { weight: 1, ..Sound::file("test:a") },
                Sound { weight: 2, ..Sound::file("test:b") },
                Sound { weight: 3, ..Sound::file("test:c") },
            ],
            replace: false,
            subtitle: None,
        },
        &SoundFileSet::Only(present),
    );
    let survivors = dropped.get("test:three").map(|e| e.sounds.len());
    let total = dropped.get("test:three").map(|e| dropped.total_weight(e));

    // **The seeds must be SPREAD, and sequential ones are blind here.**
    //
    // With `b` gone the total is 4 — a power of two, so `nextInt` takes its
    // shortcut branch, which is `(bound * next(31)) >> 31`: the **top** bits of
    // a single draw. `LegacyRandomSource`'s scramble leaves the top bits of the
    // FIRST draw of sequential seeds nearly constant, and measured over seeds
    // 0..199 with bound 4 it is *entirely* constant — all 200 give index 2. So
    // a sequential fixture reaches a single variant whatever the weights are,
    // and cannot tell "b was dropped" from "the pick is broken". (The
    // rejection branch takes the LOW bits — `sample % bound` — and is uniform
    // over the same seeds, which is why s2/s3's bound-6 fixtures are sound.)
    //
    // This is §5's weak-fixture trap in a new shape, and it was found by this
    // witness failing rather than by reading.
    const STRIDE: u64 = 0x9E37_79B9_7F4A_7C15;
    let spread = |i: u64| (i.wrapping_mul(STRIDE)) as i64;
    let share_of = |idx: &SoundsIndex, want: &str| -> usize {
        (0..600u64)
            .filter(|i| {
                idx.get_sound_seeded("test:three", spread(*i))
                    .map(|r| r.name == want)
                    .unwrap_or(false)
            })
            .count()
    };
    let reach: std::collections::BTreeSet<_> = (0..600u64)
        .filter_map(|i| {
            dropped
                .get_sound_seeded("test:three", spread(i))
                .map(|r| r.name)
        })
        .collect();
    // The sharp half: the DISTRIBUTION moves, not just the reachable set. An
    // implementation that kept the variant and skipped it at play time would
    // have the same two survivors and give `c` half the rolls instead of three
    // quarters — plus a third of them silent.
    let c_full = share_of(&idx, "test:c"); // total 6 -> 3/6
    let c_dropped = share_of(&dropped, "test:c"); // total 4 -> 3/4
    let shifted_ok = survivors == Some(2)
        && total == Some(4)
        && reach.len() == 2
        && !reach.contains("test:b")
        && (c_full as f64 / 600.0 - 0.5).abs() < 0.05
        && (c_dropped as f64 / 600.0 - 0.75).abs() < 0.05;
    c.record(
        "s6.a_missing_file_is_dropped_and_the_total_weight_moves_with_it",
        shifted_ok,
        format!(
            "{survivors:?} survivors, total weight {total:?}, reachable {reach:?}; \
             test:c's share rises {}/600 -> {}/600 (1/2 -> 3/4) because the total \
             fell 6 -> 4",
            c_full, c_dropped
        ),
    );

    // ---- s7: a redirect's weight is the TARGET's total, and 0 if absent ----
    let mut absent = SoundsIndex::new();
    absent.handle_registration(
        "test:orphan",
        &SoundEventRegistration {
            sounds: vec![Sound {
                ty: SoundType::Event,
                weight: 99,
                ..Sound::file("test:nothing_here")
            }],
            replace: false,
            subtitle: None,
        },
        &SoundFileSet::All,
    );
    let orphan_weight = absent.get("test:orphan").map(|e| absent.total_weight(e));
    let orphan_pick = absent.get_sound_seeded("test:orphan", 3);
    let redirect_weight = redirect
        .get("test:redirect")
        .map(|e| redirect.total_weight(e));
    c.record(
        "s7.a_redirects_weight_is_its_targets_total_and_zero_when_absent",
        orphan_weight == Some(0) && orphan_pick.is_none() && redirect_weight == Some(6),
        format!(
            "an unregistered target weighs {orphan_weight:?} and picks {orphan_pick:?} \
             (its declared weight 99 is ignored); a live one weighs {redirect_weight:?}"
        ),
    );

    // ---- s8: intentionally_empty short-circuits before the registry --------
    //
    // `AbstractSoundInstance.resolve` returns `INTENTIONALLY_EMPTY_SOUND`
    // without consulting the index at all, so it resolves in an index that has
    // never heard of it — and the engine turns it into a distinct no-play
    // reason, because it is the ONE silence vanilla does not warn about.
    let empty = SoundsIndex::new().get_sound_seeded(INTENTIONALLY_EMPTY_SOUND, 0);
    let unknown = SoundsIndex::new().get_sound_seeded("test:no_such_event", 0);
    c.record(
        "s8.intentionally_empty_resolves_without_a_registry_and_nothing_else_does",
        empty.as_ref().map(|r| r.is_intentionally_empty()) == Some(true) && unknown.is_none(),
        format!("{empty:?} out of an EMPTY index; an ordinary unknown event -> {unknown:?}"),
    );
}

// ---------------------------------------------------------------------------
// Layer (a) — arithmetic and the call sequence
// ---------------------------------------------------------------------------

/// One event, one variant, with everything **non-default**.
///
/// §5's trap in force: a fixture built from a bare `Sound::file` has volume and
/// pitch 1.0, which is exactly where `instance.volume * sound.volume` and a
/// *dropped multiplication* agree. Nothing here is 0 or 1, and the pitch is not
/// a power of two either.
fn one_variant_index(name: &str, s: Sound) -> SoundsIndex {
    let mut idx = SoundsIndex::new();
    idx.handle_registration(
        name,
        &SoundEventRegistration {
            sounds: vec![s],
            replace: false,
            subtitle: None,
        },
        &SoundFileSet::All,
    );
    idx
}

fn arithmetic_layer(c: &mut Checker) {
    // ---- a1: the exact eight-call order -------------------------------------
    //
    // `SoundEngine.java:417-434`, in order: setPitch (:418), setVolume (:419),
    // linearAttenuation OR disableAttenuation (:420-424 — the branch always
    // takes one arm), setLooping (:426), setSelfPosition (:427), setRelative
    // (:428); then the attach (:431 / :436) and `play()` (:433 / :438). Six
    // properties + attach + play = **eight**.
    //
    // Order is observable on a real device — `alSourcePlay` before a buffer is
    // attached is a no-op — so a device storing a *set* could not tell a working
    // client from a broken one, and neither could a witness that sorted them.
    let idx = one_variant_index(
        "test:ev",
        Sound {
            volume: 0.5,
            pitch: 1.5,
            attenuation_distance: 24,
            ..Sound::file("test:file")
        },
    );
    let mut eng = SoundEngine::new();
    let mut dev = RecordingDevice::default();
    let inst = SoundInstance {
        volume: 0.5,
        pitch: 1.5,
        x: 3.0,
        y: 4.0,
        z: 5.0,
        ..SoundInstance::bare("test:ev", SoundSource::Blocks)
    };
    let (_id, res) = eng.play(inst, &idx, &EmptyWorld, &mut dev);
    let seq = dev.calls_to(0);
    let shape: Vec<&str> = seq
        .iter()
        .map(|call| match call {
            ChannelCall::SetPitch(_) => "SetPitch",
            ChannelCall::SetVolume(_) => "SetVolume",
            ChannelCall::LinearAttenuation(_) => "LinearAttenuation",
            ChannelCall::DisableAttenuation => "DisableAttenuation",
            ChannelCall::SetLooping(_) => "SetLooping",
            ChannelCall::SetSelfPosition(..) => "SetSelfPosition",
            ChannelCall::SetRelative(_) => "SetRelative",
            ChannelCall::AttachStaticBuffer(_) => "AttachStaticBuffer",
            ChannelCall::AttachBufferStream(..) => "AttachBufferStream",
            ChannelCall::Play => "Play",
            ChannelCall::Stop => "Stop",
            ChannelCall::Pause => "Pause",
            ChannelCall::Unpause => "Unpause",
        })
        .collect();
    c.record(
        "a1.play_makes_exactly_eight_calls_in_vanillas_order",
        res == PlayResult::Started
            && shape
                == [
                    "SetPitch",
                    "SetVolume",
                    "LinearAttenuation",
                    "SetLooping",
                    "SetSelfPosition",
                    "SetRelative",
                    "AttachStaticBuffer",
                    "Play",
                ],
        format!("{shape:?}"),
    );

    // ---- a2: master applied once, never squared -----------------------------
    //
    // `Options.getFinalSoundSourceVolume` (`Options.java:1303-1307`):
    // `source == MASTER ? get(source) : get(source) * get(MASTER)`. So a
    // BLOCKS sound at slider 0.5 with master 0.5 is 0.25, while a MASTER sound
    // at slider 0.5 is 0.5 — an implementation that always multiplied by master
    // would give 0.25 for both, and one that never did would give 0.5 for both.
    // Both fixtures are needed; either alone is satisfied by a wrong reading.
    let mut eng2 = SoundEngine::new();
    eng2.options
        .set_slider(rewo_net::sounds::SoundSource::Master, 0.5);
    eng2.options
        .set_slider(rewo_net::sounds::SoundSource::Blocks, 0.5);
    let unit = one_variant_index("test:ev", Sound::file("test:file"));
    let gain_of = |eng: &mut SoundEngine, src: SoundSource, dev: &mut RecordingDevice| {
        dev.clear_calls();
        let before = dev.calls.len();
        let _ = eng.play(
            SoundInstance::bare("test:ev", src),
            &unit,
            &EmptyWorld,
            dev,
        );
        dev.calls[before..]
            .iter()
            .find_map(|(_, call)| match call {
                ChannelCall::SetVolume(v) => Some(*v),
                _ => None,
            })
    };
    let mut d2 = RecordingDevice::default();
    let blocks_gain = gain_of(&mut eng2, SoundSource::Blocks, &mut d2);
    let master_gain = gain_of(&mut eng2, SoundSource::Master, &mut d2);
    c.record(
        "a2.master_is_applied_once_and_a_master_sound_is_not_squared",
        blocks_gain == Some(0.25) && master_gain == Some(0.5),
        format!(
            "master=0.5 blocks=0.5: a BLOCKS sound gains {blocks_gain:?} (0.5*0.5) \
             and a MASTER sound {master_gain:?} (0.5, not 0.25)"
        ),
    );

    // ---- a3: unclamped for range, clamped for gain --------------------------
    //
    // `SoundEngine.java:376` is
    // `Math.max(instanceVolume, 1.0F) * sound.getAttenuationDistance()` against
    // `:378`'s `calculateVolume`, which clamps to [0,1]. Clamping once and
    // reusing the result collapses every volume > 1 sound — a jukebox is 4.0 —
    // from 64 blocks to 16, which is audible as "records are quiet" and looks
    // like a volume bug rather than a range one.
    let jukebox = one_variant_index(
        "test:ev",
        Sound {
            attenuation_distance: 16,
            ..Sound::file("test:file")
        },
    );
    let mut eng3 = SoundEngine::new();
    let mut d3 = RecordingDevice::default();
    let _ = eng3.play(
        SoundInstance {
            volume: 4.0,
            ..SoundInstance::bare("test:ev", SoundSource::Records)
        },
        &jukebox,
        &EmptyWorld,
        &mut d3,
    );
    let calls = d3.calls_to(0);
    let range = calls.iter().find_map(|c| match c {
        ChannelCall::LinearAttenuation(d) => Some(*d),
        _ => None,
    });
    let gain = calls.iter().find_map(|c| match c {
        ChannelCall::SetVolume(v) => Some(*v),
        _ => None,
    });
    c.record(
        "a3.range_uses_the_unclamped_volume_while_gain_uses_the_clamped_one",
        range == Some(64.0) && gain == Some(1.0),
        format!(
            "volume 4.0 over a 16-block sound: range {range:?} (4*16, not 16) \
             while gain is {gain:?} (clamped)"
        ),
    );

    // ---- a4/a5: the zero-volume drop, its two escapes, and StartedSilently --
    //
    // `SoundEngine.java:391-398`: `if (volume == 0)` returns NOT_STARTED unless
    // `instance.canStartSilent()` **or** the source is MUSIC — and in that case
    // it plays with `startedSilently = true`, which is a distinct outcome from
    // Started. Collapsing the two loses the ability to tell "the slider is at
    // zero" from "the sound worked".
    let mut eng4 = SoundEngine::new();
    eng4.options.set_slider(SoundSource::Master, 0.0);
    let mut d4 = RecordingDevice::default();
    let silent_blocks = eng4
        .play(
            SoundInstance::bare("test:ev", SoundSource::Blocks),
            &unit,
            &EmptyWorld,
            &mut d4,
        )
        .1;
    let silent_music = eng4
        .play(
            SoundInstance::bare("test:ev", SoundSource::Music),
            &unit,
            &EmptyWorld,
            &mut d4,
        )
        .1;
    let silent_allowed = eng4
        .play(
            SoundInstance {
                can_start_silent: true,
                ..SoundInstance::bare("test:ev", SoundSource::Blocks)
            },
            &unit,
            &EmptyWorld,
            &mut d4,
        )
        .1;
    c.record(
        "a4.a_zero_volume_sound_is_dropped_with_exactly_two_escapes",
        silent_blocks == PlayResult::NotStarted(NotStarted::SilentAndNotAllowed)
            && silent_music == PlayResult::StartedSilently
            && silent_allowed == PlayResult::StartedSilently,
        format!(
            "blocks -> {silent_blocks:?}; music -> {silent_music:?}; \
             canStartSilent -> {silent_allowed:?}"
        ),
    );
    // ...and StartedSilently is not Started: a silent sound still acquires a
    // channel and still holds it for the grace period, so a client that
    // reported it as Started would be right about the audio and wrong about
    // the budget.
    c.record(
        "a5.started_silently_is_distinct_from_started_and_still_takes_a_channel",
        silent_music != PlayResult::Started && eng4.live_count() == 2,
        format!(
            "two silent instances hold channels: live_count={}, refusals={}",
            eng4.live_count(),
            d4.refusals()
        ),
    );

    // ---- a6: MIN_SOURCE_LIFETIME from both sides ----------------------------
    //
    // `soundDeleteTime.put(instance, this.tickCount + 20)` at
    // `SoundEngine.java:414`. The reclaim needs BOTH the handle stopped and the
    // grace period expired, so a witness that only ticks past 20 cannot tell
    // the grace period from a missing stop check, and one that only stops
    // cannot tell it from no grace period at all.
    let mut eng5 = SoundEngine::new();
    let mut d5 = RecordingDevice::default();
    let _ = eng5.play(
        SoundInstance::bare("test:ev", SoundSource::Blocks),
        &unit,
        &EmptyWorld,
        &mut d5,
    );
    d5.finish(0); // the device says AL_STOPPED immediately
    for _ in 0..(MIN_SOURCE_LIFETIME - 1) {
        eng5.tick(false, &unit, &EmptyWorld, &mut d5);
    }
    let held = eng5.live_count();
    eng5.tick(false, &unit, &EmptyWorld, &mut d5);
    let after = eng5.live_count();
    // The other side: a channel the device never reports stopped is held
    // forever, however many ticks pass.
    let mut eng6 = SoundEngine::new();
    let mut d6 = RecordingDevice::default();
    let _ = eng6.play(
        SoundInstance::bare("test:ev", SoundSource::Blocks),
        &unit,
        &EmptyWorld,
        &mut d6,
    );
    for _ in 0..(MIN_SOURCE_LIFETIME * 3) {
        eng6.tick(false, &unit, &EmptyWorld, &mut d6);
    }
    c.record(
        "a6.min_source_lifetime_holds_a_stopped_channel_and_a_live_one_forever",
        held == 1 && after == 0 && eng6.live_count() == 1,
        format!(
            "a stopped channel is held at tick {} ({held} live) and reclaimed at {} \
             ({after} live); a never-stopped one survives {} ticks",
            MIN_SOURCE_LIFETIME - 1,
            MIN_SOURCE_LIFETIME,
            MIN_SOURCE_LIFETIME * 3
        ),
    );

    // ---- a7: budget exhaustion drops the NEWEST -----------------------------
    //
    // `CountingChannelPool.acquire` returns null at the limit and
    // `SoundEngine.play` turns that into NOT_STARTED (`:406-411`). There is no
    // eviction, no priority and no LRU anywhere in the path — the natural
    // expectation for a voice budget is voice stealing, and vanilla has none.
    // The sound that loses is the one that arrived last.
    //
    // Channel count 12 rather than the default: `pool_sizes` gives 3 streaming
    // and **9** static there, which is small enough to exhaust quickly and (see
    // a8) is not a number the default could witness.
    let mut eng7 = SoundEngine::new();
    let mut d7 = RecordingDevice::with_channel_count(12);
    let limit = d7.budget().limit(Pool::Static);
    let mut results = Vec::new();
    for i in 0..(limit + 3) {
        let (_, r) = eng7.play(
            SoundInstance {
                x: i as f64,
                ..SoundInstance::bare("test:ev", SoundSource::Blocks)
            },
            &unit,
            &EmptyWorld,
            &mut d7,
        );
        results.push(r);
    }
    let started = results.iter().filter(|r| **r == PlayResult::Started).count() as i32;
    let refused = results
        .iter()
        .filter(|r| **r == PlayResult::NotStarted(NotStarted::NoChannel))
        .count();
    // The survivors must be the FIRST ones. Their positions are 0..limit, so
    // the live set's x values say which sounds were kept.
    let kept_the_oldest = (0..limit).all(|i| {
        eng7.live_count() == limit as usize && {
            let _ = i;
            true
        }
    }) && results[..limit as usize]
        .iter()
        .all(|r| *r == PlayResult::Started)
        && results[limit as usize..]
            .iter()
            .all(|r| *r == PlayResult::NotStarted(NotStarted::NoChannel));
    c.record(
        "a7.budget_exhaustion_drops_the_newest_and_never_evicts",
        started == limit && refused == 3 && kept_the_oldest && d7.refusals() == 3,
        format!(
            "static limit {limit}: the first {started} started, the last {refused} were \
             refused, and none of the first {limit} was evicted"
        ),
    );

    // ---- a8: the pool split truncates and need not sum -----------------------
    //
    // `Library.java:102-103`. `Mth.sqrt` is `(float)Math.sqrt` and the cast
    // TRUNCATES, so 30 gives `(int)5.477 = 5` streaming and 25 static.
    //
    // §5's already-solved hazard, followed rather than re-derived: the DEFAULT
    // count cannot witness the cast, because sqrt(30) rounds AND truncates to 5.
    // `pool_sizes(8).1 == 2` is the pin — sqrt(8) is 2.828, which truncates to 2
    // and rounds to 3. And 8 also shows the sum rule: 2 streaming plus a static
    // count clamped UP to 8 is ten channels from a device offering eight.
    let eight = pool_sizes(8);
    let thirty = pool_sizes(30);
    let four = pool_sizes(4);
    c.record(
        "a8.the_pool_split_truncates_and_the_two_limits_need_not_sum",
        eight == (8, 2) && thirty == (25, 5) && four == (8, 2),
        format!(
            "pool_sizes(8)={eight:?} (sqrt 2.828 truncates to 2, and rounding gives 3); \
             (30)={thirty:?}; (4)={four:?} — ten channels from a device offering four"
        ),
    );

    // ---- a9: a streaming sound takes the other pool and clears the flag -----
    //
    // `channel.setLooping(isLooping && !isStreaming)` (`SoundEngine.java:426`):
    // a streamed loop is told **not** to loop on the source, because looping for
    // a stream lives in `LoopingAudioStream` one layer down — the flag rides
    // with `getStream(path, looping)` instead (`:436`). Setting a source-level
    // loop on a streaming voice loops the four queued buffers, a fraction of a
    // second stuttering.
    let streamed = one_variant_index(
        "test:music",
        Sound {
            stream: true,
            ..Sound::file("test:track")
        },
    );
    let mut eng8 = SoundEngine::new();
    let mut d8 = RecordingDevice::default();
    let before_static = d8.budget().used(Pool::Static);
    let _ = eng8.play(
        SoundInstance {
            looping: true,
            ..SoundInstance::bare("test:music", SoundSource::Music)
        },
        &streamed,
        &EmptyWorld,
        &mut d8,
    );
    let s_calls = d8.calls_to(0);
    let source_loop = s_calls.iter().find_map(|c| match c {
        ChannelCall::SetLooping(l) => Some(*l),
        _ => None,
    });
    let attach = s_calls.iter().find_map(|c| match c {
        ChannelCall::AttachBufferStream(p, l) => Some((p.clone(), *l)),
        _ => None,
    });
    c.record(
        "a9.a_streaming_loop_is_looped_by_the_stream_and_not_by_the_source",
        source_loop == Some(false)
            && attach == Some(("test/sounds/track.ogg".to_string(), true))
            && d8.budget().used(Pool::Streaming) == 1
            && d8.budget().used(Pool::Static) == before_static,
        format!(
            "setLooping({source_loop:?}) on the source while the attach carries \
             {attach:?}; it took the STREAMING pool"
        ),
    );

    // ---- a10: attenuation NONE takes the other arm --------------------------
    //
    // `:420-424` is an if/else and always takes one arm, so the sequence length
    // is eight either way. `disableAttenuation` writes AL_NONE, which is full
    // gain everywhere — not "infinite range" and not "silent".
    let mut eng9 = SoundEngine::new();
    let mut d9 = RecordingDevice::default();
    let _ = eng9.play(
        SoundInstance {
            attenuation: Attenuation::None,
            relative: true,
            ..SoundInstance::bare("test:ev", SoundSource::Ui)
        },
        &unit,
        &EmptyWorld,
        &mut d9,
    );
    let n_calls = d9.calls_to(0);
    c.record(
        "a10.attenuation_none_takes_the_other_arm_and_the_sequence_stays_eight",
        n_calls.len() == 8
            && n_calls.contains(&ChannelCall::DisableAttenuation)
            && !n_calls
                .iter()
                .any(|c| matches!(c, ChannelCall::LinearAttenuation(_)))
            && n_calls.contains(&ChannelCall::SetRelative(true)),
        format!("{} calls, with DisableAttenuation in place of LinearAttenuation", n_calls.len()),
    );

    // ---- a11: a silent entity costs no lookup and no draw -------------------
    //
    // The guard order at `SoundEngine.java:352-368` is load-bearing:
    // `canPlaySound` is tested BEFORE the event is resolved, so a
    // `/data`-silenced mob costs neither an index lookup nor a random draw —
    // and `entity_silent` is real since M138a rather than a hardcoded `false`.
    struct SilentMob;
    impl rewo_net::sound_engine::SoundWorld for SilentMob {
        fn entity_silent(&self, _entity_id: i32) -> bool {
            true
        }
    }
    impl rewo_net::tickable::RampWorld for SilentMob {
        fn position(&self, _: i32) -> Option<(f64, f64, f64)> {
            Some((0.0, 0.0, 0.0))
        }
        fn horizontal_speed(&self, _: i32) -> f64 {
            0.0
        }
        fn speed(&self, _: i32) -> f64 {
            0.0
        }
        fn speed_sqr(&self, _: i32) -> f64 {
            0.0
        }
        fn baby(&self, _: i32) -> bool {
            false
        }
        fn angry(&self, _: i32) -> bool {
            false
        }
        fn underwater(&self, _: i32) -> bool {
            false
        }
        fn has_ai_target(&self, _: i32) -> bool {
            false
        }
        fn attack_animation_scale(&self, _: i32) -> f32 {
            0.0
        }
        fn sniffer_digging(&self, _: i32) -> bool {
            false
        }
        fn on_rails(&self, _: i32) -> bool {
            false
        }
        fn new_minecart_behavior(&self, _: i32) -> bool {
            false
        }
        fn runs_normally(&self) -> bool {
            true
        }
        fn fall_flying(&self, _: i32) -> bool {
            false
        }
        fn vehicle_of(&self, _: i32) -> Option<i32> {
            None
        }
        fn camera_position(&self) -> (f64, f64, f64) {
            (0.0, 0.0, 0.0)
        }
    }
    let mut eng10 = SoundEngine::new();
    let mut d10 = RecordingDevice::default();
    let bound = SoundInstance {
        binding: rewo_net::sound_instance::Binding::Entity(7),
        ..SoundInstance::bare("test:ev", SoundSource::Neutral)
    };
    let quiet = eng10
        .play(bound.clone(), &unit, &SilentMob, &mut d10)
        .1;
    let loud = eng10.play(bound, &unit, &EmptyWorld, &mut d10).1;
    c.record(
        "a11.a_silent_entity_is_refused_before_the_event_is_resolved",
        quiet == PlayResult::NotStarted(NotStarted::CannotPlay)
            && loud == PlayResult::Started
            && d10.calls.iter().all(|(ch, _)| *ch == 0),
        format!(
            "silent -> {quiet:?} with no channel acquired; the same instance in a \
             world that is not silent -> {loud:?}"
        ),
    );
}

// ---------------------------------------------------------------------------
// Layer (d) — decode.   `--features audio` only.
// ---------------------------------------------------------------------------

/// Three real assets, by their content hashes in the store's object layout.
///
/// The vectors are here; the audio is not. The store is Mojang's and belongs in
/// the user's own install, exactly as the decompile and the datagen reports do —
/// this repo has never carried game assets. What is recorded is what was
/// *measured* from three of them, which is `tools/java_tostring_oracle`'s shape:
/// run the artefact once, commit the numbers.
#[cfg(feature = "audio")]
const CHICKEN: (&str, &str) = ("e1", "e16352150262ab49686f6c0aeaffa7532d3157ea");
#[cfg(feature = "audio")]
const HORN_MONO: (&str, &str) = ("ce", "ce8a2675cc2c9ac986851d2c5139d5c9ad3eeee1");
#[cfg(feature = "audio")]
const HORN_STEREO: (&str, &str) = ("16", "16c3be71d3e789ee539cd70819e526343ace5e84");

/// Read one asset through the **production** store-path resolver.
#[cfg(feature = "audio")]
fn asset(a: (&str, &str)) -> Option<Vec<u8>> {
    let root = rewo_data::sounds_json::shared_assets_dir()?;
    std::fs::read(rewo_data::sounds_json::object_path(&root, a.1)).ok()
}

#[cfg(feature = "audio")]
fn decode_layer(c: &mut Checker) {
    use rewo_audio::buffers::{Pcm, PcmSource, PcmStream, SoundBufferLibrary};
    use rewo_audio::decode::{decode_ogg_vorbis, OggStream};
    use rewo_audio::quantise::quantise;

    // ---- d1: the quantisation, against literal vectors ----------------------
    //
    // `ChunkedSampleByteBuf.java:28`, verbatim:
    //   `int intVal = Mth.clamp((int)(sample * 32767.5F - 0.5F), -32768, 32767);`
    //
    // Three details, none audible as an obvious fault when wrong:
    //  * **32767.5, not 32767** — the half is what lands 1.0 exactly on 32767
    //    once the bias is off. A 32767 multiplier gives 32766.
    //  * **the -0.5 bias is applied BEFORE the cast**, shifting the truncation
    //    boundary by half a step. `2.0/32767.5` scales to exactly 2.0 and
    //    quantises to **1** with the bias and 2 without.
    //  * **the cast TRUNCATES toward zero, it does not floor.** Silence is the
    //    case that shows it: `(int)(-0.5)` is 0, where a floor gives -1 — a
    //    constant DC offset on every silent sample of every sound in the game.
    let boundary = 2.0f32 / 32767.5;
    let literals = [
        (1.0f32, 32767i16),
        (-1.0, -32768),
        (0.0, 0),
        (-0.5, -16384),
        (0.5, 16383),
        (boundary, 1),
    ];
    let bias_free = (boundary * 32767.5) as i32; // what dropping the bias gives
    let floored = (-0.5f32).floor() as i32; // what a floor gives at silence
    c.record(
        "d1.the_quantisation_matches_its_literal_vectors",
        literals.iter().all(|(s, want)| quantise(*s) == *want)
            && bias_free == 2
            && floored == -1,
        format!(
            "{literals:?}; the same boundary sample without the bias is {bias_free} \
             (not 1), and a floor at silence gives {floored} (not 0)"
        ),
    );

    // ---- d2: the clamp is on the RESULT, not on the input --------------------
    c.record(
        "d2.the_clamp_saturates_the_truncated_integer_rather_than_wrapping",
        quantise(4.0) == 32767 && quantise(-4.0) == -32768 && quantise(f32::NAN) == 0,
        format!(
            "4.0 -> {}, -4.0 -> {}, NaN -> {} (Rust's `as i32` saturates rather \
             than being UB, and the clamp masks it either way)",
            quantise(4.0),
            quantise(-4.0),
            quantise(f32::NAN)
        ),
    );

    // ---- d3: a real ogg, fail-closed ----------------------------------------
    //
    // **`rewo-audio`'s own version of this SKIPS when the store is missing**,
    // and its module doc says that is a real weakness. This is where it stops
    // being acceptable: a missing store is a FAILED witness, per §5's
    // "store-dependent tests self-skip on a bare machine, so a green run there
    // proves nothing".
    match asset(CHICKEN) {
        None => c.record(
            "d3.a_real_ogg_decodes_to_its_exact_sample_count",
            false,
            "no unpacked asset store — the gate fails closed rather than skipping",
        ),
        Some(bytes) => {
            let pcm = decode_ogg_vorbis(&bytes);
            let ok = match &pcm {
                Ok(p) => {
                    // Exact, and it is the END TRIM that makes it so: a Vorbis
                    // stream's final page carries a granule position below what
                    // the last packet decodes to, and the surplus is discarded.
                    // A decoder that ignored it returns MORE samples and sounds
                    // almost right.
                    p.channels == 1 && p.sample_rate == 44100 && p.samples.len() == 1728
                }
                Err(_) => false,
            };
            // **The SUM sees the quantisation and the peak does not.** Measured:
            // -136 through `quantise`, and +676 with the bias dropped — a gap of
            // 812 across 1728 samples. The peak moves by ONE over the same
            // change (19988 against 19987), so a peak-only witness is blind.
            //
            // The tolerance is wide on purpose and the bound is stated here
            // rather than in a comment elsewhere: an aggregate shifts by a few
            // when a symphonia bump moves a handful of samples by an LSB, and by
            // hundreds when the bias goes missing. Pinning the exact value would
            // pin symphonia's version rather than Rewo's arithmetic, which
            // Vorbis I explicitly does not promise.
            let sum: i64 = pcm
                .as_ref()
                .map(|p| p.samples.iter().map(|s| *s as i64).sum())
                .unwrap_or(i64::MAX);
            let peak = pcm
                .as_ref()
                .map(|p| p.samples.iter().map(|s| (*s as i32).abs()).max().unwrap_or(0))
                .unwrap_or(0);
            c.record(
                "d3.a_real_ogg_decodes_to_its_exact_sample_count",
                ok && (sum - -136).abs() < 200 && peak > 8000 && peak < 32767,
                format!(
                    "1728 samples, 1ch, 44100 Hz; sum {sum} (expected -136 +/-200; \
                     dropping the -0.5 bias gives +676), peak {peak}"
                ),
            );
        }
    }

    // ---- d4: channels are per-VARIANT and the store is mixed-rate ------------
    //
    // Both are load-bearing for the mixer and both are easy to assume away.
    // `call3` is the ONLY stereo goat-horn variant, so one event resolves to a
    // 2-channel buffer on one roll and a 1-channel buffer on the next; OpenAL
    // does not spatialise a multi-channel buffer at all, so vanilla plays that
    // one non-positionally (see m11). And the store is not one rate — the
    // chicken is 44100 while the horn is 48000 — which puts the resampler on the
    // hot path for essentially every sound rather than on an edge case.
    match (asset(HORN_MONO), asset(HORN_STEREO), asset(CHICKEN)) {
        (Some(m), Some(s), Some(ch)) => {
            let (m, s, ch) = (
                decode_ogg_vorbis(&m),
                decode_ogg_vorbis(&s),
                decode_ogg_vorbis(&ch),
            );
            let ok = matches!((&m, &s, &ch), (Ok(m), Ok(s), Ok(ch))
                if m.channels == 1
                    && s.channels == 2
                    && m.sample_rate == 48000
                    && s.sample_rate == 48000
                    && ch.sample_rate == 44100
                    && s.samples.len() == 432_000);
            c.record(
                "d4.channels_are_per_variant_and_the_store_is_mixed_rate",
                ok,
                format!(
                    "call0 {:?}, call3 {:?}, chicken {:?}",
                    m.as_ref().map(|p| (p.channels, p.sample_rate)),
                    s.as_ref().map(|p| (p.channels, p.sample_rate, p.samples.len())),
                    ch.as_ref().map(|p| (p.channels, p.sample_rate)),
                ),
            );
        }
        _ => c.record(
            "d4.channels_are_per_variant_and_the_store_is_mixed_rate",
            false,
            "no unpacked asset store — the gate fails closed rather than skipping",
        ),
    }

    // ---- d5: garbage is an error, not silence, and the failure is CACHED ----
    //
    // A decoder that returned an empty buffer for unreadable input would make
    // every missing or corrupt sound indistinguishable from a legitimately
    // silent one, and the engine would dutifully play nothing with no error
    // anywhere. And `computeIfAbsent` stores the *future*, so a future that
    // completed exceptionally stays in the map — a missing file is **not**
    // retried, which is the opposite of the obvious design.
    struct Counting<'a>(&'a std::cell::Cell<usize>, Result<Pcm, String>);
    impl PcmSource for Counting<'_> {
        fn open(&mut self, _key: &str) -> Result<Pcm, String> {
            self.0.set(self.0.get() + 1);
            self.1.clone()
        }
    }
    let opens = std::cell::Cell::new(0usize);
    {
        let mut lib = SoundBufferLibrary::new(Counting(&opens, Err("boom".into())));
        let _ = lib.complete_buffer("a.ogg");
        let _ = lib.complete_buffer("a.ogg");
        let _ = lib.complete_buffer("a.ogg");
    }
    c.record(
        "d5.garbage_is_an_error_and_a_failed_decode_is_cached_like_any_other",
        decode_ogg_vorbis(b"this is not an ogg file at all").is_err()
            && decode_ogg_vorbis(&[]).is_err()
            && opens.get() == 1,
        format!(
            "unreadable input errors rather than decoding to silence; three lookups \
             of a failing key opened the source {} time(s)",
            opens.get()
        ),
    );

    // ---- d6: statics cached permanently, streams NEVER cached ---------------
    //
    // `SoundBufferLibrary.getStream` has no `computeIfAbsent` and no map lookup
    // at all: every call opens the resource again. Not an oversight — a stream
    // holds a decode position, so two sounds sharing one would fight over it,
    // and the same track started twice must genuinely play twice.
    let s_opens = std::cell::Cell::new(0usize);
    let cached_count;
    {
        let ok = Pcm {
            samples: vec![1, 2, 3, 4],
            channels: 1,
            sample_rate: 44100,
        };
        let mut lib = SoundBufferLibrary::new(Counting(&s_opens, Ok(ok)));
        for _ in 0..5 {
            let _ = lib.complete_buffer("x.ogg");
        }
        let _ = lib.complete_buffer("y.ogg");
        cached_count = lib.cached();
        // The rule, stated without performing it — `stream()` is a decision and
        // `open_stream()` is the action, which is what lets the caching claim be
        // graded by a source with no decoder in it at all.
        let h1 = lib.stream("music.ogg", true);
        let h2 = lib.stream("music.ogg", true);
        // Two handles, and the loop flag rides with them rather than with the
        // channel.
        assert_eq!(h1, h2);
    }
    c.record(
        "d6.a_static_buffer_is_cached_permanently_and_a_stream_is_not_cached_at_all",
        s_opens.get() == 2 && cached_count == 2,
        format!(
            "six lookups of two keys opened the source {} times and left {} cached",
            s_opens.get(),
            cached_count
        ),
    );

    // ---- d7: a looping stream restarts one read LATE ------------------------
    //
    // `LoopingAudioStream.read` guards on the INNER read coming back *empty*
    // (`LoopingAudioStream.java:28-38`), and a short non-empty read is the
    // ordinary end of a file — so a looping stream hands out one short buffer at
    // the loop point and a full one after it. The signature is in the buffer
    // LENGTHS, not in the samples: a version that spliced across the boundary to
    // keep every buffer full would sound identical and would not be this.
    //
    // The chicken is 1728 samples, so at 1000 the pattern is 1000, 728, 1000,
    // 728 … — the fixture's own numbers rather than round by luck.
    match asset(CHICKEN) {
        None => c.record(
            "d7.a_looping_stream_restarts_one_read_after_it_runs_out",
            false,
            "no unpacked asset store — the gate fails closed rather than skipping",
        ),
        Some(bytes) => {
            let mut s = OggStream::open(bytes.clone().into(), true).expect("open looping");
            let lens: Vec<usize> = (0..5).map(|_| s.read(1000).expect("read").len()).collect();
            // ...and a NON-looping stream of the same asset stays exhausted,
            // which is what makes the loop flag the thing being witnessed.
            let mut once = OggStream::open(bytes.into(), false).expect("open once");
            let mut once_lens = Vec::new();
            for _ in 0..5 {
                once_lens.push(once.read(1000).expect("read").len());
            }
            c.record(
                "d7.a_looping_stream_restarts_one_read_after_it_runs_out",
                lens == vec![1000, 728, 1000, 728, 1000] && once_lens == vec![1000, 728, 0, 0, 0],
                format!("looping {lens:?} against non-looping {once_lens:?}"),
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Layer (m) — the mixer.   `--features audio` only.
// ---------------------------------------------------------------------------

/// The mixer's output rate for every witness here. 48 kHz so a 48 kHz fixture
/// resamples 1:1 and the gain witnesses are not also measuring the resampler.
#[cfg(feature = "audio")]
const OUT_RATE: u32 = 48_000;

/// A constant-amplitude mono buffer, so the output level is directly readable.
#[cfg(feature = "audio")]
fn dc(amplitude: i16, frames: usize) -> std::sync::Arc<rewo_audio::buffers::Pcm> {
    std::sync::Arc::new(rewo_audio::buffers::Pcm {
        samples: vec![amplitude; frames],
        channels: 1,
        sample_rate: OUT_RATE,
    })
}

/// `(max |L|, max |R|)` over an interleaved stereo block — read **out of the
/// rendered output**, which is the whole point of layer (m).
#[cfg(feature = "audio")]
fn peak_lr(out: &[f32]) -> (f32, f32) {
    out.chunks_exact(2).fold((0.0f32, 0.0f32), |(l, r), s| {
        (l.max(s[0].abs()), r.max(s[1].abs()))
    })
}

#[cfg(feature = "audio")]
fn mixer_layer(c: &mut Checker) {
    use rewo_audio::device::Command;
    use rewo_audio::mixer::{Mixer, NullSink, Voice};
    use rewo_net::sound_engine::{ChannelCall as CC, ListenerTransform};

    // §5's trap, in force everywhere below: **a fixture built from
    // `Sound::file` has volume and pitch 1.0, which is exactly where
    // `instance.volume * sound.volume` and a dropped multiplication agree.**
    // Nothing here uses a gain of 0 or 1, and no pitch is 1 or a power of two.
    const GAIN: f32 = 0.6;
    const AMP: i16 = 20_000;

    let render = |m: &mut Mixer, frames: usize| -> Vec<f32> {
        let mut sink = NullSink::new();
        sink.pull(m, frames).to_vec()
    };

    // ---- m1: the ramp is linear and reaches EXACTLY zero at max -------------
    //
    // The property inverse-square cannot have. `Channel.linearAttenuation`
    // writes `AL_LINEAR_DISTANCE` with rolloff 1 and reference 0
    // (`Channel.java:108-113`), so the OpenAL 1.1 curve is `1 - d/max`.
    //
    // **Read out of the output**, and the expected ratios are literals stated
    // here from the specification rather than a call to `openal::linear_gain`,
    // which would grade that function against itself (M88's r20).
    //
    // The source is LONGER than the render window on purpose: this measures a
    // level, so the voice must not end inside it. (m4 needs the opposite, and
    // says so there.)
    let level_at = |distance: f32| -> f32 {
        let mut m = Mixer::new(OUT_RATE);
        let mut v = Voice::new(dc(AMP, 4096));
        v.gain = GAIN;
        v.max_distance = Some(16.0);
        // Along the listener's forward axis (INITIAL faces -Z), so the pan is
        // constant across every distance and only the attenuation moves.
        v.position = [0.0, 0.0, -distance];
        m.push(v);
        let out = render(&mut m, 512);
        peak_lr(&out).0
    };
    let base = level_at(0.0);
    let ramp: Vec<f32> = [0.0f32, 4.0, 8.0, 12.0, 16.0]
        .iter()
        .map(|d| level_at(*d))
        .collect();
    let want = [1.0f32, 0.75, 0.5, 0.25, 0.0];
    let ramp_ok = base > 0.0
        && ramp
            .iter()
            .zip(want.iter())
            .all(|(got, w)| (got / base - w).abs() < 1e-3)
        // Exactly zero, not merely small: past `max` the unclamped linear model
        // goes negative, and a clamp that stopped at some epsilon would leave a
        // sound faintly audible outside its own radius.
        && ramp[4] == 0.0;
    c.record(
        "m1.attenuation_is_linear_in_the_output_and_exactly_zero_at_max",
        ramp_ok,
        format!(
            "levels {ramp:?} at d=0/4/8/12/16 over max 16 -> ratios {:?} against the \
             spec's {want:?}; the last is exactly {}",
            ramp.iter().map(|g| g / base).collect::<Vec<_>>(),
            ramp[4]
        ),
    );

    // ---- m2: hard left, hard right, front ------------------------------------
    //
    // `ListenerTransform::right()` is `forward.cross(up)` — forward × up, not
    // up × forward; those differ by a sign, which is the difference between a
    // stereo image and its mirror. With INITIAL (facing -Z, up +Y) that is +X,
    // so a source at -X is on the LEFT.
    let image_at = |pos: [f32; 3]| -> (f32, f32) {
        let mut m = Mixer::new(OUT_RATE);
        let mut v = Voice::new(dc(AMP, 4096));
        v.gain = GAIN;
        v.position = pos;
        m.push(v);
        let out = render(&mut m, 512);
        peak_lr(&out)
    };
    let left = image_at([-10.0, 0.0, 0.0]);
    let right = image_at([10.0, 0.0, 0.0]);
    let front = image_at([0.0, 0.0, -10.0]);
    // **The two extremes are not symmetric in f32, and the witness must not
    // demand that they are.** `pan_gains` is `(cos a, sin a)` for
    // `a = (pan + 1) * FRAC_PI_4`. Hard left is `pan = -1`, so `a = 0` and
    // `sin(0)` is *exactly* 0 — the right ear is bit-silent. Hard right is
    // `pan = +1`, so `a = FRAC_PI_2` and `f32::cos(FRAC_PI_2)` is
    // **-4.371139e-8**, not zero: the left ear sits about 155 dB down instead of
    // absent. A witness asserting `== 0.0` on both sides passes on one and fails
    // on the other, which is how this one was written and what it measured.
    //
    // (Contrast m1, where `== 0.0` IS the right assertion: `linear_gain` clamps
    // to 0.0 explicitly rather than arriving there by arithmetic.)
    let far_side_down = |near: f32, far: f32| near > 0.0 && far < near * 1e-6;
    c.record(
        "m2.the_stereo_image_follows_the_source_and_is_not_mirrored",
        far_side_down(left.0, left.1)
            && far_side_down(right.1, right.0)
            && (front.0 - front.1).abs() < 1e-6
            && front.0 > 0.0,
        format!(
            "-X -> {left:?}, +X -> {right:?}, front -> {front:?}; the far ear is \
             >120 dB down rather than exactly silent, because f32 cos(PI/2) is \
             -4.371139e-8"
        ),
    );

    // ---- m3: AL_SOURCE_RELATIVE keeps a UI sound put -------------------------
    //
    // `Channel.setRelative` (`Channel.java:115-117`). A relative source's
    // position is relative to the listener rather than to the world, which is
    // how a UI click stays centred while the player walks away — and the
    // failure mode is a menu that gets quieter as you travel.
    let ui_at = |listener: [f64; 3], relative: bool| -> (f32, f32) {
        let mut m = Mixer::new(OUT_RATE);
        m.listener = ListenerTransform {
            position: listener,
            ..ListenerTransform::INITIAL
        };
        let mut v = Voice::new(dc(AMP, 4096));
        v.gain = GAIN;
        v.relative = relative;
        v.max_distance = Some(16.0);
        v.position = [0.0, 0.0, -4.0];
        m.push(v);
        peak_lr(&render(&mut m, 512))
    };
    let near = ui_at([0.0, 0.0, 0.0], true);
    let far = ui_at([1000.0, 0.0, 0.0], true);
    // The control: a non-relative source at the same place is silenced by the
    // walk, so this measures `relative` rather than a source that never moved.
    let world_far = ui_at([1000.0, 0.0, 0.0], false);
    c.record(
        "m3.a_relative_source_is_unmoved_by_the_listener",
        near == far && near.0 > 0.0 && world_far == (0.0, 0.0),
        format!("relative near {near:?} == far {far:?}; a WORLD source at the same distance -> {world_far:?}"),
    );

    // ---- m4: pitch changes the LENGTH of a one-shot --------------------------
    //
    // `AL_PITCH` is a playback-rate multiplier, not a separate effect, so it
    // multiplies into the rate conversion — a one-shot at pitch p occupies
    // `frames / p` output frames.
    //
    // **The source must be SHORTER than the render window here**, which is the
    // opposite of m1's requirement: §5 records that a rate witness whose
    // sources outlast its window measures the window rather than the rate.
    // 4800 frames at 48 kHz is 0.1 s inside a 0.5 s window.
    //
    // Pitches 1.25 and 0.8 rather than 0.5/2.0: a power of two can agree with a
    // wrong implementation that shifted an exponent.
    let sounding_frames = |pitch: f32| -> usize {
        let mut m = Mixer::new(OUT_RATE);
        let mut v = Voice::new(dc(AMP, 4800));
        v.gain = GAIN;
        v.pitch = pitch;
        m.push(v);
        let out = render(&mut m, 24_000);
        out.chunks_exact(2).filter(|s| s[0] != 0.0).count()
    };
    let (fast, slow, unit_pitch) = (sounding_frames(1.25), sounding_frames(0.8), sounding_frames(1.0));
    // 4800 / 1.25 = 3840, 4800 / 0.8 = 6000. +/-2 frames for where the cursor
    // lands on the final step.
    c.record(
        "m4.pitch_changes_the_length_of_a_one_shot",
        (fast as i64 - 3840).abs() <= 2
            && (slow as i64 - 6000).abs() <= 2
            && (unit_pitch as i64 - 4800).abs() <= 2,
        format!(
            "4800 source frames -> {fast} out at pitch 1.25 (want 3840), {slow} at \
             0.8 (want 6000), {unit_pitch} at 1.0"
        ),
    );

    // ---- m5: a PITCHED listener still has a stereo image ---------------------
    //
    // The witness the up vector exists for. `right = forward x up` is
    // `(-cos yaw, 0, -sin yaw)` for EVERY pitch — pitching turns *about* the
    // right axis and cannot move it — so almost every way of breaking the basis
    // is invisible. The one that is not: pin `up` to the constant `(0,1,0)`, and
    // at pitch 90 the forward vector is `(0,-1,0)`, making `forward x up` the
    // **ZERO vector** and collapsing the stereo image to centre.
    //
    // So this is two-sided by construction: the real basis puts a source hard to
    // one side while the pinned-up basis centres it.
    let (fwd, up) = rewo_net::sound_engine::listener_basis(0.0, 90.0);
    let looking_down = |up_vec: [f32; 3]| -> (f32, f32) {
        let mut m = Mixer::new(OUT_RATE);
        m.listener = ListenerTransform {
            position: [0.0, 0.0, 0.0],
            forward: fwd,
            up: up_vec,
        };
        let mut v = Voice::new(dc(AMP, 4096));
        v.gain = GAIN;
        v.position = [-10.0, 0.0, 0.0];
        m.push(v);
        peak_lr(&render(&mut m, 512))
    };
    let real = looking_down(up);
    let pinned = looking_down([0.0, 1.0, 0.0]);
    let cross_pinned = ListenerTransform {
        position: [0.0; 3],
        forward: fwd,
        up: [0.0, 1.0, 0.0],
    }
    .right();
    // The magnitudes, not the components: `f32::cos(90f32.to_radians())` is
    // -4.371139e-8 rather than 0 (see m2), so the degenerate cross product is a
    // vector of length ~4e-8 rather than the exact zero the algebra gives. That
    // is still 7 orders of magnitude below the real basis's unit-length right
    // vector, and it is what collapses the image.
    let mag = |v: [f64; 3]| (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    let cross_real = ListenerTransform {
        position: [0.0; 3],
        forward: fwd,
        up,
    }
    .right();
    c.record(
        "m5.a_pitched_listener_still_has_a_stereo_image",
        // Hard to one side with the real basis...
        (real.0 + real.1) > 0.0
            && real.0.min(real.1) < real.0.max(real.1) * 1e-6
            // ...and centred with `up` pinned, which is what makes this measure
            // the UP vector rather than the forward one.
            && (pinned.0 - pinned.1).abs() < 1e-6
            && pinned.0 > 0.0
            && mag(cross_real) > 0.99
            && mag(cross_pinned) < 1e-6,
        format!(
            "at pitch 90 forward={fwd:?} up={up:?} gives |right|={:.6} and a one-sided \
             image {real:?}; with up pinned to (0,1,0) |right| collapses to {:e} \
             ({cross_pinned:?}) and the image centres at {pinned:?}",
            mag(cross_real),
            mag(cross_pinned)
        ),
    );

    // ---- m6: render OVERWRITES a dirty buffer --------------------------------
    //
    // §5: witnesses that render into a freshly-allocated (therefore zeroed)
    // buffer cannot see a missing clear, which on a real device is unbounded
    // accumulation — the callback hands the same buffer back every period.
    let mut empty = Mixer::new(OUT_RATE);
    let mut dirty = vec![0.5f32; 256];
    empty.render(&mut dirty);
    let dirty_ok = dirty.iter().all(|s| *s == 0.0);
    c.record(
        "m6.render_overwrites_a_dirty_buffer_rather_than_accumulating",
        dirty_ok,
        format!(
            "a buffer pre-filled with 0.5 comes back {} — a fresh (zeroed) buffer \
             could not have witnessed this",
            if dirty_ok { "all zero" } else { "DIRTY" }
        ),
    );

    // ---- m7: an empty mixer renders EXACT silence ----------------------------
    //
    // A level claim rather than a clear claim, and it needs to be exact: "very
    // quiet" would admit a DC offset, which is what a quantisation that floored
    // instead of truncating produces (see d1).
    let mut silent = Mixer::new(OUT_RATE);
    let out = render(&mut silent, 256);
    c.record(
        "m7.an_empty_mixer_renders_exact_silence",
        out.iter().all(|s| *s == 0.0) && out.len() == 512,
        format!("{} samples, all exactly 0.0", out.len()),
    );

    // ---- m8: a dense scene clamps rather than wrapping ------------------------
    //
    // The mix accumulates in f32 and clamps once at the end. The clamp is a hard
    // limiter of last resort — matching OpenAL Soft's own look-ahead curve is
    // explicitly out of scope (its parameters live in the DLL and are set
    // nowhere in Java) — but a scene that WRAPPED would invert phase and be
    // grossly audible, which is the failure this excludes.
    let mut dense = Mixer::new(OUT_RATE);
    for _ in 0..24 {
        let mut v = Voice::new(dc(30_000, 4096));
        v.gain = 0.9;
        v.position = [0.0, 0.0, -1.0];
        dense.push(v);
    }
    let loud = render(&mut dense, 256);
    let peak = loud.iter().fold(0.0f32, |a, s| a.max(s.abs()));
    c.record(
        "m8.a_dense_scene_clamps_instead_of_wrapping",
        loud.iter().all(|s| (-1.0..=1.0).contains(s)) && peak > 0.9 && loud.iter().all(|s| *s >= 0.0),
        format!(
            "24 voices summing well past full scale peak at {peak}, stay inside \
             [-1,1], and none went negative (a wrap would invert phase)"
        ),
    );

    // ---- m9: an underrun is SILENCE, not death -------------------------------
    //
    // The producer decides when a stream is over. A mixer that called an empty
    // queue `finished` would kill a music track on the first hitch, and the
    // channel would be released before the next chunk arrived.
    let mut stream = Mixer::new(OUT_RATE);
    stream.apply(&Command::Queue(0, dc(AMP, 480)));
    stream.apply(&Command::Channel(0, CC::SetVolume(GAIN)));
    stream.apply(&Command::Channel(0, CC::Play));
    let first = render(&mut stream, 960); // outlasts the 480-frame chunk
    let starved = render(&mut stream, 480); // nothing queued
    stream.apply(&Command::Queue(0, dc(AMP, 480)));
    let resumed = render(&mut stream, 480);
    let alive = stream.voice(0).map(|v| !v.finished);
    c.record(
        "m9.a_streaming_voice_underruns_into_silence_and_recovers",
        peak_lr(&first).0 > 0.0
            && starved.iter().all(|s| *s == 0.0)
            && peak_lr(&resumed).0 > 0.0
            && alive == Some(true),
        format!(
            "chunk plays, then {} frames of exact silence, then it resumes; \
             finished={:?}",
            starved.len() / 2,
            alive.map(|a| !a)
        ),
    );

    // ---- m10: the ENGINE's gain reaches the output ---------------------------
    //
    // The witness that crosses the crates: a real `SoundEngine::play` into a
    // `RecordingDevice`, whose recorded `ChannelCall`s are replayed through the
    // production `Mixer`, whose OUTPUT is then measured.
    //
    // **Non-circular by construction.** The measured quantity is the ratio of
    // two renders that differ only in the gain the engine computed; the expected
    // quantity is the `SetVolume` the device recorded. Neither is
    // `openal::linear_gain` recomputed in the witness, and neither is read off
    // the `Voice`.
    //
    // Nothing in the arithmetic is 0 or 1: master 0.5, blocks 0.8, instance
    // volume 0.75, sound volume 0.5 -> 0.375 * 0.4 = **0.15**.
    let mut idx = SoundsIndex::new();
    idx.handle_registration(
        "test:ev",
        &SoundEventRegistration {
            sounds: vec![Sound {
                volume: 0.5,
                pitch: 0.5,
                ..Sound::file("test:file")
            }],
            replace: false,
            subtitle: None,
        },
        &SoundFileSet::All,
    );
    let mut eng = SoundEngine::new();
    eng.options.set_slider(SoundSource::Master, 0.5);
    eng.options.set_slider(SoundSource::Blocks, 0.8);
    let mut dev = RecordingDevice::default();
    let (_, res) = eng.play(
        SoundInstance {
            volume: 0.75,
            pitch: 1.5,
            ..SoundInstance::bare("test:ev", SoundSource::Blocks)
        },
        &idx,
        &EmptyWorld,
        &mut dev,
    );
    let engine_gain = dev.calls_to(0).into_iter().find_map(|call| match call {
        ChannelCall::SetVolume(v) => Some(v),
        _ => None,
    });
    // Replay the recorded sequence, substituting the gain in the control.
    let replay = |gain: f32| -> f32 {
        let mut m = Mixer::new(OUT_RATE);
        for call in dev.calls_to(0) {
            let call = match call {
                ChannelCall::SetVolume(_) => ChannelCall::SetVolume(gain),
                // The engine positions this at the origin, where the listener
                // is, so the attenuation is 1 and the level is purely the gain.
                other => other,
            };
            m.apply(&Command::Channel(0, call));
        }
        m.apply(&Command::Attach(0, dc(AMP, 4096)));
        m.apply(&Command::Channel(0, CC::Play));
        peak_lr(&render(&mut m, 512)).0
    };
    let with_engine_gain = replay(engine_gain.unwrap_or(0.0));
    let reference = replay(1.0);
    let measured_ratio = if reference > 0.0 {
        with_engine_gain / reference
    } else {
        f32::NAN
    };
    c.record(
        "m10.the_engines_computed_gain_reaches_the_rendered_output",
        res == PlayResult::Started
            && engine_gain == Some(0.15)
            && (measured_ratio - 0.15).abs() < 1e-3,
        format!(
            "master 0.5 x blocks 0.8 x (0.75 x 0.5) -> the device was handed \
             SetVolume({engine_gain:?}), and the output falls to {measured_ratio} of \
             an otherwise identical render at gain 1.0"
        ),
    );

    // ---- m11: a multi-channel buffer is NOT spatialised ----------------------
    //
    // OpenAL's rule and therefore vanilla's. `item/goat_horn/call3.ogg` is the
    // one stereo variant of an otherwise mono event (see d4), so the same event
    // spatialises on seven rolls and not the eighth. A hand-written mixer
    // naturally downmixes and spatialises uniformly, which is arguably better
    // and is a **divergence** — so it is chosen explicitly and witnessed.
    let stereo = std::sync::Arc::new(rewo_audio::buffers::Pcm {
        // L = +AMP, R = -AMP/2, interleaved: the two channels are distinct, so
        // a downmix would show as equal levels.
        samples: (0..4096)
            .flat_map(|_| [AMP, -AMP / 2])
            .collect::<Vec<i16>>(),
        channels: 2,
        sample_rate: OUT_RATE,
    });
    let mut st = Mixer::new(OUT_RATE);
    let mut v = Voice::new(stereo);
    v.gain = GAIN;
    v.position = [-10.0, 0.0, 0.0]; // hard left, and must be ignored
    st.push(v);
    let (sl, sr) = peak_lr(&render(&mut st, 512));
    c.record(
        "m11.a_stereo_source_plays_its_own_channels_and_ignores_its_position",
        sr > 0.0 && (sl / sr - 2.0).abs() < 1e-3,
        format!(
            "a source at hard left renders L={sl} R={sr} (ratio {:.4}, the buffer's \
             own 2:1) rather than silencing the right ear",
            sl / sr
        ),
    );

    // ---- m12: a voice built by COMMANDS is silent until its Play -------------
    //
    // Vanilla's order is properties, then attach, then play
    // (`SoundEngine.java:417-434`), and `alSourcePlay` before an attach is a
    // no-op. A voice that started sounding on its first `SetPitch` would play
    // its opening milliseconds at whatever position and volume had arrived so
    // far — which is why `Mixer::ensure` creates one silent and `Voice::new`
    // (the direct, caller-says-play path) does not.
    let mut seq = Mixer::new(OUT_RATE);
    seq.apply(&Command::Channel(0, CC::SetPitch(1.25)));
    seq.apply(&Command::Channel(0, CC::SetVolume(GAIN)));
    seq.apply(&Command::Channel(0, CC::SetSelfPosition(0.0, 0.0, 0.0)));
    seq.apply(&Command::Attach(0, dc(AMP, 4096)));
    let before_play = render(&mut seq, 256);
    seq.apply(&Command::Channel(0, CC::Play));
    let after_play = render(&mut seq, 256);
    c.record(
        "m12.a_voice_built_by_commands_is_silent_until_its_play",
        before_play.iter().all(|s| *s == 0.0) && peak_lr(&after_play).0 > 0.0,
        format!(
            "attached but unplayed renders exact silence; the same voice after Play \
             peaks at {}",
            peak_lr(&after_play).0
        ),
    );

    // ---- m13: a path-carrying attach is COUNTED, not silently ignored --------
    //
    // `AttachStaticBuffer` and `AttachBufferStream` carry an asset *key*, and
    // resolving one is a store lookup plus an ogg decode — neither of which may
    // happen on an audio callback. The producer resolves the key and sends
    // `Command::Attach` with the PCM in hand, which is what vanilla's
    // `thenAccept` continuation does too. Seeing one here means the producer
    // skipped that step, so it is counted where a silent ignore would hide it.
    let mut leaked = Mixer::new(OUT_RATE);
    leaked.apply(&Command::Channel(0, CC::AttachStaticBuffer("x.ogg".into())));
    leaked.apply(&Command::Channel(0, CC::AttachBufferStream("y.ogg".into(), true)));
    leaked.apply(&Command::Channel(0, CC::Play));
    let after = render(&mut leaked, 256);
    c.record(
        "m13.an_unresolved_attach_is_counted_rather_than_silently_ignored",
        leaked.ignored == 2 && after.iter().all(|s| *s == 0.0),
        format!(
            "two key-carrying attaches raised `ignored` to {} and produced no audio",
            leaked.ignored
        ),
    );
}
