//! Reduced motion (non-negotiable #6): the one place that decides how long a
//! transition takes and whether ambient motion runs.
//!
//! # What the prototype does
//!
//! `StyleSheet1` ("ACCESSIBILITY / MOTION REDUCTION", the last block, ~L1563):
//!
//! ```css
//! @media (prefers-reduced-motion: reduce) {
//!   * {
//!     animation-duration: 0.01ms !important;
//!     animation-iteration-count: 1 !important;
//!     transition-duration: 0.01ms !important;
//!   }
//! }
//! ```
//!
//! Two consequences, both mirrored here:
//!
//! 1. **Every finite animation and transition (modal / dropdown / button /
//!    tab / status entrances, hover ramps, ripples, progress-bar recede and
//!    ring) finishes in 0.01 ms**, i.e. it lands on its end state at the next
//!    frame. [`dur`] / [`effective_duration`] return that 0.01 ms for any
//!    base duration; every transition in the renderer divides by `dur(..)`
//!    instead of a literal.
//! 2. **Every infinite ambient loop (velvet folds, caustics, bokeh, glass
//!    tint breath, glass rims, breathing text, button rim / sheen, progress
//!    flow, firefly halo, meta-pill pulse) runs exactly one 0.01 ms
//!    iteration, then stops.** None of those animations set `animation-fill-mode`,
//!    so once the iteration ends the element falls back to its *un-animated
//!    base style*. Ambient motion is therefore frozen, not hidden. [`ambient_time`]
//!    returns phase 0 for the sites whose base style equals the 0 % keyframe,
//!    and the few layers whose base style differs (bokeh, folds, button rim,
//!    firefly, meta dot) branch on [`is_reduced`] and draw that base style.
//!
//! Deliberate deviations from the CSS (both in the user's favour):
//!
//! * `animation-delay` is *not* overridden by the media query, so the
//!   prototype's staggered rows would still wait up to a few hundred ms.
//!   Non-negotiable #6 says "all animations collapse to under 1 ms", so
//!   stagger delays go through [`dur`] too.
//! * Pearl dust and petals are `requestAnimationFrame` canvases in the
//!   prototype, which the CSS media query cannot reach (it never calls
//!   `matchMedia`). They are ambient motion, so the native port freezes them
//!   too (`Backdrop::update` receives `dt = 0`) and skips the celebrate burst.
//!
//! # Wiring
//!
//! The flag is a process-wide atomic: the render crate's draw functions are
//! free functions with no shared context, and the value is a single
//! user-environment fact (Windows `SPI_GETCLIENTAREAANIMATION`), not per-window
//! state. The launcher feeds it with [`Watcher`]; nothing else writes it.

use std::sync::atomic::{AtomicBool, Ordering};

/// The CSS `0.01ms`, in seconds.
pub const REDUCED_DURATION_SECS: f32 = 0.01 / 1000.0;

/// How often the launcher re-reads the OS setting.
pub const POLL_INTERVAL_SECS: f32 = 1.0;

static REDUCED: AtomicBool = AtomicBool::new(false);

/// Set the process-wide reduced-motion flag.
pub fn set_reduced(on: bool) {
    REDUCED.store(on, Ordering::Relaxed);
}

/// Whether reduced motion is currently on.
pub fn is_reduced() -> bool {
    REDUCED.load(Ordering::Relaxed)
}

/// Pure core of [`dur`]: the duration a transition of `base` seconds takes.
pub fn effective_duration(base: f32, reduced: bool) -> f32 {
    if reduced {
        base.min(REDUCED_DURATION_SECS)
    } else {
        base
    }
}

/// Duration (seconds) of a transition whose design value is `base`. Under
/// reduced motion this is 0.01 ms, never zero, so `x / dur(..)` stays finite
/// and callers clamp to 1 as they already do.
pub fn dur(base: f32) -> f32 {
    effective_duration(base, is_reduced())
}

/// Pure core of [`ambient_time`].
pub fn ambient_time_for(time: f32, reduced: bool) -> f32 {
    if reduced {
        0.0
    } else {
        time
    }
}

/// Clock value to feed an *ambient loop's phase*. Frozen at 0 under reduced
/// motion. Never use this for ages of one-shot transitions (those use [`dur`]).
pub fn ambient_time(time: f32) -> f32 {
    ambient_time_for(time, is_reduced())
}

/// Per-frame step for the particle layers (pearl dust, petals): 0 when reduced.
pub fn ambient_dt(dt: f32) -> f32 {
    if is_reduced() {
        0.0
    } else {
        dt
    }
}

/// Floor for `--motion-speed`, so `period / speed` stays finite.
pub const MIN_SPEED: f32 = 0.0001;

/// Pure core of [`scaled_time`].
pub fn scaled_time_for(time: f32, speed: f32, reduced: bool) -> f32 {
    ambient_time_for(time, reduced) * speed.max(MIN_SPEED)
}

