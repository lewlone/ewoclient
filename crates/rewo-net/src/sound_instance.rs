//! Vanilla's sound-instance model and the arithmetic `SoundEngine` runs on it
//! (M131) — the half of audio a machine can grade.
//!
//! # What this is, and what it deliberately is not
//!
//! M63 decoded the three sound packets, M64 built the 1,968-entry
//! `sound_event` registry and M66 the weighted-variant index, and all three
//! stopped before playback because *playing* a sound needs a human to listen
//! before it can be called correct. This module takes the next step that does
//! **not** need one: everything vanilla computes **before** it hands anything
//! to OpenAL.
//!
//! There is no audio crate here, no device, no mixer, no `.ogg` decoder and no
//! resampler. Those are the user's decisions and they are explicitly deferred;
//! [`crate::sound_engine::AudioDevice`] is the seam they will plug into.
//!
//! What a test can assert about this module: the volume and pitch a sound is
//! given, the distance at which it becomes inaudible, whether it plays at all,
//! and the exact sequence of calls a device receives. What no test here can
//! assert: that any of it sounds right.
//!
//! # Ground truth (bundled 26.2 decompile)
//!
//! - `net/minecraft/client/resources/sounds/SoundInstance.java` — the
//!   interface, its two defaults, and `Attenuation`.
//! - `net/minecraft/client/resources/sounds/AbstractSoundInstance.java` — the
//!   field defaults and the two `get*` products.
//! - `net/minecraft/client/resources/sounds/SimpleSoundInstance.java` — the
//!   six named factories.
//! - `net/minecraft/client/resources/sounds/EntityBoundSoundInstance.java`.
//! - `net/minecraft/client/sounds/SoundEngine.java` — `calculateVolume`,
//!   `calculatePitch`, the attenuation distance and the two loop predicates.
//! - `net/minecraft/client/Options.java` — `getFinalSoundSourceVolume`.
//! - `net/minecraft/client/multiplayer/ClientLevel.java` — `playSeededSound`
//!   and the private `playSound` the packets reach.

use crate::sounds::SoundSource;

/// `SoundEngine.PITCH_MIN`.
pub const PITCH_MIN: f32 = 0.5;
/// `SoundEngine.PITCH_MAX`.
pub const PITCH_MAX: f32 = 2.0;
/// `SoundEngine.VOLUME_MIN`.
pub const VOLUME_MIN: f32 = 0.0;
/// `SoundEngine.VOLUME_MAX`.
pub const VOLUME_MAX: f32 = 1.0;

/// `SimpleSoundInstance.forUI(sound, pitch)`'s volume — the two-argument
/// overload delegates to the three-argument one with this literal, so a UI
/// click is a quarter of the volume its caller never mentions.
pub const UI_DEFAULT_VOLUME: f32 = 0.25;

/// `SimpleSoundInstance.forJukeboxSong`'s volume.
///
/// **Greater than one, and that is the whole point.** See
/// [`attenuation_distance`]: a volume above 1 does not make a sound louder
/// (the gain clamps at 1) — it makes it travel further, so a jukebox is
/// audible four times as far as an ordinary block sound.
pub const JUKEBOX_VOLUME: f32 = 4.0;

/// `SoundInstance.Attenuation`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Attenuation {
    /// The sound is heard at full gain everywhere. Vanilla's UI, music and
    /// local-ambience factories all use this, together with `relative = true`.
    None,
    /// `Channel.linearAttenuation` — OpenAL's `AL_LINEAR_DISTANCE`. See
    /// [`crate::sound_engine::openal`] for the one part of this that is not in
    /// the Minecraft decompile.
    #[default]
    Linear,
}

/// Which entity, if any, a sound follows.
///
/// `EntityBoundSoundInstance` is an `AbstractTickableSoundInstance`: it moves
/// with its entity every tick, stops when the entity is removed, and refuses
/// to start while the entity is silent. A positioned sound does none of that,
/// so the distinction is not cosmetic — it decides whether the instance enters
/// `SoundEngine.tickingSounds` at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Binding {
    /// `SimpleSoundInstance` — a fixed world position.
    #[default]
    Fixed,
    /// `EntityBoundSoundInstance` — follows the entity with this id.
    Entity(i32),
}

