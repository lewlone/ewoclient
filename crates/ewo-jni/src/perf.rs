//! Opt-in render-thread profiler for the in-game HUD bridge.
//!
//! **Off by default — zero per-frame cost** unless a sentinel file exists at HUD
//! init. Two sentinels in the system temp dir (`%TEMP%` on Windows):
//!
//!   - `ewo-perf.on`  — enable instrumentation. Every frame records per-section
//!     CPU timings; every [`WINDOW`] frames a JSON-Lines summary is appended to
//!     `%TEMP%/ewo-perf.jsonl`. No visible change to the HUD.
//!   - `ewo-perf.ab`  — *additionally* run an A/B: alternate [`WINDOW`]-frame
//!     stretches of normal compositing ("full") and skipped compositing
//!     ("bypass" — the HUD blinks off). The frame-time delta between the two
//!     modes is the HUD's **true end-to-end cost**: context switches + composite
//!     + flush + any GPU-pipelining loss that per-section CPU timing can't see.
//!     The HUD visibly blinks ~once a second while this runs; that's expected.
//!
//! The summary file is truncated at session start, so it always holds just the
//! latest run. Each line is one window:
//! `{"t":12.3,"mode":"full","fps":312.4,"w":2560,"h":1440,"rate":"60",
//!   "paints":51,"frame_ms":{...},"injected_us":{...},"mc_to_us":{...}, ...}`

use std::fmt::Write as _;
use std::io::Write as _;
use std::time::Instant;

/// Frames per A/B half-window (and per summary line). ~1 s at 300 fps.
const WINDOW: u64 = 300;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Composite the HUD normally.
    Full,
    /// Skip the GPU composite this frame (A/B baseline — HUD blinks off).
    Bypass,
}

/// One section's rolling sum over the current window.
#[derive(Default, Clone, Copy)]
struct Stat {
    sum_ns: u64,
    max_ns: u64,
    n: u64,
}

impl Stat {
    #[inline]
    fn add(&mut self, ns: u64) {
        self.sum_ns += ns;
        if ns > self.max_ns {
            self.max_ns = ns;
        }
        self.n += 1;
    }
    fn mean_us(&self) -> f64 {
        if self.n == 0 {
            0.0
        } else {
            self.sum_ns as f64 / self.n as f64 / 1000.0
        }
    }
    fn max_us(&self) -> f64 {
        self.max_ns as f64 / 1000.0
    }
}

/// Section keys — indices into [`Perf::stats`].
#[derive(Clone, Copy)]
pub enum Sec {
    /// `wglMakeCurrent` → our dedicated context.
    McTo = 0,
    /// `gr.reset(None)` (resize-only; should be ~0 in steady state).
    Reset = 1,
    /// `paint()` — only counted on frames it actually painted (rate-gated).
    Paint = 2,
    /// Wrap fbo-0 in a Skia surface.
    Wrap = 3,
    /// Blit the offscreen HUD image onto fbo-0.
    Blit = 4,
    /// `flush_and_submit()`.
    Flush = 5,
    /// `wglMakeCurrent` → back to Minecraft.
    McBack = 6,
    /// Whole injected region (McTo … McBack) — the total CPU we add per frame.
    Injected = 7,
}
const NSEC: usize = 8;
const SEC_NAMES: [&str; NSEC] = [
    "mc_to", "reset", "paint", "wrap", "blit", "flush", "mc_back", "injected",
];

pub struct Perf {
    /// Instrumentation active (the `ewo-perf.on` sentinel existed at init).
    pub enabled: bool,
    /// A/B bypass cycling active (the `ewo-perf.ab` sentinel also existed).
    ab: bool,
    stats: [Stat; NSEC],
    /// Frame period (wall-clock between consecutive `frame()` entries).
    period: Stat,
    last_entry: Option<Instant>,
    /// Frames seen since enable (drives the window + A/B cadence).
    frame: u64,
    /// Paints in the current window (confirms the effective paint rate).
    paints: u64,
    w: i32,
    h: i32,
    rate: &'static str,
    started: Instant,
}