/// Clock for an ambient loop whose CSS period is `calc(Ns / var(--motion-speed))`.
///
/// A loop with period `N / speed` at clock `t` is at phase `t * speed / N`,
/// so every ambient layer divides *this* by its fixed design period `N` and
/// the user's `Settings::motion_speed` lands the same way everywhere
/// (backdrop, glass, text, particles). Frozen at 0 under reduced motion, like
/// [`ambient_time`]; at speed 1 it is exactly `time`.
pub fn scaled_time(time: f32, speed: f32) -> f32 {
    scaled_time_for(time, speed, is_reduced())
}

/// Per-frame step for the particle layers with `--motion-speed` applied:
/// [`ambient_dt`] scaled by the speed.
pub fn scaled_dt(dt: f32, speed: f32) -> f32 {
    ambient_dt(dt) * speed.max(MIN_SPEED)
}

/// Rate-limited poller that feeds the OS setting into the flag. The probe is
/// a parameter so tests (and non-Windows builds) can stub it.
#[derive(Debug, Default)]
pub struct Watcher {
    next_poll: f32,
}

impl Watcher {
    pub fn new() -> Self {
        Self::default()
    }

    /// Re-read the OS setting if `force` is set or [`POLL_INTERVAL_SECS`] has
    /// passed since the last read. `probe` returns `true` when the OS asks for
    /// reduced motion. Returns `Some(new_value)` only when the flag changed.
    pub fn poll(&mut self, now: f32, force: bool, probe: impl FnOnce() -> bool) -> Option<bool> {
        if !force && now < self.next_poll {
            return None;
        }
        self.next_poll = now + POLL_INTERVAL_SECS;
        let reduced = probe();
        if reduced == is_reduced() {
            return None;
        }
        set_reduced(reduced);
        Some(reduced)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normal_motion_leaves_durations_alone() {
        assert_eq!(effective_duration(0.24, false), 0.24);
        assert_eq!(effective_duration(1.4, false), 1.4);
    }

    #[test]
    fn reduced_motion_collapses_every_duration_under_one_ms() {
        for base in [0.0, 0.05, 0.22, 0.24, 0.4, 0.62, 1.2, 1.4, 60.0] {
            let d = effective_duration(base, true);
            assert!(d < 0.001, "{base} -> {d}");
            assert!(d <= base || base == 0.0);
        }
        assert!((REDUCED_DURATION_SECS - 1e-5).abs() < 1e-9);
    }

    #[test]
    fn reduced_duration_completes_a_transition_in_one_frame() {
        // The renderer's tick shape: anim += dt / dur, clamped to 1.
        let dt = 1.0 / 60.0;
        let step = dt / effective_duration(0.4, true);
        assert!(step.is_finite() && step > 1.0);
        // First frame after startup has dt == 0: 0 / tiny is 0, not NaN.
        assert_eq!(0.0 / effective_duration(0.4, true), 0.0);
    }

    #[test]
    fn ambient_time_freezes_only_when_reduced() {
        assert_eq!(ambient_time_for(12.5, false), 12.5);
        assert_eq!(ambient_time_for(12.5, true), 0.0);
    }

    #[test]
    fn motion_speed_scales_the_clock_and_reduced_motion_still_wins() {
        assert_eq!(scaled_time_for(12.5, 1.0, false), 12.5);
        assert_eq!(scaled_time_for(10.0, 2.0, false), 20.0);
        assert_eq!(scaled_time_for(10.0, 0.5, false), 5.0);
        // Never a zero clock rate, never NaN.
        assert!(scaled_time_for(10.0, 0.0, false) > 0.0);
        // Reduced motion freezes the loop whatever the speed.
        assert_eq!(scaled_time_for(10.0, 3.0, true), 0.0);
    }

    #[test]
    fn global_flag_drives_dur_ambient_time_and_dt() {
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        set_reduced(false);
        assert_eq!(dur(0.3), 0.3);
        assert_eq!(ambient_time(7.0), 7.0);
        assert_eq!(ambient_dt(0.016), 0.016);
        set_reduced(true);
        assert!(dur(0.3) < 0.001);
        assert_eq!(ambient_time(7.0), 0.0);
        assert_eq!(ambient_dt(0.016), 0.0);
        set_reduced(false);
    }

    #[test]
    fn watcher_rate_limits_and_reports_changes() {
        // Both global-flag tests serialise on TEST_LOCK.
        let _g = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        set_reduced(false);
        let mut w = Watcher::new();
        // First poll always reads.
        assert_eq!(w.poll(0.0, false, || true), Some(true));
        assert!(is_reduced());
        // Within the interval the probe is not even called.
        assert_eq!(w.poll(0.5, false, || panic!("rate limit")), None);
        // Unchanged value: no event.
        assert_eq!(w.poll(1.2, false, || true), None);
        // Changed value after the interval.
        assert_eq!(w.poll(2.5, false, || false), Some(false));
        assert!(!is_reduced());
        // `force` bypasses the limiter (focus gain).
        assert_eq!(w.poll(2.6, true, || true), Some(true));
        set_reduced(false);
    }

    static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
}