/// One `SoundInstance`, flattened.
///
/// Vanilla spreads these fields over an interface, an abstract class and eight
/// subclasses because the subclasses differ in *behaviour* (`tick`,
/// `canPlaySound`, `canStartSilent`). Only three of those behaviours are
/// reachable from the two sound packets and the client-local sounds M71
/// queues, and each is a predicate over data rather than code, so they are
/// fields here. The factories below are the named constructors, one per
/// vanilla `static` factory, so a caller never has to remember which of
/// `looping` / `delay` / `attenuation` / `relative` a given kind of sound
/// wants.
#[derive(Clone, Debug, PartialEq)]
pub struct SoundInstance {
    /// `getIdentifier()` — the **event** name (`minecraft:block.stone.break`),
    /// not the file. Resolving it to a file is
    /// [`rewo_data::sounds_json::SoundsIndex::get_sound`]'s job, and the two
    /// are genuinely different strings: an event with four variants names four
    /// files and none of them is the event.
    pub identifier: String,
    pub source: SoundSource,
    /// `AbstractSoundInstance.volume` — the instance's own multiplier, before
    /// the `sounds.json` entry's own volume is multiplied in. Defaults to 1.
    pub volume: f32,
    /// `AbstractSoundInstance.pitch`. Defaults to 1.
    pub pitch: f32,
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub looping: bool,
    pub delay: i32,
    pub attenuation: Attenuation,
    /// `isRelative()` — the position is relative to the **listener**, not to
    /// the world. Vanilla sets it on exactly the factories that also set
    /// `Attenuation.NONE` and a position of `(0, 0, 0)`, which together mean
    /// "in the player's head".
    pub relative: bool,
    /// `canStartSilent()` — the interface default is `false`; three tickable
    /// subclasses (bee, minecart, riding-entity) override it to `true` because
    /// they fade in from zero and would otherwise never start.
    pub can_start_silent: bool,
    pub binding: Binding,
    /// The `RandomSource.create(seed)` seed, when there is one.
    ///
    /// `None` is `SoundInstance.createUnseededRandom()` — vanilla's own
    /// factories for UI, music and ambience take it, because nothing else has
    /// to agree with them. A wire sound always has a seed so that every client
    /// picks the same variant.
    pub seed: Option<i64>,
}

impl SoundInstance {
    /// The `AbstractSoundInstance` field defaults, before any factory.
    ///
    /// Kept separate from [`Default`] on purpose: this is a *transcription* of
    /// the Java field initialisers, and the factories below are transcriptions
    /// of the constructors that overwrite them. Reading the two together is
    /// how you check that a factory really does leave `attenuation` at
    /// `LINEAR` rather than setting it.
    pub fn bare(identifier: impl Into<String>, source: SoundSource) -> SoundInstance {
        SoundInstance {
            identifier: identifier.into(),
            source,
            volume: 1.0,
            pitch: 1.0,
            x: 0.0,
            y: 0.0,
            z: 0.0,
            looping: false,
            delay: 0,
            attenuation: Attenuation::Linear,
            relative: false,
            can_start_silent: false,
            binding: Binding::Fixed,
            seed: None,
        }
    }

    /// `new SimpleSoundInstance(sound, source, volume, pitch, random, x, y, z)`
    /// — what `ClientLevel.playSound` builds for a `ClientboundSoundPacket`.
    ///
    /// The public eight-argument constructor delegates with `looping = false`,
    /// `delay = 0`, `Attenuation.LINEAR`, and then `relative = false`.
    pub fn simple(
        identifier: impl Into<String>,
        source: SoundSource,
        volume: f32,
        pitch: f32,
        seed: i64,
        x: f64,
        y: f64,
        z: f64,
    ) -> SoundInstance {
        SoundInstance {
            volume,
            pitch,
            x,
            y,
            z,
            seed: Some(seed),
            ..SoundInstance::bare(identifier, source)
        }
    }

