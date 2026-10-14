//! Reap a lingering game process before the next launch.
//!
//! # Why this exists
//!
//! On Windows the game JVM's orderly shutdown can deadlock in native
//! teardown — `DLL_PROCESS_DETACH` under the Windows loader lock, with the
//! in-game HUD's second GL context and the WinRT SMTC media thread as the
//! prime suspects. The game window closes, but a headless zombie
//! `java.exe` lingers, holding `ewo_jni.dll` and the instance's files open,
//! which makes the *next* launch fail to re-extract natives.
//!
//! # What may be killed
//!
//! Only processes **this launcher spawned**: each launch records its child's
//! PID *and process creation time* (`record`) in memory and in a small
//! pidfile, so a record survives a launcher restart. Before anything is
//! terminated the live process must still have that exact creation time
//! (PID reuse — e.g. after a reboot — is thereby ruled out), must still be
//! running, must be `java(w).exe`/`rewo.exe`, must own no visible window
//! (a live game always does; a zombie never does), and must be older than
//! a spawn grace period (a game that is still starting has no window yet).
//!
//! "Its window was seen" is an **observation** (the launcher polls
//! `EnumWindows` for the tracked PID while the game runs), never inferred
//! from the game's log text; a log marker is only a hint with a longer grace.

use std::path::PathBuf;

const PIDFILE: &str = "last-launch.pid";

/// A game process this launcher spawned. `created` is the OS process
/// creation time (Windows FILETIME ticks); two processes can share a PID
/// over time but never a (PID, creation time) pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tracked {
    pub pid: u32,
    pub created: u64,
}

/// Seconds after spawn during which a game is never reaped, whatever else
/// is known: the JVM / loader needs this long before any window can exist.
pub const SPAWN_GRACE_SECS: f32 = 30.0;

/// A log marker ("lwjgl version", "narrator library", ...) says the client
/// reached its render backend, i.e. a window is imminent. If, past this
/// many seconds, no window was ever observed, the game is stuck or dead.
pub const MARKER_GRACE_SECS: f32 = 90.0;

/// Seconds a freshly spawned game may run without a visible window, and
/// with no evidence at all that it progressed, before it's considered a
/// zombie (JVM startup + loading screens take a while).
pub const STARTUP_GRACE_SECS: f32 = 300.0;

/// Whether a still-running tracked game should be treated as a zombie.
///
/// - `has_visible_window`: right now the PID owns a visible top-level
///   window (the OS observation). Always means alive.
/// - `window_seen`: a visible window was **observed** earlier, so it has
///   since closed.
/// - `marker_seen`: only the game's log claimed a window is about to show.
///   A hint, not a fact.
/// - `secs_since_start`: age since the spawn.
///
/// Nothing is a zombie inside [`SPAWN_GRACE_SECS`]. Past it, an observed
/// then-closed window is conclusive; the log hint alone needs
/// [`MARKER_GRACE_SECS`]; with neither, [`STARTUP_GRACE_SECS`].
pub fn is_zombie(
    has_visible_window: bool,
    window_seen: bool,
    marker_seen: bool,
    secs_since_start: f32,
) -> bool {
    if has_visible_window || secs_since_start < SPAWN_GRACE_SECS {
        return false;
    }
    if window_seen {
        return true;
    }
    if marker_seen {
        return secs_since_start > MARKER_GRACE_SECS;
    }
    secs_since_start > STARTUP_GRACE_SECS
}

/// Whether a recorded process from an *earlier launcher session* may be
/// reaped. Its history is unknown (no observation, no log), so it is held
/// to the marker grace: windowless and older than [`MARKER_GRACE_SECS`].
/// `age_secs` is `None` when the age can't be read: never reap then.
pub fn is_recorded_zombie(has_visible_window: bool, age_secs: Option<f32>) -> bool {
    !has_visible_window && age_secs.is_some_and(|a| a > MARKER_GRACE_SECS)
}

/// Seconds between a FILETIME creation time and a FILETIME "now" (100 ns
/// ticks). `None` if `now` is before `created` (clock change).
pub fn age_secs_between(created: u64, now: u64) -> Option<f32> {
    now.checked_sub(created).map(|t| t as f32 / 10_000_000.0)
}

