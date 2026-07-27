//! System-audio capture → spectrum, for the media widget's visualiser.
//!
//! SMTC (see [`crate::media`]) reports *metadata* — title, artist, position —
//! and nothing about the sound itself. To make a widget move with the music
//! you need the actual samples, which means capturing the render endpoint.
//!
//! **Process loopback, excluding Minecraft.** A plain loopback capture takes
//! the whole system mix, which on this machine includes the game: every hit,
//! footstep and XP orb would shake the visualiser as hard as the music.
//! Windows 10 2004+ can capture a *process tree*, or everything except one, so
//! the capture excludes the JVM's own tree and what is left is the music.
//! When that activation fails — older Windows, or an odd audio stack — it
//! falls back to whole-mix loopback rather than showing nothing, and reports
//! which mode it got so the UI can say so.
//!
//! Everything here runs on a background thread and publishes immutable
//! snapshots. The render thread never blocks on audio.

use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};

/// Number of spectrum bands the widget draws. Log-spaced across the audible
/// range — few enough to read at a glance on a small widget, enough to show
/// the shape of a mix.
pub const BANDS: usize = 14;

/// FFT window. 1024 samples at 48 kHz is ~21 ms — fine enough to feel
/// immediate, long enough that the lowest band has a full cycle to work with.
const FFT_SIZE: usize = 1024;

/// Capture format. Process loopback does not support `GetMixFormat`, so the
/// format is stated rather than queried; Windows resamples into it.
const SAMPLE_RATE: u32 = 48_000;
const CHANNELS: u16 = 2;

/// Where the samples are coming from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// Not capturing — activation failed, or the platform has no backend.
    None,
    /// Everything except this process tree. What we ask for.
    ExcludingGame,
    /// The whole system mix, game audio included. The fallback.
    SystemMix,
}

/// One published analysis frame.
#[derive(Clone, Copy, Debug)]
pub struct Spectrum {
    /// Per-band magnitude, 0..1, already smoothed for display.
    pub bands: [f32; BANDS],
    /// Overall loudness, 0..1.
    pub level: f32,
    /// Onset pulse, 0..1 — spikes on a beat and decays. Drives anything that
    /// should *hit* rather than follow.
    pub pulse: f32,
    pub source: Source,
}

impl Spectrum {
    pub const SILENT: Spectrum = Spectrum {
        bands: [0.0; BANDS],
        level: 0.0,
        pulse: 0.0,
        source: Source::None,
    };

    /// Whether anything is actually being heard. The widget falls back to its
    /// own idle motion when this is false, so a muted game does not look
    /// broken.
    pub fn is_live(&self) -> bool {
        self.source != Source::None && self.level > 0.001
    }
}

/// Handle to the capture thread. Poll it once per frame.
pub struct AudioService {
    rx: Option<Receiver<Spectrum>>,
    /// Last value seen — the widget wants a value every frame, not only on
    /// the frames a new one happened to arrive.
    last: Spectrum,
    /// Frames since a snapshot arrived. Used to decay toward silence if the
    /// capture thread dies, rather than freezing the bars mid-song.
    stale: u32,
}

impl AudioService {
    /// Start capturing. Never fails loudly — a missing backend just means the
    /// visualiser stays still.
    pub fn start() -> Self {
        let (tx, rx) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("ewo-audio".into())
            .spawn(move || backend::run(tx));
        match spawned {
            Ok(_) => AudioService { rx: Some(rx), last: Spectrum::SILENT, stale: 0 },
            Err(e) => {
                crate::log(&format!("audio: capture thread failed to start: {e}"));
                AudioService { rx: None, last: Spectrum::SILENT, stale: 0 }
            }
        }
    }

    /// Latest analysis. Drains everything queued and keeps the newest — a
    /// visualiser wants the current value, not a backlog.
    pub fn poll(&mut self) -> Spectrum {
        let Some(rx) = &self.rx else {
            return Spectrum::SILENT;
        };
        let mut got = false;
        loop {
            match rx.try_recv() {
                Ok(s) => {
                    self.last = s;
                    got = true;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    // Thread is gone; stop pretending.
                    self.rx = None;
                    self.last = Spectrum::SILENT;
                    return self.last;
                }
            }
        }
        if got {
            self.stale = 0;
        } else {
            self.stale = self.stale.saturating_add(1);
            // ~half a second at any sane frame rate. Decay rather than freeze:
            // bars stuck mid-song read as a bug, bars settling read as silence.
            if self.stale > 240 {
                for b in self.last.bands.iter_mut() {
                    *b *= 0.90;
                }
                self.last.level *= 0.90;
                self.last.pulse *= 0.85;
            }
        }
        self.last
    }
}