    /// `new SimpleSoundInstance(sound, source, volume, pitch, random, pos)` —
    /// the `BlockPos` overload, which offsets to the **centre** of the block.
    pub fn at_block(
        identifier: impl Into<String>,
        source: SoundSource,
        volume: f32,
        pitch: f32,
        seed: i64,
        bx: i32,
        by: i32,
        bz: i32,
    ) -> SoundInstance {
        SoundInstance::simple(
            identifier,
            source,
            volume,
            pitch,
            seed,
            bx as f64 + 0.5,
            by as f64 + 0.5,
            bz as f64 + 0.5,
        )
    }

    /// `new EntityBoundSoundInstance(event, source, volume, pitch, entity, seed)`
    /// — what `ClientLevel.playSeededSound`'s entity overload builds.
    ///
    /// The initial position is `(float)entity.getX()` and friends: vanilla
    /// narrows each coordinate to `float` and stores it back into a `double`
    /// field, so the instance starts at a **truncated** copy of the entity's
    /// position and only `tick()` refreshes it. That narrowing is reproduced
    /// rather than tidied — it is worth about a millimetre near spawn and
    /// visibly more far out.
    pub fn entity_bound(
        identifier: impl Into<String>,
        source: SoundSource,
        volume: f32,
        pitch: f32,
        seed: i64,
        entity_id: i32,
        ex: f64,
        ey: f64,
        ez: f64,
    ) -> SoundInstance {
        SoundInstance {
            volume,
            pitch,
            x: ex as f32 as f64,
            y: ey as f32 as f64,
            z: ez as f32 as f64,
            binding: Binding::Entity(entity_id),
            seed: Some(seed),
            ..SoundInstance::bare(identifier, source)
        }
    }

    /// `SimpleSoundInstance.forUI(sound, pitch)`.
    pub fn for_ui(identifier: impl Into<String>, pitch: f32) -> SoundInstance {
        SoundInstance::for_ui_with_volume(identifier, pitch, UI_DEFAULT_VOLUME)
    }

    /// `SimpleSoundInstance.forUI(sound, pitch, volume)` — `SoundSource.UI`,
    /// no attenuation, relative, at the origin.
    pub fn for_ui_with_volume(
        identifier: impl Into<String>,
        pitch: f32,
        volume: f32,
    ) -> SoundInstance {
        SoundInstance {
            volume,
            pitch,
            attenuation: Attenuation::None,
            relative: true,
            ..SoundInstance::bare(identifier, SoundSource::Ui)
        }
    }

    /// `SimpleSoundInstance.forMusic(sound)`.
    ///
    /// Note what it does **not** set: `canStartSilent` stays `false`. Music
    /// starts at zero volume anyway, because
    /// [`SoundEngine::play`](crate::sound_engine::SoundEngine::play) special-cases
    /// `SoundSource::Music` beside `can_start_silent` rather than through it.
    pub fn for_music(identifier: impl Into<String>) -> SoundInstance {
        SoundInstance {
            attenuation: Attenuation::None,
            relative: true,
            ..SoundInstance::bare(identifier, SoundSource::Music)
        }
    }

    /// `SimpleSoundInstance.forJukeboxSong(sound, pos)` — `RECORDS`, volume
    /// [`JUKEBOX_VOLUME`], **linear** attenuation and **not** relative, which
    /// is what makes a jukebox a thing in the world rather than in your head.
    pub fn for_jukebox_song(identifier: impl Into<String>, x: f64, y: f64, z: f64) -> SoundInstance {
        SoundInstance {
            volume: JUKEBOX_VOLUME,
            x,
            y,
            z,
            ..SoundInstance::bare(identifier, SoundSource::Records)
        }
    }

    /// `SimpleSoundInstance.forLocalAmbience(sound, pitch, volume)`.
    ///
    /// **Pitch is the second argument and volume the third**, the opposite of
    /// every other factory here and of the constructor it calls. M66 recorded
    /// this from the other side (`level_event`'s table); reading the argument
    /// list left to right gives a sound three times too loud at a fixed pitch.
    pub fn for_local_ambience(
        identifier: impl Into<String>,
        pitch: f32,
        volume: f32,
    ) -> SoundInstance {
        SoundInstance {
            volume,
            pitch,
            attenuation: Attenuation::None,
            relative: true,
            ..SoundInstance::bare(identifier, SoundSource::Ambient)
        }
    }

