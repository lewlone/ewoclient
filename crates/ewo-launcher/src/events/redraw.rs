//! `RedrawRequested`: render one frame, in phases. `on_redraw` is the
//! driver; each phase is a method on `App` in the submodule that owns its
//! subject (the same shape as the Rewo client's `frame()` split):
//!
//! 1. skip the frame when it can't be shown (`should_skip_frame`)
//! 2. advance the clock and follow the OS reduced-motion setting
//! 3. update animation state (`update_animation_state`)
//! 4. pump services and background results (`pump`, `launch_events`)
//! 5. apply the requests the UI recorded (`requests`)
//! 6. dev overlay, then the launching screen's own clock
//! 7. build the per-frame views and draw (`views`, `paint`)

use super::*;

mod launch_events;
mod paint;
mod pump;
mod requests;
mod views;

/// The frame can't be shown: the window is occluded, minimised, or isn't the
/// OS foreground window. (`!focused` is deliberately not a signal.)
fn frame_hidden(occluded: bool, minimized: bool, foreground: bool) -> bool {
    occluded || minimized || !foreground
}

/// 0..1 entrance progress for the main menu (fade + slide); every other
/// screen is settled.
fn main_menu_enter(screen: Screen, time: f32, entered_at: f32) -> f32 {
    if screen == Screen::MainMenu {
        ((time - entered_at) / ewo_core::motion::dur(0.34)).clamp(0.0, 1.0)
    } else {
        1.0
    }
}

impl App {
    /// `WindowEvent::RedrawRequested`: render one frame.
    pub(crate) fn on_redraw(&mut self, window: &Arc<Window>) {
        if self.should_skip_frame(window) {
            return;
        }
        let (time, dt) = self.begin_frame();
        // The screen this frame draws. The launching hand-off below may
        // switch `self.screen`; that takes effect on the next frame.
        let screen = self.screen;
        self.update_animation_state(screen, time, dt);
        self.pump_services(time);
        self.apply_ui_requests();
        self.update_dev_overlay(time, dt);
        self.tick_launching_screen(time, dt);
        self.paint_frame(window, time, screen);
        // Chain the next redraw so animations keep ticking. The
        // unfocused-skip at the top of this arm short-circuits
        // before any expensive work, so leaving this on for the
        // unfocused path is cheap (one re-queue per ~100 ms while
        // unfocused, throttled by `about_to_wait`'s WaitUntil).
        if self.focused {
            window.request_redraw();
        }
    }

    /// While frames are skipped (minimised, occluded, not foreground),
    /// nothing after `should_skip_frame` in `on_redraw` runs, and that
    /// includes `pump_services`. So a game that exited while the launcher sat
    /// minimised was never noticed (no exit log, no window restore), and
    /// download, auth and social results waited in their channels until the
    /// user brought the launcher back. `about_to_wait` (which still runs on
    /// the unfocused wake cadence) calls this to keep those services moving
    /// at a low rate without drawing anything.
    pub(crate) fn pump_services_while_hidden(&mut self) {
        // The cheap interval gate comes first: `should_skip_frame` makes
        // Win32 calls (`IsIconic`, `GetForegroundWindow`) and this runs on
        // every loop iteration, which at a visible launcher's uncapped rate
        // is thousands of times a second. The visibility check therefore
        // runs at most once per interval, whether or not it finds the
        // launcher hidden.
        let now = Instant::now();
        if !hidden_pump_due(self.hidden_pump_at, now) {
            return;
        }
        self.hidden_pump_at = Some(now);
        let Some(window) = self.window.clone() else {
            return;
        };
        if !self.should_skip_frame(&window) {
            return;
        }
        self.clock.tick();
        let time = self.clock.elapsed;
        self.pump_services(time);
        self.apply_ui_requests();
    }

