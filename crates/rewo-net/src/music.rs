//! `MusicManager`'s gain ramp — the only writer of `gainBySource` (M140).
//!
//! `SoundEngine.calculateVolume` is three factors, and the third is
//! `gainBySource.getFloat(source)` (`SoundEngine.java:468`). That map has a
//! default of 1.0 and exactly **one writer in the entire client**:
//! `updateCategoryVolume`, whose only caller is `MusicManager.java:133`, for
//! `SoundSource.MUSIC` alone.
//!
//! That is worth saying plainly, because the obvious reading is wrong. The third
//! factor looks like an options-screen volume slider and is not — the sliders
//! arrive through `getFinalSoundSourceVolume`, which is the *second* factor.
//! `gainBySource` is the music crossfade and nothing else, so a client with no
//! music manager has it pinned at 1.0 for every category, correctly.
//!
//! ## The two branches have completely different shapes
//!
//! ```java
//! if (this.currentGain < volume) {
//!    this.currentGain = this.currentGain + Mth.clamp(this.currentGain, 5.0E-4F, 0.005F);
//!    if (this.currentGain > volume) { this.currentGain = volume; }
//! } else {
//!    this.currentGain = 0.03F * volume + 0.97F * this.currentGain;
//!    if (Math.abs(this.currentGain - volume) < 1.0E-4F || this.currentGain < volume) {
//!       this.currentGain = volume;
//!    }
//! }
//! ```
//! (`MusicManager.java:116-126`.)
//!
//! **Fading up, the STEP IS THE CURRENT GAIN**, clamped to `[0.0005, 0.005]` —
//! not a constant. So a track rising from silence accelerates (each tick adds
//! what it currently is) until it saturates at half a percent per tick. Writing
//! a constant step is the natural implementation and gives a fade that is far
//! too fast at the start and too slow at the end.
//!
//! **Fading down is an exponential blend** toward the target, a different curve
//! entirely — and its guard has a second disjunct, `currentGain < volume`,
//! which catches an overshoot the blend can produce.
//!
//! **The floor STOPS the music rather than clamping it.** At `<= 1.0E-4` the
//! manager calls `stopPlaying()` and returns false; a client that clamped to
//! zero instead would leave a silent track holding a streaming channel for as
//! long as it ran.

use rewo_world::particles::LegacyRandom;

/// `MusicManager`'s crossfade state.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MusicFade {
    gain: f32,
    /// `currentMusic != null`.
    playing: bool,
}

impl Default for MusicFade {
    fn default() -> Self {
        // `private float currentGain = 1.0F;` — full, not silent, so the first
        // track starts at volume rather than fading in from nothing.
        MusicFade {
            gain: 1.0,
            playing: false,
        }
    }
}

impl MusicFade {
    /// The gain to publish as `updateCategoryVolume(MUSIC, …)`.
    pub fn gain(&self) -> f32 {
        self.gain
    }

    pub fn is_playing(&self) -> bool {
        self.playing
    }

    /// A track started — `currentMusic = …`.
    pub fn start(&mut self) {
        self.playing = true;
    }

    /// `MusicManager.tick`'s guard, which is the half that decides whether the
    /// fade runs at all: `currentMusic != null && currentGain != volume`.
    ///
    /// Returns the gain to publish, or `None` when nothing should be pushed —
    /// either because there is no music, or because the fade ended the track.
    ///
    /// **The equality is an exact float compare**, and deliberately so: once the
    /// ramp assigns `currentGain = volume` the branch stops running entirely,
    /// which is what makes the fade terminate rather than creep.
    pub fn tick(&mut self, volume: f32) -> Option<f32> {
        if !self.playing {
            return None;
        }
        if self.gain == volume {
            return Some(self.gain);
        }
        if self.fade(volume) {
            Some(self.gain)
        } else {
            None
        }
    }

