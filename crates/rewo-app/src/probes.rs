//! A counter of graded comparisons, for the gates that accumulate property
//! failures instead of recording named witnesses (`skyshot`, `lightmapshot`,
//! `tintshot`).
//!
//! Those gates are one long run of `if <bad> { failures.push(..) }`, so a
//! section that renders but stops comparing (an emptied loop, a skipped branch,
//! a deleted `if`) fails nothing. A frame count does not catch that either: it
//! proves frames were read back, not that anything was graded on them. Wrapping
//! each failure condition in [`ProbeCounter::bad`] counts it every time it is
//! *evaluated*, and the gate pins the total with `!=`, like `EXPECTED_WITNESSES`
//! elsewhere. A comparison that is no longer reached moves the count.
//!
//! What it does not do: a probe that always evaluates false still counts, so
//! this guards "the comparison ran", and a control that must find the opposite
//! is still the author's job.

use std::sync::atomic::{AtomicUsize, Ordering};

pub struct ProbeCounter(AtomicUsize);

impl ProbeCounter {
    pub const fn new() -> Self {
        Self(AtomicUsize::new(0))
    }

    /// Count one graded comparison and hand back whether it found a fault.
    pub fn bad(&self, bad: bool) -> bool {
        self.0.fetch_add(1, Ordering::Relaxed);
        bad
    }

    pub fn count(&self) -> usize {
        self.0.load(Ordering::Relaxed)
    }

    /// Print `[gate] probes graded: N / EXPECTED` and fail unless N is exactly
    /// the pinned count. Update the pin, and say why, when a comparison is
    /// added or removed on purpose.
    pub fn check(&self, gate: &str, expected: usize) -> Result<(), String> {
        let got = self.count();
        println!("[{gate}] probes graded: {got} / {expected}");
        if got != expected {
            return Err(format!(
                "{gate}: graded {got} comparisons, expected {expected} — a comparison was skipped \
                 or added without updating the pin (fail-closed)"
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::ProbeCounter;

    #[test]
    fn bad_returns_its_argument_and_counts_every_evaluation() {
        let p = ProbeCounter::new();
        assert!(p.bad(true));
        assert!(!p.bad(false));
        assert_eq!(p.count(), 2);
    }

    #[test]
    fn check_fails_on_fewer_and_on_more() {
        let p = ProbeCounter::new();
        p.bad(false);
        p.bad(false);
        assert!(p.check("t", 2).is_ok());
        assert!(p.check("t", 3).is_err(), "a skipped comparison must fail");
        assert!(p.check("t", 1).is_err(), "an unpinned comparison must fail");
    }
}
