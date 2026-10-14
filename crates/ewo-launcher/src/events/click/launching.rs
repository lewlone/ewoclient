//! Launch clicks: the Launching screen's Retry / Back footer and the
//! Instances screen's Launch button.

use super::super::hit::{launching_click, LaunchingClick};
use super::*;

impl App {
    /// Step 2.9: the Launching screen's Retry / Back buttons, active only
    /// once the JVM has exited non-zero. Retry rebuilds the launch plan and
    /// respawns; Back returns to Instances.
    pub(super) fn click_launching_footer(&mut self, c: Click) -> bool {
        if !c.pressed || self.screen != Screen::Launching || !self.launching.ended_in_error() {
            return false;
        }
        match launching_click(c.pos, c.card_w, c.card_h) {
            Some(LaunchingClick::Retry) => {
                log::info!("launching: Retry clicked");
                self.retry_launch(c.time);
                true
            }
            Some(LaunchingClick::Back) => {
                log::info!("launching: Back clicked");
                self.launching.exit();
                self.screen = Screen::Instances;
                true
            }
            None => false,
        }
    }

    fn retry_launch(&mut self, time: f32) {
        let inst_name = self.launching.instance_name.clone();
        let inst_meta = self.launching.instance_meta.clone();
        // A busy launch slot counts as "fine": the refusal is already logged.
        let ok = self.launch_slot_busy(time) || {
            self.launching.reset_for_retry();
            self.try_real_launch(
                self.instance_prefs.selected,
                &inst_name,
                &inst_meta,
                time,
                PrepareFail::Error("retry could not start launch"),
            )
        };
        if !ok {
            // try_real_launch can return false silently when something's
            // missing — surface that to the user as an error rather than a
            // blank screen.
            self.launching.push_real_line(
                screens::RealSeverity::Warn,
                "[ewo] retry could not start launch — see logs above".into(),
                time,
            );
            self.launching.set_real_exit(Some(127), time);
        }
    }

    /// Step 3: the active screen's Launch button. A click starts the launch
    /// (`start_launch` gates a Pending or missing instance internally; a
    /// plain Launch passes no server); a press on the button claims the
    /// click.
    pub(super) fn click_launch_button(&mut self, c: Click) -> bool {
        let Some(b) = self.launch_button_bounds(c.card_w) else {
            return false;
        };
        let clicked = self.launch_button.update(c.pos, b, c.pressed, c.time);
        if clicked {
            self.start_launch(self.instance_prefs.selected, None, c.time);
        }
        self.launch_button.hover && c.pressed
    }
}
