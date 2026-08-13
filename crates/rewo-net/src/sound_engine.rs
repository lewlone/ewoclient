//! `SoundEngine` — vanilla's play/tick/stop state machine, its channel budget,
//! and a device-agnostic seam (M131).
//!
//! # The line this module does not cross
//!
//! Everything here is what vanilla computes **before** a sample is touched. No
//! audio crate, no device, no mixer, no `.ogg` decoder, no resampler — those
//! are the user's decisions and are explicitly deferred. [`AudioDevice`] is
//! where they will plug in; [`RecordingDevice`] is the implementation the
//! tests use, and it records the call sequence rather than making a noise.
//!
//! So: a test here can assert *which* channel a sound takes, *what* gain,
//! pitch, position, attenuation and loop flag it is given, *in what order*,
//! and *whether it plays at all*. No test here can assert that anything sounds
//! right — that needs a human, and saying so is the point of the split.
//!
//! # Ground truth (bundled 26.2 decompile)
//!
//! - `net/minecraft/client/sounds/SoundEngine.java` — `play`, `tick`,
//!   `tickInGameSound`, `stop`, `stopAll`, `isActive`, `MIN_SOURCE_LIFETIME`.
//! - `net/minecraft/client/sounds/ChannelAccess.java` — the handle, its
//!   `stopped` flag, and `scheduleTick`.
//! - `com/mojang/blaze3d/audio/Library.java` — `init`'s pool sizing,
//!   `CountingChannelPool.acquire`, `DEFAULT_CHANNEL_COUNT`.
//! - `com/mojang/blaze3d/audio/Channel.java` — the twelve source operations.
//! - `net/minecraft/client/multiplayer/ClientPacketListener.java` and
//!   `ClientLevel.java` — how the three packets reach an instance.

use std::collections::HashMap;

use rewo_data::sounds_json::{ResolvedSound, SoundsIndex};

use crate::sound_instance::{
    attenuation_distance, calculate_pitch, calculate_volume, instance_pitch, instance_volume,
    Attenuation, Binding, CategoryVolumes, SoundInstance,
};
use crate::sounds::{EntitySound, LocalSound, PositionedSound, SoundEvent, SoundSource, StopSound};

/// `SoundEngine.MIN_SOURCE_LIFETIME` — a played instance is not reclaimed for
/// this many ticks even if its channel stopped immediately.
pub const MIN_SOURCE_LIFETIME: i32 = 20;

/// `Library.DEFAULT_CHANNEL_COUNT` — the fallback when the device does not
/// advertise `ALC_MONO_SOURCES`.
pub const DEFAULT_CHANNEL_COUNT: i32 = 30;

/// The three OpenAL facts that are *not* Minecraft's.
///
/// The decompile writes bare integers (`AL10.alSourcei(this.source, 53248,
/// 53251)`), so the names below were read out of the real
/// `lwjgl-openal-3.4.1.jar` rather than from memory — the M114/M125 precedent
/// of grading against the shipped artefact. **The numbers collide across
/// namespaces**: `4112` is `AL_SOURCE_STATE` when passed to `alGetSourcei` and
/// `ALC_MONO_SOURCES` when passed to `alcGetIntegerv`, and the decompile uses
/// it both ways within one file; `4099` is `AL_PITCH` in AL and
/// `ALC_ALL_ATTRIBUTES` in ALC, likewise. Naming a constant from its integer
/// without tracking which call it belongs to gets one of each pair wrong.
pub mod openal {
    /// `AL_DISTANCE_MODEL` (AL10), the property `Channel` sets per source.
    pub const AL_DISTANCE_MODEL: i32 = 53248;
    /// `AL_LINEAR_DISTANCE` (AL11 / `AL_EXT_LINEAR_DISTANCE`).
    ///
    /// Note **not** `AL_LINEAR_DISTANCE_CLAMPED` (53252), which is a distinct
    /// enum vanilla does not use: the clamped model would first clamp the
    /// distance into `[reference, max]`, and the unclamped one does not.
    pub const AL_LINEAR_DISTANCE: i32 = 53251;
    /// `AL_NONE` — what `Channel.disableAttenuation` writes.
    pub const AL_NONE: i32 = 0;
    /// `Channel.linearAttenuation`'s `alSourcef(source, AL_ROLLOFF_FACTOR, …)`.
    pub const ROLLOFF_FACTOR: f32 = 1.0;
    /// `Channel.linearAttenuation`'s `alSourcef(source, AL_REFERENCE_DISTANCE, …)`.
    ///
    /// **Zero**, which is what makes the curve a straight line from full gain
    /// at the listener to nothing at `max_distance` — a non-zero reference
    /// distance would give a flat region around the source first.
    pub const REFERENCE_DISTANCE: f32 = 0.0;

    /// The `AL_LINEAR_DISTANCE` gain curve.
    ///
    /// **This formula is NOT in the Minecraft decompile.** Vanilla sets three
    /// source properties and lets the driver do the arithmetic, so the curve
    /// belongs to the OpenAL 1.1 specification, not to Minecraft:
    ///
    /// ```text
    /// gain = 1 - rolloff * (distance - reference) / (max - reference)
    /// ```
    ///
    /// With vanilla's parameters (`rolloff = 1`, `reference = 0`) that is
    /// `1 - distance / max`. It is reproduced here so a recording sink has
    /// something to be graded against and so the *shape* of the falloff is
    /// stated somewhere rather than assumed; a real device would compute its
    /// own, and any divergence between the two is the driver's to explain.
    ///
    /// The final `[0, 1]` clamp is `AL_MIN_GAIN` / `AL_MAX_GAIN`, whose
    /// defaults vanilla never changes — also specification, not decompile.
    /// Without it the unclamped linear model goes **negative** past `max`.
    pub fn linear_gain(distance: f32, max_distance: f32) -> f32 {
        if max_distance == REFERENCE_DISTANCE {
            // A degenerate `max` makes the spec's denominator zero. Vanilla can
            // reach it: `attenuation_distance` is `max(volume, 1) * the
            // sounds.json integer`, and a pack may write
            // `"attenuation_distance": 0`. Report silence rather than a NaN.
            return 0.0;
        }
        let raw = 1.0
            - ROLLOFF_FACTOR * (distance - REFERENCE_DISTANCE)
                / (max_distance - REFERENCE_DISTANCE);
        raw.clamp(0.0, 1.0)
    }
}

/// `Library.Pool`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Pool {
    /// A whole `.ogg` decoded into one buffer. Almost everything.
    Static,
    /// Fed from an `AudioStream` a buffer at a time. Music and records —
    /// `sounds.json`'s `"stream": true`.
    Streaming,
}

/// An opaque device-side source. Vanilla's is an OpenAL source name.
pub type ChannelId = u32;

/// One operation on a channel — the twelve `Channel` methods `SoundEngine`
/// and `ChannelAccess` reach.
///
/// Modelled as data rather than as trait methods so a device can be a
/// *recorder*: the whole point of [`RecordingDevice`] is that a test can
/// assert the exact call sequence `play` produces, and a sequence is only
/// assertable if the calls are values.
#[derive(Clone, Debug, PartialEq)]
pub enum ChannelCall {
    SetPitch(f32),
    SetVolume(f32),
    /// `Channel.linearAttenuation(maxDistance)` — three `alSourcef`s, see
    /// [`openal`].
    LinearAttenuation(f32),
    /// `Channel.disableAttenuation()`.
    DisableAttenuation,
    SetLooping(bool),
    SetSelfPosition(f64, f64, f64),
    SetRelative(bool),
    /// `Channel.attachStaticBuffer` — the argument is the asset path the
    /// buffer would be decoded from, since there is no buffer here.
    AttachStaticBuffer(String),
    /// `Channel.attachBufferStream` — the path and whether the stream itself
    /// loops (`SoundBufferLibrary.getStream(path, looping)`).
    AttachBufferStream(String, bool),
    Play,
    Stop,
    Pause,
    Unpause,
}

/// The seam a real audio backend implements.
///
/// Drawn at vanilla's `Library` + `Channel` boundary — one level *below*
/// `ChannelAccess`, because the handle bookkeeping and the delete-time grace
/// period are engine logic that a backend should not have to reproduce, and
/// one level *above* OpenAL, because nothing here names a driver.
///
/// A future device milestone implements this over cpal/rodio/kira; nothing in
/// this crate ever will.
pub trait AudioDevice {
    /// `Library.acquireChannel(pool)`.
    ///
    /// **`None` when the pool is full, and that is not a stub.**
    /// `CountingChannelPool.acquire` returns `null` at the limit and
    /// `SoundEngine.play` turns that into `NOT_STARTED` — vanilla **drops the
    /// new sound** rather than stealing a voice from an old one. See
    /// [`ChannelBudget`].
    fn acquire(&mut self, pool: Pool) -> Option<ChannelId>;
    /// `Library.releaseChannel(channel)` — vanilla destroys the source.
    fn release(&mut self, channel: ChannelId);
    /// One `handle.execute(channel -> …)` body.
    fn submit(&mut self, channel: ChannelId, call: ChannelCall);
    /// `Channel.stopped()` — `AL_SOURCE_STATE == AL_STOPPED`.
    fn stopped(&self, channel: ChannelId) -> bool;
    /// `Listener.setTransform` — where the ears are (M138a).
    ///
    /// **A trait method rather than a [`ChannelCall`]**, because the listener is
    /// not a channel: handing it a fake [`ChannelId`] so it could reuse `submit`
    /// would drop it into the per-channel call log, and every witness that
    /// asserts a channel's exact eight-call sequence would then have to learn to
    /// skip it.
    ///
    /// **Pushed per FRAME, unlike everything else on this seam.**
    /// `SoundEngine.updateSource(camera)` runs on the render path with a camera
    /// carrying the frame's partial tick, while `SoundEngine.tick` is
    /// `Minecraft.tick`'s. Driving this per tick would step the stereo image at
    /// 20 Hz while the world turned smoothly.
    fn set_listener(&mut self, transform: ListenerTransform);
}

/// `com.mojang.blaze3d.audio.ListenerTransform` — where the ears are and which
/// way they point (M138a).
///
/// ```java
/// public record ListenerTransform(Vec3 position, Vec3 forward, Vec3 up) {
///    public static final ListenerTransform INITIAL =
///       new ListenerTransform(Vec3.ZERO, new Vec3(0, 0, -1), new Vec3(0, 1, 0));
///    public Vec3 right() { return this.forward.cross(this.up); }
/// }
/// ```
///
/// `Listener.setTransform` hands it straight to OpenAL as a position plus **six
/// floats**: `alListener3f(AL_POSITION, x, y, z)` then
/// `alListenerfv(AL_ORIENTATION, {fx, fy, fz, ux, uy, uz})`
/// (`Listener.java:14-15`). There is no listener velocity — vanilla never sets
/// `AL_VELOCITY` — so there is no doppler to model.
///
/// **Why forward/up are `f32` while position is `f64`.** The record holds three
/// `Vec3`, but forward and up are *built* from `Vector3f`: `new
/// Vec3(camera.forwardVector())` at `SoundEngine.java:493` widens an f32 basis,
/// and `Listener` narrows it straight back for `alListenerfv`. Storing f32 is
/// therefore exactly what the device receives, and storing f64 would invite a
/// recomputation in double precision that vanilla never performs. The position
/// is a genuine `Vec3` — it comes from `camera.position()`, and only OpenAL
/// narrows it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ListenerTransform {
    /// `camera.position()`.
    pub position: [f64; 3],
    /// `camera.forwardVector()` — the rotated `FORWARDS` basis vector.
    pub forward: [f32; 3],
    /// `camera.upVector()`.
    pub up: [f32; 3],
}

impl ListenerTransform {
    /// `ListenerTransform.INITIAL` — the origin, facing -Z, up +Y.
    ///
    /// This is the transform a listener has **before any camera exists**, and
    /// until M138a it was the only one Rewo could express, because nothing
    /// carried a listener at all: every sound was positioned in absolute world
    /// coordinates against ears sitting at the origin facing -Z.
    pub const INITIAL: ListenerTransform = ListenerTransform {
        position: [0.0; 3],
        forward: [0.0, 0.0, -1.0],
        up: [0.0, 1.0, 0.0],
    };

    /// `ListenerTransform.right()` — `forward.cross(up)`.
    ///
    /// **In `Vec3`, i.e. f64**, because vanilla's `right()` is a method on the
    /// record's widened fields rather than on the `Vector3f` basis. The order is
    /// forward × up and not up × forward; those differ by a sign, which is the
    /// difference between a stereo image and its mirror.
    pub fn right(&self) -> [f64; 3] {
        let f = [
            self.forward[0] as f64,
            self.forward[1] as f64,
            self.forward[2] as f64,
        ];
        let u = [self.up[0] as f64, self.up[1] as f64, self.up[2] as f64];
        [
            f[1] * u[2] - f[2] * u[1],
            f[2] * u[0] - f[0] * u[2],
            f[0] * u[1] - f[1] * u[0],
        ]
    }
}

/// `Camera.setRotation`'s basis — the forward and up vectors for a yaw/pitch in
/// **degrees**, as `camera.forwardVector()` / `camera.upVector()` return them.
///
/// ```java
/// this.rotation.rotationYXZ((float)Math.PI - yRot * (float)(Math.PI / 180.0),
///                           -xRot * (float)(Math.PI / 180.0), 0.0F);
/// FORWARDS.rotate(this.rotation, this.forwards);   // FORWARDS = (0, 0, -1)
/// UP.rotate(this.rotation, this.up);               // UP       = (0, 1, 0)
/// ```
/// (`Camera.java:337-342`; the constants at `:42-44`.)
///
/// JOML's `rotationYXZ(y, x, z)` is `rotationY(y).rotateX(x).rotateZ(z)`, so the
/// rotation is `Ry(pi - yaw) * Rx(-pitch)` with the z term identically zero.
/// Composing that by hand and simplifying with `sin(pi - a) = sin a` and
/// `cos(pi - a) = -cos a` collapses both vectors to closed forms, with no
/// quaternion left in them:
///
/// ```text
/// forward = (-cos(pitch) * sin(yaw), -sin(pitch),  cos(pitch) * cos(yaw))
/// up      = (-sin(pitch) * sin(yaw),  cos(pitch),  sin(pitch) * cos(yaw))
/// ```
///
/// **The forward vector is exactly `Entity.calculateViewVector`**, and that is
/// the independent check on the whole derivation rather than a nice-to-know:
/// that method reaches `(sin(-yaw)cos(pitch), -sin(pitch), cos(-yaw)cos(pitch))`
/// by a completely different route, and the two agree identically. `up` has no
/// such twin, so it is pinned by the composition instead.
///
/// **`up` is not the constant `(0, 1, 0)`**, which is the tempting
/// simplification and is wrong the moment the player looks off the horizon: at
/// pitch 90 the forward vector points straight down and `up` becomes the
/// horizontal heading. A listener whose up never tilts has a stereo image that
/// refuses to roll when you look up, and nothing about that looks wrong in a log.
pub fn listener_basis(yaw_deg: f32, pitch_deg: f32) -> ([f32; 3], [f32; 3]) {
    let yaw = yaw_deg.to_radians();
    let pitch = pitch_deg.to_radians();
    let (sy, cy) = (yaw.sin(), yaw.cos());
    let (sp, cp) = (pitch.sin(), pitch.cos());
    ([-cp * sy, -sp, cp * cy], [-sp * sy, cp, sp * cy])
}

/// `Library.CountingChannelPool` plus `Library.init`'s sizing.
#[derive(Clone, Debug)]
pub struct ChannelBudget {
    static_limit: i32,
    streaming_limit: i32,
    static_used: i32,
    streaming_used: i32,
}

/// `Library.init`'s split of the device's mono-source count into the two
/// pools.
///
/// ```text
/// streaming = clamp((int)Mth.sqrt(total), 2, 8)
/// static    = clamp(total - streaming, 8, 255)
/// ```
///
/// Two things invert if guessed. `Mth.sqrt` is `(float)Math.sqrt`, so the cast
/// **truncates**: 30 sources give `(int)5.477 = 5` streaming, not 6, and 25
/// static. And **the two limits do not have to sum to `total`** — the static
/// clamp's *lower* bound is 8, so a device advertising four sources yields a
/// budget of 2 streaming plus 8 static, ten channels from a device that
/// offered four. Vanilla over-subscribes there deliberately (the alternative
/// is a client that can play two sounds), and reproducing it matters because
/// the sum is what a "how many voices do I have" test would naturally assert.
pub fn pool_sizes(total_channel_count: i32) -> (i32, i32) {
    let streaming = ((total_channel_count as f32).sqrt() as i32).clamp(2, 8);
    let static_count = (total_channel_count - streaming).clamp(8, 255);
    (static_count, streaming)
}

impl ChannelBudget {
    /// Size the two pools from a device's advertised mono-source count.
    pub fn from_channel_count(total: i32) -> ChannelBudget {
        let (static_limit, streaming_limit) = pool_sizes(total);
        ChannelBudget {
            static_limit,
            streaming_limit,
            static_used: 0,
            streaming_used: 0,
        }
    }

    pub fn limit(&self, pool: Pool) -> i32 {
        match pool {
            Pool::Static => self.static_limit,
            Pool::Streaming => self.streaming_limit,
        }
    }

    pub fn used(&self, pool: Pool) -> i32 {
        match pool {
            Pool::Static => self.static_used,
            Pool::Streaming => self.streaming_used,
        }
    }

    /// `CountingChannelPool.acquire()` — `if (size >= limit) return null`.
    ///
    /// There is **no eviction anywhere in this path**. The natural expectation
    /// for a voice budget is voice stealing; vanilla has none, and the sound
    /// that loses is the *newest*.
    pub fn acquire(&mut self, pool: Pool) -> bool {
        let (used, limit) = match pool {
            Pool::Static => (&mut self.static_used, self.static_limit),
            Pool::Streaming => (&mut self.streaming_used, self.streaming_limit),
        };
        if *used >= limit {
            return false;
        }
        *used += 1;
        true
    }

    /// `CountingChannelPool.release(channel)`.
    pub fn release(&mut self, pool: Pool) {
        let used = match pool {
            Pool::Static => &mut self.static_used,
            Pool::Streaming => &mut self.streaming_used,
        };
        *used = (*used - 1).max(0);
    }

    /// `Library.getChannelDebugString()` — `"Sounds: %d/%d + %d/%d"`.
    pub fn debug_string(&self) -> String {
        format!(
            "Sounds: {}/{} + {}/{}",
            self.static_used, self.static_limit, self.streaming_used, self.streaming_limit
        )
    }
}

impl Default for ChannelBudget {
    fn default() -> Self {
        ChannelBudget::from_channel_count(DEFAULT_CHANNEL_COUNT)
    }
}

/// The id-and-pool bookkeeping every [`AudioDevice`] needs, whatever it does
/// with the samples — `Library`'s half of the job, minus OpenAL.
#[derive(Clone, Debug, Default)]
pub struct ChannelAllocator {
    budget: ChannelBudget,
    next_id: ChannelId,
    /// Which pool each live channel came from, so `release` returns it to the
    /// right one. Vanilla's `Library.releaseChannel` finds this by asking both
    /// pools in turn and **throws** if neither owns the channel.
    pools: HashMap<ChannelId, Pool>,
    /// `acquire` returned `None` this many times — the dropped-sound counter,
    /// which vanilla only logs (and only when running in an IDE).
    pub refusals: u32,
}

impl ChannelAllocator {
    pub fn with_channel_count(total: i32) -> ChannelAllocator {
        ChannelAllocator {
            budget: ChannelBudget::from_channel_count(total),
            ..Default::default()
        }
    }

    pub fn budget(&self) -> &ChannelBudget {
        &self.budget
    }

    pub fn acquire(&mut self, pool: Pool) -> Option<ChannelId> {
        if !self.budget.acquire(pool) {
            self.refusals += 1;
            return None;
        }
        let id = self.next_id;
        self.next_id += 1;
        self.pools.insert(id, pool);
        Some(id)
    }

    pub fn release(&mut self, channel: ChannelId) {
        if let Some(pool) = self.pools.remove(&channel) {
            self.budget.release(pool);
        }
    }
}

/// An [`AudioDevice`] that makes no sound and records what it was asked to do.
///
/// This is the whole verification strategy for the parts of audio a machine
/// can grade: the engine's output *is* a call sequence, so a sink that keeps
/// the sequence lets a test assert it exactly. It carries a real
/// [`ChannelBudget`], so the no-eviction rule is exercised rather than assumed.
#[derive(Clone, Debug, Default)]
pub struct RecordingDevice {
    alloc: ChannelAllocator,
    /// Every call, in order, with the channel it went to.
    pub calls: Vec<(ChannelId, ChannelCall)>,
    /// Channels the test has declared finished, for the reclaim path.
    stopped: Vec<ChannelId>,
    /// Every listener transform pushed, in order (M138a).
    ///
    /// A history rather than a latest-value: the claims worth asserting are
    /// mostly about the *sequence* — that it is pushed at all, that a turn
    /// changes it, that it arrives per frame rather than per tick.
    pub listener_history: Vec<ListenerTransform>,
}

impl RecordingDevice {
    pub fn with_channel_count(total: i32) -> RecordingDevice {
        RecordingDevice {
            alloc: ChannelAllocator::with_channel_count(total),
            ..Default::default()
        }
    }

    pub fn budget(&self) -> &ChannelBudget {
        self.alloc.budget()
    }

    /// How many `acquire` calls were refused for want of a channel.
    pub fn refusals(&self) -> u32 {
        self.alloc.refusals
    }

    /// Pretend the device finished playing this channel, so the engine's
    /// reclaim path can be driven. Vanilla learns this from
    /// `AL_SOURCE_STATE == AL_STOPPED`.
    pub fn finish(&mut self, channel: ChannelId) {
        if !self.stopped.contains(&channel) {
            self.stopped.push(channel);
        }
    }

    /// The calls made to one channel, in order.
    pub fn calls_to(&self, channel: ChannelId) -> Vec<ChannelCall> {
        self.calls
            .iter()
            .filter(|(c, _)| *c == channel)
            .map(|(_, call)| call.clone())
            .collect()
    }

    pub fn clear_calls(&mut self) {
        self.calls.clear();
    }
}

impl AudioDevice for RecordingDevice {
    fn acquire(&mut self, pool: Pool) -> Option<ChannelId> {
        self.alloc.acquire(pool)
    }

    fn release(&mut self, channel: ChannelId) {
        self.alloc.release(channel);
        self.stopped.retain(|c| *c != channel);
    }

    fn submit(&mut self, channel: ChannelId, call: ChannelCall) {
        self.calls.push((channel, call));
    }

    fn stopped(&self, channel: ChannelId) -> bool {
        self.stopped.contains(&channel)
    }

    fn set_listener(&mut self, transform: ListenerTransform) {
        self.listener_history.push(transform);
    }
}

/// The [`AudioDevice`] production uses **until there is a real one**.
///
/// It allocates and frees channels through a real [`ChannelBudget`] and throws
/// every call away. Its one non-obvious choice is [`AudioDevice::stopped`]:
/// with no device there is no `AL_SOURCE_STATE` to read, and answering `false`
/// would mean nothing is ever reclaimed, the pool fills after 25 sounds and
/// the engine wedges. Answering `true` makes every channel read as finished,
/// so the engine reclaims it once the [`MIN_SOURCE_LIFETIME`] grace period
/// expires — which keeps the budget, the grace period and the manual-loop
/// requeue all exercised on the live path rather than only in tests.
///
/// The consequence to know: under this device every sound behaves as if it
/// were shorter than 20 ticks, so a *long* sound's real lifetime is not
/// modelled. Nothing here could model it — a sound's length is in its `.ogg`.
#[derive(Clone, Debug, Default)]
pub struct SilentDevice {
    /// How many listener transforms have arrived (M138a).
    ///
    /// Separate from `calls_made` because the claim r45 makes is a *rate* —
    /// one per frame — and mixing it with per-channel calls would make the
    /// number depend on how many sounds happened to be playing.
    pub listener_pushes: u64,
    /// The most recent one, so a gate can assert on the value delivered.
    pub last_listener: Option<ListenerTransform>,
    alloc: ChannelAllocator,
    /// Every call this device was asked to make. A pure counter — the calls
    /// themselves are dropped.
    pub calls_made: u64,
}

impl SilentDevice {
    pub fn with_channel_count(total: i32) -> SilentDevice {
        SilentDevice {
            alloc: ChannelAllocator::with_channel_count(total),
            ..Default::default()
        }
    }

    pub fn budget(&self) -> &ChannelBudget {
        self.alloc.budget()
    }

    pub fn refusals(&self) -> u32 {
        self.alloc.refusals
    }
}

impl AudioDevice for SilentDevice {
    fn acquire(&mut self, pool: Pool) -> Option<ChannelId> {
        self.alloc.acquire(pool)
    }
    fn release(&mut self, channel: ChannelId) {
        self.alloc.release(channel);
    }
    fn submit(&mut self, _channel: ChannelId, _call: ChannelCall) {
        self.calls_made += 1;
    }
    fn stopped(&self, _channel: ChannelId) -> bool {
        true
    }
    fn set_listener(&mut self, transform: ListenerTransform) {
        // Counted like every other call, so the live path can show the listener
        // is being pushed without a device existing to hear it. A device that
        // threw this away silently would make M138d's first symptom "no sound"
        // with nothing upstream to look at.
        self.calls_made += 1;
        // Recorded, so `--render-check`'s r45 can read back what the device was
        // handed instead of recomputing it from the session — a witness that
        // re-derives the value it is checking grades the derivation twice and
        // the delivery not at all (M88's r20).
        self.listener_pushes += 1;
        self.last_listener = Some(transform);
    }
}

/// What the engine needs to know about the world to play and tick a sound.
///
/// One question of its own — `isSilent()` — over
/// [`crate::tickable::RampWorld`], which is everything the ten `tick()` bodies
/// read.
///
/// **`entity_position` used to live here and is now `RampWorld::position`.**
/// They were the same query (`entity.isRemoved()` folded into an `Option`), and
/// two names for one query is exactly how call sites come to disagree — M89's
/// finding, which has since recurred three times. One name, one implementor
/// method, and a consumer that wants it has to ask for it.
pub trait SoundWorld: crate::tickable::RampWorld {
    /// `entity.isSilent()` — `SynchedEntityData` index 4.
    ///
    /// **Decoded since M138a.** This doc used to say "Rewo has no source for
    /// this yet … every production caller answers `false`", which was the
    /// honest shape while the metadata parser skipped index 4 and stopped
    /// being true the moment it did not. The production implementor reads the
    /// decoded flag and says so at its own definition; this one is where a
    /// reader looks first, so it is the one that matters.
    fn entity_silent(&self, entity_id: i32) -> bool;
}

/// A [`SoundWorld`] with nothing in it — every entity is gone.
#[derive(Clone, Copy, Debug, Default)]
pub struct EmptyWorld;

impl SoundWorld for EmptyWorld {
    fn entity_silent(&self, _entity_id: i32) -> bool {
        false
    }
}

impl crate::tickable::RampWorld for EmptyWorld {
    fn position(&self, _entity_id: i32) -> Option<(f64, f64, f64)> {
        None
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

/// `SoundEngine.PlayResult`, plus the reason.
///
/// Vanilla's enum has three values and throws the reason away into a log line.
/// The reason is kept here because the eight `NOT_STARTED` paths are
/// behaviourally different — "the pool was full" and "the pack has no such
/// event" both silence a sound and want completely different fixes — and
/// because a test asserting *which* one fired is a much sharper witness than
/// one asserting that nothing played.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlayResult {
    Started,
    /// `volume == 0` but the sound was allowed to start anyway — music, or an
    /// instance that sets `canStartSilent`.
    StartedSilently,
    NotStarted(NotStarted),
}

/// Why `SoundEngine.play` returned `NOT_STARTED`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotStarted {
    /// `if (!this.loaded)` — the library never came up.
    NotLoaded,
    /// `!instance.canPlaySound()` — the bound entity is silent.
    CannotPlay,
    /// `resolve` returned null: no `sounds.json` entry for this event.
    UnknownEvent,
    /// `sound == INTENTIONALLY_EMPTY_SOUND` — silence by design, and the one
    /// no-play path vanilla does **not** warn about.
    IntentionallyEmpty,
    /// `sound == EMPTY_SOUND` — an event that exists but resolved to nothing
    /// (no variants survived, or the total weight was zero).
    EmptySound,
    /// `volume == 0` and neither `canStartSilent` nor `SoundSource.MUSIC`.
    SilentAndNotAllowed,
    /// `channelAccess.createHandle(...)` gave `null` — the pool was full.
    NoChannel,
}

