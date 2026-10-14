//! Draining the game-launch event streams into the launching screen: the
//! bundled-JRE download (`RuntimeEvent`) and the JVM / native process
//! (`LaunchEvent`), including the launcher's minimize-while-playing hand-off.

use super::*;

/// Fallback delay (seconds after JVM spawn) before the launcher minimizes
/// itself if no window-ready marker was seen in the game log. Generous
/// enough to cover a slow cold start / heavy modpack on a spinning disk,
/// while still guaranteeing the hand-off eventually happens.
const MINIMIZE_FALLBACK_SECS: f32 = 25.0;

/// Does this game-log line signal that Minecraft's window is coming up on
/// screen? These substrings are logged right as the client initializes its
/// render backend / audio / narrator — i.e. the moment the game becomes
/// visible — so seeing one is our cue to minimize the launcher. Matched
/// case-insensitively; kept deliberately broad so vanilla and modded
/// (Fabric/loader) log formats both trip it.
fn is_window_ready_marker(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    const MARKERS: [&str; 5] = [
        "backend library: lwjgl", // client render backend init — window created
        "lwjgl version",          // same signal, alternate phrasing
        "openal initialized",     // sound engine — just after window shows
        "sound engine started",   // "
        "narrator library",       // accessibility init at client startup
    ];
    MARKERS.iter().any(|m| lower.contains(m))
}

/// The 10 % bucket to log for a runtime-download progress event, or
/// `None` when it was already logged (`last` is the last bucket logged,
/// `0` at the start of a download). Each of 10 %, 20 %, …, 100 % is
/// logged once, however many progress events land inside it.
fn progress_log_bucket(last: u32, downloaded: u64, total: u64) -> Option<u32> {
    if total == 0 {
        return None;
    }
    let pct = (downloaded.min(total) * 100 / total) as u32;
    let bucket = pct / 10 * 10;
    (bucket > last).then_some(bucket)
}

impl App {
    /// Drain runtime (bundled-JRE) events. Surface progress as Info lines on
    /// the launching screen; on Done, kick the JRE re-scan that retries the
    /// pending launch.
    pub(super) fn pump_runtime_events(&mut self, time: f32) {
        // Drain runtime (bundled-JRE) events. Surface progress
        // as Info lines on the launching screen; on Done, kick
        // the JRE-detector cache + retry the pending launch.
        let runtime_events = self.runtime.poll();
        for ev in runtime_events {
            match ev {
                runtime::RuntimeEvent::Resolved { major, info } => {
                    self.runtime_log_bucket = 0;
                    self.launching.push_real_line(
                        screens::RealSeverity::Info,
                        format!(
                            "[ewo] resolved Java {} → {} ({:.1} MB)",
                            major,
                            info.release_name,
                            info.size as f32 / 1_048_576.0
                        ),
                        time,
                    );
                }
                runtime::RuntimeEvent::Progress { downloaded, total } => {
                    // Drive the real pbar override (visible
                    // immediately as a smooth fill), and log a
                    // line every 10% step so the user sees
                    // discrete checkpoints in the log panel too.
                    if total > 0 {
                        let frac = downloaded as f32 / total as f32;
                        self.launching.set_real_progress(Some(frac));
                        if let Some(bucket) =
                            progress_log_bucket(self.runtime_log_bucket, downloaded, total)
                        {
                            self.runtime_log_bucket = bucket;
                            let line = format!(
                                "[ewo] downloading runtime: {:>3}% ({} / {} MB)",
                                bucket,
                                downloaded / 1_048_576,
                                total / 1_048_576,
                            );
                            self.launching
                                .push_real_line(screens::RealSeverity::Info, line, time);
                        }
                    }
                }
                runtime::RuntimeEvent::Done { major, jre_dir } => {
                    // Clear the pbar override — synthetic curve
                    // takes back over while the JVM boots.
                    self.launching.set_real_progress(None);
                    self.launching.push_real_line(
                        screens::RealSeverity::Info,
                        format!(
                            "[ewo] Java {} extracted to {} — retrying launch…",
                            major,
                            jre_dir.display()
                        ),
                        time,
                    );
                    // Re-scan installed JREs on a worker (it runs
                    // `java -version` per candidate, which must not
                    // stall the UI thread); `poll_jre_rescan` retries
                    // the deferred launch once it reports back.
                    self.jre_rescan_rx = Some(launch::jre::rescan_in_background(major));
                }
                runtime::RuntimeEvent::Failed { major, message } => {
                    log::warn!("runtime: Java {} fetch failed: {}", major, message);
                    self.launching.set_real_progress(None);
                    self.launching.push_real_line(
                        screens::RealSeverity::Warn,
                        format!("[ewo] Java {} fetch failed: {}", major, message),
                        time,
                    );
                    self.launching.set_real_exit(Some(127), time);
                    self.pending_relaunch = None;
                }
            }
        }
    }

