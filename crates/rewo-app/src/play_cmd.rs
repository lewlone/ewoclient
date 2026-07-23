//! `rewo play` — the M3 headless bot harness.
//!
//! Connects, runs a scripted survival session on a fixed 20 Hz clock
//! (spawn → walk → sprint → jump → look → dig → place → chat), and reports
//! the physics-parity meter: **server position corrections**. A world that
//! matches vanilla's simulation gets few/no corrections; drift shows up as
//! rubber-banding the server sends back. This is the DoD verification —
//! no human, no window.

use std::time::{Duration, Instant};

use clap::Args as ClapArgs;
use rewo_data::{assets, DataPaths, GameData};
use rewo_net::play::PlaySession;
use rewo_net::Connection;
use rewo_world::physics::TickInput;

const TICK: Duration = Duration::from_millis(50); // 20 Hz

#[derive(ClapArgs)]
pub struct PlayArgs {
    #[arg(long, default_value = "127.0.0.1")]
    host: String,
    #[arg(long, default_value_t = 25599)]
    port: u16,
    /// Player name. Defaults to the launcher handoff's profile name when
    /// an account is present (online-mode servers verify the name against
    /// the session join), else "RewoBot".
    #[arg(long)]
    username: Option<String>,
    #[arg(long, default_value = "26.2")]
    version: String,
    /// Total session length in seconds.
    #[arg(long, default_value_t = 40.0)]
    seconds: f32,
    /// Server command (without the leading `/`) to run once, right after
    /// spawn — world setup for a parity run, e.g. paving the floor with slabs
    /// to exercise partial collision shapes against the server's own physics.
    /// Multiple commands may be separated by `;` — enough to build a test
    /// structure (walls + roof + a torch) in one run.
    #[arg(long)]
    setup: Option<String>,
    /// Suppress the movement script. The light gate centres on the bot's
    /// final position, so it must stay inside whatever `--setup` built.
    #[arg(long, default_value_t = false)]
    still: bool,
    /// Report light at a fixed world coordinate ("x,y,z") in the summary,
    /// instead of only at the bot. Removes the bot's wandering from the
    /// measurement — the deterministic way to check the light decode, and to
    /// see whether a runtime change (a torch placed by --setup) reaches us.
    #[arg(long)]
    light_at: Option<String>,
    /// Recompute the loaded chunks' light from scratch and diff against the
    /// server's authoritative values — the lighting equivalent of the
    /// `CORRECTIONS` physics meter. Any mismatch is a bug in our light
    /// tables or flood fill, measured against vanilla's own engine.
    #[arg(long, default_value_t = false)]
    light_check: bool,
    /// Send a chat line partway through.
    #[arg(long, default_value = "rewo bot online")]
    chat: String,
    /// Skip the block dig/place actions (movement-only run).
    #[arg(long, default_value_t = false)]
    no_build: bool,
}