/// One instance the engine is holding a channel for.
///
/// Vanilla keeps this across `instanceToChannel`, `soundDeleteTime`,
/// `instanceBySource` and `tickingSounds` — four containers keyed by object
/// identity. They are one record here, which is a genuine simplification and
/// is safe for a reason worth writing down: **the four are written and erased
/// together at every site**, so their key sets are equal at all times. That
/// equality is what makes `SoundEngine.isActive`'s first branch inert (see
/// [`SoundEngine::is_active`]).
#[derive(Clone, Debug)]
struct Live {
    id: InstanceId,
    instance: SoundInstance,
    channel: ChannelId,
    pool: Pool,
    /// `soundDeleteTime` — `tickCount + MIN_SOURCE_LIFETIME` at play.
    delete_time: i32,
    /// `ChannelAccess.ChannelHandle.stopped`, set by [`SoundEngine::schedule_tick`].
    handle_stopped: bool,
    /// Which `TickableSoundInstance` subclass this is, if any — `None` is a
    /// plain `SimpleSoundInstance`, which never enters `tickingSounds`.
    ///
    /// Vanilla's `tickingSounds` is a separate set and its membership test is
    /// `instance instanceof TickableSoundInstance`; here the ramp's presence
    /// *is* that test, which is the same partition by construction rather than
    /// by two collections agreeing.
    ramp: Option<crate::tickable::Ramp>,
}

/// A handle standing in for vanilla's object identity.
///
/// `Map<SoundInstance, …>` with no `equals` override is an identity map, so
/// two equal-valued instances are two entries. A monotonic id reproduces that
/// exactly, where keying by the instance's value would collapse a doorbell
/// rung twice into one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct InstanceId(pub u64);

/// `net.minecraft.client.sounds.SoundEngine`.
#[derive(Clone, Debug)]
pub struct SoundEngine {
    loaded: bool,
    tick_count: i32,
    next_id: u64,
    /// `instanceToChannel` + `soundDeleteTime` + `instanceBySource` +
    /// `tickingSounds` — see [`Live`].
    live: Vec<Live>,
    /// `queuedSounds` — delayed and manually-relooped instances, with the tick
    /// they are due.
    queued: Vec<(InstanceId, SoundInstance, i32)>,
    /// `queuedTickableSounds`.
    queued_tickable: Vec<(SoundInstance, Option<crate::tickable::Ramp>)>,
    /// `gainBySource`, `defaultReturnValue(1.0F)`.
    gain_by_source: [f32; SoundSource::ALL.len()],
    /// `Options.soundSourceVolumes`.
    pub options: CategoryVolumes,
}

impl Default for SoundEngine {
    fn default() -> Self {
        SoundEngine {
            // `loaded` is false until `loadLibrary` succeeds; a Rewo engine
            // with a device attached is the equivalent, and the default is
            // "there is a device" so a caller does not have to remember.
            loaded: true,
            tick_count: 0,
            next_id: 0,
            live: Vec::new(),
            queued: Vec::new(),
            queued_tickable: Vec::new(),
            gain_by_source: [1.0; SoundSource::ALL.len()],
            options: CategoryVolumes::default(),
        }
    }
}

impl SoundEngine {
    pub fn new() -> SoundEngine {
        SoundEngine::default()
    }

    /// `SoundEngine.loaded` — false when the library failed to start. Vanilla
    /// logs "Turning off sounds & music" and every entry point returns early.
    pub fn set_loaded(&mut self, loaded: bool) {
        self.loaded = loaded;
    }

    pub fn tick_count(&self) -> i32 {
        self.tick_count
    }

    /// How many instances hold a channel — `instanceToChannel.size()`.
    pub fn live_count(&self) -> usize {
        self.live.len()
    }

    /// The event names of the instances currently holding a channel.
    ///
    /// Exists so a witness can name *which* sound is playing rather than
    /// counting how many are — the bee's switch replaces one loop with another
    /// and a count cannot see it happen.
    /// `BiomeAmbientSoundsHandler.tick`'s loop transition, applied to the live
    /// set (M142d). Returns the sound that still needs creating, if any.
    ///
    /// Vanilla's handler keeps its own `Object2ObjectArrayMap` of
    /// `LoopSoundInstance`s and calls `fadeOut()` / `fadeIn()` on them; the
    /// engine's live set **is** that map here, filtered to biome-loop ramps,
    /// which is why the reuse falls out rather than being arranged.
    ///
    /// Three things are transcribed rather than tidied:
    ///
    /// * **`removeIf(isStopped)` runs FIRST**, before the change test. The
    ///   engine already retires a stopped sound, so the equivalent here is
    ///   that only *live* entries are visible — and that ordering is what
    ///   guarantees the reuse below can never hand back a stopped instance and
    ///   `fadeIn()` it, reviving an object whose channel is gone.
    /// * **Every loop fades out, including the incoming one.** That reads like
    ///   a bug and is load-bearing: `fade_out`'s `min(fade, 40)` is the *only*
    ///   place a runaway `fade` is capped, and the ramp's tick never bounds it
    ///   upward. Skipping it leaves an instance that has been playing for ten
    ///   minutes re-entering with `fade == 12000`, and then taking 12000 ticks
    ///   to fade out next time. The net composition on the incoming loop is
    ///   `fade = clamp(fade, 0, 40); direction = +1`.
    /// * **A live instance is REUSED.** Crossing back inside the ~41 ticks it
    ///   takes to die finds it still here, so there is no second `play`, no
    ///   channel re-attach and no restart of the sample — `fade_in()` just
    ///   reverses the ramp from wherever it got to. Creating a fresh instance
    ///   instead restarts the audio from offset 0.
    pub fn apply_biome_loop_transition(&mut self, current: Option<&str>) -> Option<String> {
        for live in &mut self.live {
            if let Some(crate::tickable::Ramp::BiomeLoop(b)) = &mut live.ramp {
                b.fade_out();
            }
        }
        let id = current?;
        let existing = self.live.iter_mut().find(|l| {
            matches!(l.ramp, Some(crate::tickable::Ramp::BiomeLoop(_)))
                && l.instance.identifier == id
        });
        match existing {
            Some(live) => {
                if let Some(crate::tickable::Ramp::BiomeLoop(b)) = &mut live.ramp {
                    b.fade_in();
                }
                None
            }
            // Nothing live under that identifier: the caller plays one.
            None => Some(id.to_string()),
        }
    }

    /// The `fade` of the live biome loop with this identifier, for tests.
    /// `None` if no such loop is live — which is itself the fact several
    /// witnesses turn on, since a reused instance stays live and a replaced
    /// one would not.
    pub fn biome_fade(&self, identifier: &str) -> Option<i32> {
        self.live.iter().find_map(|l| match &l.ramp {
            Some(crate::tickable::Ramp::BiomeLoop(b)) if l.instance.identifier == identifier => {
                Some(b.fade)
            }
            _ => None,
        })
    }

    pub fn live_identifiers(&self) -> Vec<&str> {
        self.live.iter().map(|l| l.instance.identifier.as_str()).collect()
    }

    /// `SoundEngine.updateCategoryVolume(source, gain)` — the runtime gain,
    /// **clamped on the way in**, which is why `calculate_volume` reads it raw.
    pub fn update_category_volume(&mut self, source: SoundSource, gain: f32) {
        self.gain_by_source[source.ordinal() as usize] =
            crate::sound_instance::mth_clamp(gain, 0.0, 1.0);
    }

    pub fn category_gain(&self, source: SoundSource) -> f32 {
        self.gain_by_source[source.ordinal() as usize]
    }

    /// `SoundEngine.isActive(instance)`.
    ///
    /// Vanilla writes
    /// `soundDeleteTime.containsKey(i) && soundDeleteTime.get(i) <= tickCount
    /// ? true : instanceToChannel.containsKey(i)`, and **the first branch is
    /// inert**: the two maps are written and erased together at every site, so
    /// whenever `soundDeleteTime` contains the key `instanceToChannel` does
    /// too and the second branch would return `true` anyway. The whole
    /// expression reduces to `instanceToChannel.containsKey`. That is a
    /// provable no-op rather than a reading — see the test.
    pub fn is_active(&self, id: InstanceId) -> bool {
        self.live.iter().any(|l| l.id == id)
    }

    fn resolve(
        instance: &SoundInstance,
        sounds: &SoundsIndex,
    ) -> Option<ResolvedSound> {
        match instance.seed {
            Some(seed) => sounds.get_sound_seeded(&instance.identifier, seed),
            // `SoundInstance.createUnseededRandom()`. There is no wire seed to
            // reproduce, so any generator is as faithful as any other; a fixed
            // one keeps this crate deterministic and free of a clock. A device
            // milestone that wants real variety can seed it per play.
            None => sounds.get_sound_seeded(&instance.identifier, 0),
        }
    }

    /// `SoundEngine.play(SoundInstance)`.
    ///
    /// The guard order is vanilla's and is load-bearing at two points:
    /// `canPlaySound` is tested **before** the event is resolved (so a silent
    /// entity costs no lookup and draws no random number), and the
    /// intentionally-empty check comes **before** the empty-sound check (so
    /// the deliberate silence does not log a warning while the accidental one
    /// does).
    pub fn play(
        &mut self,
        instance: SoundInstance,
        sounds: &SoundsIndex,
        world: &dyn SoundWorld,
        device: &mut dyn AudioDevice,
    ) -> (InstanceId, PlayResult) {
        let ramp = crate::tickable::Ramp::for_instance(&instance);
        self.play_ramped(instance, ramp, sounds, world, device)
    }

    /// `SoundEngine.play(SoundInstance)` for a `TickableSoundInstance`.
    ///
    /// The ramp is what `tickingSounds` membership means here — see
    /// [`Live::ramp`]. Everything else is identical, which is why this is the
    /// real body and [`SoundEngine::play`] is the wrapper: a second copy of the
    /// eleven guards would be eleven chances to drift.
    pub fn play_ramped(
        &mut self,
        instance: SoundInstance,
        ramp: Option<crate::tickable::Ramp>,
        sounds: &SoundsIndex,
        world: &dyn SoundWorld,
        device: &mut dyn AudioDevice,
    ) -> (InstanceId, PlayResult) {
        let id = InstanceId(self.next_id);
        self.next_id += 1;

        if !self.loaded {
            return (id, PlayResult::NotStarted(NotStarted::NotLoaded));
        }

        let silent = match instance.binding {
            Binding::Entity(e) => world.entity_silent(e),
            Binding::Fixed => false,
        };
        if !instance.can_play_sound(silent) {
            return (id, PlayResult::NotStarted(NotStarted::CannotPlay));
        }

        // Vanilla asks two questions here and M66's index answers them with
        // one `Option`, so the *reason* is recovered by asking the index
        // whether the event exists at all:
        //
        //   `soundEvent == null`             → the pack has no such event;
        //   `sound == EMPTY_SOUND`           → it has one, and it resolved to
        //                                      nothing (no surviving variant,
        //                                      or a total weight of zero).
        //
        // Both are `NOT_STARTED` with a warning in vanilla, so collapsing them
        // would be behaviourally right and would throw away which fix applies.
        //
        // What is NOT done here is comparing the resolved *name* against
        // `minecraft:empty`. Vanilla's test is `sound == SoundManager
        // .EMPTY_SOUND` — reference equality against a singleton — so a pack
        // that genuinely declares a file named `minecraft:empty` is **played**,
        // not skipped. A name comparison would silence it.
        let resolved = match Self::resolve(&instance, sounds) {
            Some(r) if r.is_intentionally_empty() => {
                // The one no-play path vanilla does not warn about.
                return (id, PlayResult::NotStarted(NotStarted::IntentionallyEmpty));
            }
            Some(r) => r,
            None if sounds.get(&instance.identifier).is_some() => {
                return (id, PlayResult::NotStarted(NotStarted::EmptySound));
            }
            None => return (id, PlayResult::NotStarted(NotStarted::UnknownEvent)),
        };

        // `float instanceVolume = instance.getVolume();` — called ONCE and
        // used for both roles. It is not a pure getter in vanilla (it samples
        // the `SampledFloat`), so calling it twice would be a second draw.
        let inst_volume = instance_volume(instance.volume, resolved.volume);
        let atten_distance = attenuation_distance(inst_volume, resolved.attenuation_distance);
        let gain = calculate_volume(
            inst_volume,
            instance.source,
            &self.options,
            self.category_gain(instance.source),
        );
        let pitch = calculate_pitch(instance_pitch(instance.pitch, resolved.pitch));

        let mut started_silently = false;
        if gain == 0.0 {
            if !instance.can_start_silent && instance.source != SoundSource::Music {
                return (id, PlayResult::NotStarted(NotStarted::SilentAndNotAllowed));
            }
            started_silently = true;
        }

        let looping = instance.should_loop_automatically();
        let streaming = resolved.stream;
        let pool = if streaming { Pool::Streaming } else { Pool::Static };
        let Some(channel) = device.acquire(pool) else {
            return (id, PlayResult::NotStarted(NotStarted::NoChannel));
        };

        // The call order is `handle.execute`'s lambda body, verbatim.
        device.submit(channel, ChannelCall::SetPitch(pitch));
        device.submit(channel, ChannelCall::SetVolume(gain));
        match instance.attenuation {
            Attenuation::Linear => {
                device.submit(channel, ChannelCall::LinearAttenuation(atten_distance))
            }
            Attenuation::None => device.submit(channel, ChannelCall::DisableAttenuation),
        }
        // `channel.setLooping(isLooping && !isStreaming)` — a streaming loop is
        // looped by the *stream*, not by the source, so the flag is cleared
        // here and passed to `getStream` instead.
        device.submit(channel, ChannelCall::SetLooping(looping && !streaming));
        device.submit(
            channel,
            ChannelCall::SetSelfPosition(instance.x, instance.y, instance.z),
        );
        device.submit(channel, ChannelCall::SetRelative(instance.relative));

        let path = resolved.asset_path();
        if streaming {
            device.submit(channel, ChannelCall::AttachBufferStream(path, looping));
        } else {
            device.submit(channel, ChannelCall::AttachStaticBuffer(path));
        }
        device.submit(channel, ChannelCall::Play);

        self.live.push(Live {
            id,
            instance,
            channel,
            pool,
            delete_time: self.tick_count + MIN_SOURCE_LIFETIME,
            handle_stopped: false,
            ramp,
        });

        (
            id,
            if started_silently {
                PlayResult::StartedSilently
            } else {
                PlayResult::Started
            },
        )
    }

    /// `SoundEngine.playDelayed(instance, delay)`.
    pub fn play_delayed(&mut self, instance: SoundInstance, delay: i32) -> InstanceId {
        let id = InstanceId(self.next_id);
        self.next_id += 1;
        self.queued.push((id, instance, self.tick_count + delay));
        id
    }

    /// `SoundEngine.queueTickingSound(instance)` — played at the top of the
    /// next tick, and only if it still `canPlaySound()` then.
    pub fn queue_ticking_sound(
        &mut self,
        instance: SoundInstance,
        ramp: Option<crate::tickable::Ramp>,
    ) {
        self.queued_tickable.push((instance, ramp));
    }

    /// `SoundEngine.stop(SoundInstance)` — asks the device to stop the
    /// channel and **changes no bookkeeping**. The instance is reclaimed by a
    /// later tick, no sooner than [`MIN_SOURCE_LIFETIME`] after it started.
    pub fn stop(&mut self, id: InstanceId, device: &mut dyn AudioDevice) {
        if !self.loaded {
            return;
        }
        if let Some(l) = self.live.iter().find(|l| l.id == id) {
            let ch = l.channel;
            device.submit(ch, ChannelCall::Stop);
        }
    }

    /// `SoundEngine.stop(@Nullable Identifier, @Nullable SoundSource)` — what
    /// `ClientboundStopSoundPacket` reaches.
    ///
    /// Three arms, and the middle one is the whole reason `stop_sound`'s flags
    /// byte matters: **naming neither a sound nor a category is `stopAll`**,
    /// not a no-op. The other asymmetry is which container is walked — with a
    /// category it is `instanceBySource`, without one it is
    /// `instanceToChannel` — which makes no difference here because both are
    /// [`Live`], and is recorded so the reduction is visible rather than
    /// accidental.
    pub fn stop_matching(
        &mut self,
        name: Option<&str>,
        source: Option<SoundSource>,
        device: &mut dyn AudioDevice,
    ) {
        if !self.loaded {
            return;
        }
        match (name, source) {
            (_, Some(src)) => {
                let targets: Vec<ChannelId> = self
                    .live
                    .iter()
                    .filter(|l| l.instance.source == src)
                    .filter(|l| name.is_none_or(|n| l.instance.identifier == n))
                    .map(|l| l.channel)
                    .collect();
                for ch in targets {
                    device.submit(ch, ChannelCall::Stop);
                }
            }
            (None, None) => self.stop_all(device),
            (Some(n), None) => {
                let targets: Vec<ChannelId> = self
                    .live
                    .iter()
                    .filter(|l| l.instance.identifier == n)
                    .map(|l| l.channel)
                    .collect();
                for ch in targets {
                    device.submit(ch, ChannelCall::Stop);
                }
            }
        }
    }

    /// `SoundEngine.stopAll()`.
    ///
    /// Note the last line of vanilla's body: `this.gainBySource.clear()`. On a
    /// map whose `defaultReturnValue` is `1.0F` that **resets every runtime
    /// category gain to full** — so a `/stopsound` with no arguments, arriving
    /// while music is halfway through a fade, snaps the music gain back to 1.
    /// It is a side effect of a "stop everything" call and is transcribed
    /// rather than tidied.
    pub fn stop_all(&mut self, device: &mut dyn AudioDevice) {
        if !self.loaded {
            return;
        }
        for l in std::mem::take(&mut self.live) {
            device.release(l.channel);
        }
        self.queued.clear();
        self.queued_tickable.clear();
        self.gain_by_source = [1.0; SoundSource::ALL.len()];
    }

    /// `ChannelAccess.scheduleTick()` — ask the device which channels have
    /// finished, mark their handles stopped and release them.
    ///
    /// Vanilla runs this at the **end** of `tick`, so the reclaim loop at the
    /// top of the *next* tick is what sees the flag. That one-tick lag is real
    /// and is reproduced: a sound cannot be reclaimed on the same tick its
    /// channel stopped.
    fn schedule_tick(&mut self, device: &mut dyn AudioDevice) {
        let finished: Vec<(usize, ChannelId, Pool)> = self
            .live
            .iter()
            .enumerate()
            .filter(|(_, l)| !l.handle_stopped && device.stopped(l.channel))
            .map(|(i, l)| (i, l.channel, l.pool))
            .collect();
        for (i, ch, _pool) in finished {
            self.live[i].handle_stopped = true;
            device.release(ch);
        }
    }

    /// `SoundEngine.tick(paused)`.
    pub fn tick(
        &mut self,
        paused: bool,
        sounds: &SoundsIndex,
        world: &dyn SoundWorld,
        device: &mut dyn AudioDevice,
    ) {
        if paused {
            self.tick_music_when_paused();
        } else {
            self.tick_in_game_sound(sounds, world, device);
        }
        self.schedule_tick(device);
    }

    /// `SoundEngine.tickInGameSound()`.
    fn tick_in_game_sound(
        &mut self,
        sounds: &SoundsIndex,
        world: &dyn SoundWorld,
        device: &mut dyn AudioDevice,
    ) {
        self.tick_count += 1;

        // `queuedTickableSounds.stream().filter(canPlaySound).forEach(play)`.
        for (inst, ramp) in std::mem::take(&mut self.queued_tickable) {
            let silent = match inst.binding {
                Binding::Entity(e) => world.entity_silent(e),
                Binding::Fixed => false,
            };
            if inst.can_play_sound(silent) {
                self.play_ramped(inst, ramp, sounds, world, device);
            }
        }

        // ```java
        // for (TickableSoundInstance instance : this.tickingSounds) {
        //    if (!instance.canPlaySound()) this.stop(instance);
        //    instance.tick();
        //    if (instance.isStopped()) this.stop(instance);
        //    else { volume/pitch/position -> the channel }
        // }
        // ```
        //
        // Three shapes worth keeping visible. The `canPlaySound` stop does NOT
        // `continue` — vanilla stops the channel and then ticks the instance
        // anyway. `tick()` is called before `isStopped()` is read, so a body
        // that stops itself this tick still runs. And the volume/pitch/position
        // push happens **only** in the else, so a stopping instance's last
        // ramp write never reaches the device.
        let options = self.options;
        let gains = self.gain_by_source;
        let mut to_stop: Vec<ChannelId> = Vec::new();
        let mut updates: Vec<(ChannelId, f32, f32, (f64, f64, f64))> = Vec::new();
        let mut queued: Vec<(SoundInstance, Option<crate::tickable::Ramp>)> = Vec::new();
        for l in self.live.iter_mut() {
            let Some(ramp) = l.ramp.as_mut() else {
                continue;
            };
            // `if (!instance.canPlaySound()) this.stop(instance);` — and
            // that is the RAMP's predicate, not the followed entity: four of
            // the ten classes take the interface default and are never
            // silence-gated. See `Ramp::silence_gated_entity`.
            if let Some(entity) = ramp.silence_gated_entity() {
                if world.entity_silent(entity) {
                    to_stop.push(l.channel);
                }
            }

            let outcome = ramp.tick(&mut l.instance, world);
            if let Some((inst, next)) = outcome.queued {
                queued.push((inst, Some(next)));
            }
            if outcome.stopped {
                // `AbstractTickableSoundInstance.stop()` sets `stopped` and
                // **clears `looping`** — so a stopped instance does not come
                // back through the manual-loop requeue.
                l.instance.looping = false;
                to_stop.push(l.channel);
                continue;
            }
            updates.push((
                l.channel,
                calculate_volume(
                    l.instance.volume,
                    l.instance.source,
                    &options,
                    gains[l.instance.source.ordinal() as usize],
                ),
                calculate_pitch(l.instance.pitch),
                (l.instance.x, l.instance.y, l.instance.z),
            ));
        }
        // `SoundManager.queueTickingSound` from inside a `tick()` — the bee's
        // switch. Vanilla's `queuedTickableSounds` is drained at the TOP of a
        // tick, so a replacement queued here first plays on the NEXT one, and
        // there is no tick in which both bee loops are live.
        self.queued_tickable.extend(queued);
        for ch in to_stop {
            device.submit(ch, ChannelCall::Stop);
        }
        for (ch, vol, pitch, pos) in updates {
            device.submit(ch, ChannelCall::SetVolume(vol));
            device.submit(ch, ChannelCall::SetPitch(pitch));
            device.submit(ch, ChannelCall::SetSelfPosition(pos.0, pos.1, pos.2));
        }

        // The reclaim loop: a stopped handle whose grace period has expired.
        let tick_count = self.tick_count;
        let mut requeue: Vec<(InstanceId, SoundInstance, i32)> = Vec::new();
        self.live.retain(|l| {
            if !l.handle_stopped || l.delete_time > tick_count {
                return true;
            }
            if l.instance.should_loop_manually() {
                requeue.push((l.id, l.instance.clone(), tick_count + l.instance.delay));
            }
            false
        });
        self.queued.extend(requeue);

        // The delayed queue.
        let due: Vec<(InstanceId, SoundInstance)> = {
            let mut due = Vec::new();
            self.queued.retain(|(id, inst, at)| {
                if tick_count >= *at {
                    due.push((*id, inst.clone()));
                    false
                } else {
                    true
                }
            });
            due
        };
        for (_, inst) in due {
            self.play(inst, sounds, world, device);
        }
    }

    /// `SoundEngine.tickMusicWhenPaused()` — the only thing that happens while
    /// the game is paused is that finished **music** is reclaimed.
    ///
    /// Everything else keeps its channel: `tickCount` does not advance, no
    /// delayed sound comes due, and no ticking sound follows its entity. That
    /// is why a paused game holds its voices.
    fn tick_music_when_paused(&mut self) {
        self.live
            .retain(|l| !(l.instance.source == SoundSource::Music && l.handle_stopped));
    }

    /// `SoundEngine.pauseAllExcept(SoundSource...)`.
    pub fn pause_all_except(&mut self, ignored: &[SoundSource], device: &mut dyn AudioDevice) {
        if !self.loaded {
            return;
        }
        let targets: Vec<ChannelId> = self
            .live
            .iter()
            .filter(|l| !ignored.contains(&l.instance.source))
            .map(|l| l.channel)
            .collect();
        for ch in targets {
            device.submit(ch, ChannelCall::Pause);
        }
    }

    /// `SoundEngine.resume()` — note it unpauses **every** channel, including
    /// the ones `pauseAllExcept` skipped. `Channel.unpause` guards on
    /// `AL_PAUSED`, so an already-playing source is untouched.
    pub fn resume(&mut self, device: &mut dyn AudioDevice) {
        if !self.loaded {
            return;
        }
        let targets: Vec<ChannelId> = self.live.iter().map(|l| l.channel).collect();
        for ch in targets {
            device.submit(ch, ChannelCall::Unpause);
        }
    }
}

// ---------------------------------------------------------------------------
// The wire → instance adapter
// ---------------------------------------------------------------------------

/// Why a decoded [`SoundEvent`] produced no instance.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoInstance {
    /// `SoundRef::Registry(id)` naming an id this jar's registry does not have
    /// — a newer server naming a newer sound. M64's rule: a wrong sound is
    /// harder to notice than a missing one.
    UnknownSoundId,
    /// `handleSoundEntityEvent`'s `if (entity != null)` — a sound addressed to
    /// an entity the client is not tracking is **dropped**, not played at the
    /// player.
    UnknownEntity,
    /// The event is a [`SoundEvent::Stop`]; see [`stop_from_event`].
    IsStop,
    /// The event is a [`SoundEvent::BiomeLoopTransition`] — it names a change
    /// to ramps the engine already holds rather than a sound to start, so it
    /// has no instance at all (M142d).
    IsBiomeTransition,
    /// The event is a [`SoundEvent::Tickable`] — it has a ramp as well as an
    /// instance, so [`instance_and_ramp`] is its entry point (M141e).
    IsTickable,
    /// The event is a [`SoundEvent::Music`] — one `MusicManager.tick`, which
    /// may start nothing, stop something, or both, so it has no single
    /// instance (M146).
    IsMusicTick,
}

/// Turn one decoded [`SoundEvent`] into the [`SoundInstance`] vanilla builds.
///
/// This is `ClientPacketListener.handleSoundEvent` /
/// `handleSoundEntityEvent` plus `ClientLevel.playSeededSound`, with the
/// `except == this.minecraft.player` guard already discharged: **both
/// overloads play the sound only when the "except" entity IS the local
/// player**, which reads backwards until you notice that the handler passes
/// `this.minecraft.player` unconditionally. M71 found the same inversion on
/// the `game_event` path. The condition is therefore always true for these two
/// packets and is not a decision this function has to make.
///
/// Note also what the positioned overload passes for `distanceDelay`:
/// **`false`**. A network sound is never distance-delayed; only
/// `playLocalSound` can be, and nothing on the wire reaches it.
pub fn instance_from_event(
    event: &SoundEvent,
    registry: &rewo_data::sound_events::SoundEvents,
    world: &dyn SoundWorld,
) -> Result<SoundInstance, NoInstance> {
    match event {
        SoundEvent::At(p) => Ok(instance_from_positioned(p, registry)?),
        SoundEvent::OnEntity(e) => instance_from_entity(e, registry, world),
        SoundEvent::Stop(_) => Err(NoInstance::IsStop),
        SoundEvent::Local(l) => Ok(instance_from_local(l)),
        // Already an instance — the client built it itself, with whatever
        // `relative`/`attenuation` its vanilla factory chose (M142b).
        SoundEvent::Instance(i) => Ok(i.clone()),
        // A tickable carries a ramp as well as an instance, so it cannot be
        // reduced to one here — `instance_and_ramp` is its entry point and
        // this arm exists so a caller that only wants an instance says so.
        SoundEvent::Tickable(_) => Err(NoInstance::IsTickable),
        // Not a sound at all: it mutates ramps the engine already owns.
        SoundEvent::BiomeLoopTransition { .. } => Err(NoInstance::IsBiomeTransition),
        // Also not a sound: it drives a state machine that may or may not
        // decide to start one.
        SoundEvent::Music { .. } => Err(NoInstance::IsMusicTick),
    }
}

