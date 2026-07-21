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

    /// The gaming "1% / 0.1% low": the MEAN of the slowest `q` fraction of
    /// frames (not the percentile *edge*). This is the frame-consistency
    /// north-star — it captures how bad the worst frames actually are, which
    /// a single percentile point hides.
    pub fn low_mean(&self, q: f32) -> f32 {
        if self.ms.is_empty() {
            return 0.0;
        }
        let mut sorted = self.ms.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let n = ((sorted.len() as f32) * q).ceil() as usize;
        let n = n.clamp(1, sorted.len());
        let tail = &sorted[sorted.len() - n..];
        tail.iter().sum::<f32>() / n as f32
    }

    pub fn len(&self) -> usize {
        self.ms.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ms.is_empty()
    }

    /// A compact ASCII histogram of frame times over `[0, max_ms]` in
    /// `buckets` bins, each row labeled by its upper edge + a bar + count.
    pub fn histogram(&self, buckets: usize, max_ms: f32) -> String {
        if self.ms.is_empty() {
            return "(no samples)".into();
        }
        let mut counts = vec![0usize; buckets];
        let mut over = 0usize;
        for &v in &self.ms {
            let b = ((v / max_ms) * buckets as f32) as usize;
            if b < buckets {
                counts[b] += 1;
            } else {
                over += 1;
            }
        }
        let peak = counts.iter().copied().max().unwrap_or(1).max(1);
        let mut out = String::new();
        for (i, &c) in counts.iter().enumerate() {
            let hi = (i + 1) as f32 * max_ms / buckets as f32;
            let bar = (c * 40 / peak).min(40);
            out.push_str(&format!(
                "  <={:5.1}ms |{}{} {}\n",
                hi,
                "#".repeat(bar),
                " ".repeat(40 - bar),
                c
            ));
        }
        if over > 0 {
            out.push_str(&format!("  > {max_ms:.1}ms |{:40} {over}\n", ""));
        }
        out
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
    fn low_mean_averages_the_worst_tail() {
        let mut s = StatsAccum::default();
        for i in 1..=1000 {
            s.push(i as f32);
        }
        // Worst 1% = the slowest 10 frames (991..=1000), mean 995.5.
        assert!((s.low_mean(0.01) - 995.5).abs() < 0.01);
        // Worst 0.1% = the single slowest frame (1000).
        assert_eq!(s.low_mean(0.001), 1000.0);
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
