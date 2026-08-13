//! The mixer — voices to interleaved stereo, with no device and no thread.
//!
//! **Caller-driven, exactly like `alcRenderSamplesSOFT`.** Nothing here owns a
//! clock, spawns a thread or opens anything; a caller hands it a slice and it
//! fills it. That is what lets the same `Mixer` run under a real device and
//! under a gate that renders to memory — and it is why the witnesses can read
//! assertions **out of the output** rather than recomputing gain in the test,
//! which is M88's r20 lesson (a witness reading a value that merely *implies*
//! the render is a proxy that looks more rigorous than it is).
//!
//! ## What is transcription and what is approximation
//!
//! This module is unusual in Rewo: the thing it replaces is not in the
//! decompile. `Channel.java` sets seven source properties and OpenAL Soft does
//! the arithmetic, so:
//!
//! * **The attenuation curve is transcription** — of the OpenAL 1.1
//!   specification rather than of Minecraft. `openal::linear_gain` is
//!   `1 - distance / max`, from the three properties `Channel.linearAttenuation`
//!   writes, and it is exact.
//! * **The pan law is an approximation, and a stated one.** OpenAL Soft's
//!   panning is in the DLL, not in any source Rewo can read. This uses equal
//!   power, which is the conventional choice and is *not* claimed to match.
//! * **The resampler is an approximation.** Linear interpolation against
//!   OpenAL Soft's higher-order filter. Audible as slightly more aliasing on
//!   large pitch shifts; inaudible at pitch 1.
//! * **HRTF is not implemented at all**, so `directionalAudio` is unsupported.
//!
//! M139's loopback oracle is what turns "approximation" into a measured
//! divergence in dB. Until then these are declarations, and the gate grades
//! Rewo against them rather than against vanilla.

use crate::buffers::Pcm;
use rewo_net::sound_engine::{openal, ChannelId, ListenerTransform};
use std::sync::Arc;

/// One sounding voice — a channel, in OpenAL's sense.
#[derive(Clone)]
pub struct Voice {
    pub pcm: Arc<Pcm>,
    /// Read position in **frames**, fractional because pitch and rate
    /// conversion both land between samples.
    pub cursor: f64,
    /// `Channel.setVolume` — the engine's already-computed gain
    /// (`calculateVolume`: instance x category x master, all clamped).
    pub gain: f32,
    /// `Channel.setPitch`, clamped 0.5..2.0 by `SoundEngine.calculatePitch`.
    pub pitch: f32,
    /// `Channel.setSelfPosition`.
    pub position: [f32; 3],
    /// `AL_SOURCE_RELATIVE` — `Channel.setRelative`. When set, `position` is
    /// relative to the listener rather than to the world, which is how a UI
    /// sound stays put while the player walks.
    pub relative: bool,
    /// `Channel.linearAttenuation`'s max distance, or `None` for
    /// `disableAttenuation`. `None` is not "infinite range": it is full gain
    /// everywhere, which is what a music track or a UI click wants.
    pub max_distance: Option<f32>,
    pub looping: bool,
    /// Buffers waiting behind [`Self::pcm`], oldest first — an AL source's
    /// queued buffers (M144). **Empty for every static voice**, which is what
    /// makes the streaming path an addition rather than a change.
    pub queue: std::collections::VecDeque<Arc<Pcm>>,
    /// This voice is fed by a stream rather than holding its whole sound.
    ///
    /// The one thing it changes in the render is what happens when the buffers
    /// run out: a static voice has **finished**, a streaming one has
    /// **underrun** and must stay alive and silent until more arrives. Treating
    /// an underrun as the end would kill a music track on the first hitch.
    pub streaming: bool,
    /// Buffers fully played and dropped. **A diagnostic for witnesses, and the
    /// producer deliberately does not read it** — that would be a feedback
    /// channel from the audio callback, which M143 avoided on purpose by
    /// modelling consumption from the tick clock instead.
    pub consumed_buffers: u64,
    /// Set once the source runs off the end of a non-looping buffer.
    pub finished: bool,
    /// `Channel.play()` has been called.
    ///
    /// **Vanilla's order is properties, then attach, then play**
    /// (`SoundEngine.java:417-434`), and `alSourcePlay` before a buffer is
    /// attached is a no-op — so a voice that exists but has not been played must
    /// be silent, or the eight-call sequence would start sounding halfway
    /// through itself with whatever properties had arrived so far.
    pub playing: bool,
    /// `Channel.pause()` / `unpause()`. Distinct from `finished`: a paused voice
    /// keeps its cursor and resumes where it stopped.
    pub paused: bool,
}

impl Voice {
    pub fn new(pcm: Arc<Pcm>) -> Voice {
        Voice {
            pcm,
            cursor: 0.0,
            gain: 1.0,
            pitch: 1.0,
            position: [0.0; 3],
            relative: false,
            max_distance: None,
            looping: false,
            queue: std::collections::VecDeque::new(),
            streaming: false,
            consumed_buffers: 0,
            finished: false,
            // **True here and false on the command path**, which is not an
            // inconsistency. A `Voice` built directly is a caller saying "play
            // this", and every witness in this file does exactly that; a voice
            // the ring creates is mid-sequence and must wait for its `Play`.
            playing: true,
            paused: false,
        }
    }
}

/// Stereo output at a fixed rate, fed by any number of voices.
pub struct Mixer {
    /// The device's output rate. Sources are resampled into it, and the store
    /// is genuinely mixed-rate (44100 and 48000 inside one event family), so
    /// this conversion is on the hot path for most sounds rather than an edge.
    pub out_rate: u32,
    pub listener: ListenerTransform,
    /// Keyed by channel, and a `Vec` rather than a map so the mix order is the
    /// order voices arrived. Float addition is not associative, so a `HashMap`
    /// would make the output depend on hashing and two identical scenes could
    /// differ in the last bits.
    voices: Vec<(ChannelId, Voice)>,
    /// Commands that arrived for a channel the mixer has never heard of, or that
    /// the callback cannot honour. Counted rather than logged, because the
    /// callback must not take a lock or a syscall.
    pub ignored: u64,
}

impl Mixer {
    pub fn new(out_rate: u32) -> Mixer {
        Mixer {
            out_rate,
            listener: ListenerTransform::INITIAL,
            voices: Vec::new(),
            ignored: 0,
        }
    }

    /// Add a voice directly, outside the command path — what the witnesses use.
    pub fn push(&mut self, v: Voice) {
        let id = self.voices.len() as ChannelId;
        self.voices.push((id, v));
    }

    pub fn voice_count(&self) -> usize {
        self.voices.len()
    }

    pub fn voice(&self, id: ChannelId) -> Option<&Voice> {
        self.voices.iter().find(|(k, _)| *k == id).map(|(_, v)| v)
    }

    /// Apply one [`crate::device::Command`] — the callback's whole vocabulary.
    ///
    /// **`AttachStaticBuffer` and `AttachBufferStream` are counted and
    /// ignored**, and that is the design rather than a gap: they carry an asset
    /// *key*, and resolving one means a store lookup and an ogg decode, neither
    /// of which may happen on an audio callback. The producer resolves the key
    /// and sends [`crate::device::Command::Attach`] with the PCM already in
    /// hand. Seeing a path-carrying attach here means the producer skipped that
    /// step, so it is counted where a silent ignore would hide it.
    pub fn apply(&mut self, cmd: &crate::device::Command) {
        use crate::device::Command;
        use rewo_net::sound_engine::ChannelCall as C;
        match cmd {
            Command::Listener(t) => self.listener = *t,
            Command::Attach(id, pcm) => {
                let v = self.ensure(*id);
                v.pcm = Arc::clone(pcm);
                v.cursor = 0.0;
                v.finished = false;
            }
            Command::Queue(id, pcm) => {
                let v = self.ensure(*id);
                v.streaming = true;
                if v.pcm.samples.is_empty() {
                    // The head of the stream, or the first chunk after an
                    // underrun. Promoted rather than queued so the caller does
                    // not have to know which it is.
                    v.pcm = Arc::clone(pcm);
                    v.cursor = 0.0;
                    v.finished = false;
                } else {
                    v.queue.push_back(Arc::clone(pcm));
                }
            }
            Command::Channel(id, call) => match call {
                C::Stop => self.voices.retain(|(k, _)| k != id),
                C::AttachStaticBuffer(_) | C::AttachBufferStream(_, _) => self.ignored += 1,
                _ => {
                    let v = self.ensure(*id);
                    match call {
                        C::SetPitch(p) => v.pitch = *p,
                        C::SetVolume(g) => v.gain = *g,
                        C::LinearAttenuation(d) => v.max_distance = Some(*d),
                        C::DisableAttenuation => v.max_distance = None,
                        C::SetLooping(l) => v.looping = *l,
                        C::SetRelative(r) => v.relative = *r,
                        C::SetSelfPosition(x, y, z) => {
                            v.position = [*x as f32, *y as f32, *z as f32]
                        }
                        C::Play => {
                            v.playing = true;
                            v.paused = false;
                        }
                        C::Pause => v.paused = true,
                        C::Unpause => v.paused = false,
                        // Handled above; listed so a new variant fails the build
                        // rather than being silently dropped.
                        C::Stop | C::AttachStaticBuffer(_) | C::AttachBufferStream(_, _) => {}
                    }
                }
            },
        }
    }

    /// The voice for a channel, created silent if it is new.
    ///
    /// **Silent, because the properties arrive before the `Play`.** A voice that
    /// started sounding on its first `SetPitch` would play the first few
    /// milliseconds at whatever position and volume had not yet been set.
    fn ensure(&mut self, id: ChannelId) -> &mut Voice {
        if let Some(i) = self.voices.iter().position(|(k, _)| *k == id) {
            return &mut self.voices[i].1;
        }
        let mut v = Voice::new(Arc::new(Pcm {
            samples: Vec::new(),
            channels: 1,
            sample_rate: 44100,
        }));
        v.playing = false;
        self.voices.push((id, v));
        let n = self.voices.len();
        &mut self.voices[n - 1].1
    }

    /// Drop every finished voice. Separate from [`Self::render`] so a caller can
    /// decide when a channel is reclaimable — vanilla's own reclaim is on a
    /// 20-tick grace period, not on the sound ending.
    pub fn retire_finished(&mut self) {
        self.voices.retain(|(_, v)| !v.finished);
    }

