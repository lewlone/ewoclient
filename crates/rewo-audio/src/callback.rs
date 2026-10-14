//! The device callback's body, without the device.
//!
//! [`RenderCallback`] is everything `cpal_sink` runs on the audio thread: drain
//! the command ring, mix, retire finished voices, publish counters. It lives
//! here rather than inside the cpal closure so it can be driven by a test —
//! `tests/rt_alloc.rs` runs it under a counting global allocator and asserts
//! that steady-state callback work neither allocates nor frees.
//!
//! ## What "real-time safe" means here, exactly
//!
//! - **No allocation.** Voice slots, queue storage and the empty buffer are
//!   preallocated in [`Mixer::new`]; the scratch buffer for integer formats is
//!   made in [`RenderCallback::new`]. Overflow is refused and counted, never
//!   grown: see [`crate::mixer::MAX_VOICES`] and [`crate::mixer::MAX_QUEUED`].
//! - **No free.** Everything the mixer lets go of — a replaced or finished
//!   buffer, a stopped voice's queue, an unusable command — goes out as a
//!   [`Retired`] over a second SPSC ring and is dropped by whoever drains it
//!   (`CpalSink::service`, on the engine's thread). If that ring is ever full
//!   the value is freed in place and counted in
//!   [`CallbackStats::freed_in_place`], so a too-small ring is visible rather
//!   than a silent regression.
//! - **No lock, no syscall, no logging.** Counters are relaxed atomic stores;
//!   the engine's thread reads and logs them.

use crate::device::{CommandConsumer, Producer};
use crate::mixer::{Mixer, Retired};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Frames of scratch for a non-`f32` device: the mixer renders `f32` and the
/// callback converts. A period longer than this is rendered in several passes,
/// so the size bounds memory, not correctness.
pub const SCRATCH_FRAMES: usize = 4096;

/// Counters the callback publishes and the engine's thread reads.
///
/// All relaxed: each is a monotone diagnostic read on its own, and nothing is
/// ordered against it.
#[derive(Default)]
pub struct CallbackStats {
    /// Callbacks run. A value that stops moving is a stream that has stopped
    /// calling back — the device-loss signal that needs no error callback.
    pub heartbeat: AtomicU64,
    /// [`Mixer::ignored`].
    pub ignored: AtomicU64,
    /// [`Mixer::refused`].
    pub refused: AtomicU64,
    /// [`Mixer::queue_overflow`].
    pub queue_overflow: AtomicU64,
    /// [`Mixer::freed_in_place`]: frees that happened on the audio thread
    /// because the retire ring was full. Should stay zero.
    pub freed_in_place: AtomicU64,
}

/// The audio thread's whole state.
pub struct RenderCallback {
    mixer: Mixer,
    commands: CommandConsumer,
    scratch: Box<[f32]>,
    stats: Arc<CallbackStats>,
}

impl RenderCallback {
    /// Build the callback over `mixer`, draining `commands` and retiring into
    /// `graveyard`.
    pub fn new(
        mut mixer: Mixer,
        commands: CommandConsumer,
        graveyard: Producer<Retired>,
        stats: Arc<CallbackStats>,
    ) -> RenderCallback {
        mixer.set_graveyard(graveyard);
        RenderCallback {
            mixer,
            commands,
            scratch: vec![0.0; SCRATCH_FRAMES * 2].into_boxed_slice(),
            stats,
        }
    }

    /// The device's rate changed (a rebuilt stream on another device). Voices
    /// keep their place; the step is recomputed on the next render.
    pub fn set_out_rate(&mut self, rate: u32) {
        self.mixer.out_rate = rate;
    }

    pub fn mixer(&self) -> &Mixer {
        &self.mixer
    }

    /// One period for an `f32` device.
    pub fn render_f32(&mut self, out: &mut [f32]) {
        self.drain();
        self.mixer.render(out);
        self.finish();
    }

    /// One period for an integer device: mix into scratch, convert per sample.
    ///
    /// `out.len()` is a whole number of stereo frames, and so is every chunk,
    /// because the scratch length is even.
    pub fn render_into<T>(&mut self, out: &mut [T], convert: impl Fn(f32) -> T) {
        self.drain();
        let scratch_len = self.scratch.len();
        for chunk in out.chunks_mut(scratch_len) {
            let mixed = &mut self.scratch[..chunk.len()];
            self.mixer.render(mixed);
            for (o, s) in chunk.iter_mut().zip(mixed.iter()) {
                *o = convert(*s);
            }
        }
        self.finish();
    }

