//! The device — cpal, and the only code in Rewo that can make a noise.
//!
//! **Nothing here is checked by any gate, and nothing can be.** No test in this
//! project opens an audio device: a device that is absent, muted, exclusive-mode
//! or simply not connected to speakers all look identical from inside the
//! process, and whether the result *sounds* like Minecraft is not a property a
//! machine can read. `REWO_AUDIO_PLAN.md` §4 sets this out at length. What holds
//! this module up is that everything beneath it — the ring, the mixer, the
//! decode, the quantisation, the listener basis — is graded on its own, so the
//! ungraded part is the thin binding rather than the behaviour.
//!
//! **It reaches `rewo-app` only under that crate's `audio` feature, which is
//! off by default** (M143). A default build links no audio stack at all, so the
//! 34 gates are unchanged and none of them can open a device by accident; a
//! build with `--features audio` and a run with `rewo live --audio` is the only
//! path from the client to a noise. `cargo run -p rewo-audio --example listen`
//! is the other one. **Either way the listening pass is a human's job** — a
//! green suite says nothing about this module, by construction.
//!
//! ## Real-time discipline, and the one place this first cut breaks it
//!
//! The callback may not allocate, lock, or make a syscall — a heap lock held by
//! another thread turns into a dropout. Draining the ring and rendering obey
//! that: the ring is fixed-capacity and `Mixer::render` writes into the caller's
//! slice.
//!
//! **`retire_finished` is the exception, and it is a stated deviation.** Removing
//! a finished voice drops its `Arc<Pcm>`, and if that was the last reference the
//! deallocation happens *in the callback*. The plan's design sends retired
//! buffers back to a worker over a second ring; this cut does not have one, so
//! it takes the simpler path and says so. The consequence is a possible dropout
//! at the moment a sound ends — worth fixing before anyone calls the audio
//! smooth, and not worth pretending is absent.

use crate::buffers::PcmSource;
use crate::device::{Command, CommandRing, DEFAULT_RING_CAPACITY};
use crate::live_sink::LiveSink;
use crate::mixer::Mixer;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use rewo_net::sound_engine::{
    ChannelCall, ChannelId, ChannelSink, ListenerTransform, SinkDiagnostics,
};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// An open output stream, with the ring the engine pushes into.
///
/// Dropping it stops the stream — `cpal::Stream` is not `Send`, so this must
/// live on the thread that built it.
pub struct CpalSink {
    ring: Arc<CommandRing>,
    errors: Arc<AtomicU64>,
    out_rate: u32,
    _stream: cpal::Stream,
}

impl CpalSink {
    /// Open the default output device and start the stream.
    ///
    /// **Requires a stereo device.** The mixer renders interleaved stereo, and
    /// silently playing a stereo mix into a mono or 5.1 device would come out as
    /// mangled rather than as an error, so an unexpected channel count is
    /// refused with the number in the message. Supporting other layouts is a
    /// downmix decision, not a bug fix.
    pub fn open() -> Result<CpalSink, String> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or_else(|| "no default output device".to_string())?;
        let supported = device
            .default_output_config()
            .map_err(|e| format!("no default output config: {e}"))?;
        let channels = supported.channels();
        if channels != 2 {
            return Err(format!(
                "the mixer renders stereo and this device wants {channels} channels; \
                 a downmix is a decision rather than a fix"
            ));
        }
        let out_rate = supported.sample_rate().0;
        let config: cpal::StreamConfig = supported.into();

        let ring = CommandRing::with_capacity(DEFAULT_RING_CAPACITY);
        let errors = Arc::new(AtomicU64::new(0));

        let ring_cb = Arc::clone(&ring);
        let errors_cb = Arc::clone(&errors);
        let mut mixer = Mixer::new(out_rate);
        let stream = device
            .build_output_stream(
                &config,
                move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                    // Drain everything queued since the last period. Bounded by
                    // the ring's capacity, so this cannot run long.
                    while let Some(cmd) = ring_cb.pop() {
                        mixer.apply(&cmd);
                    }
                    mixer.render(data);
                    // The stated deviation: this can drop an `Arc<Pcm>` here.
                    mixer.retire_finished();
                },
                move |err| {
                    // **Counted, not logged.** A logger takes a lock and may
                    // allocate, and this runs on cpal's error path which is
                    // close enough to the callback to matter. A caller reads the
                    // count; the alternative is a stream that has died with
                    // nothing to show for it.
                    let _ = &err;
                    errors_cb.fetch_add(1, Ordering::Relaxed);
                },
                None,
            )
            .map_err(|e| format!("could not build the output stream: {e}"))?;
        stream
            .play()
            .map_err(|e| format!("could not start the stream: {e}"))?;

        Ok(CpalSink {
            ring,
            errors,
            out_rate,
            _stream: stream,
        })
    }

    /// The producer end. One `push` per `ChannelCall`, and it never blocks.
    pub fn ring(&self) -> &Arc<CommandRing> {
        &self.ring
    }

    /// The device's sample rate. Sources are resampled into it.
    pub fn out_rate(&self) -> u32 {
        self.out_rate
    }

    /// Stream errors reported by cpal since opening.
    ///
    /// **Non-zero means the device has trouble**, and it is worth reading
    /// alongside `ring().dropped()`: errors here with drops there is a stalled
    /// device, while drops alone with no errors is a callback that is simply not
    /// keeping up.
    pub fn errors(&self) -> u64 {
        self.errors.load(Ordering::Relaxed)
    }

    /// Convenience for a caller that has a decoded buffer and wants it heard.
    ///
    /// The order is vanilla's: properties, attach, play
    /// (`SoundEngine.java:417-434`). Returns `false` if any command was refused,
    /// which means the ring was full and the sequence is now incomplete — a
    /// caller that ignored this would leave a half-configured voice.
    pub fn play_once(&self, id: rewo_net::sound_engine::ChannelId, pcm: Arc<crate::buffers::Pcm>) -> bool {
        use rewo_net::sound_engine::ChannelCall as C;
        let mut ok = true;
        ok &= self.ring.push(Command::Channel(id, C::SetPitch(1.0)));
        ok &= self.ring.push(Command::Channel(id, C::SetVolume(1.0)));
        ok &= self.ring.push(Command::Channel(id, C::DisableAttenuation));
        ok &= self.ring.push(Command::Channel(id, C::SetLooping(false)));
        ok &= self.ring.push(Command::Channel(id, C::SetSelfPosition(0.0, 0.0, 0.0)));
        ok &= self.ring.push(Command::Channel(id, C::SetRelative(true)));
        ok &= self.ring.push(Command::Attach(id, pcm));
        ok &= self.ring.push(Command::Channel(id, C::Play));
        ok
    }
}

