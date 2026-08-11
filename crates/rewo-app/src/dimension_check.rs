//! `rewo play --dimension-check` — M16's **authoritative live** dimension gate.
//!
//! One `PlaySession`, no user, no window: the bot issues the server commands
//! itself, waits for each expected respawn to actually be observed, lets the new
//! world settle, and then validates the *property* at each of four checkpoints —
//! Overworld → Nether → End → Overworld.
//!
//! Design rules, each of which exists because its opposite has faked a result
//! here or elsewhere in this project:
//!
//! * **Nothing is queued ahead.** The next command is not sent until the prior
//!   transition's respawn has been seen *and* its world has settled, so a
//!   silently-rejected command (a non-op account, a disabled dimension) can only
//!   ever become a timeout — never a check that quietly did not run.
//! * **Every wait is bounded and reports what it was waiting for.** A hang is a
//!   red result with diagnostics, not an infinite loop.
//! * **Coordinates are never used as proof of a transition.** Both dimensions
//!   load column (0,0) at (0,·,0); the proof that the old world was discarded
//!   comes from [`rewo_net::play::DimensionTransition`]'s own witnesses, which
//!   are recorded inside the single production `WorldTransition` path — the only
//!   code that can see the world being thrown away.
//! * **Claims are limited to measurements.** The properties this gate cannot
//!   observe are not asserted and not claimed.
//!
//! Determinism: the movement script, the build actions, the scripted chat and
//! `--setup` are all off for the whole run. `--setup` is *rejected* (its paced
//! command stream would race this one for the server's chat rate limiter);
//! movement/build are forced off rather than rejected, because the gate's own
//! still/no-build path is the deterministic one and requiring the user to also
//! pass `--still --no-build` would be a trap.

use std::time::{Duration, Instant};

use rewo_net::play::{DimensionTransition, PlaySession};
use rewo_world::dimension::DimensionTypeDef;

use crate::dimensioncheck_cmd::{Expect, EXPECT};

/// Server chat/command rate limit spacing. The established interval `--setup`
/// paces at; commands here are separated by whole settle windows, so this is a
/// floor that should never bind — it is enforced anyway so that a future,
/// faster plan cannot silently start dropping commands.
const RATE_LIMIT: Duration = Duration::from_millis(250);

/// Ticks a world must hold a constant, nonzero column count before it counts as
/// settled (20 Hz → 2.0 s).
const SETTLE_STABLE_TICKS: u64 = 40;
/// Ticks of settled observation before a checkpoint is validated. The window the
/// corrections meter is read over.
const OBSERVE_TICKS: u64 = 60;
/// Bound on "the world reached a settled state" (20 Hz → 30 s). Generous: the
/// End is generated on first entry.
const SETTLE_TIMEOUT_TICKS: u64 = 600;
/// Bound on "the respawn we asked for was observed" (20 Hz → 30 s).
const RESPAWN_TIMEOUT_TICKS: u64 = 600;

/// A hop: the level to `execute in`, and where to land in it.
struct Hop {
    level: &'static str,
    x: i32,
    y: i32,
    z: i32,
}

/// Overworld → Nether → End → Overworld.
///
/// The Overworld return lands at y=-55: the test server is `level-type=flat`
/// with empty `generator-settings`, i.e. vanilla's default flat preset —
/// bedrock at -64, dirt at -63/-62, grass at -61 — so -55 is six blocks of air
/// above the surface and the player settles onto y=-60 well inside the settle
/// window. A `tp` into terrain would be the one way this step could produce a
/// server correction that was not our physics.
const HOPS: [Hop; 3] = [
    Hop {
        level: "minecraft:the_nether",
        x: 0,
        y: 80,
        z: 0,
    },
    Hop {
        level: "minecraft:the_end",
        x: 0,
        y: 100,
        z: 0,
    },
    Hop {
        level: "minecraft:overworld",
        x: 0,
        y: -55,
        z: 0,
    },
];

/// The level key expected at each of the four checkpoints.
const CHECKPOINTS: [&str; 4] = [
    "minecraft:overworld",
    "minecraft:the_nether",
    "minecraft:the_end",
    "minecraft:overworld",
];