/// Whether the live process still is the tracked one. `current` is the
/// creation time the OS reports for `tracked.pid` now (`None` = gone).
pub fn same_process(tracked: &Tracked, current: Option<u64>) -> bool {
    current == Some(tracked.created)
}

/// Observed state of a tracked process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GameState {
    /// Exited, or its PID now belongs to a different process.
    Gone,
    Running {
        visible_window: bool,
    },
}

fn pidfile_path() -> Option<PathBuf> {
    let mut p = dirs::config_dir()?;
    p.push("EwoClient");
    p.push(PIDFILE);
    Some(p)
}

fn parse_records(text: &str) -> Vec<Tracked> {
    text.lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            let pid = it.next()?.parse().ok()?;
            let created = it.next()?.parse().ok()?;
            Some(Tracked { pid, created })
        })
        .collect()
}

fn format_records(records: &[Tracked]) -> String {
    records
        .iter()
        .map(|t| format!("{} {}\n", t.pid, t.created))
        .collect()
}

/// Every launch recorded (in this or a previous launcher session) that
/// hasn't been forgotten. Pre-creation-time pidfiles (a bare PID) yield
/// nothing — such a PID can't be verified, so it is never reaped.
pub fn recorded() -> Vec<Tracked> {
    pidfile_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|t| parse_records(&t))
        .unwrap_or_default()
}

fn write_records(records: &[Tracked]) {
    let Some(path) = pidfile_path() else { return };
    let result = if records.is_empty() {
        match std::fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    } else {
        crate::util::atomic_write(&path, format_records(records).as_bytes())
    };
    if let Err(e) = result {
        log::debug!("launch: could not update pidfile {}: {e}", path.display());
    }
}

/// Persist a just-spawned game so it can be reaped later if it zombies,
/// even across a launcher restart.
pub fn record(tracked: Tracked) {
    let mut all = recorded();
    all.retain(|t| t.pid != tracked.pid);
    all.push(tracked);
    write_records(&all);
}

/// Forget a record — called on a clean exit and after reaping.
pub fn forget(tracked: &Tracked) {
    let mut all = recorded();
    let before = all.len();
    all.retain(|t| t != tracked);
    if all.len() != before {
        write_records(&all);
    }
}

/// Reap every recorded game process that is a zombie (still running, no
/// visible window, older than the grace period). Live, windowed games and
/// games still starting are left alone; records of processes that are gone
/// (or whose PID was reused) are dropped. Returns how many processes were
/// terminated.
pub fn reap_recorded_zombies() -> usize {
    let mut reaped = 0;
    for t in recorded() {
        match state(&t) {
            GameState::Gone => forget(&t),
            GameState::Running { visible_window } => {
                if !is_recorded_zombie(visible_window, process_age_secs(&t)) {
                    continue;
                }
                if reap(&t) {
                    reaped += 1;
                }
                forget(&t);
            }
        }
    }
    reaped
}

#[cfg(target_os = "windows")]
mod imp {
    use super::{same_process, GameState, Tracked};
    use windows::Win32::Foundation::{CloseHandle, FALSE, FILETIME, HANDLE, WAIT_TIMEOUT};
    use windows::Win32::System::Threading::{
        GetProcessTimes, OpenProcess, TerminateProcess, WaitForSingleObject,
        PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE,
    };

    struct Handle(HANDLE);
    impl Drop for Handle {
        fn drop(&mut self) {
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }

    fn open(pid: u32, terminate: bool) -> Option<Handle> {
        if pid == 0 {
            return None;
        }
        let mut access = PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE;
        if terminate {
            access |= PROCESS_TERMINATE;
        }
        match unsafe { OpenProcess(access, FALSE, pid) } {
            Ok(h) if !h.is_invalid() => Some(Handle(h)),
            _ => None,
        }
    }

    fn creation_time(h: &Handle) -> Option<u64> {
        let (mut c, mut e, mut k, mut u) = (
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
        );
        unsafe { GetProcessTimes(h.0, &mut c, &mut e, &mut k, &mut u) }.ok()?;
        Some(((c.dwHighDateTime as u64) << 32) | c.dwLowDateTime as u64)
    }

    fn still_running(h: &Handle) -> bool {
        unsafe { WaitForSingleObject(h.0, 0) == WAIT_TIMEOUT }
    }