    /// `SimpleSoundInstance.forAmbientAddition(sound)`.
    pub fn for_ambient_addition(identifier: impl Into<String>) -> SoundInstance {
        SoundInstance::for_local_ambience(identifier, 1.0, 1.0)
    }

    /// `SimpleSoundInstance.forAmbientMood(sound, random, x, y, z)` — unlike
    /// every other ambience factory this one is **positioned and attenuated**,
    /// which is why a cave mood sound seems to come from somewhere.
    pub fn for_ambient_mood(
        identifier: impl Into<String>,
        x: f64,
        y: f64,
        z: f64,
    ) -> SoundInstance {
        SoundInstance {
            x,
            y,
            z,
            ..SoundInstance::bare(identifier, SoundSource::Ambient)
        }
    }

    /// `SoundEngine.requiresManualLooping` — `getDelay() > 0`.
    pub fn requires_manual_looping(&self) -> bool {
        self.delay > 0
    }

    /// `SoundEngine.shouldLoopManually` — re-queued by the engine after the
    /// channel stops, `delay` ticks later.
    pub fn should_loop_manually(&self) -> bool {
        self.looping && self.requires_manual_looping()
    }

    /// `SoundEngine.shouldLoopAutomatically` — looped by the device itself.
    ///
    /// The two predicates partition a looping sound by whether it has a delay,
    /// and they are not interchangeable: an automatic loop never leaves the
    /// channel, so it is never reclaimed and never re-picks its variant, while
    /// a manual one does both.
    pub fn should_loop_automatically(&self) -> bool {
        self.looping && !self.requires_manual_looping()
    }

    /// `EntityBoundSoundInstance.canPlaySound()` = `!entity.isSilent()`; every
    /// other reachable instance takes the interface default `true`.
    ///
    /// `silent` is passed in rather than read, because **Rewo does not decode
    /// `Entity.DATA_SILENT`** — it is `SynchedEntityData` index 4 and
    /// `metadata.rs` skips it. Until it does, every caller passes `false` and
    /// a `/data`-silenced entity's sounds are **not** suppressed. That is a
    /// stated gap, not a modelling choice: the predicate is here and correct,
    /// and the input that would make it fire is missing.
    pub fn can_play_sound(&self, silent: bool) -> bool {
        match self.binding {
            Binding::Fixed => true,
            Binding::Entity(_) => !silent,
        }
    }

    /// Whether this instance enters `SoundEngine.tickingSounds`.
    ///
    /// `instance instanceof TickableSoundInstance` — of the instances reachable
    /// from the wire, only the entity-bound one is.
    pub fn is_tickable(&self) -> bool {
        matches!(self.binding, Binding::Entity(_))
    }
}

/// `Options.soundSourceVolumes` — the eleven sliders, each in `0..=1`.
///
/// A separate type from anything in `rewo-net` because it is *not* protocol:
/// the server never sends it and never sees it. It is here so the volume
/// arithmetic has one input rather than eleven loose floats.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CategoryVolumes {
    per_source: [f32; SoundSource::ALL.len()],
}

impl Default for CategoryVolumes {
    /// `createSoundSliderOptionInstance(..., 1.0, ...)` — every slider starts
    /// at full.
    fn default() -> Self {
        CategoryVolumes {
            per_source: [1.0; SoundSource::ALL.len()],
        }
    }
}

impl CategoryVolumes {
    /// `Options.getSoundSourceVolume(source)` — the raw slider.
    pub fn slider(&self, source: SoundSource) -> f32 {
        self.per_source[source.ordinal() as usize]
    }

    pub fn set_slider(&mut self, source: SoundSource, value: f32) {
        self.per_source[source.ordinal() as usize] = value;
    }

