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

#[cfg(test)]
mod tests {
    use super::*;
    use SlotDecision::*;
    use SlotInput::*;

    #[test] fn preparing_refuses_the_click() {
        assert_eq!(decide(Preparing), Busy("a launch is already being prepared"));
    }
    #[test] fn never_launched_is_free_and_clears_nothing() {
        assert_eq!(decide(NeverLaunched), Free);
    }
    #[test] fn starting_refuses_the_click() {
        assert_eq!(decide(Starting), Busy("a game is starting"));
    }
    #[test] fn exited_game_is_forgotten_then_free() {
        assert_eq!(decide(Gone), ForgetThenFree);
    }
    #[test] fn zombie_game_is_reaped_then_free() {
        assert_eq!(decide(Running { zombie: true }), ReapThenFree);
    }
    #[test] fn running_game_refuses_the_click() {
        assert_eq!(decide(Running { zombie: false }), Busy("a game is already running"));
    }

    /// The "never kill a running game" guarantee, over the observations
    /// `reaper::is_zombie` actually classifies: every non-zombie running game
    /// must be `Busy` — never a decision that reaps or frees the slot.
    #[test]
    fn a_non_zombie_running_game_is_always_busy() {
        for visible in [false, true] {
            for seen in [false, true] {
                for elapsed in [0.0, 100.0, 300.0, 301.0, 10_000.0] {
                    let zombie = crate::launch::reaper::is_zombie(visible, seen, elapsed);
                    let decision = decide(Running { zombie });
                    let at = format!("visible={visible} seen={seen} elapsed={elapsed}");
                    if zombie {
                        assert_eq!(decision, ReapThenFree, "{at}");
                    } else {
                        assert!(matches!(decision, Busy(_)), "live game must be Busy: {decision:?} ({at})");
                    }
                }
            }
        }
    }
}
