//! Frame-time accounting: the overlay's 240-sample ring + the run-long
//! accumulator that the end-of-run summary (and later the M6 merge gate)
//! reads percentiles from.

use rewo_gpu::overlay::OVERLAY_SAMPLES;

/// Ring feeding the on-screen strip chart. `head` is the next write slot,
/// which after wraparound is also the OLDEST sample — exactly what the
/// shader wants as its left edge.
pub struct OverlayRing {
    pub data: [f32; OVERLAY_SAMPLES],
    head: usize,
}

impl Default for OverlayRing {
    fn default() -> Self {
        Self {
            data: [0.0; OVERLAY_SAMPLES],
            head: 0,
        }
    }
}

impl OverlayRing {
    pub fn push(&mut self, ms: f32) {
        self.data[self.head] = ms;
        self.head = (self.head + 1) % OVERLAY_SAMPLES;
    }

    /// Index of the oldest sample (the shader's left edge).
    pub fn head(&self) -> u32 {
        self.head as u32
    }

    /// Deterministic animated test pattern for the headless PNG check:
    /// a sine sweep through all four budget colors plus periodic "hitch"
    /// spikes, so bars, colors, and both gridlines are all exercised.
    pub fn fill_demo(&mut self, t: f32) {
        for i in 0..OVERLAY_SAMPLES {
            let phase = t * 1.5 + i as f32 * 0.09;
            let mut ms = 2.0 + 7.0 * (1.0 + phase.sin()) / 2.0;
            if i % 37 == 0 {
                ms = 18.5; // ember spike
            }
            self.data[i] = ms;
        }
        self.head = 0;
    }
}

/// Whole-run accumulator. Frame times in ms, order preserved.
#[derive(Default)]
pub struct StatsAccum {
    pub ms: Vec<f32>,
}

impl StatsAccum {
    pub fn push(&mut self, ms: f32) {
        // A multi-second sample is a pause (debugger, driver dialog), not a
        // frame — recording it would poison the percentiles.
        if ms > 0.0 && ms < 5_000.0 {
            self.ms.push(ms);
        }
    }

    /// Nearest-rank percentile: the smallest sample ≥ q of the population.
    pub fn percentile(&self, q: f32) -> f32 {
        if self.ms.is_empty() {
            return 0.0;
        }
        let mut sorted = self.ms.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let rank = (q * sorted.len() as f32).ceil() as usize;
        sorted[rank.clamp(1, sorted.len()) - 1]
    }

    pub fn average(&self) -> f32 {
        if self.ms.is_empty() {
            return 0.0;
        }
        self.ms.iter().sum::<f32>() / self.ms.len() as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percentiles_pick_the_tail() {
        let mut s = StatsAccum::default();
        for i in 1..=100 {
            s.push(i as f32);
        }
        assert_eq!(s.percentile(0.0), 1.0);
        assert_eq!(s.percentile(0.5), 50.0);
        assert_eq!(s.percentile(0.99), 99.0);
        assert_eq!(s.percentile(1.0), 100.0);
    }

    #[test]
    fn pause_samples_are_dropped() {
        let mut s = StatsAccum::default();
        s.push(16.0);
        s.push(9_000.0); // debugger pause — must not poison percentiles
        assert_eq!(s.ms.len(), 1);
    }

    #[test]
    fn ring_head_is_oldest() {
        let mut r = OverlayRing::default();
        for i in 0..(OVERLAY_SAMPLES + 3) {
            r.push(i as f32);
        }
        // After capacity+3 pushes the head points at the slot holding the
        // oldest surviving sample (index 3).
        assert_eq!(r.data[r.head() as usize], 3.0);
    }
}