/// The dimension **type** each of those level keys is served by on a vanilla
/// server. Kept apart from the key on purpose: they are different registries,
/// and conflating them is the bug this milestone's decoder was written against.
fn expected_type_for(level: &str) -> &'static str {
    match level {
        "minecraft:the_nether" => "minecraft:the_nether",
        "minecraft:the_end" => "minecraft:the_end",
        _ => "minecraft:overworld",
    }
}

fn expect_for(type_name: &str) -> Option<&'static Expect> {
    EXPECT.iter().find(|e| e.name == type_name)
}

// ------------------------------------------------------------------- results

/// What one checkpoint measured. Printed verbatim at the end — one line per
/// dimension.
pub struct Checkpoint {
    index: usize,
    level: String,
    type_name: String,
    holder: i32,
    min_y: i32,
    height: i32,
    has_sky_light: bool,
    skybox: &'static str,
    cardinal: &'static str,
    ambient_light: f32,
    has_day_timeline: bool,
    generation: u64,
    columns: usize,
    /// Loaded cells sampled for the sky-channel contract, and the maximum sky
    /// level any of them returned.
    sky_samples: usize,
    max_sky: u8,
    /// Sparse/unloaded reads sampled, and the maximum sky level they returned.
    sparse_samples: usize,
    max_sparse_sky: u8,
    corrections_in_window: u32,
    teleports_total: u32,
    decode_failures: u64,
}

impl Checkpoint {
    fn line(&self) -> String {
        format!(
            "[dimension-check] checkpoint {} {:<21} type {:<21} holder {} gen {} | \
             y {}..{} ({} sections) sky_light {:<3} skybox {:<9} cardinal {:<7} ambient {:.2} \
             day_timeline {:<3} | columns {:>3} sky {}/{} loaded max {} sparse max {} | \
             corrections in window {} teleports {} decode failures {}",
            self.index,
            self.level,
            self.type_name,
            self.holder,
            self.generation,
            self.min_y,
            self.min_y + self.height,
            self.height / 16,
            if self.has_sky_light { "yes" } else { "no" },
            self.skybox,
            self.cardinal,
            self.ambient_light,
            if self.has_day_timeline { "yes" } else { "no" },
            self.columns,
            self.sky_samples,
            self.sky_samples + self.sparse_samples,
            self.max_sky,
            self.max_sparse_sky,
            self.corrections_in_window,
            self.teleports_total,
            self.decode_failures,
        )
    }
}

// -------------------------------------------------------------- state machine

enum Phase {
    /// Waiting for the login world to have chunks and hold still.
    Settling {
        stable_ticks: u64,
        last_columns: usize,
        deadline: u64,
    },
    /// Settled; watching the corrections meter over a quiet window.
    Observing {
        until: u64,
        corrections_at: u32,
    },
    /// A command has gone out; waiting for the respawn it must cause.
    AwaitingRespawn {
        hop: usize,
        generation_before: u64,
        deadline: u64,
    },
    Done,
}

pub struct DimensionCheck {
    /// The name commands are addressed to — the *selected* username, whatever
    /// it is. Never hardcoded: sending `tp RewoOp` from a session logged in as
    /// something else would teleport nobody and read as a client bug.
    username: String,
    phase: Phase,
    /// Checkpoints completed so far; also the index into [`CHECKPOINTS`].
    checkpoint: usize,
    results: Vec<Checkpoint>,
    /// Wall-clock of the last command, for the rate-limit floor.
    last_command: Option<Instant>,
    /// Every (holder → dimension type name) the session has resolved, so the
    /// same raw id resolving to two different entries is caught.
    holder_names: Vec<(i32, String)>,
}

impl DimensionCheck {
    /// `username` must be the name the session actually logged in with.
    pub fn new(username: &str) -> Self {
        Self {
            username: username.to_string(),
            phase: Phase::Settling {
                stable_ticks: 0,
                last_columns: usize::MAX,
                deadline: SETTLE_TIMEOUT_TICKS,
            },
            checkpoint: 0,
            results: Vec::new(),
            last_command: None,
            holder_names: Vec::new(),
        }
    }

