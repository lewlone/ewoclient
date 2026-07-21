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
    /// child PID so the launcher can reap it if the JVM later zombies on
    /// exit (see `launch::reaper`).
    Started { pid: u32 },
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

/// Spawn the child on a worker thread + start the readers. Returns
/// immediately. Caller should poll the receiver each frame.
pub fn spawn(plan: LaunchPlan, tx: Sender<LaunchEvent>) -> thread::JoinHandle<()> {
    thread::Builder::new()
        .name("ewo-launch-supervisor".into())
        .spawn(move || run_launch(plan, tx))
        .expect("spawn launch supervisor thread")
}

fn run_launch(plan: LaunchPlan, tx: Sender<LaunchEvent>) {
    log::info!(
        "launch: {} {} {} {}",
        plan.jvm_path.display(),
        plan.jvm_args.join(" "),
        plan.main_class,
        plan.game_args.join(" "),
    );

    let mut cmd = Command::new(&plan.jvm_path);
    cmd.args(&plan.jvm_args)
        .arg(&plan.main_class)
        .args(&plan.game_args)
        .current_dir(&plan.working_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null());
    // Don't pop a console window for the game JVM. stdout/stderr are piped
    // above, so we still stream every log line to the launching screen —
    // we just don't flash a black `java.exe` console alongside the game.
    super::no_window(&mut cmd);

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            let _ = tx.send(LaunchEvent::SpawnFailed(format!(
                "could not spawn {}: {}",
                plan.jvm_path.display(),
                e
            )));
            return;
        }
    };

    let _ = tx.send(LaunchEvent::Started { pid: child.id() });

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
