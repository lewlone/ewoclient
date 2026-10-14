//! The device — cpal, and the only code in Rewo that can make a noise.
//!
//! **Nothing here is checked by any gate, and nothing can be.** No test in this
//! project opens an audio device: a device that is absent, muted, exclusive-mode
//! or simply not connected to speakers all look identical from inside the
//! process, and whether the result *sounds* like Minecraft is not a property a
//! machine can read. `REWO_AUDIO_PLAN.md` §4 sets this out at length. What holds
//! this module up is that everything beneath it — the ring, the mixer, the
//! callback body, the decode, the quantisation, the listener basis, the
//! recovery schedule — is graded on its own, so the ungraded part is the thin
//! binding rather than the behaviour.
//!
//! **It reaches `rewo-app` only under that crate's `audio` feature, which is
//! off by default** (M143). A default build links no audio stack at all, so the
//! gates are unchanged and none of them can open a device by accident; a
//! build with `--features audio` and a run with `rewo live --audio` is the only
//! path from the client to a noise. `cargo run -p rewo-audio --example listen`
//! is the other one. **Either way the listening pass is a human's job** — a
//! green suite says nothing about this module, by construction.
//!
//! ## Real-time discipline
//!
//! The data callback may not allocate, free, lock, or make a syscall — a heap
//! lock held by another thread turns into a dropout. Its whole body is
//! [`crate::callback::RenderCallback`], which `tests/rt_alloc.rs` runs under a
//! counting allocator. Retired buffers come back to [`CpalSink::service`] over
//! a second ring and are dropped there, on the engine's thread. (Until this
//! change the last `Arc<Pcm>` could be dropped in the callback; that deviation
//! is gone.)
//!
//! The *error* callback is cpal's, not the data path, and may drop the error it
//! is handed (a `BackendSpecific` one owns a `String`). It only counts and sets
//! flags; [`CpalSink::service`] does the logging.
//!
//! ## Device loss
//!
//! A stream is rebuilt on the **current default device** when the error
//! callback reports `DeviceNotAvailable`, when the callback heartbeat stops for
//! [`STALL`], or when the default output device changes (checked every
//! [`DEVICE_CHECK`], vanilla's `SoundEngine` checks once a second too). The
//! mixer — every voice, mid-sound — survives the rebuild: the closure hands its
//! state back when cpal drops it. Failed attempts back off from
//! [`BACKOFF_MIN`] to [`BACKOFF_MAX`].

use crate::buffers::PcmSource;
use crate::callback::{f32_to_i16, f32_to_u16, CallbackStats, RenderCallback};
use crate::device::{
    command_ring, ring, Command, CommandProducer, Consumer, DEFAULT_RING_CAPACITY,
};
use crate::live_sink::LiveSink;
use crate::mixer::{Mixer, Retired};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use rewo_net::sound_engine::{
    ChannelCall, ChannelId, ChannelSink, ListenerTransform, SinkDiagnostics,
};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// " (N similar message(s) held back)" for a rate-limited recovery log line.
fn held_note(held: u32) -> String {
    if held == 0 {
        String::new()
    } else {
        format!(" ({held} similar message(s) held back)")
    }
}

/// Retired values the callback can hand back between two drains.
///
/// A drain happens every engine tick (50 ms, several periods). One period can
/// retire at most one buffer per command drained plus one per queue join,
/// underrun and finished voice; the command ring is 1024, so 4096 covers a
/// period that drains a full command ring of stops on streaming voices with
/// room to spare. Overflow frees in place and is counted
/// ([`CallbackStats::freed_in_place`]).
pub const RETIRE_RING_CAPACITY: usize = 4096;

/// No callback for this long, while a stream is supposed to be running, is a
/// dead stream.
pub const STALL: Duration = Duration::from_secs(2);
/// How often the default output device is compared with the one in use.
pub const DEVICE_CHECK: Duration = Duration::from_secs(1);
pub const BACKOFF_MIN: Duration = Duration::from_millis(500);
pub const BACKOFF_MAX: Duration = Duration::from_secs(30);

/// A rebuilt stream must have its callbacks flowing this long before the
/// backoff is forgiven. Without it a device that builds fine and never calls
/// back is rebuilt every ~2.5 s forever, each rebuild a blocking build on the
/// engine thread.
pub const STABLE: Duration = Duration::from_secs(10);
/// While rebuilds are flapping (not yet proven stable), recovery log lines
/// appear at most this often; the rest are counted and reported with the next.
pub const LOG_INTERVAL: Duration = Duration::from_secs(30);