    pub fn finished(&self) -> bool {
        matches!(self.phase, Phase::Done)
    }

    /// One 20 Hz tick of the gate, run *after* `session.tick()`.
    ///
    /// `tick_n` is the session tick index. Returns `Err` on any failed property
    /// or any exceeded bound — this gate never downgrades a failure to a note.
    pub fn tick(&mut self, session: &mut PlaySession, tick_n: u64) -> Result<(), String> {
        match self.phase {
            Phase::Done => Ok(()),
            Phase::Settling {
                stable_ticks,
                last_columns,
                deadline,
            } => {
                if !session.spawned {
                    // Not a live participant yet (pre-login, or mid-respawn):
                    // the settle clock has not started.
                    if tick_n > deadline {
                        return Err(self.timeout(session, tick_n, "spawn"));
                    }
                    return Ok(());
                }
                let columns = session.world.loaded_columns();
                let stable = if columns > 0 && columns == last_columns {
                    stable_ticks + 1
                } else {
                    0
                };
                if stable >= SETTLE_STABLE_TICKS {
                    self.phase = Phase::Observing {
                        until: tick_n + OBSERVE_TICKS,
                        corrections_at: session.corrections,
                    };
                    return Ok(());
                }
                if tick_n > deadline {
                    return Err(self.timeout(session, tick_n, "chunk settle"));
                }
                self.phase = Phase::Settling {
                    stable_ticks: stable,
                    last_columns: columns,
                    deadline,
                };
                Ok(())
            }
            Phase::Observing {
                until,
                corrections_at,
            } => {
                if tick_n < until {
                    return Ok(());
                }
                let corrections = session.corrections.saturating_sub(corrections_at);
                self.validate(session, corrections)?;
                if self.checkpoint == CHECKPOINTS.len() {
                    self.phase = Phase::Done;
                    return Ok(());
                }
                let hop = self.checkpoint - 1;
                self.send_hop(session, hop, tick_n)
            }
            Phase::AwaitingRespawn {
                hop,
                generation_before,
                deadline,
            } => {
                if session.dimension_generation == generation_before {
                    if tick_n > deadline {
                        return Err(format!(
                            "dimension-check: no respawn observed {}s after `{}` — the \
                             server never changed our dimension. A command that is \
                             silently rejected (the account is not op, or the target \
                             dimension is disabled) looks exactly like this. Session: \
                             level {:?}, generation {}, spawned {}, columns {}, \
                             chat lines {}.",
                            (tick_n.saturating_sub(deadline - RESPAWN_TIMEOUT_TICKS)) / 20,
                            self.command(hop),
                            session.active_dimension_key,
                            session.dimension_generation,
                            session.spawned,
                            session.world.loaded_columns(),
                            session.chat_log.len(),
                        ));
                    }
                    return Ok(());
                }
                // Exactly one change per command, no more.
                if session.dimension_generation != generation_before + 1 {
                    return Err(format!(
                        "dimension-check: generation jumped {generation_before} → {} for \
                         one command — a dimension change was counted more than once",
                        session.dimension_generation
                    ));
                }
                self.phase = Phase::Settling {
                    stable_ticks: 0,
                    last_columns: usize::MAX,
                    deadline: tick_n + SETTLE_TIMEOUT_TICKS,
                };
                Ok(())
            }
        }
    }

    fn command(&self, hop: usize) -> String {
        let h = &HOPS[hop];
        format!(
            "execute in {} run tp {} {} {} {}",
            h.level, self.username, h.x, h.y, h.z
        )
    }