    /// `Options.getFinalSoundSourceVolume(source)`.
    ///
    /// **`MASTER` returns its own slider, not its square.** Every other
    /// category is multiplied by `MASTER`; `MASTER` is not multiplied by
    /// itself. The plausible-and-wrong version — always multiplying by master
    /// — is invisible while the master slider sits at 1 and halves twice at
    /// 0.5, and `SoundSource::Master` is a real category a datapack can send.
    pub fn final_volume(&self, source: SoundSource) -> f32 {
        if source == SoundSource::Master {
            self.slider(SoundSource::Master)
        } else {
            self.slider(source) * self.slider(SoundSource::Master)
        }
    }
}

/// `net.minecraft.util.Mth.clamp(float, float, float)`.
///
/// Written out rather than using `f32::clamp` because the two differ on `NaN`:
/// Java's is a pair of `if`s that returns the value itself when it compares
/// false against both bounds, so `Mth.clamp(NaN, 0, 1)` is `NaN`, while Rust's
/// `f32::clamp` panics on a `NaN` *bound* and returns `NaN` for a `NaN` value.
/// The value can be `NaN` here — a server may send any float as a volume — so
/// the difference is reachable.
pub fn mth_clamp(value: f32, min: f32, max: f32) -> f32 {
    if value < min {
        min
    } else if value > max {
        max
    } else {
        value
    }
}

/// `SoundEngine.calculateVolume(float, SoundSource)` — the gain a channel is
/// given, in `0..=1`.
///
/// Three factors, and each is a different kind of thing:
///
/// 1. the instance's volume (already multiplied by the `sounds.json` entry's
///    own volume — see [`instance_volume`]), clamped to `0..=1`;
/// 2. the player's sliders, via [`CategoryVolumes::final_volume`], clamped;
/// 3. `gainBySource`, a **runtime** per-category gain that is not a setting.
///    It defaults to 1 and vanilla moves it from exactly one place —
///    `MusicManager.fadePlaying`, which is how music fades rather than cutting.
///
/// The third factor is deliberately not clamped here: vanilla clamps it on the
/// way *in* (`updateCategoryVolume`) and reads it raw here.
pub fn calculate_volume(volume: f32, source: SoundSource, options: &CategoryVolumes, gain: f32) -> f32 {
    mth_clamp(volume, VOLUME_MIN, VOLUME_MAX)
        * mth_clamp(options.final_volume(source), VOLUME_MIN, VOLUME_MAX)
        * gain
}

/// `SoundEngine.calculatePitch(SoundInstance)`.
pub fn calculate_pitch(pitch: f32) -> f32 {
    mth_clamp(pitch, PITCH_MIN, PITCH_MAX)
}

/// `AbstractSoundInstance.getVolume()` — `this.volume * sound.getVolume().sample(random)`.
///
/// The `SampledFloat` never touches the generator in practice: the only
/// producer of a `Sound` is `SoundEventRegistrationSerializer`, which reads
/// `volume` with `getAsFloat` and wraps it in `ConstantFloat`, whose `sample`
/// returns its field. A redirect makes it a `MultipliedFloats` of two
/// `ConstantFloat`s, which is still draw-free. **That is why the packet seed
/// selects a variant and nothing else** — there is exactly one `nextInt` per
/// level of redirect and no other draw anywhere in the resolution.
pub fn instance_volume(instance_volume: f32, sound_volume: f32) -> f32 {
    instance_volume * sound_volume
}

/// `AbstractSoundInstance.getPitch()`.
pub fn instance_pitch(instance_pitch: f32, sound_pitch: f32) -> f32 {
    instance_pitch * sound_pitch
}

/// `SoundEngine.play`'s `attenuationDistance`.
///
/// **This is the second, unclamped role of `volume`, and it is the one that
/// surprises.** The gain clamps at 1, so a `/playsound … 4 1` is no louder
/// than a `/playsound … 1 1` — but its `max(volume, 1)` multiplies the
/// `sounds.json` entry's `attenuation_distance`, so it is audible four times
/// as far. The `max` with 1 is what stops a *quiet* sound also becoming a
/// *short-ranged* one: at volume 0.2 the range is unchanged, not a fifth.
///
/// The argument is the **already-multiplied** instance volume (vanilla calls
/// `instance.getVolume()` once and uses the result for both roles), which
/// matters because `getVolume()` is not a pure getter.
pub fn attenuation_distance(instance_volume: f32, sound_attenuation_distance: i32) -> f32 {
    instance_volume.max(1.0) * sound_attenuation_distance as f32
}

