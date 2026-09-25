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
//! running, must be `java(w).exe`/`rewo.exe`, and must own no visible
//! window (a live game always does; a zombie never does).

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

/// Seconds a freshly spawned game may run without a visible window before
/// it's considered a zombie (JVM startup + loading screens take a while).
pub const STARTUP_GRACE_SECS: f32 = 300.0;

/// Whether a still-running tracked game should be treated as a zombie:
/// no visible window, and either its window was already seen once (the
/// game closed it) or it's been windowless far longer than startup takes.
pub fn is_zombie(has_visible_window: bool, window_was_seen: bool, secs_since_start: f32) -> bool {
    !has_visible_window && (window_was_seen || secs_since_start > STARTUP_GRACE_SECS)
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
    Running { visible_window: bool },
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
/// visible window). Live, windowed games are left alone; records of
/// processes that are gone (or whose PID was reused) are dropped. Returns
/// how many processes were terminated.
pub fn reap_recorded_zombies() -> usize {
    let mut reaped = 0;
    for t in recorded() {
        match state(&t) {
            GameState::Gone => forget(&t),
            GameState::Running { visible_window: false } => {
                if reap(&t) {
                    reaped += 1;
                }
                forget(&t);
            }
            GameState::Running { visible_window: true } => {}
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
        path.ends_with("\\java.exe") || path.ends_with("\\javaw.exe") || path.ends_with("\\rewo.exe")
    }

    pub fn process_creation_time(pid: u32) -> Option<u64> {
        creation_time(&open(pid, false)?)
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

    pub fn process_creation_time(pid: u32) -> Option<u64> {
        // Field 22 of /proc/<pid>/stat: start time in clock ticks since boot.
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        let after_comm = stat.rsplit_once(')')?.1;
        after_comm.split_whitespace().nth(19)?.parse().ok()
    }

    pub fn state(t: &Tracked) -> GameState {
        if process_creation_time(t.pid) == Some(t.created) {
            GameState::Running { visible_window: true }
        } else {
            GameState::Gone
        }
    }

    pub fn reap(_t: &Tracked) -> bool {
        false
    }
}

pub use imp::process_creation_time;

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
            Tracked { pid: 42, created: 133_000_000_000_000_000 },
            Tracked { pid: 7, created: 1 },
        ];
        assert_eq!(parse_records(&format_records(&recs)), recs);
        // A pre-creation-time pidfile held just "1234": unverifiable, ignored.
        assert!(parse_records("1234\n").is_empty());
        assert!(parse_records("garbage\n\n12 x\n").is_empty());
    }

    #[test]
    fn same_process_requires_matching_creation_time() {
        let t = Tracked { pid: 10, created: 99 };
        assert!(same_process(&t, Some(99)));
        assert!(!same_process(&t, Some(100)), "PID reused by a newer process");
        assert!(!same_process(&t, None), "process gone");
    }

    #[test]
    fn zombie_classification() {
        // A game with a visible window is never a zombie.
        assert!(!is_zombie(true, true, 10_000.0));
        // Still starting up (no window yet, never seen one): not a zombie.
        assert!(!is_zombie(false, false, 30.0));
        // Its window closed but the process lingers: zombie.
        assert!(is_zombie(false, true, 30.0));
        // Windowless far past any plausible startup: zombie.
        assert!(is_zombie(false, false, STARTUP_GRACE_SECS + 1.0));
    }

    #[test]
    fn own_process_is_running_and_matches() {
        let pid = std::process::id();
        let Some(created) = process_creation_time(pid) else {
            return; // no /proc (e.g. macOS) — nothing to check
        };
        let t = Tracked { pid, created };
        assert!(matches!(state(&t), GameState::Running { .. }));
        let stale = Tracked { pid, created: created.wrapping_add(1) };
        assert_eq!(state(&stale), GameState::Gone);
    }
}