/// When to rebuild the stream. Pure, so the schedule is testable without a
/// device.
///
/// **The backoff is only forgiven by a stream that has run.** A rebuild that
/// succeeds (`rebuilt(ok)`) does not reset it: the next failure waits out the
/// escalated delay, and the delay is reset to [`BACKOFF_MIN`] only once the
/// heartbeat has been advancing for [`STABLE`] after a rebuild. Resetting at
/// "build ok" or at every healthy-to-unhealthy edge (the old rule) could not
/// tell a device that works from one that builds and then stays silent.
#[derive(Debug)]
pub struct Recovery {
    healthy: bool,
    last_heartbeat: u64,
    last_progress: Instant,
    next_attempt: Instant,
    backoff: Duration,
    next_device_check: Instant,
    /// When the heartbeat first advanced since the last (re)build; `None` until
    /// it does, and again after an unhealthy edge.
    flowing_since: Option<Instant>,
    /// A rebuild has succeeded and its stream has not yet run for [`STABLE`].
    unproven: bool,
    log_last: Option<Instant>,
    log_suppressed: u32,
}

impl Recovery {
    pub fn new(now: Instant) -> Recovery {
        Recovery {
            healthy: true,
            last_heartbeat: 0,
            last_progress: now,
            next_attempt: now,
            backoff: BACKOFF_MIN,
            next_device_check: now + DEVICE_CHECK,
            flowing_since: None,
            unproven: false,
            log_last: None,
            log_suppressed: 0,
        }
    }

    /// Whether the default device is due a comparison. Only while healthy: an
    /// unhealthy stream is being rebuilt onto the default device anyway.
    pub fn device_check_due(&mut self, now: Instant) -> bool {
        if self.healthy && now >= self.next_device_check {
            self.next_device_check = now + DEVICE_CHECK;
            return true;
        }
        false
    }

    /// Feed one observation; `true` means rebuild now.
    pub fn observe(
        &mut self,
        now: Instant,
        heartbeat: u64,
        lost: bool,
        default_changed: bool,
    ) -> bool {
        if heartbeat != self.last_heartbeat {
            self.last_heartbeat = heartbeat;
            self.last_progress = now;
            if self.healthy && self.flowing_since.is_none() {
                self.flowing_since = Some(now);
            }
        }
        if self.healthy {
            let stalled = now.duration_since(self.last_progress) >= STALL;
            if lost || stalled || default_changed {
                self.healthy = false;
                self.flowing_since = None;
                // Not `now` and not a reset backoff: an unproven stream's
                // failure waits out the delay `rebuilt` set.
                self.next_attempt = self.next_attempt.max(now);
            } else if self.flapping() {
                if let Some(since) = self.flowing_since {
                    if now.duration_since(since) >= STABLE {
                        self.unproven = false;
                        self.backoff = BACKOFF_MIN;
                        self.next_attempt = now;
                        self.log_last = None;
                    }
                }
            }
        }
        !self.healthy && now >= self.next_attempt
    }

    /// Record the outcome of a rebuild attempt.
    ///
    /// Either way the next attempt is `backoff` away and the backoff doubles;
    /// success only restores the healthy flag. [`Self::observe`] forgives the
    /// backoff once the stream has run for [`STABLE`].
    pub fn rebuilt(&mut self, now: Instant, ok: bool) {
        self.next_attempt = now + self.backoff;
        self.backoff = (self.backoff * 2).min(BACKOFF_MAX);
        if ok {
            self.healthy = true;
            self.unproven = true;
            self.last_progress = now;
            self.flowing_since = None;
            self.next_device_check = now + DEVICE_CHECK;
        }
    }

    pub fn healthy(&self) -> bool {
        self.healthy
    }

    /// Rebuilds are recurring: one has not yet been followed by [`STABLE`] of
    /// callbacks, or a failure has escalated the backoff.
    pub fn flapping(&self) -> bool {
        self.unproven || self.backoff > BACKOFF_MIN
    }

    /// How long until the next attempt may run.
    pub fn retry_in(&self, now: Instant) -> Duration {
        self.next_attempt.saturating_duration_since(now)
    }

