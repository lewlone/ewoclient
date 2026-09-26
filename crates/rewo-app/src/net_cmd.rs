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
    let disconnect = tick_error.or_else(|| session.disconnect.clone());

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
    if let Some(reason) = &disconnect {
        println!("[rewo-m1] disconnect: {reason}");
    }
    if query.len() == 3 {
        let (x, y, z) = (query[0], query[1], query[2]);
        let state = world.block_state_at(x, y, z);
        let name = data.blocks.block_name(state).unwrap_or("<unknown>");
        println!("[rewo-m1] block at ({x},{y},{z}) = state {state} ({name})");
    }
    if let (Some(path), Some(n)) = (&record_path, recorded) {
        println!("[rewo-m1] recording written to {} ({n} packets)", path.display());
    }

    // The soak is only meaningful if we actually got into the world.
    if !reached_play {
        return Err("never reached the Play phase".into());
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
    }
    Ok(())
}