    /// `fadePlaying(volume)` — `false` once the track has been stopped.
    pub fn fade(&mut self, volume: f32) -> bool {
        if !self.playing {
            return false;
        }
        if self.gain == volume {
            return true;
        }
        if self.gain < volume {
            // The step is the gain itself, clamped. Not a constant: a track
            // rising from silence accelerates until it saturates at 0.005.
            self.gain += self.gain.clamp(5.0e-4, 0.005);
            if self.gain > volume {
                self.gain = volume;
            }
        } else {
            // A blend, not a subtraction — a different curve from the way up.
            self.gain = 0.03 * volume + 0.97 * self.gain;
            // The second disjunct catches an overshoot the blend can produce.
            if (self.gain - volume).abs() < 1.0e-4 || self.gain < volume {
                self.gain = volume;
            }
        }
        self.gain = self.gain.clamp(0.0, 1.0);
        if self.gain <= 1.0e-4 {
            // `stopPlaying()` — the track ends rather than sitting silent.
            self.playing = false;
            return false;
        }
        true
    }
}

// ── the selection and the timers (M145) ───────────────────────────────────

use rewo_world::music::Music;

/// `MusicManager.STARTING_DELAY` — five seconds before the first track.
pub const STARTING_DELAY: i32 = 100;

/// `MusicManager.MusicFrequency` — the options slider, in ticks.
///
/// `maxFrequencyMinutes * 1200` in the constructor, so the stored value is
/// **ticks**: 20 minutes is 24 000 and 10 is 12 000. `CONSTANT` stores **0**,
/// which matters in one place — see [`Self::next_song_delay`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum MusicFrequency {
    #[default]
    Default,
    Frequent,
    Constant,
}

impl MusicFrequency {
    /// `maxFrequency`, in ticks.
    pub fn max_frequency(self) -> i32 {
        match self {
            MusicFrequency::Default => 20 * 1200,
            MusicFrequency::Frequent => 10 * 1200,
            MusicFrequency::Constant => 0,
        }
    }

    /// `getNextSongDelay(music, random)`.
    ///
    /// ```java
    /// if (music == null) return this.maxFrequency;
    /// if (this == CONSTANT) return 100;
    /// int minFrequency = Math.min(music.minDelay(), this.maxFrequency);
    /// int maxFrequency = Math.min(music.maxDelay(), this.maxFrequency);
    /// return Mth.nextInt(random, minFrequency, maxFrequency);
    /// ```
    ///
    /// **The null check comes FIRST, and that inverts `CONSTANT`.** With no
    /// music to describe, `CONSTANT` returns its `maxFrequency` — which is
    /// **0**, not the 100 the line below would give. Reordering the two, which
    /// reads more natural, makes `stopPlaying()` on a silent screen queue the
    /// next track five seconds out instead of immediately.
    ///
    /// Both bounds are clamped to `maxFrequency`, so a track whose own window is
    /// wider than the slider is squeezed into the slider — and because
    /// [`mth_next_int`] returns `min` unchanged when `min >= max`, a clamp that
    /// collapses the window also **skips the draw entirely**.
    pub fn next_song_delay(self, music: Option<&Music>, random: &mut LegacyRandom) -> i32 {
        let Some(music) = music else {
            return self.max_frequency();
        };
        if self == MusicFrequency::Constant {
            return 100;
        }
        let min = music.min_delay.min(self.max_frequency());
        let max = music.max_delay.min(self.max_frequency());
        mth_next_int(random, min, max)
    }
}

/// `Mth.nextInt(random, minInclusive, maxInclusive)`.
///
/// ```java
/// return minInclusive >= maxInclusive ? minInclusive : random.nextInt(maxInclusive - minInclusive + 1) + minInclusive;
/// ```
///
/// **Inclusive at both ends**, and the degenerate case does not merely return
/// early — it **does not draw**, so the generator is left un-advanced and every
/// later draw shifts. A version that always drew would produce a different
/// sequence of songs from the same seed while looking correct at every
/// individual call.
pub fn mth_next_int(random: &mut LegacyRandom, min_inclusive: i32, max_inclusive: i32) -> i32 {
    if min_inclusive >= max_inclusive {
        return min_inclusive;
    }
    random.next_int(max_inclusive - min_inclusive + 1) + min_inclusive
}

