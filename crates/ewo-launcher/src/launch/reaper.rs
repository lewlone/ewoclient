//! Reap a lingering game JVM before the next launch.
//!
//! # Why this exists
//!
//! On Windows the game JVM's orderly shutdown can deadlock in native
//! teardown — `DLL_PROCESS_DETACH` under the Windows loader lock, with the
//! in-game HUD's second GL context and the WinRT SMTC media thread as the
//! prime suspects. The game window closes, but a headless zombie
//! `java.exe` lingers, holding `ewo_jni.dll` and the instance's files open.
//! That lock is what makes the *next* launch fail: natives can't be
//! re-extracted into the instance dir and the DLL can't be reloaded. To the
//! user this reads as "closing Minecraft makes it impossible to start a new
//! instance."
//!
//! The in-game mod arms its own shutdown watchdog (`EwoHudMod`), but that
//! only fires once an orderly JVM shutdown actually *begins*; a hang before
//! that point never triggers it. This launcher-side reaper is the backstop:
//! the launcher is a separate process that is not deadlocked, so it can
//! always terminate the zombie.
//!
//! # How it's wired
//!
//! - Each real launch records its child PID (`record`) — in memory on
//!   `App` and in a small pidfile so a zombie survives even a launcher
//!   restart.
//! - A cleanly-exited launch clears the record (`clear`), so the common
//!   case reaps nothing.
//! - `App::start_launch` calls `reap` on any still-recorded PID right
//!   before spawning the new JVM. Only a process that never reported exit
//!   (i.e. the zombie) is still on record, so a healthy prior launch is
//!   left alone in practice.
//!
//! Reaping is guarded against PID reuse: the target is terminated only if
//! it is still alive *and* its image is `java.exe`/`javaw.exe`. Across a
//! launcher restart the recorded PID could have been recycled by an
//! unrelated program; the image-name check keeps us from killing that.

use std::path::PathBuf;

const PIDFILE: &str = "last-launch.pid";

fn pidfile_path() -> Option<PathBuf> {
    let mut p = dirs::config_dir()?;
    p.push("EwoClient");
    p.push(PIDFILE);
    Some(p)
}

/// Persist the just-spawned JVM's PID so it can be reaped later, even if
/// the launcher is closed and reopened before the next launch.
pub fn record(pid: u32) {
    let Some(path) = pidfile_path() else { return };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(e) = std::fs::write(&path, pid.to_string()) {
        log::debug!("launch: could not write pidfile {}: {e}", path.display());
    }
}

/// Forget the recorded PID — called on a clean JVM exit, so the next
/// launch has nothing to reap.
pub fn clear() {
    if let Some(path) = pidfile_path() {
        let _ = std::fs::remove_file(path);
    }
}

fn read_recorded() -> Option<u32> {
    std::fs::read_to_string(pidfile_path()?)
        .ok()?
        .trim()
        .parse()
        .ok()
}

/// Reap a JVM recorded by a *previous launcher session* that is still
/// lingering (launcher was closed + reopened while a zombie held on).
/// Reads the pidfile, reaps if the process is still a live java, then
/// clears the record. In-session reaping goes through [`reap`] directly
/// with the live PID held on `App`.
pub fn reap_recorded() {
    if let Some(pid) = read_recorded() {
        reap(pid);
    }
    clear();
}

/// Force-terminate the given process if it is still alive and is a game
/// JVM. No-op if the PID is gone or belongs to something else.
#[cfg(target_os = "windows")]
pub fn reap(pid: u32) {
    use windows::Win32::Foundation::{CloseHandle, FALSE, WAIT_TIMEOUT};
    use windows::Win32::System::Threading::{
        OpenProcess, TerminateProcess, WaitForSingleObject, PROCESS_QUERY_LIMITED_INFORMATION,
        PROCESS_TERMINATE,
    };

    if pid == 0 {
        return;
    }

    unsafe {
        let handle = match OpenProcess(
            PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION,
            FALSE,
            pid,
        ) {
            // A valid handle means the process still exists. If it already
            // exited, OpenProcess fails and there is nothing to reap.
            Ok(h) if !h.is_invalid() => h,
            _ => return,
        };

        // `WAIT_TIMEOUT` = still running; `WAIT_OBJECT_0` = already
        // signalled (exited). Combined with the java image-name check,
        // this is the PID-reuse guard.
        let still_running = WaitForSingleObject(handle, 0) == WAIT_TIMEOUT;
        if still_running && process_is_java(handle) {
            log::warn!(
                "launch: reaping lingering game JVM (pid {pid}) that never exited — \
                 it was holding the instance/DLL locks that block a new launch"
            );
            // Non-zero exit code: anyone waiting on it sees a crash-ish
            // status rather than a clean 0.
            let _ = TerminateProcess(handle, 1);
        }

        let _ = CloseHandle(handle);
    }
}