impl Default for AudioService {
    fn default() -> Self {
        Self::start()
    }
}

// ────────────────────────────────────────────────────────────────────────
// Analysis — shared by both capture modes.
// ────────────────────────────────────────────────────────────────────────

/// Rolling analyser: accumulates interleaved samples, emits a [`Spectrum`]
/// every [`FFT_SIZE`] mono samples.
struct Analyser {
    /// Mono ring of the last FFT_SIZE samples.
    window: Vec<f32>,
    filled: usize,
    /// Hann window, precomputed.
    hann: Vec<f32>,
    /// Display-smoothed band values.
    smooth: [f32; BANDS],
    /// Moving average of low-band energy, for onset detection.
    low_avg: f32,
    pulse: f32,
    level: f32,
}

impl Analyser {
    fn new() -> Self {
        let hann = (0..FFT_SIZE)
            .map(|i| {
                let t = i as f32 / (FFT_SIZE - 1) as f32;
                0.5 - 0.5 * (std::f32::consts::TAU * t).cos()
            })
            .collect();
        Analyser {
            window: vec![0.0; FFT_SIZE],
            filled: 0,
            hann,
            smooth: [0.0; BANDS],
            low_avg: 0.0,
            pulse: 0.0,
            level: 0.0,
        }
    }

    /// Feed interleaved f32 frames. Returns a snapshot each time a window
    /// fills.
    fn feed(&mut self, interleaved: &[f32], channels: usize, out: &mut Vec<Spectrum>, source: Source) {
        if channels == 0 {
            return;
        }
        for frame in interleaved.chunks_exact(channels) {
            // Downmix to mono. A stereo-difference visualiser would be
            // prettier but would go dead on mono sources.
            let mono = frame.iter().sum::<f32>() / channels as f32;
            self.window[self.filled] = mono;
            self.filled += 1;
            if self.filled == FFT_SIZE {
                out.push(self.analyse(source));
                // 50% overlap: keeps the update rate at ~93 Hz without
                // doubling the FFT cost, and avoids the stutter a
                // non-overlapping window gives at this size.
                self.window.copy_within(FFT_SIZE / 2.., 0);
                self.filled = FFT_SIZE / 2;
            }
        }
    }

    fn analyse(&mut self, source: Source) -> Spectrum {
        // RMS before windowing — the honest loudness of the block.
        let rms = (self.window.iter().map(|s| s * s).sum::<f32>() / FFT_SIZE as f32).sqrt();
        // To dB, then map a musically useful range (-60..0 dBFS) onto 0..1.
        let db = 20.0 * (rms.max(1e-7)).log10();
        let target_level = ((db + 60.0) / 60.0).clamp(0.0, 1.0);
        // Asymmetric smoothing: jump up, ease down. Matches how a level meter
        // is expected to behave, and stops the widget flickering on transients.
        self.level += (target_level - self.level) * if target_level > self.level { 0.5 } else { 0.08 };

        let mut re: Vec<f32> = self
            .window
            .iter()
            .zip(&self.hann)
            .map(|(s, w)| s * w)
            .collect();
        let mut im = vec![0.0f32; FFT_SIZE];
        fft(&mut re, &mut im);

        // Log-spaced band edges from 40 Hz to 14 kHz. Linear bins would give
        // twelve bands of treble and two of everything you can actually hear.
        let bin_hz = SAMPLE_RATE as f32 / FFT_SIZE as f32;
        let (f_lo, f_hi) = (40.0f32, 14_000.0f32);
        let mut bands = [0.0f32; BANDS];
        for (b, slot) in bands.iter_mut().enumerate() {
            let t0 = b as f32 / BANDS as f32;
            let t1 = (b + 1) as f32 / BANDS as f32;
            let lo = f_lo * (f_hi / f_lo).powf(t0);
            let hi = f_lo * (f_hi / f_lo).powf(t1);
            let bin0 = ((lo / bin_hz) as usize).max(1);
            let bin1 = ((hi / bin_hz) as usize).min(FFT_SIZE / 2 - 1).max(bin0);
            let mut peak = 0.0f32;
            for k in bin0..=bin1 {
                let mag = (re[k] * re[k] + im[k] * im[k]).sqrt();
                peak = peak.max(mag);
            }
            // Magnitude → dB → 0..1 over a 70 dB window. Peak rather than mean
            // across the band: a mean washes out narrow tones into nothing.
            let bdb = 20.0 * (peak.max(1e-7) / (FFT_SIZE as f32 * 0.25)).log10();
            *slot = ((bdb + 70.0) / 70.0).clamp(0.0, 1.0);
        }

        // Display smoothing, same asymmetry as the level.
        for (s, target) in self.smooth.iter_mut().zip(bands.iter()) {
            let k = if *target > *s { 0.55 } else { 0.12 };
            *s += (*target - *s) * k;
        }

        // Onset: low-band energy jumping well above its own moving average.
        // Crude next to a real beat tracker, but it fires on kicks, which is
        // what "vibrates with the song" actually means.
        let low: f32 = bands[..BANDS / 4].iter().copied().fold(0.0, f32::max);
        if low > self.low_avg * 1.35 + 0.05 {
            self.pulse = 1.0;
        }
        self.low_avg += (low - self.low_avg) * 0.10;
        self.pulse *= 0.86;

        Spectrum {
            bands: self.smooth,
            level: self.level,
            pulse: self.pulse,
            source,
        }
    }
}