/// The `range` `SoundEngine.play` reports to its `SoundEventListener`s — the
/// distance within which the subtitle overlay shows this sound.
///
/// Infinite for a relative sound or one with no attenuation, which is why a UI
/// click and the music both caption wherever you are.
pub fn listener_range(instance: &SoundInstance, attenuation_distance: f32) -> f32 {
    if !instance.relative && instance.attenuation != Attenuation::None {
        attenuation_distance
    } else {
        f32::INFINITY
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- the field defaults ------------------------------------------------

    #[test]
    fn bare_instance_matches_the_abstract_class_field_initialisers() {
        let i = SoundInstance::bare("minecraft:block.stone.break", SoundSource::Blocks);
        assert_eq!(i.volume, 1.0);
        assert_eq!(i.pitch, 1.0);
        assert_eq!((i.x, i.y, i.z), (0.0, 0.0, 0.0));
        assert!(!i.looping);
        assert_eq!(i.delay, 0);
        // `protected SoundInstance.Attenuation attenuation = LINEAR;` — the
        // default is attenuated, and the factories that want NONE say so.
        assert_eq!(i.attenuation, Attenuation::Linear);
        assert!(!i.relative);
        // `SoundInstance.canStartSilent()`'s interface default.
        assert!(!i.can_start_silent);
    }

    // ---- the factories -----------------------------------------------------

    #[test]
    fn for_ui_uses_the_quarter_volume_the_two_argument_overload_never_mentions() {
        let i = SoundInstance::for_ui("minecraft:ui.button.click", 1.0);
        assert_eq!(i.volume, 0.25);
        assert_eq!(i.source, SoundSource::Ui);
        assert_eq!(i.attenuation, Attenuation::None);
        assert!(i.relative);
        assert_eq!((i.x, i.y, i.z), (0.0, 0.0, 0.0));
    }

    #[test]
    fn for_local_ambience_takes_pitch_second_and_volume_third() {
        // The trap: reading `forLocalAmbience(sound, pitch, volume)` left to
        // right as (volume, pitch). Give the two arguments different values so
        // a transposition is visible.
        let i = SoundInstance::for_local_ambience("minecraft:block.portal.travel", 0.25, 3.0);
        assert_eq!(i.pitch, 0.25, "second argument is the PITCH");
        assert_eq!(i.volume, 3.0, "third argument is the VOLUME");
        assert_eq!(i.source, SoundSource::Ambient);
        assert_eq!(i.attenuation, Attenuation::None);
        assert!(i.relative);
    }

    #[test]
    fn ambient_mood_is_the_one_positioned_attenuated_ambience_factory() {
        let addition = SoundInstance::for_ambient_addition("minecraft:ambient.cave");
        let mood = SoundInstance::for_ambient_mood("minecraft:ambient.cave", 10.0, 20.0, 30.0);
        assert_eq!(addition.attenuation, Attenuation::None);
        assert!(addition.relative);
        assert_eq!(mood.attenuation, Attenuation::Linear);
        assert!(!mood.relative);
        assert_eq!((mood.x, mood.y, mood.z), (10.0, 20.0, 30.0));
    }

    #[test]
    fn jukebox_is_records_at_volume_four_and_stays_in_the_world() {
        let i = SoundInstance::for_jukebox_song("minecraft:music_disc.cat", 1.0, 2.0, 3.0);
        assert_eq!(i.source, SoundSource::Records);
        assert_eq!(i.volume, 4.0);
        assert_eq!(i.attenuation, Attenuation::Linear);
        assert!(!i.relative, "a jukebox is a thing in the world, not in your head");
    }

    #[test]
    fn for_music_does_not_set_can_start_silent() {
        // The engine's zero-volume escape is `!canStartSilent() && source !=
        // MUSIC`; music takes the second disjunct, not the first. A model that
        // set the flag here instead would agree on music and disagree on the
        // three tickable classes that really do set it.
        let i = SoundInstance::for_music("minecraft:music.game");
        assert!(!i.can_start_silent);
        assert_eq!(i.source, SoundSource::Music);
    }

    #[test]
    fn at_block_centres_on_the_block() {
        let i = SoundInstance::at_block("e", SoundSource::Blocks, 1.0, 1.0, 0, -4, 7, -9);
        assert_eq!((i.x, i.y, i.z), (-3.5, 7.5, -8.5));
    }

    #[test]
    fn entity_bound_narrows_its_initial_position_through_f32() {
        // `this.x = (float)this.entity.getX();` into a `double` field. Pick a
        // coordinate that is not representable in f32 so the narrowing shows.
        let ex = 1_000_000.1_f64;
        let i = SoundInstance::entity_bound("e", SoundSource::Neutral, 1.0, 1.0, 0, 7, ex, 0.0, 0.0);
        assert_eq!(i.x, ex as f32 as f64);
        assert_ne!(i.x, ex, "the f32 narrowing is real, not a no-op");
        assert_eq!(i.binding, Binding::Entity(7));
        assert!(i.is_tickable());
    }

    #[test]
    fn only_an_entity_bound_instance_ticks_or_can_be_silenced() {
        let fixed = SoundInstance::simple("e", SoundSource::Blocks, 1.0, 1.0, 0, 0.0, 0.0, 0.0);
        let bound = SoundInstance::entity_bound("e", SoundSource::Blocks, 1.0, 1.0, 0, 1, 0.0, 0.0, 0.0);
        assert!(!fixed.is_tickable());
        assert!(bound.is_tickable());
        assert!(fixed.can_play_sound(true), "a fixed sound has no entity to be silent");
        assert!(!bound.can_play_sound(true));
        assert!(bound.can_play_sound(false));
    }

    // ---- the loop predicates ----------------------------------------------

    #[test]
    fn the_two_loop_predicates_partition_a_looping_sound_by_its_delay() {
        let mut i = SoundInstance::bare("e", SoundSource::Blocks);
        i.looping = true;
        assert!(i.should_loop_automatically());
        assert!(!i.should_loop_manually());
        i.delay = 1;
        assert!(!i.should_loop_automatically());
        assert!(i.should_loop_manually());
        // Not looping: neither, whatever the delay.
        i.looping = false;
        assert!(!i.should_loop_automatically());
        assert!(!i.should_loop_manually());
        i.delay = 0;
        assert!(!i.should_loop_automatically());
        assert!(!i.should_loop_manually());
    }

    // ---- the volume arithmetic --------------------------------------------

    #[test]
    fn master_is_not_multiplied_by_itself() {
        let mut v = CategoryVolumes::default();
        v.set_slider(SoundSource::Master, 0.5);
        v.set_slider(SoundSource::Blocks, 0.5);
        // `source == MASTER ? vol(MASTER) : vol(source) * vol(MASTER)`.
        assert_eq!(v.final_volume(SoundSource::Master), 0.5, "not 0.25");
        assert_eq!(v.final_volume(SoundSource::Blocks), 0.25);
    }

    #[test]
    fn master_at_zero_silences_every_category_including_itself() {
        let mut v = CategoryVolumes::default();
        v.set_slider(SoundSource::Master, 0.0);
        for source in SoundSource::ALL {
            assert_eq!(v.final_volume(source), 0.0, "{source:?}");
        }
    }

    #[test]
    fn calculate_volume_is_the_three_factor_product() {
        let mut v = CategoryVolumes::default();
        v.set_slider(SoundSource::Master, 0.5);
        v.set_slider(SoundSource::Hostile, 0.5);
        // 0.8 * (0.5*0.5) * 0.5
        let g = calculate_volume(0.8, SoundSource::Hostile, &v, 0.5);
        assert!((g - 0.1).abs() < 1e-6, "{g}");
    }

    #[test]
    fn calculate_volume_clamps_the_instance_volume_but_the_runtime_gain_is_raw() {
        let v = CategoryVolumes::default();
        // A volume of 4 is clamped to 1 — the gain does not exceed full.
        assert_eq!(calculate_volume(4.0, SoundSource::Records, &v, 1.0), 1.0);
        // The third factor is read raw. Vanilla clamps it in
        // `updateCategoryVolume`, not here, so a value above 1 arriving by any
        // other route would exceed 1 — and reproducing that is the point of
        // transcribing rather than tidying.
        assert_eq!(calculate_volume(1.0, SoundSource::Records, &v, 2.0), 2.0);
    }

    #[test]
    fn a_negative_volume_clamps_to_silence_rather_than_inverting() {
        let v = CategoryVolumes::default();
        assert_eq!(calculate_volume(-3.0, SoundSource::Blocks, &v, 1.0), 0.0);
    }

    #[test]
    fn mth_clamp_passes_nan_through_where_a_naive_min_max_would_not() {
        // Java's `Mth.clamp` is `value < min ? min : value > max ? max : value`
        // — both comparisons are false for NaN, so NaN falls through.
        assert!(mth_clamp(f32::NAN, 0.0, 1.0).is_nan());
        // The plausible-and-wrong `value.max(min).min(max)` returns 0.0 for
        // NaN on the first call, silently turning a malformed packet into
        // silence rather than into a NaN a caller can see.
        assert_eq!(f32::NAN.max(0.0_f32).min(1.0), 0.0);
    }

    #[test]
    fn pitch_clamps_to_the_half_to_double_band() {
        assert_eq!(calculate_pitch(0.1), 0.5);
        assert_eq!(calculate_pitch(9.0), 2.0);
        assert_eq!(calculate_pitch(1.5), 1.5);
        // The band is exactly `PITCH_MIN..=PITCH_MAX` from the decompile's own
        // `private static final float` literals.
        assert_eq!(PITCH_MIN, 0.5);
        assert_eq!(PITCH_MAX, 2.0);
    }

    #[test]
    fn the_sounds_json_entry_multiplies_rather_than_replaces() {
        assert_eq!(instance_volume(0.5, 0.4), 0.2);
        assert_eq!(instance_pitch(2.0, 0.5), 1.0);
    }

    // ---- attenuation distance ----------------------------------------------

    #[test]
    fn volume_above_one_extends_the_range_instead_of_raising_the_gain() {
        let v = CategoryVolumes::default();
        // A jukebox: gain saturates, range quadruples.
        assert_eq!(calculate_volume(JUKEBOX_VOLUME, SoundSource::Records, &v, 1.0), 1.0);
        assert_eq!(attenuation_distance(JUKEBOX_VOLUME, 16), 64.0);
    }

    #[test]
    fn a_quiet_sound_keeps_its_full_range() {
        // The `max(_, 1.0)`. Without it a volume-0.2 sound would carry a fifth
        // as far, which is not what vanilla does.
        assert_eq!(attenuation_distance(0.2, 16), 16.0);
        assert_eq!(attenuation_distance(1.0, 16), 16.0);
    }

    #[test]
    fn the_sounds_json_attenuation_distance_is_the_multiplicand() {
        // `sounds.json`'s `attenuation_distance` defaults to 16 and packs may
        // set anything; the product is what reaches the device.
        assert_eq!(attenuation_distance(1.0, 4), 4.0);
        assert_eq!(attenuation_distance(2.0, 100), 200.0);
    }

    #[test]
    fn listener_range_is_infinite_exactly_when_the_sound_is_unplaced() {
        let mut i = SoundInstance::simple("e", SoundSource::Blocks, 1.0, 1.0, 0, 0.0, 0.0, 0.0);
        assert_eq!(listener_range(&i, 16.0), 16.0);
        i.attenuation = Attenuation::None;
        assert!(listener_range(&i, 16.0).is_infinite());
        i.attenuation = Attenuation::Linear;
        i.relative = true;
        assert!(listener_range(&i, 16.0).is_infinite());
    }
}
