//! Moving a deleted instance's folder to the Recycle Bin (Windows) or the
//! Trash (Linux). **Never a permanent delete**: if the move fails, the folder
//! stays exactly where it is and the caller shows the error. Nothing in this
//! module removes a file or directory itself.
//!
//! Only an instance's own folder, `<instances root>/<id>`, is ever handed to
//! the recycler: [`instance_folder`] validates the id as a single plain path
//! component and checks the resolved folder is a direct child of the
//! instances root (so a link or junction pointing elsewhere is refused).
//! Shared libraries, assets and versions live outside `instances/` and are
//! never touched.

use std::path::{Path, PathBuf};

/// Sends a path to the OS trash. Behind a trait so the rules around it
/// (validation, never falling back to a permanent delete) are tested with a
/// fake on every host.
pub trait Recycler {
    /// Move `path` (a file or a whole folder) to the trash, or say why not.
    /// An implementation must never delete it for good.
    fn recycle(&self, path: &Path) -> Result<(), String>;
}

/// What a successful delete did to the disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Removed {
    /// The instance never had a folder (never launched): nothing to move.
    NoFolder,
    /// The folder is in the trash.
    Recycled,
}

/// The folder of instance `id` under `root`, validated: `Ok(None)` when it
/// does not exist, `Err` when the id is not a plain single path component or
/// the folder is not a real directory directly inside `root`.
pub fn instance_folder(root: &Path, id: &str) -> Result<Option<PathBuf>, String> {
    if id.is_empty() || crate::util::validate_name(id, 64).ok() != Some(id) {
        return Err(format!("\"{id}\" is not a valid instance folder name"));
    }
    let folder = root.join(id);
    match std::fs::symlink_metadata(&folder) {
        Ok(meta) => {
            if meta.file_type().is_symlink() || !meta.is_dir() {
                return Err(format!("{} is not a plain folder", folder.display()));
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("cannot read {}: {e}", folder.display())),
    }
    // Resolve both ends: a junction/mount point or a link in the path would
    // lead out of the instances root.
    let canon_root = std::fs::canonicalize(root)
        .map_err(|e| format!("cannot resolve {}: {e}", root.display()))?;
    let canon_folder = std::fs::canonicalize(&folder)
        .map_err(|e| format!("cannot resolve {}: {e}", folder.display()))?;
    if canon_folder.parent() != Some(canon_root.as_path()) {
        return Err(format!(
            "{} is not inside the instances folder",
            folder.display()
        ));
    }
    Ok(Some(folder))
}

/// Move instance `id`'s folder to the trash through `recycler`. On any error
/// the folder is left in place: there is no fallback to a permanent delete.
/// A recycler that reports success while the folder is still there is an
/// error too.
pub fn recycle_instance_folder(
    root: &Path,
    id: &str,
    recycler: &dyn Recycler,
) -> Result<Removed, String> {
    let Some(folder) = instance_folder(root, id)? else {
        return Ok(Removed::NoFolder);
    };
    recycler.recycle(&folder)?;
    if folder.exists() {
        return Err(format!(
            "{} is still there after the move",
            folder.display()
        ));
    }
    Ok(Removed::Recycled)
}

/// The platform's trash.
#[derive(Default)]
pub struct OsRecycler {
    /// The launcher window's native handle (Windows `HWND`). The shell's
    /// "permanently delete?" prompt (`FOF_WANTNUKEWARNING`) is owned by this
    /// window so it opens in front of the launcher; with no owner it could
    /// sit behind it while the modal reads "Moving..." with nothing to
    /// click. Unused elsewhere.
    pub owner: Option<isize>,
}

impl Recycler for OsRecycler {
    fn recycle(&self, path: &Path) -> Result<(), String> {
        os_recycle(path, self.owner)
    }
}

#[cfg(windows)]
fn os_recycle(path: &Path, owner: Option<isize>) -> Result<(), String> {
    use windows::core::PCWSTR;
    use windows::Win32::UI::Shell::{
        SHFileOperationW, FOF_ALLOWUNDO, FOF_NOERRORUI, FOF_SILENT, FOF_WANTNUKEWARNING, FO_DELETE,
        SHFILEOPSTRUCTW,
    };

    // SHFileOperation fails on a `\\?\` path, and a relative one is not sent
    // to the Recycle Bin at all, so the path must be plain and absolute.
    let text = path.to_string_lossy();
    if text.starts_with(r"\\?\") || !path.is_absolute() {
        return Err(format!("{} is not a plain absolute path", path.display()));
    }
    // `pFrom` is a double-NUL-terminated list.
    let mut from: Vec<u16> = text.encode_utf16().collect();
    from.extend([0, 0]);

    // FOF_ALLOWUNDO sends it to the Recycle Bin. FOF_WANTNUKEWARNING keeps
    // Windows asking (this flag partly overrides FOF_NOCONFIRMATION, which is
    // deliberately not set) when an item could not be recycled, e.g. it is
    // too large for the bin or the drive has none, so a permanent delete is
    // never silent; declining aborts the operation, which is an error here.
    let flags = FOF_ALLOWUNDO.0 | FOF_NOERRORUI.0 | FOF_SILENT.0 | FOF_WANTNUKEWARNING.0;
    let mut op = SHFILEOPSTRUCTW {
        hwnd: windows::Win32::Foundation::HWND(owner.unwrap_or(0) as *mut _),
        wFunc: FO_DELETE,
        pFrom: PCWSTR(from.as_ptr()),
        fFlags: flags as u16,
        ..Default::default()
    };
    let code = unsafe { SHFileOperationW(&mut op) };
    if code != 0 {
        return Err(format!("the shell refused it (error {code:#x})"));
    }
    if op.fAnyOperationsAborted.as_bool() {
        return Err("the move was cancelled or not allowed".to_string());
    }
    Ok(())
}

/// How long `gio trash` gets before it is killed. A wedged D-Bus/GVfs
/// session can hang it forever, and the delete modal ignores Cancel and Esc
/// while the worker runs.
#[cfg(any(target_os = "linux", test))]
const GIO_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Why [`run_with_timeout`] gave no exit status.
#[cfg(any(target_os = "linux", test))]
#[derive(Debug)]
enum RunError {
    /// The program could not be started (`NotFound` when it is not installed).
    Spawn(std::io::Error),
    /// It was still running at the deadline and was killed.
    TimedOut,
}

/// Run `cmd` (stdin closed, stderr captured) and wait at most `timeout`; a
/// child still running then is killed and reaped. Returns its exit status
/// and stderr.
#[cfg(any(target_os = "linux", test))]
fn run_with_timeout(
    mut cmd: std::process::Command,
    timeout: std::time::Duration,
) -> Result<(std::process::ExitStatus, String), RunError> {
    use std::io::Read;
    use std::process::Stdio;
    use std::time::Instant;

    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(RunError::Spawn)?;
    // Drain stderr on its own thread so a chatty child can't block on a
    // full pipe while we poll for its exit.
    let mut stderr = child.stderr.take();
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        if let Some(s) = stderr.as_mut() {
            let _ = s.read_to_end(&mut bytes);
        }
        String::from_utf8_lossy(&bytes).into_owned()
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() >= deadline => break None,
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(20)),
            Err(e) => return Err(RunError::Spawn(e)),
        }
    };
    let Some(status) = status else {
        let _ = child.kill();
        let _ = child.wait();
        // The pipe closes with the child, so the reader ends.
        let _ = reader.join();
        return Err(RunError::TimedOut);
    };
    Ok((status, reader.join().unwrap_or_default()))
}