/// The instance **and** its ramp, for a [`SoundEvent::Tickable`] (M141e).
///
/// The two are built together on purpose. Vanilla's constructor sets the
/// instance's fields and its `tick()` maintains them, and the pair only makes
/// sense as a pair: an `ElytraOnPlayerSoundInstance` whose ramp was a bee's
/// would fade on the wrong input and stop on the wrong condition. Splitting
/// them across two functions is one more place for them to disagree.
pub fn instance_and_ramp(
    t: crate::sounds::TickableSound,
    world: &dyn SoundWorld,
) -> Result<(SoundInstance, crate::tickable::Ramp), NoInstance> {
    match t {
        crate::sounds::TickableSound::ElytraOnPlayer { player } => {
            // `ElytraOnPlayerSoundInstance`'s constructor: looping, no delay,
            // and **volume 0.1** — which `tick()` overwrites on its very first
            // run, so the only thing the initial value decides is whether
            // `play` drops it as silent. It does not set `canStartSilent`, so
            // that 0.1 is load-bearing: at 0.0 the sound would never start.
            let (x, y, z) = world.position(player).ok_or(NoInstance::UnknownEntity)?;
            let inst = SoundInstance {
                volume: 0.1,
                looping: true,
                delay: 0,
                x,
                y,
                z,
                // **`Binding::Fixed`, not `Entity(player)`** — the binding
                // is what silence-gates a sound at `play`, and
                // `ElytraOnPlayerSoundInstance` does not override
                // `canPlaySound()`. Binding it to the player would refuse to
                // start for a `/data`-silenced one, which vanilla does not do.
                // The position still tracks: that is the ramp's job.
                ..SoundInstance::bare("minecraft:item.elytra.flying", SoundSource::Players)
            };
            Ok((
                inst,
                crate::tickable::Ramp::Elytra(crate::tickable::ElytraRamp { player, time: 0 }),
            ))
        }
        crate::sounds::TickableSound::MinecartRiding { minecart } => {
            // `MinecartSoundInstance`'s constructor: looping, no delay,
            // **volume 0.0** and `canStartSilent() = true` — the pair that
            // lets a stationary cart's loop exist at all, since `play` drops a
            // zero-volume instance unless it says it can start silent.
            let (x, y, z) = world.position(minecart).ok_or(NoInstance::UnknownEntity)?;
            let inst = SoundInstance {
                volume: 0.0,
                looping: true,
                delay: 0,
                can_start_silent: true,
                x,
                y,
                z,
                // It DOES override `canPlaySound()`, so unlike the elytra it is
                // silence-gated on its entity.
                binding: Binding::Entity(minecart),
                ..SoundInstance::bare("minecraft:entity.minecart.riding", SoundSource::Neutral)
            };
            Ok((
                inst,
                crate::tickable::Ramp::Minecart(crate::tickable::MinecartRamp {
                    minecart,
                    shadowed_pitch: 0.0,
                }),
            ))
        }
        crate::sounds::TickableSound::GuardianAttack { guardian } => {
            // `GuardianAttackSoundInstance`'s constructor: **`Attenuation.NONE`**,
            // looping, no delay — and it leaves `volume` and `pitch` at the
            // `AbstractSoundInstance` defaults of 1.0, which `tick()` then
            // overwrites on its first run. No attenuation means the beam is
            // heard at full gain wherever you are, which is what makes it a
            // warning rather than an ambience.
            let (x, y, z) = world.position(guardian).ok_or(NoInstance::UnknownEntity)?;
            let inst = SoundInstance {
                looping: true,
                delay: 0,
                attenuation: Attenuation::None,
                x,
                y,
                z,
                binding: Binding::Entity(guardian),
                ..SoundInstance::bare("minecraft:entity.guardian.attack", SoundSource::Hostile)
            };
            Ok((
                inst,
                crate::tickable::Ramp::Guardian {
                    guardian,
                    attack_duration: crate::tickable::GUARDIAN_ATTACK_DURATION,
                },
            ))
        }
        crate::sounds::TickableSound::SnifferDigging { sniffer } => {
            // `SnifferSoundInstance`'s constructor: `Attenuation.LINEAR`
            // (the default, set explicitly) and **`looping = false`** — the
            // one tickable here that is a one-shot, so its ramp's job is the
            // stop condition rather than a fade.
            let (x, y, z) = world.position(sniffer).ok_or(NoInstance::UnknownEntity)?;
            let inst = SoundInstance {
                looping: false,
                delay: 0,
                x,
                y,
                z,
                binding: Binding::Entity(sniffer),
                ..SoundInstance::bare("minecraft:entity.sniffer.digging", SoundSource::Neutral)
            };
            Ok((inst, crate::tickable::Ramp::Sniffer { sniffer }))
        }
        crate::sounds::TickableSound::UnderwaterLoop { player } => {
            // `UnderwaterAmbientSoundInstance`'s constructor: looping, delay 0,
            // **volume 1.0**, `relative` — and attenuation left at the
            // inherited LINEAR, which `relative` does NOT imply.
            //
            // The 1.0 is load-bearing rather than cosmetic. `SoundEngine.play`
            // returns `NOT_STARTED` for a zero-volume instance unless
            // `canStartSilent()`, and this class does not override it — so
            // constructing it at 0.0 "because it fades in from silence" makes
            // it never play at all, with only a debug log to say so. The first
            // tick immediately rewrites the volume to 1/40.
            let inst = SoundInstance {
                looping: true,
                delay: 0,
                volume: 1.0,
                relative: true,
                ..SoundInstance::bare(
                    crate::ambient_handlers::UNDERWATER_LOOP,
                    SoundSource::Ambient,
                )
            };
            Ok((
                inst,
                crate::tickable::Ramp::UnderwaterLoop(crate::tickable::UnderwaterRamp {
                    player,
                    fade: 0,
                }),
            ))
        }
        crate::sounds::TickableSound::UnderwaterSub { player, sound } => {
            // `SubSound`'s constructor — the same shape but **not looping**,
            // and its whole `tick()` is two stop conditions: it writes no
            // volume, no pitch and no position, so surfacing CUTS an addition
            // mid-sample rather than fading it.
            let inst = SoundInstance {
                looping: false,
                delay: 0,
                volume: 1.0,
                relative: true,
                ..SoundInstance::bare(sound, SoundSource::Ambient)
            };
            Ok((
                inst,
                crate::tickable::Ramp::UnderwaterSub { player },
            ))
        }
        crate::sounds::TickableSound::BiomeLoop { sound } => {
            // `LoopSoundInstance`'s constructor: looping, delay 0, **volume
            // 1.0**, relative — and attenuation left at the inherited LINEAR.
            //
            // Same reason as the underwater loop's: `SoundEngine.play` refuses
            // a zero-volume instance unless `canStartSilent()`, which this
            // class does not override. It is played BEFORE `fadeIn()` in
            // vanilla, so 1.0 is what the channel actually receives for one
            // tick before the ramp writes `fade/40`.
            let inst = SoundInstance {
                looping: true,
                delay: 0,
                volume: 1.0,
                relative: true,
                ..SoundInstance::bare(sound.clone(), SoundSource::Ambient)
            };
            Ok((
                inst,
                // `fadeIn()` applied at construction — see the variant's doc.
                crate::tickable::Ramp::BiomeLoop(crate::tickable::BiomeLoopRamp {
                    fade: 0,
                    fade_direction: 1,
                }),
            ))
        }
        crate::sounds::TickableSound::Riding(r) => {
            // `RidingEntitySoundInstance`'s constructor: **`Attenuation.NONE`**,
            // looping, no delay, and `volume = volumeMin` — which is 0.0 for
            // all four, so `canStartSilent()` (true) is what lets them start.
            // No attenuation because you are sitting on the thing.
            let (x, y, z) = world.position(r.vehicle).ok_or(NoInstance::UnknownEntity)?;
            let inst = SoundInstance {
                volume: r.volume_min,
                looping: true,
                delay: 0,
                can_start_silent: true,
                attenuation: Attenuation::None,
                x,
                y,
                z,
                // It overrides `canPlaySound()` (`!entity.isSilent()`), and on
                // the **vehicle** — so a silenced cart silences its rider's
                // loop, which is why the binding is the vehicle rather than
                // the player.
                binding: Binding::Entity(r.vehicle),
                ..SoundInstance::bare(r.sound, SoundSource::Neutral)
            };
            Ok((
                inst,
                crate::tickable::Ramp::Riding(crate::tickable::RidingRamp {
                    player: r.player,
                    vehicle: r.vehicle,
                    underwater_sound: r.underwater_sound,
                    volume_min: r.volume_min,
                    volume_max: r.volume_max,
                    volume_amplifier: r.volume_amplifier,
                    is_minecart: r.is_minecart,
                }),
            ))
        }
        crate::sounds::TickableSound::BeeLoop { bee, aggressive } => {
            let kind = if aggressive {
                crate::tickable::BeeLoop::Aggressive
            } else {
                crate::tickable::BeeLoop::Flying
            };
            let pos = world.position(bee).ok_or(NoInstance::UnknownEntity)?;
            Ok((
                crate::tickable::bee_instance(kind, bee, pos),
                crate::tickable::Ramp::Bee(crate::tickable::BeeRamp {
                    bee,
                    loop_kind: kind,
                    has_switched: false,
                }),
            ))
        }
    }
}

fn instance_from_positioned(
    p: &PositionedSound,
    registry: &rewo_data::sound_events::SoundEvents,
) -> Result<SoundInstance, NoInstance> {
    let name = p.sound.resolve(registry).ok_or(NoInstance::UnknownSoundId)?;
    Ok(SoundInstance::simple(
        name, p.source, p.volume, p.pitch, p.seed, p.x, p.y, p.z,
    ))
}

fn instance_from_entity(
    e: &EntitySound,
    registry: &rewo_data::sound_events::SoundEvents,
    world: &dyn SoundWorld,
) -> Result<SoundInstance, NoInstance> {
    let name = e.sound.resolve(registry).ok_or(NoInstance::UnknownSoundId)?;
    let (x, y, z) = world
        .position(e.entity_id)
        .ok_or(NoInstance::UnknownEntity)?;
    Ok(SoundInstance::entity_bound(
        name, e.source, e.volume, e.pitch, e.seed, e.entity_id, x, y, z,
    ))
}

fn instance_from_local(l: &LocalSound) -> SoundInstance {
    // M71's client-decided sounds carry no seed — `level.random.nextLong()` is
    // drawn at play time — so this is the one wire-adjacent path that takes an
    // unseeded instance.
    SoundInstance {
        volume: l.volume,
        pitch: l.pitch,
        x: l.x,
        y: l.y,
        z: l.z,
        ..SoundInstance::bare(l.name.clone(), l.source)
    }
}

/// The `(name, source)` pair `SoundEngine.stop` takes, from a decoded
/// [`StopSound`].
pub fn stop_from_event(stop: &StopSound) -> (Option<&str>, Option<SoundSource>) {
    (stop.name.as_deref(), stop.source)
}

/// A [`SoundWorld`] over a live [`rewo_world::entities::EntityTable`].
///
/// The position is `render_pos(1.0)` — the **current** interpolated position,
/// which is what `entity.getX()` returns at tick time — not the synced target
/// `x/y/z`, which is where the entity is heading. Using the target would put a
/// moving mob's sound up to three ticks ahead of the mob.
pub struct EntityTableWorld<'a> {
    pub table: &'a rewo_world::entities::EntityTable,
    /// The local player, who is **not a row in the table** (M141d).
    ///
    /// `EntityTable` holds only entities the server sent an `add_entity` for,
    /// and it never sends one for you — the same asymmetry M73 hit with
    /// attributes. Every ramp but one reads a remote entity and does not care;
    /// `ElytraOnPlayerSoundInstance` reads `player.getDeltaMovement()`, and
    /// without this it would see a removed entity and stop itself on its first
    /// tick, which is a silence that looks exactly like a correct one.
    pub local: Option<LocalPlayerView>,
    /// `level.getGameTime()` (M141f).
    ///
    /// Only the bee asks, and it asks because `NeutralMob.isAngry()` is a
    /// **deadline** compared against the clock rather than a flag — so the
    /// answer changes every tick with no packet arriving, and a world that did
    /// not carry the time could only ever report a stale one.
    pub game_time: i64,
}

/// The local player's half of [`crate::tickable::RampWorld`].
///
/// Small and explicit rather than a handle to `PlayerState`, because only
/// three of the sixteen queries have a local answer and naming them is what
/// stops a fourth being silently wrong.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LocalPlayerView {
    pub id: i32,
    /// `getX/Y/Z()`. Feet position, as the wire carries it.
    pub position: (f64, f64, f64),
    /// `getDeltaMovement()` — **the physics velocity**, not a decayed packet
    /// echo. The local player is `canSimulateMovement()`, so vanilla never
    /// decays it either; the two agree by construction rather than by
    /// coincidence.
    pub velocity: (f64, f64, f64),
    /// `isFallFlying()` — shared flag 7.
    pub fall_flying: bool,
    /// `LocalPlayer.isUnderWater()` — `isEyeInFluid(FluidTags.WATER)`,
    /// **one term** (M141h, corrected in M142b).
    ///
    /// The two-term `wasEyeInWater && isInWater()` this comment used to
    /// cite is `Entity.isUnderWater()` (`Entity.java:1622-1624`), which
    /// every other entity answers. `LocalPlayer` OVERRIDES it to return
    /// `wasUnderwater` (`LocalPlayer.java:1172-1175`), and
    /// `Player.updateIsUnderwater` writes that from the eye alone
    /// (`Player.java:304-307`), once per tick at the TOP of
    /// `Player.tick()`. The conjunct is redundant within one tracker
    /// update, so nothing was audibly wrong — but this is the citation a
    /// re-derivation follows, one line above the field that gates the
    /// whole underwater feature.
    ///
    /// The local player's, because `RidingMinecartSoundInstance` overrides
    /// `shouldNotPlayUnderwaterSound()` to ask the **rider** rather than the
    /// vehicle — which is the whole reason the minecart's two loops crossfade
    /// on *your* submersion and not the cart's.
    pub underwater: bool,
}

/// The half of [`crate::tickable::RampWorld`] an [`rewo_world::entities::EntityTable`]
/// can answer, and an explicit account of the half it cannot.
///
/// **Nine of the sixteen queries are answered; seven are not, and each says
/// so.** M141d closed the three speed queries, which were the ones that
/// mattered — they gated four of the ten ramps.
///
/// **Count it rather than trusting this sentence**: an unanswered query is
/// exactly one whose signature is `fn name(&self, _: i32)`, because it does
/// not look at the entity. Both halves of this doc have been wrong before —
/// before M141d the caption said "nine … seven" over a table whose rows summed
/// to *ten* queries, and the first M141d edit said "twelve … four". A number
/// beside a table is worth what the table says, and the table is worth what
/// the signatures say.
///
/// The rule applied to the seven is *make the ramp inert, never plausible*: a
/// sound that stays at its minimum volume is a smaller lie than one that ramps
/// on a number nobody derived, because silence is attributable and a wrong
/// ramp is not. This is M96's principle — greying a recipe you could make
/// beats lighting one you cannot — applied to audio.
///
/// One of the seven is not an approximation at all. [`RampWorld::has_ai_target`]
/// answers **`false` exactly**: `Mob.target` is a plain field written by AI
/// goals in `serverAiStep` and has no `EntityDataAccessor`, so no client ever
/// sees one. It is listed here because it *looks* like a gap and is not.
///
/// The genuine gaps that remain, and what each costs:
///
/// | query | why | consequence |
/// |---|---|---|
/// | `angry` | `Bee.DATA_ANGER_END_TIME` is not decoded | a bee never switches to its aggressive loop |
/// | `underwater` | needs the level's fluid at the entity, not the table | the dry half of the riding pair plays and the wet half mutes |
/// | `attack_animation_scale` | `clientSideAttackTime` is a client counter Rewo does not run | a guardian's beam is silent at pitch 0.7 |
/// | `sniffer_digging` | the state enum is decoded for the gesture rig, not exposed here | a sniffer's dig sound stops on its first tick |
/// | `on_rails`, `new_minecart_behavior` | needs blocks | **both `false`, which reads as "on rails, old behaviour"** — the audible case, chosen because the pair is a conjunction and `(false, false)` makes `off_rail` false |
/// | `camera_position` | not a property of the entity table | only `Ramp::Directional` asks, and nothing constructs one |
///
/// Six rows, seven queries — `on_rails`/`new_minecart_behavior` is the only
/// row naming two, and that is where the caption's number comes from.
///
/// One of them is closer than it looks. `NewMinecartBehavior.tick`'s **client**
/// branch runs `this.minecart.setOnRails(BaseRailBlock.isRail(...))`, so
/// `isOnRails()` is a client-side read of the block below and Rewo has the
/// blocks. `OldMinecartBehavior`'s client branch does not, so the old-behaviour
/// answer stays server-sent — which is why this is a note rather than a fix.
///
/// Closing them is per-input work with its own witnesses, not a refactor —
/// which is why they are a table rather than a TODO.
impl crate::tickable::RampWorld for EntityTableWorld<'_> {
    /// `entity.getX/Y/Z()` at tick time.
    ///
    /// `render_pos(1.0)` — the **current** interpolated position, not the
    /// synced target `x/y/z`, which is where the entity is heading. Using the
    /// target would put a moving mob's sound up to three ticks ahead of it.
    fn position(&self, entity_id: i32) -> Option<(f64, f64, f64)> {
        if let Some(l) = self.local.filter(|l| l.id == entity_id) {
            // Not in the table, and `None` here is `isRemoved()` — so without
            // this branch the elytra ramp stops itself on its first tick.
            return Some(l.position);
        }
        let e = self.table.get(entity_id)?;
        let p = e.render_pos(1.0);
        Some((p[0], p[1], p[2]))
    }

    /// `getDeltaMovement().horizontalDistance()` — real since M141d.
    ///
    /// **This is not how fast the entity is moving.** It is the decaying echo
    /// of the last `set_entity_motion` packet, which is exactly what vanilla's
    /// ramps read: a client never integrates a remote entity's velocity into
    /// its position, so a bee gliding past with no motion packets has this
    /// falling to zero while it is visibly moving, and its buzz fades with it.
    /// A finite difference over the interpolated positions is the obvious
    /// implementation, is more truthful about the bee, and is not what vanilla
    /// sounds like.
    fn horizontal_speed(&self, entity_id: i32) -> f64 {
        if let Some(l) = self.local.filter(|l| l.id == entity_id) {
            let (x, _, z) = l.velocity;
            return (x * x + z * z).sqrt();
        }
        self.table.horizontal_speed(entity_id)
    }

    /// `getDeltaMovement().length()`.
    fn speed(&self, entity_id: i32) -> f64 {
        if self.local.is_some_and(|l| l.id == entity_id) {
            return self.speed_sqr(entity_id).sqrt();
        }
        self.table.speed(entity_id)
    }

    /// `getDeltaMovement().lengthSqr()` — not rooted; the elytra quarters it.
    fn speed_sqr(&self, entity_id: i32) -> f64 {
        if let Some(l) = self.local.filter(|l| l.id == entity_id) {
            let (x, y, z) = l.velocity;
            return x * x + y * y + z * z;
        }
        self.table.speed_sqr(entity_id)
    }

    /// `entity.isBaby()` — decoded, and the bee's pitch band reads it.
    fn baby(&self, entity_id: i32) -> bool {
        self.table.is_baby(entity_id)
    }

    /// `bee.isAngry()` — real since M141f.
    ///
    /// A synced deadline against the world clock, not a flag: see
    /// [`crate::tickable::is_angry`]. An entity that has never sent one is not
    /// angry, and that is exact rather than a fallback — `Bee` seeds the
    /// accessor to **-1**, which the same predicate reads as calm.
    fn angry(&self, entity_id: i32) -> bool {
        crate::tickable::is_angry(
            self.table.anger_end_time(entity_id).unwrap_or(-1),
            self.game_time,
        )
    }
    /// `entity.isUnderWater()` — answered for the local player since M141h.
    ///
    /// **A remote entity still answers `false`, and the cost is precise.**
    /// `RidingEntitySoundInstance`'s base class asks the *vehicle*, so a happy
    /// ghast reads correctly (it is never submerged) and **a ridden nautilus's
    /// loop is permanently muted**, because its instance is the
    /// `underwaterSound = true` one and mutes whenever the vehicle is not
    /// submerged. Closing it wants a per-type eye height, which
    /// `entity_attachments` does not carry — it extracted seats, not
    /// dimensions.
    fn underwater(&self, entity_id: i32) -> bool {
        self.local.is_some_and(|l| l.id == entity_id && l.underwater)
    }

    /// **Exact, not a stand-in** — see the type's doc.
    fn has_ai_target(&self, _: i32) -> bool {
        false
    }

    /// `guardian.getAttackAnimationScale(0.0F)` — real since M141g.
    ///
    /// The duration is the **base** species' 80. An elder guardian's is 60,
    /// and this world cannot tell them apart because the table stores no type
    /// id — a stated divergence, bounded to an elder's beam reaching full
    /// volume a third late rather than misbehaving. See
    /// `EntityTable::tick_guardian_attacks`.
    fn attack_animation_scale(&self, entity_id: i32) -> f32 {
        self.table.guardian_attack_scale(
            entity_id,
            rewo_world::entities::GUARDIAN_MAX_ATTACK_DURATION,
        )
    }

    /// `sniffer.canPlayDiggingSound()` — real since M141g. Two states, not
    /// one: DIGGING **or** SEARCHING.
    fn sniffer_digging(&self, entity_id: i32) -> bool {
        self.table.sniffer_digging(entity_id)
    }
    fn on_rails(&self, _: i32) -> bool {
        false
    }
    fn new_minecart_behavior(&self, _: i32) -> bool {
        false
    }

    /// `level.tickRateManager().runsNormally()`.
    ///
    /// **Also exact rather than a stand-in, for now**: Rewo does not decode
    /// `ticking_state`, and vanilla's default tick rate *is* normal — so this
    /// is wrong only against a server that has frozen or slowed ticks, which
    /// is a decode gap with a name rather than an unknown.
    fn runs_normally(&self) -> bool {
        true
    }

    /// `player.isFallFlying()` — `DATA_SHARED_FLAGS_ID` bit **7**, which the
    /// cape rig already reads.
    fn fall_flying(&self, entity_id: i32) -> bool {
        if let Some(l) = self.local.filter(|l| l.id == entity_id) {
            return l.fall_flying;
        }
        self.table.shared_flag(entity_id, 7)
    }

    /// `player.getVehicle()` — M70's riding graph, and `None` is exactly
    /// `!isPassenger()`.
    fn vehicle_of(&self, entity_id: i32) -> Option<i32> {
        self.table.vehicle_of(entity_id)
    }

    fn camera_position(&self) -> (f64, f64, f64) {
        (0.0, 0.0, 0.0)
    }
}

impl SoundWorld for EntityTableWorld<'_> {
    fn entity_silent(&self, entity_id: i32) -> bool {
        // `Entity.isSilent()` — metadata index 4, decoded since M138a. This was
        // a hardcoded `false` with a comment saying so, which is the honest
        // shape for an undecodable fact and the wrong shape once it is one
        // line of table.
        //
        // **An entity this table has never seen is audible**, and that is
        // exact rather than a fallback: `Entity.java:322` seeds `DATA_SILENT`
        // to `false`, so an unknown entity and an un-silenced one really do
        // answer the same thing.
        self.table.is_silent(entity_id)
    }
}

/// What happened to the sound events drained from one tick, for a caller that
/// wants to know the pipeline is alive without hearing anything.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SoundStats {
    pub started: u32,
    pub started_silently: u32,
    /// A `stop_sound` packet applied.
    pub stops: u32,
    /// Refused by [`SoundEngine::play`] — see [`NotStarted`].
    pub not_started: u32,
    /// Never reached the engine: the registry, the entity table or the
    /// event's own kind ruled it out. See [`NoInstance`].
    pub no_instance: u32,
    /// Biome-loop transitions applied (M142d). Counted separately because a
    /// transition is not a play: most of them fade a live loop rather than
    /// starting anything, so folding them into `started` would make the
    /// commonest case invisible.
    pub biome_transitions: u32,
}

impl SoundStats {
    pub fn total(&self) -> u32 {
        self.started + self.started_silently + self.stops + self.not_started + self.no_instance
    }
}

/// Engine + asset index + counters — the object a client owns.
///
/// The seam `PlaySession::take_sound_events` was built for in M63 and which
/// nothing had ever called: before this, the decoded queue filled to its cap
/// and rotated forever, and `SoundsIndex`, `SoundEvents::name` and
/// `level_event_sounds::resolve` had **zero production callers between them**.
#[derive(Clone, Debug, Default)]
pub struct SoundSystem {
    pub engine: SoundEngine,
    /// The merged `sounds.json` index (M66). Empty until loaded, in which case
    /// every event resolves to [`NotStarted::UnknownEvent`] — a client with no
    /// resource pack on disk is silent, not broken.
    pub sounds: SoundsIndex,
    pub stats: SoundStats,
    /// `Minecraft.musicManager` (M146).
    ///
    /// **It lives here rather than in `PlaySession` because of one method.**
    /// `MusicManager.tick` asks `soundManager.isActive(currentMusic)`, and only
    /// the engine can answer that; the session, meanwhile, is the only thing
    /// that knows the biome, the abilities and the boss bar. So the session
    /// names the situation ([`SoundEvent::Music`]) and this side runs the
    /// machine — the same split M142d used for the biome loop.
    pub music: crate::music::MusicManager,
    /// The instance the manager last started, so `isActive` has something to
    /// ask about. `None` whenever the manager holds no track.
    music_instance: Option<InstanceId>,
}

impl SoundSystem {
    pub fn new(sounds: SoundsIndex) -> SoundSystem {
        SoundSystem {
            engine: SoundEngine::new(),
            sounds,
            stats: SoundStats::default(),
            // Vanilla seeds from `RandomSource.create()`, which is a unique
            // seed per session; a fixed one here makes a run reproducible,
            // which is what the witnesses need and what a player never notices.
            music: crate::music::MusicManager::new(
                0,
                crate::music::MusicFrequency::Default,
            ),
            music_instance: None,
        }
    }