    fn send_hop(
        &mut self,
        session: &mut PlaySession,
        hop: usize,
        tick_n: u64,
    ) -> Result<(), String> {
        // The rate-limit floor. Nothing here should ever hit it (settle windows
        // are seconds long), but a dropped command is invisible, so the guard is
        // unconditional rather than trusted.
        if let Some(last) = self.last_command {
            let since = last.elapsed();
            if since < RATE_LIMIT {
                std::thread::sleep(RATE_LIMIT - since);
            }
        }
        let cmd = self.command(hop);
        session
            .send_command(&cmd)
            .map_err(|e| format!("dimension-check: sending `/{cmd}` failed: {e}"))?;
        log::info!("dimension-check: → /{cmd}");
        self.last_command = Some(Instant::now());
        self.phase = Phase::AwaitingRespawn {
            hop,
            generation_before: session.dimension_generation,
            deadline: tick_n + RESPAWN_TIMEOUT_TICKS,
        };
        Ok(())
    }

    fn timeout(&self, session: &PlaySession, tick_n: u64, what: &str) -> String {
        format!(
            "dimension-check: timed out waiting for {what} at checkpoint {} ({}) after \
             {tick_n} ticks — level {:?}, type {:?}, generation {}, spawned {}, columns {}, \
             dirty {}, chunk decode failures {}",
            self.checkpoint,
            CHECKPOINTS[self.checkpoint.min(CHECKPOINTS.len() - 1)],
            session.active_dimension_key,
            session.active_dimension_type.as_ref().map(|d| &d.name),
            session.dimension_generation,
            session.spawned,
            session.world.loaded_columns(),
            session.dirty_len(),
            session.chunk_decode_failures,
        )
    }

    // ------------------------------------------------------------ validation