    /// Whether a recovery log line may be printed now. `Some(n)` means yes, and
    /// `n` lines were held back since the last one; `None` means hold it back.
    /// Everything prints while not [`Self::flapping`]; while flapping, one line
    /// per [`LOG_INTERVAL`].
    pub fn log_gate(&mut self, now: Instant) -> Option<u32> {
        if !self.flapping() {
            return Some(std::mem::take(&mut self.log_suppressed));
        }
        let due = self
            .log_last
            .is_none_or(|t| now.duration_since(t) >= LOG_INTERVAL);
        if due {
            self.log_last = Some(now);
            Some(std::mem::take(&mut self.log_suppressed))
        } else {
            self.log_suppressed += 1;
            None
        }
    }
}

/// What the error callback leaves for the engine's thread.
#[derive(Default)]
struct DeviceFlags {
    errors: AtomicU64,
    /// Set by `DeviceNotAvailable`; taken by `service`.
    lost: AtomicBool,
    /// 1 = device not available, 2 = backend-specific.
    last_kind: AtomicU8,
}

impl DeviceFlags {
    fn record(&self, err: &cpal::Error) {
        self.errors.fetch_add(1, Ordering::Relaxed);
        if err.kind() == cpal::ErrorKind::DeviceNotAvailable {
            self.last_kind.store(1, Ordering::Relaxed);
            self.lost.store(true, Ordering::Relaxed);
        } else {
            self.last_kind.store(2, Ordering::Relaxed);
        }
    }
}

/// Where the render state goes when cpal drops the closure that owns it.
type Home = Arc<Mutex<Option<RenderCallback>>>;

/// Owns the [`RenderCallback`] inside cpal's closure and sends it home on drop.
///
/// That is what lets a rebuilt stream carry on every voice mid-sound: cpal
/// drops the closure when the stream is dropped (or fails to build), on a
/// thread where the callback can no longer run, and the lock is taken only
/// then — never in the callback.
struct Homing {
    cb: Option<RenderCallback>,
    home: Home,
}

impl Drop for Homing {
    fn drop(&mut self) {
        if let Ok(mut slot) = self.home.lock() {
            *slot = self.cb.take();
        }
    }
}

struct Built {
    stream: cpal::Stream,
    out_rate: u32,
    device_name: Option<String>,
    format: cpal::SampleFormat,
}

/// The device's human-readable name: what identifies "the default device" for
/// the change check, and what the log prints. cpal 0.18 replaced
/// `Device::name` with `description()` (the WASAPI FriendlyName) and `id()`.
fn device_name(device: &cpal::Device) -> Option<String> {
    device.description().ok().map(|d| d.name().to_string())
}

/// The device's default output config, keeping cpal 0.15's format preference.
///
/// cpal 0.18 ranks I32 and I24 above I16 when a device offers no f32, where
/// 0.15 picked f32, then i16, then u16. The render callback converts to those
/// three only, so a device that used to open as i16 would now be refused; this
/// asks for i16, then u16, at the same channel count and rate before letting
/// the caller refuse the format. (WASAPI's shared-mode mix format is f32, so on
/// Windows the first return is the only one taken.)
fn output_config(device: &cpal::Device) -> Result<cpal::SupportedStreamConfig, String> {
    use cpal::SampleFormat;
    let supported = device
        .default_output_config()
        .map_err(|e| format!("no default output config: {e}"))?;
    if matches!(
        supported.sample_format(),
        SampleFormat::F32 | SampleFormat::I16 | SampleFormat::U16
    ) {
        return Ok(supported);
    }
    let Ok(ranges) = device.supported_output_configs() else {
        return Ok(supported);
    };
    let ranges: Vec<_> = ranges
        .filter(|r| r.channels() == supported.channels())
        .collect();
    for format in [SampleFormat::I16, SampleFormat::U16] {
        if let Some(config) = ranges
            .iter()
            .filter(|r| r.sample_format() == format)
            .find_map(|r| r.try_with_sample_rate(supported.sample_rate()))
        {
            return Ok(config);
        }
    }
    Ok(supported)
}