/// An open device and the engine's backend, as one [`ChannelSink`].
///
/// **The pairing exists so a `!Send` handle never has to be threaded through
/// the client.** `cpal::Stream` stops the moment it is dropped and cannot be
/// moved off the thread that built it, so something must hold it for the life
/// of the session. Making that something the sink itself means the client
/// stores one `Box<dyn ChannelSink>` and the device rides along inside it —
/// rather than every function between `main` and the tick loop growing a
/// keepalive parameter whose only job is not to be dropped.
///
/// Nothing here is checked by any gate and nothing can be; see this module's
/// own doc, and [`crate::live_sink`] for the part that is.
pub struct CpalBackend {
    sink: LiveSink<Box<dyn PcmSource>>,
    /// Held, not used. Dropping it stops the stream.
    device: CpalSink,
}

impl CpalBackend {
    /// Open the default output device and build a backend over it.
    ///
    /// `source` turns an asset key into samples. It is the caller's because the
    /// store lookup needs `rewo-data`, and this crate does not depend on it —
    /// see [`crate::decode::BytesSource`].
    /// `worker_source` moves every future STATIC decode onto a worker thread
    /// (M156). `None` keeps the synchronous path, which is what every gate and
    /// every unit test uses.
    ///
    /// A second source rather than a shared one because the worker owns its on
    /// another thread, and because streams are deliberately not moved — see
    /// [`crate::decode_worker`].
    pub fn open_with_worker(
        source: Box<dyn PcmSource>,
        worker_source: Option<Box<dyn PcmSource + Send>>,
    ) -> Result<CpalBackend, String> {
        Self::open_with_workers(source, worker_source, None)
    }

    /// Both workers: the static decode pool (M156) and the streaming thread
    /// (M159).
    ///
    /// **Two sources, not one shared behind a lock**, for the reason
    /// `audio_backend` already states about the first pair: they are independent
    /// readers of an immutable store, so nothing needs synchronising. A third
    /// reader costs one more handle and keeps the streaming thread from ever
    /// waiting on the static one — which is vanilla's arrangement, where the two
    /// executors are entirely separate.
    pub fn open_with_workers(
        source: Box<dyn PcmSource>,
        worker_source: Option<Box<dyn PcmSource + Send>>,
        stream_source: Option<Box<dyn PcmSource + Send>>,
    ) -> Result<CpalBackend, String> {
        let mut b = Self::open(source)?;
        if let Some(w) = worker_source {
            b.sink.buffers_mut().with_worker(w);
        }
        if let Some(s) = stream_source {
            b.sink
                .set_stream_worker(crate::stream_worker::StreamWorker::spawn(s));
        }
        Ok(b)
    }

    pub fn open(source: Box<dyn PcmSource>) -> Result<CpalBackend, String> {
        let device = CpalSink::open()?;
        let sink = LiveSink::new(Arc::clone(device.ring()), source);
        Ok(CpalBackend { sink, device })
    }

    /// The device's sample rate — worth logging, because a device running at a
    /// rate no source uses means the resampler is on every sound.
    pub fn out_rate(&self) -> u32 {
        self.device.out_rate()
    }
}

impl ChannelSink for CpalBackend {
    fn submit(&mut self, channel: ChannelId, call: &ChannelCall) {
        self.sink.submit(channel, call);
    }
    fn release(&mut self, channel: ChannelId) {
        self.sink.release(channel);
    }
    fn set_listener(&mut self, transform: ListenerTransform) {
        self.sink.set_listener(transform);
    }
    fn tick(&mut self) {
        self.sink.tick();
    }
    fn stopped(&self, channel: ChannelId) -> Option<bool> {
        self.sink.stopped(channel)
    }
    fn diagnostics(&self) -> SinkDiagnostics {
        SinkDiagnostics {
            // The one number the sink cannot know: the stream's own error
            // count lives on the device side of the pairing.
            device_errors: self.device.errors(),
            ..self.sink.diagnostics()
        }
    }
}