pub fn run(args: PlayArgs) -> Result<(), String> {
    let data = GameData::load_for_version(&args.version)?;

    // Build the state→solid table from the asset bake: any block that
    // resolved to a full cube collides. Non-baked (partial/plants) fall
    // through to "non-air is solid" in the session, which is right for the
    // flat-world test.
    let baked = match client_jar_path(&args.version) {
        Some(jar) => {
            let paths = DataPaths::for_version(&args.version).ok_or("no config dir")?;
            match assets::bake(&jar, &paths.blocks_json()) {
                Ok(b) => Some(b),
                Err(e) => {
                    log::warn!("play: asset bake failed ({e}); using non-air-is-solid");
                    None
                }
            }
        }
        None => None,
    };
    let collide = baked.as_ref().map(|b| b.collide.clone()).unwrap_or_default();

    let dirt_item = data.items.id("dirt");
    // Launcher account handoff (REWO_ACCESS_TOKEN/UUID/USERNAME) — lets the
    // bot harness join online-mode servers; offline servers ignore it. An
    // online-mode server verifies the hello name against the session join,
    // so the profile name wins unless --username overrides it.
    let auth = rewo_net::crypt::OnlineAuth::from_env();
    let username = args
        .username
        .clone()
        .or_else(|| auth.as_ref().map(|a| a.username.clone()))
        .unwrap_or_else(|| "RewoBot".into());
    let conn = Connection::connect(&args.host, args.port, &data)?;
    let mut session = conn.into_play(
        &args.host,
        args.port,
        &username,
        auth.as_ref(),
        collide,
        data.blocks.global_palette_bits,
    )?;
    // Entity collision: per-type footprint + whether it shoves (living only).
    session.entity_push = crate::live_cmd::entity_push_table(&data.entity_types);
    // Client-side relighting of our own edits — the server only sends light
    // on chunk load, never for a placed torch or a broken roof.
    if let Some(b) = baked.as_ref() {
        session.set_light_tables(
            b.emission.clone(),
            b.dampening.clone(),
            b.face_occludes.clone(),
        );
    }
    log::info!("play: entered live session, waiting for spawn…");

    let start = Instant::now();
    let total_ticks = (args.seconds / 0.05) as u64;
    let mut acted = Actions::default();
    // Tick index at which the server first spawned us — the action clock is
    // relative to spawn, not connect (chunk streaming can take a second).
    let mut spawn_tick: Option<u64> = None;

    for tick_n in 0..total_ticks {
        let deadline = start + TICK * (tick_n as u32 + 1);
        if session.spawned && spawn_tick.is_none() {
            spawn_tick = Some(tick_n);
            log::info!("play: spawned at tick {tick_n}, running the action script");
        }

        // Movement input + one-shot actions, on a spawn-relative clock.
        let input = if let Some(st) = spawn_tick {
            let secs = (tick_n - st) as f32 * 0.05;
            drive(&mut session, &args, secs, dirt_item, &mut acted)?
        } else {
            TickInput::default()
        };

        session.tick(&input)?;
        if let Some(reason) = &session.disconnect {
            return Err(format!("disconnected: {reason}"));
        }
        // Real-time pacing so the server sees a genuine 20 Hz client.
        let now = Instant::now();
        if now < deadline {
            std::thread::sleep(deadline - now);
        }
    }

    report(&mut session, &acted, &args, baked.as_ref(), &data);
    if !session.spawned {
        return Err("never spawned (no initial position from server)".into());
    }
    Ok(())
}

#[derive(Default)]
struct Actions {
    /// How many `--setup` commands have gone out (they are paced).
    setup_sent: usize,
    walked: bool,
    sprinted: bool,
    jumped: bool,
    looked: bool,
    dug: bool,
    placed: bool,
    chatted: bool,
    gave_block: bool,
    /// Anchor feet block captured when the build phase starts (bot is idle
    /// by then, so place/dig targets stay fixed).
    anchor: Option<(i32, i32, i32)>,
    /// Where dirt was placed / grass was dug — queried in the report to
    /// prove the actions mutated the server world end-to-end.
    placed_at: Option<(i32, i32, i32)>,
    dug_at: Option<(i32, i32, i32)>,
}