    /// Fill `out` with interleaved stereo. `out.len()` must be even.
    ///
    /// **Additive, then clamped once at the end.** Accumulating in f32 gives
    /// plenty of headroom for a dense scene, and the single clamp is a hard
    /// limiter of last resort — the look-ahead limiter that keeps dense scenes
    /// from ever reaching it is the device's, and matching OpenAL Soft's own
    /// curve is explicitly out of scope (its parameters live in the DLL and are
    /// set nowhere in Java).
    pub fn render(&mut self, out: &mut [f32]) {
        for s in out.iter_mut() {
            *s = 0.0;
        }
        let frames = out.len() / 2;
        let listener_pos = [
            self.listener.position[0] as f32,
            self.listener.position[1] as f32,
            self.listener.position[2] as f32,
        ];
        let right = self.listener.right();
        let right = [right[0] as f32, right[1] as f32, right[2] as f32];

        for (_, v) in self.voices.iter_mut() {
            if v.finished || v.pcm.samples.is_empty() || !v.playing || v.paused {
                continue;
            }
            let channels = v.pcm.channels.max(1) as usize;
            let src_frames = v.pcm.samples.len() / channels;
            // Frames of source per frame of output: the rate conversion and the
            // pitch multiply into one step, because `AL_PITCH` *is* a playback
            // rate multiplier rather than a separate effect.
            let step = (v.pcm.sample_rate as f64 / self.out_rate as f64) * v.pitch as f64;

            // Distance and direction are per-BLOCK, not per-sample: vanilla
            // pushes a source's position once per call and lets the driver hold
            // it, so a moving sound's gain steps at the block rate there too.
            let rel = if v.relative {
                v.position
            } else {
                [
                    v.position[0] - listener_pos[0],
                    v.position[1] - listener_pos[1],
                    v.position[2] - listener_pos[2],
                ]
            };
            let distance = (rel[0] * rel[0] + rel[1] * rel[1] + rel[2] * rel[2]).sqrt();
            let attenuation = match v.max_distance {
                Some(max) => openal::linear_gain(distance, max),
                None => 1.0,
            };
            let gain = v.gain * attenuation;

            // Equal-power pan from the component of the source direction along
            // the listener's right vector. `right()` is forward x up, so a
            // rolled listener rolls the image with it — which is the whole
            // reason the up vector is carried rather than assumed.
            let pan = if distance > 1e-6 {
                ((rel[0] * right[0] + rel[1] * right[1] + rel[2] * right[2]) / distance)
                    .clamp(-1.0, 1.0)
            } else {
                // At the listener's own position there is no direction to pan
                // along, and centring is the only defensible answer.
                0.0
            };
            let (l_gain, r_gain) = pan_gains(pan, channels);

            // Mutable, because a streaming voice changes buffer part-way
            // through a block and the next one is a different length. Rate and
            // channel count cannot change within one Vorbis stream, but they
            // are re-read on the swap rather than assumed — the cost is a
            // divide per buffer, about once a second.
            let (mut channels, mut src_frames, mut step, mut l_gain, mut r_gain) =
                (channels, src_frames, step, l_gain, r_gain);

            for f in 0..frames {
                if v.finished {
                    break;
                }
                // An underrun: nothing to read until the producer queues more.
                // Silence rather than a stall, and rather than death.
                if v.pcm.samples.is_empty() {
                    continue;
                }
                let (ls, rs) = sample_at(&v.pcm, v.cursor, channels);
                out[f * 2] += ls * gain * l_gain;
                out[f * 2 + 1] += rs * gain * r_gain;
                v.cursor += step;
                // **A stated approximation, once per buffer.** The interpolator
                // reads frames `i` and `i+1` of the CURRENT buffer and clamps at
                // its end, so at a join it cannot interpolate into the buffer
                // that follows — it repeats the outgoing buffer's last sample
                // for the fractional part. With one-second buffers that is a
                // sub-sample error once a second, inaudible and not zero. The
                // fix is a one-frame carry across the join, which is more state
                // than the error justifies.
                if v.cursor >= src_frames as f64 {
                    if let Some(next) = v.queue.pop_front() {
                        // **Carry the fraction across the join.** Subtracting
                        // the old buffer's length rather than zeroing keeps the
                        // resampler's phase continuous; resetting to 0 would
                        // insert a sub-sample gap at every buffer boundary,
                        // which at one per second is an audible tick.
                        v.cursor -= src_frames as f64;
                        v.pcm = next;
                        v.consumed_buffers += 1;
                        channels = v.pcm.channels.max(1) as usize;
                        src_frames = v.pcm.samples.len() / channels;
                        step = (v.pcm.sample_rate as f64 / self.out_rate as f64) * v.pitch as f64;
                        let (l, r) = pan_gains(pan, channels);
                        l_gain = l;
                        r_gain = r;
                    } else if v.looping {
                        // Wrap rather than reset: dropping the fractional part
                        // would insert a tiny gap once per loop, which is
                        // audible as a click on a short looped bed.
                        v.cursor %= src_frames as f64;
                    } else if v.streaming {
                        // **Underrun, not the end.** The producer decides when a
                        // stream is over; a mixer that called this `finished`
                        // would kill a music track on the first hitch, and the
                        // channel would be released before the next chunk
                        // arrived.
                        v.consumed_buffers += 1;
                        v.pcm = Arc::new(Pcm {
                            samples: Vec::new(),
                            channels: v.pcm.channels,
                            sample_rate: v.pcm.sample_rate,
                        });
                        v.cursor = 0.0;
                    } else {
                        v.finished = true;
                    }
                }
            }
        }

        for s in out.iter_mut() {
            *s = s.clamp(-1.0, 1.0);
        }
    }
}

/// Equal-power pan, and OpenAL's refusal to spatialise a multi-channel buffer.
///
/// **A multi-channel buffer is not spatialised**, which is OpenAL's rule and
/// therefore vanilla's: a stereo source plays its own channels straight out.
/// `item/goat_horn/call3.ogg` is the one stereo variant of an otherwise mono
/// event, so the same event spatialises on seven rolls and not the eighth.
///
/// A free function rather than inline, because M144 has to re-evaluate it when
/// a streaming voice swaps buffers — and a second copy of the rule is how the
/// swapped case comes to disagree with the first (M89, four times over).
fn pan_gains(pan: f32, channels: usize) -> (f32, f32) {
    if channels >= 2 {
        return (1.0, 1.0);
    }
    let angle = (pan + 1.0) * std::f32::consts::FRAC_PI_4;
    (angle.cos(), angle.sin())
}

/// Linearly interpolated sample at a fractional frame position.
///
/// Returns a (left, right) pair: mono is duplicated so the caller does not have
/// to branch, stereo is taken as-is.
fn sample_at(pcm: &Pcm, cursor: f64, channels: usize) -> (f32, f32) {
    let frames = pcm.samples.len() / channels;
    if frames == 0 {
        return (0.0, 0.0);
    }
    let i = cursor.floor() as usize;
    let frac = (cursor - cursor.floor()) as f32;
    let j = if i + 1 < frames { i + 1 } else { i };
    let get = |frame: usize, ch: usize| -> f32 {
        // i16 to f32 by the full-scale divisor. `-32768 / 32768` is exactly
        // -1.0, so the range maps without an asymmetric peak.
        pcm.samples[frame * channels + ch] as f32 / 32768.0
    };
    let l = get(i, 0) * (1.0 - frac) + get(j, 0) * frac;
    let r = if channels >= 2 {
        get(i, 1) * (1.0 - frac) + get(j, 1) * frac
    } else {
        l
    };
    (l, r)
}

/// The gate's device: renders the production [`Mixer`] into memory.
///
/// **A green run through this is not evidence that anything is audible.** No
/// gate in this project opens a device; everything from a real sink through
/// format negotiation to the speakers is ungraded, and a client that mixes
/// perfectly into a stream nobody opened passes every witness here. That is
/// stated in full in `REWO_AUDIO_PLAN.md` §4, and the milestone ends with a
/// human listening pass for exactly this reason.
pub struct NullSink {
    pub rendered: Vec<f32>,
}

impl NullSink {
    pub fn new() -> NullSink {
        NullSink {
            rendered: Vec::new(),
        }
    }

    /// Pull `frames` stereo frames out of the mixer and keep them.
    pub fn pull(&mut self, mixer: &mut Mixer, frames: usize) -> &[f32] {
        let start = self.rendered.len();
        self.rendered.resize(start + frames * 2, 0.0);
        mixer.render(&mut self.rendered[start..]);
        &self.rendered[start..]
    }

    /// Peak absolute sample over everything rendered so far.
    pub fn peak(&self) -> f32 {
        self.rendered.iter().fold(0.0f32, |a, s| a.max(s.abs()))
    }
}

