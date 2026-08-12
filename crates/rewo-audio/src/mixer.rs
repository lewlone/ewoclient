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

    /// **Three queued buffers are bit-identical to one buffer of their
    /// concatenation.**
    ///
    /// The whole claim of the streaming path in one assertion: how the audio is
    /// *delivered* must not change what comes out. The rate ratio is deliberately
    /// non-integer (48 kHz source into a 44.1 kHz device, step 1.088…) so the
    /// cursor lands mid-sample at both joins — which is exactly where a swap that
    /// reset the cursor to zero instead of carrying the fraction would drift, and
    /// where a fixture at 1:1 would prove nothing.
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
        let one = voice(dc(48_000, 3_000, 1), vec![]);
        let three = voice(
            dc(48_000, 1_000, 1),
            vec![dc(48_000, 1_000, 1), dc(48_000, 1_000, 1)],
        );
        assert_eq!(one, three, "the join must not be audible or countable");
        // …and the fixture really did sound, so the equality is not two silences.
        assert!(peak_lr(&one).0 > 0.0);
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
}
