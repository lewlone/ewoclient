//! Frame pacing: how often the event loop wakes to service the pending
//! redraw. Moved out of `main.rs`'s `about_to_wait`, with the cap decision
//! split out as pure functions.

use super::*;

/// Seconds of no user input before the focused redraw loop throttles down.
/// Long enough that transient interactions (ripples, sheen sweeps, modal
/// entrances) finish at full rate; short enough that an idle launcher stops
/// burning the full monitor rate quickly.
const IDLE_THRESHOLD_SECS: f32 = 0.5;
/// Frame rate the focused loop drops to while idle. Ambient motion (folds,
/// caustics, breathing, drifting motes) runs on multi-second periods, so it's
/// fully smooth here; the eye can't resolve more on those layers.
const IDLE_FPS: f32 = 120.0;
/// Unfocused cadence: the GPU stays idle while the user is in another window.
const UNFOCUSED_WAKE: Duration = Duration::from_millis(100);

/// Idle means no input for [`IDLE_THRESHOLD_SECS`] and not mid-launch (the
/// Launching screen always runs at full rate).
fn is_idle(secs_since_input: f32, screen: Screen) -> bool {
    secs_since_input >= IDLE_THRESHOLD_SECS && screen != Screen::Launching
}

/// The frame-rate cap for a focused launcher, or `None` for uncapped.
///
/// - idle: [`IDLE_FPS`], regardless of vsync (vsync alone caps at the monitor
///   rate, up to 500 Hz on the OLED, which is overkill at idle);
/// - active with vsync off and `0 < max_fps < 240`: `max_fps`;
/// - active otherwise: uncapped (vsync caps at refresh; the dev-overlay
///   uncapped path validates the 500 fps OLED target).
fn frame_cap(idle: bool, vsync_on: bool, max_fps: f32) -> Option<f32> {
    if idle {
        Some(IDLE_FPS)
    } else if !vsync_on && max_fps > 0.0 && max_fps < 240.0 {
        Some(max_fps)
    } else {
        None
    }
}

/// How long to wait for a capped frame rate.
fn frame_interval(fps: f32) -> Duration {
    Duration::from_nanos((1_000_000_000.0 / fps).max(1.0) as u64)
}

impl App {
    /// `ApplicationHandler::about_to_wait`.
    ///
    /// Frame pacing. `request_redraw` only marks the window dirty; this
    /// ControlFlow decides *when* the loop wakes to service that redraw, so
    /// a WaitUntil here caps the effective frame rate even though the
    /// redraw is always pending.
    ///
    ///  1. Unfocused → ~10 FPS — GPU stays idle in another window.
    ///  2. Focused + idle (no input past IDLE_THRESHOLD, and not mid-launch)
    ///     → IDLE_FPS. Ambient motion is far slower than that, so it reads
    ///     identical while cutting GPU/heat/power on an untouched launcher.
    ///  3. Focused + active, VSync off, `max_fps < 240` → cap to max_fps.
    ///  4. Focused + active otherwise → Poll.
    pub(crate) fn pace_frames(&self, event_loop: &ActiveEventLoop) {
        // GPU device lost: `GlBackend::render` can draw nothing until its
        // recreation backoff (up to 30 s) elapses, so polling (or even the
        // 100 ms unfocused cadence) would only burn a core. Sleep until the
        // next attempt; input events still wake the loop.
        if let Some(retry_at) = self.backend.as_ref().and_then(|b| b.retry_deadline()) {
            if retry_at > Instant::now() {
                event_loop.set_control_flow(ControlFlow::WaitUntil(retry_at));
                return;
            }
        }

        if !self.focused {
            event_loop.set_control_flow(ControlFlow::WaitUntil(Instant::now() + UNFOCUSED_WAKE));
            if let Some(w) = self.window.as_ref() {
                w.request_redraw();
            }
            return;
        }

        let idle = is_idle(self.last_activity.elapsed().as_secs_f32(), self.screen);
        let cap_fps = frame_cap(idle, self.prefs.vsync.on, self.prefs.max_fps.value);

        match cap_fps {
            Some(fps) => {
                event_loop
                    .set_control_flow(ControlFlow::WaitUntil(Instant::now() + frame_interval(fps)));
                if let Some(w) = self.window.as_ref() {
                    w.request_redraw();
                }
            }
            None => event_loop.set_control_flow(ControlFlow::Poll),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_needs_the_threshold_and_not_the_launching_screen() {
        assert!(!is_idle(0.0, Screen::MainMenu));
        assert!(!is_idle(IDLE_THRESHOLD_SECS - 0.01, Screen::Settings));
        assert!(is_idle(IDLE_THRESHOLD_SECS, Screen::Settings));
        assert!(is_idle(30.0, Screen::MainMenu));
        assert!(!is_idle(30.0, Screen::Launching), "never throttle a launch");
    }

    #[test]
    fn idle_caps_at_idle_fps_whatever_the_user_settings() {
        for (vsync, max) in [(true, 0.0), (false, 0.0), (false, 60.0), (true, 500.0)] {
            assert_eq!(frame_cap(true, vsync, max), Some(IDLE_FPS));
        }
    }

    #[test]
    fn active_frames_obey_max_fps_only_with_vsync_off_and_a_sub_240_value() {
        assert_eq!(frame_cap(false, false, 60.0), Some(60.0));
        assert_eq!(frame_cap(false, false, 239.0), Some(239.0));
        // Vsync on: the display caps, we poll.
        assert_eq!(frame_cap(false, true, 60.0), None);
        // 0 means unlimited; 240 and above is not a cap.
        assert_eq!(frame_cap(false, false, 0.0), None);
        assert_eq!(frame_cap(false, false, 240.0), None);
        assert_eq!(frame_cap(false, false, 500.0), None);
    }

    #[test]
    fn the_wake_interval_is_the_frame_period() {
        assert_eq!(frame_interval(100.0), Duration::from_millis(10));
        assert_eq!(frame_interval(IDLE_FPS), Duration::from_nanos(8_333_333));
        // A pathological rate can't produce a zero wait.
        assert!(frame_interval(f32::MAX) >= Duration::from_nanos(1));
    }
}