/// True if the open process handle's executable is a game child we spawned:
/// `java.exe`/`javaw.exe` (JVM instances) or `rewo.exe` (Native instances).
#[cfg(target_os = "windows")]
unsafe fn process_is_java(handle: windows::Win32::Foundation::HANDLE) -> bool {
    use windows::core::PWSTR;
    use windows::Win32::System::Threading::{QueryFullProcessImageNameW, PROCESS_NAME_WIN32};

    let mut buf = [0u16; 260]; // MAX_PATH
    let mut len = buf.len() as u32;
    let ok = QueryFullProcessImageNameW(handle, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len);
    if ok.is_err() {
        return false;
    }
    let path = String::from_utf16_lossy(&buf[..len as usize]).to_ascii_lowercase();
    path.ends_with("java.exe") || path.ends_with("javaw.exe") || path.ends_with("rewo.exe")
}

/// Non-Windows: the zombie-JVM deadlock is a Windows-loader-lock symptom,
/// so this is a no-op. If a lingering process ever bites on Linux, this is
/// where a `SIGKILL` would go.
#[cfg(not(target_os = "windows"))]
pub fn reap(pid: u32) {
    let _ = pid;
}

/// Reap every **windowless** `java.exe`/`javaw.exe` on the system and return
/// how many were killed.
///
/// The PID/pidfile reaper only knows about launches *this* launcher
/// recorded. A zombie left by an older launcher build (or any launch whose
/// `Started` event never fired) is invisible to it — yet it still holds an
/// instance's `natives/` dir and the DLL locked, silently forcing the next
/// launch to fall back to the synthetic animation. This is the catch-all:
/// called only when native extraction fails on a lock, it clears any lingering
/// game JVM so the extraction can be retried.
///
/// Windowless is the zombie signal: a live game owns a visible top-level
/// window; a hung post-shutdown JVM does not. A java process the user is
/// actively playing is therefore left alone.
#[cfg(target_os = "windows")]
pub fn reap_windowless_java() -> usize {
    use windows::Win32::Foundation::{CloseHandle, FALSE, HANDLE, WAIT_TIMEOUT};
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    use windows::Win32::System::Threading::{
        OpenProcess, TerminateProcess, WaitForSingleObject, PROCESS_QUERY_LIMITED_INFORMATION,
        PROCESS_TERMINATE,
    };

    let windowed = pids_with_visible_windows();
    let mut reaped = 0usize;

    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
            return 0;
        };
        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        if Process32FirstW(snap, &mut entry).is_ok() {
            loop {
                let name = wide_to_string(&entry.szExeFile).to_ascii_lowercase();
                let pid = entry.th32ProcessID;
                let is_java = name == "java.exe" || name == "javaw.exe";
                if is_java && pid != 0 && !windowed.contains(&pid) {
                    if let Ok(h) = OpenProcess(
                        PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION,
                        FALSE,
                        pid,
                    ) {
                        if !h.is_invalid() {
                            // Still alive? (WAIT_TIMEOUT = running.)
                            if WaitForSingleObject(h, 0) == WAIT_TIMEOUT {
                                log::warn!(
                                    "launch: reaping windowless {name} (pid {pid}) — a zombie \
                                     game JVM holding instance files locked"
                                );
                                if TerminateProcess(h, 1).is_ok() {
                                    reaped += 1;
                                }
                            }
                            let _ = CloseHandle(h);
                        }
                    }
                }
                if Process32NextW(snap, &mut entry).is_err() {
                    break;
                }
            }
        }
        let _: HANDLE = snap;
        let _ = CloseHandle(snap);
    }
    reaped
}

/// PIDs that own at least one visible top-level window.
#[cfg(target_os = "windows")]
fn pids_with_visible_windows() -> std::collections::HashSet<u32> {
    use std::collections::HashSet;
    use windows::Win32::Foundation::{BOOL, HWND, LPARAM, TRUE};
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowThreadProcessId, IsWindowVisible,
    };

    unsafe extern "system" fn enum_cb(hwnd: HWND, lparam: LPARAM) -> BOOL {
        unsafe {
            if IsWindowVisible(hwnd).as_bool() {
                let mut pid = 0u32;
                GetWindowThreadProcessId(hwnd, Some(&mut pid));
                if pid != 0 {
                    let set = &mut *(lparam.0 as *mut HashSet<u32>);
                    set.insert(pid);
                }
            }
        }
        TRUE // keep enumerating
    }

    let mut set: HashSet<u32> = HashSet::new();
    unsafe {
        let _ = EnumWindows(Some(enum_cb), LPARAM(&mut set as *mut _ as isize));
    }
    set
}

/// Convert a null-terminated wide buffer (e.g. `PROCESSENTRY32W::szExeFile`)
/// to a `String`, stopping at the first NUL.
#[cfg(target_os = "windows")]
fn wide_to_string(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

/// Non-Windows stub — see [`reap`].
#[cfg(not(target_os = "windows"))]
pub fn reap_windowless_java() -> usize {
    0
}
