//! The game directory: where `options.txt` lives (vanilla's `gameDirectory`,
//! `Options.java:1468` `new File(workingDirectory, "options.txt")`).
//!
//! Resolved once at start-up, explicitly, rather than taken from the process's
//! current directory: the launcher spawns `rewo` from wherever the launcher
//! itself runs, so a bare relative `options.txt` landed in the launcher's
//! working directory (and failed to save under Program Files).
//!
//! Precedence: `--game-dir`, then the launcher's `REWO_GAME_DIR` handoff (the
//! instance's game folder, REWO_PLAN §9.1's env contract), then the current
//! directory for a standalone run.

use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// The launcher's handoff variable.
pub const ENV: &str = "REWO_GAME_DIR";

static GAME_DIR: OnceLock<PathBuf> = OnceLock::new();

/// The precedence rule, as a pure function. An empty value counts as unset.
pub fn resolve(
    arg: Option<PathBuf>,
    env: Option<OsString>,
    cwd: impl FnOnce() -> std::io::Result<PathBuf>,
) -> PathBuf {
    arg.filter(|p| !p.as_os_str().is_empty())
        .or_else(|| env.filter(|v| !v.is_empty()).map(PathBuf::from))
        .unwrap_or_else(|| cwd().unwrap_or_else(|_| PathBuf::from(".")))
}

/// Resolve from the real environment and fix it for this process. The first
/// call wins; later calls return the directory already chosen.
pub fn init(arg: Option<PathBuf>) -> &'static Path {
    GAME_DIR.get_or_init(|| {
        let dir = resolve(arg, std::env::var_os(ENV), std::env::current_dir);
        log::info!("game directory: {}", dir.display());
        dir
    })
}

/// The game directory, resolving it from the environment if [`init`] was
/// never called (a subcommand that reads options without the `live` args).
pub fn get() -> &'static Path {
    match GAME_DIR.get() {
        Some(dir) => dir,
        None => init(None),
    }
}

/// `options.txt` inside `dir`.
pub fn options_file(dir: &Path) -> PathBuf {
    dir.join("options.txt")
}

/// Replace `path` with `contents` atomically: write a sibling temporary file,
/// flush it to disk, then rename it over the target. A crash or a full disk
/// mid-save leaves the old file intact instead of a truncated one that a
/// vanilla client sharing the directory would then read. (Vanilla itself
/// writes in place, `Options.java:1771`.) The parent directory is created if
/// missing, since a fresh instance folder may not exist yet.
pub fn write_atomic(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    let mut tmp_name = path.file_name().unwrap_or_default().to_os_string();
    tmp_name.push(format!(".{}.tmp", std::process::id()));
    let tmp = path.with_file_name(tmp_name);
    let written = (|| {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(contents)?;
        f.sync_all()
    })();
    // `rename` replaces an existing target on Windows too (MoveFileExW with
    // MOVEFILE_REPLACE_EXISTING).
    let result = written.and_then(|()| std::fs::rename(&tmp, path));
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cwd() -> std::io::Result<PathBuf> {
        Ok(PathBuf::from("C:/cwd"))
    }

    #[test]
    fn the_argument_beats_the_env_beats_the_current_directory() {
        let arg = Some(PathBuf::from("C:/arg"));
        let env = Some(OsString::from("C:/env"));
        assert_eq!(
            resolve(arg.clone(), env.clone(), cwd),
            PathBuf::from("C:/arg")
        );
        assert_eq!(resolve(None, env, cwd), PathBuf::from("C:/env"));
        assert_eq!(resolve(None, None, cwd), PathBuf::from("C:/cwd"));
    }

    #[test]
    fn empty_values_count_as_unset() {
        let got = resolve(Some(PathBuf::new()), Some(OsString::new()), cwd);
        assert_eq!(got, PathBuf::from("C:/cwd"));
    }

    #[test]
    fn an_unreadable_current_directory_falls_back_to_dot() {
        let got = resolve(None, None, || Err(std::io::ErrorKind::NotFound.into()));
        assert_eq!(got, PathBuf::from("."));
    }

    #[test]
    fn options_live_in_the_game_directory() {
        assert_eq!(
            options_file(Path::new("C:/inst/game")),
            Path::new("C:/inst/game").join("options.txt")
        );
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rewo-gamedir-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn an_atomic_write_creates_the_directory_and_replaces_the_file() {
        let dir = scratch("replace");
        let path = options_file(&dir.join("nested"));
        write_atomic(&path, b"ao:true\n").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"ao:true\n");
        write_atomic(&path, b"ao:false\n").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"ao:false\n");
        // No temporary file is left beside it.
        let names: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, vec![OsString::from("options.txt")]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_failed_rename_reports_the_error_and_leaves_no_temp_file() {
        let dir = scratch("fail");
        let path = options_file(&dir);
        write_atomic(&path, b"old\n").unwrap();
        // A target that cannot be replaced (a non-empty directory): the
        // temporary file is written, the rename fails, the error surfaces,
        // the temporary file is removed and nothing else in the directory
        // changes.
        let blocked = dir.join("blocked");
        std::fs::create_dir_all(blocked.join("child")).unwrap();
        assert!(write_atomic(&blocked, b"new\n").is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"old\n");
        let leftovers = std::fs::read_dir(&dir)
            .unwrap()
            .filter(|e| {
                e.as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .ends_with(".tmp")
            })
            .count();
        assert_eq!(leftovers, 0);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