    /// Skip render + swap entirely when the window can't be
    /// shown. On Windows, `wglSwapBuffers` on an obscured /
    /// minimised window queues presentations in the GL driver
    /// indefinitely (the compositor can't display them, so
    /// they pile up) — that's the per-frame ~6 KB / frame
    /// C++-side leak we hunted.
    ///
    /// Four signals, any of which skips this frame:
    ///  - `Occluded(true)` — winit's official signal. Reliable
    ///    on most platforms but on Win11 it sometimes fails to
    ///    fire when another app takes fullscreen.
    ///  - `is_minimized() == Some(true)` — minimised to taskbar.
    ///  - `Win32 GetForegroundWindow() != our hwnd` — we're not
    ///    the user's active app. The Win32 fallback that fires
    ///    when winit's Focused / Occluded didn't.
    ///  - (`!self.focused` is intentionally NOT a skip signal —
    ///    the user might have a chat window focused while
    ///    watching the launcher animations in another monitor.
    ///    Foreground covers the actually-leak-causing case.)
    fn should_skip_frame(&self, window: &Arc<Window>) -> bool {
        let minimized = window.is_minimized().unwrap_or(false);
        let foreground = window::is_foreground(window);
        frame_hidden(self.occluded, minimized, foreground)
    }

    /// Advance the clock and poll the OS reduced-motion setting. Returns the
    /// frame's `(time, dt)`.
    fn begin_frame(&mut self) -> (f32, f32) {
        self.clock.tick();
        let time = self.clock.elapsed;
        let dt = self.clock.dt;

        // Reduced motion (non-negotiable #6): follow the OS setting, polled at
        // most once a second plus on every focus gain. The first frame always
        // reads it. Flipping it also drops the cached backdrop so the frozen
        // (or resumed) ambient layers show on the very next frame.
        let force = std::mem::take(&mut self.motion_recheck);
        if let Some(reduced) = self
            .motion_watch
            .poll(time, force, window::prefers_reduced_motion)
        {
            log::info!("reduced motion: {}", if reduced { "on" } else { "off" });
            if let Some(b) = self.backdrop.as_ref() {
                b.invalidate_cache();
            }
        }

        (time, dt)
    }

    /// Screen-entrance stamp, celebrate expiry, backdrop and widget hover
    /// animations.
    fn update_animation_state(&mut self, screen: Screen, time: f32, dt: f32) {
        // Smooth screen entrance: stamp the wall-time the screen
        // changed so the renderer can fade + slide the main menu in.
        if screen != self.prev_screen {
            self.prev_screen = screen;
            self.screen_enter_at = time;
        }

        // Auto-end celebrate after the configured duration.
        if let Some(end) = self.celebrate_until {
            if time >= end {
                if let Some(b) = self.backdrop.as_mut() {
                    b.celebrate(false);
                }
                self.celebrate_until = None;
            }
        }

        if let Some(backdrop) = self.backdrop.as_mut() {
            backdrop.update(dt, &self.settings);
        }

        // Tick widget hover animations.
        self.launch_button.tick(dt);
        for s in self.menu_items.iter_mut() {
            s.tick(dt);
        }
        self.prefs.tick(dt);
        self.instance_prefs.tick(dt);
        if self.instance_prefs.renaming {
            self.instance_prefs.rename_focus_time += dt;
        }
        if self.prefs.profile_renaming.is_some() {
            self.prefs.profile_rename_focus_time += dt;
        }
        self.modal.tick(dt);
        self.about_modal.tick(dt);
        self.delete_modal.tick(dt);
        self.launcher_link_modal.tick(dt);
        self.friends_prefs.tick(dt);
    }

