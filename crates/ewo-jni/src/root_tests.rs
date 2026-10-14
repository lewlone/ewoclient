//! Tests for the crate root's private helpers (log limiter, clock).

use super::*;
use std::time::Duration;

#[test]
fn identical_messages_in_the_window_are_collapsed_and_summarised() {
    let t0 = Instant::now();
    let mut lim = LogLimiter::new();

    let first = lim.admit("wglMakeCurrent failed", t0);
    assert_eq!(first.line.as_deref(), Some("wglMakeCurrent failed"));
    assert_eq!(first.summary, None);

    // A per-frame failure: 1000 repeats inside the window write nothing.
    for i in 1..=1000u64 {
        let r = lim.admit("wglMakeCurrent failed", t0 + Duration::from_millis(i));
        assert_eq!(r.line, None, "repeat {i} must be suppressed");
        assert_eq!(r.summary, None);
    }

    // A different message flushes the count before itself.
    let next = lim.admit("something else", t0 + Duration::from_secs(1));
    assert_eq!(next.line.as_deref(), Some("something else"));
    assert_eq!(
        next.summary.as_deref(),
        Some("(previous message repeated 1000 more times)")
    );
}

#[test]
fn a_repeat_after_the_window_is_logged_again() {
    let t0 = Instant::now();
    let mut lim = LogLimiter::new();
    lim.admit("same", t0);
    assert_eq!(lim.admit("same", t0 + Duration::from_secs(1)).line, None);
    let later = lim.admit("same", t0 + LOG_REPEAT_WINDOW + Duration::from_secs(1));
    assert_eq!(later.line.as_deref(), Some("same"));
    assert_eq!(
        later.summary.as_deref(),
        Some("(previous message repeated 1 more times)")
    );
}

#[test]
fn distinct_messages_are_never_suppressed() {
    let t0 = Instant::now();
    let mut lim = LogLimiter::new();
    for (i, m) in ["a", "b", "a", "b"].into_iter().enumerate() {
        let r = lim.admit(m, t0 + Duration::from_millis(i as u64));
        assert_eq!(r.line.as_deref(), Some(m));
        assert_eq!(r.summary, None);
    }
}

#[test]
fn the_clock_is_f64_and_monotonic() {
    let a: f64 = elapsed_secs();
    let b: f64 = elapsed_secs();
    assert!(b >= a);
    // The reason it is f64: an f32 near 20_000 s (5.5 h) cannot step by 0.5 ms,
    // so the paint/capture/frost gates would quantise further the longer the
    // game has been open.
    let t = 20_000.0_f32;
    assert_eq!(t + 0.0005, t, "f32 cannot represent a 0.5 ms step here");
    assert_ne!(20_000.0_f64 + 0.0005, 20_000.0_f64);
}