    /// True if the process image is a game child we spawn:
    /// `java.exe`/`javaw.exe` (JVM instances) or `rewo.exe` (Native).
    fn is_game_image(h: &Handle) -> bool {
        use windows::core::PWSTR;
        use windows::Win32::System::Threading::{QueryFullProcessImageNameW, PROCESS_NAME_WIN32};
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = unsafe {
            QueryFullProcessImageNameW(h.0, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len)
        };
        if ok.is_err() {
            return false;
        }
        let path = String::from_utf16_lossy(&buf[..len as usize]).to_ascii_lowercase();
        path.ends_with("\\java.exe")
            || path.ends_with("\\javaw.exe")
            || path.ends_with("\\rewo.exe")
    }

    pub fn process_creation_time(pid: u32) -> Option<u64> {
        creation_time(&open(pid, false)?)
    }

    /// The system clock as FILETIME ticks (the creation time's epoch).
    pub fn now_filetime() -> Option<u64> {
        /// Seconds from 1601-01-01 (FILETIME epoch) to 1970-01-01.
        const UNIX_EPOCH_AS_FILETIME_SECS: u64 = 11_644_473_600;
        let unix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?;
        Some((unix.as_secs() + UNIX_EPOCH_AS_FILETIME_SECS) * 10_000_000)
    }

    pub fn state(t: &Tracked) -> GameState {
        let Some(h) = open(t.pid, false) else {
            return GameState::Gone;
        };
        if !same_process(t, creation_time(&h)) || !still_running(&h) {
            return GameState::Gone;
        }
        GameState::Running {
            visible_window: pid_has_visible_window(t.pid),
        }
    }

    pub fn reap(t: &Tracked) -> bool {
        let Some(h) = open(t.pid, true) else {
            return false;
        };
        // Re-verify on the handle we terminate through: same process (not a
        // reused PID), still running, and one of our game images.
        if !same_process(t, creation_time(&h)) || !still_running(&h) || !is_game_image(&h) {
            return false;
        }
        log::warn!(
            "launch: reaping lingering game process (pid {}) that never exited — \
             it was holding the instance/DLL locks that block a new launch",
            t.pid
        );
        unsafe { TerminateProcess(h.0, 1) }.is_ok()
    }

    fn pid_has_visible_window(pid: u32) -> bool {
        use windows::Win32::Foundation::{BOOL, HWND, LPARAM};
        use windows::Win32::UI::WindowsAndMessaging::{
            EnumWindows, GetWindowThreadProcessId, IsWindowVisible,
        };

        struct Search {
            pid: u32,
            found: bool,
        }

        unsafe extern "system" fn enum_cb(hwnd: HWND, lparam: LPARAM) -> BOOL {
            unsafe {
                let s = &mut *(lparam.0 as *mut Search);
                if IsWindowVisible(hwnd).as_bool() {
                    let mut pid = 0u32;
                    GetWindowThreadProcessId(hwnd, Some(&mut pid));
                    if pid == s.pid {
                        s.found = true;
                        return BOOL(0); // stop
                    }
                }
                BOOL(1)
            }
        }

        let mut s = Search { pid, found: false };
        unsafe {
            let _ = EnumWindows(Some(enum_cb), LPARAM(&mut s as *mut _ as isize));
        }
        s.found
    }
}

/// Non-Windows: the zombie-JVM deadlock is a Windows-loader-lock symptom, so
/// nothing is ever reaped. A tracked process whose `/proc` entry still
/// exists is reported as running *with* a window, so it's never treated
/// as a zombie.
#[cfg(not(target_os = "windows"))]
mod imp {
    use super::{GameState, Tracked};

    /// Process age only gates a reap, and nothing is ever reaped here.
    pub fn now_filetime() -> Option<u64> {
        None
    }

    pub fn process_creation_time(pid: u32) -> Option<u64> {
        // Field 22 of /proc/<pid>/stat: start time in clock ticks since boot.
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        let after_comm = stat.rsplit_once(')')?.1;
        after_comm.split_whitespace().nth(19)?.parse().ok()
    }

    pub fn state(t: &Tracked) -> GameState {
        if process_creation_time(t.pid) == Some(t.created) {
            GameState::Running {
                visible_window: true,
            }
        } else {
            GameState::Gone
        }
    }

