//! Spawn a child JVM process and stream its stdout/stderr back to the
//! UI thread via mpsc.
//!
//! Threading model: one worker thread per launch.
//!   - Main thread spawns `Child` + drains the launching screen's mpsc.
//!   - Two reader threads (one stdout, one stderr) read line-by-line and
//!     forward each line as a `LaunchLogLine` event. They exit when the
//!     pipe closes (= JVM exited).
//!   - The supervisor thread waits on the child, emits `Exited(code)`,
//!     joins the readers.

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::mpsc::Sender;
use std::thread;

use super::plan::LaunchPlan;

#[derive(Debug)]
pub enum LaunchEvent {
    /// Process spawned — UI flips to the streaming-log layout. Carries the
    /// child PID + its OS creation time so the launcher can reap exactly
    /// this process if it later zombies on exit (see `launch::reaper`).
    Started { pid: u32, created: Option<u64> },
    /// One line of output (stdout or stderr — we don't distinguish today;
    /// the JVM mostly logs to stderr but Minecraft's own logger goes to
    /// stdout). Includes a hint via `Severity` for log-coloring.
    Line { severity: Severity, text: String },
    /// Process exited. UI returns to the Instances screen, or shows an
    /// error variant of the pbar if the code is non-zero.
    Exited(Option<i32>),
    /// Spawn itself failed (e.g. `java` not on PATH). UI surfaces the
    /// error inline on the launching screen.
    SpawnFailed(String),
}

#[derive(Debug, Clone, Copy)]
pub enum Severity {
    /// stdout — Minecraft's own logger output. Treated as info.
    Info,
    /// stderr — JVM warnings, startup messages, native library traces.
    /// Often noisy but not actually errors.
    Warn,
}

/// A native (non-JVM) child — Rewo. Same event model as the JVM path.
#[derive(Debug)]
pub struct NativePlan {
    pub program: std::path::PathBuf,
    pub args: Vec<String>,
    /// REWO_* handoff contract (REWO_PLAN.md §9.1) — env, never argv.
    pub envs: Vec<(String, String)>,
}

/// Spawn the child on a worker thread + start the readers. Returns
/// immediately. Caller should poll the receiver each frame.
pub fn spawn(plan: LaunchPlan, tx: Sender<LaunchEvent>) -> thread::JoinHandle<()> {
    thread::Builder::new()
        .name("ewo-launch-supervisor".into())
        .spawn(move || run_launch(plan, tx))
        .expect("spawn launch supervisor thread")
}

/// Spawn a native binary (Rewo) with the same supervision + log streaming.
pub fn spawn_native(plan: NativePlan, tx: Sender<LaunchEvent>) -> thread::JoinHandle<()> {
    thread::Builder::new()
        .name("ewo-launch-native".into())
        .spawn(move || {
            log::info!(
                "launch: {} {}",
                plan.program.display(),
                redact_args(&plan.args).join(" ")
            );
            let mut cmd = Command::new(&plan.program);
            cmd.args(&plan.args)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .stdin(Stdio::null());
            scrub_inherited_secrets(&mut cmd);
            // The REWO_* contract is passed deliberately (after the scrub).
            for (key, value) in &plan.envs {
                cmd.env(key, value);
            }
            super::no_window(&mut cmd);
            run_child(cmd, tx, &plan.program.display().to_string());
        })
        .expect("spawn native supervisor thread")
}

fn run_launch(plan: LaunchPlan, tx: Sender<LaunchEvent>) {
    log::info!(
        "launch: {} {} {} {}",
        plan.jvm_path.display(),
        redact_args(&plan.jvm_args).join(" "),
        plan.main_class,
        redact_args(&plan.game_args).join(" "),
    );

    let mut cmd = Command::new(&plan.jvm_path);
    cmd.args(&plan.jvm_args)
        .arg(&plan.main_class)
        .args(&plan.game_args)
        .current_dir(&plan.working_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null());
    scrub_inherited_secrets(&mut cmd);
    // Don't pop a console window for the game JVM. stdout/stderr are piped
    // above, so we still stream every log line to the launching screen —
    // we just don't flash a black `java.exe` console alongside the game.
    super::no_window(&mut cmd);

    let program = plan.jvm_path.display().to_string();
    run_child(cmd, tx, &program);
}

