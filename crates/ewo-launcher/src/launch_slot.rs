//! Pure decision for the launcher's one-game-at-a-time launch slot: whether
//! a Launch click is refused (busy) or allowed, and what cleanup an allowed
//! click owes the previous game. `App::launch_slot_busy` gathers the input
//! (the OS process queries live there) and performs the side effects.

/// What the launcher knows about the launch slot right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SlotInput {
    /// A launch is being prepared, or a relaunch is pending.
    Preparing,
    /// No game has ever been launched this session (`launch_rx` is `None`).
    NeverLaunched,
    /// The JVM was spawned but hasn't reported `Started` yet.
    Starting,
    /// The tracked game exited, or its PID no longer is that process.
    Gone,
    /// The tracked game still runs; `zombie` is `reaper::is_zombie`'s verdict.
    Running { zombie: bool },
}

/// What to do with the launch slot. The free variants differ in cleanup:
/// `Free` touches nothing, `ReapThenFree` kills a zombie game first, and
/// `ForgetThenFree` drops the record of a game that already exited.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SlotDecision {
    /// Refuse the click. The payload is the reason clause the caller logs;
    /// the running game's pid is appended at the call site, because a pure
    /// decision can't know it.
    Busy(&'static str),
    /// Allow the launch; nothing to clean up.
    Free,
    /// Allow the launch after reaping the zombie game.
    ReapThenFree,
    /// Allow the launch after forgetting the game that already exited.
    ForgetThenFree,
}

/// The launch-slot policy: one game at a time. A live game is never killed —
/// the click is refused; only a zombie (or a game that already exited) frees
/// the slot for the next launch.
pub(crate) fn decide(input: SlotInput) -> SlotDecision {
    use SlotDecision::*;
    use SlotInput::*;
    match input {
        Preparing => Busy("a launch is already being prepared"),
        NeverLaunched => Free,
        Starting => Busy("a game is starting"),
        Gone => ForgetThenFree,
        Running { zombie: true } => ReapThenFree,
        Running { zombie: false } => Busy("a game is already running"),
    }
}

/// How often the running game is checked for a visible window.
pub(crate) const WINDOW_POLL_EVERY: std::time::Duration = std::time::Duration::from_secs(1);

/// Whether the "has the game's window been seen" poll is due: a game is
/// tracked, no window was observed yet (once seen it stays seen), and the
/// last poll is at least [`WINDOW_POLL_EVERY`] old (or there was none).
/// Pure, so the cadence is testable without a window system.
pub(crate) fn window_poll_due(
    tracked: bool,
    window_seen: bool,
    last_poll: Option<std::time::Instant>,
    now: std::time::Instant,
) -> bool {
    tracked
        && !window_seen
        && last_poll.is_none_or(|t| now.saturating_duration_since(t) >= WINDOW_POLL_EVERY)
}

#[cfg(test)]
mod tests {
    use super::*;
    use SlotDecision::*;
    use SlotInput::*;

    #[test]
    fn window_poll_runs_about_once_a_second_until_a_window_is_seen() {
        use std::time::{Duration, Instant};
        let t0 = Instant::now();
        let s = Duration::from_secs;
        assert!(!window_poll_due(false, false, None, t0), "nothing tracked");
        assert!(window_poll_due(true, false, None, t0), "never polled");
        assert!(!window_poll_due(true, false, Some(t0), t0 + s(0)));
        assert!(!window_poll_due(
            true,
            false,
            Some(t0),
            t0 + Duration::from_millis(999)
        ));
        assert!(window_poll_due(true, false, Some(t0), t0 + s(1)));
        // A window already observed ends the polling for this launch.
        assert!(!window_poll_due(true, true, Some(t0), t0 + s(60)));
        // Time never runs backwards on an Instant, but a stale stamp from
        // the future must not wedge the poll.
        assert!(!window_poll_due(true, false, Some(t0 + s(5)), t0));
    }

    #[test]
    fn preparing_refuses_the_click() {
        assert_eq!(
            decide(Preparing),
            Busy("a launch is already being prepared")
        );
    }
    #[test]
    fn never_launched_is_free_and_clears_nothing() {
        assert_eq!(decide(NeverLaunched), Free);
    }
    #[test]
    fn starting_refuses_the_click() {
        assert_eq!(decide(Starting), Busy("a game is starting"));
    }
    #[test]
    fn exited_game_is_forgotten_then_free() {
        assert_eq!(decide(Gone), ForgetThenFree);
    }
    #[test]
    fn zombie_game_is_reaped_then_free() {
        assert_eq!(decide(Running { zombie: true }), ReapThenFree);
    }
    #[test]
    fn running_game_refuses_the_click() {
        assert_eq!(
            decide(Running { zombie: false }),
            Busy("a game is already running")
        );
    }

    /// The "never kill a running game" guarantee, over the observations
    /// `reaper::is_zombie` actually classifies: every non-zombie running game
    /// must be `Busy` — never a decision that reaps or frees the slot.
    #[test]
    fn a_non_zombie_running_game_is_always_busy() {
        for visible in [false, true] {
            for seen in [false, true] {
                for marker in [false, true] {
                    for elapsed in [0.0, 29.0, 100.0, 300.0, 301.0, 10_000.0] {
                        let zombie =
                            crate::launch::reaper::is_zombie(visible, seen, marker, elapsed);
                        let decision = decide(Running { zombie });
                        let at = format!(
                            "visible={visible} seen={seen} marker={marker} elapsed={elapsed}"
                        );
                        if zombie {
                            assert_eq!(decision, ReapThenFree, "{at}");
                        } else {
                            assert!(
                                matches!(decision, Busy(_)),
                                "live game must be Busy: {decision:?} ({at})"
                            );
                        }
                    }
                }
            }
        }
    }
}