/// In-place radix-2 Cooley–Tukey FFT. `re`/`im` must be [`FFT_SIZE`] long and
/// a power of two.
///
/// Hand-rolled rather than pulling in `rustfft`: this runs on 1024 points a
/// hundred times a second, which is nothing, and the dependency would be the
/// largest thing in the crate for the sake of ~40 lines.
fn fft(re: &mut [f32], im: &mut [f32]) {
    let n = re.len();
    debug_assert!(n.is_power_of_two());

    // Bit-reversal permutation.
    let mut j = 0usize;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }

    let mut len = 2;
    while len <= n {
        let ang = -std::f32::consts::TAU / len as f32;
        let (wr, wi) = (ang.cos(), ang.sin());
        let mut i = 0;
        while i < n {
            let (mut cr, mut ci) = (1.0f32, 0.0f32);
            for k in 0..len / 2 {
                let (ur, ui) = (re[i + k], im[i + k]);
                let (xr, xi) = (re[i + k + len / 2], im[i + k + len / 2]);
                let (vr, vi) = (xr * cr - xi * ci, xr * ci + xi * cr);
                re[i + k] = ur + vr;
                im[i + k] = ui + vi;
                re[i + k + len / 2] = ur - vr;
                im[i + k + len / 2] = ui - vi;
                let ncr = cr * wr - ci * wi;
                ci = cr * wi + ci * wr;
                cr = ncr;
            }
            i += len;
        }
        len <<= 1;
    }
}

// ────────────────────────────────────────────────────────────────────────
// WASAPI backend.
// ────────────────────────────────────────────────────────────────────────

/// Non-Windows stand-in. The visualiser falls back to its own idle motion, so
/// a build without a capture backend is quiet rather than broken.
#[cfg(not(windows))]
mod backend {
    use super::{Sender, Spectrum};

    pub(super) fn run(_tx: Sender<Spectrum>) {}
}

#[cfg(windows)]
mod backend {
    use super::{Analyser, Sender, Source, Spectrum, CHANNELS, SAMPLE_RATE};

    use std::sync::mpsc::Sender as Tx;