/// Open the default device and start a stream over the state in `home`.
///
/// On failure the state is back in `home`, because the closure that took it
/// has been dropped.
fn build(host: &cpal::Host, home: &Home, flags: &Arc<DeviceFlags>) -> Result<Built, String> {
    let device = host
        .default_output_device()
        .ok_or_else(|| "no default output device".to_string())?;
    let device_name = device_name(&device);
    let supported = output_config(&device)?;
    // **Requires a stereo device.** The mixer renders interleaved stereo, and
    // silently playing a stereo mix into a mono or 5.1 device would come out
    // mangled rather than as an error. Supporting other layouts is a downmix
    // decision, not a bug fix.
    let channels = supported.channels();
    if channels != 2 {
        return Err(format!(
            "the mixer renders stereo and this device wants {channels} channels; \
             a downmix is a decision rather than a fix"
        ));
    }
    let out_rate = supported.sample_rate();
    let format = supported.sample_format();
    let config: cpal::StreamConfig = supported.into();

    let mut cb = home
        .lock()
        .map_err(|_| "audio render state lock poisoned".to_string())?
        .take()
        .ok_or_else(|| "audio render state is missing".to_string())?;
    cb.set_out_rate(out_rate);
    let mut homing = Homing {
        cb: Some(cb),
        home: Arc::clone(home),
    };
    let flags_cb = Arc::clone(flags);
    // **Counted, not logged.** A logger takes a lock and may allocate; the
    // engine's thread reads the flags and logs.
    let on_error = move |err: cpal::Error| flags_cb.record(&err);

    // The device's own format, converted in the callback when it is not f32.
    // The conversion is per sample into a preallocated scratch buffer — see
    // `RenderCallback::render_into` — so the fallback allocates nothing either.
    let stream = match format {
        cpal::SampleFormat::F32 => device.build_output_stream(
            config,
            move |data: &mut [f32], _: &cpal::OutputCallbackInfo| match homing.cb.as_mut() {
                Some(cb) => cb.render_f32(data),
                None => data.fill(0.0),
            },
            on_error,
            None,
        ),
        cpal::SampleFormat::I16 => device.build_output_stream(
            config,
            move |data: &mut [i16], _: &cpal::OutputCallbackInfo| match homing.cb.as_mut() {
                Some(cb) => cb.render_into(data, f32_to_i16),
                None => data.fill(0),
            },
            on_error,
            None,
        ),
        cpal::SampleFormat::U16 => device.build_output_stream(
            config,
            move |data: &mut [u16], _: &cpal::OutputCallbackInfo| match homing.cb.as_mut() {
                Some(cb) => cb.render_into(data, f32_to_u16),
                None => data.fill(32768),
            },
            on_error,
            None,
        ),
        other => {
            // `homing` drops on the way out and the state goes home.
            return Err(format!(
                "the device's default format is {other:?}; only f32, i16 and u16 are supported"
            ));
        }
    }
    .map_err(|e| format!("could not build the output stream: {e}"))?;
    stream
        .play()
        .map_err(|e| format!("could not start the stream: {e}"))?;
    Ok(Built {
        stream,
        out_rate,
        device_name,
        format,
    })
}

/// An open output stream, with the ring the engine pushes into.
///
/// Dropping it stops the stream — `cpal::Stream` is not `Send`, so this must
/// live on the thread that built it. [`Self::service`] must be called
/// regularly (the backend does it every engine tick): it frees what the
/// callback retired, logs device trouble and rebuilds a lost stream.
pub struct CpalSink {
    /// The producer end, unless a [`CpalBackend`] took it for its `LiveSink`.
    ring: Option<CommandProducer>,
    graveyard: Consumer<Retired>,
    flags: Arc<DeviceFlags>,
    stats: Arc<CallbackStats>,
    home: Home,
    host: cpal::Host,
    out_rate: u32,
    device_name: Option<String>,
    stream: Option<cpal::Stream>,
    recovery: Recovery,
    logged_errors: u64,
    logged: [u64; 4],
}