/// What one [`MusicManager::tick`] asks the caller to do.
///
/// The same seam M142 used for the biome loop: the manager names the outcome
/// and the engine applies it, because the manager has no `SoundManager` and
/// should not grow one. Several parts can be set on the same tick — a fade
/// publishes a volume *and* a stop can follow it — so this is a record rather
/// than an enum.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MusicOutcome {
    /// `soundManager.stop(currentMusic)`.
    pub stop_current: bool,
    /// `startPlaying(music)` — the track to begin.
    pub start: Option<Music>,
    /// `updateCategoryVolume(MUSIC, gain)`.
    pub category_volume: Option<f32>,
}

/// `MusicManager` — which track plays, and when the next one starts.
///
/// **It owns no sound.** `current` is the identifier of whatever the engine
/// last started on its behalf, and `is_active` is asked of the caller each
/// tick, so the manager stays a pure state machine that a test can drive
/// without an engine, a device or a clock.
#[derive(Clone, Debug)]
pub struct MusicManager {
    random: LegacyRandom,
    /// `currentMusic` — the sound event id of the playing track.
    current: Option<String>,
    frequency: MusicFrequency,
    next_song_delay: i32,
    fade: MusicFade,
}

impl Default for MusicManager {
    /// A fixed seed and `DEFAULT` frequency — vanilla's seed is unique per
    /// session, so nothing here can be reproduced against it anyway, and a
    /// deterministic default is what a witness can pin.
    fn default() -> MusicManager {
        MusicManager::new(0, MusicFrequency::Default)
    }
}

impl MusicManager {
    /// `random` is seeded by the caller. Vanilla's is
    /// `RandomSource.create()` — a `LegacyRandomSource` on a unique seed — so
    /// the sequence is not reproducible between sessions there either; making
    /// the seed an argument is what lets a witness pin one.
    pub fn new(seed: i64, frequency: MusicFrequency) -> MusicManager {
        MusicManager {
            random: LegacyRandom::new(seed),
            current: None,
            frequency,
            next_song_delay: STARTING_DELAY,
            fade: MusicFade::default(),
        }
    }

    pub fn current(&self) -> Option<&str> {
        self.current.as_deref()
    }

    pub fn next_song_delay(&self) -> i32 {
        self.next_song_delay
    }

    pub fn gain(&self) -> f32 {
        self.fade.gain()
    }

    /// `isPlayingMusic(music)`.
    pub fn is_playing_music(&self, music: &Music) -> bool {
        self.current.as_deref() == Some(music.sound.as_str())
    }

    /// `canReplace(music, currentMusic)` — replaces, **and is not already it**.
    ///
    /// The second half is what stops a replacing track restarting itself every
    /// tick: the menu music has `replaceCurrentMusic`, so without the identifier
    /// comparison it would stop and restart forever.
    fn can_replace(music: &Music, current: &str) -> bool {
        music.replace_current_music && music.sound != current
    }