/// Movement input for this spawn-relative second, firing one-shot gameplay
/// actions on their scheduled tick. Timeline:
///   0-2s settle · 2-6s walk · 6-9s sprint · 9-12s jump · 12s look ·
///   14s give+place · 18s dig · 22s chat · rest settle.
fn drive(
    session: &mut PlaySession,
    args: &PlayArgs,
    secs: f32,
    dirt_item: Option<i32>,
    acted: &mut Actions,
) -> Result<TickInput, String> {
    let mut input = TickInput::default();
    if args.still {
        // fall through to the one-shot actions with no movement
    } else if (2.0..6.0).contains(&secs) {
        input.forward = 1.0;
        acted.walked = true;
    } else if (6.0..9.0).contains(&secs) {
        input.forward = 1.0;
        input.sprint = true;
        acted.sprinted = true;
    } else if (9.0..12.0).contains(&secs) {
        input.forward = 1.0;
        input.jump = true;
        acted.jumped = true;
    }

    // One-shot actions fire on the first tick of their scheduled second.
    if secs >= 12.0 && !acted.looked {
        session.player.yaw = 90.0;
        session.player.pitch = 20.0;
        acted.looked = true;
    }
    if !args.no_build {
        if secs >= 14.0 && !acted.gave_block {
            // Anchor the build targets to the (now-stationary) feet block.
            acted.anchor = Some(feet_block(session));
            // Give + hold a stack of dirt (creative server), then select it.
            if let Some(dirt) = dirt_item {
                let _ = session.creative_set_hotbar(0, dirt, 64);
                let _ = session.select_hotbar(0);
            }
            acted.gave_block = true;
        }
        if let Some((fx, fy, fz)) = acted.anchor {
            if secs >= 15.0 && !acted.placed {
                // Place against the TOP face of the grass block one to the
                // east (fx+1, fy-1) → dirt lands at (fx+1, fy) which is air,
                // beside the bot (not inside it, so the server accepts it).
                let target = (fx + 1, fy, fz);
                match session.use_item_on(fx + 1, fy - 1, fz, 1) {
                    Ok(()) => log::info!("play: place → {target:?} (on top of {:?})", (fx + 1, fy - 1, fz)),
                    Err(e) => log::warn!("play: place failed: {e}"),
                }
                acted.placed_at = Some(target);
                acted.placed = true;
            }
            if secs >= 18.0 && !acted.dug {
                // Break the grass block one to the WEST (a different block,
                // so removal is observable). Creative breaks on dig start.
                let target = (fx - 1, fy - 1, fz);
                match session.start_dig(fx - 1, fy - 1, fz, 1) {
                    Ok(()) => log::info!("play: dig → {target:?}"),
                    Err(e) => log::warn!("play: dig failed: {e}"),
                }
                acted.dug_at = Some(target);
                acted.dug = true;
            }
        }
    }
    // Setup commands go out one per 250 ms. Firing them all in one tick trips
    // the server's chat rate limit and the tail is silently dropped — which
    // looks exactly like a light bug, because the structure never appears.
    if let Some(cmd) = args.setup.as_deref() {
        let cmds: Vec<&str> = cmd.split(';').map(str::trim).filter(|c| !c.is_empty()).collect();
        let due = ((secs - 1.0) / 0.25).floor();
        if secs >= 1.0 && due >= 0.0 && (due as usize) < cmds.len() && acted.setup_sent <= due as usize {
            let one = cmds[due as usize];
            match session.send_command(one) {
                Ok(()) => log::info!("play: setup → /{one}"),
                Err(e) => log::warn!("play: setup failed: {e}"),
            }
            acted.setup_sent = due as usize + 1;
        }
    }
    if secs >= 22.0 && !acted.chatted {
        let _ = session.send_chat(&args.chat);
        acted.chatted = true;
    }

    // After the scripted phase, wander continuously so long runs keep
    // stressing physics + collision parity (the "survival session" proxy).
    // Walk forward, curving the yaw slowly, with a periodic sprint-jump.
    if secs >= 24.0 {
        let t = secs - 24.0;
        input.forward = 1.0;
        input.sprint = (t as u32 / 3) % 2 == 0;
        input.jump = (t * 20.0) as u32 % 40 == 0;
        session.player.yaw += 1.3; // slow left curve
    }
    Ok(input)
}

/// Integer block coords of the bot's feet.
fn feet_block(session: &PlaySession) -> (i32, i32, i32) {
    (
        session.player.x.floor() as i32,
        session.player.y.floor() as i32,
        session.player.z.floor() as i32,
    )
}