impl CpalSink {
    /// Open the default output device and start the stream.
    ///
    /// Takes the device's default sample format: `f32` natively, `i16` and
    /// `u16` converted in the callback. Anything else, or a channel count other
    /// than two, is refused with the reason in the message.
    pub fn open() -> Result<CpalSink, String> {
        let (producer, commands) = command_ring(DEFAULT_RING_CAPACITY);
        let (grave_tx, graveyard) = ring::<Retired>(RETIRE_RING_CAPACITY);
        let stats = Arc::new(CallbackStats::default());
        let flags = Arc::new(DeviceFlags::default());
        // 48 kHz until a device says otherwise; `build` sets the real rate
        // before the first callback.
        let cb = RenderCallback::new(Mixer::new(48_000), commands, grave_tx, Arc::clone(&stats));
        let home: Home = Arc::new(Mutex::new(Some(cb)));
        let host = cpal::default_host();
        let built = build(&host, &home, &flags)?;
        log::info!(
            "audio: output stream on {} at {} Hz, {:?}",
            built.device_name.as_deref().unwrap_or("<unnamed device>"),
            built.out_rate,
            built.format
        );
        Ok(CpalSink {
            ring: Some(producer),
            graveyard,
            flags,
            stats,
            home,
            host,
            out_rate: built.out_rate,
            device_name: built.device_name,
            stream: Some(built.stream),
            recovery: Recovery::new(Instant::now()),
            logged_errors: 0,
            logged: [0; 4],
        })
    }

    /// The producer end. One `push` per `ChannelCall`, and it never blocks.
    ///
    /// # Panics
    /// If a [`CpalBackend`] has taken it (see [`Self::take_ring`]).
    pub fn ring(&self) -> &CommandProducer {
        self.ring
            .as_ref()
            .expect("the command ring was taken by a CpalBackend")
    }

    /// Take the producer end, for a `LiveSink`. After a rebuild that could not
    /// recover the old ring, [`Self::service`] returns the new one.
    pub fn take_ring(&mut self) -> Option<CommandProducer> {
        self.ring.take()
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
        self.flags.errors.load(Ordering::Relaxed)
    }

    /// The callback's counters.
    pub fn stats(&self) -> &CallbackStats {
        &self.stats
    }

    /// Engine-thread housekeeping, once per tick: free retired buffers, log
    /// what the callback could not, and rebuild a lost stream.
    ///
    /// Returns a new producer if the stream was rebuilt over a fresh command
    /// ring and the old producer had been taken — the caller must hand it to
    /// whoever pushes commands. `None` otherwise.
    pub fn service(&mut self) -> Option<CommandProducer> {
        // The frees the callback was not allowed to do.
        while let Some(r) = self.graveyard.pop() {
            drop(r);
        }
        self.log_counters();

        let now = Instant::now();
        let heartbeat = self.stats.heartbeat.load(Ordering::Relaxed);
        let lost = self.flags.lost.swap(false, Ordering::Relaxed);
        let default_changed = self.recovery.device_check_due(now) && {
            let current = self
                .host
                .default_output_device()
                .and_then(|d| device_name(&d));
            current != self.device_name
        };
        let was_healthy = self.recovery.healthy();
        let rebuild = self.recovery.observe(now, heartbeat, lost, default_changed);
        // Said once per healthy-to-unhealthy edge, and rate-limited while
        // rebuilds are flapping (see `Recovery::log_gate`).
        if was_healthy && !self.recovery.healthy() {
            let cause = if lost {
                "the output device is no longer available"
            } else if default_changed {
                "the default output device changed; moving the stream"
            } else {
                "the output stream stalled (no callbacks)"
            };
            if let Some(held) = self.recovery.log_gate(now) {
                log::warn!("audio: {cause}{}", held_note(held));
            }
        }
        if rebuild {
            return self.rebuild(now);
        }
        None
    }

    fn log_counters(&mut self) {
        let errors = self.flags.errors.load(Ordering::Relaxed);
        if errors > self.logged_errors {
            let kind = match self.flags.last_kind.load(Ordering::Relaxed) {
                1 => "device not available",
                _ => "backend error",
            };
            log::warn!(
                "audio: {} stream error(s) from the device (last: {kind}); {errors} in total",
                errors - self.logged_errors
            );
            self.logged_errors = errors;
        }
        let s = &self.stats;
        let now = [
            s.ignored.load(Ordering::Relaxed),
            s.refused.load(Ordering::Relaxed),
            s.queue_overflow.load(Ordering::Relaxed),
            s.freed_in_place.load(Ordering::Relaxed),
        ];
        const WHAT: [&str; 4] = [
            "unresolved attach command(s) reached the mixer",
            "command(s) refused: every voice slot was taken",
            "stream buffer(s) dropped: a voice's queue was full",
            "buffer(s) freed on the audio thread: the retire ring was full",
        ];
        for i in 0..4 {
            // `>` rather than `!=`: a fresh mixer after a rebuild restarts at 0.
            if now[i] > self.logged[i] {
                log::warn!("audio: {} {}", now[i] - self.logged[i], WHAT[i]);
            }
            self.logged[i] = now[i];
        }
    }