    use windows::core::{implement, Interface, GUID, PCWSTR};
    use windows::Win32::Foundation::{HANDLE, S_OK, WAIT_OBJECT_0};
    use windows::Win32::Media::Audio::{
        eConsole, eRender, ActivateAudioInterfaceAsync, IActivateAudioInterfaceAsyncOperation,
        IActivateAudioInterfaceCompletionHandler, IActivateAudioInterfaceCompletionHandler_Impl,
        IAudioCaptureClient, IAudioClient, IMMDeviceEnumerator, MMDeviceEnumerator,
        AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_EVENTCALLBACK, AUDCLNT_STREAMFLAGS_LOOPBACK,
        AUDIOCLIENT_ACTIVATION_PARAMS, AUDIOCLIENT_ACTIVATION_PARAMS_0,
        AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK, AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS,
        PROCESS_LOOPBACK_MODE_EXCLUDE_TARGET_PROCESS_TREE, WAVEFORMATEX,
    };
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_ALL, COINIT_MULTITHREADED,
    };
    use windows::Win32::System::Threading::{
        CreateEventW, GetCurrentProcessId, SetEvent, WaitForSingleObject,
    };

    /// The pseudo-device that process loopback activates against.
    const VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK: &str = "VAD\\Process_Loopback";

    /// `VT_BLOB`.
    const VT_BLOB: u16 = 65;

    /// `WAVE_FORMAT_IEEE_FLOAT` — declared in mmreg.h, which the crate exposes
    /// only under feature combinations we do not otherwise need.
    const WAVE_FORMAT_IEEE_FLOAT: u16 = 0x0003;

    /// A `PROPVARIANT` holding a `BLOB`, laid out by hand.
    ///
    /// `windows-core` keeps `PROPVARIANT`'s innards private and offers no
    /// blob constructor, so the activation params — which the API only accepts
    /// as a blob — have to be assembled at this level. The compile-time size
    /// assertion below is the guard: if the crate's layout ever changes, this
    /// fails to build instead of passing garbage to the audio stack.
    #[repr(C)]
    struct BlobPropVariant {
        vt: u16,
        r1: u16,
        r2: u16,
        r3: u16,
        cb_size: u32,
        _pad: u32,
        p_blob: *mut u8,
    }

    const _: () = assert!(
        core::mem::size_of::<BlobPropVariant>()
            == core::mem::size_of::<windows::core::PROPVARIANT>(),
        "PROPVARIANT layout changed — the hand-built activation blob is no longer valid"
    );

    /// Completion callback for `ActivateAudioInterfaceAsync`. Its only job is
    /// to signal the event the capture thread is parked on.
    #[implement(IActivateAudioInterfaceCompletionHandler)]
    struct ActivationHandler {
        done: HANDLE,
    }

    impl IActivateAudioInterfaceCompletionHandler_Impl for ActivationHandler_Impl {
        fn ActivateCompleted(
            &self,
            _op: Option<&IActivateAudioInterfaceAsyncOperation>,
        ) -> windows::core::Result<()> {
            unsafe {
                let _ = SetEvent(self.done);
            }
            Ok(())
        }
    }

    /// Capture-thread entry point.
    pub(super) fn run(tx: Sender<Spectrum>) {
        unsafe {
            // MTA: this thread does nothing but audio, and the WASAPI objects
            // are not shared with Minecraft's STA render thread.
            if CoInitializeEx(None, COINIT_MULTITHREADED).is_err() {
                super::super::log("audio: CoInitializeEx failed");
                return;
            }
            let result = capture_loop(&tx);
            if let Err(e) = result {
                super::super::log(&format!("audio: capture ended: {e}"));
            }
            CoUninitialize();
        }
    }

    unsafe fn capture_loop(tx: &Tx<Spectrum>) -> windows::core::Result<()> {
        // Preferred: everything except our own process tree.
        let (client, source) = match activate_process_loopback() {
            Ok(c) => (c, Source::ExcludingGame),
            Err(e) => {
                super::super::log(&format!(
                    "audio: process loopback unavailable ({e}) — falling back to the system mix, \
                     game audio will drive the visualiser too"
                ));
                (activate_system_loopback()?, Source::SystemMix)
            }
        };

        let format = wave_format();
        let event = CreateEventW(None, false, false, PCWSTR::null())?;

        // 200 ms buffer. Generous: a starved capture drops audio, and this
        // thread has no realtime priority.
        client.Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            AUDCLNT_STREAMFLAGS_LOOPBACK | AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
            2_000_000,
            0,
            &format,
            None,
        )?;
        client.SetEventHandle(event)?;
        let capture: IAudioCaptureClient = client.GetService()?;
        client.Start()?;

        let mut analyser = Analyser::new();
        let mut out: Vec<Spectrum> = Vec::new();

        loop {
            if WaitForSingleObject(event, 2_000) != WAIT_OBJECT_0 {
                // No audio for two seconds. Not an error — nothing is playing.
                // Publish silence so the bars settle instead of freezing.
                if tx.send(Spectrum { source, ..Spectrum::SILENT }).is_err() {
                    return Ok(());
                }
                continue;
            }
            loop {
                match capture.GetNextPacketSize() {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {}
                }
                let mut data: *mut u8 = std::ptr::null_mut();
                let mut frames = 0u32;
                let mut flags = 0u32;
                if capture
                    .GetBuffer(&mut data, &mut frames, &mut flags, None, None)
                    .is_err()
                {
                    break;
                }
                if !data.is_null() && frames > 0 {
                    let n = frames as usize * CHANNELS as usize;
                    let samples = std::slice::from_raw_parts(data as *const f32, n);
                    analyser.feed(samples, CHANNELS as usize, &mut out, source);
                }
                let _ = capture.ReleaseBuffer(frames);
            }
            // Publish only the newest — the UI wants current, not history.
            if let Some(latest) = out.pop() {
                out.clear();
                if tx.send(latest).is_err() {
                    return Ok(()); // UI dropped the receiver; we are done.
                }
            }
        }
    }

    /// Activate a loopback client that captures everything *except* this
    /// process tree — i.e. the music without the game.
    unsafe fn activate_process_loopback() -> windows::core::Result<IAudioClient> {
        let mut params = AUDIOCLIENT_ACTIVATION_PARAMS {
            ActivationType: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
            Anonymous: AUDIOCLIENT_ACTIVATION_PARAMS_0 {
                ProcessLoopbackParams: AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS {
                    TargetProcessId: GetCurrentProcessId(),
                    ProcessLoopbackMode: PROCESS_LOOPBACK_MODE_EXCLUDE_TARGET_PROCESS_TREE,
                },
            },
        };
        let blob = BlobPropVariant {
            vt: VT_BLOB,
            r1: 0,
            r2: 0,
            r3: 0,
            cb_size: core::mem::size_of::<AUDIOCLIENT_ACTIVATION_PARAMS>() as u32,
            _pad: 0,
            p_blob: (&mut params) as *mut _ as *mut u8,
        };

        let done = CreateEventW(None, false, false, PCWSTR::null())?;
        let handler: IActivateAudioInterfaceCompletionHandler =
            ActivationHandler { done }.into();

        let device: Vec<u16> = VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();

        let op = ActivateAudioInterfaceAsync(
            PCWSTR(device.as_ptr()),
            &IAudioClient::IID as *const GUID,
            Some((&blob) as *const _ as *const windows::core::PROPVARIANT),
            &handler,
        )?;

        // The activation is asynchronous but there is nothing else for this
        // thread to do until it lands.
        if WaitForSingleObject(done, 3_000) != WAIT_OBJECT_0 {
            return Err(windows::core::Error::from_win32());
        }

        let mut hr = S_OK;
        let mut unknown: Option<windows::core::IUnknown> = None;
        op.GetActivateResult(&mut hr, &mut unknown)?;
        hr.ok()?;
        unknown
            .ok_or_else(windows::core::Error::from_win32)?
            .cast::<IAudioClient>()
    }

    /// Plain whole-system loopback on the default render endpoint. Includes
    /// the game's own audio — the fallback when process loopback is refused.
    unsafe fn activate_system_loopback() -> windows::core::Result<IAudioClient> {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let device = enumerator.GetDefaultAudioEndpoint(eRender, eConsole)?;
        device.Activate::<IAudioClient>(CLSCTX_ALL, None)
    }

    /// The format both paths capture in. 32-bit float stereo at 48 kHz —
    /// stated rather than queried, because process loopback does not implement
    /// `GetMixFormat`.
    fn wave_format() -> WAVEFORMATEX {
        let bits = 32u16;
        let block_align = CHANNELS * bits / 8;
        WAVEFORMATEX {
            wFormatTag: WAVE_FORMAT_IEEE_FLOAT as u16,
            nChannels: CHANNELS,
            nSamplesPerSec: SAMPLE_RATE,
            nAvgBytesPerSec: SAMPLE_RATE * block_align as u32,
            nBlockAlign: block_align,
            wBitsPerSample: bits,
            cbSize: 0,
        }
    }
}