    /// Feed one drained batch of [`SoundEvent`]s to the engine, in order.
    ///
    /// **Order is the contract**, which is why M63 put all four kinds in one
    /// queue: a `stop_sound` that overtook the `sound` it cancels would leave
    /// that sound playing forever.
    pub fn accept(
        &mut self,
        events: &[SoundEvent],
        registry: &rewo_data::sound_events::SoundEvents,
        world: &dyn SoundWorld,
        device: &mut dyn AudioDevice,
    ) {
        for ev in events {
            if let SoundEvent::Stop(stop) = ev {
                let (name, source) = stop_from_event(stop);
                self.engine.stop_matching(name, source, device);
                self.stats.stops += 1;
                continue;
            }
            // The biome loop's transition mutates ramps the engine already
            // holds, so it is applied here rather than built into an instance
            // — and it may ask for exactly one new loop, which is then played
            // through the ordinary path below so it gets the same channel
            // budget and the same `NOT_STARTED` accounting as everything else.
            // One `MusicManager.tick`. Like the transition above it decides
            // rather than describes, and what it decides is applied through the
            // ordinary `play` path so music gets the same channel budget and
            // the same accounting as everything else.
            if let SoundEvent::Music { situational } = ev {
                self.tick_music(situational.as_ref(), world, device);
                continue;
            }
            if let SoundEvent::BiomeLoopTransition { current } = ev {
                let needed = self.engine.apply_biome_loop_transition(current.as_deref());
                self.stats.biome_transitions += 1;
                if let Some(sound) = needed {
                    let ev = SoundEvent::Tickable(crate::sounds::TickableSound::BiomeLoop { sound });
                    if let Ok((i, r)) = instance_and_ramp(
                        match &ev {
                            SoundEvent::Tickable(t) => t.clone(),
                            _ => unreachable!(),
                        },
                        world,
                    ) {
                        let (_, result) =
                            self.engine
                                .play_ramped(i, Some(r), &self.sounds, world, device);
                        match result {
                            PlayResult::Started => self.stats.started += 1,
                            PlayResult::StartedSilently => self.stats.started_silently += 1,
                            PlayResult::NotStarted(_) => self.stats.not_started += 1,
                        }
                    }
                }
                continue;
            }
            // A tickable carries a ramp, so it goes through `play_ramped`.
            // Same queue and therefore the same order, which is the contract
            // `stop_sound` depends on.
            // A tickable carries a ramp, so it goes through `play_ramped`.
            // Everything else goes through `play`, which is NOT the same as
            // `play_ramped(.., None)`: `play` attaches `Ramp::EntityBound` for
            // an entity-bound instance, and passing `None` here would silently
            // stop every `sound_entity` following its entity. The engine's own
            // follow test calls `play` directly and could not have seen it.
            let built = match ev {
                SoundEvent::Tickable(t) => instance_and_ramp(t.clone(), world).map(|(i, r)| (i, Some(r))),
                _ => instance_from_event(ev, registry, world)
                    .map(|i| {
                        let r = crate::tickable::Ramp::for_instance(&i);
                        (i, r)
                    }),
            };
            match built {
                Ok((instance, ramp)) => {
                    let (_, result) =
                        self.engine
                            .play_ramped(instance, ramp, &self.sounds, world, device);
                    match result {
                        PlayResult::Started => self.stats.started += 1,
                        PlayResult::StartedSilently => self.stats.started_silently += 1,
                        PlayResult::NotStarted(_) => self.stats.not_started += 1,
                    }
                }
                Err(_) => self.stats.no_instance += 1,
            }
        }
    }

    /// `MusicManager.tick`, and the three things it can ask for (M146).
    ///
    /// `isActive` is asked of the engine here — that is the whole reason the
    /// manager lives on this side. A stopped or reclaimed instance answers
    /// false, which is what lets the manager notice a track has ended and
    /// schedule the next one.
    fn tick_music(
        &mut self,
        situational: Option<&rewo_world::music::Music>,
        world: &dyn SoundWorld,
        device: &mut dyn AudioDevice,
    ) {
        let is_active = self
            .music_instance
            .map(|id| self.engine.is_active(id))
            .unwrap_or(false);
        // `getMusicVolume()` is the `MUSIC_VOLUME` environment attribute, which
        // Rewo does not sample — no biome or dimension in 26.2 declares it, so
        // the probe's answer is its default of 1.0 everywhere. Passing the
        // constant is therefore exact today and a stated assumption tomorrow.
        let outcome = self.music.tick(1.0, situational, is_active, false);

        if outcome.stop_current {
            if let Some(id) = self.music_instance.take() {
                self.engine.stop(id, device);
                self.stats.stops += 1;
            }
        }
        if let Some(gain) = outcome.category_volume {
            // `updateCategoryVolume(MUSIC, gain)` — the crossfade's one writer
            // (M140b), and not the options slider, which arrives a factor
            // earlier.
            self.engine.update_category_volume(SoundSource::Music, gain);
        }
        if let Some(music) = outcome.start {
            let instance = SoundInstance::for_music(music.sound);
            let (id, result) = self.engine.play(instance, &self.sounds, world, device);
            match result {
                PlayResult::Started | PlayResult::StartedSilently => {
                    self.music_instance = Some(id);
                    match result {
                        PlayResult::Started => self.stats.started += 1,
                        _ => self.stats.started_silently += 1,
                    }
                }
                // **The channel budget can refuse music like anything else**,
                // and when it does the manager must not go on believing a track
                // is playing — otherwise it waits for a sound that never
                // started, and the next song never comes.
                PlayResult::NotStarted(_) => {
                    self.stats.not_started += 1;
                    self.music_instance = None;
                    self.music.stop_playing(situational);
                }
            }
        }
    }

    /// One 20 Hz engine tick.
    pub fn tick(&mut self, paused: bool, world: &dyn SoundWorld, device: &mut dyn AudioDevice) {
        self.engine.tick(paused, &self.sounds, world, device);
    }
}

/// A backend that can actually make a noise, bolted onto the silent path.
///
/// **This is not a second [`AudioDevice`].** The channel pools, the refusal
/// count and the listener record are `Library`'s bookkeeping, not any device's
/// — `CountingChannelPool` is plain arithmetic and would be identical behind
/// cpal, behind OpenAL, and behind nothing at all. So [`SilentDevice`] keeps
/// doing all of it, every existing witness keeps reading it, and a backend
/// implements only the three things it can do that bookkeeping cannot: pass the
/// call on, answer whether a channel has finished, and say why it is quiet.
///
/// It lives in this crate because it names nothing but [`ChannelCall`],
/// [`ChannelId`] and [`ListenerTransform`]. **The implementor is in
/// `rewo-audio` and the dependency runs one way only**: `rewo-audio` may depend
/// on `rewo-net`, and `rewo-net` must never depend on `rewo-audio`, or every
/// gate that decodes a packet links a codec and a device.
pub trait ChannelSink {
    /// One `handle.execute(channel -> …)` body, by reference — [`SilentDevice`]
    /// consumes the same call for its counter, and cloning it for both would
    /// copy `AttachStaticBuffer`'s `String` on the hot path.
    fn submit(&mut self, channel: ChannelId, call: &ChannelCall);

    /// `Library.releaseChannel` — vanilla **destroys the source**. A backend
    /// that treats this as bookkeeping and leaves the voice sounding will run
    /// out of voices; one that honours it while answering [`Self::stopped`]
    /// carelessly will cut every sound short. See [`Self::stopped`].
    fn release(&mut self, channel: ChannelId);

    /// `Listener.setTransform` — pushed per frame, not per tick (M138a).
    fn set_listener(&mut self, transform: ListenerTransform);

    /// One engine tick, so a backend can model `AL_STOPPED` on the tick clock.
    ///
    /// Driven from [`LiveSounds::drive`], which is the same 20 Hz clock
    /// `SoundEngine.tick` runs on. A backend wanting wall-clock time is free to
    /// ignore it; one that uses it gets a `stopped()` that is deterministic and
    /// therefore gradeable without a device, which is the whole reason this is
    /// on the trait rather than an `Instant` inside an implementor.
    fn tick(&mut self) {}

    /// `Channel.stopped()` — `AL_SOURCE_STATE == AL_STOPPED`. `None` means "no
    /// opinion", and the silent path's unconditional `true` stands.
    ///
    /// **This is the method a naive backend gets wrong, and it is inaudible in
    /// every test that does not open a device.** `SoundEngine.schedule_tick`
    /// turns a `true` here straight into `device.release(channel)` on the very
    /// next tick — not after [`MIN_SOURCE_LIFETIME`], which gates the *instance*
    /// reclaim and not the release — and a real `release` destroys the source.
    /// Inherit [`SilentDevice`]'s unconditional `true` and **every sound becomes
    /// a 50 ms blip**: correct-looking code, a green suite, and a client that
    /// clicks instead of playing.
    ///
    /// A source that has been acquired but never played is `AL_INITIAL`, not
    /// `AL_STOPPED`, so the answer there is `false` — vanilla leaks that channel
    /// too, and reporting `true` to avoid the leak would cut a sound that had
    /// not started yet.
    fn stopped(&self, channel: ChannelId) -> Option<bool>;

    /// Why it is quiet, for a caller with no sound and nothing to look at.
    fn diagnostics(&self) -> SinkDiagnostics {
        SinkDiagnostics::default()
    }
}

/// A backend's own counters — the numbers that separate "resolved nothing"
/// from "resolved everything and the device is dead".
///
/// Counters rather than logs, for the reason `CommandRing::dropped` gives: the
/// interesting failures happen where a lock or an allocation is forbidden, and
/// a number a caller reads afterwards is what makes them diagnosable at all.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SinkDiagnostics {
    /// Commands the backend could not hand on. **Non-zero means the consumer
    /// stopped consuming**, i.e. a stalled device — not a busy engine.
    pub dropped: u64,
    /// Static attaches whose asset could not be resolved or decoded. These are
    /// silent channels that still hold a voice.
    pub unresolved: u64,
    /// Streams that could not be opened (M144; before it, streams the backend
    /// declined outright).
    ///
    /// **Separate from [`Self::unresolved`] on purpose**: a client whose music
    /// and ambient beds are silent while its sounds are fine is a different
    /// diagnosis from the reverse, and the two go through different code — one
    /// decodes a whole asset and caches it, the other opens a decode position
    /// and never caches. One counter for both would make the commonest audio
    /// complaint ("no music") indistinguishable from the rarest.
    pub streams_failed: u64,
    /// Distinct buffers decoded and held. Zero with a healthy device and a
    /// healthy index means resolution is the thing that is broken.
    pub cached_buffers: u64,
    /// Errors the output stream itself reported. Read together with
    /// [`Self::dropped`]: **errors with drops is a stalled device, drops alone
    /// is a callback not keeping up**, and neither alongside silence means the
    /// sound reached the backend and the backend is wrong.
    pub device_errors: u64,
}

/// [`SilentDevice`]'s bookkeeping and a [`ChannelSink`]'s output, as one device.
///
/// Built for the duration of one call rather than stored, so `LiveSounds` keeps
/// owning its two halves outright and neither is behind a trait object.
struct Tee<'a> {
    book: &'a mut SilentDevice,
    sink: &'a mut dyn ChannelSink,
}

impl AudioDevice for Tee<'_> {
    /// Bookkeeping only. A backend does not name channels — `CountingChannelPool`
    /// does, and it is the same arithmetic with or without one.
    fn acquire(&mut self, pool: Pool) -> Option<ChannelId> {
        self.book.acquire(pool)
    }

    fn release(&mut self, channel: ChannelId) {
        self.book.release(channel);
        self.sink.release(channel);
    }

    fn submit(&mut self, channel: ChannelId, call: ChannelCall) {
        self.sink.submit(channel, &call);
        self.book.submit(channel, call);
    }

    /// **Deliberately does not consult `book`.** [`SilentDevice::stopped`] is
    /// unconditionally `true`, which is right for a device that makes no noise
    /// and catastrophic for one that does — see [`ChannelSink::stopped`]. The
    /// fallback for a sink with no opinion is that same `true`, so a backend
    /// that declines to answer degrades to the silent path's behaviour rather
    /// than to a pool that never drains.
    fn stopped(&self, channel: ChannelId) -> bool {
        self.sink.stopped(channel).unwrap_or(true)
    }

    fn set_listener(&mut self, transform: ListenerTransform) {
        self.book.set_listener(transform);
        self.sink.set_listener(transform);
    }
}

/// Everything a live client needs to drive the sound model, in one object.
///
/// The bookkeeping device is a **concrete** [`SilentDevice`] rather than a
/// `Box<dyn AudioDevice>`, and stays so now that a backend exists: what a
/// backend adds is [`ChannelSink`], which is three methods, and what it does
/// *not* add is a second copy of the channel pools. Keeping `device` concrete
/// is what lets every witness written against it — `--render-check`'s r45 among
/// them — keep reading the counters at the same place, with or without a
/// backend attached.
///
/// It exists so the live wiring is one field and one call per tick site
/// instead of five, and so the drain logic has a test module — `PlaySession`
/// has none anywhere in the repo (it owns a socket), which is M71's finding
/// and the reason nothing that matters should be written there.
#[derive(Default)]
pub struct LiveSounds {
    pub system: SoundSystem,
    pub device: SilentDevice,
    pub registry: rewo_data::sound_events::SoundEvents,
    /// M143 — the backend, when one has been opened. `None` is the silent
    /// client, which is every gate and every default build.
    sink: Option<Box<dyn ChannelSink>>,
}

impl LiveSounds {
    pub fn new(
        sounds: SoundsIndex,
        registry: rewo_data::sound_events::SoundEvents,
    ) -> LiveSounds {
        LiveSounds {
            system: SoundSystem::new(sounds),
            device: SilentDevice::default(),
            registry,
            sink: None,
        }
    }

    /// Attach a backend. Until this is called the client is silent by
    /// construction, which is what every gate and every default build runs.
    pub fn attach_sink(&mut self, sink: Box<dyn ChannelSink>) {
        self.sink = Some(sink);
    }

    /// Whether a backend is attached. **Not whether anything is audible** — a
    /// sink whose device was opened against a muted endpoint reports `true`
    /// here and produces silence, which is the gap no check in this project can
    /// close.
    pub fn has_sink(&self) -> bool {
        self.sink.is_some()
    }

    /// The backend's counters, or all-zero when there is none.
    pub fn sink_diagnostics(&self) -> SinkDiagnostics {
        self.sink
            .as_ref()
            .map(|s| s.diagnostics())
            .unwrap_or_default()
    }

    /// Run `f` against the bookkeeping device, teed into the backend if there
    /// is one.
    ///
    /// One arm rather than two call paths: every caller here would otherwise
    /// have to remember to build the [`Tee`], and a caller that forgot would
    /// drive a perfectly correct silent engine while a device sat open and
    /// heard nothing from it.
    fn with_device<R>(
        &mut self,
        f: impl FnOnce(&mut SoundSystem, &rewo_data::sound_events::SoundEvents, &mut dyn AudioDevice) -> R,
    ) -> R {
        let LiveSounds {
            system,
            device,
            registry,
            sink,
        } = self;
        match sink {
            Some(s) => {
                let mut tee = Tee {
                    book: device,
                    sink: s.as_mut(),
                };
                f(system, registry, &mut tee)
            }
            None => f(system, registry, device),
        }
    }

    /// Drain one tick's decoded sound events and advance the engine.
    ///
    /// Call once per **client tick**, not once per frame: `SoundEngine.tick`
    /// is `Minecraft.tick`'s, `tickCount` is a tick counter, and
    /// `MIN_SOURCE_LIFETIME` is 20 of them. Driving it per frame would make a
    /// channel's grace period depend on the frame rate.
    pub fn drive(
        &mut self,
        events: &[SoundEvent],
        entities: &rewo_world::entities::EntityTable,
        local: Option<LocalPlayerView>,
        game_time: i64,
    ) {
        let world = EntityTableWorld {
            table: entities,
            local,
            game_time,
        };
        let before = self.system.stats;
        // The backend's clock, advanced with the engine's rather than from an
        // `Instant`: `stopped()` is then a pure function of tick counts, which
        // is what makes it gradeable on a machine with no audio device.
        if let Some(s) = self.sink.as_mut() {
            s.tick();
        }
        self.with_device(|system, registry, device| {
            system.accept(events, registry, &world, device);
            system.tick(false, &world, device);
        });
        if !events.is_empty() {
            let s = self.system.stats;
            log::debug!(
                "sound: +{} started, +{} silent, +{} stopped, +{} refused, +{} unresolvable                  ({} holding a channel)",
                s.started - before.started,
                s.started_silently - before.started_silently,
                s.stops - before.stops,
                s.not_started - before.not_started,
                s.no_instance - before.no_instance,
                self.system.engine.live_count(),
            );
        }
    }

    /// `SoundEngine.updateSource(camera)` — push this frame's listener (M138a).
    ///
    /// ```java
    /// public void updateSource(final Camera camera) {
    ///    if (this.loaded && camera.isInitialized()) {
    ///       ListenerTransform transform = new ListenerTransform(
    ///          camera.position(), new Vec3(camera.forwardVector()), new Vec3(camera.upVector()));
    ///       this.executor.execute(() -> this.listener.setTransform(transform));
    ///    }
    /// }
    /// ```
    ///
    /// **Call once per FRAME**, which is the opposite of [`Self::drive`]'s
    /// contract and deliberately so: vanilla calls this from the render path
    /// with a camera carrying the frame's partial tick, while `SoundEngine.tick`
    /// is `Minecraft.tick`'s. Pushing the listener per tick would step the
    /// stereo image at 20 Hz while the world turned smoothly, which reads as
    /// the audio "lagging" the view and has nothing to do with latency.
    ///
    /// The `loaded` guard has no analogue here — there is no device to load —
    /// and `isInitialized()` is subsumed by the caller only having a camera once
    /// the session is up.
    pub fn update_listener(&mut self, position: [f64; 3], yaw_deg: f32, pitch_deg: f32) {
        let (forward, up) = listener_basis(yaw_deg, pitch_deg);
        let transform = ListenerTransform {
            position,
            forward,
            up,
        };
        // Through the tee, so the backend's ears move with the bookkeeping
        // device's record. r45 reads that record and is unaffected by whether a
        // backend exists — which is the point of leaving `device` concrete.
        self.with_device(|_, _, device| device.set_listener(transform));
    }