    /// Validate the checkpoint we are standing in. Every failure is fatal.
    fn validate(&mut self, session: &PlaySession, corrections: u32) -> Result<(), String> {
        let index = self.checkpoint;
        let want_level = CHECKPOINTS[index];
        let want_type = expected_type_for(want_level);
        let expect = expect_for(want_type)
            .ok_or_else(|| format!("no expectation for dimension type {want_type}"))?;
        let at = |msg: String| format!("dimension-check: checkpoint {index} ({want_level}): {msg}");

        // -- identity: the level key, and the *type* it resolved to ----------
        let level = session
            .active_dimension_key
            .as_deref()
            .ok_or_else(|| at("no active dimension key".into()))?;
        if level != want_level {
            return Err(at(format!(
                "active level key is {level}, expected {want_level}"
            )));
        }
        let holder = session
            .active_dimension_holder
            .ok_or_else(|| at("no active dimension holder".into()))?;
        let def = session
            .active_dimension_type
            .as_ref()
            .ok_or_else(|| at("no active dimension type".into()))?;
        if def
            .name
            .starts_with(DimensionTypeDef::UNRESOLVED_NAME_PREFIX)
        {
            return Err(at(format!(
                "holder {holder} resolved to nothing ({}) — the synced registry has \
                 {} entries",
                def.name,
                session.dimension_types().len()
            )));
        }
        if def.name != want_type {
            return Err(at(format!(
                "dimension type is {}, expected {want_type}",
                def.name
            )));
        }

        // The holder is checked against the **synced registry**, never against
        // an assumed numeric order: the id must index the entry we resolved,
        // and the entry we resolved must be the only slot with that name.
        let registry = session.dimension_types();
        let slot = registry
            .get(usize::try_from(holder).map_err(|_| at(format!("negative holder {holder}")))?)
            .ok_or_else(|| {
                at(format!(
                    "holder {holder} is past the synced registry's {} entries",
                    registry.len()
                ))
            })?;
        if slot != def {
            return Err(at(format!(
                "holder {holder} indexes {} but the session resolved {}",
                slot.name, def.name
            )));
        }
        let by_name: Vec<usize> = registry
            .iter()
            .enumerate()
            .filter(|(_, d)| d.name == want_type)
            .map(|(i, _)| i)
            .collect();
        if by_name != vec![holder as usize] {
            return Err(at(format!(
                "the synced registry has {want_type} at slots {by_name:?}, but the packet \
                 named holder {holder}"
            )));
        }
        // The same raw id must never resolve to two different entries.
        if let Some((_, seen)) = self.holder_names.iter().find(|(h, _)| *h == holder) {
            if seen != &def.name {
                return Err(at(format!(
                    "holder {holder} resolved to {seen} earlier and {} now",
                    def.name
                )));
            }
        } else {
            self.holder_names.push((holder, def.name.clone()));
        }

        // -- the full property matrix, against the independent expectation ---
        expect
            .grade("live", holder as usize, def)
            .map_err(|e| at(e))?;

        // -- the world must equal the active definition ----------------------
        let w = &session.world;
        if w.shape != def.shape {
            return Err(at(format!(
                "world shape {:?} != active definition {:?}",
                w.shape, def.shape
            )));
        }
        if w.has_sky_light() != def.has_sky_light {
            return Err(at(format!(
                "world has_sky_light {} != active definition {}",
                w.has_sky_light(),
                def.has_sky_light
            )));
        }
        if w.cardinal_light_type() != def.cardinal_light_type
            || w.cardinal_light() != def.cardinal_light
        {
            return Err(at("world cardinal lighting != active definition".into()));
        }

        // -- chunks: this dimension's own, and none of them rejected ---------
        let columns = w.loaded_columns();
        if columns == 0 {
            return Err(at("no loaded columns after settle".into()));
        }
        if session.chunk_decode_failures != 0 {
            return Err(at(format!(
                "{} chunk columns were rejected by the decoder — the cumulative count \
                 must stay 0, and a nonzero one after a dimension change is the exact \
                 signature of a stale vertical shape",
                session.chunk_decode_failures
            )));
        }

        // -- the sky channel contract, sampled from real loaded cells --------
        //
        // `has_sky_light=false` must hold at *every* read path, not just inside
        // a fully-populated section: the sparse-section fallback and the
        // unloaded-column fallback are where an impossible Nether sky 15 came
        // from before M16.
        let (mut sky_samples, mut max_sky) = (0usize, 0u8);
        let (px, pz) = (
            session.player.x.floor() as i32,
            session.player.z.floor() as i32,
        );
        for dx in [-8, 0, 8] {
            for dz in [-8, 0, 8] {
                let (x, z) = (px + dx, pz + dz);
                if !w.is_loaded(x, z) {
                    continue;
                }
                // Whole-column sweep: sections that are present, sections that
                // are sparse, the floor and the ceiling.
                for step in 0..=16 {
                    let y = def.shape.min_y + step * (def.shape.height - 1) / 16;
                    let (_, sky) = w.light_at(x, y, z);
                    sky_samples += 1;
                    max_sky = max_sky.max(sky);
                }
            }
        }
        if sky_samples == 0 {
            return Err(at("no loaded cell could be sampled for sky light".into()));
        }
        // Unloaded / out-of-world reads.
        let mut sparse_samples = 0usize;
        let mut max_sparse_sky = 0u8;
        for (x, y, z) in [
            (px + 100_000, def.shape.min_y + 8, pz + 100_000),
            (px, def.shape.min_y + def.shape.height + 64, pz),
            (px, def.shape.min_y - 64, pz),
        ] {
            let (_, sky) = w.light_at(x, y, z);
            sparse_samples += 1;
            max_sparse_sky = max_sparse_sky.max(sky);
        }
        if !def.has_sky_light && (max_sky != 0 || max_sparse_sky != 0) {
            return Err(at(format!(
                "has_skylight=false but a read returned sky light (loaded max {max_sky}, \
                 sparse/unloaded max {max_sparse_sky}) — this dimension has no sky light \
                 engine at all"
            )));
        }
        if def.has_sky_light && max_sky == 0 {
            return Err(at(format!(
                "has_skylight=true but all {sky_samples} sampled loaded cells read sky 0 \
                 — either the sky channel did not decode, or the sample is vacuous and \
                 the Nether's sky-0 assertion proves nothing"
            )));
        }

        // -- the transition that got us here ---------------------------------
        if session.dimension_generation != index as u64 {
            return Err(at(format!(
                "dimension generation is {}, expected {index} (login establishes 0 and \
                 each changed key adds exactly one)",
                session.dimension_generation
            )));
        }
        if session.dimension_transitions.len() != index {
            return Err(at(format!(
                "{} transitions recorded, expected {index}",
                session.dimension_transitions.len()
            )));
        }
        if index > 0 {
            let t = &session.dimension_transitions[index - 1];
            check_transition(t, CHECKPOINTS[index - 1], want_level, def, index as u64)
                .map_err(at)?;
        }

        // -- the settled window ----------------------------------------------
        if corrections != 0 {
            return Err(at(format!(
                "{corrections} server position corrections during the {} tick settled \
                 window — the respawn teleport itself is excluded by the session's \
                 spawned=false behaviour, so these are real physics disagreements",
                OBSERVE_TICKS
            )));
        }
        // The re-mesh queue is *not* asserted here: the bot never drains it, so
        // its length is an accumulation over the whole session and says nothing
        // about staleness. The queue's transition behaviour is witnessed where
        // it is decidable — `DimensionTransition::dirty_after`, recorded inside
        // the transition itself and checked above.

        self.results.push(Checkpoint {
            index,
            level: level.to_string(),
            type_name: def.name.clone(),
            holder,
            min_y: def.shape.min_y,
            height: def.shape.height,
            has_sky_light: def.has_sky_light,
            skybox: def.skybox.name(),
            cardinal: def.cardinal_light_type.name(),
            ambient_light: def.ambient_light,
            has_day_timeline: def.has_day_timeline,
            generation: session.dimension_generation,
            columns,
            sky_samples,
            max_sky,
            sparse_samples,
            max_sparse_sky,
            corrections_in_window: corrections,
            teleports_total: session.teleports,
            decode_failures: session.chunk_decode_failures,
        });
        self.checkpoint += 1;
        Ok(())
    }