    /// `MusicManager.tick(...)`, transcribed.
    ///
    /// `is_active` is `soundManager.isActive(currentMusic)`; `on_loading_screen`
    /// is `gui.screen() instanceof LevelLoadingScreen`.
    ///
    /// **The stop and the clear happen on the SAME tick.** Vanilla calls
    /// `soundManager.stop(currentMusic)` and then, four lines later, asks
    /// `isActive(currentMusic)` — which is now false, because `stop` is
    /// synchronous. So the replace path also takes the `!isActive` branch
    /// immediately, drawing a **second** random number and clearing
    /// `currentMusic` in the same pass. Modelling the stop as something the
    /// caller applies "next tick" delays every replacement by a tick and, worse,
    /// skips that second draw — which changes the whole subsequent song
    /// sequence for a given seed.
    pub fn tick(
        &mut self,
        volume: f32,
        music: Option<&Music>,
        is_active: bool,
        on_loading_screen: bool,
    ) -> MusicOutcome {
        let mut out = MusicOutcome::default();

        // `if (this.currentMusic != null && this.currentGain != volume)`.
        if self.current.is_some() && self.fade.gain() != volume {
            match self.fade.tick(volume) {
                Some(gain) => out.category_volume = Some(gain),
                None => {
                    // The fade hit the floor and stopped the track. `stopPlaying`
                    // sets the next delay from the *situational* music, which is
                    // the argument here.
                    out.stop_current = true;
                    self.stop_playing(music);
                    return out;
                }
            }
        }

        let Some(music) = music else {
            // Nothing to play here: hold the delay at at least five seconds so
            // walking back into a musical place does not start a track instantly.
            self.next_song_delay = self.next_song_delay.max(STARTING_DELAY);
            return out;
        };

        if let Some(current) = self.current.clone() {
            let mut active = is_active;
            if Self::can_replace(music, &current) {
                out.stop_current = true;
                active = false;
                self.next_song_delay = mth_next_int(&mut self.random, 0, music.min_delay / 2);
            }
            if !active {
                self.current = None;
                self.fade = MusicFade::default();
                self.next_song_delay = self
                    .next_song_delay
                    .min(self.frequency.next_song_delay(Some(music), &mut self.random));
            }
        }

        // **Outside the `currentMusic != null` block**, so it runs every tick a
        // track is on offer — which is what pulls the `Integer.MAX_VALUE`
        // `startPlaying` leaves behind back down to something finite.
        self.next_song_delay = self.next_song_delay.min(music.max_delay);

        if self.current.is_none() && !on_loading_screen {
            // Pre-decrement: the tick that takes it to zero is the tick that
            // starts the song, so a delay of 1 means "next tick".
            self.next_song_delay -= 1;
            if self.next_song_delay <= 0 {
                self.start_playing(music);
                out.start = Some(music.clone());
            }
        }
        out
    }

    /// `startPlaying(music)`.
    ///
    /// `nextSongDelay = Integer.MAX_VALUE` is not a delay anyone waits out — the
    /// `min(music.maxDelay())` on the next tick immediately pulls it back. It is
    /// a "do not start anything else" marker, and the cap is what makes it safe.
    pub fn start_playing(&mut self, music: &Music) {
        self.current = Some(music.sound.clone());
        self.fade = MusicFade::default();
        self.fade.start();
        self.next_song_delay = i32::MAX;
    }

    /// `stopPlaying()` — and note the trailing `+ 100`.
    ///
    /// The next delay is the frequency's answer **plus five seconds**, which is
    /// the one place `STARTING_DELAY` is used other than the initial value.
    pub fn stop_playing(&mut self, situational: Option<&Music>) {
        self.current = None;
        self.fade = MusicFade::default();
        self.next_song_delay = self
            .frequency
            .next_song_delay(situational, &mut self.random)
            .saturating_add(STARTING_DELAY);
    }