#[cfg(target_os = "linux")]
fn os_recycle(path: &Path, _owner: Option<isize>) -> Result<(), String> {
    use std::process::Command;

    // `gio trash` knows every mount's trash directory and the desktop's own
    // conventions; it is the best option when present. It gets a deadline:
    // on a timeout the delete reports an error instead of hanging, and the
    // folder is left where it was.
    let mut gio_note = String::new();
    let mut gio = Command::new("gio");
    gio.arg("trash").arg("--").arg(path);
    match run_with_timeout(gio, GIO_TIMEOUT) {
        Ok((status, _)) if status.success() => return Ok(()),
        Ok((_, stderr)) => gio_note = format!("gio trash: {}", stderr.trim()),
        Err(RunError::TimedOut) => {
            return Err(format!(
                "gio trash did not finish within {} s and was stopped; the folder was not moved",
                GIO_TIMEOUT.as_secs()
            ))
        }
        Err(RunError::Spawn(e)) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(RunError::Spawn(e)) => gio_note = format!("gio trash: {e}"),
    }
    // Otherwise the freedesktop.org Trash spec's home trash, by rename (same
    // filesystem only). A cross-device folder is an error, not a copy+delete.
    let home_trash = dirs::data_dir()
        .map(|d| d.join("Trash"))
        .ok_or_else(|| "no data directory for the Trash".to_string())?;
    trash::move_into(&home_trash, path, std::time::SystemTime::now()).map_err(|e| {
        if gio_note.is_empty() {
            e
        } else {
            format!("{e} ({gio_note})")
        }
    })
}