    /// The final report. `Err` if any checkpoint or transition is missing.
    pub fn report(&self, session: &PlaySession) -> Result<(), String> {
        for c in &self.results {
            println!("{}", c.line());
        }
        for (i, t) in session.dimension_transitions.iter().enumerate() {
            println!(
                "[dimension-check] transition {} {:?} -> {} (holder {}, type {}, gen {}) | \
                 discarded {} columns, queued {} for renderer removal (queue {}), new world \
                 {} columns, re-mesh queue {}, clock reset {}",
                i + 1,
                t.old_key,
                t.new_key,
                t.holder,
                t.type_name,
                t.generation,
                t.old_columns,
                t.queued_for_removal,
                t.removal_queue_len,
                t.new_world_columns,
                t.dirty_after,
                t.clock_reset,
            );
        }
        let settled_corrections: u32 = self.results.iter().map(|c| c.corrections_in_window).sum();
        let ok = self.results.len() == CHECKPOINTS.len()
            && session.dimension_transitions.len() == HOPS.len()
            && session.chunk_decode_failures == 0
            && settled_corrections == 0;
        println!(
            "DIMENSION-CHECK: {}/{} checkpoints, {}/{} transitions, chunk decode failures {}, \
             settled corrections {}, teleports {} (respawn + `tp` teleports are reported, \
             not graded — only corrections in the settled windows are){}",
            self.results.len(),
            CHECKPOINTS.len(),
            session.dimension_transitions.len(),
            HOPS.len(),
            session.chunk_decode_failures,
            settled_corrections,
            session.teleports,
            if ok { "" } else { "  — FAILED" },
        );
        if !ok {
            return Err(format!(
                "dimension-check: incomplete — {}/{} checkpoints, {}/{} transitions, \
                 {} decode failures, {} settled corrections",
                self.results.len(),
                CHECKPOINTS.len(),
                session.dimension_transitions.len(),
                HOPS.len(),
                session.chunk_decode_failures,
                settled_corrections,
            ));
        }
        Ok(())
    }
}

