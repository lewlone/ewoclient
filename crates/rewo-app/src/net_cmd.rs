//! `rewo net` — the M1 protocol driver (headless verification).
//!
//! Modes:
//!   rewo net soak   --host 127.0.0.1 --port 25599 --seconds 600 --record x.rewo
//!   rewo net replay --file x.rewo [--expect-digest N]
//!   rewo net probe  --host … --port …    (query.json for one block)
//!
//! `soak` is the M1 DoD: connect, stay connected N seconds answering the
//! liveness contract, decode the world, print stats + digest, optionally
//! record. `replay` re-decodes a recording and compares digests — the
//! equivalence proof.

use std::path::PathBuf;
use std::time::Duration;

use clap::Args as ClapArgs;
use rewo_data::GameData;
use rewo_net::{ids::Ids, record, Connection};
use rewo_ui::disconnect_screen::DisconnectCause;

#[derive(ClapArgs)]
pub struct NetArgs {
    #[command(subcommand)]
    mode: Mode,

    /// MC version whose data tables to load (must be datagen'd on disk).
    #[arg(long, default_value = "26.2", global = true)]
    version: String,
}

#[derive(clap::Subcommand)]
enum Mode {
    /// Connect and stay connected, decoding the world.
    Soak {
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
        #[arg(long, default_value_t = 25599)]
        port: u16,
        #[arg(long, default_value = "Rewo")]
        username: String,
        #[arg(long, default_value_t = 30.0)]
        seconds: f32,
        /// Record inbound packets to this file for replay.
        #[arg(long)]
        record: Option<PathBuf>,
        /// After the soak, query the block state at these world coords.
        #[arg(long, num_args = 3, value_names = ["X", "Y", "Z"], allow_hyphen_values = true)]
        query: Vec<i32>,
    },
    /// Re-decode a recording, print + optionally check the world digest.
    Replay {
        #[arg(long)]
        file: PathBuf,
        #[arg(long)]
        expect_digest: Option<u64>,
    },
}

pub fn run(args: NetArgs) -> Result<(), String> {
    let data = GameData::load_for_version(&args.version).map_err(|e| {
        format!(
            "load game data for {}: {e}\n  (run the data generator first — see REWO_PLAN.md §5)",
            args.version
        )
    })?;
    log::info!(
        "rewo-net: loaded {} block states, protocol {}",
        data.blocks.state_count(),
        rewo_net::PROTOCOL_VERSION
    );

    match args.mode {
        Mode::Soak {
            host,
            port,
            username,
            seconds,
            record: record_path,
            query,
        } => soak(&data, &host, port, &username, seconds, record_path, query),
        Mode::Replay {
            file,
            expect_digest,
        } => replay(&data, &file, expect_digest),
    }
}

fn soak(
    data: &GameData,
    host: &str,
    port: u16,
    username: &str,
    seconds: f32,
    record_path: Option<PathBuf>,
    query: Vec<i32>,
) -> Result<(), String> {
    let mut conn = Connection::connect(host, port, data)?;
    if let Some(path) = &record_path {
        conn.recorder =
            Some(record::Recorder::create(path).map_err(|e| format!("create recording: {e}"))?);
    }
    // The same PlaySession `rewo live` runs: an idle player ticked at 20 Hz,
    // so the soak exercises the live packet handling, not a second copy of it.
    let mut session = conn.into_play(
        host,
        port,
        username,
        None,
        Vec::new(),
        data.blocks.global_palette_bits,
        rewo_world::biome::Colormaps::neutral(),
    )?;
    let tick_error = session.run_idle(Duration::from_secs_f32(seconds)).err();
    let recorded = session.finish_recording()?;
    let world = &session.world;
    let reached_play = session.spawned;
    // Read before the session drops: the soak's own shutdown is that drop
    // (the socket closes after the verdict), so it can never set this. Any
    // disconnect here happened during the run.
    let ended = SoakEnd::classify(
        tick_error,
        session.disconnect.clone(),
        session.disconnect_cause,
    );

    println!("[rewo-m1] soak against {host}:{port} for {seconds:.0}s");
    println!("[rewo-m1] reached play: {reached_play}");
    println!(
        "[rewo-m1] packets in: {}  bytes in: {}  keepalives: {}  teleports: {}  decode failures: {}",
        session.packets_in, session.bytes_in, session.keepalives, session.teleports,
        session.decode_failures
    );
    println!(
        "[rewo-m1] chunks decoded: {}  columns resident: {}",
        session.chunk_packets,
        world.loaded_columns()
    );
    println!("[rewo-m1] world digest: {:#018x}", world.digest());
    println!("[rewo-m1] ended: {ended}");
    if query.len() == 3 {
        let (x, y, z) = (query[0], query[1], query[2]);
        let state = world.block_state_at(x, y, z);
        let name = data.blocks.block_name(state).unwrap_or("<unknown>");
        println!("[rewo-m1] block at ({x},{y},{z}) = state {state} ({name})");
    }
    if let (Some(path), Some(n)) = (&record_path, recorded) {
        println!(
            "[rewo-m1] recording written to {} ({n} packets)",
            path.display()
        );
    }

    soak_verdict(reached_play, &ended, session.decode_failures)
}

/// How a soak's session ended.
#[derive(Debug, PartialEq)]
enum SoakEnd {
    /// The full duration ran with the connection up; the soak then closes
    /// it itself.
    Completed,
    /// The server ended the session during the run: a `disconnect` packet
    /// (a kick), or the socket closing with no packet.
    ServerDisconnected { reason: String, cause: String },
    /// The client gave up during the run: a read error or timeout, a
    /// malformed frame or bundle, a failed write, or a handler error.
    ClientError(String),
}

