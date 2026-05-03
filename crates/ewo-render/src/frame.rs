//! Frame clock — wall-time, dt, frame counter. Fed to every animated layer.
//!
//! Build-sequence step 3 introduces this. Every backdrop layer animates, so
//! we track elapsed seconds since startup and the per-frame dt.

use std::time::Instant;

const FPS_WINDOW: usize = 60;

pub struct Clock {
    start: Instant,
    last_frame: Instant,
    /// Seconds since `Clock::new()`. Wraps once we hit f32 precision limits at
    /// ~277 hours of uptime — fine for an interactive launcher.
    pub elapsed: f32,
    /// Seconds since the previous `tick()`.
    pub dt: f32,
    pub frame_count: u64,
    /// Rolling buffer of the last 60 frame deltas. `tick()` writes the
    /// current dt at `dt_idx` and advances modulo `FPS_WINDOW`.
    dt_window: [f32; FPS_WINDOW],
    dt_idx: usize,
    dt_count: usize,
}

impl Default for Clock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock {
    pub fn new() -> Self {
        let now = Instant::now();
        Self {
            start: now,
            last_frame: now,
            elapsed: 0.0,
            dt: 0.0,
            frame_count: 0,
            dt_window: [0.0; FPS_WINDOW],
            dt_idx: 0,
            dt_count: 0,
        }
    }

    pub fn tick(&mut self) {
        let now = Instant::now();
        self.elapsed = now.duration_since(self.start).as_secs_f32();
        self.dt = now.duration_since(self.last_frame).as_secs_f32();
        self.last_frame = now;
        self.frame_count = self.frame_count.wrapping_add(1);

        if self.dt > 0.0 && self.dt < 1.0 {
            self.dt_window[self.dt_idx] = self.dt;
            self.dt_idx = (self.dt_idx + 1) % FPS_WINDOW;
            self.dt_count = (self.dt_count + 1).min(FPS_WINDOW);
        }
    }

    /// Average frame delta over the rolling window, in seconds. Returns 0
    /// before the first frame is recorded.
    pub fn avg_dt(&self) -> f32 {
        if self.dt_count == 0 {
            return 0.0;
        }
        let sum: f32 = self.dt_window.iter().take(self.dt_count).sum();
        sum / self.dt_count as f32
    }

    /// Average FPS over the rolling window. `0.0` until the first frame.
    pub fn avg_fps(&self) -> f32 {
        let dt = self.avg_dt();
        if dt > 0.0 {
            1.0 / dt
        } else {
            0.0
        }
    }

    /// Worst frame delta seen in the rolling window (~p100 of the last 60
    /// frames). Useful for catching hitches the average smooths over.
    pub fn worst_dt(&self) -> f32 {
        self.dt_window
            .iter()
            .take(self.dt_count)
            .copied()
            .fold(0.0_f32, f32::max)
    }
}