fn report(
    session: &mut PlaySession,
    acted: &Actions,
    args: &PlayArgs,
    baked: Option<&assets::BakedAssets>,
    data: &GameData,
) {
    let (px, py, pz) = (session.player.x, session.player.y, session.player.z);
    let ground = session.world.block_state_at(px.floor() as i32, py as i32 - 1, pz.floor() as i32);
    println!("[rewo-m3] play session summary");
    println!("[rewo-m3] spawned: {}  ticks: {}", session.spawned, session.ticks);
    println!(
        "[rewo-m3] final pos: ({:.2}, {:.2}, {:.2})  on_ground: {}",
        px, py, pz, session.player.on_ground
    );
    println!("[rewo-m3] block below feet: state {ground}");
    println!(
        "[rewo-m3] teleports: {}  CORRECTIONS: {}  (physics-parity meter — lower is better)",
        session.teleports, session.corrections
    );
    println!("[rewo-m3] block_updates received: {}", session.block_updates);
    println!(
        "[rewo-m3] actions — walk:{} sprint:{} jump:{} look:{} dig:{} place:{} chat:{} give:{}",
        acted.walked,
        acted.sprinted,
        acted.jumped,
        acted.looked,
        acted.dug,
        acted.placed,
        acted.chatted,
        acted.gave_block,
    );
    // Prove build/dig mutated the server world: query the echoed states.
    // Light readout at the bot's final position — the headless check for the
    // light decode. In a sealed, torch-lit room sky must be 0 and block must
    // fall off by 1 per block from the torch; under open sky it's 15/0.
    {
        let p = &session.player;
        let (bx, by, bz) = (p.x.floor() as i32, p.eye_y().floor() as i32, p.z.floor() as i32);
        let (bl, sl) = session.world.light_at(bx, by, bz);
        println!("[rewo-m3] LIGHT @ ({bx},{by},{bz}) = {} (sky {sl}, block {bl})", bl.max(sl));
        // A short horizontal profile makes block-light falloff visible.
        let profile: Vec<String> = (0..6)
            .map(|d| {
                let (b, sk) = session.world.light_at(bx + d, by, bz);
                format!("{}:{}/{}", d, b, sk)
            })
            .collect();
        println!("[rewo-m3] LIGHT profile +x (block/sky): {}", profile.join("  "));
        if let Some(spec) = args.light_at.as_deref() {
            let n: Vec<i32> = spec.split(',').filter_map(|v| v.trim().parse().ok()).collect();
            if let [x, y, z] = n[..] {
                let (b, sk) = session.world.light_at(x, y, z);
                let st = session.world.block_state_at(x, y, z);
                println!(
                    "[rewo-m3] LIGHT at ({x},{y},{z}) = {} (sky {sk}, block {b})  state {st}",
                    b.max(sk)
                );
            } else {
                println!("[rewo-m3] LIGHT at: bad --light-at {spec:?} (want \"x,y,z\")");
            }
        }
    }
    if args.light_check {
        match baked {
            Some(b) => light_parity_check(session, b, data),
            None => println!("[rewo-light] LIGHT-CHECK: no asset bake — skipped"),
        }
    }
    if let Some((x, y, z)) = acted.placed_at {
        let s = session.world.block_state_at(x, y, z);
        println!(
            "[rewo-m3] PLACE verify @ ({x},{y},{z}) = state {s} {}",
            if s != 0 { "(non-air ✓ block appeared)" } else { "(still air ✗)" }
        );
    }
    if let Some((x, y, z)) = acted.dug_at {
        let s = session.world.block_state_at(x, y, z);
        println!(
            "[rewo-m3] DIG verify @ ({x},{y},{z}) = state {s} {}",
            if s == 0 { "(air ✓ block removed)" } else { "(still solid ✗)" }
        );
    }
    println!("[rewo-m3] chat received: {} lines", session.chat_log.len());
    for line in session.chat_log.iter().take(6) {
        println!("[rewo-m3]   > {line}");
    }
    println!("[rewo-m3] loaded columns: {}", session.world.loaded_columns());
}

fn client_jar_path(version: &str) -> Option<std::path::PathBuf> {
    let mut p = dirs::config_dir()?;
    p.push("EwoClient");
    p.push("shared/versions");
    p.push(version);
    p.push(format!("{version}.jar"));
    p.exists().then_some(p)
}