impl SoakEnd {
    fn classify(
        tick_error: Option<String>,
        disconnect: Option<String>,
        cause: Option<DisconnectCause>,
    ) -> Self {
        if let Some(e) = tick_error {
            return SoakEnd::ClientError(e);
        }
        let Some(reason) = disconnect else {
            return SoakEnd::Completed;
        };
        match cause {
            Some(DisconnectCause::ServerRequested) => SoakEnd::ServerDisconnected {
                reason,
                cause: "kicked by the server".into(),
            },
            Some(DisconnectCause::EndOfStream) => SoakEnd::ServerDisconnected {
                reason,
                cause: "the server closed the connection".into(),
            },
            // `ClientError`, or a disconnect recorded without a cause: the
            // client's side of the connection failed.
            Some(DisconnectCause::ClientError) | None => SoakEnd::ClientError(reason),
        }
    }
}

impl std::fmt::Display for SoakEnd {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SoakEnd::Completed => write!(f, "completed the run, connection still up"),
            SoakEnd::ServerDisconnected { reason, cause } => {
                write!(f, "disconnected mid-run ({cause}): {reason}")
            }
            SoakEnd::ClientError(e) => write!(f, "connection failed mid-run: {e}"),
        }
    }
}

/// The soak's pass/fail rule.
fn soak_verdict(reached_play: bool, ended: &SoakEnd, decode_failures: u64) -> Result<(), String> {
    // The soak is only meaningful if we actually got into the world.
    if !reached_play {
        return Err(format!("never reached the Play phase ({ended})"));
    }
    // ...and stayed there for the whole run. A soak that was kicked after
    // ten seconds of a sixty-second run did not soak; it used to pass.
    if *ended != SoakEnd::Completed {
        return Err(format!("the soak did not run its full duration: {ended}"));
    }
    // ...and only if every packet decoded. `decode_failures` counts the
    // malformed or unhandled bodies the session dropped (and kept going), so
    // grading "reached Play" alone passed a soak that threw away packets.
    if decode_failures > 0 {
        return Err(format!(
            "{decode_failures} packet(s) failed to decode during the soak (see the log for the first)"
        ));
    }
    Ok(())
}

fn replay(data: &GameData, file: &std::path::Path, expect: Option<u64>) -> Result<(), String> {
    let ids = Ids::resolve(&data.packets)?;
    let (world, chunks) = record::replay(file, data, &ids)?;
    let digest = world.digest();
    let columns = world.loaded_columns();
    println!("[rewo-m1] replay of {}", file.display());
    println!("[rewo-m1] chunks decoded: {chunks}  columns: {columns}");
    println!("[rewo-m1] world digest: {digest:#018x}");
    if let Some(expected) = expect {
        if digest == expected {
            println!("[rewo-m1] digest MATCHES live session ✓");
        } else {
            return Err(format!(
                "digest mismatch: replay {digest:#018x} != expected {expected:#018x}"
            ));
        }
    } else {
        // Without a digest to compare against, a replay only shows the file
        // decodes; it says nothing about equivalence with the live session.
        // Say so rather than letting an exit code 0 read as a verdict.
        println!(
            "[rewo-m1] UNCHECKED: no --expect-digest given, so this replay asserts only that \
             the recording decodes, not that it matches the live session"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_run_that_reached_its_deadline_is_completed_and_passes() {
        let ended = SoakEnd::classify(None, None, None);
        assert_eq!(ended, SoakEnd::Completed);
        assert!(soak_verdict(true, &ended, 0).is_ok());
    }

    #[test]
    fn a_kick_mid_run_fails_with_its_reason() {
        let ended = SoakEnd::classify(
            None,
            Some("Kicked by an operator".into()),
            Some(DisconnectCause::ServerRequested),
        );
        assert!(matches!(ended, SoakEnd::ServerDisconnected { .. }));
        let err = soak_verdict(true, &ended, 0).unwrap_err();
        assert!(err.contains("Kicked by an operator"), "{err}");
        assert!(err.contains("kicked by the server"), "{err}");
    }

    #[test]
    fn the_server_closing_the_socket_mid_run_fails() {
        let ended = SoakEnd::classify(
            None,
            Some("connection closed".into()),
            Some(DisconnectCause::EndOfStream),
        );
        let err = soak_verdict(true, &ended, 0).unwrap_err();
        assert!(err.contains("the server closed the connection"), "{err}");
    }

    #[test]
    fn a_read_error_or_timeout_mid_run_fails() {
        let ended = SoakEnd::classify(
            None,
            Some("timed out".into()),
            Some(DisconnectCause::ClientError),
        );
        assert_eq!(ended, SoakEnd::ClientError("timed out".into()));
        let err = soak_verdict(true, &ended, 0).unwrap_err();
        assert!(err.contains("timed out"), "{err}");
        // A disconnect recorded without a cause is not a clean end either.
        let ended = SoakEnd::classify(None, Some("send: broken pipe".into()), None);
        assert!(soak_verdict(true, &ended, 0).is_err());
    }

    #[test]
    fn a_tick_error_fails_even_without_a_recorded_disconnect() {
        let ended = SoakEnd::classify(Some("play keep_alive: eof".into()), None, None);
        assert_eq!(ended, SoakEnd::ClientError("play keep_alive: eof".into()));
        assert!(soak_verdict(true, &ended, 0).is_err());
    }

    #[test]
    fn decode_failures_and_never_reaching_play_still_fail() {
        assert!(soak_verdict(true, &SoakEnd::Completed, 1).is_err());
        assert!(soak_verdict(false, &SoakEnd::Completed, 0).is_err());
    }
}