    /// `setMinutesBetweenSongs` — the options slider moved.
    pub fn set_frequency(&mut self, frequency: MusicFrequency, situational: Option<&Music>) {
        self.frequency = frequency;
        self.next_song_delay = self.frequency.next_song_delay(situational, &mut self.random);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn started(gain: f32) -> MusicFade {
        let mut f = MusicFade::default();
        f.start();
        f.gain = gain;
        f
    }

    #[test]
    fn the_default_is_full_gain_and_not_playing() {
        // `currentGain = 1.0F` at construction, so the first track starts at
        // volume rather than fading in from silence.
        let f = MusicFade::default();
        assert_eq!(f.gain(), 1.0);
        assert!(!f.is_playing());
        // …and with no music there is nothing to publish.
        let mut f = f;
        assert_eq!(f.tick(0.5), None);
    }

    /// **Fading up, the step IS the current gain**, clamped — not a constant.
    #[test]
    fn the_upward_step_is_the_gain_itself() {
        // Below the lower clamp: the step is the floor, 0.0005.
        let mut f = started(0.0001);
        f.fade(1.0);
        assert!((f.gain() - 0.0006).abs() < 1e-9, "got {}", f.gain());

        // In the band: the step is the gain, so it doubles.
        let mut f = started(0.002);
        f.fade(1.0);
        assert!((f.gain() - 0.004).abs() < 1e-9, "got {}", f.gain());

        // Above the upper clamp: the step saturates at 0.005 and stops
        // accelerating — a constant-step implementation agrees only here.
        let mut f = started(0.5);
        f.fade(1.0);
        assert!((f.gain() - 0.505).abs() < 1e-6, "got {}", f.gain());
    }

    /// A rise never overshoots its target.
    #[test]
    fn a_rise_lands_exactly_on_the_target() {
        let mut f = started(0.999);
        assert!(f.fade(1.0));
        assert_eq!(f.gain(), 1.0, "clamped to the target, not past it");
    }

    /// **Fading down is a blend, and a different curve entirely.**
    #[test]
    fn the_downward_ramp_is_an_exponential_blend() {
        let mut f = started(1.0);
        f.fade(0.0);
        // 0.03 * 0 + 0.97 * 1.0
        assert!((f.gain() - 0.97).abs() < 1e-6, "got {}", f.gain());
        f.fade(0.0);
        assert!((f.gain() - 0.9409).abs() < 1e-5, "got {}", f.gain());
        // Not the upward rule reflected: that would step by 0.005 here.
        assert!(f.gain() < 0.99, "a symmetric ramp would still be at 0.99");
    }

    /// **The second disjunct on the way down is UNREACHABLE**, and vanilla has
    /// it anyway.
    ///
    /// The else-branch is entered only when `currentGain > volume`, and the
    /// blend is `volume + 0.97 * (currentGain - volume)` rearranged — for a
    /// positive difference that is always strictly greater than `volume`, so
    /// `currentGain < volume` cannot hold after it. Underflow does not open a
    /// path either: the smallest the term can become is zero, which lands
    /// exactly on `volume` and is caught by the first disjunct.
    ///
    /// Found by a mutation surviving. It is an equivalent mutant rather than a
    /// weak fixture, which is worth the distinction — the code is dead, so no
    /// witness could kill it, and the battery records it as an expected
    /// survivor with this proof rather than leaving it looking untested. Kept
    /// because it is vanilla's, and because a future change to the blend
    /// (a weight above 1, say) would make it live again.
    #[test]
    fn the_downward_overshoot_disjunct_cannot_fire() {
        // Sampled rather than argued: every pair with gain > volume, across the
        // whole range and down to the denormal end.
        for &volume in &[0.0f32, 1e-6, 0.001, 0.1, 0.5, 0.9, 1.0] {
            for &delta in &[1e-9f32, 1e-7, 1e-4, 0.01, 0.3, 1.0] {
                let gain = volume + delta;
                if gain <= volume {
                    continue; // delta underflowed; not this branch
                }
                let blended = 0.03 * volume + 0.97 * gain;
                assert!(
                    blended >= volume,
                    "volume {volume} gain {gain} blended to {blended}, below the target"
                );
            }
        }
    }

    /// The blend closes the last gap rather than approaching forever.
    #[test]
    fn a_fall_snaps_to_the_target_inside_the_epsilon() {
        let mut f = started(0.50005);
        assert!(f.fade(0.5));
        assert_eq!(f.gain(), 0.5, "within 1e-4, so it lands exactly");
    }

    /// **The floor stops the track; it does not clamp to zero.**
    ///
    /// A client that clamped instead would leave a silent track holding a
    /// streaming channel for as long as it ran — one of only two to eight of
    /// them, so it would starve the pool rather than merely be quiet.
    #[test]
    fn fading_to_silence_stops_the_music() {
        let mut f = started(0.0002);
        // 0.03*0 + 0.97*0.0002 = 0.000194, still above the 1e-4 floor. The
        // first cut of this test asserted the NEXT tick crossed it; the blend
        // only multiplies by 0.97, so it takes about twenty-two. Counting them
        // by hand was the error — the property is that it terminates, so the
        // witness loops and bounds it rather than predicting the tick.
        assert!(f.fade(0.0), "not on the first tick");
        assert!(f.is_playing());
        let mut ticks = 1;
        while f.fade(0.0) {
            ticks += 1;
            assert!(ticks < 1000, "the fade never reached the floor");
        }
        assert!(!f.is_playing(), "stopPlaying(), not a clamp");
        // A 0.97 blend from 0.000194 needs ln(1e-4/1.94e-4)/ln(0.97) ~= 22, so
        // a bound either side catches both a wrong ratio and a wrong floor.
        assert!((15..40).contains(&ticks), "took {ticks} ticks");
        // Once stopped there is nothing to publish.
        assert_eq!(f.tick(0.0), None);
    }

    /// An equal gain publishes without running the ramp, which is what makes
    /// the fade terminate rather than creep.
    #[test]
    fn an_equal_gain_is_a_no_op() {
        let mut f = started(0.42);
        assert_eq!(f.tick(0.42), Some(0.42));
        assert_eq!(f.gain(), 0.42, "untouched");
    }

    /// A full fade in reaches its target and stays there.
    #[test]
    fn a_rise_from_silence_terminates() {
        let mut f = started(0.001);
        let mut ticks = 0;
        while f.tick(1.0) != Some(1.0) && ticks < 100_000 {
            ticks += 1;
        }
        assert!(ticks < 100_000, "the ramp never arrived");
        assert_eq!(f.gain(), 1.0);
        // The saturating step means this takes at least 1/0.005 = 200 ticks,
        // which is the number a constant 0.005 step would also give — the
        // acceleration only shows below 0.005, so the count alone cannot
        // witness it and `the_upward_step_is_the_gain_itself` does.
        assert!(ticks > 150, "only {ticks} ticks; the step is too large");
    }
    // ── the selection and the timers (M145) ───────────────────────────────

    use rewo_world::music::{musics, Music};

    fn mgr() -> MusicManager {
        MusicManager::new(1234, MusicFrequency::Default)
    }

    /// Drive `n` ticks at full volume with nothing playing, returning the tick
    /// on which a track started.
    fn ticks_until_start(m: &mut MusicManager, music: &Music, limit: i32) -> Option<i32> {
        for t in 1..=limit {
            if m.tick(1.0, Some(music), false, false).start.is_some() {
                return Some(t);
            }
        }
        None
    }

    /// **The first track waits `STARTING_DELAY` ticks, and the pre-decrement
    /// makes that exact.**
    ///
    /// `--nextSongDelay <= 0` decrements *before* the test, so a delay of 100
    /// starts on the 100th tick and not the 101st. Off by one either way and
    /// the whole timer ladder shifts.
    #[test]
    fn the_first_track_starts_after_the_starting_delay() {
        let mut m = mgr();
        assert_eq!(m.next_song_delay(), STARTING_DELAY);
        let game = musics::game();
        assert_eq!(ticks_until_start(&mut m, &game, 200), Some(100));
        assert_eq!(m.current(), Some("minecraft:music.game"));
    }

    /// `startPlaying` parks the delay at `MAX`, and the very next tick pulls it
    /// back to the track's own `maxDelay`.
    ///
    /// The cap lives **outside** the `currentMusic != null` block, which is what
    /// makes the park safe: a reader who moved it inside would leave the delay
    /// at `i32::MAX` and the next song would never come.
    #[test]
    fn the_parked_delay_is_pulled_back_to_the_tracks_own_window() {
        let mut m = mgr();
        let game = musics::game();
        ticks_until_start(&mut m, &game, 200).expect("started");
        assert_eq!(m.next_song_delay(), i32::MAX, "parked");
        m.tick(1.0, Some(&game), true, false);
        assert_eq!(m.next_song_delay(), game.max_delay, "capped to 24000");
    }

    /// **A replacing track stops the current one and clears it on the SAME
    /// tick**, drawing twice.
    ///
    /// Vanilla's `stop()` is synchronous, so the `!isActive` branch four lines
    /// down also fires. A model that waited for the caller to apply the stop
    /// would take two ticks and skip the second draw.
    #[test]
    fn a_replacing_track_stops_and_clears_in_one_tick() {
        let mut m = mgr();
        let game = musics::game();
        ticks_until_start(&mut m, &game, 200).expect("started");
        assert_eq!(m.current(), Some("minecraft:music.game"));

        // The menu replaces, and is a different track.
        let menu = musics::menu();
        let out = m.tick(1.0, Some(&menu), true, false);
        assert!(out.stop_current, "the current track is stopped");
        assert_eq!(m.current(), None, "and cleared in the same pass");
        // The delay is now inside the menu's own window rather than the game's.
        assert!(
            m.next_song_delay() <= menu.max_delay,
            "delay {} is not inside the menu's window",
            m.next_song_delay()
        );
    }

    /// **A track that replaces does not replace ITSELF.**
    ///
    /// `canReplace` compares identifiers, and without that the menu music —
    /// which sets `replaceCurrentMusic` — would stop and restart every tick.
    #[test]
    fn a_replacing_track_does_not_restart_itself() {
        let mut m = MusicManager::new(99, MusicFrequency::Default);
        let menu = musics::menu();
        ticks_until_start(&mut m, &menu, 200).expect("started");
        for _ in 0..500 {
            let out = m.tick(1.0, Some(&menu), true, false);
            assert!(!out.stop_current, "the menu music stopped itself");
            assert!(out.start.is_none());
        }
        assert_eq!(m.current(), Some("minecraft:music.menu"));
    }

    /// Ordinary game music does **not** replace, so walking into a biome with
    /// its own track leaves the one you are hearing alone.
    #[test]
    fn non_replacing_music_leaves_a_playing_track_alone() {
        let mut m = mgr();
        let game = musics::game();
        ticks_until_start(&mut m, &game, 200).expect("started");
        let jungle = rewo_world::music::musics::create_game_music("minecraft:music.overworld.jungle");
        for _ in 0..200 {
            let out = m.tick(1.0, Some(&jungle), true, false);
            assert!(!out.stop_current);
        }
        assert_eq!(m.current(), Some("minecraft:music.game"), "still the first track");
    }

    /// With nothing on offer the delay is held at at least five seconds, so
    /// walking back into a musical place does not start a track instantly.
    #[test]
    fn silence_holds_the_delay_at_the_starting_value() {
        let mut m = mgr();
        let game = musics::game();
        // Burn most of the initial delay.
        for _ in 0..90 {
            m.tick(1.0, Some(&game), false, false);
        }
        assert_eq!(m.next_song_delay(), 10);
        // Now nothing is on offer.
        m.tick(1.0, None, false, false);
        assert_eq!(m.next_song_delay(), STARTING_DELAY, "raised back to 100");
        // …and it is a max, not an assignment: a larger delay survives.
        let mut n = mgr();
        n.stop_playing(Some(&game));
        let big = n.next_song_delay();
        assert!(big > STARTING_DELAY);
        n.tick(1.0, None, false, false);
        assert_eq!(n.next_song_delay(), big, "max(), not =");
    }

    /// The loading screen suppresses the start but **not** the rest of the tick.
    #[test]
    fn the_loading_screen_suppresses_the_start_and_not_the_delay() {
        let mut m = mgr();
        let game = musics::game();
        for _ in 0..500 {
            let out = m.tick(1.0, Some(&game), false, true);
            assert!(out.start.is_none(), "nothing starts on the loading screen");
        }
        assert_eq!(m.current(), None);
        // The delay was NOT decremented while suppressed — the decrement lives
        // inside the same guard — so the moment the screen goes away the full
        // wait is still ahead.
        assert_eq!(m.next_song_delay(), STARTING_DELAY);
        assert_eq!(ticks_until_start(&mut m, &game, 200), Some(100));
    }

    /// **`CONSTANT` with no music returns 0, not 100** — the null check comes
    /// first, and its answer is `maxFrequency`, which `CONSTANT` stores as 0.
    ///
    /// Reordering the two reads more natural and is wrong: it would make
    /// `stopPlaying()` on a silent screen queue the next track five seconds out
    /// rather than immediately.
    #[test]
    fn the_frequency_null_check_comes_before_the_constant_case() {
        let mut r = LegacyRandom::new(7);
        assert_eq!(MusicFrequency::Constant.next_song_delay(None, &mut r), 0);
        assert_eq!(
            MusicFrequency::Constant.next_song_delay(Some(&musics::game()), &mut r),
            100,
            "with music it is the flat 100"
        );
        assert_eq!(
            MusicFrequency::Default.next_song_delay(None, &mut r),
            24_000,
            "and DEFAULT's own maxFrequency is twenty minutes"
        );
        assert_eq!(MusicFrequency::Frequent.next_song_delay(None, &mut r), 12_000);
    }

    /// The slider clamps the track's window from above, and a clamp that
    /// collapses it **skips the draw**.
    #[test]
    fn the_frequency_clamps_the_window_and_a_collapsed_window_does_not_draw() {
        // FREQUENT is 12000, and game music is 12000..24000 — so both bounds
        // clamp to 12000, min >= max, and `Mth.nextInt` returns min without
        // touching the generator.
        let mut r = LegacyRandom::new(5);
        let before = r.next_int(1_000);
        let mut r2 = LegacyRandom::new(5);
        let _ = r2.next_int(1_000);
        let d = MusicFrequency::Frequent.next_song_delay(Some(&musics::game()), &mut r2);
        assert_eq!(d, 12_000, "the collapsed window is its own answer");
        // The generator is where it was: the next draw matches a generator that
        // never saw the call at all.
        let mut r3 = LegacyRandom::new(5);
        let _ = r3.next_int(1_000);
        assert_eq!(r2.next_int(1_000), r3.next_int(1_000), "no draw was taken");
        let _ = before;

        // DEFAULT leaves the window open, so it does draw and lands inside it.
        let mut r4 = LegacyRandom::new(5);
        let d = MusicFrequency::Default.next_song_delay(Some(&musics::game()), &mut r4);
        assert!((12_000..=24_000).contains(&d), "{d} outside the window");
    }

    /// `Mth.nextInt` is inclusive at both ends.
    #[test]
    fn mth_next_int_is_inclusive_and_degenerate_at_the_ends() {
        let mut r = LegacyRandom::new(3);
        assert_eq!(mth_next_int(&mut r, 5, 5), 5);
        assert_eq!(mth_next_int(&mut r, 9, 2), 9, "min >= max returns min");
        let mut seen = [false; 3];
        for _ in 0..200 {
            let v = mth_next_int(&mut r, 1, 3);
            assert!((1..=3).contains(&v), "{v} outside [1,3]");
            seen[(v - 1) as usize] = true;
        }
        assert!(seen.iter().all(|s| *s), "both ends must be reachable");
    }

    /// `stopPlaying` adds five seconds on top of the frequency's answer.
    #[test]
    fn stopping_adds_the_starting_delay_on_top() {
        let mut m = MusicManager::new(11, MusicFrequency::Constant);
        let game = musics::game();
        m.start_playing(&game);
        m.stop_playing(Some(&game));
        assert_eq!(m.current(), None);
        // CONSTANT with music is a flat 100, plus the trailing 100.
        assert_eq!(m.next_song_delay(), 200);
    }

    /// A fade that reaches the floor stops the track and reports it.
    #[test]
    fn a_fade_to_silence_stops_the_track() {
        let mut m = mgr();
        let game = musics::game();
        ticks_until_start(&mut m, &game, 200).expect("started");
        let mut stopped = false;
        for _ in 0..100_000 {
            let out = m.tick(0.0, Some(&game), true, false);
            if out.stop_current {
                stopped = true;
                break;
            }
        }
        assert!(stopped, "fading to zero never stopped the track");
        assert_eq!(m.current(), None);
    }

    /// The whole ladder: a track starts, ends, and the next one is scheduled
    /// inside the frequency's window rather than immediately.
    #[test]
    fn a_finished_track_schedules_the_next_one_inside_the_window() {
        let mut m = mgr();
        let game = musics::game();
        ticks_until_start(&mut m, &game, 200).expect("started");
        // The engine reports the sound has ended.
        m.tick(1.0, Some(&game), false, false);
        assert_eq!(m.current(), None);
        let d = m.next_song_delay();
        assert!(
            (12_000..=24_000).contains(&(d + 1)),
            "next delay {d} is outside game music's own window"
        );
    }
}