#[cfg(not(any(windows, target_os = "linux")))]
fn os_recycle(_path: &Path, _owner: Option<isize>) -> Result<(), String> {
    Err("moving to the trash is not supported on this platform".to_string())
}

/// The freedesktop.org Trash specification, home-trash half: the folder is
/// renamed into `<trash>/files/` and described by
/// `<trash>/info/<name>.trashinfo`. Pure `std`, so it is built and tested on
/// every host even though only Linux uses it.
#[cfg(any(target_os = "linux", test))]
mod trash {
    use std::path::Path;
    use std::time::{SystemTime, UNIX_EPOCH};

    /// Move `path` into the trash directory `trash`. Never copies or deletes:
    /// if the rename fails (another filesystem, permissions) the source stays
    /// and this is an error.
    pub(super) fn move_into(trash: &Path, path: &Path, now: SystemTime) -> Result<(), String> {
        let abs = std::fs::canonicalize(path)
            .map_err(|e| format!("cannot resolve {}: {e}", path.display()))?;
        let base = abs
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| format!("{} has no usable name", abs.display()))?
            .to_string();
        let (files, info) = (trash.join("files"), trash.join("info"));
        for d in [&files, &info] {
            std::fs::create_dir_all(d)
                .map_err(|e| format!("cannot create {}: {e}", d.display()))?;
        }
        // A free name in both directories; the .trashinfo is created
        // exclusively so two deletes can never claim the same name.
        for n in 0..1000u32 {
            let name = if n == 0 {
                base.clone()
            } else {
                format!("{base}.{n}")
            };
            let (dest, info_path) = (files.join(&name), info.join(format!("{name}.trashinfo")));
            if dest.exists() {
                continue;
            }
            let text = info_text(&abs, now);
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&info_path)
            {
                Ok(mut f) => {
                    use std::io::Write;
                    if let Err(e) = f.write_all(text.as_bytes()) {
                        let _ = std::fs::remove_file(&info_path);
                        return Err(format!("cannot write {}: {e}", info_path.display()));
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(format!("cannot write {}: {e}", info_path.display())),
            }
            return match std::fs::rename(&abs, &dest) {
                Ok(()) => Ok(()),
                Err(e) => {
                    // Not moved: drop only the description we just made.
                    let _ = std::fs::remove_file(&info_path);
                    Err(format!("cannot move {} into the Trash: {e}", abs.display()))
                }
            };
        }
        Err("no free name in the Trash".to_string())
    }

    /// The `.trashinfo` file's text for the original absolute path `abs`.
    pub(super) fn info_text(abs: &Path, now: SystemTime) -> String {
        format!(
            "[Trash Info]\nPath={}\nDeletionDate={}\n",
            encode_path(abs),
            iso_date(now)
        )
    }