/// Shared child supervision: spawn, stream both pipes, report exit.
fn run_child(mut cmd: Command, tx: Sender<LaunchEvent>, program: &str) {
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            let _ = tx.send(LaunchEvent::SpawnFailed(format!(
                "could not spawn {program}: {e}"
            )));
            return;
        }
    };

    // Read the creation time while we still hold the child, so the PID
    // can't have been recycled yet.
    let pid = child.id();
    let created = super::reaper::process_creation_time(pid);
    let _ = tx.send(LaunchEvent::Started { pid, created });

    // Reader threads — one per pipe.
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    let tx_out = tx.clone();
    let tx_err = tx.clone();

    let h_out = thread::Builder::new()
        .name("ewo-launch-stdout".into())
        .spawn(move || drain_pipe(stdout, Severity::Info, tx_out))
        .expect("spawn stdout reader");
    let h_err = thread::Builder::new()
        .name("ewo-launch-stderr".into())
        .spawn(move || drain_pipe(stderr, Severity::Warn, tx_err))
        .expect("spawn stderr reader");

    // Wait for JVM exit, then join readers.
    let exit_code = match child.wait() {
        Ok(status) => status.code(),
        Err(e) => {
            let _ = tx.send(LaunchEvent::SpawnFailed(format!("wait: {}", e)));
            return;
        }
    };
    let _ = h_out.join();
    let _ = h_err.join();
    let _ = tx.send(LaunchEvent::Exited(exit_code));
}

fn drain_pipe<R: std::io::Read + Send + 'static>(
    pipe: R,
    severity: Severity,
    tx: Sender<LaunchEvent>,
) {
    let reader = BufReader::new(pipe);
    for line in reader.lines() {
        match line {
            Ok(text) => {
                if tx
                    .send(LaunchEvent::Line { severity, text })
                    .is_err()
                {
                    break;
                }
            }
            Err(_) => break,
        }
    }
}

/// True for an environment variable name that looks like a credential.
fn is_secret_env(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    upper.contains("TOKEN") || upper.contains("SECRET")
}

/// Stop the child inheriting credentials from the launcher's environment
/// (e.g. `EWO_LOADER_TOKEN`, the GitHub PAT used for private loader
/// downloads). Anything the child genuinely needs is set explicitly after.
fn scrub_inherited_secrets(cmd: &mut Command) {
    for (key, _) in std::env::vars_os() {
        if key.to_str().is_some_and(is_secret_env) {
            cmd.env_remove(&key);
        }
    }
}

const REDACTED: &str = "<redacted>";

/// A flag whose *following* argument is a credential.
fn is_secret_flag(arg: &str) -> bool {
    let lower = arg.trim_start_matches('-').to_ascii_lowercase();
    arg.starts_with('-')
        && (lower.contains("token") || lower.contains("session") || lower.contains("password"))
}

/// Copy of `args` safe to log: the value after `--accessToken` (and any
/// other token/session/password flag) is replaced, as is the value of any
/// `key=value` / `-Dkey=value` arg whose key names a token, and anything
/// shaped like a JWT.
pub fn redact_args(args: &[String]) -> Vec<String> {
    let mut out = Vec::with_capacity(args.len());
    let mut redact_next = false;
    for arg in args {
        if redact_next {
            out.push(REDACTED.to_string());
            redact_next = false;
            continue;
        }
        if let Some((key, _)) = arg.split_once('=') {
            if is_secret_flag(key) || (key.starts_with("-D") && is_secret_env(key)) {
                out.push(format!("{key}={REDACTED}"));
                continue;
            }
        } else if is_secret_flag(arg) {
            redact_next = true;
            out.push(arg.clone());
            continue;
        }
        if looks_like_jwt(arg) {
            out.push(REDACTED.to_string());
        } else {
            out.push(arg.clone());
        }
    }
    out
}

fn looks_like_jwt(s: &str) -> bool {
    s.starts_with("eyJ")
        && s.split('.').count() == 3
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'='))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(args: &[&str]) -> Vec<String> {
        args.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn access_token_value_is_redacted() {
        let args = v(&[
            "--username", "Vwyla", "--accessToken", "secret-mc-token", "--version", "26.2",
        ]);
        let r = redact_args(&args);
        assert_eq!(r[3], REDACTED);
        assert_eq!(r[1], "Vwyla");
        assert_eq!(r[5], "26.2");
        assert!(!r.join(" ").contains("secret-mc-token"));
    }

    #[test]
    fn inline_and_jwt_tokens_are_redacted() {
        let jwt = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJ4In0.c2lnbmF0dXJl";
        let args = v(&[
            "--session=abc",
            "-Dsome.api.token=xyz",
            "-Dfabric.debug.disableModIds=iris",
            jwt,
            "--quickPlayMultiplayer",
            "play.example.net",
        ]);
        let r = redact_args(&args);
        assert_eq!(r[0], "--session=<redacted>");
        assert_eq!(r[1], "-Dsome.api.token=<redacted>");
        assert_eq!(r[2], "-Dfabric.debug.disableModIds=iris");
        assert_eq!(r[3], REDACTED);
        assert_eq!(r[5], "play.example.net");
    }

    #[test]
    fn secret_env_names() {
        assert!(is_secret_env("EWO_LOADER_TOKEN"));
        assert!(is_secret_env("github_token"));
        assert!(is_secret_env("AWS_SECRET_ACCESS_KEY"));
        assert!(!is_secret_env("PATH"));
        assert!(!is_secret_env("REWO_VERSION"));
    }
}