/// Grade one recorded transition: the chain it links, the properties it carries,
/// and its discard/reset witnesses.
///
/// Split out as a free function over the recorded values so it can be driven by
/// synthetic transitions in tests without a socket.
fn check_transition(
    t: &DimensionTransition,
    old_key: &str,
    new_key: &str,
    def: &DimensionTypeDef,
    generation: u64,
) -> Result<(), String> {
    if t.old_key.as_deref() != Some(old_key) || t.new_key != new_key {
        return Err(format!(
            "transition is {:?} -> {}, expected {old_key} -> {new_key}",
            t.old_key, t.new_key
        ));
    }
    if t.generation != generation {
        return Err(format!(
            "transition generation {} != {generation}",
            t.generation
        ));
    }
    if t.type_name != def.name
        || t.shape != def.shape
        || t.has_sky_light != def.has_sky_light
        || t.skybox != def.skybox
        || t.ambient_light != def.ambient_light
        || t.cardinal_light_type != def.cardinal_light_type
        || t.has_day_timeline != def.has_day_timeline
    {
        return Err(format!(
            "the recorded transition's properties disagree with the active definition: \
             recorded {t:?}, active {def:?}"
        ));
    }
    // The discard witnesses. Coordinates cannot stand in for these — the new
    // dimension loads the very same column coordinates.
    if t.old_columns == 0 {
        return Err(
            "the world we left had no loaded columns, so the discard is unproven — the \
             gate must not transition out of an unsettled world"
                .into(),
        );
    }
    if t.queued_for_removal != t.old_columns {
        return Err(format!(
            "{} columns were discarded but {} were queued for the renderer to free — \
             the difference is orphaned GPU buffers",
            t.old_columns, t.queued_for_removal
        ));
    }
    if t.removal_queue_len < t.queued_for_removal {
        return Err(format!(
            "the removal queue holds {} entries after queuing {}",
            t.removal_queue_len, t.queued_for_removal
        ));
    }
    if t.new_world_columns != 0 {
        return Err(format!(
            "the replacement world already held {} columns — old columns survived the \
             change",
            t.new_world_columns
        ));
    }
    if t.dirty_after != 0 {
        return Err(format!(
            "{} columns stayed queued for re-mesh across the change — every one names a \
             column of the world we left",
            t.dirty_after
        ));
    }
    if !t.clock_reset {
        return Err(
            "the world clock, its game time or the derived day tick survived the change \
             — they are `ClientLevel` state and go with the level"
                .into(),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rewo_world::dimension::{
        CardinalLightType, DimensionShape, Skybox, DEFAULT_AMBIENT_LIGHT_COLOR,
        DEFAULT_CLOUD_COLOR, DEFAULT_CLOUD_HEIGHT, DEFAULT_SKY_LIGHT_COLOR,
        DEFAULT_SKY_LIGHT_FACTOR,
    };

    fn nether_def() -> DimensionTypeDef {
        DimensionTypeDef {
            name: "minecraft:the_nether".into(),
            shape: DimensionShape::NETHER,
            has_fixed_time: true,
            has_day_timeline: false,
            has_sky_light: false,
            skybox: Skybox::None,
            ambient_light: 0.1,
            cardinal_light_type: CardinalLightType::Nether,
            cardinal_light: CardinalLightType::Nether.get(),
            sky_color: None,
            fog_color: None,
            ambient_light_color: DEFAULT_AMBIENT_LIGHT_COLOR,
            sky_light_color: DEFAULT_SKY_LIGHT_COLOR,
            sky_light_factor: DEFAULT_SKY_LIGHT_FACTOR,
            // The Nether sets no cloud attributes at all; the defaults ARE the
            // behaviour, and the transparent colour is why it has no clouds.
            cloud_color: DEFAULT_CLOUD_COLOR,
            cloud_height: DEFAULT_CLOUD_HEIGHT,
            // …and it sets no ambient sounds either, which is NOT a default
            // shared with the other three: the Overworld, its caves and the End
            // all declare LEGACY_CAVE_SETTINGS here. A universal cave default
            // would play `ambient.cave` in the Nether.
            ambient_sounds: None,
        }
    }

    fn good(def: &DimensionTypeDef) -> DimensionTransition {
        DimensionTransition {
            old_key: Some("minecraft:overworld".into()),
            new_key: "minecraft:the_nether".into(),
            holder: 3,
            type_name: def.name.clone(),
            shape: def.shape,
            has_sky_light: def.has_sky_light,
            skybox: def.skybox,
            ambient_light: def.ambient_light,
            cardinal_light_type: def.cardinal_light_type,
            has_day_timeline: def.has_day_timeline,
            generation: 1,
            old_columns: 25,
            queued_for_removal: 25,
            removal_queue_len: 25,
            new_world_columns: 0,
            dirty_after: 0,
            clock_reset: true,
        }
    }

    fn check(t: &DimensionTransition, def: &DimensionTypeDef) -> Result<(), String> {
        check_transition(t, "minecraft:overworld", "minecraft:the_nether", def, 1)
    }

    #[test]
    fn a_complete_transition_passes() {
        let def = nether_def();
        check(&good(&def), &def).unwrap();
    }

    /// Each witness on its own must be able to fail the gate — otherwise the
    /// checker would pass a transition that never discarded anything.
    #[test]
    fn every_discard_witness_is_load_bearing() {
        let def = nether_def();
        let cases: [(&str, fn(&mut DimensionTransition)); 6] = [
            ("nothing to discard", |t| t.old_columns = 0),
            ("queued fewer than discarded", |t| t.queued_for_removal = 24),
            ("queue shorter than the push", |t| t.removal_queue_len = 3),
            ("old columns carried over", |t| t.new_world_columns = 7),
            ("stale re-mesh entries", |t| t.dirty_after = 2),
            ("clock survived", |t| t.clock_reset = false),
        ];
        for (what, break_it) in cases {
            let mut t = good(&def);
            break_it(&mut t);
            assert!(check(&t, &def).is_err(), "{what} must fail the gate");
        }
    }

    /// A transition whose recorded properties disagree with the dimension we
    /// ended up in is a failure, not a note — this is the "shape decoded one
    /// way, applied another" bug class.
    #[test]
    fn recorded_properties_must_equal_the_active_definition() {
        let def = nether_def();
        let mut t = good(&def);
        t.shape = DimensionShape::OVERWORLD;
        assert!(check(&t, &def).is_err());
        let mut t = good(&def);
        t.has_sky_light = true;
        assert!(check(&t, &def).is_err());
        let mut t = good(&def);
        t.has_day_timeline = true;
        assert!(check(&t, &def).is_err());
        let mut t = good(&def);
        t.cardinal_light_type = CardinalLightType::Default;
        assert!(check(&t, &def).is_err());
    }

    /// The chain, and the generation it must carry.
    #[test]
    fn the_chain_and_generation_are_checked() {
        let def = nether_def();
        let mut t = good(&def);
        t.old_key = Some("minecraft:the_end".into());
        assert!(check(&t, &def).is_err());
        let mut t = good(&def);
        t.generation = 2;
        assert!(check(&t, &def).is_err());
    }

    /// The commands are addressed to the *selected* username and name the level
    /// to `execute in` — the two things a hardcoded account would get wrong.
    #[test]
    fn commands_target_the_selected_username() {
        let dc = DimensionCheck::new("SomeOtherName");
        assert_eq!(
            dc.command(0),
            "execute in minecraft:the_nether run tp SomeOtherName 0 80 0"
        );
        assert_eq!(
            dc.command(1),
            "execute in minecraft:the_end run tp SomeOtherName 0 100 0"
        );
        assert_eq!(
            dc.command(2),
            "execute in minecraft:overworld run tp SomeOtherName 0 -55 0"
        );
    }

    /// The four checkpoints and the three hops line up: hop `n` leaves
    /// checkpoint `n` and lands on checkpoint `n+1`.
    #[test]
    fn the_route_is_overworld_nether_end_overworld() {
        assert_eq!(CHECKPOINTS.len(), HOPS.len() + 1);
        for (i, h) in HOPS.iter().enumerate() {
            assert_eq!(h.level, CHECKPOINTS[i + 1]);
        }
        assert_eq!(CHECKPOINTS[0], "minecraft:overworld");
        assert_eq!(CHECKPOINTS[3], "minecraft:overworld");
        for level in CHECKPOINTS {
            assert!(expect_for(expected_type_for(level)).is_some());
        }
    }
}
