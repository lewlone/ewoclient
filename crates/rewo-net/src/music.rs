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
}