impl Default for NullSink {
    fn default() -> Self {
        Self::new()
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    /// A constant-valued buffer, so an output amplitude IS a gain.
    ///
    /// Deliberately not a sine: a waveform makes every assertion depend on where
    /// in the cycle the resampler landed, and this milestone is about gain and
    /// placement rather than about interpolation.
    fn dc(rate: u32, frames: usize, channels: u16) -> Arc<Pcm> {
        Arc::new(Pcm {
            samples: vec![16384; frames * channels as usize],
            channels,
            sample_rate: rate,
        })
    }

    fn render(mixer: &mut Mixer, frames: usize) -> Vec<f32> {
        let mut sink = NullSink::new();
        sink.pull(mixer, frames);
        sink.rendered
    }

    fn peak_lr(out: &[f32]) -> (f32, f32) {
        let l = out.iter().step_by(2).fold(0.0f32, |a, s| a.max(s.abs()));
        let r = out[1..].iter().step_by(2).fold(0.0f32, |a, s| a.max(s.abs()));
        (l, r)
    }

    /// A voice in FRONT of the listener, so the pan is centred and the two
    /// channels carry the same thing — which isolates distance from direction.
    fn front_voice(rate: u32, distance: f32, max: f32) -> Voice {
        let mut v = Voice::new(dc(rate, 512, 1));
        // `INITIAL` faces -Z, so straight ahead is negative Z.
        v.position = [0.0, 0.0, -distance];
        v.max_distance = Some(max);
        v.looping = true;
        v
    }

    /// **The curve is a straight line, and it reaches exactly zero.**
    ///
    /// Read out of the OUTPUT, never from `openal::linear_gain` recomputed here
    /// — a witness that recomputed the formula would agree with any formula,
    /// which is M88's r20 in miniature. The ratios below are what an inverse
    /// or inverse-square model cannot produce, and hitting *exactly* zero at
    /// `max` is the property no `1/d` family has at all.
    #[test]
    fn distance_attenuation_is_linear_and_reaches_zero() {
        let max = 16.0;
        let mut at = |d: f32| {
            let mut m = Mixer::new(44100);
            m.push(front_voice(44100, d, max));
            peak_lr(&render(&mut m, 64)).0
        };
        let (near, quarter, half, three_q, edge) = (at(0.0), at(4.0), at(8.0), at(12.0), at(16.0));

        assert!(near > 0.0);
        // 1 - d/max, so the halfway point is half as loud, and the quarter and
        // three-quarter points bracket it evenly. An inverse-distance model
        // would put the halfway point at about a sixth of this.
        assert!((half / near - 0.5).abs() < 1e-3, "half: {half} vs {near}");
        assert!((quarter / near - 0.75).abs() < 1e-3);
        assert!((three_q / near - 0.25).abs() < 1e-3);
        // **Exactly zero**, not merely small.
        assert_eq!(edge, 0.0, "at max distance the source is silent");
        // …and it stays zero past it rather than going negative, which the
        // unclamped linear model does without AL_MIN_GAIN.
        assert_eq!(at(64.0), 0.0);
    }

    /// Left, right and centre, from the output alone.
    #[test]
    fn the_stereo_image_follows_the_source() {
        let mut place = |pos: [f32; 3]| {
            let mut m = Mixer::new(44100);
            let mut v = Voice::new(dc(44100, 512, 1));
            v.position = pos;
            v.looping = true;
            m.push(v);
            peak_lr(&render(&mut m, 64))
        };
        // `INITIAL`'s right vector is forward x up = (0,0,-1) x (0,1,0) = +X.
        let (l, r) = place([8.0, 0.0, 0.0]);
        assert!(r > 0.0 && l < r * 1e-3, "hard right: l={l} r={r}");
        let (l, r) = place([-8.0, 0.0, 0.0]);
        assert!(l > 0.0 && r < l * 1e-3, "hard left: l={l} r={r}");
        let (l, r) = place([0.0, 0.0, -8.0]);
        assert!((l - r).abs() < 1e-6, "in front: l={l} r={r}");
    }

    /// **Yaw moves a source between the ears, and pitch must NOT.**
    ///
    /// Worked out rather than assumed: `right = forward x up` comes to
    /// `(-cos yaw, 0, -sin yaw)` for every pitch, because pitching rotates about
    /// the right axis and cannot move it. So the first half of this is the
    /// directional claim, and the second is the one that catches a pinned `up`.
    ///
    /// Note yaw 0 puts a `+X` source on the LEFT, which is not a slip: a camera
    /// at yaw 0 faces `+Z` while `ListenerTransform::INITIAL` faces `-Z`, and
    /// they differ by the half turn in `rotationYXZ(pi - yaw, ...)`. M138a's own
    /// witness got this wrong first; so did this one.
    #[test]
    fn yaw_moves_a_source_between_the_ears() {
        let mut aim = |yaw: f32, pitch: f32| {
            let mut m = Mixer::new(44100);
            let (forward, up) = rewo_net::sound_engine::listener_basis(yaw, pitch);
            m.listener = ListenerTransform {
                position: [0.0; 3],
                forward,
                up,
            };
            let mut v = Voice::new(dc(44100, 512, 1));
            v.position = [8.0, 0.0, 0.0];
            v.looping = true;
            m.push(v);
            peak_lr(&render(&mut m, 64))
        };
        let (l0, r0) = aim(0.0, 0.0);
        assert!(l0 > r0, "facing +Z, a +X source is on the left");
        let (l180, r180) = aim(180.0, 0.0);
        assert!(r180 > l180, "turn around and it crosses over");

        // **Pitch must not move it**, because the right axis is what pitch turns
        // about. A basis that pitched the right vector too would swing the image
        // as the player looked up and down.
        let (l90, r90) = aim(0.0, 90.0);
        assert!((l90 - l0).abs() < 1e-6 && (r90 - r0).abs() < 1e-6);
    }

    /// **Looking straight down, the image must not collapse to centre** — which
    /// is the one thing a listener with a pinned `up` cannot do.
    ///
    /// With the real basis, `right` is unit length at every pitch. Pin `up` to
    /// `(0,1,0)` and at pitch 90 the forward vector is `(0,-1,0)`, so
    /// `forward x up` is the ZERO vector: the pan collapses and every source
    /// centres. That is the failure this catches, and nothing else in this file
    /// would notice it, because `right` is pitch-invariant everywhere else.
    #[test]
    fn a_downward_listener_still_has_a_stereo_image() {
        let mut m = Mixer::new(44100);
        let (forward, _) = rewo_net::sound_engine::listener_basis(0.0, 90.0);
        // The real up for this angle; a pinned (0,1,0) is what the test excludes.
        let (_, up) = rewo_net::sound_engine::listener_basis(0.0, 90.0);
        m.listener = ListenerTransform {
            position: [0.0; 3],
            forward,
            up,
        };
        let mut v = Voice::new(dc(44100, 512, 1));
        v.position = [8.0, 0.0, 0.0];
        v.looping = true;
        m.push(v);
        let (l, r) = peak_lr(&render(&mut m, 64));
        assert!(l > 0.0);
        assert!(r < l * 1e-3, "still hard left, not centred: l={l} r={r}");

        // And the degenerate case really would centre it, so the assertion above
        // is about the basis rather than about the mixer panning at all.
        let mut degenerate = Mixer::new(44100);
        degenerate.listener = ListenerTransform {
            position: [0.0; 3],
            forward,
            up: [0.0, 1.0, 0.0],
        };
        let mut v2 = Voice::new(dc(44100, 512, 1));
        v2.position = [8.0, 0.0, 0.0];
        v2.looping = true;
        degenerate.push(v2);
        let (dl, dr) = peak_lr(&render(&mut degenerate, 64));
        assert!((dl - dr).abs() < 1e-6, "a pinned up centres everything");
    }

    /// `AL_SOURCE_RELATIVE` — a UI sound does not move when the player does.
    #[test]
    fn a_relative_source_is_unmoved_by_the_listener() {
        let mut at_listener = |pos: [f64; 3], relative: bool| {
            let mut m = Mixer::new(44100);
            m.listener = ListenerTransform {
                position: pos,
                ..ListenerTransform::INITIAL
            };
            let mut v = Voice::new(dc(44100, 512, 1));
            v.position = [0.0, 0.0, 0.0];
            v.relative = relative;
            v.max_distance = Some(16.0);
            v.looping = true;
            m.push(v);
            peak_lr(&render(&mut m, 64)).0
        };
        // Relative: at the listener's own head however far the listener walks.
        assert!(at_listener([0.0; 3], true) > 0.0);
        assert_eq!(
            at_listener([0.0; 3], true),
            at_listener([100.0, 0.0, 0.0], true),
            "a UI sound must not fade as the player walks"
        );
        // Absolute: the same source at the world origin fades out entirely.
        assert!(at_listener([0.0; 3], false) > 0.0);
        assert_eq!(at_listener([100.0, 0.0, 0.0], false), 0.0);
    }

    /// Pitch is a playback RATE, so it changes how long a sound lasts.
    #[test]
    fn pitch_changes_the_length_of_a_one_shot() {
        let sounding_frames = |pitch: f32| {
            let mut m = Mixer::new(44100);
            let mut v = Voice::new(dc(44100, 441, 1));
            v.pitch = pitch;
            v.position = [0.0, 0.0, -1.0];
            m.push(v);
            let out = render(&mut m, 2048);
            out.chunks(2).filter(|f| f[0].abs() > 1e-9).count()
        };
        let one = sounding_frames(1.0);
        assert!(one > 0);
        // Double rate, half the duration; half rate, double. The tolerance is
        // one frame, for the boundary the fractional cursor lands on.
        assert!((sounding_frames(2.0) as i64 - (one as i64 / 2)).abs() <= 1);
        assert!((sounding_frames(0.5) as i64 - (one as i64 * 2)).abs() <= 1);
    }

    /// A source at a different rate from the device is resampled, not replayed
    /// at the wrong speed. The store is genuinely mixed-rate, so this is the
    /// common case rather than an edge.
    #[test]
    fn a_48k_source_lasts_the_same_wall_time_in_a_44k_device() {
        let frames_at = |src_rate: u32, n: usize| {
            let mut m = Mixer::new(44100);
            let mut v = Voice::new(dc(src_rate, n, 1));
            v.position = [0.0, 0.0, -1.0];
            m.push(v);
            render(&mut m, 8192)
                .chunks(2)
                .filter(|f| f[0].abs() > 1e-9)
                .count()
        };
        // **50 ms, not half a second.** The first version used half-second
        // sources against an 8192-frame window, so BOTH ran off the end of the
        // buffer and reported 8192 — the counts agreed because neither had
        // finished, and a mutation dropping the rate conversion entirely
        // survived. A fixture has to be shorter than the window it is measured
        // in, or it measures the window.
        let a = frames_at(44100, 2205);
        let b = frames_at(48000, 2400);
        assert!(a < 8000 && b < 8000, "both must finish inside the window");
        assert!((a as i64 - b as i64).abs() <= 2, "{a} vs {b}");
        // Without the rate conversion these differ by about 195, so the bound
        // is not merely tight — it is the whole claim.
        assert!(a > 2000, "and the sound really played: {a}");
    }

    /// Voices end, and an ended voice is retired rather than looping silently.
    #[test]
    fn a_one_shot_finishes_and_a_loop_does_not() {
        let mut m = Mixer::new(44100);
        let mut one = Voice::new(dc(44100, 64, 1));
        one.position = [0.0, 0.0, -1.0];
        let mut loops = Voice::new(dc(44100, 64, 1));
        loops.position = [0.0, 0.0, -1.0];
        loops.looping = true;
        m.push(one);
        m.push(loops);
        assert_eq!(m.voice_count(), 2);
        let _ = render(&mut m, 4096);
        m.retire_finished();
        assert_eq!(m.voice_count(), 1, "the one-shot is gone, the loop is not");
        // …and the loop is still sounding well past its own length.
        let out = render(&mut m, 256);
        assert!(peak_lr(&out).0 > 0.0);
    }

    /// `render` OVERWRITES its slice; it does not accumulate into it.
    ///
    /// Found by a mutation surviving: every other witness here renders into a
    /// freshly-allocated buffer, which arrives zeroed, so deleting the clear at
    /// the top of `render` changed nothing any of them could see. A real device
    /// hands the same buffer back every callback, so the bug would have been
    /// unbounded accumulation into a screech on the second period.
    #[test]
    fn render_overwrites_a_dirty_buffer() {
        let mut m = Mixer::new(44100);
        let mut buf = vec![0.5f32; 64];
        m.render(&mut buf);
        assert!(buf.iter().all(|s| *s == 0.0), "an empty mixer must zero it");

        let mut v = Voice::new(dc(44100, 256, 1));
        v.position = [0.0, 0.0, -1.0];
        v.looping = true;
        m.push(v);
        let mut a = vec![0.0f32; 64];
        m.render(&mut a);
        let mut b = vec![0.9f32; 64];
        // Same voice state would differ, so rewind it rather than compare
        // across two renders of a moving cursor.
        m.retire_finished();
        let mut m2 = Mixer::new(44100);
        let mut v2 = Voice::new(dc(44100, 256, 1));
        v2.position = [0.0, 0.0, -1.0];
        v2.looping = true;
        m2.push(v2);
        m2.render(&mut b);
        assert_eq!(a, b, "a dirty buffer must not change the result");
    }

    /// No voices is silence, not garbage — the underrun case.
    #[test]
    fn an_empty_mixer_renders_exact_silence() {
        let mut m = Mixer::new(44100);
        let out = render(&mut m, 128);
        assert_eq!(out.len(), 256);
        assert!(out.iter().all(|s| *s == 0.0));
        // And a buffer is cleared rather than accumulated into: rendering twice
        // into the same sink must not double anything.
        let mut sink = NullSink::new();
        sink.pull(&mut m, 64);
        assert_eq!(sink.peak(), 0.0);
    }

    /// Many loud voices clamp rather than wrapping into noise.
    #[test]
    fn a_dense_scene_clamps_instead_of_wrapping() {
        let mut m = Mixer::new(44100);
        for _ in 0..32 {
            let mut v = Voice::new(dc(44100, 256, 1));
            v.position = [0.0, 0.0, -1.0];
            v.looping = true;
            m.push(v);
        }
        let out = render(&mut m, 128);
        assert!(out.iter().all(|s| s.abs() <= 1.0), "output left [-1, 1]");
        // …and it really is loud, so the clamp is being exercised rather than
        // the test passing on a quiet scene.
        assert!(peak_lr(&out).0 > 0.9);
    }

    /// **A stereo source is not spatialised**, which is OpenAL's rule and so
    /// vanilla's: its channels go straight out. `goat_horn/call3` is the one
    /// stereo variant of an otherwise mono event, so the same event spatialises
    /// on seven rolls of the dice and not on the eighth.
    #[test]
    fn a_stereo_source_ignores_its_position() {
        let mut place = |x: f32| {
            let mut m = Mixer::new(44100);
            let mut v = Voice::new(dc(44100, 256, 2));
            v.position = [x, 0.0, 0.0];
            v.looping = true;
            m.push(v);
            peak_lr(&render(&mut m, 64))
        };
        let (l_r, r_r) = place(8.0);
        let (l_l, r_l) = place(-8.0);
        assert!((l_r - l_l).abs() < 1e-6 && (r_r - r_l).abs() < 1e-6);
        assert!((l_r - r_r).abs() < 1e-6, "and both channels come through");
        // A mono source in the same two places does NOT agree, which is what
        // makes the assertion above about stereo rather than about the mixer
        // ignoring position generally.
        let mut mono = |x: f32| {
            let mut m = Mixer::new(44100);
            let mut v = Voice::new(dc(44100, 256, 1));
            v.position = [x, 0.0, 0.0];
            v.looping = true;
            m.push(v);
            peak_lr(&render(&mut m, 64))
        };
        assert!(mono(8.0).0 < mono(-8.0).0);
    }

    /// Gain multiplies the output, and silence at gain 0 is exact.
    #[test]
    fn the_engines_gain_reaches_the_output() {
        let with_gain = |g: f32| {
            let mut m = Mixer::new(44100);
            let mut v = Voice::new(dc(44100, 256, 1));
            v.position = [0.0, 0.0, -1.0];
            v.gain = g;
            v.looping = true;
            m.push(v);
            peak_lr(&render(&mut m, 64)).0
        };
        let full = with_gain(1.0);
        assert!((with_gain(0.5) / full - 0.5).abs() < 1e-3);
        assert_eq!(with_gain(0.0), 0.0);
    }

    // ── the command path (M138d) ──────────────────────────────────────────

    use crate::device::Command;
    use rewo_net::sound_engine::ChannelCall as C;

    fn drive(m: &mut Mixer, id: rewo_net::sound_engine::ChannelId, calls: &[C]) {
        for c in calls {
            m.apply(&Command::Channel(id, c.clone()));
        }
    }

    /// **A voice is silent until its `Play`**, which is the whole reason the
    /// eight calls are a sequence rather than a set.
    ///
    /// `alSourcePlay` before a buffer is attached is a no-op in OpenAL, so
    /// vanilla's order is properties, attach, play. A mixer that started
    /// sounding on the first `SetPitch` would play the opening milliseconds at
    /// whatever position and volume had arrived so far — audible as a click from
    /// the wrong direction, and impossible to attribute.
    #[test]
    fn a_voice_built_by_commands_is_silent_until_play() {
        let mut m = Mixer::new(44100);
        drive(&mut m, 7, &[C::SetVolume(1.0), C::SetSelfPosition(0.0, 0.0, -1.0)]);
        m.apply(&Command::Attach(7, dc(44100, 256, 1)));
        drive(&mut m, 7, &[C::SetLooping(true)]);
        assert_eq!(m.voice_count(), 1);
        assert_eq!(peak_lr(&render(&mut m, 64)).0, 0.0, "not played yet");

        drive(&mut m, 7, &[C::Play]);
        assert!(peak_lr(&render(&mut m, 64)).0 > 0.0, "now it sounds");
    }

    /// The eight-call sequence lands where it should — read off the voice.
    #[test]
    fn the_eight_calls_reach_the_voice() {
        let mut m = Mixer::new(44100);
        drive(
            &mut m,
            3,
            &[
                C::SetPitch(1.5),
                C::SetVolume(0.25),
                C::LinearAttenuation(24.0),
                C::SetLooping(true),
                C::SetSelfPosition(1.0, 2.0, 3.0),
                C::SetRelative(true),
                C::Play,
            ],
        );
        let v = m.voice(3).expect("channel 3");
        assert_eq!(v.pitch, 1.5);
        assert_eq!(v.gain, 0.25);
        assert_eq!(v.max_distance, Some(24.0));
        assert!(v.looping && v.relative && v.playing);
        assert_eq!(v.position, [1.0, 2.0, 3.0]);
        // …and `disableAttenuation` is the other arm rather than a zero.
        drive(&mut m, 3, &[C::DisableAttenuation]);
        assert_eq!(m.voice(3).unwrap().max_distance, None);
    }

    /// Pause keeps the cursor; stop takes the voice away entirely.
    #[test]
    fn pause_holds_its_place_and_stop_does_not() {
        let mut m = Mixer::new(44100);
        m.apply(&Command::Attach(1, dc(44100, 4096, 1)));
        drive(&mut m, 1, &[C::SetSelfPosition(0.0, 0.0, -1.0), C::Play]);
        let _ = render(&mut m, 64);
        let cursor = m.voice(1).unwrap().cursor;
        assert!(cursor > 0.0);

        drive(&mut m, 1, &[C::Pause]);
        assert_eq!(peak_lr(&render(&mut m, 64)).0, 0.0, "paused is silent");
        assert_eq!(m.voice(1).unwrap().cursor, cursor, "and does not advance");

        drive(&mut m, 1, &[C::Unpause]);
        assert!(peak_lr(&render(&mut m, 64)).0 > 0.0);
        assert!(m.voice(1).unwrap().cursor > cursor, "it resumed, not restarted");

        drive(&mut m, 1, &[C::Stop]);
        assert_eq!(m.voice_count(), 0, "stop removes the channel");
    }

    /// **A path-carrying attach is counted, not silently dropped.**
    ///
    /// It can only arrive if the producer skipped the decode, and the callback
    /// cannot do that work — a store lookup and an ogg decode are a syscall and
    /// a large allocation. Counting it is what makes "no sound, and no idea
    /// why" into a number someone can read.
    #[test]
    fn an_unresolved_attach_is_counted_rather_than_ignored() {
        let mut m = Mixer::new(44100);
        assert_eq!(m.ignored, 0);
        m.apply(&Command::Channel(1, C::AttachStaticBuffer("a.ogg".into())));
        m.apply(&Command::Channel(1, C::AttachBufferStream("b.ogg".into(), true)));
        assert_eq!(m.ignored, 2);
        // A resolved attach is the supported path, and it rewinds the voice.
        m.apply(&Command::Attach(1, dc(44100, 64, 1)));
        assert_eq!(m.voice(1).unwrap().cursor, 0.0);
        assert_eq!(m.ignored, 2, "and is not itself counted");
    }

    /// The listener rides the same ring, so it lands through the same path.
    #[test]
    fn a_listener_command_moves_the_ears() {
        let mut m = Mixer::new(44100);
        let (forward, up) = rewo_net::sound_engine::listener_basis(180.0, 0.0);
        m.apply(&Command::Listener(ListenerTransform {
            position: [5.0, 0.0, 0.0],
            forward,
            up,
        }));
        assert_eq!(m.listener.position, [5.0, 0.0, 0.0]);
        assert_eq!(m.listener.forward, forward);
    }

    // ── streaming voices (M144) ───────────────────────────────────────────

    /// A buffer whose sample VALUE encodes its absolute position.
    ///
    /// **A DC fixture cannot witness anything about the cursor**, because every
    /// position in it holds the same number — so a swap that reset the cursor to
    /// zero, and one that kept the previous buffer's length, both rendered
    /// identically under the first version of the test below and survived their
    /// mutations. A ramp makes position observable.
    fn ramp(rate: u32, start: i16, frames: usize) -> Arc<Pcm> {
        Arc::new(Pcm {
            samples: (0..frames).map(|i| start + i as i16).collect(),
            channels: 1,
            sample_rate: rate,
        })
    }

    /// **Queued buffers are bit-identical to one buffer of their
    /// concatenation.**
    ///
    /// The whole claim of the streaming path in one assertion: how the audio is
    /// *delivered* must not change what comes out.
    ///
    /// Three things about the fixture are load-bearing, and the first version
    /// had none of them. The samples are a **ramp**, so where the cursor is
    /// changes what comes out. The chunks are **unequal** (700, 1100, 1200), so
    /// a swap that kept the outgoing buffer's length cuts the next one short.
    /// And the rate ratio is **non-integer** (48 kHz source into a 44.1 kHz
    /// device, step 1.088…), so the cursor lands mid-sample at both joins, which
    /// is where dropping the carried fraction diverges. A DC fixture at 1:1 with
    /// equal chunks is invariant under all three.
    #[test]
    fn queued_buffers_render_identically_to_one_long_buffer() {
        let voice = |pcm: Arc<Pcm>, rest: Vec<Arc<Pcm>>| {
            let mut m = Mixer::new(44_100);
            let mut v = Voice::new(pcm);
            v.position = [0.0, 0.0, -1.0];
            v.streaming = true;
            v.queue = rest.into();
            m.push(v);
            render(&mut m, 4096)
        };
        let one = voice(ramp(48_000, 0, 3_000), vec![]);
        let three = voice(
            ramp(48_000, 0, 700),
            vec![ramp(48_000, 700, 1_100), ramp(48_000, 1_800, 1_200)],
        );
        assert_eq!(one.len(), three.len());
        assert!(peak_lr(&one).0 > 0.0, "the fixture must actually sound");

        // **Not bit-identical, and the difference is the stated approximation**
        // this file records at the swap: the interpolator clamps at the end of
        // the current buffer, so at a join it repeats the outgoing buffer's last
        // sample for the fractional part instead of reading into the next one.
        // The single-buffer render interpolates straight through.
        //
        // Measuring it is better than a DC fixture that hides it. Exactly one
        // output frame per join can differ — two samples, since the source is
        // mono and centred — and by less than one step of the ramp, which is the
        // most a sub-sample error can be worth.
        let differing: Vec<usize> = (0..one.len()).filter(|i| one[*i] != three[*i]).collect();
        let worst = differing
            .iter()
            .map(|i| (one[*i] - three[*i]).abs())
            .fold(0.0f32, f32::max);
        assert!(
            differing.len() <= 4,
            "two joins, one frame each: {} samples differ",
            differing.len()
        );
        assert!(
            worst < 1.0 / 32_768.0,
            "a sub-sample error is worth less than one LSB: {worst}"
        );
    }

    /// The buffers play in the order they were queued, not all at once and not
    /// backwards.
    ///
    /// Amplitudes differ per buffer, so the output says which one was playing
    /// when. A queue drained from the wrong end would put them in 3-2-1 order
    /// and every aggregate over the whole render would be unchanged.
    #[test]
    fn a_queue_plays_oldest_first() {
        let level = |v: i16, frames: usize| {
            Arc::new(Pcm {
                samples: vec![v; frames],
                channels: 1,
                sample_rate: 44_100,
            })
        };
        let mut m = Mixer::new(44_100);
        let mut v = Voice::new(level(4_000, 100));
        v.position = [0.0, 0.0, -1.0];
        v.streaming = true;
        v.queue = vec![level(16_000, 100), level(8_000, 100)].into();
        m.push(v);
        let out = render(&mut m, 300);

        let at = |f: usize| out[f * 2].abs();
        assert!(at(50) < at(150), "buffer 2 is the loud one");
        assert!(at(250) < at(150), "buffer 3 is quieter than 2");
        assert!(at(50) < at(250), "…and buffer 1 is the quietest of the three");
    }

    /// **An underrun is silence, and the voice survives it.**
    ///
    /// A streaming voice that runs dry has not finished — the producer decides
    /// that. A mixer that marked it finished would kill a music track on the
    /// first hitch, and `retire_finished` would drop it before the next chunk
    /// arrived.
    #[test]
    fn a_streaming_voice_underruns_into_silence_and_recovers() {
        let mut m = Mixer::new(44_100);
        m.apply(&Command::Queue(1, dc(44_100, 64, 1)));
        drive(&mut m, 1, &[C::SetSelfPosition(0.0, 0.0, -1.0), C::Play]);

        // Long enough to run past the one queued buffer several times over.
        let out = render(&mut m, 512);
        assert!(peak_lr(&out).0 > 0.0, "it played what it had");
        assert_eq!(m.voice_count(), 1);
        m.retire_finished();
        assert_eq!(m.voice_count(), 1, "an underrun must not retire the voice");

        // Dry now: exact silence rather than a repeat of the last buffer.
        let dry = render(&mut m, 256);
        assert_eq!(peak_lr(&dry).0, 0.0, "a dry stream is silent, not looping");

        // …and it picks up again when the producer catches up.
        m.apply(&Command::Queue(1, dc(44_100, 256, 1)));
        assert!(peak_lr(&render(&mut m, 128)).0 > 0.0, "it resumed");
    }

    /// A static voice is exactly as it was: no queue, not streaming, and it
    /// still finishes.
    ///
    /// The regression guard for the whole milestone — M144 is an addition, and
    /// the way it could stop being one is by changing what an empty queue means.
    #[test]
    fn a_static_voice_is_untouched_by_the_streaming_path() {
        let mut m = Mixer::new(44_100);
        let mut v = Voice::new(dc(44_100, 64, 1));
        v.position = [0.0, 0.0, -1.0];
        m.push(v);
        assert!(!m.voice(0).unwrap().streaming);
        assert!(m.voice(0).unwrap().queue.is_empty());
        let _ = render(&mut m, 4096);
        assert!(m.voice(0).unwrap().finished, "a static voice still ends");
        m.retire_finished();
        assert_eq!(m.voice_count(), 0);
    }

    /// The first `Queue` becomes the playing buffer; the rest queue behind it.
    ///
    /// So a producer does not have to know whether it is starting a stream or
    /// continuing one — which is the case that would otherwise need a flag on
    /// the command and a decision at every call site.
    #[test]
    fn the_first_queued_chunk_is_promoted_and_later_ones_are_not() {
        let mut m = Mixer::new(44_100);
        m.apply(&Command::Queue(2, dc(44_100, 128, 1)));
        let v = m.voice(2).unwrap();
        assert!(v.streaming);
        assert_eq!(v.pcm.samples.len(), 128, "promoted, not queued");
        assert!(v.queue.is_empty());

        m.apply(&Command::Queue(2, dc(44_100, 128, 1)));
        assert_eq!(m.voice(2).unwrap().queue.len(), 1, "this one waits");
        // Still silent: `Queue` is not `Play`, exactly as `Attach` is not.
        assert_eq!(peak_lr(&render(&mut m, 32)).0, 0.0);
    }

    /// `Attach` REPLACES and `Queue` APPENDS — the difference the two commands
    /// exist for.
    #[test]
    fn attach_replaces_where_queue_appends() {
        let mut m = Mixer::new(44_100);
        m.apply(&Command::Queue(3, dc(44_100, 128, 1)));
        m.apply(&Command::Queue(3, dc(44_100, 128, 1)));
        assert_eq!(m.voice(3).unwrap().queue.len(), 1);
        // An attach on the same channel is a new sound, not a continuation —
        // but it deliberately leaves the queue alone, because a producer that
        // wanted the queue gone sends `Stop`.
        m.apply(&Command::Attach(3, dc(44_100, 64, 1)));
        assert_eq!(m.voice(3).unwrap().pcm.samples.len(), 64);
        assert_eq!(m.voice(3).unwrap().cursor, 0.0, "and it rewinds");
    }

    /// Consumed buffers are counted for witnesses — and for nobody else.
    ///
    /// The producer models consumption from its own tick clock (M143), so this
    /// is deliberately not a feedback channel. It is here because "did the mixer
    /// actually walk the queue" is otherwise only visible as a sample level.
    #[test]
    fn consumed_buffers_counts_the_joins() {
        let mut m = Mixer::new(44_100);
        let mut v = Voice::new(dc(44_100, 64, 1));
        v.position = [0.0, 0.0, -1.0];
        v.streaming = true;
        v.queue = vec![dc(44_100, 64, 1), dc(44_100, 64, 1)].into();
        m.push(v);
        assert_eq!(m.voice(0).unwrap().consumed_buffers, 0);
        let _ = render(&mut m, 200);
        // Three buffers of 64 frames is 192; at 200 frames all three are done,
        // which is two joins plus the underrun that follows the last.
        assert_eq!(m.voice(0).unwrap().consumed_buffers, 3);
        assert!(m.voice(0).unwrap().queue.is_empty());
    }

    /// `disableAttenuation` is full gain everywhere, not infinite range with a
    /// curve — a music track does not fade as you walk away from wherever it
    /// was started.
    #[test]
    fn a_source_without_attenuation_does_not_fade() {
        let at = |d: f32| {
            let mut m = Mixer::new(44100);
            let mut v = Voice::new(dc(44100, 256, 1));
            v.position = [0.0, 0.0, -d];
            v.max_distance = None;
            v.looping = true;
            m.push(v);
            peak_lr(&render(&mut m, 64)).0
        };
        assert_eq!(at(1.0), at(1000.0));
        assert!(at(1000.0) > 0.0);
    }

    /// M139 — the loopback oracle's vectors, and what they say about this
    /// mixer.
    ///
    /// The module doc at the top of this file declares the pan law and the
    /// resampler to be **stated approximations**, because vanilla computes
    /// neither: `Channel.java:88-121` sets seven source properties,
    /// `Listener.java:14-15` sets two listener ones, and OpenAL Soft does all
    /// the arithmetic inside a DLL no decompile here contains. Until M139 those
    /// declarations were graded against themselves.
    ///
    /// `tools/openal_loopback_oracle/` drives **vanilla's own**
    /// `Channel`/`Listener`/`SoundBuffer` against a real OpenAL Soft through an
    /// `ALC_SOFT_loopback` device and captures the PCM. The vectors are checked
    /// in, so nothing here needs a JVM.
    ///
    /// **These tests assert divergences, not equality.** Where Rewo and OpenAL
    /// agree they say so exactly; where they do not, the assertion is a window
    /// around the *measured* number with that number in the message. A test
    /// demanding equality would fail on arrival and teach nobody anything.
    ///
    /// The windows are deliberately two-sided. Narrowing a divergence is a real
    /// improvement and it is expected to fail these — at which point the new
    /// number should be re-measured and recorded here, which is the whole point
    /// of pinning a measurement rather than a bound.
    ///
    /// **What none of this grades**: it is a comparison of two renderers'
    /// arithmetic. No test in this project opens an audio device, so nothing
    /// here is evidence that the client makes a sound. See `REWO_AUDIO_PLAN.md`
    /// s4 "What the gate does NOT assert", and the listening pass it requires.
    mod oracle {
        use super::*;
        use rewo_net::sound_engine::listener_basis;

        /// The capture. Regenerate with
        /// `pwsh tools/openal_loopback_oracle/run.ps1`.
        ///
        /// Two independent captures on the same machine are **byte-identical**,
        /// which is a property of having captured `ALC_FLOAT_SOFT`;
        /// `ALC_SHORT_SOFT` is dithered and would put noise in this file.
        const VECTORS: &str = include_str!("../../../tools/openal_loopback_oracle/vectors.tsv");

        /// Must equal `LoopbackOracle.WARMUP_FRAMES`. OpenAL ramps a voice's
        /// gain over its first mixing quantum, so its measured window starts
        /// here; Rewo has no ramp, but the window is skipped on both sides so
        /// the two are aligned rather than merely similar.
        const WARMUP_FRAMES: usize = 4096;
        /// Must equal `LoopbackOracle.MEASURE_FRAMES`.
        const MEASURE_FRAMES: usize = 8200;
        /// Must equal `LoopbackOracle.DFT_N`.
        const DFT_N: usize = 2048;
        /// Must equal `LoopbackOracle.FUND_HALFWIDTH`.
        const FUND_HALFWIDTH: i64 = 6;
        /// Must equal `LoopbackOracle.RATE`.
        const RATE: u32 = 44100;

        /// One captured experiment: the whole stimulus plus what OpenAL made of
        /// it. The Java side emits every field, so this is the only description
        /// of each experiment and there is no second transcription to drift.
        #[derive(Clone, Debug)]
        struct Row {
            id: String,
            srate: u32,
            frames: usize,
            chans: u16,
            freq_l: f64,
            amp_l: f64,
            freq_r: f64,
            amp_r: f64,
            vol: f32,
            pitch: f32,
            rel: bool,
            /// `-1` is `disableAttenuation()`.
            maxd: f32,
            src: [f64; 3],
            lyaw: f32,
            lpitch: f32,
            lpos: [f64; 3],
            voices: usize,
            /// `AL_SOURCE_RESAMPLER_SOFT` forced to Linear, which is Rewo's own
            /// algorithm.
            res_lin: bool,
            src_hash: u64,
            rms_l: f64,
            rms_r: f64,
            peak_l: f64,
            #[allow(dead_code)]
            peak_r: f64,
            /// `NaN` where the statistic is not meaningful for the row.
            dsr_db: f64,
            fund_hz: f64,
        }

        fn rows() -> Vec<Row> {
            let mut out = Vec::new();
            // `trim_start_matches('\u{feff}')` because a BOM is invisible in
            // every viewer and turns the first comment line into a data row
            // with one column. `run.ps1` writes without one; this is the belt
            // to that brace, since the writer is a shell script and shells
            // reintroduce BOMs.
            for line in VECTORS.trim_start_matches('\u{feff}').lines() {
                if line.starts_with('#') || line.trim().is_empty() {
                    continue;
                }
                let f: Vec<&str> = line.split('\t').collect();
                assert_eq!(
                    f.len(),
                    29,
                    "vectors.tsv row has {} columns, expected 29: {line}",
                    f.len()
                );
                let n = |i: usize| -> f64 { f[i].parse().unwrap() };
                out.push(Row {
                    id: f[0].to_string(),
                    srate: n(1) as u32,
                    frames: n(2) as usize,
                    chans: n(3) as u16,
                    freq_l: n(4),
                    amp_l: n(5),
                    freq_r: n(6),
                    amp_r: n(7),
                    vol: n(8) as f32,
                    pitch: n(9) as f32,
                    rel: n(10) != 0.0,
                    maxd: n(11) as f32,
                    src: [n(12), n(13), n(14)],
                    lyaw: n(15) as f32,
                    lpitch: n(16) as f32,
                    lpos: [n(17), n(18), n(19)],
                    voices: n(20) as usize,
                    res_lin: n(21) != 0.0,
                    // A Java `long`, so it can be negative; the bit pattern is
                    // what matters.
                    src_hash: f[22].parse::<i64>().unwrap() as u64,
                    rms_l: n(23),
                    rms_r: n(24),
                    peak_l: n(25),
                    peak_r: n(26),
                    dsr_db: if f[27] == "nan" { f64::NAN } else { n(27) },
                    fund_hz: n(28),
                });
            }
            assert!(!out.is_empty(), "vectors.tsv carried no data rows");
            out
        }

        fn row(id: &str) -> Row {
            rows()
                .into_iter()
                .find(|r| r.id == id)
                .unwrap_or_else(|| panic!("no vector row {id:?}"))
        }

        /// Regenerate the row's source PCM.
        ///
        /// **`LoopbackOracle.tone` verbatim**, and the FNV-1a check below is
        /// why that can be claimed rather than hoped: `Math.sin` and
        /// `f64::sin` are each specified to within an ULP and are not required
        /// to agree, so a silent one-LSB disagreement would have the two sides
        /// comparing renders of two different sounds.
        fn stimulus(r: &Row) -> Arc<Pcm> {
            let mut s = Vec::with_capacity(r.frames * r.chans as usize);
            for i in 0..r.frames {
                let t = i as f64;
                let l = (r.amp_l * (2.0 * std::f64::consts::PI * r.freq_l * t / r.srate as f64).sin())
                    .round() as i16;
                s.push(l);
                if r.chans == 2 {
                    let rr = (r.amp_r
                        * (2.0 * std::f64::consts::PI * r.freq_r * t / r.srate as f64).sin())
                    .round() as i16;
                    s.push(rr);
                }
            }
            Arc::new(Pcm {
                samples: s,
                channels: r.chans,
                sample_rate: r.srate,
            })
        }

        /// `LoopbackOracle.fnv1a`, over the little-endian sample bytes.
        fn fnv1a(pcm: &Pcm) -> u64 {
            let mut h: u64 = 0xcbf2_9ce4_8422_2325;
            for v in &pcm.samples {
                h = (h ^ (*v as u16 & 0xFF) as u64).wrapping_mul(0x0000_0100_0000_01b3);
                h = (h ^ ((*v as u16 >> 8) & 0xFF) as u64).wrapping_mul(0x0000_0100_0000_01b3);
            }
            h
        }

        /// Drive the **production** [`Mixer`] with the row's stimulus and return
        /// the same window OpenAL measured.
        ///
        /// Assertions read out of this, never out of `pan_gains` or
        /// `openal::linear_gain` recomputed in the test — M88's `r20` lesson,
        /// that a witness reading a value which merely *implies* the render is a
        /// proxy that looks more rigorous than it is.
        fn render_rewo(r: &Row) -> Vec<f32> {
            let pcm = stimulus(r);
            let mut m = Mixer::new(RATE);
            let (forward, up) = listener_basis(r.lyaw, r.lpitch);
            m.listener = ListenerTransform {
                position: r.lpos,
                forward,
                up,
            };
            for _ in 0..r.voices {
                let mut v = Voice::new(Arc::clone(&pcm));
                v.gain = r.vol;
                v.pitch = r.pitch;
                v.relative = r.rel;
                v.max_distance = if r.maxd < 0.0 { None } else { Some(r.maxd) };
                v.position = [r.src[0] as f32, r.src[1] as f32, r.src[2] as f32];
                v.looping = true;
                m.push(v);
            }
            let mut sink = NullSink::new();
            sink.pull(&mut m, WARMUP_FRAMES + MEASURE_FRAMES);
            sink.rendered[WARMUP_FRAMES * 2..].to_vec()
        }

        fn rms(out: &[f32], ch: usize) -> f64 {
            let n = out.len() / 2;
            let acc: f64 = (0..n).map(|i| (out[i * 2 + ch] as f64).powi(2)).sum();
            (acc / n as f64).sqrt()
        }

        fn peak(out: &[f32], ch: usize) -> f64 {
            (0..out.len() / 2).fold(0.0f64, |a, i| a.max((out[i * 2 + ch] as f64).abs()))
        }

        /// `LoopbackOracle.distortionToSignalDb` verbatim: energy away from the
        /// fundamental over energy at it, in dB.
        ///
        /// Delay-invariant, which is the whole reason it is a spectrum and not a
        /// sample difference — OpenAL's higher-order resamplers delay the signal
        /// and linear interpolation does not, so a direct difference would
        /// measure the group delay rather than the filter.
        fn distortion_to_signal_db(out: &[f32], fundamental_hz: f64) -> f64 {
            let n = DFT_N;
            let mut x = vec![0.0f64; n];
            for (i, xi) in x.iter_mut().enumerate() {
                // 4-term Blackman-Harris. Under Hann the fundamental's summed
                // sidelobe leakage floors this near -46 dB, which is where the
                // default resampler's rows land — so the window would be what
                // was being measured.
                let t = 2.0 * std::f64::consts::PI * i as f64 / n as f64;
                let w = 0.35875 - 0.48829 * t.cos() + 0.14128 * (2.0 * t).cos()
                    - 0.01168 * (3.0 * t).cos();
                *xi = out[i * 2] as f64 * w;
            }
            let bin = fundamental_hz * n as f64 / RATE as f64;
            let lo = (bin.floor() as i64 - FUND_HALFWIDTH).max(0);
            let hi = (bin.ceil() as i64 + FUND_HALFWIDTH).min(n as i64 / 2 - 1);
            let (mut fund, mut total) = (0.0f64, 0.0f64);
            for k in 0..n / 2 {
                let (mut sr, mut si) = (0.0f64, 0.0f64);
                for (i, xi) in x.iter().enumerate() {
                    let a = -2.0 * std::f64::consts::PI * k as f64 * i as f64 / n as f64;
                    sr += xi * a.cos();
                    si += xi * a.sin();
                }
                let m2 = sr * sr + si * si;
                total += m2;
                if k as i64 >= lo && k as i64 <= hi {
                    fund += m2;
                }
            }
            let rest = (total - fund).max(0.0);
            if fund <= 0.0 {
                return f64::NAN;
            }
            10.0 * (rest / fund + 1.0e-300).log10()
        }

        /// `b` relative to `a`, in dB. Positive means Rewo is louder.
        fn db(rewo: f64, openal: f64) -> f64 {
            20.0 * (rewo / openal).log10()
        }

        /// Assert a measured divergence sits in a window around the number this
        /// milestone recorded, with the live value in the message.
        fn pin(what: &str, measured: f64, recorded: f64, window: f64) {
            assert!(
                (measured - recorded).abs() <= window,
                "{what}: measured {measured:+.4} dB, M139 recorded {recorded:+.4} dB \
                 (window +-{window} dB). If the mixer was deliberately changed, \
                 re-measure and record the new number here rather than widening the window."
            );
        }

        // ------------------------------------------------------------ the file

        /// Trap 4. Every row's stimulus is regenerated here and must be the one
        /// the JVM rendered.
        ///
        /// Without this, a one-ULP `Math.sin` / `f64::sin` disagreement would
        /// have both sides confidently comparing renders of different sounds,
        /// and every divergence below would be partly an artefact of the input.
        #[test]
        fn every_regenerated_stimulus_is_the_one_the_jvm_rendered() {
            for r in rows() {
                let pcm = stimulus(&r);
                assert_eq!(
                    fnv1a(&pcm),
                    r.src_hash,
                    "stimulus for {} does not match the capture: Rust {:#x}, JVM {:#x}. \
                     Math.sin and f64::sin have diverged, or the generator has.",
                    r.id,
                    fnv1a(&pcm),
                    r.src_hash
                );
            }
        }

        /// The capture's own carryover control.
        ///
        /// The OpenAL context carries state between stimuli — the output
        /// limiter's gain above all — so a row's value can depend on what ran
        /// before it. `ctl.posx.first` and `ctl.posx.last` are the same stimulus
        /// at opposite ends of the run with the 32-voice row between them. At
        /// one second of settle they differed by 0.75 dB; the capture uses
        /// sixty, where they are bit-identical.
        ///
        /// This grades the **capture**, not the mixer, and it is the reason the
        /// other rows can be read as measurements at all.
        #[test]
        fn the_captures_carryover_control_pair_agrees_exactly() {
            let first = row("ctl.posx.first");
            let last = row("ctl.posx.last");
            assert_eq!(
                first.rms_l, last.rms_l,
                "the capture drifted across the run: {} vs {}. Raise settle.frames \
                 in LoopbackOracle and recapture.",
                first.rms_l, last.rms_l
            );
            assert!(first.rms_l > 0.0);
        }

        // ------------------------------------------------- what MATCHES exactly

        /// The attenuation curve is a **transcription**, and it is exact.
        ///
        /// `openal::linear_gain` is `1 - d/max` from the three properties
        /// `Channel.linearAttenuation` writes (`Channel.java:108-113`), and the
        /// capture reproduces it to the last digit at every distance — including
        /// **exactly zero at the radius**, the property an inverse-square model
        /// cannot have.
        ///
        /// Read as a ratio against `dist.d0p0`, so the pan divides out and this
        /// is the curve alone.
        #[test]
        fn the_distance_curve_matches_openal_exactly() {
            let d0 = row("dist.d0p0");
            for (id, d) in [
                ("dist.d1p0", 1.0f64),
                ("dist.d4p0", 4.0),
                ("dist.d8p0", 8.0),
                ("dist.d15p0", 15.0),
                ("dist.d16p0", 16.0),
                ("dist.d64p0", 64.0),
            ] {
                let r = row(id);
                let openal_ratio = r.rms_l / d0.rms_l;
                let rewo = render_rewo(&r);
                let rewo_ratio = rms(&rewo, 0) / rms(&render_rewo(&d0), 0);
                let expected = (1.0 - d / 16.0).max(0.0);
                assert!(
                    (openal_ratio - expected).abs() < 1e-6,
                    "{id}: OpenAL gave {openal_ratio}, 1 - d/max predicts {expected}"
                );
                assert!(
                    (rewo_ratio - expected).abs() < 1e-6,
                    "{id}: Rewo gave {rewo_ratio}, 1 - d/max predicts {expected}"
                );
            }
        }

        /// Hard left and hard right agree to better than a thousandth of a dB.
        ///
        /// This is the one bearing where equal power and whatever OpenAL does
        /// must coincide: all the energy is in one channel, so there is no split
        /// to disagree about. It is worth pinning anyway, because it is what
        /// makes the front/back divergence below a statement about the *law*
        /// rather than about a level error somewhere in the chain.
        #[test]
        fn hard_panning_matches_openal() {
            for (id, ch) in [("pan.posx", 0usize), ("pan.negx", 1)] {
                let r = row(id);
                let openal = if ch == 0 { r.rms_l } else { r.rms_r };
                let rewo = rms(&render_rewo(&r), ch);
                pin(&format!("{id} loud channel"), db(rewo, openal), 0.0, 0.002);
            }
        }

        /// A source directly to one side lands on the same side in both.
        ///
        /// **This is the assertion an earlier draft of the oracle got wrong**,
        /// and the way it got it wrong is the trap: OpenAL's untouched listener
        /// is `ListenerTransform::INITIAL`, facing **-Z**, while
        /// `listener_basis(0, 0)` faces **+Z**. A capture that never wrote the
        /// listener therefore compared two orientations a half turn apart and
        /// reported a left/right inversion that did not exist. Every row now
        /// writes the listener from the same formula, and the sides agree — at
        /// yaw 0, at yaw 90/180/270, and at listener pitch +-90.
        #[test]
        fn the_left_right_axis_agrees_at_every_orientation() {
            for id in [
                "pan.posx",
                "pan.negx",
                "yaw90p0.posx",
                "yaw180p0.posx",
                "yaw270p0.posx",
                "lpitch90p0.posx",
                "lpitchneg90p0.posx",
                "lpitch45p0.posx",
            ] {
                let r = row(id);
                let rewo = render_rewo(&r);
                let (rl, rr) = (rms(&rewo, 0), rms(&rewo, 1));
                let openal_left_louder = r.rms_l > r.rms_r;
                let rewo_left_louder = rl > rr;
                let openal_centred = (r.rms_l - r.rms_r).abs() / r.rms_l.max(r.rms_r) < 1e-3;
                let rewo_centred = (rl - rr).abs() / rl.max(rr) < 1e-3;
                assert_eq!(
                    openal_centred, rewo_centred,
                    "{id}: OpenAL centred={openal_centred}, Rewo centred={rewo_centred} \
                     (OpenAL {:.6}/{:.6}, Rewo {rl:.6}/{rr:.6})",
                    r.rms_l, r.rms_r
                );
                if !openal_centred {
                    assert_eq!(
                        openal_left_louder, rewo_left_louder,
                        "{id}: the image is on opposite sides. OpenAL {:.6}/{:.6}, \
                         Rewo {rl:.6}/{rr:.6}",
                        r.rms_l, r.rms_r
                    );
                }
            }
        }

        /// The up vector reaches the decode, on both sides.
        ///
        /// `ListenerTransform::right()` is `forward x up`
        /// (`ListenerTransform.java:8-10`) and this mixer's `right()` is the
        /// same cross product, so pinning up to a constant `(0,1,0)` would make
        /// it the **zero vector** at pitch +-90 and collapse the image to
        /// centre. It does not: at both pitches a source to the side stays hard
        /// to that side, in the capture and in Rewo.
        #[test]
        fn a_pitched_listener_keeps_its_stereo_image() {
            for id in ["lpitch90p0.posx", "lpitchneg90p0.posx"] {
                let r = row(id);
                let rewo = render_rewo(&r);
                let (rl, rr) = (rms(&rewo, 0), rms(&rewo, 1));
                assert!(
                    r.rms_l / r.rms_r > 1e6,
                    "{id}: OpenAL did not keep the image hard over ({:.9}/{:.9})",
                    r.rms_l,
                    r.rms_r
                );
                assert!(
                    rl / rr.max(1e-12) > 1e6,
                    "{id}: Rewo collapsed the image at pitch — the up vector is not \
                     reaching right() ({rl:.9}/{rr:.9})"
                );
            }
        }

        // ------------------------------------------------ what DIVERGES, and by
        // ------------------------------------------------ how much

        /// **The pan law diverges, and no tuning of this mixer can close it.**
        ///
        /// Measured against the hard-panned rows, which agree exactly, OpenAL
        /// puts a source directly in front at 0.5957 of full and one directly
        /// behind at 0.4043 — **summing to 1.0000**, the signature of a
        /// directional decode rather than a pairwise pan. Rewo puts both at
        /// `cos(pi/4)` = 0.7071, because its pan input is
        /// `dot(direction, right)` and that is **zero for both bearings**.
        ///
        /// So this is structural, not a curve to fit: front and back are the
        /// same number going into `pan_gains`, and no function of that number
        /// can separate them. Closing it means a different pan input, which is
        /// a different design and not a tuning pass.
        #[test]
        fn the_pan_law_diverges_in_front_and_behind() {
            let hard = row("pan.posx");
            let front = row("pan.posz");
            let behind = row("pan.negz");

            // Fractions of the hard-panned level, which is the common reference
            // the two renderers agree on.
            let f_openal = front.rms_l / hard.rms_l;
            let b_openal = behind.rms_l / hard.rms_l;
            assert!(
                (f_openal + b_openal - 1.0).abs() < 1e-4,
                "front {f_openal:.6} + behind {b_openal:.6} should sum to 1"
            );

            let hard_rewo = rms(&render_rewo(&hard), 0);
            let f_rewo = rms(&render_rewo(&front), 0) / hard_rewo;
            let b_rewo = rms(&render_rewo(&behind), 0) / hard_rewo;
            assert!(
                (f_rewo - b_rewo).abs() < 1e-9,
                "Rewo distinguishes front from behind, which its pan input cannot do: \
                 {f_rewo} vs {b_rewo}"
            );

            pin("front", db(f_rewo, f_openal), 1.4903, 0.02);
            pin("behind", db(b_rewo, b_openal), 4.8546, 0.02);
        }

        /// Directly overhead, OpenAL gives exactly half the hard-panned level in
        /// each channel and Rewo gives `cos(pi/4)`.
        ///
        /// Included because it is the bearing where the two laws are furthest
        /// apart while both are *centred*, so it isolates the law's level from
        /// its left/right behaviour entirely.
        #[test]
        fn overhead_diverges_by_three_db() {
            let hard = row("pan.posx");
            let up = row("pan.posy");
            let openal = up.rms_l / hard.rms_l;
            assert!(
                (openal - 0.5).abs() < 1e-6,
                "OpenAL overhead is {openal}, expected exactly 0.5 of hard-panned"
            );
            let rewo = rms(&render_rewo(&up), 0) / rms(&render_rewo(&hard), 0);
            pin("overhead", db(rewo, openal), 3.0103, 0.02);
        }

        /// **The resampler costs about 23 dB of distortion, measured inside
        /// OpenAL so nothing else can contaminate it.**
        ///
        /// The capture renders each pitch twice: once with the device default
        /// (Cubic Spline on the capturing machine, witnessed in the file header)
        /// and once with `AL_SOURCE_RESAMPLER_SOFT` forced to Linear, which is
        /// this mixer's algorithm. Both are OpenAL renders of the same source
        /// through the same everything, so the difference **is** the
        /// interpolator's own contribution and needs no cross-implementation
        /// alignment at all.
        ///
        /// `pitch.p2p0` is in the set and is **degenerate on purpose**: a step
        /// of exactly two source frames lands every output sample on a source
        /// sample, so no interpolator interpolates and the two agree to the bit.
        /// A comparison drawn only from powers of two would measure zero and
        /// conclude the algorithms agree. Its reading is also the measurement's
        /// own noise floor, at -85.5 dB, which is what says the -55 dB and
        /// -59 dB default rows are real numbers rather than the window.
        #[test]
        fn openals_default_resampler_beats_linear_by_about_23_db() {
            for (id, recorded) in [
                ("rate.48kto44k", 22.9005),
                ("pitch.p0p5", 23.0237),
                ("pitch.p0p7", 22.9307),
                ("pitch.p1p3", 22.9324),
                ("pitch.p1p5", 23.0248),
            ] {
                let default = row(id);
                let linear = row(&format!("linres.{id}"));
                // The pairing is the whole experiment, so it is asserted rather
                // than assumed from the id prefix: a `linres.` row that was not
                // actually captured with the resampler forced would make this
                // test a comparison of a row with itself.
                assert!(!default.res_lin, "{id} should be the device default");
                assert!(linear.res_lin, "linres.{id} should have been forced to Linear");
                let delta = linear.dsr_db - default.dsr_db;
                assert!(
                    (delta - recorded).abs() <= 0.05,
                    "{id}: linear costs {delta:+.4} dB of distortion over the default \
                     resampler, M139 recorded {recorded:+.4} dB"
                );
            }

            let deg = row("pitch.p2p0");
            let deg_lin = row("linres.pitch.p2p0");
            assert_eq!(
                deg.dsr_db, deg_lin.dsr_db,
                "an integer resampling step must land on source samples, so the two \
                 interpolators cannot differ there"
            );
            assert!(
                deg.dsr_db < -80.0,
                "the degenerate row is the measurement floor and it should be far below \
                 anything measured: {}",
                deg.dsr_db
            );
        }

        /// This mixer's linear interpolation lands where OpenAL's does.
        ///
        /// The previous test measured what choosing linear costs *inside*
        /// OpenAL. This one asks whether Rewo's linear is the same linear, by
        /// running the identical stimulus through the production [`Mixer`] and
        /// comparing the same statistic against the `linres.` rows. Agreement
        /// here plus the gap above is what makes "Rewo's resampler is about
        /// 23 dB noisier than vanilla's" a measurement rather than an inference.
        #[test]
        fn rewos_interpolation_matches_openals_linear() {
            // Every row is measured before anything is asserted, so a failure
            // reports the whole picture rather than the first row that trips.
            let mut report = Vec::new();
            let mut worst: f64 = 0.0;
            for id in [
                "linres.rate.48kto44k",
                "linres.pitch.p0p5",
                "linres.pitch.p0p7",
                "linres.pitch.p1p3",
                "linres.pitch.p1p5",
            ] {
                let r = row(id);
                let rewo = distortion_to_signal_db(&render_rewo(&r), r.fund_hz);
                let delta = rewo - r.dsr_db;
                worst = worst.max(delta.abs());
                report.push(format!("{id}: rewo {rewo:+.4} openal {:+.4} d {delta:+.4}", r.dsr_db));
            }
            // 0.35 dB, and the shape of the disagreement is the interesting
            // part. Rate conversion agrees to 0.005 dB; the pitch rows are up
            // to 0.30 dB apart. Both sides interpolate between the same two
            // samples, so what differs is WHERE they land: this mixer
            // accumulates a fractional cursor in f64
            // (`cursor += step`, `sample_at`), and OpenAL Soft advances a
            // fixed-point fractional counter whose increment is quantised.
            // At a step of 48000/44100 the two happen to track; at 0.7 they
            // drift by a fraction of a sample and the interpolation error
            // differs slightly with them.
            //
            // The bound is deliberately tight enough that the consumer's DFT
            // window stays load-bearing: at 3 dB it could drop to Hann, whose
            // leakage floor near -46 dB would leave these -36 dB rows passing
            // while every default-resampler row became unreadable. The
            // mutation battery found exactly that.
            assert!(
                worst <= 0.35,
                "Rewo's interpolation and OpenAL's Linear should agree closely; \
                 worst gap {worst:+.4} dB.\n  {}",
                report.join("\n  ")
            );

            // **At an integer step this mixer interpolates nothing either.**
            // `step` is exactly 2.0, so `frac` is exactly 0 on every frame and
            // `sample_at` returns the source sample untouched - the same reason
            // OpenAL's two interpolators agree to the bit on that row. This is
            // what makes the 23 dB above attributable to the interpolation
            // rather than to something else in Rewo's path: with the
            // interpolator out of the picture, Rewo sits at the measurement
            // floor too.
            //
            // It is also the one assertion here that the DFT window has to be
            // right for. The floor under Blackman-Harris is near -85 dB and
            // under Hann near -46 dB, so this bound is what stops the consumer
            // quietly reverting to a window that cannot read the default
            // resampler's rows at all.
            let deg = row("pitch.p2p0");
            let deg_rewo = distortion_to_signal_db(&render_rewo(&deg), deg.fund_hz);
            assert!(
                deg_rewo < -70.0,
                "at an integer resampling step Rewo should add no distortion, but it \
                 measured {deg_rewo:+.4} dB against OpenAL's {:+.4} dB. If the mixer is \
                 unchanged, check the DFT window: Hann floors this statistic near -46 dB.",
                deg.dsr_db
            );
        }

        /// **OpenAL does not spatialise a multi-channel buffer, and it does not
        /// attenuate one either.** The second half is the part nothing in this
        /// project had written down.
        ///
        /// `REWO_AUDIO_PLAN.md` s5 carries the first half as `[concurring]` and
        /// unverified, and it matters because `item/goat_horn/call3.ogg` is the
        /// one stereo variant of an otherwise mono event — so the same event
        /// spatialises on seven rolls and not the eighth. The capture settles
        /// it: `stereo.d1p0` and `stereo.d8p0` are **byte-identical** despite an
        /// eightfold distance change, the channels keep the source's own exact
        /// 2:1 ratio, and halving `AL_GAIN` halves the output.
        ///
        /// [`pan_gains`] already returns `(1.0, 1.0)` above one channel, so Rewo
        /// matches on the panning. **It diverges on the attenuation**: `render`
        /// applies `openal::linear_gain` before the pan regardless of channel
        /// count, so Rewo fades a stereo source with distance where vanilla does
        /// not. At the capture's 8 blocks of 16 that is a 6 dB divergence, and
        /// it grows to silence at the radius where vanilla stays at full level.
        #[test]
        fn a_stereo_buffer_is_neither_panned_nor_attenuated_by_openal() {
            let d1 = row("stereo.d1p0");
            let d8 = row("stereo.d8p0");
            let half = row("stereo.halfvol");

            assert_eq!(
                (d1.rms_l, d1.rms_r),
                (d8.rms_l, d8.rms_r),
                "distance changed a stereo source's level in OpenAL"
            );
            assert!(
                (d1.rms_l / d1.rms_r - 2.0).abs() < 1e-4,
                "the stereo channels did not pass through at the source's own 2:1 ratio: {}",
                d1.rms_l / d1.rms_r
            );
            assert!(
                (half.rms_l / d1.rms_l - 0.5).abs() < 1e-4,
                "AL_GAIN is not applied to a stereo source: {}",
                half.rms_l / d1.rms_l
            );

            // Rewo matches at d=1 (attenuation 0.9375 there is nearly unity is
            // NOT the reason — 0.9375 is a real 0.56 dB, so this row is a
            // genuine agreement check on the pan), and diverges at d=8.
            let r1 = render_rewo(&d1);
            assert!(
                (rms(&r1, 0) / rms(&r1, 1) - 2.0).abs() < 1e-4,
                "Rewo panned a stereo source: {}",
                rms(&r1, 0) / rms(&r1, 1)
            );

            let r8 = render_rewo(&d8);
            pin(
                "stereo at 8 of 16 blocks",
                db(rms(&r8, 0), d8.rms_l),
                -6.0206,
                0.02,
            );
            pin(
                "stereo at 1 of 16 blocks",
                db(rms(&r1, 0), d1.rms_l),
                -0.5606,
                0.02,
            );
        }

        /// **The limiter is the largest divergence in this file**, and it is
        /// exactly the dense-scene case `REWO_AUDIO_PLAN.md` s4 says no CPU-side
        /// gate can see.
        ///
        /// Thirty-two coherent full-volume voices sum to about eight times full
        /// scale. OpenAL's output limiter — whose *enable* is in Java
        /// (`Library.java:131`) and whose *curve* is in the DLL — brings that to
        /// exactly full scale with **no measurable added distortion**: the
        /// 32-voice row's distortion statistic matches the single-voice row's to
        /// three decimal places, both at the measurement floor. Rewo's single
        /// hard `clamp(-1, 1)` instead squares the wave off.
        ///
        /// Both halves are asserted, because they say different things: the
        /// level divergence is what a listener notices as loudness, and the
        /// distortion divergence is what they notice as the sound breaking up.
        #[test]
        fn the_output_limiter_diverges_from_a_hard_clamp() {
            let x32 = row("limiter.x32");
            let x1 = row("limiter.x1");

            assert!(
                (x32.dsr_db - x1.dsr_db).abs() < 0.01,
                "OpenAL's limiter added distortion: {} vs {} for one voice",
                x32.dsr_db,
                x1.dsr_db
            );
            assert!(
                (x32.peak_l - 1.0).abs() < 1e-6,
                "the limiter did not land on full scale: {}",
                x32.peak_l
            );

            let rewo = render_rewo(&x32);
            assert!(
                (peak(&rewo, 0) - 1.0).abs() < 1e-6,
                "the clamp should pin at exactly full scale: {}",
                peak(&rewo, 0)
            );
            // +2.79 dB, not the +3.01 an ideal square wave over a sine would
            // give: the sum is about eight times full scale, so the clipped
            // wave is nearly but not quite square and its RMS falls a little
            // short of 1.0. This number was PREDICTED as 3.01 and the test
            // caught it — which is what pinning a measurement is for.
            pin("32 coherent voices", db(rms(&rewo, 0), x32.rms_l), 2.7913, 0.02);

            let rewo_dsr = distortion_to_signal_db(&rewo, x32.fund_hz);
            let excess = rewo_dsr - x32.dsr_db;
            assert!(
                excess > 60.0,
                "a hard clamp on 32 coherent voices should be vastly more distorted than \
                 a limiter; measured {rewo_dsr:+.2} dB against OpenAL's {:+.2} dB \
                 ({excess:+.2} dB)",
                x32.dsr_db
            );
        }

        /// **`AL_SOURCE_RELATIVE` uses a fixed listener-local frame in OpenAL,
        /// and this mixer's current world-space right vector.** They agree only
        /// when the listener faces the default direction.
        ///
        /// The capture is unambiguous: `relative.yaw0`, `relative.yaw90` and
        /// `relative.walked` are **byte-identical**, so turning and walking the
        /// listener move a relative source not at all in OpenAL. `render` instead
        /// takes `rel = v.position` and then pans it with `right()`, which turns
        /// with the listener — so a relative source off the centre line would
        /// swing across the image as the player looks around, and sit on the
        /// opposite side at yaw 0 (`pan.posx` and `relative.yaw0` are the same
        /// magnitude in opposite channels).
        ///
        /// **It is unreachable in vanilla today**, which is why this is recorded
        /// rather than filed as a live fault. Every relative instance in 26.2
        /// sits at the origin: `SimpleSoundInstance`'s three relative factories
        /// pass `0.0, 0.0, 0.0` (`SimpleSoundInstance.java:26-60`), and
        /// `BiomeAmbientSoundsHandler.LoopSoundInstance` and the two
        /// `UnderwaterAmbientSoundInstances` set `relative = true` without ever
        /// writing a position. At the origin the divergence collapses into the
        /// front-versus-centre one the pan test already measures. It becomes
        /// reachable the moment anything gives a relative sound a bearing.
        #[test]
        fn a_relative_source_ignores_the_listener_in_openal_and_not_in_rewo() {
            let y0 = row("relative.yaw0");
            let y90 = row("relative.yaw90");
            let walked = row("relative.walked");
            assert_eq!(
                (y0.rms_l, y0.rms_r),
                (y90.rms_l, y90.rms_r),
                "turning the listener moved a relative source in OpenAL"
            );
            assert_eq!(
                (y0.rms_l, y0.rms_r),
                (walked.rms_l, walked.rms_r),
                "walking the listener moved a relative source in OpenAL"
            );

            let r0 = render_rewo(&y0);
            let r90 = render_rewo(&y90);
            let walked_rewo = render_rewo(&walked);

            // **Rewo does skip the listener-position subtraction, and this is
            // the only fixture that can see it.** `relative.yaw0` and
            // `relative.yaw90` both put the listener at the ORIGIN, where
            // subtracting its position changes nothing — so a mixer that had
            // forgotten `v.relative` entirely would render them identically and
            // every assertion below would still hold. `relative.walked` moves
            // the listener 141 blocks away, which is what makes the claim
            // checkable. The battery found this: deleting the `v.relative`
            // branch survived until this row was rendered.
            assert!(
                (rms(&walked_rewo, 0) - rms(&r0, 0)).abs() < 1e-9
                    && (rms(&walked_rewo, 1) - rms(&r0, 1)).abs() < 1e-9,
                "Rewo moved a relative source when the listener walked: \
                 {:.9}/{:.9} against {:.9}/{:.9} at the origin",
                rms(&walked_rewo, 0),
                rms(&walked_rewo, 1),
                rms(&r0, 0),
                rms(&r0, 1)
            );

            let side = |o: &[f32]| rms(o, 0) > rms(o, 1);
            assert_ne!(
                side(&r0),
                side(&r90),
                "Rewo is expected to swing a relative source with the listener's yaw; \
                 if this now holds, the frame was fixed and this test should become an \
                 agreement check"
            );
            assert_ne!(
                side(&r0),
                y0.rms_l > y0.rms_r,
                "Rewo is expected to place a relative source on the opposite side at \
                 yaw 0, because its right vector is the listener's and OpenAL's frame \
                 is fixed"
            );
        }

        /// `disableAttenuation` is full gain at any distance in both, so the
        /// only divergence at 400 blocks is the pan law's.
        ///
        /// Worth its own row because it is the one place where a mixer that
        /// implemented "no attenuation" as "a very large radius" would diverge
        /// enormously and silently, and this one does not.
        #[test]
        fn an_unattenuated_source_is_full_gain_at_any_distance() {
            let far = row("noatten.far");
            let behind = row("pan.negz");
            // Same bearing, 400 blocks against 1, and the only difference in the
            // capture is the 0.8 volume against `pan.negz`'s attenuation at
            // d=1. Compare Rewo against OpenAL directly instead.
            let rewo = rms(&render_rewo(&far), 0);
            pin("unattenuated at 400 blocks", db(rewo, far.rms_l), 4.8546, 0.02);
            assert!(
                far.rms_l > behind.rms_l,
                "an unattenuated source 400 blocks away should be louder than an \
                 attenuated one at 1 block: {} vs {}",
                far.rms_l,
                behind.rms_l
            );
        }
    }
}