    /// Observe the tracked game for a visible window (about once a second):
    /// the real "its window was seen" signal for the zombie decision, which
    /// the log markers only hint at.
    fn poll_game_window(&mut self, time: f32) {
        const POLL_EVERY_SECS: f32 = 1.0;
        let Some(t) = self.active_launch else { return };
        if self.active_launch_window_seen || time - self.active_launch_polled_at < POLL_EVERY_SECS {
            return;
        }
        self.active_launch_polled_at = time;
        if matches!(
            launch::reaper::state(&t),
            launch::reaper::GameState::Running {
                visible_window: true
            }
        ) {
            self.active_launch_window_seen = true;
        }
    }

    /// Drain JVM launch events into the launching screen. Each line, every
    /// stage transition, and the exit code arrives via this channel. When
    /// the JVM exits we drop the receiver; the next launch creates a fresh
    /// one. Also drives the minimize-while-playing hand-off.
    pub(super) fn pump_launch_events(&mut self, time: f32) {
        // Drain JVM launch events into the launching screen.
        // Each line, every stage transition, and the exit code
        // arrives via this channel. When the JVM exits we drop
        // the receiver — the next launch will create a fresh one.
        let mut launch_finished = false;
        if let Some(rx) = self.launch_rx.as_ref() {
            while let Ok(event) = rx.try_recv() {
                match event {
                    launch::LaunchEvent::Started { pid, created } => {
                        log::info!("launch: JVM started (pid {pid})");
                        // Record PID + creation time so a zombie JVM
                        // (deadlocked in native teardown on exit) can
                        // be reaped before the next launch — and only
                        // that exact process. See launch::reaper.
                        self.active_launch_started_at = time;
                        self.active_launch_window_seen = false;
                        self.active_launch_marker_seen = false;
                        self.active_launch_polled_at = time;
                        self.active_launch = created.map(|created| {
                            let t = launch::reaper::Tracked { pid, created };
                            launch::reaper::record(t);
                            t
                        });
                        // Don't minimize yet — the JVM has only just
                        // spawned; Minecraft's window is ~10-30s away.
                        // Arm a fallback deadline instead. We minimize
                        // the moment the game's window-ready marker shows
                        // in the log (see the `Line` arm), or when this
                        // deadline fires, whichever comes first. Restored
                        // on JVM exit (the `launch_finished` block below).
                        self.pending_minimize = Some(time + MINIMIZE_FALLBACK_SECS);
                    }
                    launch::LaunchEvent::Line { severity, text } => {
                        if is_window_ready_marker(&text) {
                            // Only a hint: the window may not exist yet.
                            self.active_launch_marker_seen = true;
                        }
                        // Minimize as soon as the game is visibly coming
                        // up. These markers are logged right as the client
                        // creates its render backend / window — the point
                        // where MC becomes visible — so this tracks "the
                        // game is on screen now" far better than the spawn
                        // event does. Only acts while a minimize is armed.
                        if self.pending_minimize.is_some() && is_window_ready_marker(&text) {
                            if let Some(win) = self.window.as_ref() {
                                win.set_minimized(true);
                            }
                            self.pending_minimize = None;
                            log::info!("launch: game window up — minimized launcher");
                        }
                        let sev = match severity {
                            launch::Severity::Info => screens::RealSeverity::Info,
                            launch::Severity::Warn => screens::RealSeverity::Warn,
                        };
                        self.launching.push_real_line(sev, text, time);
                    }
                    launch::LaunchEvent::Exited(code) => {
                        log::info!("launch: JVM exited code={:?}", code);
                        self.launching.set_real_exit(code, time);
                        // Dump the in-memory log to disk so the
                        // user can grab it later (especially on
                        // a crash). Best-effort: errors don't
                        // surface to the UI.
                        persist_launch_log(
                            self.active_launch_instance_id.as_deref().unwrap_or(""),
                            &self.launching.instance_name,
                            self.launching.real_log.as_deref().unwrap_or(&[]),
                            code,
                        );
                        launch_finished = true;
                    }
                    launch::LaunchEvent::SpawnFailed(msg) => {
                        log::warn!("launch: spawn failed: {}", msg);
                        self.launching.push_real_line(
                            screens::RealSeverity::Warn,
                            format!("[ewo] spawn failed: {}", msg),
                            time,
                        );
                        self.launching.set_real_exit(Some(127), time);
                        launch_finished = true;
                    }
                }
            }
        }
        self.poll_game_window(time);
        // Fallback minimize: if the game has been starting for a while
        // but we never saw a window-ready marker (modded logs vary),
        // minimize once the armed deadline passes so the hand-off still
        // happens.
        if let Some(deadline) = self.pending_minimize {
            if time >= deadline {
                if let Some(win) = self.window.as_ref() {
                    win.set_minimized(true);
                }
                self.pending_minimize = None;
                log::info!("launch: minimize fallback fired — minimized launcher");
            }
        }

        if launch_finished {
            self.launch_rx = None;
            // JVM reported exit — forget its record (see launch::reaper).
            if let Some(t) = self.active_launch.take() {
                launch::reaper::forget(&t);
            }
            self.active_launch_instance_id = None;
            // A launch that ends before it ever minimized (fast crash,
            // spawn failure): disarm so we don't minimize after the fact.
            self.pending_minimize = None;
            // The game (or a failed spawn) is done — bring the launcher
            // back from the taskbar and re-focus it so the user lands on
            // the post-launch screen. `set_minimized(false)` is a no-op
            // if we never minimized (e.g. a spawn that failed before
            // `Started`), so this is safe on every finish path.
            if let Some(win) = self.window.as_ref() {
                win.set_minimized(false);
                win.focus_window();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_progress_bucket_logs_once() {
        let total = 1000;
        let mut last = 0;
        let mut logged = Vec::new();
        // Many events per bucket, as the download thread sends them.
        for downloaded in (0..=total).step_by(7).chain([total]) {
            if let Some(b) = progress_log_bucket(last, downloaded, total) {
                last = b;
                logged.push(b);
            }
        }
        assert_eq!(logged, vec![10, 20, 30, 40, 50, 60, 70, 80, 90, 100]);
    }

    #[test]
    fn progress_edge_cases() {
        assert_eq!(progress_log_bucket(0, 5, 0), None, "unknown total");
        assert_eq!(progress_log_bucket(0, 50, 1000), None, "under 10 %");
        assert_eq!(
            progress_log_bucket(0, 2000, 1000),
            Some(100),
            "overshoot clamps"
        );
        assert_eq!(
            progress_log_bucket(30, 350, 1000),
            None,
            "same bucket again"
        );
    }

    #[test]
    fn window_ready_markers_match_case_insensitively() {
        for line in [
            "[Render thread/INFO]: Backend library: LWJGL version 3.3.3",
            "LWJGL Version 3.3.3-snapshot",
            "[Sound engine started]",
            "OpenAL initialized.",
            "Narrator library for x64 successfully loaded",
        ] {
            assert!(is_window_ready_marker(line), "{line}");
        }
        for line in ["", "Loading mods", "Setting user: Player"] {
            assert!(!is_window_ready_marker(line), "{line}");
        }
    }
}