    /// The path percent-encoded as the spec asks: `/` stays, everything
    /// outside the unreserved set is `%XX`.
    pub(super) fn encode_path(p: &Path) -> String {
        p.to_string_lossy()
            .split('/')
            .map(|seg| urlencoding::encode(seg).into_owned())
            .collect::<Vec<_>>()
            .join("/")
    }

    /// `YYYY-MM-DDThh:mm:ss`. UTC: the spec wants local time, and `std` has
    /// no time zone; trash managers only display it.
    pub(super) fn iso_date(t: SystemTime) -> String {
        let secs = t.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
        let (days, rem) = (secs / 86_400, secs % 86_400);
        let (y, m, d) = civil_from_days(days as i64);
        format!(
            "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}",
            rem / 3600,
            rem % 3600 / 60,
            rem % 60
        )
    }

    /// Days since 1970-01-01 to a (year, month, day) civil date.
    fn civil_from_days(z: i64) -> (i64, u32, u32) {
        let z = z + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z.rem_euclid(146_097);
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
        let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
        let y = yoe + era * 400 + i64::from(m <= 2);
        (y, m, d)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::fs;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ewo-recycle-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A trash that is just another directory.
    struct FakeBin(PathBuf);
    impl Recycler for FakeBin {
        fn recycle(&self, path: &Path) -> Result<(), String> {
            fs::rename(path, self.0.join(path.file_name().unwrap())).map_err(|e| e.to_string())
        }
    }

    /// A recycler that cannot do it (no Recycle Bin on this drive, ...) and
    /// records what it was asked to move.
    struct Failing(RefCell<Vec<PathBuf>>);
    impl Recycler for Failing {
        fn recycle(&self, path: &Path) -> Result<(), String> {
            self.0.borrow_mut().push(path.to_path_buf());
            Err("the Recycle Bin is not available on this drive".into())
        }
    }

    /// Claims success and does nothing.
    struct Liar;
    impl Recycler for Liar {
        fn recycle(&self, _: &Path) -> Result<(), String> {
            Ok(())
        }
    }

    fn world(root: &Path, id: &str) -> PathBuf {
        let dir = root.join(id);
        fs::create_dir_all(dir.join("saves/World 1")).unwrap();
        fs::write(dir.join("saves/World 1/level.dat"), b"precious").unwrap();
        dir
    }

    #[test]
    fn the_folder_moves_to_the_bin_and_only_that_folder() {
        let tmp = scratch("moves");
        let (root, bin) = (tmp.join("instances"), tmp.join("bin"));
        fs::create_dir_all(&bin).unwrap();
        world(&root, "velvet-1");
        world(&root, "other-2");
        let shared = tmp.join("shared");
        fs::create_dir_all(&shared).unwrap();

        let got = recycle_instance_folder(&root, "velvet-1", &FakeBin(bin.clone()));
        assert_eq!(got, Ok(Removed::Recycled));
        assert!(!root.join("velvet-1").exists());
        assert_eq!(
            fs::read(bin.join("velvet-1/saves/World 1/level.dat")).unwrap(),
            b"precious",
            "contents are in the bin, intact"
        );
        assert!(root.join("other-2/saves").exists(), "siblings untouched");
        assert!(shared.exists(), "shared data untouched");
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn a_failed_move_keeps_the_folder_and_never_falls_back_to_deleting_it() {
        let tmp = scratch("fails");
        let root = tmp.join("instances");
        let dir = world(&root, "velvet-1");
        let rec = Failing(RefCell::new(Vec::new()));

        let err = recycle_instance_folder(&root, "velvet-1", &rec).unwrap_err();
        assert!(err.contains("Recycle Bin"), "{err}");
        assert_eq!(
            rec.0.borrow().as_slice(),
            std::slice::from_ref(&dir),
            "asked exactly once"
        );
        assert_eq!(
            fs::read(dir.join("saves/World 1/level.dat")).unwrap(),
            b"precious",
            "folder and contents are still there"
        );
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn a_recycler_that_claims_success_but_left_the_folder_is_an_error() {
        let tmp = scratch("liar");
        let root = tmp.join("instances");
        world(&root, "velvet-1");
        let err = recycle_instance_folder(&root, "velvet-1", &Liar).unwrap_err();
        assert!(err.contains("still there"), "{err}");
        assert!(root.join("velvet-1").exists());
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn an_instance_that_never_had_a_folder_has_nothing_to_recycle() {
        let tmp = scratch("nofolder");
        let root = tmp.join("instances");
        fs::create_dir_all(&root).unwrap();
        let rec = Failing(RefCell::new(Vec::new()));
        assert_eq!(
            recycle_instance_folder(&root, "never-launched", &rec),
            Ok(Removed::NoFolder)
        );
        assert!(rec.0.borrow().is_empty(), "the recycler was not involved");
        // Not even an instances root yet.
        assert_eq!(
            recycle_instance_folder(&tmp.join("absent"), "x", &rec),
            Ok(Removed::NoFolder)
        );
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn only_a_plain_single_component_id_can_name_a_folder() {
        let tmp = scratch("ids");
        let root = tmp.join("instances");
        world(&root, "ok-1");
        // A sibling of the root that a traversing id would reach.
        fs::create_dir_all(tmp.join("shared")).unwrap();
        let rec = Failing(RefCell::new(Vec::new()));
        for bad in [
            "",
            "..",
            ".",
            "../shared",
            "..\\shared",
            "a/b",
            "a\\b",
            "C:",
            "C:\\Windows",
            "/etc",
            "x..y",
            "con",
        ] {
            assert!(
                recycle_instance_folder(&root, bad, &rec).is_err(),
                "{bad:?} must be refused"
            );
        }
        assert!(rec.0.borrow().is_empty(), "nothing was ever handed over");
        assert!(tmp.join("shared").exists() && root.join("ok-1").exists());
        // A padded id is not the folder's name either.
        assert!(instance_folder(&root, " ok-1").is_err());
        assert_eq!(instance_folder(&root, "ok-1"), Ok(Some(root.join("ok-1"))));
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn a_file_where_the_folder_should_be_is_refused() {
        let tmp = scratch("file");
        let root = tmp.join("instances");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("oops"), b"x").unwrap();
        assert!(instance_folder(&root, "oops").is_err());
        let _ = fs::remove_dir_all(&tmp);
    }

    #[cfg(unix)]
    #[test]
    fn a_link_out_of_the_instances_folder_is_refused() {
        let tmp = scratch("link");
        let root = tmp.join("instances");
        fs::create_dir_all(&root).unwrap();
        let outside = world(&tmp, "outside");
        std::os::unix::fs::symlink(&outside, root.join("sneaky")).unwrap();
        let rec = Failing(RefCell::new(Vec::new()));
        assert!(recycle_instance_folder(&root, "sneaky", &rec).is_err());
        assert!(rec.0.borrow().is_empty());
        assert!(outside.join("saves").exists());
        let _ = fs::remove_dir_all(&tmp);
    }

    /// Sends a real folder to the real Recycle Bin (it stays there, a few
    /// bytes; empty the bin to clear it): `cargo test -p ewo-launcher
    /// windows_recycle_bin -- --ignored`.
    #[cfg(windows)]
    #[test]
    #[ignore = "touches the real Recycle Bin"]
    fn windows_recycle_bin_takes_the_folder() {
        let tmp = scratch("winbin");
        let root = tmp.join("instances");
        let dir = world(&root, "velvet-1");
        let got = recycle_instance_folder(&root, "velvet-1", &OsRecycler::default());
        assert_eq!(got, Ok(Removed::Recycled));
        assert!(!dir.exists());
        let _ = fs::remove_dir_all(&tmp);
    }

    /// A program that outlives any test timeout, as a command.
    fn long_sleep() -> std::process::Command {
        #[cfg(windows)]
        {
            let mut c = std::process::Command::new("ping");
            c.args(["-n", "60", "127.0.0.1"]);
            c
        }
        #[cfg(not(windows))]
        {
            let mut c = std::process::Command::new("sleep");
            c.arg("60");
            c
        }
    }

    #[test]
    fn a_hung_trash_command_is_killed_at_the_deadline() {
        let t0 = std::time::Instant::now();
        let got = run_with_timeout(long_sleep(), std::time::Duration::from_millis(300));
        assert!(matches!(got, Err(RunError::TimedOut)), "{got:?}");
        assert!(
            t0.elapsed() < std::time::Duration::from_secs(20),
            "returned at the deadline, not when the child exited"
        );
    }

    #[test]
    fn a_quick_command_reports_status_and_stderr_and_a_missing_one_is_notfound() {
        let quick = if cfg!(windows) {
            let mut c = std::process::Command::new("cmd");
            c.args(["/C", "echo oops 1>&2 & exit 3"]);
            c
        } else {
            let mut c = std::process::Command::new("sh");
            c.args(["-c", "echo oops >&2; exit 3"]);
            c
        };
        let (status, err) = run_with_timeout(quick, GIO_TIMEOUT).unwrap();
        assert_eq!(status.code(), Some(3));
        assert!(err.contains("oops"), "{err:?}");
        let missing = std::process::Command::new("ewo-no-such-program-xyz");
        match run_with_timeout(missing, GIO_TIMEOUT) {
            Err(RunError::Spawn(e)) => assert_eq!(e.kind(), std::io::ErrorKind::NotFound),
            other => panic!("{other:?}"),
        }
    }

    // The freedesktop.org Trash half, exercised on every host.

    #[test]
    fn the_home_trash_gets_the_folder_and_a_trashinfo() {
        let tmp = scratch("trash");
        let root = tmp.join("instances");
        let dir = world(&root, "velvet-1");
        let trash_dir = tmp.join("Trash");
        let now = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);

        trash::move_into(&trash_dir, &dir, now).unwrap();
        assert!(!dir.exists());
        assert_eq!(
            fs::read(trash_dir.join("files/velvet-1/saves/World 1/level.dat")).unwrap(),
            b"precious"
        );
        let info = fs::read_to_string(trash_dir.join("info/velvet-1.trashinfo")).unwrap();
        assert!(info.starts_with("[Trash Info]\nPath="), "{info}");
        assert!(info.contains("velvet-1"), "{info}");
        assert!(info.contains("DeletionDate=2023-11-14T22:13:20"), "{info}");

        // A second folder of the same name gets a free name, nothing is
        // overwritten.
        let again = world(&root, "velvet-1");
        trash::move_into(&trash_dir, &again, now).unwrap();
        assert!(trash_dir.join("files/velvet-1.1/saves").exists());
        assert!(trash_dir.join("info/velvet-1.1.trashinfo").exists());
        assert!(trash_dir.join("files/velvet-1/saves").exists());
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn a_trash_that_cannot_take_the_folder_leaves_it_alone() {
        let tmp = scratch("trash-fail");
        let root = tmp.join("instances");
        let dir = world(&root, "velvet-1");
        // A file where the trash directory should be: create_dir_all fails.
        let blocked = tmp.join("Trash");
        fs::write(&blocked, b"not a dir").unwrap();
        let err = trash::move_into(&blocked, &dir, std::time::SystemTime::now()).unwrap_err();
        assert!(err.contains("cannot create"), "{err}");
        assert!(dir.join("saves/World 1/level.dat").exists());
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn trashinfo_paths_are_percent_encoded_and_dates_are_iso() {
        assert_eq!(
            trash::encode_path(Path::new("/home/a b/ewo/instances/x#1")),
            "/home/a%20b/ewo/instances/x%231"
        );
        assert_eq!(
            trash::iso_date(std::time::UNIX_EPOCH),
            "1970-01-01T00:00:00"
        );
        let leap = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_709_164_800 + 3661);
        assert_eq!(trash::iso_date(leap), "2024-02-29T01:01:01");
    }
}