impl Perf {
    /// Read the sentinels once. When enabled, truncate the output file so it
    /// holds only this session.
    pub fn new() -> Perf {
        let dir = std::env::temp_dir();
        let enabled = dir.join("ewo-perf.on").exists();
        let ab = dir.join("ewo-perf.ab").exists();
        if enabled {
            let _ = std::fs::write(dir.join("ewo-perf.jsonl"), b"");
        }
        Perf {
            enabled,
            ab,
            stats: [Stat::default(); NSEC],
            period: Stat::default(),
            last_entry: None,
            frame: 0,
            paints: 0,
            w: 0,
            h: 0,
            rate: "",
            started: Instant::now(),
        }
    }

    /// Whether the A/B bypass cadence is active (the `ewo-perf.ab` sentinel).
    pub fn ab_enabled(&self) -> bool {
        self.ab
    }

    /// This frame's A/B mode. `Full` always unless the `ab` sentinel is set.
    #[inline]
    pub fn mode(&self) -> Mode {
        if self.ab && (self.frame / WINDOW) % 2 == 1 {
            Mode::Bypass
        } else {
            Mode::Full
        }
    }

    #[inline]
    pub fn rec(&mut self, s: Sec, ns: u64) {
        self.stats[s as usize].add(ns);
    }

    #[inline]
    pub fn note_paint(&mut self) {
        self.paints += 1;
    }

    /// Call at the top of every `frame()`. Records the frame period, advances
    /// the window/A-B counters, flushes a summary at window boundaries, and
    /// returns this frame's mode.
    pub fn begin_frame(&mut self, w: i32, h: i32, rate: &'static str) -> Mode {
        self.w = w;
        self.h = h;
        self.rate = rate;
        let now = Instant::now();
        if let Some(prev) = self.last_entry {
            self.period.add(now.duration_since(prev).as_nanos() as u64);
        }
        self.last_entry = Some(now);
        let mode = self.mode();
        self.frame += 1;
        if self.frame % WINDOW == 0 {
            self.flush_window();
        }
        mode
    }

    /// Append one window summary and reset the window accumulators.
    fn flush_window(&mut self) {
        // The window that just completed is the previous block.
        let block = self.frame / WINDOW;
        let mode = if self.ab && (block.wrapping_sub(1)) % 2 == 1 {
            "bypass"
        } else {
            "full"
        };
        let mean_us = self.period.mean_us();
        let fps = if mean_us > 0.0 { 1_000_000.0 / mean_us } else { 0.0 };

        let mut line = String::with_capacity(512);
        let _ = write!(
            line,
            "{{\"t\":{:.1},\"mode\":\"{}\",\"win\":{},\"fps\":{:.1},\"w\":{},\"h\":{},\"rate\":\"{}\",\"paints\":{},\"frame_ms\":{{\"mean\":{:.3},\"max\":{:.3}}}",
            self.started.elapsed().as_secs_f64(),
            mode,
            WINDOW,
            fps,
            self.w,
            self.h,
            self.rate,
            self.paints,
            mean_us / 1000.0,
            self.period.max_us() / 1000.0,
        );
        for i in 0..NSEC {
            let s = &self.stats[i];
            let _ = write!(
                line,
                ",\"{}_us\":{{\"mean\":{:.2},\"max\":{:.2},\"n\":{}}}",
                SEC_NAMES[i],
                s.mean_us(),
                s.max_us(),
                s.n,
            );
        }
        line.push('}');

        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(std::env::temp_dir().join("ewo-perf.jsonl"))
        {
            let _ = writeln!(f, "{line}");
        }

        self.stats = [Stat::default(); NSEC];
        self.period = Stat::default();
        self.paints = 0;
    }
}