/// Recompute a loaded column's light from scratch and diff it against the
/// server's authoritative values.
///
/// The server runs vanilla's own light engine, so its `chunk_data` /
/// `light_update` payloads are ground truth. Zeroing a column and refilling it
/// from our tables exercises emission, dampening, sky sources and both flood
/// passes at once — a mismatch is a real bug, not a rendering opinion. This is
/// the "verify the property, not a proxy" gate for lighting.
///
/// Columns are relit twice (once to settle the 3×3 neighbourhood, once for the
/// centre) so light entering across a chunk border is accounted for; edge
/// columns of the loaded region are skipped because their neighbours are not
/// present to feed them.
fn light_parity_check(
    session: &mut rewo_net::play::PlaySession,
    baked: &assets::BakedAssets,
    data: &GameData,
) {
    use rewo_world::light::{LightEngine, LightTables};

    let tables = LightTables {
        emission: &baked.emission,
        dampening: &baked.dampening,
        face_occludes: &baked.face_occludes,
    };
    let shape = session.world.shape;
    let (y0, y1) = (shape.min_y, shape.min_y + shape.height);

    // Centre on the bot, and only check columns whose whole 3×3 is loaded.
    let (px, pz) = (
        session.player.x.floor() as i32 >> 4,
        session.player.z.floor() as i32 >> 4,
    );
    let mut targets = Vec::new();
    for dx in -1..=1i32 {
        for dz in -1..=1i32 {
            let (cx, cz) = (px + dx, pz + dz);
            if (-1..=1).all(|ax: i32| {
                (-1..=1).all(|az: i32| session.world.is_loaded((cx + ax) * 16, (cz + az) * 16))
            }) {
                targets.push((cx, cz));
            }
        }
    }
    if targets.is_empty() {
        println!("[rewo-light] LIGHT-CHECK: no fully-surrounded column loaded — skipped");
        return;
    }

    // Snapshot the server's values before we overwrite anything.
    let mut truth = Vec::new();
    for &(cx, cz) in &targets {
        let mut col = Vec::with_capacity(16 * 16 * (y1 - y0) as usize);
        for lx in 0..16 {
            for lz in 0..16 {
                for y in y0..y1 {
                    col.push(session.world.light_at(cx * 16 + lx, y, cz * 16 + lz));
                }
            }
        }
        truth.push(col);
    }

    let mut engine = LightEngine::new();
    for &(cx, cz) in &targets {
        engine.relight_column(&mut session.world, tables, cx, cz);
    }
    for &(cx, cz) in &targets {
        engine.relight_column(&mut session.world, tables, cx, cz);
    }

    // Diff. Report per-channel, and keep a few concrete examples — a bare
    // count would not say whether the gap is a source, a cost, or a border.
    let (mut cells, mut bad_block, mut bad_sky) = (0usize, 0usize, 0usize);
    let mut examples: Vec<String> = Vec::new();
    // Direction and size of the error say what kind of bug it is: uniformly
    // brighter means a source is one too high, darker means a path is blocked
    // that should not be.
    let (mut brighter, mut darker, mut max_delta) = (0usize, 0usize, 0i32);
    // The brightest disagreeing cell is nearest whatever source is wrong.
    let mut worst: Option<(u8, String)> = None;
    for (ci, &(cx, cz)) in targets.iter().enumerate() {
        let mut i = 0;
        for lx in 0..16 {
            for lz in 0..16 {
                for y in y0..y1 {
                    let (want_b, want_s) = truth[ci][i];
                    i += 1;
                    cells += 1;
                    let (x, z) = (cx * 16 + lx, cz * 16 + lz);
                    let (got_b, got_s) = session.world.light_at(x, y, z);
                    if got_b != want_b {
                        bad_block += 1;
                    }
                    if got_s != want_s {
                        bad_sky += 1;
                    }
                    if got_s != want_s {
                        let d = got_s as i32 - want_s as i32;
                        if d > 0 { brighter += 1 } else { darker += 1 }
                        max_delta = max_delta.max(d.abs());
                        if worst.as_ref().map_or(true, |(lv, _)| want_s.max(got_s) > *lv) {
                            let n = data.blocks.block_name(
                                session.world.block_state_at(x, y, z)).unwrap_or("?");
                            worst = Some((
                                want_s.max(got_s),
                                format!("({x},{y},{z}) {n}: want s{want_s} got s{got_s}"),
                            ));
                        }
                    }
                    if (got_b != want_b || got_s != want_s) && examples.len() < 8 {
                        let st = session.world.block_state_at(x, y, z);
                        let name = data.blocks.block_name(st).unwrap_or("?");
                        let (e, d) = (
                            baked.emission.get(st as usize).copied().unwrap_or(0),
                            baked.dampening.get(st as usize).copied().unwrap_or(0),
                        );
                        examples.push(format!(
                            "({x},{y},{z}) {name} [emit {e} damp {d}]: want b{want_b}/s{want_s} got b{got_b}/s{got_s}"
                        ));
                    }
                }
            }
        }
    }

    let pct = |n: usize| 100.0 * n as f64 / cells.max(1) as f64;
    println!(
        "[rewo-light] LIGHT-CHECK {} columns, {cells} cells: block {bad_block} ({:.3}%), sky {bad_sky} ({:.3}%) {}",
        targets.len(),
        pct(bad_block),
        pct(bad_sky),
        if bad_block == 0 && bad_sky == 0 { "✓ EXACT" } else { "✗" }
    );
    if bad_sky > 0 {
        println!(
            "[rewo-light]   sky: {brighter} too bright, {darker} too dark, max delta {max_delta}"
        );
        if let Some((_, w)) = &worst {
            println!("[rewo-light]   brightest disagreement: {w}");
        }
    }
    for e in &examples {
        println!("[rewo-light]   {e}");
    }
}