    fn rebuild(&mut self, now: Instant) -> Option<CommandProducer> {
        // Dropping the stream drops cpal's closure, which sends the render
        // state — every voice — back into `home`.
        self.stream = None;
        let came_home = self.home.lock().map(|h| h.is_some()).unwrap_or(false);
        let mut fresh = None;
        if !came_home {
            // The backend did not hand the closure back. Start over with an
            // empty mixer on a new pair of rings; what was playing is lost,
            // which is also what vanilla's `SoundEngine.reload` does.
            log::warn!("audio: the old stream's state was not recovered; starting a fresh mixer");
            let (producer, commands) = command_ring(DEFAULT_RING_CAPACITY);
            let (grave_tx, graveyard) = ring::<Retired>(RETIRE_RING_CAPACITY);
            while let Some(r) = self.graveyard.pop() {
                drop(r);
            }
            self.graveyard = graveyard;
            let cb = RenderCallback::new(
                Mixer::new(self.out_rate),
                commands,
                grave_tx,
                Arc::clone(&self.stats),
            );
            match self.home.lock() {
                Ok(mut slot) => *slot = Some(cb),
                Err(poisoned) => *poisoned.into_inner() = Some(cb),
            }
            fresh = Some(producer);
        }
        match build(&self.host, &self.home, &self.flags) {
            Ok(b) => {
                self.recovery.rebuilt(now, true);
                if let Some(held) = self.recovery.log_gate(now) {
                    log::info!(
                        "audio: output stream rebuilt on {} at {} Hz, {:?}{}",
                        b.device_name.as_deref().unwrap_or("<unnamed device>"),
                        b.out_rate,
                        b.format,
                        held_note(held)
                    );
                }
                self.out_rate = b.out_rate;
                self.device_name = b.device_name;
                self.stream = Some(b.stream);
                self.flags.lost.store(false, Ordering::Relaxed);
            }
            Err(e) => {
                self.recovery.rebuilt(now, false);
                if let Some(held) = self.recovery.log_gate(now) {
                    log::warn!(
                        "audio: could not rebuild the output stream ({e}); next attempt in {:?}{}",
                        self.recovery.retry_in(now),
                        held_note(held)
                    );
                }
            }
        }
        let producer = fresh?;
        if self.ring.is_some() {
            self.ring = Some(producer);
            None
        } else {
            Some(producer)
        }
    }