    pub fn reap(_t: &Tracked) -> bool {
        false
    }
}

pub use imp::process_creation_time;

/// Age of a tracked process in seconds, from its recorded creation time.
/// `None` where the platform can't say (nothing is reaped there anyway).
pub fn process_age_secs(t: &Tracked) -> Option<f32> {
    age_secs_between(t.created, imp::now_filetime()?)
}

/// Current state of a tracked game process.
pub fn state(t: &Tracked) -> GameState {
    imp::state(t)
}

/// Force-terminate `t` if it is still exactly that process (same PID *and*
/// creation time), still running, and a game image. Returns whether it was
/// terminated. Callers decide *whether* it's a zombie (see [`is_zombie`]).
pub fn reap(t: &Tracked) -> bool {
    imp::reap(t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pidfile_round_trips_and_ignores_legacy_bare_pids() {
        let recs = vec![
            Tracked {
                pid: 42,
                created: 133_000_000_000_000_000,
            },
            Tracked { pid: 7, created: 1 },
        ];
        assert_eq!(parse_records(&format_records(&recs)), recs);
        // A pre-creation-time pidfile held just "1234": unverifiable, ignored.
        assert!(parse_records("1234\n").is_empty());
        assert!(parse_records("garbage\n\n12 x\n").is_empty());
    }

    #[test]
    fn same_process_requires_matching_creation_time() {
        let t = Tracked {
            pid: 10,
            created: 99,
        };
        assert!(same_process(&t, Some(99)));
        assert!(
            !same_process(&t, Some(100)),
            "PID reused by a newer process"
        );
        assert!(!same_process(&t, None), "process gone");
    }

    #[test]
    fn zombie_classification() {
        // A game with a visible window is never a zombie.
        assert!(!is_zombie(true, true, true, 10_000.0));
        // Still starting up (no window yet, never seen one): not a zombie.
        assert!(!is_zombie(false, false, false, 60.0));
        // Windowless far past any plausible startup, no evidence: zombie.
        assert!(is_zombie(false, false, false, STARTUP_GRACE_SECS + 1.0));
        assert!(!is_zombie(false, false, false, STARTUP_GRACE_SECS - 1.0));
        // Its window was observed, then closed, but the process lingers.
        assert!(is_zombie(false, true, false, SPAWN_GRACE_SECS + 1.0));
    }

    #[test]
    fn a_log_marker_alone_never_makes_a_starting_game_a_zombie() {
        // The regression: a second Launch click after the log said "lwjgl
        // version" but before the window exists. The marker is a hint with
        // its own grace; inside it the game is left alone.
        for secs in [0.0, 5.0, SPAWN_GRACE_SECS + 1.0, MARKER_GRACE_SECS - 1.0] {
            assert!(!is_zombie(false, false, true, secs), "secs={secs}");
        }
        assert!(is_zombie(false, false, true, MARKER_GRACE_SECS + 1.0));
    }

    #[test]
    fn nothing_is_reaped_inside_the_spawn_grace() {
        for visible in [false, true] {
            for seen in [false, true] {
                for marker in [false, true] {
                    for secs in [0.0, 1.0, SPAWN_GRACE_SECS - 0.1] {
                        assert!(!is_zombie(visible, seen, marker, secs));
                    }
                }
            }
        }
    }

    #[test]
    fn recorded_processes_from_an_earlier_session_need_age_and_no_window() {
        assert!(!is_recorded_zombie(true, Some(10_000.0)));
        assert!(!is_recorded_zombie(false, Some(MARKER_GRACE_SECS - 1.0)));
        assert!(!is_recorded_zombie(false, None), "unknown age: never reap");
        assert!(is_recorded_zombie(false, Some(MARKER_GRACE_SECS + 1.0)));
    }

    #[test]
    fn age_is_filetime_ticks_over_ten_million_and_survives_clock_skew() {
        assert_eq!(age_secs_between(100, 100 + 30 * 10_000_000), Some(30.0));
        assert_eq!(age_secs_between(200, 100), None);
    }

    #[test]
    fn own_process_is_running_and_matches() {
        let pid = std::process::id();
        let Some(created) = process_creation_time(pid) else {
            return; // no /proc (e.g. macOS) — nothing to check
        };
        let t = Tracked { pid, created };
        assert!(matches!(state(&t), GameState::Running { .. }));
        let stale = Tracked {
            pid,
            created: created.wrapping_add(1),
        };
        assert_eq!(state(&stale), GameState::Gone);
    }
}