    fn drain(&mut self) {
        // Bounded by the ring's capacity plus whatever the producer manages to
        // write meanwhile, so this cannot run away.
        while let Some(cmd) = self.commands.pop() {
            self.mixer.apply_owned(cmd);
        }
    }

    fn finish(&mut self) {
        self.mixer.retire_finished();
        let s = &self.stats;
        s.ignored.store(self.mixer.ignored, Ordering::Relaxed);
        s.refused.store(self.mixer.refused, Ordering::Relaxed);
        s.queue_overflow
            .store(self.mixer.queue_overflow, Ordering::Relaxed);
        s.freed_in_place
            .store(self.mixer.freed_in_place(), Ordering::Relaxed);
        s.heartbeat.fetch_add(1, Ordering::Relaxed);
    }
}

/// `f32` in `-1..=1` to a signed 16-bit sample. Clamped first, so an
/// out-of-range or NaN input cannot wrap (`NaN as i16` is 0).
pub fn f32_to_i16(s: f32) -> i16 {
    (s.clamp(-1.0, 1.0) * 32767.0) as i16
}

/// `f32` to an unsigned 16-bit sample: the signed value offset by 32768, so
/// silence is exactly the midpoint.
pub fn f32_to_u16(s: f32) -> u16 {
    (f32_to_i16(s) as i32 + 32768) as u16
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buffers::Pcm;
    use crate::device::{command_ring, ring, Command};
    use rewo_net::sound_engine::ChannelCall as C;

    #[test]
    fn integer_conversions_hit_the_ends_and_the_middle() {
        assert_eq!(f32_to_i16(0.0), 0);
        assert_eq!(f32_to_i16(1.0), 32767);
        assert_eq!(f32_to_i16(-1.0), -32767);
        assert_eq!(f32_to_i16(7.0), 32767, "clamped, not wrapped");
        assert_eq!(f32_to_i16(f32::NAN), 0);
        assert_eq!(f32_to_u16(0.0), 32768, "silence is the midpoint");
        assert_eq!(f32_to_u16(1.0), 65535);
        assert_eq!(f32_to_u16(-1.0), 1);
    }

    /// An integer device hears what an `f32` one does, converted — including
    /// across a period longer than the scratch buffer.
    #[test]
    fn an_i16_period_matches_the_f32_render() {
        let run = |as_i16: bool| -> Vec<i16> {
            let (p, c) = command_ring(64);
            let (g, _gc) = ring::<Retired>(64);
            let mut cb = RenderCallback::new(Mixer::new(44_100), c, g, Arc::default());
            let pcm = Arc::new(Pcm {
                samples: (0..20_000).map(|i| (i % 2000) as i16 * 8).collect(),
                channels: 1,
                sample_rate: 44_100,
            });
            p.push(Command::Attach(1, pcm));
            p.push(Command::Channel(1, C::SetSelfPosition(0.0, 0.0, -1.0)));
            p.push(Command::Channel(1, C::Play));
            let frames = SCRATCH_FRAMES * 2 + 100;
            if as_i16 {
                let mut out = vec![0i16; frames * 2];
                cb.render_into(&mut out, f32_to_i16);
                out
            } else {
                let mut out = vec![0f32; frames * 2];
                cb.render_f32(&mut out);
                out.into_iter().map(f32_to_i16).collect()
            }
        };
        let (a, b) = (run(true), run(false));
        assert!(a.iter().any(|&s| s != 0), "it rendered something");
        assert_eq!(a, b);
    }

    #[test]
    fn the_heartbeat_counts_callbacks() {
        let (_p, c) = command_ring(4);
        let (g, _gc) = ring::<Retired>(4);
        let stats = Arc::new(CallbackStats::default());
        let mut cb = RenderCallback::new(Mixer::new(48_000), c, g, Arc::clone(&stats));
        let mut out = [0f32; 64];
        cb.render_f32(&mut out);
        cb.render_f32(&mut out);
        assert_eq!(stats.heartbeat.load(Ordering::Relaxed), 2);
    }
}