    /// The `--dev` overlay: its own clock, settings it drives, and the
    /// simulated launch error it mirrors into the launching screen.
    fn update_dev_overlay(&mut self, time: f32, dt: f32) {
        if let Some(overlay) = self.dev_overlay.as_mut() {
            overlay.tick(dt);
            let density_changed = overlay.apply_to_settings(&mut self.settings);
            if density_changed {
                if let (Some(window), Some(backdrop)) =
                    (self.window.as_ref(), self.backdrop.as_mut())
                {
                    // Logical pixels — match `draw_frame`'s coord
                    // space (post-canvas-scale).
                    let size = window.inner_size();
                    let scale = window.scale_factor() as f32;
                    let logical_w = ((size.width as f32) / scale) as u32;
                    let logical_h = ((size.height as f32) / scale) as u32;
                    let (cw, ch) = app_window::card_content_size(logical_w, logical_h);
                    backdrop.resize(cw, ch, &self.settings);
                }
            }
            // Mirror dev overlay's sim_error into the launching
            // state so the pbar variant matches what the dev pill
            // shows. Auto-starts a synthetic launch if needed so
            // the error has a bar to render against.
            if overlay.sim_error != self.launching.error {
                match overlay.sim_error {
                    Some(variant) => {
                        if self.launching.start_time.is_none() {
                            let (n, m) = self.demo_launch_label();
                            self.launching.enter(time, &n, &m);
                        }
                        self.launching.trigger_error(variant, time);
                    }
                    None => self.launching.clear_error(),
                }
            }
        }
    }

    /// The launching screen's progress clock and its hand-off back to
    /// Instances.
    fn tick_launching_screen(&mut self, time: f32, dt: f32) {
        if self.screen == Screen::Launching {
            self.launching.tick(time, dt);
            if self.launching.should_handoff(time) {
                log::info!("launching: handoff complete → returning to Instances");
                self.launching.exit();
                self.screen = Screen::Instances;
            }
        }
    }
}

/// How often `pump_services_while_hidden` runs: often enough that a game
/// exit restores the launcher promptly, rare enough to cost nothing.
const HIDDEN_PUMP_INTERVAL: std::time::Duration = std::time::Duration::from_millis(250);

/// Whether the hidden-state service pump is due at `now`.
fn hidden_pump_due(last: Option<Instant>, now: Instant) -> bool {
    last.is_none_or(|t| now.saturating_duration_since(t) >= HIDDEN_PUMP_INTERVAL)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_hidden_pump_runs_first_time_then_at_its_interval() {
        let t0 = Instant::now();
        assert!(hidden_pump_due(None, t0));
        assert!(!hidden_pump_due(Some(t0), t0));
        assert!(!hidden_pump_due(Some(t0), t0 + HIDDEN_PUMP_INTERVAL / 2));
        assert!(hidden_pump_due(Some(t0), t0 + HIDDEN_PUMP_INTERVAL));
    }

    #[test]
    fn a_frame_is_hidden_when_any_visibility_signal_says_so() {
        assert!(!frame_hidden(false, false, true));
        assert!(frame_hidden(true, false, true), "occluded");
        assert!(frame_hidden(false, true, true), "minimised");
        assert!(
            frame_hidden(false, false, false),
            "not the foreground window"
        );
    }

    #[test]
    fn main_menu_entrance_runs_zero_to_one_and_other_screens_are_settled() {
        let d = ewo_core::motion::dur(0.34);
        assert_eq!(main_menu_enter(Screen::MainMenu, 5.0, 5.0), 0.0);
        let half = main_menu_enter(Screen::MainMenu, 5.0 + d * 0.5, 5.0);
        assert!((half - 0.5).abs() < 1e-3, "{half}");
        assert_eq!(main_menu_enter(Screen::MainMenu, 5.0 + d * 3.0, 5.0), 1.0);
        // Before the stamp (the never-entered sentinel) it clamps, not goes negative.
        assert_eq!(main_menu_enter(Screen::MainMenu, 0.0, 5.0), 0.0);
        for s in [
            Screen::Instances,
            Screen::Settings,
            Screen::Friends,
            Screen::Launching,
        ] {
            assert_eq!(main_menu_enter(s, 5.0, 5.0), 1.0, "{s:?}");
        }
    }
}