    /// Convenience for a caller that has a decoded buffer and wants it heard.
    ///
    /// The order is vanilla's: properties, attach, play
    /// (`SoundEngine.java:417-434`). Returns `false` if any command was refused,
    /// which means the ring was full and the sequence is now incomplete — a
    /// caller that ignored this would leave a half-configured voice.
    pub fn play_once(
        &self,
        id: rewo_net::sound_engine::ChannelId,
        pcm: Arc<crate::buffers::Pcm>,
    ) -> bool {
        use rewo_net::sound_engine::ChannelCall as C;
        let ring = self.ring();
        let mut ok = true;
        ok &= ring.push(Command::Channel(id, C::SetPitch(1.0)));
        ok &= ring.push(Command::Channel(id, C::SetVolume(1.0)));
        ok &= ring.push(Command::Channel(id, C::DisableAttenuation));
        ok &= ring.push(Command::Channel(id, C::SetLooping(false)));
        ok &= ring.push(Command::Channel(id, C::SetSelfPosition(0.0, 0.0, 0.0)));
        ok &= ring.push(Command::Channel(id, C::SetRelative(true)));
        ok &= ring.push(Command::Attach(id, pcm));
        ok &= ring.push(Command::Channel(id, C::Play));
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
        let mut device = CpalSink::open()?;
        let ring = device
            .take_ring()
            .ok_or_else(|| "the device's command ring was already taken".to_string())?;
        let sink = LiveSink::new(ring, source);
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
        // Before the sink's own tick, so a ring replaced by a rebuild is the
        // one this tick's commands go into.
        if let Some(ring) = self.device.service() {
            self.sink.replace_ring(ring);
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn at(t0: Instant, ms: u64) -> Instant {
        t0 + Duration::from_millis(ms)
    }

    #[test]
    fn a_running_stream_is_left_alone() {
        let t0 = Instant::now();
        let mut r = Recovery::new(t0);
        for i in 1..100u64 {
            assert!(!r.observe(at(t0, i * 100), i, false, false));
        }
        assert!(r.healthy());
    }

    #[test]
    fn device_loss_rebuilds_at_once() {
        let t0 = Instant::now();
        let mut r = Recovery::new(t0);
        assert!(r.observe(at(t0, 10), 1, true, false));
        assert!(!r.healthy());
    }

    #[test]
    fn a_changed_default_device_rebuilds() {
        let t0 = Instant::now();
        let mut r = Recovery::new(t0);
        assert!(r.observe(at(t0, 10), 1, false, true));
    }

    #[test]
    fn a_stalled_heartbeat_rebuilds_after_the_stall_window_and_not_before() {
        let t0 = Instant::now();
        let mut r = Recovery::new(t0);
        assert!(!r.observe(at(t0, 100), 5, false, false));
        // Five callbacks, then silence.
        assert!(!r.observe(
            at(t0, 100) + STALL - Duration::from_millis(1),
            5,
            false,
            false
        ));
        assert!(r.observe(at(t0, 100) + STALL, 5, false, false));
    }

    #[test]
    fn failed_rebuilds_back_off_and_cap() {
        let t0 = Instant::now();
        let mut r = Recovery::new(t0);
        let mut now = at(t0, 10);
        assert!(r.observe(now, 0, true, false));
        let mut waits = Vec::new();
        for _ in 0..10 {
            r.rebuilt(now, false);
            // Not before the backoff...
            assert!(!r.observe(now + Duration::from_millis(1), 0, false, false));
            // ...and exactly at it.
            let wait = r.next_attempt.duration_since(now);
            waits.push(wait);
            now = r.next_attempt;
            assert!(r.observe(now, 0, false, false));
        }
        assert_eq!(waits[0], BACKOFF_MIN);
        assert_eq!(waits[1], BACKOFF_MIN * 2);
        assert!(waits.windows(2).all(|w| w[1] >= w[0]), "{waits:?}");
        assert_eq!(*waits.last().unwrap(), BACKOFF_MAX, "{waits:?}");
        // A success restores the healthy flag but does NOT forgive the backoff.
        r.rebuilt(now, true);
        assert!(r.healthy());
        assert!(!r.observe(now + Duration::from_millis(5), 1, false, false));
        assert_eq!(r.backoff, BACKOFF_MAX);
    }

    /// Drive `r` with a heartbeat that `beats(ms)` says is advancing or not,
    /// rebuilding (successfully) whenever asked. Returns the rebuild times.
    fn simulate(
        r: &mut Recovery,
        t0: Instant,
        from_ms: u64,
        to_ms: u64,
        beats: impl Fn(u64) -> bool,
    ) -> Vec<u64> {
        let mut rebuilds = Vec::new();
        let mut hb = r.last_heartbeat;
        let mut ms = from_ms;
        while ms <= to_ms {
            if beats(ms) {
                hb += 1;
            }
            let now = at(t0, ms);
            if r.observe(now, hb, false, false) {
                rebuilds.push(ms);
                r.rebuilt(now, true);
            }
            ms += 100;
        }
        rebuilds
    }

    /// Silent time forward from `from_ms` until a rebuild is due; performs it
    /// (successfully) and returns its time.
    fn until_rebuilt(r: &mut Recovery, t0: Instant, from_ms: u64) -> u64 {
        let mut ms = from_ms;
        loop {
            let now = at(t0, ms);
            if r.observe(now, r.last_heartbeat, false, false) {
                r.rebuilt(now, true);
                return ms;
            }
            ms += 100;
            assert!(ms < from_ms + 120_000, "no rebuild ever came due");
        }
    }

    /// A device that builds but never calls back is not rebuilt every ~2.5 s
    /// forever. The backoff used to reset on every healthy-to-unhealthy edge
    /// and on `rebuilt(ok)`, so the cadence never grew.
    #[test]
    fn a_stream_that_builds_but_stays_silent_backs_off_to_the_cap() {
        let t0 = Instant::now();
        let mut r = Recovery::new(t0);
        let rebuilds = simulate(&mut r, t0, 0, 300_000, |_| false);
        let gaps: Vec<u64> = rebuilds.windows(2).map(|w| w[1] - w[0]).collect();
        assert!(
            gaps.windows(2).all(|g| g[1] >= g[0]),
            "gaps must not shrink: {gaps:?}"
        );
        assert_eq!(
            *gaps.last().unwrap(),
            BACKOFF_MAX.as_millis() as u64,
            "{gaps:?}"
        );
        // The old behaviour was ~120 rebuilds in 300 s.
        assert!(
            rebuilds.len() < 20,
            "{} rebuilds: {rebuilds:?}",
            rebuilds.len()
        );
    }

    /// A stream whose callbacks flow for STABLE is proven, and only then is
    /// the backoff forgiven: the next outage rebuilds at once.
    #[test]
    fn a_stream_that_ran_for_the_stable_window_is_forgiven() {
        let t0 = Instant::now();
        let mut r = Recovery::new(t0);
        // Silent for a while, so the backoff has escalated.
        let rebuilds = simulate(&mut r, t0, 0, 30_000, |_| false);
        assert!(rebuilds.len() >= 3, "{rebuilds:?}");
        assert!(r.backoff > BACKOFF_MIN);
        let last = until_rebuilt(&mut r, t0, 30_100);
        // It starts calling back and keeps doing so for STABLE.
        let stable_ms = STABLE.as_millis() as u64;
        let flowing_until = last + stable_ms + 2_000;
        let more = simulate(&mut r, t0, last + 100, flowing_until, |_| true);
        assert!(more.is_empty(), "a flowing stream was rebuilt: {more:?}");
        assert_eq!(r.backoff, BACKOFF_MIN);
        assert!(!r.flapping());
        // Now it dies: the rebuild is immediate (loss), not a stale backoff away.
        let now = at(t0, flowing_until + 100);
        assert!(r.observe(now, r.last_heartbeat, true, false));
    }

    /// Callbacks that flow for less than STABLE and then stop earn nothing.
    #[test]
    fn a_stream_that_dies_before_the_stable_window_keeps_its_backoff() {
        let t0 = Instant::now();
        let mut r = Recovery::new(t0);
        let _ = simulate(&mut r, t0, 0, 30_000, |_| false);
        let last = until_rebuilt(&mut r, t0, 30_100);
        let backoff = r.backoff;
        let stable_ms = STABLE.as_millis() as u64;
        // Flows for 3 s short of STABLE, then the loss is reported.
        let flow_end = last + stable_ms - 3_000;
        let _ = simulate(&mut r, t0, last + 100, flow_end, |_| true);
        assert_eq!(r.backoff, backoff, "forgiven too early");
        assert!(r.flapping());
        // A lost device right now still waits out the backoff.
        let now = at(t0, flow_end + 100);
        assert!(!r.observe(now, r.last_heartbeat, true, false));
        assert!(r.retry_in(now) > Duration::ZERO);
    }

    #[test]
    fn recovery_logs_are_rate_limited_only_while_flapping() {
        let t0 = Instant::now();
        let mut r = Recovery::new(t0);
        // A healthy, never-rebuilt recovery logs everything.
        assert_eq!(r.log_gate(t0), Some(0));
        assert_eq!(r.log_gate(at(t0, 1)), Some(0));
        // After an unproven rebuild: first line passes, the rest are held back.
        r.rebuilt(at(t0, 2_000), true);
        assert!(r.flapping());
        assert_eq!(r.log_gate(at(t0, 2_000)), Some(0));
        assert_eq!(r.log_gate(at(t0, 4_000)), None);
        assert_eq!(r.log_gate(at(t0, 6_000)), None);
        let later = at(t0, 2_000) + LOG_INTERVAL;
        assert_eq!(r.log_gate(later), Some(2));
        assert_eq!(r.log_gate(later), None);
    }

    #[test]
    fn the_default_device_is_checked_once_a_second_while_healthy() {
        let t0 = Instant::now();
        let mut r = Recovery::new(t0);
        assert!(!r.device_check_due(at(t0, 500)));
        assert!(r.device_check_due(at(t0, 1000)));
        assert!(!r.device_check_due(at(t0, 1500)));
        assert!(r.device_check_due(at(t0, 2000)));
        // Not while a rebuild is pending: that targets the default anyway.
        assert!(r.observe(at(t0, 2100), 1, true, false));
        assert!(!r.device_check_due(at(t0, 5000)));
    }
}