    pub fn stats(&self) -> SoundStats {
        self.system.stats
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sounds::SoundRef;
    use rewo_data::sounds_json::{Sound, SoundEventRegistration, SoundFileSet};

    /// Build a one-event index whose single variant is a plain file.
    fn index_with(event: &str, sound: Sound) -> SoundsIndex {
        let mut idx = SoundsIndex::new();
        idx.handle_registration(
            event,
            &SoundEventRegistration {
                sounds: vec![sound],
                replace: false,
                subtitle: None,
            },
            &SoundFileSet::All,
        );
        idx
    }

    /// An index carrying several events — the ramp tests need one because a
    /// bee's switch names a second event, and `play` refuses an unresolvable
    /// one (which is how the first cut of those tests failed: correctly).
    fn index_of(events: &[&str]) -> SoundsIndex {
        let mut idx = SoundsIndex::new();
        for e in events {
            idx.handle_registration(
                e,
                &SoundEventRegistration {
                    sounds: vec![Sound::file(&format!("{}1", e.replace(['.', ':'], "/")))],
                    replace: false,
                    subtitle: None,
                },
                &SoundFileSet::All,
            );
        }
        idx
    }

    fn plain_index() -> SoundsIndex {
        index_with(
            "minecraft:block.stone.break",
            Sound::file("minecraft:block/stone/break1"),
        )
    }

    #[derive(Default)]
    struct TestWorld {
        positions: HashMap<i32, (f64, f64, f64)>,
        silent: Vec<i32>,
        /// Only the ramp-driving tests set these; everything else leaves them
        /// empty, which is why the engine's older witnesses are unchanged by
        /// M141c.
        horizontal_speed: HashMap<i32, f64>,
        angry: Vec<i32>,
    }

    impl SoundWorld for TestWorld {
        fn entity_silent(&self, entity_id: i32) -> bool {
            self.silent.contains(&entity_id)
        }
    }

    impl crate::tickable::RampWorld for TestWorld {
        fn position(&self, entity_id: i32) -> Option<(f64, f64, f64)> {
            self.positions.get(&entity_id).copied()
        }
        fn horizontal_speed(&self, e: i32) -> f64 {
            self.horizontal_speed.get(&e).copied().unwrap_or(0.0)
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
        fn angry(&self, e: i32) -> bool {
            self.angry.contains(&e)
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
        fn on_rails(&self, e: i32) -> bool {
            // The ramp tests want a cart that is audible, and `(false, false)`
            // already reads as "old behaviour" — this is set so the fixture
            // states its intent rather than relying on that.
            self.horizontal_speed.contains_key(&e)
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

    fn stone(volume: f32, pitch: f32) -> SoundInstance {
        SoundInstance::simple(
            "minecraft:block.stone.break",
            SoundSource::Blocks,
            volume,
            pitch,
            0,
            1.0,
            2.0,
            3.0,
        )
    }

    // ---- the channel budget -----------------------------------------------

    #[test]
    fn the_default_device_splits_thirty_sources_into_twenty_five_and_five() {
        // `(int)Mth.sqrt(30)` truncates 5.477 to 5, not 6.
        assert_eq!(pool_sizes(DEFAULT_CHANNEL_COUNT), (25, 5));
        assert_eq!(DEFAULT_CHANNEL_COUNT, 30);
    }

    #[test]
    fn the_streaming_pool_is_clamped_to_two_through_eight() {
        assert_eq!(pool_sizes(1).1, 2, "sqrt(1)=1 clamps up to 2");
        assert_eq!(pool_sizes(64).1, 8);
        assert_eq!(pool_sizes(10_000).1, 8, "sqrt(10000)=100 clamps down to 8");
    }

    #[test]
    fn the_two_pools_do_not_have_to_sum_to_the_device_count() {
        // The static clamp's LOWER bound is 8, so a tiny device is
        // over-subscribed. This is the property a "how many voices" test would
        // get wrong by asserting the sum.
        let (statics, streaming) = pool_sizes(4);
        assert_eq!((statics, streaming), (8, 2));
        assert_eq!(statics + streaming, 10, "ten channels from a four-source device");
        // And a huge device is under-subscribed by the 255 upper bound.
        let (statics, streaming) = pool_sizes(1000);
        assert_eq!((statics, streaming), (255, 8));
        assert!(statics + streaming < 1000);
    }

    #[test]
    fn the_cast_truncates_and_the_default_device_cannot_show_it() {
        // `(int)Mth.sqrt(total)` is a C-style cast, so it truncates. **The
        // default count cannot witness that**: sqrt(30) is 5.477 and trunc and
        // round both give 5, so a fixture built from `DEFAULT_CHANNEL_COUNT`
        // sits exactly where the two readings agree. 8 separates them —
        // sqrt(8) is 2.828, trunc 2 against round 3.
        assert_eq!((30.0_f32).sqrt() as i32, 5);
        assert_eq!((30.0_f32).sqrt().round() as i32, 5, "the default agrees");
        assert_eq!(pool_sizes(8).1, 2, "trunc; a rounding sqrt would say 3");
        assert_eq!((8.0_f32).sqrt().round() as i32, 3);
    }

    #[test]
    fn a_full_pool_refuses_rather_than_evicting() {
        let mut b = ChannelBudget::from_channel_count(DEFAULT_CHANNEL_COUNT);
        for _ in 0..25 {
            assert!(b.acquire(Pool::Static));
        }
        assert!(!b.acquire(Pool::Static), "no voice stealing anywhere");
        assert_eq!(b.used(Pool::Static), 25, "a refusal does not consume a slot");
        // The other pool is unaffected — they are counted separately.
        assert!(b.acquire(Pool::Streaming));
        b.release(Pool::Static);
        assert!(b.acquire(Pool::Static), "and a release frees exactly one");
    }

    #[test]
    fn the_channel_debug_string_is_vanillas_format() {
        let mut b = ChannelBudget::from_channel_count(DEFAULT_CHANNEL_COUNT);
        b.acquire(Pool::Static);
        b.acquire(Pool::Streaming);
        b.acquire(Pool::Streaming);
        assert_eq!(b.debug_string(), "Sounds: 1/25 + 2/5");
    }

    // ---- the OpenAL curve --------------------------------------------------

    #[test]
    fn the_linear_curve_is_full_at_the_listener_and_silent_at_the_range() {
        assert_eq!(openal::linear_gain(0.0, 16.0), 1.0);
        assert!((openal::linear_gain(8.0, 16.0) - 0.5).abs() < 1e-6);
        assert_eq!(openal::linear_gain(16.0, 16.0), 0.0);
    }

    #[test]
    fn past_the_range_the_curve_clamps_instead_of_going_negative() {
        // The unclamped AL_LINEAR_DISTANCE model really does produce a
        // negative gain past `max`; AL_MIN_GAIN is what stops it.
        assert_eq!(openal::linear_gain(100.0, 16.0), 0.0);
        assert!(1.0 - ROLLOFF_UNCLAMPED_AT_100 > 0.0);
    }
    /// `1 - 100/16` — what the formula gives before the clamp.
    const ROLLOFF_UNCLAMPED_AT_100: f32 = 1.0 - 100.0 / 16.0;

    #[test]
    fn a_zero_range_is_silence_rather_than_a_nan() {
        assert_eq!(openal::linear_gain(0.0, 0.0), 0.0);
        assert!(!openal::linear_gain(5.0, 0.0).is_nan());
    }

    #[test]
    fn vanilla_uses_the_unclamped_linear_model_and_a_zero_reference() {
        // Pinned against the decompile's bare integers, whose names were read
        // from the shipped lwjgl-openal jar.
        assert_eq!(openal::AL_LINEAR_DISTANCE, 53251);
        assert_ne!(openal::AL_LINEAR_DISTANCE, 53252, "not the CLAMPED variant");
        assert_eq!(openal::AL_DISTANCE_MODEL, 53248);
        assert_eq!(openal::AL_NONE, 0);
        assert_eq!(openal::ROLLOFF_FACTOR, 1.0);
        assert_eq!(openal::REFERENCE_DISTANCE, 0.0);
    }

    // ---- play: the call sequence ------------------------------------------

    #[test]
    fn play_issues_vanillas_six_setup_calls_in_order_then_attaches_and_plays() {
        let mut eng = SoundEngine::new();
        let mut dev = RecordingDevice::default();
        let (_, r) = eng.play(stone(1.0, 1.0), &plain_index(), &EmptyWorld, &mut dev);
        assert_eq!(r, PlayResult::Started);
        assert_eq!(
            dev.calls_to(0),
            vec![
                ChannelCall::SetPitch(1.0),
                ChannelCall::SetVolume(1.0),
                ChannelCall::LinearAttenuation(16.0),
                ChannelCall::SetLooping(false),
                ChannelCall::SetSelfPosition(1.0, 2.0, 3.0),
                ChannelCall::SetRelative(false),
                ChannelCall::AttachStaticBuffer("minecraft/sounds/block/stone/break1.ogg".into()),
                ChannelCall::Play,
            ]
        );
    }

    /// Four equally-weighted variants, like the real `block.stone.break`.
    fn four_variant_index() -> SoundsIndex {
        let mut idx = SoundsIndex::new();
        idx.handle_registration(
            "minecraft:block.stone.break",
            &SoundEventRegistration {
                sounds: (1..=4)
                    .map(|i| Sound::file(format!("minecraft:dig/stone{i}")))
                    .collect(),
                replace: false,
                subtitle: None,
            },
            &SoundFileSet::All,
        );
        idx
    }

    #[test]
    fn the_packet_seed_reaches_the_variant_pick() {
        // The whole reason a seed is on the wire: every client hearing the same
        // event must hear the same file.
        //
        // **Two fixture traps, both of which this witness fell into first.**
        // A one-variant event cannot witness anything — any seed picks the only
        // file — so this uses four, which is what a real block sound has. And
        // *small consecutive seeds cannot witness anything either*:
        // `java.util.Random`'s scramble is `seed ^ 0x5DEECE66D` and one step of
        // the LCG does not carry a difference in the low five bits up to the
        // two bits `nextInt(4)` reads, so `new Random(n).nextInt(4)` is **2 for
        // every n in 0..24**. Seeding 0..23 produced one variant twenty-four
        // times over and read as "the seed is ignored". Vanilla's seeds are
        // `level.random.nextLong()` outputs, i.e. full-range, so the fixture
        // spreads them with a 64-bit odd multiplier.
        let idx = four_variant_index();
        let spread = |k: i64| k.wrapping_mul(0x9E37_79B9_7F4A_7C15_u64 as i64);
        let attached = |seed: i64| -> String {
            let mut eng = SoundEngine::new();
            let mut dev = RecordingDevice::default();
            let mut i = stone(1.0, 1.0);
            i.seed = Some(seed);
            eng.play(i, &idx, &EmptyWorld, &mut dev);
            dev.calls_to(0)
                .into_iter()
                .find_map(|c| match c {
                    ChannelCall::AttachStaticBuffer(p) => Some(p),
                    _ => None,
                })
                .expect("a file was attached")
        };

        let picked: std::collections::BTreeSet<String> =
            (1..=24).map(|k| attached(spread(k))).collect();
        assert_eq!(
            picked.len(),
            4,
            "24 spread seeds reached {picked:?} — all four variants must be reachable"
        );
        // Deterministic: the same seed twice is the same file.
        assert_eq!(attached(spread(7)), attached(spread(7)));
        // Pinned against `LegacyRandom48` — M66's transcription of
        // `java.util.Random` — so a change to the generator lands here too.
        let mut rng = rewo_data::sounds_json::LegacyRandom48::new(spread(7));
        let expect = 1 + rewo_data::sounds_json::SoundRandom::next_int(&mut rng, 4);
        assert_eq!(
            attached(spread(7)),
            format!("minecraft/sounds/dig/stone{expect}.ogg")
        );
        // And the small-seed degeneracy itself, so a future gate author does
        // not rediscover it the hard way.
        let small: std::collections::BTreeSet<String> = (0..24).map(attached).collect();
        assert_eq!(
            small.len(),
            1,
            "seeds 0..23 must all land on one variant — that is the trap"
        );
    }

    #[test]
    fn attenuation_none_disables_rather_than_passing_an_infinite_distance() {
        let mut eng = SoundEngine::new();
        let mut dev = RecordingDevice::default();
        let mut i = stone(1.0, 1.0);
        i.attenuation = Attenuation::None;
        eng.play(i, &plain_index(), &EmptyWorld, &mut dev);
        assert!(dev.calls_to(0).contains(&ChannelCall::DisableAttenuation));
        assert!(!dev
            .calls_to(0)
            .iter()
            .any(|c| matches!(c, ChannelCall::LinearAttenuation(_))));
    }

    #[test]
    fn a_streaming_sound_takes_the_streaming_pool_and_loops_in_the_stream() {
        let mut idx = SoundsIndex::new();
        idx.handle_registration(
            "minecraft:music.game",
            &SoundEventRegistration {
                sounds: vec![Sound {
                    stream: true,
                    ..Sound::file("minecraft:music/game/calm1")
                }],
                replace: false,
                subtitle: None,
            },
            &SoundFileSet::All,
        );
        let mut eng = SoundEngine::new();
        let mut dev = RecordingDevice::default();
        let mut i = SoundInstance::for_music("minecraft:music.game");
        i.looping = true;
        eng.play(i, &idx, &EmptyWorld, &mut dev);
        let calls = dev.calls_to(0);
        // The source's own looping flag is CLEARED for a stream…
        assert!(calls.contains(&ChannelCall::SetLooping(false)));
        // …and the looping is handed to the stream instead.
        assert!(calls.contains(&ChannelCall::AttachBufferStream(
            "minecraft/sounds/music/game/calm1.ogg".into(),
            true
        )));
        assert_eq!(dev.budget().used(Pool::Streaming), 1);
        assert_eq!(dev.budget().used(Pool::Static), 0);
    }

    #[test]
    fn the_sounds_json_entrys_own_volume_and_pitch_multiply_the_instances() {
        // `AbstractSoundInstance.getVolume()` is `this.volume *
        // sound.getVolume().sample(random)`. **Every fixture built from
        // `Sound::file` has volume and pitch 1**, where the product equals the
        // instance's own value and the two readings agree — so the mutation
        // that drops the multiplication survived until this existed.
        let mut idx = SoundsIndex::new();
        idx.handle_registration(
            "minecraft:block.stone.break",
            &SoundEventRegistration {
                sounds: vec![Sound {
                    volume: 0.5,
                    pitch: 1.5,
                    attenuation_distance: 8,
                    ..Sound::file("minecraft:dig/stone1")
                }],
                replace: false,
                subtitle: None,
            },
            &SoundFileSet::All,
        );
        let mut eng = SoundEngine::new();
        let mut dev = RecordingDevice::default();
        // Instance volume 0.8, pitch 1.0 → gain 0.4, pitch 1.5.
        eng.play(stone(0.8, 1.0), &idx, &EmptyWorld, &mut dev);
        let calls = dev.calls_to(0);
        assert!(calls.contains(&ChannelCall::SetVolume(0.4)), "{calls:?}");
        assert!(calls.contains(&ChannelCall::SetPitch(1.5)), "{calls:?}");
        // And the *same* product feeds the attenuation distance: 0.4 is below
        // 1, so `max(_, 1)` holds the range at the entry's own 8.
        assert!(calls.contains(&ChannelCall::LinearAttenuation(8.0)), "{calls:?}");

        // Now push the product above 1 so the range moves: 4.0 * 0.5 = 2.0.
        dev.clear_calls();
        eng.play(stone(4.0, 1.0), &idx, &EmptyWorld, &mut dev);
        let calls = dev.calls_to(1);
        assert!(calls.contains(&ChannelCall::SetVolume(1.0)), "gain saturates");
        assert!(
            calls.contains(&ChannelCall::LinearAttenuation(16.0)),
            "range is 2.0 * 8 = 16, not 4.0 * 8 = 32: {calls:?}"
        );
    }

    #[test]
    fn a_non_streaming_loop_keeps_the_source_looping_flag() {
        let mut eng = SoundEngine::new();
        let mut dev = RecordingDevice::default();
        let mut i = stone(1.0, 1.0);
        i.looping = true;
        eng.play(i, &plain_index(), &EmptyWorld, &mut dev);
        assert!(dev.calls_to(0).contains(&ChannelCall::SetLooping(true)));
    }

    // ---- play: the eight refusals -----------------------------------------

    #[test]
    fn an_unloaded_engine_refuses_before_touching_anything() {
        let mut eng = SoundEngine::new();
        eng.set_loaded(false);
        let mut dev = RecordingDevice::default();
        let (_, r) = eng.play(stone(1.0, 1.0), &plain_index(), &EmptyWorld, &mut dev);
        assert_eq!(r, PlayResult::NotStarted(NotStarted::NotLoaded));
        assert!(dev.calls.is_empty());
        assert_eq!(dev.budget().used(Pool::Static), 0);
    }

    #[test]
    fn a_silent_entity_refuses_before_the_event_is_resolved() {
        let mut eng = SoundEngine::new();
        let mut dev = RecordingDevice::default();
        let world = TestWorld {
            positions: HashMap::from([(7, (0.0, 0.0, 0.0))]),
            silent: vec![7],
            ..Default::default()
        };
        let i = SoundInstance::entity_bound(
            "minecraft:block.stone.break",
            SoundSource::Blocks,
            1.0,
            1.0,
            0,
            7,
            0.0,
            0.0,
            0.0,
        );
        let (_, r) = eng.play(i, &plain_index(), &world, &mut dev);
        assert_eq!(r, PlayResult::NotStarted(NotStarted::CannotPlay));
    }

    #[test]
    fn an_unregistered_event_and_an_empty_one_are_different_refusals() {
        let mut eng = SoundEngine::new();
        let mut dev = RecordingDevice::default();
        // Nothing registered at all.
        let (_, r) = eng.play(
            SoundInstance::simple("minecraft:nope", SoundSource::Blocks, 1.0, 1.0, 0, 0.0, 0.0, 0.0),
            &SoundsIndex::new(),
            &EmptyWorld,
            &mut dev,
        );
        assert_eq!(r, PlayResult::NotStarted(NotStarted::UnknownEvent));

        // `minecraft:intentionally_empty` short-circuits before the registry.
        let (_, r) = eng.play(
            SoundInstance::simple(
                rewo_data::sounds_json::INTENTIONALLY_EMPTY_SOUND,
                SoundSource::Blocks,
                1.0,
                1.0,
                0,
                0.0,
                0.0,
                0.0,
            ),
            &SoundsIndex::new(),
            &EmptyWorld,
            &mut dev,
        );
        assert_eq!(r, PlayResult::NotStarted(NotStarted::IntentionallyEmpty));
        assert!(dev.calls.is_empty());
    }

    #[test]
    fn a_registered_event_that_resolves_to_nothing_is_a_different_refusal() {
        // Vanilla asks two questions — `soundEvent == null` and
        // `sound == EMPTY_SOUND` — and M66's index answers both with `None`,
        // so the *reason* is recovered by asking whether the event exists.
        // Both are silence, so collapsing them would be behaviourally right;
        // they are told apart because they want different fixes. Nothing
        // reached this arm until this witness: every other fixture registers
        // an event that resolves, so `UnknownEvent` and `EmptySound` agreed.
        let mut eng = SoundEngine::new();
        let mut dev = RecordingDevice::default();

        // (a) `validateSoundResource` dropped the only variant, so the event
        // exists with no sounds — the real-world case, since a pack can list a
        // file it does not ship.
        let mut idx = SoundsIndex::new();
        idx.handle_registration(
            "minecraft:block.stone.break",
            &SoundEventRegistration {
                sounds: vec![Sound::file("minecraft:dig/absent")],
                replace: false,
                subtitle: None,
            },
            &SoundFileSet::Only(Default::default()),
        );
        assert!(idx.get("minecraft:block.stone.break").is_some(), "registered");
        let (_, r) = eng.play(stone(1.0, 1.0), &idx, &EmptyWorld, &mut dev);
        assert_eq!(r, PlayResult::NotStarted(NotStarted::EmptySound));

        // (b) A total weight of zero — vanilla's `weight != 0` guard.
        let mut idx = SoundsIndex::new();
        idx.handle_registration(
            "minecraft:block.stone.break",
            &SoundEventRegistration {
                sounds: vec![Sound {
                    weight: 0,
                    ..Sound::file("minecraft:dig/stone1")
                }],
                replace: false,
                subtitle: None,
            },
            &SoundFileSet::All,
        );
        let (_, r) = eng.play(stone(1.0, 1.0), &idx, &EmptyWorld, &mut dev);
        assert_eq!(r, PlayResult::NotStarted(NotStarted::EmptySound));

        // …against the event that was never registered at all.
        let (_, r) = eng.play(stone(1.0, 1.0), &SoundsIndex::new(), &EmptyWorld, &mut dev);
        assert_eq!(r, PlayResult::NotStarted(NotStarted::UnknownEvent));
        assert!(dev.calls.is_empty(), "none of the three touched a channel");
    }

    #[test]
    fn a_zero_gain_sound_is_dropped_unless_it_is_music_or_may_start_silent() {
        let idx = plain_index();
        let mut eng = SoundEngine::new();
        eng.options.set_slider(SoundSource::Master, 0.0);
        let mut dev = RecordingDevice::default();

        let (_, r) = eng.play(stone(1.0, 1.0), &idx, &EmptyWorld, &mut dev);
        assert_eq!(r, PlayResult::NotStarted(NotStarted::SilentAndNotAllowed));

        // `canStartSilent` — the flag three tickable subclasses set.
        let mut i = stone(1.0, 1.0);
        i.can_start_silent = true;
        let (_, r) = eng.play(i, &idx, &EmptyWorld, &mut dev);
        assert_eq!(r, PlayResult::StartedSilently);

        // `soundSource != SoundSource.MUSIC` — the other disjunct, which is
        // why `forMusic` does not need the flag.
        let mut music_idx = SoundsIndex::new();
        music_idx.handle_registration(
            "minecraft:music.game",
            &SoundEventRegistration {
                sounds: vec![Sound::file("minecraft:music/game/calm1")],
                replace: false,
                subtitle: None,
            },
            &SoundFileSet::All,
        );
        let (_, r) = eng.play(
            SoundInstance::for_music("minecraft:music.game"),
            &music_idx,
            &EmptyWorld,
            &mut dev,
        );
        assert_eq!(r, PlayResult::StartedSilently);
    }

    #[test]
    fn a_full_pool_drops_the_newest_sound() {
        let idx = plain_index();
        let mut eng = SoundEngine::new();
        // Two static channels only.
        let mut dev = RecordingDevice::default();
        for _ in 0..25 {
            assert!(matches!(
                eng.play(stone(1.0, 1.0), &idx, &EmptyWorld, &mut dev).1,
                PlayResult::Started
            ));
        }
        let (_, r) = eng.play(stone(1.0, 1.0), &idx, &EmptyWorld, &mut dev);
        assert_eq!(r, PlayResult::NotStarted(NotStarted::NoChannel));
        assert_eq!(dev.refusals(), 1);
        // The 25 that were already playing are untouched — no eviction.
        assert_eq!(eng.live_count(), 25);
    }

    // ---- is_active ---------------------------------------------------------

    #[test]
    fn is_active_reduces_to_holding_a_channel() {
        // Vanilla's first branch (`deleteTime <= tickCount`) can only be true
        // when the second is also true, because the two maps are written and
        // erased together. Drive an instance past its delete time while it
        // still holds a channel and check that `is_active` says the same thing
        // as `live_count` sees.
        let idx = plain_index();
        let mut eng = SoundEngine::new();
        let mut dev = RecordingDevice::default();
        let (id, _) = eng.play(stone(1.0, 1.0), &idx, &EmptyWorld, &mut dev);
        assert!(eng.is_active(id));
        for _ in 0..(MIN_SOURCE_LIFETIME + 5) {
            eng.tick(false, &idx, &EmptyWorld, &mut dev);
        }
        // The channel never reported stopped, so it is still held — and still
        // active, even though its delete time is long past.
        assert!(eng.tick_count() > MIN_SOURCE_LIFETIME);
        assert!(eng.is_active(id));
        assert_eq!(eng.live_count(), 1);
    }

    // ---- the grace period and the reclaim ---------------------------------

    #[test]
    fn the_grace_period_is_twenty_ticks_pinned_against_the_decompiles_literal() {
        // `SoundEngine`'s `private static final int MIN_SOURCE_LIFETIME = 20;`.
        //
        // Stated separately because the boundary test below computes its own
        // expectation *from this constant* and is therefore self-calibrating:
        // changing it moves the code and the expectation together, so that
        // test alone cannot see a wrong value. M93r's finding, and this
        // battery caught it here — the 20 → 19 mutation survived until this
        // assertion existed.
        assert_eq!(MIN_SOURCE_LIFETIME, 20);
    }

    #[test]
    fn a_finished_sound_is_not_reclaimed_before_the_minimum_lifetime() {
        let idx = plain_index();
        let mut eng = SoundEngine::new();
        let mut dev = RecordingDevice::default();
        let (id, _) = eng.play(stone(1.0, 1.0), &idx, &EmptyWorld, &mut dev);
        dev.finish(0);
        // `soundDeleteTime` is `tickCount + MIN_SOURCE_LIFETIME` = 20, and the
        // reclaim test is `minDeleteTime <= tickCount` — so tick 20 IS the
        // reclaim tick and tick 19 is the last one on which it is still held.
        // Asserted from both sides, because an off-by-one in either direction
        // is exactly what this witness exists to catch (and it caught one: the
        // first version of it looped 20 times and expected 20 survivals).
        for _ in 0..(MIN_SOURCE_LIFETIME - 1) {
            eng.tick(false, &idx, &EmptyWorld, &mut dev);
            assert!(eng.is_active(id), "at tick {}", eng.tick_count());
        }
        assert_eq!(eng.tick_count(), 19, "literal, not derived from the const");
        eng.tick(false, &idx, &EmptyWorld, &mut dev);
        assert_eq!(eng.tick_count(), 20);
        assert!(!eng.is_active(id), "reclaimed when deleteTime <= tickCount");
    }

    #[test]
    fn schedule_tick_runs_last_so_a_stop_cannot_be_reclaimed_on_its_own_tick() {
        let idx = plain_index();
        let mut eng = SoundEngine::new();
        let mut dev = RecordingDevice::default();
        eng.play(stone(1.0, 1.0), &idx, &EmptyWorld, &mut dev);
        // Age past the grace period first, with the channel still running.
        for _ in 0..(MIN_SOURCE_LIFETIME + 1) {
            eng.tick(false, &idx, &EmptyWorld, &mut dev);
        }
        assert_eq!(eng.live_count(), 1);
        dev.finish(0);
        // This tick's reclaim loop runs BEFORE schedule_tick sees the stop…
        eng.tick(false, &idx, &EmptyWorld, &mut dev);
        assert_eq!(eng.live_count(), 1, "the one-tick lag is real");
        // …and the next one collects it.
        eng.tick(false, &idx, &EmptyWorld, &mut dev);
        assert_eq!(eng.live_count(), 0);
    }

    #[test]
    fn a_manual_loop_is_requeued_and_an_automatic_one_is_not() {
        let idx = plain_index();
        let mut eng = SoundEngine::new();
        let mut dev = RecordingDevice::default();
        let mut i = stone(1.0, 1.0);
        i.looping = true;
        i.delay = 3;
        assert!(i.should_loop_manually());
        eng.play(i, &idx, &EmptyWorld, &mut dev);
        dev.finish(0);
        for _ in 0..(MIN_SOURCE_LIFETIME + 2) {
            eng.tick(false, &idx, &EmptyWorld, &mut dev);
        }
        // Reclaimed and requeued, then replayed `delay` ticks later on a new
        // channel.
        for _ in 0..4 {
            eng.tick(false, &idx, &EmptyWorld, &mut dev);
        }
        assert_eq!(eng.live_count(), 1);
        assert!(dev.calls.iter().any(|(c, _)| *c == 1), "a second channel was taken");
    }

    // ---- ticking an entity-bound sound ------------------------------------

    #[test]
    fn an_entity_bound_sound_follows_its_entity_and_stops_when_it_is_removed() {
        let idx = plain_index();
        let mut eng = SoundEngine::new();
        let mut dev = RecordingDevice::default();
        let mut world = TestWorld {
            positions: HashMap::from([(7, (0.0, 64.0, 0.0))]),
            silent: vec![],
            ..Default::default()
        };
        let i = SoundInstance::entity_bound(
            "minecraft:block.stone.break",
            SoundSource::Blocks,
            1.0,
            1.0,
            0,
            7,
            0.0,
            64.0,
            0.0,
        );
        eng.play(i, &idx, &world, &mut dev);
        dev.clear_calls();

        world.positions.insert(7, (10.0, 65.0, -3.0));
        eng.tick(false, &idx, &world, &mut dev);
        assert!(dev
            .calls_to(0)
            .contains(&ChannelCall::SetSelfPosition(10.0, 65.0, -3.0)));

        dev.clear_calls();
        world.positions.remove(&7);
        eng.tick(false, &idx, &world, &mut dev);
        assert!(dev.calls_to(0).contains(&ChannelCall::Stop));
    }

    /// **The per-tick refresh is narrowed through f32**, exactly as the
    /// constructor's read is: `EntityBoundSoundInstance.java:33-35` is three
    /// `this.x = (float)this.entity.getX();` assignments into `double` fields.
    ///
    /// The test above cannot witness this and never could: it moves the entity
    /// to `(10.0, 65.0, -3.0)`, and all three are exactly representable in
    /// f32, so a narrowing and a non-narrowing client agree to the bit. That
    /// is why the refresh shipped un-narrowed with a comment asserting it was
    /// correct. **A fixture sitting where two readings coincide is this
    /// project's most-repeated failure shape**, so this one picks a coordinate
    /// where they do not.
    #[test]
    fn an_entity_bound_sound_position_is_narrowed_through_f32() {
        let idx = plain_index();
        let mut eng = SoundEngine::new();
        let mut dev = RecordingDevice::default();
        // 1_000_000.1 has no f32 representation; the nearest is 1000000.0625.
        // A block sound a million blocks out is unusual, but the *divergence*
        // begins as soon as a coordinate needs more than 24 bits of mantissa,
        // which is a few million blocks for the integer part alone and much
        // sooner for the fraction — an entity at x = 131072.1 is already off.
        let far = 1_000_000.1_f64;
        let mut world = TestWorld {
            positions: HashMap::from([(7, (0.0, 64.0, 0.0))]),
            silent: vec![],
            ..Default::default()
        };
        let i = SoundInstance::entity_bound(
            "minecraft:block.stone.break",
            SoundSource::Blocks,
            1.0,
            1.0,
            0,
            7,
            0.0,
            64.0,
            0.0,
        );
        eng.play(i, &idx, &world, &mut dev);
        dev.clear_calls();

        world.positions.insert(7, (far, 64.0, 0.0));
        eng.tick(false, &idx, &world, &mut dev);

        let narrowed = far as f32 as f64;
        assert_ne!(narrowed, far, "the fixture must not sit where both agree");
        assert!(
            dev.calls_to(0)
                .contains(&ChannelCall::SetSelfPosition(narrowed, 64.0, 0.0)),
            "expected the f32-narrowed x, got {:?}",
            dev.calls_to(0)
        );
    }

    // ---- M141e: the elytra, the first ramp anything constructs ------------

    /// **The first tickable sound this client can start.** Driven through
    /// `SoundSystem::accept` rather than `play_ramped` directly, so what is
    /// graded is the queue path a `SoundEvent::Tickable` really takes.
    #[test]
    fn a_tickable_event_starts_a_ramped_sound() {
        let idx = index_of(&["minecraft:item.elytra.flying"]);
        let mut sys = SoundSystem::new(idx);
        let mut dev = RecordingDevice::default();
        let t = rewo_world::entities::EntityTable::default();
        let world = EntityTableWorld {
            table: &t,
            local: Some(LocalPlayerView {
                id: 42,
                position: (1.0, 100.0, 2.0),
                // Past the 1e-7 floor and below the 4.0 that saturates it, so
                // the ramp's answer is neither 0 nor 1: 2.0/4 = 0.5.
                velocity: (0.0, 0.0, 0.0),
                fall_flying: true,
                underwater: false,
            }),
            game_time: 0,
        };
        sys.accept(
            &[SoundEvent::Tickable(
                crate::sounds::TickableSound::ElytraOnPlayer { player: 42 },
            )],
            &registry(),
            &world,
            &mut dev,
        );
        assert_eq!(sys.stats.started, 1, "stats: {:?}", sys.stats);
        assert_eq!(
            sys.engine.live_identifiers(),
            vec!["minecraft:item.elytra.flying"]
        );
        // It LOOPS — `ElytraOnPlayerSoundInstance` sets `looping = true`, and
        // a one-shot would end after one buffer however well the ramp ran.
        // The battery found this untested.
        assert!(
            dev.calls_to(0).contains(&ChannelCall::SetLooping(true)),
            "the elytra sound must loop; got {:?}",
            dev.calls_to(0)
        );
        // …and it is NOT silence-gated: it does not override `canPlaySound`.
        assert_eq!(
            crate::tickable::Ramp::Elytra(crate::tickable::ElytraRamp { player: 42, time: 0 })
                .silence_gated_entity(),
            None
        );

        // …and it is RAMPED: forty ticks of a fall-flying player with a real
        // speed must push the ramp's volume, which the fade puts at
        // `clamp(speed_sqr/4) * 1.0` once `time >= 40`.
        let world = EntityTableWorld {
            table: &t,
            local: Some(LocalPlayerView {
                id: 42,
                position: (1.0, 100.0, 2.0),
                velocity: (1.0, 0.0, 1.0), // lengthSqr 2.0 → volume 0.5
                fall_flying: true,
                underwater: false,
            }),
            game_time: 0,
        };
        for _ in 0..40 {
            sys.tick(false, &world, &mut dev);
        }
        assert!(
            dev.calls_to(0).contains(&ChannelCall::SetVolume(0.5)),
            "the elytra ramp must reach the device; got {:?}",
            dev.calls_to(0)
        );
    }

    /// **The ramp stops when the player stops flying** — and not before its
    /// twentieth tick, because the guard is `time <= 20 || isFallFlying()`.
    #[test]
    fn the_elytra_ramp_stops_when_the_player_lands() {
        let idx = index_of(&["minecraft:item.elytra.flying"]);
        let mut sys = SoundSystem::new(idx);
        let mut dev = RecordingDevice::default();
        let t = rewo_world::entities::EntityTable::default();
        let grounded = EntityTableWorld {
            table: &t,
            local: Some(LocalPlayerView {
                id: 42,
                position: (1.0, 100.0, 2.0),
                velocity: (0.0, 0.0, 0.0),
                fall_flying: false,
                underwater: false,
            }),
            game_time: 0,
        };
        sys.accept(
            &[SoundEvent::Tickable(
                crate::sounds::TickableSound::ElytraOnPlayer { player: 42 },
            )],
            &registry(),
            &grounded,
            &mut dev,
        );
        dev.clear_calls();
        for tick in 1..=20 {
            sys.tick(false, &grounded, &mut dev);
            assert!(
                !dev.calls_to(0).contains(&ChannelCall::Stop),
                "tick {tick} must survive: the guard is `time <= 20 || flying`"
            );
        }
        sys.tick(false, &grounded, &mut dev);
        assert!(dev.calls_to(0).contains(&ChannelCall::Stop), "tick 21 stops");
    }

    /// **A silenced player does not silence their elytra**, because
    /// `ElytraOnPlayerSoundInstance` takes the interface default for
    /// `canPlaySound()`. The tick loop's stop must therefore ask the ramp's
    /// rule and not the entity it follows.
    ///
    /// The battery found this untested: swapping `silence_gated_entity()` for
    /// `entity()` in the tick loop survived, because nothing had a *live,
    /// ticking* ramp whose entity was silent.
    #[test]
    fn a_silenced_player_does_not_silence_their_elytra() {
        let idx = index_of(&["minecraft:item.elytra.flying"]);
        let mut eng = SoundEngine::new();
        let mut dev = RecordingDevice::default();
        let world = TestWorld {
            positions: HashMap::from([(7, (0.0, 100.0, 0.0))]),
            silent: vec![7],
            ..Default::default()
        };
        let inst = SoundInstance {
            volume: 0.1,
            looping: true,
            ..SoundInstance::bare("minecraft:item.elytra.flying", SoundSource::Players)
        };
        eng.play_ramped(
            inst,
            Some(crate::tickable::Ramp::Elytra(crate::tickable::ElytraRamp {
                player: 7,
                time: 0,
            })),
            &idx,
            &world,
            &mut dev,
        );
        assert_eq!(eng.live_count(), 1, "a silenced player still starts it");
        dev.clear_calls();
        eng.tick(false, &idx, &world, &mut dev);
        assert!(
            !dev.calls_to(0).contains(&ChannelCall::Stop),
            "no override, no gate; got {:?}",
            dev.calls_to(0)
        );

        // …whereas a bee, which DOES override it, stops.
        //
        // It has to fall silent *after* starting: `play` refuses a silent
        // entity outright (`Binding::Entity` gates there), so a bee that was
        // silent all along never acquires a channel and the TICK-time gate —
        // which is what this is about — is unobservable. The first cut did
        // exactly that and measured an empty call list.
        let mut eng = SoundEngine::new();
        let mut dev = RecordingDevice::default();
        let idx = index_of(&["minecraft:entity.bee.loop"]);
        let mut audible = TestWorld {
            positions: HashMap::from([(7, (0.0, 100.0, 0.0))]),
            ..Default::default()
        };
        eng.play_ramped(
            crate::tickable::bee_instance(crate::tickable::BeeLoop::Flying, 7, (0.0, 100.0, 0.0)),
            Some(crate::tickable::Ramp::Bee(crate::tickable::BeeRamp {
                bee: 7,
                loop_kind: crate::tickable::BeeLoop::Flying,
                has_switched: false,
            })),
            &idx,
            &audible,
            &mut dev,
        );
        assert_eq!(eng.live_count(), 1, "an audible bee starts");
        dev.clear_calls();
        audible.silent.push(7);
        eng.tick(false, &idx, &audible, &mut dev);
        assert!(
            dev.calls_to(0).contains(&ChannelCall::Stop),
            "a bee IS gated; got {:?}",
            dev.calls_to(0)
        );
    }

    /// A tickable naming an entity the world does not know is dropped, like
    /// any other entity-addressed sound — and **that includes the local player
    /// when no `LocalPlayerView` is supplied**, which is the failure mode
    /// `local: None` produces.
    #[test]
    fn a_tickable_for_an_unknown_entity_is_dropped() {
        let idx = index_of(&["minecraft:item.elytra.flying"]);
        let mut sys = SoundSystem::new(idx);
        let mut dev = RecordingDevice::default();
        let t = rewo_world::entities::EntityTable::default();
        let world = EntityTableWorld {
            table: &t,
            local: None,
            game_time: 0,
        };
        sys.accept(
            &[SoundEvent::Tickable(
                crate::sounds::TickableSound::ElytraOnPlayer { player: 42 },
            )],
            &registry(),
            &world,
            &mut dev,
        );
        assert_eq!(sys.stats.started, 0);
        assert_eq!(sys.stats.no_instance, 1);
    }

    /// **`play` and the queue path must agree about the ramp.** An
    /// entity-bound sound that went through `accept` used to get its
    /// `EntityBound` ramp from `play`; routing tickables through `play_ramped`
    /// made it possible to pass `None` there instead and silently stop every
    /// `sound_entity` following its entity. `Ramp::for_instance` is the one
    /// derivation, and this is the witness the engine's own follow test could
    /// not be (it calls `play` directly).
    #[test]
    fn an_entity_bound_sound_queued_through_accept_still_follows() {
        // The index must carry the name the REGISTRY resolves id 0 to, not the
        // one `plain_index` happens to hold — the first cut used the latter
        // and the engine correctly refused an unresolvable event.
        let idx = index_of(&["minecraft:entity.allay.ambient_with_item"]);
        let mut sys = SoundSystem::new(idx);
        let mut dev = RecordingDevice::default();
        let mut world = TestWorld {
            positions: HashMap::from([(7, (0.0, 64.0, 0.0))]),
            ..Default::default()
        };
        sys.accept(
            &[SoundEvent::OnEntity(crate::sounds::EntitySound {
                sound: crate::sounds::SoundRef::Registry(0),
                source: SoundSource::Blocks,
                entity_id: 7,
                volume: 1.0,
                pitch: 1.0,
                seed: 0,
            })],
            &registry(),
            &world,
            &mut dev,
        );
        assert_eq!(sys.stats.started, 1, "stats: {:?}", sys.stats);
        dev.clear_calls();
        world.positions.insert(7, (10.0, 65.0, -3.0));
        sys.tick(false, &world, &mut dev);
        assert!(
            dev.calls_to(0)
                .contains(&ChannelCall::SetSelfPosition(10.0, 65.0, -3.0)),
            "it must still follow; got {:?}",
            dev.calls_to(0)
        );
    }

    // ---- M141h: the riding pair ------------------------------------------

    /// **`startRiding`'s minecart arm plays TWO instances and both stay live**,
    /// each muting itself on the wrong side of the waterline. Picking one would
    /// leave you silent for whichever half you guessed wrong.
    #[test]
    fn a_minecart_ride_starts_both_loops_and_mutes_the_wrong_one() {
        let idx = index_of(&[
            "minecraft:entity.minecart.inside",
            "minecraft:entity.minecart.inside.underwater",
        ]);
        let mut sys = SoundSystem::new(idx);
        let mut dev = RecordingDevice::default();
        let mut table = rewo_world::entities::EntityTable::default();
        table.add(
            2,
            rewo_world::entities::EntityState::new(0, 0, 0.0, 64.0, 0.0, 0.0, 0.0),
        );
        table.set_passengers(2, vec![1]);
        // Both loops read the RIDER's submersion, not the cart's — that is
        // `RidingMinecartSoundInstance`'s override.
        let dry = EntityTableWorld {
            table: &table,
            local: Some(LocalPlayerView {
                id: 1,
                position: (0.0, 64.0, 0.0),
                velocity: (0.0, 0.0, 0.0),
                fall_flying: false,
                underwater: false,
            }),
            game_time: 0,
        };

        let events: Vec<SoundEvent> = crate::sounds::RIDING_MINECART
            .into_iter()
            .map(|mut r| {
                r.player = 1;
                r.vehicle = 2;
                SoundEvent::Tickable(crate::sounds::TickableSound::Riding(r))
            })
            .collect();
        sys.accept(&events, &registry(), &dry, &mut dev);
        assert_eq!(sys.stats.started_silently, 2, "stats: {:?}", sys.stats);
        let mut live = sys.engine.live_identifiers();
        live.sort_unstable();
        assert_eq!(
            live,
            vec![
                "minecraft:entity.minecart.inside",
                "minecraft:entity.minecart.inside.underwater"
            ]
        );
    }

    /// **The ghast's and the nautilus's `underwaterSound` are opposite
    /// constants for the same class** — so with a dry rider the ghast's loop
    /// ramps and the nautilus's holds at its minimum.
    #[test]
    fn the_ghast_and_nautilus_specs_are_opposite() {
        assert!(!crate::sounds::RIDING_HAPPY_GHAST.underwater_sound);
        assert!(crate::sounds::RIDING_NAUTILUS.underwater_sound);
        // Same class, same amplifier, different sound and side.
        assert_eq!(
            crate::sounds::RIDING_HAPPY_GHAST.volume_amplifier,
            crate::sounds::RIDING_NAUTILUS.volume_amplifier
        );
        assert!(!crate::sounds::RIDING_HAPPY_GHAST.is_minecart);
        assert!(!crate::sounds::RIDING_NAUTILUS.is_minecart);
        // …and the minecart's pair differs from both in max and amplifier.
        for r in crate::sounds::RIDING_MINECART {
            assert!(r.is_minecart);
            assert_eq!(r.volume_max, 0.75);
            assert_eq!(r.volume_amplifier, 1.0);
        }
        // The two minecart instances differ ONLY in side and sound.
        let [wet, dry] = crate::sounds::RIDING_MINECART;
        assert!(wet.underwater_sound && !dry.underwater_sound);
        assert_ne!(wet.sound, dry.sound);
        assert_eq!(wet.volume_max, dry.volume_max);
    }

    /// **The riding loop is `Attenuation.NONE`** — you are sitting on the
    /// thing — and it is silence-gated on the **vehicle**, so a silenced cart
    /// silences its rider's loop.
    #[test]
    fn a_riding_loop_is_unattenuated_and_gated_on_the_vehicle() {
        let idx = index_of(&["minecraft:entity.happy_ghast.riding"]);
        let mut sys = SoundSystem::new(idx);
        let mut dev = RecordingDevice::default();
        let mut table = rewo_world::entities::EntityTable::default();
        table.add(
            2,
            rewo_world::entities::EntityState::new(0, 0, 0.0, 100.0, 0.0, 0.0, 0.0),
        );
        table.set_passengers(2, vec![1]);
        let world = EntityTableWorld {
            table: &table,
            local: Some(LocalPlayerView {
                id: 1,
                position: (0.0, 100.0, 0.0),
                velocity: (0.0, 0.0, 0.0),
                fall_flying: false,
                underwater: false,
            }),
            game_time: 0,
        };
        let mut spec = crate::sounds::RIDING_HAPPY_GHAST;
        spec.player = 1;
        spec.vehicle = 2;
        sys.accept(
            &[SoundEvent::Tickable(crate::sounds::TickableSound::Riding(
                spec,
            ))],
            &registry(),
            &world,
            &mut dev,
        );
        assert_eq!(sys.stats.started_silently, 1, "stats: {:?}", sys.stats);
        assert!(
            dev.calls_to(0).contains(&ChannelCall::DisableAttenuation),
            "got {:?}",
            dev.calls_to(0)
        );
        assert_eq!(
            crate::tickable::Ramp::Riding(crate::tickable::RidingRamp {
                player: 1,
                vehicle: 2,
                underwater_sound: false,
                volume_min: 0.0,
                volume_max: 1.0,
                volume_amplifier: 5.0,
                is_minecart: false,
            })
            .silence_gated_entity(),
            Some(2),
            "the VEHICLE, not the rider"
        );

        // **And the BINDING is the vehicle too**, which is a different
        // mechanism from the ramp's gate and needs its own witness: the
        // binding refuses at `play`, the ramp stops later. Asserting only the
        // ramp left `Binding::Entity(r.player)` alive in the battery — the
        // same confusion M141f hit on the minecart.
        let silent = TestWorld {
            positions: HashMap::from([(1, (0.0, 100.0, 0.0)), (2, (0.0, 100.0, 0.0))]),
            silent: vec![2], // the VEHICLE is silent; the rider is not
            ..Default::default()
        };
        let mut sys = SoundSystem::new(index_of(&["minecraft:entity.happy_ghast.riding"]));
        let mut dev = RecordingDevice::default();
        sys.accept(
            &[SoundEvent::Tickable(crate::sounds::TickableSound::Riding(
                spec,
            ))],
            &registry(),
            &silent,
            &mut dev,
        );
        assert_eq!(sys.stats.not_started, 1, "a silenced vehicle refuses it");
        assert_eq!(sys.engine.live_count(), 0);
    }

    /// `World::is_water_at_point` — the eye test's data half (M141h).
    ///
    /// The battery found it untested: the whole body could be replaced with
    /// `false` and nothing noticed, because every riding witness supplies
    /// `underwater` directly rather than deriving it.
    #[test]
    fn the_water_point_query_reads_the_block_it_is_given() {
        let shape = rewo_world::dimension::DimensionShape::OVERWORLD;
        let mut world = rewo_world::World::new(shape);
        // `set_block` writes into an EXISTING column and is otherwise a no-op,
        // so the columns have to be there first — the first cut of this test
        // wrote into an empty world and measured the write being dropped.
        for (cx, cz) in [(0, 0), (-1, 0)] {
            world.insert_column(cx, cz, rewo_world::chunk::Column::empty_lit(&shape, cx, cz));
        }
        // State 1 is water, 0 is not.
        let water = vec![false, true];
        world.set_block(0, 64, 0, 1);
        assert!(world.is_water_at_point(0.5, 64.5, 0.5, &water));
        // …the block below is not, and `floor` is what picks between them.
        assert!(!world.is_water_at_point(0.5, 63.5, 0.5, &water));
        assert!(!world.is_water_at_point(1.5, 64.5, 0.5, &water), "next column");
        // A negative coordinate floors AWAY from zero, which `as i32` alone
        // would not: -0.5 is in block -1, not block 0.
        world.set_block(-1, 64, 0, 1);
        assert!(world.is_water_at_point(-0.5, 64.5, 0.5, &water));
        // An out-of-range state id is not water rather than a panic.
        assert!(!world.is_water_at_point(0.5, 64.5, 0.5, &[]));
    }

    /// The local player's submersion reaches the ramp world, and **only the
    /// local player's** — a remote entity still reads dry.
    #[test]
    fn the_local_players_submersion_reaches_the_ramp_world() {
        use crate::tickable::RampWorld;
        let t = rewo_world::entities::EntityTable::default();
        let w = EntityTableWorld {
            table: &t,
            local: Some(LocalPlayerView {
                id: 1,
                position: (0.0, 60.0, 0.0),
                velocity: (0.0, 0.0, 0.0),
                fall_flying: false,
                underwater: true,
            }),
            game_time: 0,
        };
        assert!(w.underwater(1));
        assert!(!w.underwater(2), "a remote entity has no eye height here");
    }

    // ---- M141g: the guardian and the sniffer -----------------------------

    /// **The guardian's counter runs only while it has a target, and never
    /// counts down.** What zeroes it is the *metadata* arriving, not the
    /// target going away.
    #[test]
    fn the_guardian_counter_climbs_while_targeting_and_resets_on_the_packet() {
        use crate::tickable::RampWorld;
        let mut t = rewo_world::entities::EntityTable::default();
        t.add(
            5,
            rewo_world::entities::EntityState::new(0, 0, 0.0, 40.0, 0.0, 0.0, 0.0),
        );
        // No target yet: `hasActiveAttackTarget()` is `!= 0`, and 0 means none.
        t.set_guardian_attack_target(5, 0);
        for _ in 0..10 {
            t.tick_lerp();
        }
        let w = |t: &rewo_world::entities::EntityTable| {
            EntityTableWorld {
                table: t,
                local: None,
                game_time: 0,
            }
            .attack_animation_scale(5)
        };
        assert_eq!(w(&t), 0.0, "no target, no wind-up");

        t.set_guardian_attack_target(5, 77);
        for _ in 0..20 {
            t.tick_lerp();
        }
        assert!((w(&t) - 0.25).abs() < 1e-6, "20/80, got {}", w(&t));

        // Losing the target holds the counter where it is — vanilla's `if`
        // has no else, so nothing decrements.
        let held = w(&t);
        t.set_guardian_attack_target(5, 77); // same value…
        assert_eq!(w(&t), 0.0, "…and the ARRIVAL resets it, change or not");
        assert!(held > 0.0);
    }

    /// The counter caps, so the scale saturates at 1 rather than running past
    /// it — a guardian that holds its target does not get louder forever.
    #[test]
    fn the_guardian_scale_saturates() {
        use crate::tickable::RampWorld;
        let mut t = rewo_world::entities::EntityTable::default();
        t.add(
            5,
            rewo_world::entities::EntityState::new(0, 0, 0.0, 40.0, 0.0, 0.0, 0.0),
        );
        t.set_guardian_attack_target(5, 77);
        for _ in 0..500 {
            t.tick_lerp();
        }
        let w = EntityTableWorld {
            table: &t,
            local: None,
            game_time: 0,
        };
        assert!((w.attack_animation_scale(5) - 1.0).abs() < 1e-6);
    }

    /// **Two sniffer states keep the sound alive, not one.** Reading only
    /// DIGGING cuts it exactly when the sniffer has found something.
    #[test]
    fn the_sniffer_digs_audibly_while_searching_too() {
        use crate::tickable::RampWorld;
        let mut t = rewo_world::entities::EntityTable::default();
        t.add(
            4,
            rewo_world::entities::EntityState::new(0, 0, 0.0, 64.0, 0.0, 0.0, 0.0),
        );
        let w = |t: &rewo_world::entities::EntityTable| {
            EntityTableWorld {
                table: t,
                local: None,
                game_time: 0,
            }
            .sniffer_digging(4)
        };
        for (state, want) in [(0u8, false), (3, false), (4, true), (5, true), (6, false)] {
            t.set_gesture_state(4, state);
            assert_eq!(w(&t), want, "state {state}");
        }
    }

    /// **The guardian's beam has `Attenuation.NONE`** — heard at full gain
    /// wherever you are, which is what makes it a warning rather than an
    /// ambience — and its ramp is silence-gated, unlike the elytra's.
    #[test]
    fn a_guardian_event_starts_an_unattenuated_ramped_beam() {
        let idx = index_of(&["minecraft:entity.guardian.attack"]);
        let mut sys = SoundSystem::new(idx);
        let mut dev = RecordingDevice::default();
        let mut table = rewo_world::entities::EntityTable::default();
        table.add(
            5,
            rewo_world::entities::EntityState::new(0, 0, 0.0, 40.0, 0.0, 0.0, 0.0),
        );
        table.set_guardian_attack_target(5, 77);
        for _ in 0..40 {
            table.tick_lerp(); // scale 40/80 = 0.5
        }
        let world = EntityTableWorld {
            table: &table,
            local: None,
            game_time: 0,
        };
        sys.accept(
            &[SoundEvent::Tickable(
                crate::sounds::TickableSound::GuardianAttack { guardian: 5 },
            )],
            &registry(),
            &world,
            &mut dev,
        );
        assert_eq!(sys.stats.started, 1, "stats: {:?}", sys.stats);
        assert!(
            dev.calls_to(0).contains(&ChannelCall::DisableAttenuation),
            "Attenuation.NONE; got {:?}",
            dev.calls_to(0)
        );

        // One tick of the ramp: volume is the scale SQUARED (0.25) while the
        // pitch is linear in it (0.7 + 0.5*0.5 = 0.95).
        dev.clear_calls();
        sys.tick(false, &world, &mut dev);
        assert!(
            dev.calls_to(0).contains(&ChannelCall::SetVolume(0.25)),
            "squared, not linear; got {:?}",
            dev.calls_to(0)
        );
        assert!(dev.calls_to(0).contains(&ChannelCall::SetPitch(0.95)));
    }

    /// **The sniffer's is the one tickable here that does NOT loop** — its
    /// ramp's job is the stop condition, not a fade.
    #[test]
    fn a_sniffer_event_starts_a_one_shot_that_stops_when_it_stops_digging() {
        let idx = index_of(&["minecraft:entity.sniffer.digging"]);
        let mut sys = SoundSystem::new(idx);
        let mut dev = RecordingDevice::default();
        let mut table = rewo_world::entities::EntityTable::default();
        table.add(
            4,
            rewo_world::entities::EntityState::new(0, 0, 0.0, 64.0, 0.0, 0.0, 0.0),
        );
        table.set_gesture_state(4, 5); // DIGGING
        fn world(t: &rewo_world::entities::EntityTable) -> EntityTableWorld<'_> {
            EntityTableWorld {
                table: t,
                local: None,
                game_time: 0,
            }
        }
        sys.accept(
            &[SoundEvent::Tickable(
                crate::sounds::TickableSound::SnifferDigging { sniffer: 4 },
            )],
            &registry(),
            &world(&table),
            &mut dev,
        );
        assert_eq!(sys.stats.started, 1, "stats: {:?}", sys.stats);
        assert!(
            dev.calls_to(0).contains(&ChannelCall::SetLooping(false)),
            "a one-shot; got {:?}",
            dev.calls_to(0)
        );

        dev.clear_calls();
        sys.tick(false, &world(&table), &mut dev);
        assert!(!dev.calls_to(0).contains(&ChannelCall::Stop), "still digging");

        table.set_gesture_state(4, 0); // IDLING
        sys.tick(false, &world(&table), &mut dev);
        assert!(dev.calls_to(0).contains(&ChannelCall::Stop), "stopped digging");
    }

    // ---- M141f: the bee and the minecart ---------------------------------

    /// **`bee.isAngry()` is a deadline against the clock, so the answer changes
    /// with no packet arriving.** A world that stored a boolean would freeze a
    /// bee's anger at whatever it was when the last metadata came in.
    #[test]
    fn a_bees_anger_expires_without_another_packet() {
        use crate::tickable::RampWorld;
        let mut t = rewo_world::entities::EntityTable::default();
        t.add(
            9,
            rewo_world::entities::EntityState::new(0, 0, 0.0, 64.0, 0.0, 0.0, 0.0),
        );
        t.set_anger_end_time(9, 100);
        let angry = EntityTableWorld {
            table: &t,
            local: None,
            game_time: 99,
        };
        assert!(angry.angry(9));
        // One tick later the deadline has passed — same table, same entity, no
        // packet in between.
        let calm = EntityTableWorld {
            table: &t,
            local: None,
            game_time: 100,
        };
        assert!(!calm.angry(9), "the second test is strict");

        // An entity that never sent one is calm, and that is exact rather than
        // a fallback: `Bee` seeds the accessor to -1.
        assert!(!angry.angry(404));
    }

    /// **A spawning minecart brings its own loop**, and it is silence-gated —
    /// unlike the elytra, `MinecartSoundInstance` overrides `canPlaySound()`.
    #[test]
    fn a_spawning_minecart_starts_a_ramped_loop() {
        let idx = index_of(&["minecraft:entity.minecart.riding"]);
        let mut sys = SoundSystem::new(idx);
        let mut dev = RecordingDevice::default();
        let world = TestWorld {
            positions: HashMap::from([(5, (1.0, 64.0, 2.0))]),
            horizontal_speed: HashMap::from([(5, 0.5)]),
            ..Default::default()
        };
        sys.accept(
            &[SoundEvent::Tickable(
                crate::sounds::TickableSound::MinecartRiding { minecart: 5 },
            )],
            &registry(),
            &world,
            &mut dev,
        );
        // It starts at volume 0 and `canStartSilent`, so `play` allows it
        // through as SILENT rather than refusing it — the pair that lets a
        // stationary cart's loop exist at all.
        assert_eq!(sys.stats.started_silently, 1, "stats: {:?}", sys.stats);
        assert_eq!(sys.stats.not_started, 0);

        // …and one tick of the ramp gives its ceiling of 0.35, not 0.7.
        sys.tick(false, &world, &mut dev);
        assert!(
            dev.calls_to(0).contains(&ChannelCall::SetVolume(0.35)),
            "got {:?}",
            dev.calls_to(0)
        );
        assert_eq!(
            crate::tickable::Ramp::Minecart(crate::tickable::MinecartRamp {
                minecart: 5,
                shadowed_pitch: 0.0
            })
            .silence_gated_entity(),
            Some(5),
            "a minecart DOES override canPlaySound"
        );

        // **And the gate applies at `play` too, which is the BINDING's job
        // rather than the ramp's.** The two are different mechanisms — the
        // binding refuses a silent entity before a channel is acquired, the
        // ramp stops one that falls silent later — and asserting only the ramp
        // left `Binding::Fixed` alive in the battery.
        let silent = TestWorld {
            positions: HashMap::from([(5, (1.0, 64.0, 2.0))]),
            silent: vec![5],
            ..Default::default()
        };
        let mut sys = SoundSystem::new(index_of(&["minecraft:entity.minecart.riding"]));
        let mut dev = RecordingDevice::default();
        sys.accept(
            &[SoundEvent::Tickable(
                crate::sounds::TickableSound::MinecartRiding { minecart: 5 },
            )],
            &registry(),
            &silent,
            &mut dev,
        );
        assert_eq!(sys.stats.started_silently, 0, "stats: {:?}", sys.stats);
        assert_eq!(sys.stats.not_started, 1, "a silent cart is refused");
        assert_eq!(sys.engine.live_count(), 0);
    }

    /// **The bee's loop is chosen once, at spawn**, from its anger then — and
    /// the spec carries which loop rather than the bee's id alone, so
    /// resolving it twice cannot re-decide.
    #[test]
    fn a_spawning_bee_starts_the_loop_its_anger_chose() {
        let idx = index_of(&[
            "minecraft:entity.bee.loop",
            "minecraft:entity.bee.loop_aggressive",
        ]);
        let world = TestWorld {
            positions: HashMap::from([(9, (0.0, 64.0, 0.0))]),
            ..Default::default()
        };
        for (aggressive, want) in [
            (false, "minecraft:entity.bee.loop"),
            (true, "minecraft:entity.bee.loop_aggressive"),
        ] {
            let mut sys = SoundSystem::new(idx.clone());
            let mut dev = RecordingDevice::default();
            sys.accept(
                &[SoundEvent::Tickable(
                    crate::sounds::TickableSound::BeeLoop { bee: 9, aggressive },
                )],
                &registry(),
                &world,
                &mut dev,
            );
            assert_eq!(sys.engine.live_identifiers(), vec![want], "{aggressive}");
            // A bee's loop starts silent and is allowed to, like the minecart.
            assert_eq!(sys.stats.started_silently, 1, "stats: {:?}", sys.stats);
        }
    }

    // ---- M141d: the velocity input ---------------------------------------

    /// **The local player's velocity reaches the ramps**, and it is not the
    /// table's — the table has no row for the local player at all, so without
    /// `LocalPlayerView` every one of these answers zero.
    #[test]
    fn the_local_players_velocity_and_position_reach_the_ramp_world() {
        use crate::tickable::RampWorld;
        let t = rewo_world::entities::EntityTable::default();
        let w = EntityTableWorld {
            table: &t,
            local: Some(LocalPlayerView {
                id: 42,
                position: (1.5, 70.0, -2.5),
                // 3/4/12 — a triple whose length is 13 and whose horizontal
                // part is not its length, so a query reading the wrong
                // reduction cannot agree by accident.
                velocity: (3.0, 4.0, 12.0),
                fall_flying: true,
                underwater: false,
            }),
            game_time: 0,
        };
        assert_eq!(w.position(42), Some((1.5, 70.0, -2.5)));
        assert_eq!(w.speed_sqr(42), 169.0);
        assert_eq!(w.speed(42), 13.0);
        assert!((w.horizontal_speed(42) - 153.0_f64.sqrt()).abs() < 1e-12);
        assert!(w.fall_flying(42));

        // …and a DIFFERENT id still goes to the table, which knows nothing.
        assert_eq!(w.position(43), None);
        assert_eq!(w.speed_sqr(43), 0.0);
        assert!(!w.fall_flying(43));
    }

    /// Without the local view the local player reads as **removed**, which is
    /// what the elytra ramp would see: `position` returning `None` is
    /// `isRemoved()`, so it stops itself on its first tick rather than playing
    /// silently. Named because the two failures look identical from outside.
    #[test]
    fn with_no_local_view_the_local_player_reads_as_removed() {
        use crate::tickable::RampWorld;
        let t = rewo_world::entities::EntityTable::default();
        let w = EntityTableWorld {
            table: &t,
            local: None,
            game_time: 0,
        };
        assert_eq!(w.position(42), None);
        let mut r = crate::tickable::Ramp::Elytra(crate::tickable::ElytraRamp {
            player: 42,
            time: 0,
        });
        let mut i = SoundInstance::bare("minecraft:item.elytra.flying", SoundSource::Players);
        assert!(r.tick(&mut i, &w).stopped, "an absent local view is a removal");
    }

    /// **A remote entity's velocity reaches the ramps through the table**, and
    /// it decays — the whole point of M141d being a stored echo rather than a
    /// finite difference. Driven through the production `EntityTableWorld`
    /// rather than a fake, so the wiring is what is graded.
    #[test]
    fn a_remote_entitys_decaying_velocity_reaches_the_ramp_world() {
        use crate::tickable::RampWorld;
        let mut t = rewo_world::entities::EntityTable::default();
        t.add(
            9,
            rewo_world::entities::EntityState::new(0, 0, 0.0, 64.0, 0.0, 0.0, 0.0),
        );
        t.lerp_motion(9, [0.5, 0.0, 0.0], true, false);
        {
            let w = EntityTableWorld {
                table: &t,
                local: None,
                game_time: 0,
            };
            assert_eq!(w.horizontal_speed(9), 0.5);
        }
        t.tick_lerp();
        let w = EntityTableWorld {
            table: &t,
            local: None,
            game_time: 0,
        };
        assert!(
            (w.horizontal_speed(9) - 0.49).abs() < 1e-12,
            "0.98 of 0.5, got {}",
            w.horizontal_speed(9)
        );
    }

    // ---- M141c: the engine drives a ramp ---------------------------------

    /// **A ramp's volume reaches the device**, which is the whole point of
    /// M141c and the thing M141b could not assert: `tickable.rs` had no caller,
    /// so its `tick` signature was unvalidated. M93i is the precedent — a model
    /// shipped without its call site had a defect that only wiring exposed.
    #[test]
    fn a_ramped_sound_pushes_its_ramped_volume_at_the_channel() {
        let idx = index_of(&["minecraft:entity.minecart.riding"]);
        let mut eng = SoundEngine::new();
        let mut dev = RecordingDevice::default();
        let world = TestWorld {
            positions: HashMap::from([(1, (0.0, 64.0, 0.0))]),
            // 0.5 saturates the minecart's lerp factor, so the ramp's answer is
            // its ceiling of 0.35 — a number that is neither 0 nor 1, so a
            // dropped multiplication cannot agree with it (the audio plan's §5
            // rule about fixtures where two readings coincide).
            horizontal_speed: HashMap::from([(1, 0.5)]),
            ..Default::default()
        };
        let inst = SoundInstance {
            volume: 0.0,
            looping: true,
            can_start_silent: true,
            binding: Binding::Entity(1),
            ..SoundInstance::bare("minecraft:entity.minecart.riding", SoundSource::Neutral)
        };
        eng.play_ramped(
            inst,
            Some(crate::tickable::Ramp::Minecart(
                crate::tickable::MinecartRamp {
                    minecart: 1,
                    shadowed_pitch: 0.0,
                },
            )),
            &idx,
            &world,
            &mut dev,
        );
        dev.clear_calls();
        eng.tick(false, &idx, &world, &mut dev);

        assert!(
            dev.calls_to(0).contains(&ChannelCall::SetVolume(0.35)),
            "the ramp's volume must reach the device, got {:?}",
            dev.calls_to(0)
        );
        // …and the pitch is the untouched 1.0, because the minecart's ramp
        // writes a shadowed field. See `a_minecarts_pitch_ramp_is_dead_code`.
        assert!(dev.calls_to(0).contains(&ChannelCall::SetPitch(1.0)));
    }

    /// **A stopping ramp's last write never reaches the device.** Vanilla's
    /// loop pushes volume/pitch/position only in the `else` of
    /// `if (instance.isStopped())`, so the tick that stops a sound is silent on
    /// the wire except for the stop itself.
    #[test]
    fn a_stopping_ramp_pushes_a_stop_and_nothing_else() {
        let idx = index_of(&["minecraft:entity.minecart.riding"]);
        let mut eng = SoundEngine::new();
        let mut dev = RecordingDevice::default();
        let mut world = TestWorld {
            positions: HashMap::from([(1, (0.0, 64.0, 0.0))]),
            horizontal_speed: HashMap::from([(1, 0.5)]),
            ..Default::default()
        };
        let inst = SoundInstance {
            looping: true,
            binding: Binding::Entity(1),
            ..SoundInstance::bare("minecraft:entity.minecart.riding", SoundSource::Neutral)
        };
        eng.play_ramped(
            inst,
            Some(crate::tickable::Ramp::Minecart(
                crate::tickable::MinecartRamp {
                    minecart: 1,
                    shadowed_pitch: 0.0,
                },
            )),
            &idx,
            &world,
            &mut dev,
        );
        dev.clear_calls();

        world.positions.remove(&1); // `isRemoved()`
        eng.tick(false, &idx, &world, &mut dev);
        let calls = dev.calls_to(0);
        assert!(calls.contains(&ChannelCall::Stop));
        assert!(
            !calls.iter().any(|c| matches!(c, ChannelCall::SetVolume(_))),
            "a stopping tick must push no volume, got {calls:?}"
        );
    }

    /// **The bee's switch round-trips through `queuedTickableSounds`**, so the
    /// replacement first plays on the tick *after* the one that queued it —
    /// and there is no tick in which both loops hold a channel.
    ///
    /// This is the one ramp whose output is another sound, and it is the reason
    /// `queue_ticking_sound` takes a ramp: a replacement queued without one
    /// would play once and then never tick again, so an angry bee would be
    /// stuck on a loop that cannot switch back.
    #[test]
    fn an_angered_bee_hands_its_channel_to_the_aggressive_loop_next_tick() {
        let idx = index_of(&[
            "minecraft:entity.bee.loop",
            "minecraft:entity.bee.loop_aggressive",
        ]);
        let mut eng = SoundEngine::new();
        let mut dev = RecordingDevice::default();
        let world = TestWorld {
            positions: HashMap::from([(9, (0.0, 64.0, 0.0))]),
            horizontal_speed: HashMap::from([(9, 0.3)]),
            angry: vec![9],
            ..Default::default()
        };
        let bee = crate::tickable::bee_instance(
            crate::tickable::BeeLoop::Flying,
            9,
            (0.0, 64.0, 0.0),
        );
        eng.play_ramped(
            bee,
            Some(crate::tickable::Ramp::Bee(crate::tickable::BeeRamp {
                bee: 9,
                loop_kind: crate::tickable::BeeLoop::Flying,
                has_switched: false,
            })),
            &idx,
            &world,
            &mut dev,
        );
        assert_eq!(eng.live_count(), 1);

        // Tick 1: the flying loop stops and queues the aggressive one. It has
        // NOT started yet — the queue is drained at the top of a tick.
        eng.tick(false, &idx, &world, &mut dev);
        assert_eq!(eng.live_count(), 1, "still only the (now stopping) original");

        // Tick 2: the queue drains and the replacement plays.
        eng.tick(false, &idx, &world, &mut dev);
        assert!(
            eng.live_identifiers()
                .iter()
                .any(|n| *n == "minecraft:entity.bee.loop_aggressive"),
            "the aggressive loop must have started, live: {:?}",
            eng.live_identifiers()
        );

        // …and the replacement carries a RAMP, so it can switch back. A
        // replacement queued without one starts, holds its channel forever and
        // never notices the bee calming down.
        //
        // **The obvious witness for this cannot see it.** Calming the bee and
        // asserting `entity.bee.loop` is live passes either way, because the
        // ORIGINAL flying loop is still in `live` — `MIN_SOURCE_LIFETIME` is 20
        // ticks and only four have passed, so its name is there whether or not
        // anything switched back. The battery caught exactly that. What a
        // ramp-less replacement cannot do is **stop itself**, so the witness is
        // its channel's `Stop`.
        let aggressive_channel = dev
            .calls
            .iter()
            .map(|(c, _)| *c)
            .max()
            .expect("the replacement acquired a channel");
        dev.clear_calls();
        let calm = TestWorld {
            positions: HashMap::from([(9, (0.0, 64.0, 0.0))]),
            horizontal_speed: HashMap::from([(9, 0.3)]),
            ..Default::default() // not angry
        };
        eng.tick(false, &idx, &calm, &mut dev);
        assert!(
            dev.calls_to(aggressive_channel).contains(&ChannelCall::Stop),
            "a tickable replacement stops itself when the bee calms; got {:?}",
            dev.calls_to(aggressive_channel)
        );
        eng.tick(false, &idx, &calm, &mut dev); // the queue drains
        assert!(
            eng.live_identifiers()
                .iter()
                .any(|n| *n == "minecraft:entity.bee.loop"),
            "…and the flying loop comes back, live: {:?}",
            eng.live_identifiers()
        );
    }

    /// **A ticking sound whose entity goes silent is stopped mid-flight**, and
    /// vanilla does NOT `continue` — it stops the channel and then ticks the
    /// instance anyway.
    ///
    /// The battery found nothing covering this: `play`'s silence guard was
    /// witnessed and the *tick-time* one was not, so the whole
    /// `if (!instance.canPlaySound()) this.stop(instance);` line could be
    /// deleted unnoticed. It predates M141c — the same line was there before
    /// the loop was generalised.
    #[test]
    fn a_ticking_sound_stops_when_its_entity_falls_silent() {
        let idx = plain_index();
        let mut eng = SoundEngine::new();
        let mut dev = RecordingDevice::default();
        let mut world = TestWorld {
            positions: HashMap::from([(7, (0.0, 64.0, 0.0))]),
            ..Default::default()
        };
        let i = SoundInstance::entity_bound(
            "minecraft:block.stone.break",
            SoundSource::Blocks,
            1.0,
            1.0,
            0,
            7,
            0.0,
            64.0,
            0.0,
        );
        eng.play(i, &idx, &world, &mut dev);
        dev.clear_calls();

        world.silent.push(7);
        eng.tick(false, &idx, &world, &mut dev);
        let calls = dev.calls_to(0);
        assert!(calls.contains(&ChannelCall::Stop), "got {calls:?}");
        // …and the instance was still ticked: the position push happens too,
        // because vanilla's stop does not skip the rest of the body.
        assert!(
            calls
                .iter()
                .any(|c| matches!(c, ChannelCall::SetSelfPosition(..))),
            "vanilla does not `continue` after the silence stop; got {calls:?}"
        );
    }

    #[test]
    fn a_fixed_sound_is_never_repositioned() {
        let idx = plain_index();
        let mut eng = SoundEngine::new();
        let mut dev = RecordingDevice::default();
        eng.play(stone(1.0, 1.0), &idx, &EmptyWorld, &mut dev);
        dev.clear_calls();
        for _ in 0..5 {
            eng.tick(false, &idx, &EmptyWorld, &mut dev);
        }
        assert!(
            dev.calls.is_empty(),
            "a SimpleSoundInstance is not a TickableSoundInstance"
        );
    }

    #[test]
    fn a_removed_entity_clears_looping_so_the_sound_does_not_come_back() {
        // `AbstractTickableSoundInstance.stop()` sets `stopped = true` AND
        // `looping = false`. Without the second half a manually-looping
        // entity sound would be requeued forever after its entity died.
        let idx = plain_index();
        let mut eng = SoundEngine::new();
        let mut dev = RecordingDevice::default();
        let world = TestWorld::default();
        let mut i = SoundInstance::entity_bound(
            "minecraft:block.stone.break",
            SoundSource::Blocks,
            1.0,
            1.0,
            0,
            7,
            0.0,
            0.0,
            0.0,
        );
        i.looping = true;
        i.delay = 2;
        eng.play(i, &idx, &world, &mut dev);
        dev.finish(0);
        for _ in 0..(MIN_SOURCE_LIFETIME + 6) {
            eng.tick(false, &idx, &world, &mut dev);
        }
        assert_eq!(eng.live_count(), 0, "not requeued");
    }

    // ---- pause -------------------------------------------------------------

    #[test]
    fn a_paused_game_advances_no_clock_and_reclaims_only_music() {
        let idx = plain_index();
        let mut eng = SoundEngine::new();
        let mut dev = RecordingDevice::default();
        eng.play(stone(1.0, 1.0), &idx, &EmptyWorld, &mut dev);
        dev.finish(0);
        eng.tick(false, &idx, &EmptyWorld, &mut dev); // sets handle_stopped
        let before = eng.tick_count();
        for _ in 0..(MIN_SOURCE_LIFETIME * 3) {
            eng.tick(true, &idx, &EmptyWorld, &mut dev);
        }
        assert_eq!(eng.tick_count(), before, "tickCount does not advance");
        assert_eq!(eng.live_count(), 1, "a non-music sound keeps its channel");
    }

    #[test]
    fn pause_all_except_skips_the_named_categories_and_resume_unpauses_everything() {
        let idx = plain_index();
        let mut eng = SoundEngine::new();
        let mut dev = RecordingDevice::default();
        eng.play(stone(1.0, 1.0), &idx, &EmptyWorld, &mut dev); // channel 0, BLOCKS
        let mut ui_idx = SoundsIndex::new();
        ui_idx.handle_registration(
            "minecraft:ui.button.click",
            &SoundEventRegistration {
                sounds: vec![Sound::file("minecraft:ui/button/click")],
                replace: false,
                subtitle: None,
            },
            &SoundFileSet::All,
        );
        eng.play(
            SoundInstance::for_ui("minecraft:ui.button.click", 1.0),
            &ui_idx,
            &EmptyWorld,
            &mut dev,
        ); // channel 1, UI
        dev.clear_calls();

        eng.pause_all_except(&[SoundSource::Ui], &mut dev);
        assert_eq!(dev.calls_to(0), vec![ChannelCall::Pause]);
        assert!(dev.calls_to(1).is_empty());

        dev.clear_calls();
        eng.resume(&mut dev);
        // `resume()` unpauses every channel, not only the paused ones.
        assert_eq!(dev.calls_to(0), vec![ChannelCall::Unpause]);
        assert_eq!(dev.calls_to(1), vec![ChannelCall::Unpause]);
        let _ = idx;
    }

    // ---- stop --------------------------------------------------------------

    #[test]
    fn stop_asks_the_device_and_leaves_the_bookkeeping_alone() {
        let idx = plain_index();
        let mut eng = SoundEngine::new();
        let mut dev = RecordingDevice::default();
        let (id, _) = eng.play(stone(1.0, 1.0), &idx, &EmptyWorld, &mut dev);
        dev.clear_calls();
        eng.stop(id, &mut dev);
        assert_eq!(dev.calls_to(0), vec![ChannelCall::Stop]);
        assert!(eng.is_active(id), "still holding the channel until a tick reclaims it");
    }

    #[test]
    fn stop_with_neither_a_name_nor_a_category_stops_absolutely_everything() {
        let idx = plain_index();
        let mut eng = SoundEngine::new();
        let mut dev = RecordingDevice::default();
        eng.play(stone(1.0, 1.0), &idx, &EmptyWorld, &mut dev);
        eng.play(stone(1.0, 1.0), &idx, &EmptyWorld, &mut dev);
        assert_eq!(eng.live_count(), 2);
        eng.stop_matching(None, None, &mut dev);
        assert_eq!(eng.live_count(), 0);
        assert_eq!(dev.budget().used(Pool::Static), 0, "the channels went back");
    }

    #[test]
    fn stop_by_category_spares_the_other_categories() {
        let mut idx = plain_index();
        idx.handle_registration(
            "minecraft:ui.button.click",
            &SoundEventRegistration {
                sounds: vec![Sound::file("minecraft:ui/button/click")],
                replace: false,
                subtitle: None,
            },
            &SoundFileSet::All,
        );
        let mut eng = SoundEngine::new();
        let mut dev = RecordingDevice::default();
        eng.play(stone(1.0, 1.0), &idx, &EmptyWorld, &mut dev); // 0 BLOCKS
        eng.play(
            SoundInstance::for_ui("minecraft:ui.button.click", 1.0),
            &idx,
            &EmptyWorld,
            &mut dev,
        ); // 1 UI
        dev.clear_calls();
        eng.stop_matching(None, Some(SoundSource::Blocks), &mut dev);
        assert_eq!(dev.calls_to(0), vec![ChannelCall::Stop]);
        assert!(dev.calls_to(1).is_empty());
    }

    #[test]
    fn stop_by_name_and_category_needs_both_to_match() {
        let mut idx = plain_index();
        idx.handle_registration(
            "minecraft:block.stone.step",
            &SoundEventRegistration {
                sounds: vec![Sound::file("minecraft:block/stone/step1")],
                replace: false,
                subtitle: None,
            },
            &SoundFileSet::All,
        );
        let mut eng = SoundEngine::new();
        let mut dev = RecordingDevice::default();
        eng.play(stone(1.0, 1.0), &idx, &EmptyWorld, &mut dev); // 0 break/BLOCKS
        eng.play(
            SoundInstance::simple(
                "minecraft:block.stone.step",
                SoundSource::Blocks,
                1.0,
                1.0,
                0,
                0.0,
                0.0,
                0.0,
            ),
            &idx,
            &EmptyWorld,
            &mut dev,
        ); // 1 step/BLOCKS
        dev.clear_calls();
        eng.stop_matching(
            Some("minecraft:block.stone.break"),
            Some(SoundSource::Blocks),
            &mut dev,
        );
        assert_eq!(dev.calls_to(0), vec![ChannelCall::Stop]);
        assert!(dev.calls_to(1).is_empty());
    }

    #[test]
    fn stop_all_resets_every_runtime_category_gain_to_full() {
        // `this.gainBySource.clear()` on a map with `defaultReturnValue(1.0F)`.
        let mut eng = SoundEngine::new();
        let mut dev = RecordingDevice::default();
        eng.update_category_volume(SoundSource::Music, 0.1);
        assert_eq!(eng.category_gain(SoundSource::Music), 0.1);
        eng.stop_all(&mut dev);
        assert_eq!(eng.category_gain(SoundSource::Music), 1.0);
    }

    #[test]
    fn update_category_volume_clamps_on_the_way_in() {
        let mut eng = SoundEngine::new();
        eng.update_category_volume(SoundSource::Music, 5.0);
        assert_eq!(eng.category_gain(SoundSource::Music), 1.0);
        eng.update_category_volume(SoundSource::Music, -1.0);
        assert_eq!(eng.category_gain(SoundSource::Music), 0.0);
    }

    // ---- the wire adapter --------------------------------------------------

    fn registry() -> rewo_data::sound_events::SoundEvents {
        rewo_data::sound_events::SoundEvents::from_pairs(vec![
            (0, "minecraft:entity.allay.ambient_with_item".to_string()),
            (7, "minecraft:ambient.cave".to_string()),
        ])
    }

    #[test]
    fn a_positioned_packet_becomes_a_simple_instance() {
        let ev = SoundEvent::At(PositionedSound {
            sound: SoundRef::Registry(7),
            source: SoundSource::Ambient,
            x: 1.5,
            y: 2.5,
            z: 3.5,
            volume: 0.5,
            pitch: 1.25,
            seed: 99,
        });
        let i = instance_from_event(&ev, &registry(), &EmptyWorld).unwrap();
        assert_eq!(i.identifier, "minecraft:ambient.cave");
        assert_eq!((i.x, i.y, i.z), (1.5, 2.5, 3.5));
        assert_eq!(i.volume, 0.5);
        assert_eq!(i.pitch, 1.25);
        assert_eq!(i.seed, Some(99));
        assert_eq!(i.binding, Binding::Fixed);
        // `SimpleSoundInstance`'s defaults survive.
        assert_eq!(i.attenuation, Attenuation::Linear);
        assert!(!i.relative);
        assert!(!i.looping);
    }

    #[test]
    fn an_entity_packet_for_an_untracked_entity_is_dropped_not_relocated() {
        // `handleSoundEntityEvent`'s `if (entity != null)`. Playing it at the
        // player would be a plausible fallback and is not what vanilla does.
        let ev = SoundEvent::OnEntity(EntitySound {
            sound: SoundRef::Registry(7),
            source: SoundSource::Ambient,
            entity_id: 42,
            volume: 1.0,
            pitch: 1.0,
            seed: 0,
        });
        assert_eq!(
            instance_from_event(&ev, &registry(), &EmptyWorld),
            Err(NoInstance::UnknownEntity)
        );
        let world = TestWorld {
            positions: HashMap::from([(42, (8.0, 9.0, 10.0))]),
            silent: vec![],
            ..Default::default()
        };
        let i = instance_from_event(&ev, &registry(), &world).unwrap();
        assert_eq!(i.binding, Binding::Entity(42));
        assert_eq!((i.x, i.y, i.z), (8.0, 9.0, 10.0));
    }

    #[test]
    fn an_unknown_registry_id_produces_no_instance() {
        let ev = SoundEvent::At(PositionedSound {
            sound: SoundRef::Registry(9999),
            source: SoundSource::Blocks,
            x: 0.0,
            y: 0.0,
            z: 0.0,
            volume: 1.0,
            pitch: 1.0,
            seed: 0,
        });
        assert_eq!(
            instance_from_event(&ev, &registry(), &EmptyWorld),
            Err(NoInstance::UnknownSoundId)
        );
    }

    #[test]
    fn an_inline_sound_needs_no_registry_at_all() {
        let ev = SoundEvent::At(PositionedSound {
            sound: SoundRef::Inline {
                name: "mypack:custom.jingle".into(),
                fixed_range: None,
            },
            source: SoundSource::Records,
            x: 0.0,
            y: 0.0,
            z: 0.0,
            volume: 1.0,
            pitch: 1.0,
            seed: 0,
        });
        let i = instance_from_event(&ev, &registry(), &EmptyWorld).unwrap();
        assert_eq!(i.identifier, "mypack:custom.jingle");
    }

    #[test]
    fn a_client_local_sound_carries_no_seed() {
        let ev = SoundEvent::Local(LocalSound {
            name: "minecraft:entity.arrow.hit_player".into(),
            source: SoundSource::Players,
            x: 1.0,
            y: 2.0,
            z: 3.0,
            volume: 0.18,
            pitch: 0.45,
        });
        let i = instance_from_event(&ev, &registry(), &EmptyWorld).unwrap();
        assert_eq!(i.seed, None);
        assert_eq!(i.volume, 0.18);
        assert_eq!(i.identifier, "minecraft:entity.arrow.hit_player");
    }

    // ---- the system: order, stats, and the silent device ------------------

    fn positioned(seed: i64) -> SoundEvent {
        SoundEvent::At(PositionedSound {
            sound: SoundRef::Inline {
                name: "minecraft:block.stone.break".into(),
                fixed_range: None,
            },
            source: SoundSource::Blocks,
            x: 0.0,
            y: 0.0,
            z: 0.0,
            volume: 1.0,
            pitch: 1.0,
            seed,
        })
    }

    #[test]
    fn the_queue_order_decides_whether_a_stop_cancels_the_sound_before_it() {
        // The whole reason M63 put four kinds in one queue. A stop that
        // overtook its sound would leave that sound playing forever.
        let mut sys = SoundSystem::new(plain_index());
        let mut dev = RecordingDevice::default();
        let reg = registry();
        let stop = SoundEvent::Stop(StopSound {
            source: Some(SoundSource::Blocks),
            name: None,
        });

        sys.accept(&[positioned(1), stop.clone()], &reg, &EmptyWorld, &mut dev);
        assert_eq!(dev.calls_to(0).last(), Some(&ChannelCall::Stop));

        // Reversed: the stop finds nothing and the sound survives it.
        let mut sys = SoundSystem::new(plain_index());
        let mut dev = RecordingDevice::default();
        sys.accept(&[stop, positioned(1)], &reg, &EmptyWorld, &mut dev);
        assert_eq!(dev.calls_to(0).last(), Some(&ChannelCall::Play));
    }

    #[test]
    fn the_stats_account_for_every_event_in_the_batch() {
        let mut sys = SoundSystem::new(plain_index());
        let mut dev = RecordingDevice::default();
        let batch = vec![
            positioned(1),
            // An entity sound for an entity nobody is tracking → no instance.
            SoundEvent::OnEntity(EntitySound {
                sound: SoundRef::Registry(7),
                source: SoundSource::Ambient,
                entity_id: 999,
                volume: 1.0,
                pitch: 1.0,
                seed: 0,
            }),
            // A registered-nowhere event → the engine refuses it.
            SoundEvent::At(PositionedSound {
                sound: SoundRef::Inline {
                    name: "minecraft:nope".into(),
                    fixed_range: None,
                },
                source: SoundSource::Blocks,
                x: 0.0,
                y: 0.0,
                z: 0.0,
                volume: 1.0,
                pitch: 1.0,
                seed: 0,
            }),
            SoundEvent::Stop(StopSound::default()),
        ];
        sys.accept(&batch, &registry(), &EmptyWorld, &mut dev);
        assert_eq!(sys.stats.started, 1);
        assert_eq!(sys.stats.no_instance, 1);
        assert_eq!(sys.stats.not_started, 1);
        assert_eq!(sys.stats.stops, 1);
        assert_eq!(sys.stats.total(), batch.len() as u32);
    }

    #[test]
    fn an_empty_index_is_silent_rather_than_broken() {
        // A client with no resource pack unpacked: every event resolves to
        // UnknownEvent, nothing panics, and the counters say so.
        let mut sys = SoundSystem::default();
        let mut dev = RecordingDevice::default();
        sys.accept(&[positioned(1)], &registry(), &EmptyWorld, &mut dev);
        assert_eq!(sys.stats.not_started, 1);
        assert!(dev.calls.is_empty());
    }

    #[test]
    fn the_silent_device_recycles_its_channels_instead_of_wedging() {
        // `stopped()` answering `true` is what keeps the pool from filling
        // permanently. With `false` the 26th sound and every one after it
        // would be dropped for the rest of the session.
        let idx = plain_index();
        let mut sys = SoundSystem::new(idx);
        let mut dev = SilentDevice::default();
        let reg = registry();
        for _ in 0..200 {
            sys.accept(&[positioned(1)], &reg, &EmptyWorld, &mut dev);
            for _ in 0..MIN_SOURCE_LIFETIME {
                sys.tick(false, &EmptyWorld, &mut dev);
            }
        }
        assert_eq!(sys.stats.started, 200);
        assert_eq!(dev.refusals(), 0);
        assert!(dev.calls_made > 0);
        assert_eq!(sys.engine.live_count(), 0, "everything was reclaimed");
    }

    #[test]
    fn the_silent_device_still_enforces_the_budget_within_the_grace_period() {
        // The other half: it does not become an unlimited sink. 25 static
        // channels, and the 26th sound inside one grace period is dropped.
        let idx = plain_index();
        let mut sys = SoundSystem::new(idx);
        let mut dev = SilentDevice::default();
        let reg = registry();
        let batch: Vec<SoundEvent> = (0..26).map(positioned).collect();
        sys.accept(&batch, &reg, &EmptyWorld, &mut dev);
        assert_eq!(sys.stats.started, 25);
        assert_eq!(sys.stats.not_started, 1);
        assert_eq!(dev.refusals(), 1);
    }

    // ── the music tick reaches the engine (M146) ──────────────────────────

    /// A `sounds.json` index carrying one streamed music event.
    fn music_index() -> SoundsIndex {
        let mut idx = SoundsIndex::new();
        let mut sound = Sound::file("minecraft:music/game/calm1");
        sound.stream = true;
        idx.handle_registration(
            "minecraft:music.game",
            &SoundEventRegistration {
                sounds: vec![sound],
                replace: false,
                subtitle: None,
            },
            &SoundFileSet::All,
        );
        idx
    }

    /// **A music tick starts a track through the ordinary play path.**
    ///
    /// Drives `SoundSystem::accept` with `SoundEvent::Music` for as many ticks
    /// as the starting delay, and asserts the engine ends up holding a channel
    /// — so the manager, the engine, the index and the channel budget are all
    /// on the same path a real session takes. Before M146 nothing called the
    /// manager at all and this stayed at zero for ever.
    #[test]
    fn a_music_tick_eventually_starts_a_track_through_the_engine() {
        let mut sys = SoundSystem::new(music_index());
        let mut dev = RecordingDevice::default();
        let reg = registry();
        let ev = SoundEvent::Music {
            situational: Some(rewo_world::music::musics::game()),
        };
        for _ in 0..crate::music::STARTING_DELAY {
            sys.accept(&[ev.clone()], &reg, &EmptyWorld, &mut dev);
        }
        assert_eq!(sys.stats.started, 1, "one track started");
        assert_eq!(sys.engine.live_count(), 1, "and it holds a channel");
        // A streamed attach, because music is `stream: true` — the channel came
        // from the streaming pool rather than the static one.
        assert!(dev
            .calls_to(0)
            .iter()
            .any(|c| matches!(c, ChannelCall::AttachBufferStream(_, _))));
    }

    /// **Nothing on offer starts nothing**, however long the client runs.
    #[test]
    fn a_music_tick_with_no_situational_track_starts_nothing() {
        let mut sys = SoundSystem::new(music_index());
        let mut dev = RecordingDevice::default();
        let reg = registry();
        for _ in 0..1_000 {
            sys.accept(
                &[SoundEvent::Music { situational: None }],
                &reg,
                &EmptyWorld,
                &mut dev,
            );
        }
        assert_eq!(sys.stats.started, 0);
        assert_eq!(sys.engine.live_count(), 0);
    }

    /// **A refused track must not leave the manager believing it is playing.**
    ///
    /// The channel budget can refuse music like anything else — the streaming
    /// pool is small (`pool_sizes` clamps it to 2..8) and music competes with
    /// the Nether beds and the underwater loop for it. If the manager kept
    /// `currentMusic` after a refusal it would wait on `isActive` for a sound
    /// that never started, and since `nextSongDelay` is parked at `MAX` by
    /// `startPlaying`, **the next song would never come for the rest of the
    /// session**.
    ///
    /// Found by a mutation surviving: nothing here drove the refusal path, so
    /// deleting the `stop_playing` that recovers from it changed nothing.
    #[test]
    fn a_refused_music_track_does_not_wedge_the_manager() {
        let mut idx = music_index();
        // Two more streamed events, to fill a deliberately tiny streaming pool
        // before the music asks for a channel.
        for (i, name) in ["minecraft:ambient.a", "minecraft:ambient.b"].iter().enumerate() {
            let mut snd = Sound::file(format!("minecraft:filler/{i}"));
            snd.stream = true;
            idx.handle_registration(
                name,
                &SoundEventRegistration {
                    sounds: vec![snd],
                    replace: false,
                    subtitle: None,
                },
                &SoundFileSet::All,
            );
        }
        let mut sys = SoundSystem::new(idx);
        // `pool_sizes(4)` is 2 streaming and 8 static, so two streamed sounds
        // exhaust the streaming pool exactly.
        let mut dev = RecordingDevice::with_channel_count(4);
        let reg = registry();
        assert_eq!(dev.budget().limit(Pool::Streaming), 2);

        for name in ["minecraft:ambient.a", "minecraft:ambient.b"] {
            sys.accept(
                &[SoundEvent::At(PositionedSound {
                    sound: SoundRef::Inline {
                        name: name.into(),
                        fixed_range: None,
                    },
                    source: SoundSource::Ambient,
                    x: 0.0,
                    y: 0.0,
                    z: 0.0,
                    volume: 1.0,
                    pitch: 1.0,
                    seed: 0,
                })],
                &reg,
                &EmptyWorld,
                &mut dev,
            );
        }
        assert_eq!(sys.stats.started, 2, "the pool is full");

        let ev = SoundEvent::Music {
            situational: Some(rewo_world::music::musics::game()),
        };
        for _ in 0..crate::music::STARTING_DELAY {
            sys.accept(&[ev.clone()], &reg, &EmptyWorld, &mut dev);
        }
        assert_eq!(sys.stats.not_started, 1, "the budget refused the track");
        assert_eq!(
            sys.music.current(),
            None,
            "the manager must not hold a track that never started"
        );
        // And it is not parked: `startPlaying` set MAX, and the recovery has to
        // have replaced it with something reachable.
        assert!(
            sys.music.next_song_delay() < i32::MAX,
            "delay {} is still parked; the next song would never come",
            sys.music.next_song_delay()
        );
    }

    /// A replacing track stops the one playing, through the engine.
    #[test]
    fn a_replacing_track_stops_the_current_one_through_the_engine() {
        let mut idx = music_index();
        let mut menu = Sound::file("minecraft:music/menu/menu1");
        menu.stream = true;
        idx.handle_registration(
            "minecraft:music.menu",
            &SoundEventRegistration {
                sounds: vec![menu],
                replace: false,
                subtitle: None,
            },
            &SoundFileSet::All,
        );
        let mut sys = SoundSystem::new(idx);
        let mut dev = RecordingDevice::default();
        let reg = registry();

        let game = SoundEvent::Music {
            situational: Some(rewo_world::music::musics::game()),
        };
        for _ in 0..crate::music::STARTING_DELAY {
            sys.accept(&[game.clone()], &reg, &EmptyWorld, &mut dev);
        }
        assert_eq!(sys.stats.started, 1);
        let stops_before = sys.stats.stops;

        // The menu replaces, and is a different track.
        sys.accept(
            &[SoundEvent::Music {
                situational: Some(rewo_world::music::musics::menu()),
            }],
            &reg,
            &EmptyWorld,
            &mut dev,
        );
        assert_eq!(sys.stats.stops, stops_before + 1, "the game track was stopped");
    }

    #[test]
    fn live_sounds_drives_the_whole_chain_from_a_decoded_event() {
        // The end-to-end shape the live client uses: a decoded packet in, an
        // engine holding a channel out — through the production registry
        // lookup, instance construction, resolution and budget. If any link
        // were missing this would count a `no_instance` or a `not_started`.
        use rewo_world::entities::{EntityState, EntityTable};
        let mut live = LiveSounds::new(plain_index(), registry());
        let mut entities = EntityTable::default();
        entities.add(11, EntityState::new(0, 0, 5.0, 64.0, -5.0, 0.0, 0.0));

        live.drive(&[positioned(3)], &entities, None, 0);
        assert_eq!(live.stats().started, 1);
        assert_eq!(live.system.engine.live_count(), 1);

        // …and an entity-bound one resolves its position through the table.
        let ev = SoundEvent::OnEntity(EntitySound {
            sound: SoundRef::Inline {
                name: "minecraft:block.stone.break".into(),
                fixed_range: None,
            },
            source: SoundSource::Neutral,
            entity_id: 11,
            volume: 1.0,
            pitch: 1.0,
            seed: 0,
        });
        live.drive(&[ev.clone()], &entities, None, 0);
        assert_eq!(live.stats().started, 2);

        // An entity the table does not have is dropped, not relocated.
        entities.remove(11);
        live.drive(&[ev], &entities, None, 0);
        assert_eq!(live.stats().no_instance, 1);
        assert_eq!(live.stats().started, 2);
    }

    // ── the backend seam (M143) ───────────────────────────────────────────

    /// What a [`ChannelSink`] was asked to do, readable after it has been boxed.
    #[derive(Default)]
    struct SinkLog {
        submits: Vec<(ChannelId, ChannelCall)>,
        releases: Vec<ChannelId>,
        listeners: u64,
        ticks: u32,
    }

    struct FakeSink {
        log: std::rc::Rc<std::cell::RefCell<SinkLog>>,
        /// What this backend claims about every channel.
        verdict: Option<bool>,
    }

    impl ChannelSink for FakeSink {
        fn submit(&mut self, channel: ChannelId, call: &ChannelCall) {
            self.log.borrow_mut().submits.push((channel, call.clone()));
        }
        fn release(&mut self, channel: ChannelId) {
            self.log.borrow_mut().releases.push(channel);
        }
        fn set_listener(&mut self, _t: ListenerTransform) {
            self.log.borrow_mut().listeners += 1;
        }
        fn tick(&mut self) {
            self.log.borrow_mut().ticks += 1;
        }
        fn stopped(&self, _channel: ChannelId) -> Option<bool> {
            self.verdict
        }
    }

    fn live_with_sink(verdict: Option<bool>) -> (LiveSounds, std::rc::Rc<std::cell::RefCell<SinkLog>>) {
        let log = std::rc::Rc::new(std::cell::RefCell::new(SinkLog::default()));
        let mut live = LiveSounds::new(plain_index(), registry());
        live.attach_sink(Box::new(FakeSink {
            log: std::rc::Rc::clone(&log),
            verdict,
        }));
        (live, log)
    }

    /// **The backend's `stopped()` decides the release, and the bookkeeping
    /// device's unconditional `true` must not override it.**
    ///
    /// This is the one place a plausible backend is silently, audibly wrong.
    /// `schedule_tick` turns `stopped()` straight into `release(channel)` on the
    /// very next tick — `MIN_SOURCE_LIFETIME` gates the *instance* reclaim, not
    /// the release — and a real `release` destroys the source. A `Tee` that
    /// consulted `book` (or that just inherited `SilentDevice`'s answer) would
    /// tear down every voice about 50 ms after starting it: every sound in the
    /// game becomes a click, with a green suite and no error anywhere.
    #[test]
    fn the_backends_stopped_decides_the_release_not_the_bookkeeping_devices() {
        use rewo_world::entities::EntityTable;
        let entities = EntityTable::default();

        // A sound that is still playing: the channel stays with it.
        let (mut live, log) = live_with_sink(Some(false));
        live.drive(&[positioned(3)], &entities, None, 0);
        assert_eq!(live.stats().started, 1, "the fixture must actually play");
        assert!(
            log.borrow().releases.is_empty(),
            "a sounding voice was torn down: {:?}",
            log.borrow().releases
        );

        // The same drive with the backend reporting the sound finished.
        let (mut live, log) = live_with_sink(Some(true));
        live.drive(&[positioned(3)], &entities, None, 0);
        assert_eq!(live.stats().started, 1);
        assert_eq!(log.borrow().releases.len(), 1, "a finished voice is released");

        // And the claim is about the tee rather than about the fixture: the
        // bookkeeping device really does answer `true` for that same channel,
        // so a tee reading it would have released in the first case too.
        let ch = log.borrow().submits[0].0;
        assert!(
            AudioDevice::stopped(&SilentDevice::default(), ch),
            "SilentDevice::stopped is the unconditional true this must not inherit"
        );
    }

    /// A backend with no opinion degrades to the silent path, not to a pool that
    /// never drains.
    #[test]
    fn a_backend_with_no_opinion_behaves_like_the_silent_path() {
        use rewo_world::entities::EntityTable;
        let (mut live, log) = live_with_sink(None);
        live.drive(&[positioned(3)], &EntityTable::default(), None, 0);
        assert_eq!(log.borrow().releases.len(), 1);
    }

    /// Every engine call reaches both halves, in the order `play` emits them.
    #[test]
    fn the_eight_calls_reach_the_backend_as_well_as_the_bookkeeping_device() {
        use rewo_world::entities::EntityTable;
        let (mut live, log) = live_with_sink(Some(false));
        live.drive(&[positioned(3)], &EntityTable::default(), None, 0);

        let log = log.borrow();
        assert_eq!(log.submits.len(), 8, "SoundEngine.java:417-434 is eight calls");
        // The order is the contract, not the set: a backend has to see the
        // pitch and the loop flag BEFORE the attach, or it cannot know how long
        // the sound will last (which is what `stopped()` needs).
        assert!(matches!(log.submits[0].1, ChannelCall::SetPitch(_)));
        assert!(matches!(log.submits[1].1, ChannelCall::SetVolume(_)));
        assert!(matches!(log.submits[3].1, ChannelCall::SetLooping(_)));
        assert!(matches!(
            log.submits[6].1,
            ChannelCall::AttachStaticBuffer(_) | ChannelCall::AttachBufferStream(_, _)
        ));
        assert!(matches!(log.submits[7].1, ChannelCall::Play), "play is last");
        // …and the bookkeeping device counted the same calls, so attaching a
        // backend neither steals them nor doubles them.
        assert_eq!(live.device.calls_made, 8);
    }

    /// **r45 keeps reading the same place with a backend attached.**
    ///
    /// The listener counter lives on the bookkeeping device precisely so a
    /// witness reads what the device was *handed* rather than recomputing it,
    /// and leaving `device` concrete is what preserves that. A backend gets the
    /// transform too, on the same call.
    #[test]
    fn the_listener_reaches_both_halves() {
        let (mut live, log) = live_with_sink(Some(false));
        assert_eq!(live.device.listener_pushes, 0);
        live.update_listener([1.0, 2.0, 3.0], 90.0, 0.0);
        assert_eq!(live.device.listener_pushes, 1, "r45's counter still moves");
        assert_eq!(
            live.device.last_listener.map(|t| t.position),
            Some([1.0, 2.0, 3.0])
        );
        assert_eq!(log.borrow().listeners, 1, "and the backend heard it too");
    }

    /// The backend's clock runs with the engine's, once per drive.
    #[test]
    fn the_backend_is_ticked_once_per_drive() {
        use rewo_world::entities::EntityTable;
        let (mut live, log) = live_with_sink(Some(false));
        let entities = EntityTable::default();
        for _ in 0..5 {
            live.drive(&[], &entities, None, 0);
        }
        assert_eq!(log.borrow().ticks, 5, "an empty drive is still a tick");
    }

    /// A client with no backend is untouched — which is every gate and every
    /// default build.
    #[test]
    fn without_a_backend_nothing_changes() {
        use rewo_world::entities::EntityTable;
        let mut live = LiveSounds::new(plain_index(), registry());
        assert!(!live.has_sink());
        assert_eq!(live.sink_diagnostics(), SinkDiagnostics::default());
        live.drive(&[positioned(3)], &EntityTable::default(), None, 0);
        assert_eq!(live.stats().started, 1);
        assert_eq!(live.device.calls_made, 8);
    }

    #[test]
    fn the_entity_table_world_reads_the_current_position_not_the_synced_target() {
        use crate::tickable::RampWorld;
        use rewo_world::entities::{EntityState, EntityTable};
        let mut t = EntityTable::default();
        t.add(5, EntityState::new(0, 0, 0.0, 64.0, 0.0, 0.0, 0.0));
        // Give it somewhere to head for; `x/y/z` moves at once, `render_pos`
        // interpolates towards it over the next three ticks.
        if let Some(e) = t.get_mut(5) {
            e.set_target(30.0, 64.0, 0.0);
        }
        let w = EntityTableWorld { table: &t, local: None, game_time: 0 };
        let (x, _, _) = w.position(5).unwrap();
        assert_ne!(x, 30.0, "the target is not where the entity is yet");
        assert_eq!(w.position(6), None, "an untracked entity is gone");
        // The gap this cannot close: nothing tells Rewo an entity is silent.
        assert!(!w.entity_silent(5));
    }

    #[test]
    fn a_stop_event_is_not_an_instance() {
        let ev = SoundEvent::Stop(StopSound {
            source: Some(SoundSource::Records),
            name: None,
        });
        assert_eq!(
            instance_from_event(&ev, &registry(), &EmptyWorld),
            Err(NoInstance::IsStop)
        );
        let SoundEvent::Stop(s) = &ev else { unreachable!() };
        assert_eq!(stop_from_event(s), (None, Some(SoundSource::Records)));
    }
    /// **A tickable ambient instance must be born at volume 1.0.**
    ///
    /// `SoundEngine.play` returns `NOT_STARTED` for a zero-volume instance
    /// unless `canStartSilent()`, and neither underwater class overrides it —
    /// only Bee, Minecart and RidingEntity do. So constructing the loop at 0.0
    /// "because it fades in from silence" makes it never play at all, with a
    /// debug log as the only trace. The first tick immediately rewrites the
    /// volume to `fade/40`.
    ///
    /// Both are `relative` — head-locked at the origin — while the enter/exit
    /// one-shots created in the same breath are positioned world sounds.
    /// Neither sets `Attenuation::None`: `relative` does **not** imply it, and
    /// these three classes are exactly what falsifies that pairing.
    #[test]
    fn the_underwater_instances_are_born_audible_and_head_locked() {
        let world = EntityTableWorld {
            table: &rewo_world::entities::EntityTable::default(),
            local: Some(LocalPlayerView {
                id: 1,
                position: (0.0, 64.0, 0.0),
                velocity: (0.0, 0.0, 0.0),
                fall_flying: false,
                underwater: true,
            }),
            game_time: 0,
        };

        let (loop_inst, loop_ramp) = instance_and_ramp(
            crate::sounds::TickableSound::UnderwaterLoop { player: 1 },
            &world,
        )
        .expect("the loop needs no world lookup");
        assert_eq!(loop_inst.volume, 1.0, "born audible, or `play` refuses it");
        assert!(loop_inst.looping, "it is the bed");
        assert!(loop_inst.relative, "head-locked, not a world sound");
        assert_eq!(loop_inst.delay, 0);
        assert!(
            !loop_inst.can_start_silent,
            "it does NOT override canStartSilent — which is why the 1.0 matters"
        );
        assert_eq!(
            loop_inst.attenuation,
            Attenuation::Linear,
            "`relative` does not imply NONE — the inherited LINEAR survives"
        );
        assert_eq!(loop_inst.source, SoundSource::Ambient);
        assert!(matches!(
            loop_ramp,
            crate::tickable::Ramp::UnderwaterLoop(crate::tickable::UnderwaterRamp {
                player: 1,
                fade: 0
            })
        ));

        let (sub, sub_ramp) = instance_and_ramp(
            crate::sounds::TickableSound::UnderwaterSub {
                player: 1,
                sound: crate::ambient_handlers::UNDERWATER_ADDITIONS_RARE,
            },
            &world,
        )
        .expect("nor does the sub-sound");
        assert_eq!(sub.volume, 1.0);
        assert!(
            !sub.looping,
            "the SubSound is a ONE-SHOT — the loop beside it is the looping one"
        );
        assert!(sub.relative);
        assert_eq!(sub.identifier, crate::ambient_handlers::UNDERWATER_ADDITIONS_RARE);
        assert!(matches!(
            sub_ramp,
            crate::tickable::Ramp::UnderwaterSub { player: 1 }
        ));
    }
    /// **A live loop is REUSED, not replaced.** Crossing back inside the ~41
    /// ticks it takes to die finds the instance still here, so `fade_in()`
    /// just reverses the ramp from wherever it got to — no second `play`, no
    /// channel re-attach, no restart of the sample from offset 0.
    ///
    /// And **every** loop fades out on a transition, including the one about
    /// to fade back in. That reads like a bug and is the only place a runaway
    /// `fade` is capped, since the ramp's tick never bounds it upward.
    #[test]
    fn a_biome_loop_transition_fades_all_and_reuses_the_live_one() {
        let idx = index_of(&["a", "b"]);
        let mut sys = SoundSystem::new(idx);
        let mut dev = RecordingDevice::default();
        let table = rewo_world::entities::EntityTable::default();
        let world = EntityTableWorld {
            table: &table,
            local: None,
            game_time: 0,
        };

        // Entering a biome whose loop is "a": nothing live, so one is created.
        sys.accept(
            &[SoundEvent::BiomeLoopTransition {
                current: Some("a".into()),
            }],
            &registry(),
            &world,
            &mut dev,
        );
        assert_eq!(sys.engine.live_identifiers(), vec!["a"]);
        assert_eq!(sys.stats.biome_transitions, 1);
        let started = sys.stats.started + sys.stats.started_silently;
        assert_eq!(started, 1, "the missing loop was played");

        // Let it run up, so a restart would be visible as a reset.
        for _ in 0..30 {
            sys.tick(false, &world, &mut dev);
        }
        let fade_before = sys.engine.biome_fade("a").expect("live");
        assert!(fade_before > 20, "it ramped up: {fade_before}");

        // Cross to a biome with no loop: it fades OUT, and stays live while
        // it does — which is what makes the reuse below reachable at all.
        sys.accept(
            &[SoundEvent::BiomeLoopTransition { current: None }],
            &registry(),
            &world,
            &mut dev,
        );
        sys.tick(false, &world, &mut dev);
        let fading = sys.engine.biome_fade("a").expect("still live, fading");
        assert!(fading < fade_before, "now heading down: {fading}");

        // Cross straight back: the SAME instance resumes, and nothing new is
        // played.
        let before = sys.stats.started + sys.stats.started_silently;
        sys.accept(
            &[SoundEvent::BiomeLoopTransition {
                current: Some("a".into()),
            }],
            &registry(),
            &world,
            &mut dev,
        );
        assert_eq!(
            sys.stats.started + sys.stats.started_silently,
            before,
            "the live instance was REUSED — a replacement would play again"
        );
        assert_eq!(sys.engine.live_identifiers(), vec!["a"], "still one voice");
        sys.tick(false, &world, &mut dev);
        assert!(
            sys.engine.biome_fade("a").expect("live") > fading,
            "and it resumed from where it was rather than from silence"
        );
    }

    /// A transition to a DIFFERENT loop fades the old one and plays the new
    /// one — two live voices for the length of the crossfade, which is what
    /// makes it a crossfade rather than a cut.
    #[test]
    fn a_new_loop_crossfades_against_the_old() {
        let idx = index_of(&["a", "b"]);
        let mut sys = SoundSystem::new(idx);
        let mut dev = RecordingDevice::default();
        let table = rewo_world::entities::EntityTable::default();
        let world = EntityTableWorld {
            table: &table,
            local: None,
            game_time: 0,
        };
        for id in ["a", "b"] {
            sys.accept(
                &[SoundEvent::BiomeLoopTransition {
                    current: Some(id.into()),
                }],
                &registry(),
                &world,
                &mut dev,
            );
            sys.tick(false, &world, &mut dev);
        }
        let mut live = sys.engine.live_identifiers();
        live.sort_unstable();
        assert_eq!(live, vec!["a", "b"], "both alive during the crossfade");
        // "a" is on its way out, "b" on its way in.
        assert!(sys.engine.biome_fade("a").unwrap() < sys.engine.biome_fade("b").unwrap());
    }

    /// The outgoing loop **dies on its own**: nothing stops it, and its own
    /// `-1` per tick carries it past zero into the ramp's stop condition.
    #[test]
    fn an_abandoned_loop_retires_itself() {
        let idx = index_of(&["a"]);
        let mut sys = SoundSystem::new(idx);
        let mut dev = RecordingDevice::default();
        let table = rewo_world::entities::EntityTable::default();
        let world = EntityTableWorld {
            table: &table,
            local: None,
            game_time: 0,
        };
        sys.accept(
            &[SoundEvent::BiomeLoopTransition {
                current: Some("a".into()),
            }],
            &registry(),
            &world,
            &mut dev,
        );
        for _ in 0..10 {
            sys.tick(false, &world, &mut dev);
        }
        sys.accept(
            &[SoundEvent::BiomeLoopTransition { current: None }],
            &registry(),
            &world,
            &mut dev,
        );
        let stops_before = dev
            .calls
            .iter()
            .filter(|(_, c)| matches!(c, ChannelCall::Stop))
            .count();
        for _ in 0..40 {
            sys.tick(false, &world, &mut dev);
        }
        // The observable is the STOP submitted to the device, not the live
        // entry vanishing: an entry is reclaimed when the device reports its
        // channel finished, which is asynchronous in vanilla too and which a
        // recording device never volunteers. A witness that waited for the
        // live set to empty would be waiting on the test's own device.
        let stops_after = dev
            .calls
            .iter()
            .filter(|(_, c)| matches!(c, ChannelCall::Stop))
            .count();
        assert!(
            stops_after > stops_before,
            "the ramp stopped itself — nobody sent it a stop"
        );
        // …and it went silent on the way, rather than being cut at full gain.
        assert!(sys.engine.biome_fade("a").expect("still tracked") < 0);
    }
    /// **The reuse matches on the RAMP KIND as well as the identifier**, and
    /// dropping that guard is worse than it sounds.
    ///
    /// A server can send an ordinary `sound` packet naming the very event a
    /// biome uses for its bed — `ambient.nether_wastes.loop` is a registry
    /// sound like any other. Matching on the identifier alone then finds that
    /// one-shot, fails the `if let` that fades it (it is not a biome-loop
    /// ramp), and returns "nothing to create" — so **the bed never starts at
    /// all**, silently, for as long as the coincidental sound is live.
    #[test]
    fn a_coincidental_sound_of_the_same_name_does_not_stand_in_for_the_loop() {
        let idx = index_of(&["a"]);
        let mut sys = SoundSystem::new(idx);
        let mut dev = RecordingDevice::default();
        let table = rewo_world::entities::EntityTable::default();
        let world = EntityTableWorld {
            table: &table,
            local: None,
            game_time: 0,
        };

        // An ordinary positioned sound that happens to be named "a".
        sys.accept(
            &[SoundEvent::Local(crate::sounds::LocalSound {
                name: "a".into(),
                source: SoundSource::Ambient,
                x: 0.0,
                y: 0.0,
                z: 0.0,
                volume: 1.0,
                pitch: 1.0,
            })],
            &registry(),
            &world,
            &mut dev,
        );
        assert_eq!(sys.engine.live_identifiers(), vec!["a"]);
        assert!(
            sys.engine.biome_fade("a").is_none(),
            "it is not a biome loop — only its name matches"
        );

        // Entering a biome whose loop is also "a" must still START one.
        sys.accept(
            &[SoundEvent::BiomeLoopTransition {
                current: Some("a".into()),
            }],
            &registry(),
            &world,
            &mut dev,
        );
        assert_eq!(
            sys.engine.live_identifiers().len(),
            2,
            "the bed is its own voice, alongside the coincidental one-shot"
        );
        assert!(
            sys.engine.biome_fade("a").is_some(),
            "and it really is a biome loop, with a ramp that can fade"
        );
    }
}
#[cfg(test)]
mod listener_tests {
    //! `ListenerTransform` and `Camera.setRotation`'s basis (M138a).
    //!
    //! Before this, nothing in Rewo carried a listener at all: `AudioDevice`
    //! had four methods and none of them was one, so every `SetSelfPosition`
    //! was an absolute world coordinate panned against ears at the origin
    //! facing -Z. These pin the basis, the record, and that the seam carries it.

    use super::{listener_basis, AudioDevice, ListenerTransform, RecordingDevice};

    /// `Entity.calculateViewVector` — an INDEPENDENT derivation of the same
    /// forward vector, reached without composing a single rotation.
    ///
    /// ```java
    /// float f = pitch * (pi/180), g = -yaw * (pi/180);
    /// float h = cos(g), i = sin(g), j = cos(f), k = sin(f);
    /// return new Vec3(i * j, -k, h * j);
    /// ```
    ///
    /// This is what makes `listener_basis` checkable rather than
    /// self-consistent: `Camera.setRotation` builds the vector by rotating
    /// `(0,0,-1)` through `Ry(pi - yaw) * Rx(-pitch)`, and this reaches the same
    /// place by a different route, so agreeing is evidence.
    fn view_vector(yaw_deg: f32, pitch_deg: f32) -> [f32; 3] {
        let f = pitch_deg.to_radians();
        let g = -yaw_deg.to_radians();
        [g.sin() * f.cos(), -f.sin(), g.cos() * f.cos()]
    }

    fn close(a: [f32; 3], b: [f32; 3], what: &str) {
        for i in 0..3 {
            assert!(
                (a[i] - b[i]).abs() < 1e-5,
                "{what}: component {i}, {a:?} vs {b:?}"
            );
        }
    }

    #[test]
    fn forward_agrees_with_calculate_view_vector_everywhere() {
        // Not one angle: a transposed sine or a dropped negation agrees at 0 and
        // diverges elsewhere, and a fixture at the origin is the shape this
        // repo keeps being caught by.
        for yaw in [-180.0f32, -135.0, -90.0, -45.0, 0.0, 30.0, 90.0, 179.0] {
            for pitch in [-90.0f32, -45.0, -12.5, 0.0, 17.0, 45.0, 90.0] {
                let (fwd, _) = listener_basis(yaw, pitch);
                close(fwd, view_vector(yaw, pitch), &format!("yaw {yaw} pitch {pitch}"));
            }
        }
    }

    #[test]
    fn the_basis_is_orthonormal() {
        // `alListenerfv(AL_ORIENTATION, ...)` takes an at-vector and an
        // up-vector; OpenAL orthonormalises, but a basis that is not already
        // orthogonal means the up we computed is not the up that gets used, and
        // the error is silent.
        for yaw in [0.0f32, 37.0, -128.0] {
            for pitch in [0.0f32, 41.0, -73.0] {
                let (f, u) = listener_basis(yaw, pitch);
                let dot = f[0] * u[0] + f[1] * u[1] + f[2] * u[2];
                assert!(dot.abs() < 1e-5, "yaw {yaw} pitch {pitch}: dot {dot}");
                for (v, name) in [(f, "forward"), (u, "up")] {
                    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
                    assert!((len - 1.0).abs() < 1e-5, "{name} length {len}");
                }
            }
        }
    }

    #[test]
    fn up_is_not_the_constant_zero_one_zero() {
        // The tempting simplification, and wrong off the horizon. At pitch 90
        // the forward vector points straight down and `up` becomes the
        // horizontal heading — a listener pinned to (0,1,0) has a stereo image
        // that refuses to roll when you look up, which looks like nothing at all
        // in a log.
        let (fwd, up) = listener_basis(0.0, 90.0);
        close(fwd, [0.0, -1.0, 0.0], "looking straight down");
        close(up, [0.0, 0.0, 1.0], "up becomes the heading");

        // …and at the horizon it really is (0,1,0), so the constant is right for
        // exactly the fixture someone would have chosen.
        let (_, level) = listener_basis(45.0, 0.0);
        close(level, [0.0, 1.0, 0.0], "level");
    }

    #[test]
    fn right_is_forward_cross_up_and_not_the_reverse() {
        // The two differ by a sign, which is a stereo image against its mirror.
        // Facing -Z with up +Y, vanilla's `right()` is forward x up =
        // (0,0,-1) x (0,1,0) = (0*0 - -1*1, -1*0 - 0*0, 0*1 - 0*0) = (1, 0, 0).
        let r = ListenerTransform::INITIAL.right();
        assert!((r[0] - 1.0).abs() < 1e-9, "got {r:?}");
        assert!(r[1].abs() < 1e-9 && r[2].abs() < 1e-9, "got {r:?}");
    }

    #[test]
    fn initial_is_the_unrotated_constant_and_not_a_camera_at_yaw_zero() {
        // `new ListenerTransform(Vec3.ZERO, new Vec3(0, 0, -1), new Vec3(0, 1, 0))`.
        assert_eq!(ListenerTransform::INITIAL.position, [0.0, 0.0, 0.0]);
        assert_eq!(ListenerTransform::INITIAL.forward, [0.0, 0.0, -1.0]);
        assert_eq!(ListenerTransform::INITIAL.up, [0.0, 1.0, 0.0]);

        // **And a level camera at yaw 0 faces the OTHER way.** This test first
        // asserted the two were equal, which reads as obvious and is wrong:
        // `INITIAL` is `Camera`'s raw `FORWARDS` constant, the value the field
        // holds before any rotation, whereas `setRotation` opens with
        // `rotationYXZ(pi - yaw, ...)` — a half turn about Y even at yaw 0. So
        // yaw 0 is +Z (south, as Minecraft's yaw convention has it) and the
        // record's default is -Z. Conflating them would put the ears backwards
        // for exactly the fixture a test would reach for first.
        let (f, u) = listener_basis(0.0, 0.0);
        close(f, [0.0, 0.0, 1.0], "a level camera at yaw 0 faces +Z");
        assert!(
            (f[2] - ListenerTransform::INITIAL.forward[2]).abs() > 1.5,
            "the pi in rotationYXZ is what separates them"
        );
        close(u, ListenerTransform::INITIAL.up, "up does agree, at the horizon");
    }

    #[test]
    fn the_seam_carries_the_listener() {
        // The load-bearing one: a basis nothing pushes is inert, which is the
        // state every other test here would still pass in.
        let mut d = RecordingDevice::default();
        assert!(d.listener_history.is_empty());
        let (forward, up) = listener_basis(90.0, 0.0);
        d.set_listener(ListenerTransform {
            position: [1.0, 2.0, 3.0],
            forward,
            up,
        });
        assert_eq!(d.listener_history.len(), 1);
        assert_eq!(d.listener_history[0].position, [1.0, 2.0, 3.0]);
        close(d.listener_history[0].forward, [-1.0, 0.0, 0.0], "facing +yaw 90");
    }

    #[test]
    fn live_sounds_builds_the_transform_from_camera_angles() {
        // `update_listener` is the production entry point, and driving it rather
        // than `set_listener` is what stops this witnessing a helper the app does
        // not call. It goes to a `SilentDevice`, which counts rather than stores,
        // so the assertion is on the count moving.
        let live = super::LiveSounds::new(
            super::SoundsIndex::new(),
            rewo_data::sound_events::SoundEvents::default(),
        );
        let mut live = live;
        let before = live.device.calls_made;
        live.update_listener([0.0, 64.0, 0.0], 12.0, -3.0);
        assert_eq!(
            live.device.calls_made,
            before + 1,
            "the listener push must reach the device"
        );
    }


}
