//! `rewo live` — the M3 capstone: connect, play, and SEE it. A real
//! windowed client — the M1 protocol + M3 physics session feeding the M2
//! renderer, with live re-meshing as the world changes.
//!
//! The main loop drives the 20 Hz tick on a 50 ms accumulator and renders
//! every frame from the player's eye. **Meshing happens off the frame**
//! (REWO_PLAN §4): dirty columns are snapshotted (`Arc`-shared, no copies)
//! and meshed on a rayon worker pool (`rewo_mesh::pool::MeshPool`); the
//! frame only uploads finished meshes, metered by a per-frame budget. The
//! socket reader is its own thread, as before.
//!
//! Headless-verifiable: `--run-seconds N` auto-exits and `--out PNG` writes
//! the final frame from the player's eye — a machine-checkable artifact
//! proving the live client renders the actual server world at the player's
//! position.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ash::vk;
use clap::Args as ClapArgs;
use glam::{Mat4, Vec3};
use raw_window_handle::{HasDisplayHandle, HasWindowHandle};
use rewo_data::assets::{self, BakedFont};
use rewo_data::entity_types::EntityTypes;
use rewo_data::{DataPaths, GameData};
use rewo_gpu::celestial::{CelestialImage, CelestialState, CelestialTextures};
use rewo_gpu::end_sky::EndSkyImage;
use rewo_gpu::entities::{
    srgb_to_linear, EntityDraw, EntityModelKind, FontData, MobTexEntry, MobTextures,
};
use rewo_gpu::offscreen::Offscreen;
use rewo_gpu::overlay::OverlayDraw;
use rewo_gpu::renderer::{RenderOutcome, Renderer};
use rewo_gpu::world::{SkyMode, WorldLightmapState, WorldRenderer};
use rewo_gpu::Gpu;
use rewo_mesh::pool::{MeshPool, MeshTables};
use rewo_net::play::PlaySession;
use rewo_net::Connection;
use rewo_world::dimension::{DimensionTypeDef, Skybox};
use rewo_world::lightmap::{
    darkness_lightmap, night_vision_intensity, rgb24_to_vec3, sample, BlockLightFlicker,
    LightmapState,
};
use rewo_world::physics::{TickInput, EYE_HEIGHT};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{DeviceEvent, DeviceId, ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{CursorGrabMode, Window, WindowId};

use crate::stats::{OverlayRing, StatsAccum};

const CLEAR_SKY: [f32; 4] = [0.184, 0.380, 1.0, 1.0];
const TICK_DT: f32 = 0.05; // 20 Hz
/// Max finished meshes uploaded to the GPU per frame — the rest stay queued
/// in the pool's result channel. (Meshing itself is unmetered: it runs on
/// the worker pool, off the frame.)
const UPLOAD_BUDGET: usize = 6;

#[derive(ClapArgs)]
pub struct LiveArgs {
    #[arg(long, default_value = "127.0.0.1")]
    host: String,
    #[arg(long, default_value_t = 25599)]
    port: u16,
    /// Player name. Defaults to the launcher's `REWO_USERNAME` env handoff
    /// (REWO_PLAN §9.1), then "RewoLive".
    #[arg(long)]
    username: Option<String>,
    #[arg(long, default_value = "26.2")]
    version: String,
    /// Auto-exit after N seconds (headless soak).
    #[arg(long)]
    run_seconds: Option<f32>,
    /// Write the final frame to this PNG (headless verification artifact).
    #[arg(long)]
    out: Option<PathBuf>,
    /// Frames in flight (M6 latency knob): 1 = lowest latency, 2 = default.
    #[arg(long, default_value_t = 2)]
    fif: usize,
    /// Load an OptiFine CEM resource-pack zip (M9): mobs render with the
    /// pack's custom models. Also read from `REWO_PACK` if unset.
    #[arg(long)]
    pack: Option<PathBuf>,
    /// Brightness gamma (`Options.gamma`, default 0.5) — the notGamma lift
    /// weight in the camera lightmap (M13). Must be in `[0, 1]`.
    #[arg(long, default_value_t = 0.5)]
    gamma: f32,
    /// Darkness mob-effect scale (`Options.darknessEffectScale`, default 1.0)
    /// — scales the Darkness pulse in the camera lightmap (M13). `[0, 1]`.
    #[arg(long = "darkness-effect-scale", default_value_t = 1.0)]
    darkness_effect_scale: f32,
    #[arg(long, default_value_t = false)]
    no_validation: bool,
}

/// Reject a `[0, 1]` option before any I/O so a bad `--gamma` /
/// `--darkness-effect-scale` fails fast with a named error (M13).
fn validate_unit(name: &str, v: f32) -> Result<(), String> {
    if !v.is_finite() || !(0.0..=1.0).contains(&v) {
        return Err(format!(
            "--{name} must be a finite value in [0, 1], got {v}"
        ));
    }
    Ok(())
}

pub fn run(args: LiveArgs) -> Result<(), String> {
    // M13 camera-lightmap options — validate BEFORE loading/baking/connecting
    // so a bad value fails fast without any side effects.
    validate_unit("gamma", args.gamma)?;
    validate_unit("darkness-effect-scale", args.darkness_effect_scale)?;

    let data = GameData::load_for_version(&args.version)?;
    let jar = client_jar_path(&args.version).ok_or("client jar not found")?;
    let paths = DataPaths::for_version(&args.version).ok_or("no config dir")?;
    let baked = assets::bake(&jar, &paths.blocks_json())?;
    // Per-state collision shapes (slabs/stairs/fences, not just full cubes).
    let collide: Vec<Vec<[f32; 6]>> = baked.collide.clone();
    let global_bits = data.blocks.global_palette_bits;
    // Launcher account handoff — online-mode servers need it, offline
    // servers ignore it. The explicit --username wins for the name.
    let auth = rewo_net::crypt::OnlineAuth::from_env();
    let username = args
        .username
        .clone()
        .or_else(|| auth.as_ref().map(|a| a.username.clone()))
        // Offline launcher handoff sets the name without a token.
        .or_else(|| std::env::var("REWO_USERNAME").ok())
        .unwrap_or_else(|| "RewoLive".into());

    let dirt_item = data.items.id("dirt");
    let colormaps = rewo_world::biome::Colormaps::from_pixels(
        baked.grass_colormap.clone(),
        baked.foliage_colormap.clone(),
        baked.dry_foliage_colormap.clone(),
    );
    let conn = Connection::connect(&args.host, args.port, &data)?;
    let mut session = conn.into_play(
        &args.host,
        args.port,
        &username,
        auth.as_ref(),
        collide,
        global_bits,
        colormaps,
    )?;
    // Entity collision: per-type footprint + whether it shoves (living only).
    session.entity_push = entity_push_table(&data.entity_types);
    // Resolve the kinds whose entity events drive model rigs — a
    // `ClientboundEntityEventPacket` byte is polymorphic by entity class, so
    // the id alone can't name the animation.
    session.warden_type_id = data.entity_types.id_of("minecraft:warden");
    session.armadillo_type_id = data.entity_types.id_of("minecraft:armadillo");
    // The Allay's type id disambiguates its index-16 `DATA_DANCING` from the
    // modeled baby path at the same slot (both index 16, both BOOLEAN).
    session.allay_type_id = data.entity_types.id_of("minecraft:allay");
    // Client-side relighting of our own edits — the server only sends light
    // on chunk load, never for a placed torch or a broken roof.
    session.set_light_tables(
        baked.emission.clone(),
        baked.dampening.clone(),
        baked.face_occludes.clone(),
    );
    log::info!("live: session up, opening window…");
    let etypes = data.entity_types;

    let want_validation = cfg!(debug_assertions) && !args.no_validation;
    match &args.out {
        // Headless: pump the session until spawn + a settle window, render
        // one frame from the eye, save. No window at all.
        Some(out) if args.run_seconds.is_none() => {
            let settle = std::env::var("REWO_SETTLE")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(6.0);
            run_headless(
                session,
                baked,
                etypes,
                want_validation,
                out,
                settle,
                dirt_item,
                args.pack.clone(),
                args.gamma,
                args.darkness_effect_scale,
            )
        }
        _ => run_windowed(session, baked, etypes, args, want_validation, dirt_item),
    }
}

/// Velvet-tinted linear colors for the capsule set.
fn linear_rgb(r: u8, g: u8, b: u8) -> [f32; 3] {
    [
        srgb_to_linear(r as f32 / 255.0),
        srgb_to_linear(g as f32 / 255.0),
        srgb_to_linear(b as f32 / 255.0),
    ]
}

/// Camera basis vectors for nametag billboards, from MC-convention angles.
fn camera_basis(yaw_deg: f32, pitch_deg: f32) -> ([f32; 3], [f32; 3]) {
    let yaw = yaw_deg.to_radians();
    let pitch = pitch_deg.to_radians();
    let dir = Vec3::new(
        -yaw.sin() * pitch.cos(),
        -pitch.sin(),
        yaw.cos() * pitch.cos(),
    );
    let right = dir.cross(Vec3::Y).normalize_or_zero();
    let up = right.cross(dir).normalize_or_zero();
    (right.to_array(), up.to_array())
}

/// One resolved player skin: the atlas UV offset relocating the default
/// player quads onto the uploaded slot, plus the arm model.
#[derive(Clone, Copy)]
pub(crate) struct PlayerSkin {
    uv: [f32; 2],
    slim: bool,
}

pub(crate) type SkinRegistry = std::collections::HashMap<u128, PlayerSkin>;

/// Async player-skin loader: a worker thread fetches + decodes skin PNGs
/// off the render/tick path; the main loop uploads each result into the
/// entity atlas and records its UV offset. Skins arrive rarely (once per
/// player at join), so the per-skin `wait_idle` in the upload is cheap.
pub(crate) struct SkinLoader {
    req_tx: std::sync::mpsc::Sender<(u128, String, bool)>,
    res_rx: std::sync::mpsc::Receiver<(u128, bool, Vec<u8>)>,
    requested: std::collections::HashSet<u128>,
    registry: SkinRegistry,
}

impl SkinLoader {
    fn new() -> Self {
        let (req_tx, req_rx) = std::sync::mpsc::channel::<(u128, String, bool)>();
        let (res_tx, res_rx) = std::sync::mpsc::channel::<(u128, bool, Vec<u8>)>();
        std::thread::Builder::new()
            .name("rewo-skin-fetch".into())
            .spawn(move || {
                while let Ok((uuid, url, slim)) = req_rx.recv() {
                    match crate::skin_fetch::fetch_rgba64(&url) {
                        Ok(rgba) => {
                            if res_tx.send((uuid, slim, rgba)).is_err() {
                                return;
                            }
                        }
                        Err(e) => log::warn!("skin: fetch {url} failed: {e}"),
                    }
                }
            })
            .ok();
        Self {
            req_tx,
            res_rx,
            requested: std::collections::HashSet::new(),
            registry: SkinRegistry::new(),
        }
    }

    /// Queue a skin fetch (once per UUID).
    fn request(&mut self, uuid: u128, info: &rewo_net::skins::SkinInfo) {
        if self.requested.insert(uuid) {
            let _ = self.req_tx.send((uuid, info.url.clone(), info.slim));
        }
    }

    /// Upload any fetched skins into the atlas + record their UV offsets.
    fn poll_uploads(&mut self, gpu: &mut Gpu, wr: &mut WorldRenderer) {
        while let Ok((uuid, slim, rgba)) = self.res_rx.try_recv() {
            if let Some(uv) = wr.upload_player_skin(gpu, &rgba) {
                self.registry.insert(uuid, PlayerSkin { uv, slim });
                log::info!(
                    "skin: uploaded for {uuid:032x} ({} model)",
                    if slim { "slim" } else { "wide" }
                );
            }
        }
    }
}

/// Tracks when each entity's gesture-driving state changed. The wire
/// carries only the *current* pose/state — vanilla clients time the rigs
/// from the transition instant, so we record it here. Both a wall-clock
/// second (the rig's time base) and the network tick of the transition are
/// kept: the tick is what the entity-event ownership rules compare against
/// (vanilla orders `AnimationState.start`/`.stop` by tick — see
/// [`resolve_mob_anim`]).
#[derive(Clone, Copy)]
struct GestureEntry {
    gesture: rewo_gpu::mobs::Gesture,
    start_seconds: f32,
    start_tick: i64,
}

#[derive(Default)]
pub(crate) struct GestureTracker {
    map: std::collections::HashMap<i32, GestureEntry>,
}

impl GestureTracker {
    /// Record `wanted` for this entity at `now` seconds / `tick`; returns the
    /// active gesture, its age in seconds, and the network tick it started on.
    /// `head_start` pre-advances a *newly entered* gesture's clock (vanilla's
    /// SCARED `fastForward`). A repeated *same* gesture keeps its original
    /// transition tick — so metadata that re-arrives without an actual pose
    /// change is not a new transition.
    fn update(
        &mut self,
        id: i32,
        wanted: Option<rewo_gpu::mobs::Gesture>,
        now: f32,
        head_start: f32,
        tick: i64,
    ) -> Option<(rewo_gpu::mobs::Gesture, f32, i64)> {
        match wanted {
            None => {
                self.map.remove(&id);
                None
            }
            Some(g) => {
                let e = match self.map.get(&id) {
                    Some(e) if e.gesture == g => *e,
                    _ => {
                        let e = GestureEntry {
                            gesture: g,
                            start_seconds: now - head_start,
                            start_tick: tick,
                        };
                        self.map.insert(id, e);
                        e
                    }
                };
                Some((g, now - e.start_seconds, e.start_tick))
            }
        }
    }
}

/// Elapsed seconds since a one-shot event's receipt tick, using vanilla's
/// `ageInTicks = tickCount + partialTick` convention: `(now_tick − start +
/// partial) · 0.05`. `None` passes through (the event never fired). Clamped
/// non-negative for the degenerate same-tick-receipt case.
fn event_age_seconds(start_tick: Option<i64>, tick: i64, alpha: f32) -> Option<f32> {
    start_tick.map(|s| ((((tick - s) as f32) + alpha) * 0.05).max(0.0))
}

/// Resolve a mob's per-frame rig inputs from its wire pose/state plus the
/// one-shot entity-event side-table, applying the exact vanilla ownership
/// rules. Shared by the live collector and the `eventshot` oracle so those
/// rules are exercised through production code, not a copy.
///
/// - **Warden id 4 (attack)** calls `roarAnimationState.stop()` and never
///   restarts the roar until a fresh ROARING pose transition. So the metadata
///   roar is suppressed iff the attack event's receipt tick is at/after the
///   roar's transition tick — vanilla's start/stop tick ordering. The attack
///   rig itself still plays (through the returned `events`).
/// - **Armadillo id 64 (peek)** stops and re-`startIfStopped`s the SCARED
///   peek — the SAME shared `peekAnimationState` as the metadata 2.5 s hold —
///   so it re-clocks the existing SCARED gesture from age 0 rather than adding
///   a second rig. Only when the event landed during this SCARED episode
///   (receipt tick at/after the SCARED transition), else a stale event from a
///   previous roll can't disturb the current hold.
///
/// Returns the gesture (post-ownership-rules) and the per-`ModelEvent` ages.
pub(crate) fn resolve_mob_anim(
    kind: EntityModelKind,
    pose: u8,
    state: u8,
    attack_tick: Option<i64>,
    sonic_tick: Option<i64>,
    peek_tick: Option<i64>,
    gestures: &mut GestureTracker,
    id: i32,
    now: f32,
    tick: i64,
    alpha: f32,
) -> (
    Option<(rewo_gpu::mobs::Gesture, f32)>,
    [Option<f32>; rewo_gpu::mobs::ModelEvent::COUNT],
) {
    use rewo_gpu::mobs::Gesture;
    let events = [
        event_age_seconds(attack_tick, tick, alpha),
        event_age_seconds(sonic_tick, tick, alpha),
    ];
    let wanted = wanted_gesture(kind, pose, state);
    // Entering SCARED starts the peek at its held ball pose — vanilla
    // `fastForward(SCARED.animationDuration())` = 2.5 s.
    let head_start = if wanted == Some(Gesture::ArmadilloScared) { 2.5 } else { 0.0 };
    let resolved = gestures.update(id, wanted, now, head_start, tick);
    let gesture = resolved.and_then(|(g, age, start_tick)| match g {
        // Attack stopped the roar durably (until a fresh ROARING transition).
        Gesture::WardenRoar if attack_tick.is_some_and(|a| a >= start_tick) => None,
        // Peek re-clocks the SCARED hold from age 0.
        Gesture::ArmadilloScared if peek_tick.is_some_and(|p| p >= start_tick) => {
            Some((g, event_age_seconds(peek_tick, tick, alpha).unwrap_or(0.0)))
        }
        _ => Some((g, age)),
    });
    (gesture, events)
}

/// Wire state → gesture for the rigged kinds. Pose ordinals are
/// `Pose.java`'s ids; state ordinals are the `Sniffer.State` /
/// `ArmadilloState` enum orders (metadata index 17).
fn wanted_gesture(kind: EntityModelKind, pose: u8, state: u8) -> Option<rewo_gpu::mobs::Gesture> {
    use rewo_gpu::mobs::Gesture::*;
    Some(match kind {
        EntityModelKind::Warden => match pose {
            11 => WardenRoar,
            12 => WardenSniff,
            13 => WardenEmerge,
            14 => WardenDig,
            _ => return None,
        },
        EntityModelKind::Frog => match pose {
            8 => FrogCroak,
            9 => FrogTongue,
            _ => return None,
        },
        EntityModelKind::Breeze => match pose {
            6 => BreezeJump,
            15 => BreezeSlide,
            16 => BreezeShoot,
            17 => BreezeInhale,
            _ => return None,
        },
        EntityModelKind::Sniffer => match state {
            1 => SnifferFeelingHappy,
            2 => SnifferScenting,
            3 => SnifferSniffing,
            4 => SnifferSearching,
            5 => SnifferDigging,
            6 => SnifferRising,
            _ => return None,
        },
        EntityModelKind::Armadillo => match state {
            1 => ArmadilloRoll,
            2 => ArmadilloScared,
            3 => ArmadilloUnroll,
            _ => return None,
        },
        _ => return None,
    })
}

/// Resolve an entity's Allay dance render inputs — the production seam shared by
/// the live collector and the `danceshot` oracle, so the kind gate + the
/// counter → `(is_spinning, spinning_progress)` → [`rewo_gpu::mobs::AllayDance`]
/// mapping are the same code the gate proves. `Some` only for an Allay-kind
/// entity that is currently dancing; every other kind is inert here even if the
/// entity somehow carried a dance clock.
pub(crate) fn resolve_allay_dance(
    kind: EntityModelKind,
    entities: &rewo_world::entities::EntityTable,
    id: i32,
    alpha: f32,
) -> Option<rewo_gpu::mobs::AllayDance> {
    (kind == EntityModelKind::Allay)
        .then(|| entities.allay_dance_render(id, alpha))
        .flatten()
        .map(|(is_spinning, spinning_progress)| rewo_gpu::mobs::AllayDance {
            is_spinning,
            spinning_progress,
        })
}

/// Snapshot every tracked entity into this frame's draw list. `alpha` is
/// the partial-tick blend (0..1). Players get the rose capsule + nametag;
/// everything else gets mauve, sized by the type table. `now` is the
/// render clock in seconds — the gesture rigs' time base.
fn collect_entities<'a>(
    session: &'a PlaySession,
    etypes: &EntityTypes,
    alpha: f32,
    gestures: &mut GestureTracker,
    now: f32,
    skins: &SkinRegistry,
    lightmap: &LightmapState,
) -> Vec<EntityDraw<'a>> {
    let player_color = linear_rgb(0xE5, 0xB8, 0xC5); // accent rose
    let mob_color = linear_rgb(0x9A, 0x80, 0x87); // text mauve
                                                  // Headless-only verification knob: `REWO_FORCE_LIMB=swing,amount`
                                                  // pins every player's walk pose so a still-target PNG can prove the
                                                  // limb-swing mechanism deterministically (a live walker's phase at
                                                  // capture time is timing-dependent). One-shot; zero-cost when unset.
    let force_limb: Option<(f32, f32)> = std::env::var("REWO_FORCE_LIMB").ok().and_then(|s| {
        let mut it = s.split(',');
        Some((
            it.next()?.trim().parse().ok()?,
            it.next()?.trim().parse().ok()?,
        ))
    });
    // Headless-only knob: `REWO_FORCE_HEAD=<degrees>` cranks every mob's head
    // yaw to body-yaw + this offset, so a PNG can prove head-look turns the
    // head independently of the body without depending on live server AI.
    let force_head: Option<f32> = std::env::var("REWO_FORCE_HEAD")
        .ok()
        .and_then(|s| s.trim().parse().ok());
    // Headless-only knob: `REWO_FORCE_GESTURE=<name>[,<age_s>]` pins every
    // gesture-rigged mob into that state (mobshot names, e.g.
    // "warden_roar,1.5") — deterministic gesture PNGs without server AI.
    let force_gesture: Option<(rewo_gpu::mobs::Gesture, f32)> =
        std::env::var("REWO_FORCE_GESTURE").ok().and_then(|s| {
            let mut it = s.split(',');
            let g = rewo_gpu::mobs::Gesture::from_name(it.next()?.trim())?;
            let age = it.next().and_then(|a| a.trim().parse().ok()).unwrap_or(0.0);
            Some((g, age))
        });
    let mut out = Vec::new();
    for (id, e) in session.world.entities.iter() {
        let p = e.render_pos(alpha);
        let name = etypes.name(e.type_id).unwrap_or("");
        let is_player = e.type_id == etypes.player_id;
        // A player with a resolved skin wears it (slim → the Alex model);
        // otherwise the default wide Steve.
        let player_skin = if is_player { skins.get(&e.uuid) } else { None };
        let kind = if is_player {
            match player_skin {
                Some(ps) if ps.slim => EntityModelKind::PlayerSlim,
                _ => EntityModelKind::Player,
            }
        } else {
            rewo_gpu::mobs::kind_for_entity_name(name)
        };
        // Slime / magma-cube size (metadata index 16, vanilla default 1;
        // the model + bbox scale linearly by it). Our slime model is baked
        // at the size-2 look, so scale_mul = size/2 (size 2 → 1.0).
        let cube_size = matches!(kind, EntityModelKind::Slime | EntityModelKind::MagmaCube)
            .then(|| session.world.entities.size(id).unwrap_or(2).clamp(1, 32));
        let (mut w, mut h) = match cube_size {
            Some(sz) => (0.51 * sz as f32, 0.51 * sz as f32),
            None => etypes.dimensions(e.type_id),
        };
        let mut scale_mul = cube_size.map_or(1.0, |sz| sz as f32 / 2.0);
        // Baby mobs (ageable / zombie families) render at ~half scale.
        // Uniform approximation — vanilla keeps the head proportionally
        // larger (a per-part transform), deferred.
        if !is_player && session.world.entities.is_baby(id) {
            scale_mul *= 0.5;
            w *= 0.5;
            h *= 0.5;
        }
        let (limb_swing, limb_amount) = force_limb.unwrap_or_else(|| e.limb());
        // Gesture + wire-event rigs: pose/state → rig, timed from the observed
        // change; one-shot events (warden attack/sonic boom, armadillo peek)
        // resolved against their receipt ticks with the exact vanilla ownership
        // rules (roar-stop, peek re-clock). A forced gesture (headless knob)
        // bypasses the tracker and carries no events.
        let (gesture, events) = if let Some(fg) = force_gesture {
            (Some(fg), [None; rewo_gpu::mobs::ModelEvent::COUNT])
        } else {
            use rewo_world::entities::EntityEvent;
            let ents = &session.world.entities;
            resolve_mob_anim(
                kind,
                ents.pose(id),
                ents.gesture_state(id),
                ents.event_start(id, EntityEvent::WardenAttack),
                ents.event_start(id, EntityEvent::WardenSonicBoom),
                ents.event_start(id, EntityEvent::ArmadilloPeek),
                gestures,
                id,
                now,
                session.ticks as i64,
                alpha,
            )
        };
        // Armadillo shell swap (vanilla `shouldHideInShell` per state):
        // ROLLING balls up after 5 ticks, SCARED always, UNROLLING opens
        // at tick 26.
        let shell = kind == EntityModelKind::Armadillo
            && match gesture {
                Some((rewo_gpu::mobs::Gesture::ArmadilloRoll, age)) => age > 0.25,
                Some((rewo_gpu::mobs::Gesture::ArmadilloScared, _)) => true,
                Some((rewo_gpu::mobs::Gesture::ArmadilloUnroll, age)) => age < 1.3,
                _ => false,
            };
        // Allay dance (index-16 `DATA_DANCING` metadata → client counters).
        // Shared with the `danceshot` oracle so the kind gate + counter mapping
        // are exercised by the gate and can't regress here silently.
        let allay_dance = resolve_allay_dance(kind, &session.world.entities, id, alpha);
        out.push(EntityDraw {
            pos: [p[0] as f32, p[1] as f32, p[2] as f32],
            width: w,
            height: h,
            color: if is_player { player_color } else { mob_color },
            // Players show their profile name; any entity with a metadata
            // custom name shows that (named mobs).
            name: if is_player {
                session.world.entities.name_of(e.uuid)
            } else {
                session.world.entities.custom_name(id)
            },
            kind,
            yaw: e.yaw,
            head_yaw: force_head.map_or(e.head_yaw, |off| e.yaw + off),
            pitch: e.pitch,
            limb_swing,
            limb_amount,
            gesture,
            events,
            shell,
            allay_dance,
            skin_uv: player_skin.map(|ps| ps.uv),
            scale_mul,
            anim_id: (id & 0xffff) as f32,
            light: entity_light(&session.world, p[0], p[1] + h as f64 * 0.85, p[2], lightmap),
        });
    }
    // Drop tracker entries for despawned entities (recycled server ids
    // must not inherit a stale gesture clock).
    gestures
        .map
        .retain(|id, _| session.world.entities.get(*id).is_some());
    out
}

/// Borrow the baked mob-texture table into the entity pass's view type.
/// Initialize the entity pass, optionally overriding mob models from an
/// OptiFine CEM resource pack (`--pack` / `REWO_PACK`). Shared by the
/// headless and windowed live paths (M9).
pub(crate) fn init_entities_maybe_cem(
    wr: &mut WorldRenderer,
    gpu: &mut Gpu,
    baked: &assets::BakedAssets,
    pack: &Option<PathBuf>,
) -> Result<(), String> {
    let pack = pack
        .clone()
        .or_else(|| std::env::var("REWO_PACK").ok().map(PathBuf::from));
    match pack {
        Some(path) => {
            let cem = crate::mobshot_cmd::load_cem_overrides(&path)?;
            log::info!(
                "live: CEM pack {} → {} model overrides",
                path.display(),
                cem.len()
            );
            wr.init_entities_with_cem(gpu, font_data(baked), entity_textures(baked), cem)
        }
        None => wr.init_entities(gpu, font_data(baked), entity_textures(baked)),
    }
}

pub(crate) fn entity_textures(baked: &assets::BakedAssets) -> MobTextures<'_> {
    MobTextures {
        entries: baked
            .mob_textures
            .iter()
            .map(|t| MobTexEntry {
                key: t.key,
                w: t.w,
                h: t.h,
                rgba: &t.rgba,
            })
            .collect(),
    }
}

pub(crate) fn font_data(baked: &assets::BakedAssets) -> Option<FontData<'_>> {
    baked.font.as_ref().map(|f: &BakedFont| FontData {
        atlas: &f.atlas,
        size: f.atlas_size,
        cell: f.cell,
        advance: &f.advance,
        white_texel: f.white_texel,
    })
}

/// Borrow the baked HUD sprites into the renderer's view type.
fn hud_sprite(sp: &assets::HudSprite) -> rewo_gpu::hud::HudSpriteData<'_> {
    rewo_gpu::hud::HudSpriteData {
        rgba: &sp.rgba,
        w: sp.w,
        h: sp.h,
    }
}

fn hud_sprites(baked: &assets::BakedAssets) -> Option<rewo_gpu::hud::HudSpritesData<'_>> {
    let h = baked.hud.as_ref()?;
    Some(rewo_gpu::hud::HudSpritesData {
        hotbar: hud_sprite(&h.hotbar),
        selection: hud_sprite(&h.selection),
        crosshair: hud_sprite(&h.crosshair),
        heart_full: hud_sprite(&h.heart_full),
        heart_half: hud_sprite(&h.heart_half),
        heart_container: hud_sprite(&h.heart_container),
        food_full: hud_sprite(&h.food_full),
        food_half: hud_sprite(&h.food_half),
        food_empty: hud_sprite(&h.food_empty),
    })
}

pub(crate) fn layer_animations(
    baked: &assets::BakedAssets,
) -> Vec<rewo_gpu::world::LayerAnimation> {
    baked
        .animations
        .iter()
        .map(|a| rewo_gpu::world::LayerAnimation {
            layer: a.layer,
            frames: a.frames.clone(),
            order: a.order.clone(),
            frametime: a.frametime,
        })
        .collect()
}

/// MC-convention eye camera: yaw 0 faces +Z (south), yaw+ turns west,
/// pitch+ looks down.
fn eye_view_proj(eye: Vec3, yaw_deg: f32, pitch_deg: f32, aspect: f32) -> [[f32; 4]; 4] {
    let yaw = yaw_deg.to_radians();
    let pitch = pitch_deg.to_radians();
    let dir = Vec3::new(
        -yaw.sin() * pitch.cos(),
        -pitch.sin(),
        yaw.cos() * pitch.cos(),
    );
    let view = Mat4::look_to_rh(eye, dir, Vec3::Y);
    let proj = Mat4::from_cols_array_2d(&rewo_gpu::world::perspective_reverse_z(
        70f32.to_radians(),
        aspect.max(0.01),
        0.05,
    ));
    (proj * view).to_cols_array_2d()
}

fn player_eye(session: &PlaySession) -> Vec3 {
    Vec3::new(
        session.player.x as f32,
        session.player.eye_y() as f32,
        session.player.z as f32,
    )
}

/// Is a finished mesh from a world that no longer exists?
///
/// A dimension change bumps `PlaySession::dimension_generation`, and jobs
/// meshed from the old world can still be in flight. Such an output must be
/// a pure no-op: it may not upload (its geometry belongs to another world)
/// and it may not *remove* either, because the same (cx, cz) may already
/// hold a freshly uploaded column from the current dimension.
fn mesh_output_is_stale(out_generation: u64, current_generation: u64) -> bool {
    out_generation != current_generation
}

/// Per-frame mesh pump. Frees removed columns, uploads up to
/// `upload_budget` finished meshes from the worker pool, then submits
/// fresh snapshots for dirty columns. Returns how many meshes uploaded.
fn pump_meshing(
    session: &mut PlaySession,
    gpu: &mut Gpu,
    world_renderer: &mut WorldRenderer,
    pool: &mut MeshPool,
    upload_budget: usize,
) -> Result<usize, String> {
    for (cx, cz) in session.take_removed() {
        world_renderer.remove_column(gpu, cx, cz);
    }

    // Drain finished jobs — uploads are metered, removals are free.
    let mut uploaded = 0;
    while uploaded < upload_budget {
        let Some(out) = pool.try_recv() else { break };
        // Dimension changed while its job was in flight → drop it whole,
        // before any gone/upload/remove decision: the coords mean nothing in
        // the new world, and removing them could free a current column.
        if mesh_output_is_stale(out.generation, session.dimension_generation) {
            continue;
        }
        // Column forgotten while its job was in flight → don't resurrect it.
        let gone = session.world.column(out.cx, out.cz).is_none();
        match out.mesh {
            Some(mesh) if !gone => {
                world_renderer.upload_column(
                    gpu,
                    out.cx,
                    out.cz,
                    bytemuck::cast_slice(&mesh.vertices),
                    &mesh.indices,
                    bytemuck::cast_slice(&mesh.tvertices),
                    &mesh.tindices,
                    mesh.y_min,
                    mesh.y_max,
                )?;
                uploaded += 1;
            }
            _ => world_renderer.remove_column(gpu, out.cx, out.cz),
        }
    }

    // Submit dirty columns, nearest-to-player first so what you're looking
    // at appears soonest. A column already in flight stays dirty and
    // resubmits after its result lands (per-column ordering — see
    // `rewo_mesh::pool`).
    let mut dirty = session.take_dirty();
    if !dirty.is_empty() {
        let (px, pz) = (session.player.x as f32, session.player.z as f32);
        dirty.sort_by(|a, b| {
            let da = col_dist(*a, px, pz);
            let db = col_dist(*b, px, pz);
            da.partial_cmp(&db).unwrap()
        });
        let mut deferred = Vec::new();
        for (cx, cz) in dirty {
            if session.world.column(cx, cz).is_none() {
                // Nothing to mesh (matches the old `None` arm's removal).
                world_renderer.remove_column(gpu, cx, cz);
                continue;
            }
            if !pool.submit(session.dimension_generation, &session.world, cx, cz) {
                deferred.push((cx, cz));
            }
        }
        session.requeue_dirty(deferred);
    }
    Ok(uploaded)
}

fn col_dist((cx, cz): (i32, i32), px: f32, pz: f32) -> f32 {
    let x = cx as f32 * 16.0 + 8.0 - px;
    let z = cz as f32 * 16.0 + 8.0 - pz;
    x * x + z * z
}

// -- headless ---------------------------------------------------------------

fn run_headless(
    mut session: PlaySession,
    baked: assets::BakedAssets,
    etypes: EntityTypes,
    want_validation: bool,
    out: &std::path::Path,
    settle_seconds: f32,
    dirt_item: Option<i32>,
    pack: Option<PathBuf>,
    gamma: f32,
    darkness_option: f32,
) -> Result<(), String> {
    let _ = dirt_item;
    let mut gpu = Gpu::new(None, want_validation)?;
    let mut off = Offscreen::new(&mut gpu, 1280, 720)?;
    let mut world_renderer =
        WorldRenderer::new(&mut gpu, off.format, assets::TEX_SIZE, &baked.layers)?;
    init_entities_maybe_cem(&mut world_renderer, &mut gpu, &baked, &pack)?;
    init_celestial_if_present(&mut world_renderer, &mut gpu, &baked)?;
    world_renderer.set_animations(layer_animations(&baked));
    if let Some(hud) = hud_sprites(&baked) {
        world_renderer.init_hud(&mut gpu, &hud)?;
    }
    if let Some(font) = font_data(&baked) {
        world_renderer.init_text(&mut gpu, &font)?;
    }

    // Pump the session on a real 20 Hz clock until spawned + settled, so
    // chunks arrive and the player position is real.
    let start = Instant::now();
    let idle = TickInput::default();
    let mut tick = 0u64;
    let mut summoned = false;
    // The block-light flicker (M13): a 48-bit LCG advanced exactly once per
    // successful 20 Hz client tick, mirroring `LightmapRenderStateExtractor`.
    let mut flicker = BlockLightFlicker::random();
    while start.elapsed().as_secs_f32() < settle_seconds {
        let deadline = start + Duration::from_millis(50) * (tick as u32 + 1);
        session.tick(&idle)?;
        flicker.tick();
        if let Some(reason) = &session.disconnect {
            return Err(format!("disconnected: {reason}"));
        }
        // REWO_SUMMON=mob: once spawned, /summon a mob ~3 blocks in front
        // (op required) so the model can be verified without a live one.
        if !summoned && session.spawned {
            // REWO_PRECMD: run one op command before the summon (e.g. clear
            // prior test mobs with `kill @e[type=husk]`), so a re-run starts
            // from a clean scene.
            if let Ok(cmd) = std::env::var("REWO_PRECMD") {
                if !cmd.is_empty() {
                    let _ = session.send_command(&cmd);
                    log::info!("REWO_PRECMD: {cmd}");
                    std::env::remove_var("REWO_PRECMD");
                }
            }
            if let Ok(mob) = std::env::var("REWO_SUMMON") {
                let dir = look_dir(session.player.yaw, 0.0);
                let dist = std::env::var("REWO_SUMMON_DIST")
                    .ok()
                    .and_then(|s| s.trim().parse().ok())
                    .unwrap_or(3.0);
                // Optional vertical offset — float the mob into empty sky so a
                // verification shot isn't occluded by ground clutter.
                let dy: f64 = std::env::var("REWO_SUMMON_DY")
                    .ok()
                    .and_then(|s| s.trim().parse().ok())
                    .unwrap_or(0.0);
                let (sx, sy, sz) = (
                    session.player.x + dir[0] * dist,
                    session.player.y + dy,
                    session.player.z + dir[2] * dist,
                );
                // Optional NBT tail (e.g. REWO_SUMMON_NBT={CustomName:'"Bo"'}).
                let nbt = std::env::var("REWO_SUMMON_NBT").unwrap_or_default();
                let cmd = format!("summon minecraft:{mob} {sx:.2} {sy:.2} {sz:.2} {nbt}");
                if let Err(e) = session.send_command(&cmd) {
                    log::warn!("REWO_SUMMON: {e}");
                }
                log::info!("REWO_SUMMON: {cmd}");
                summoned = true;
            }
        }
        // REWO_CHAT: send a chat line once (verifies the chat overlay).
        if summoned || session.spawned {
            if let Ok(msg) = std::env::var("REWO_CHAT") {
                if !msg.is_empty() {
                    let _ = session.send_chat(&msg);
                    std::env::remove_var("REWO_CHAT");
                }
            }
        }
        tick += 1;
        let now = Instant::now();
        if now < deadline {
            std::thread::sleep(deadline - now);
        }
    }
    if !session.spawned {
        return Err("never spawned".into());
    }
    // Mesh everything loaded — one shot, parallel across the rayon pool
    // (order-preserving; nothing mutates the world while this runs).
    for (cx, cz) in session.take_removed() {
        world_renderer.remove_column(&mut gpu, cx, cz);
    }
    let _ = session.take_dirty(); // superseded — we mesh every column below
    let mut coords = session.world.column_coords();
    coords.sort_unstable();
    let t0 = Instant::now();
    let outputs = rewo_mesh::pool::mesh_all(
        session.dimension_generation,
        &session.world,
        &baked.render,
        &baked.models,
        &coords,
    );
    let mut meshed = 0usize;
    for out in outputs {
        if let Some(mesh) = out.mesh {
            world_renderer.upload_column(
                &mut gpu,
                out.cx,
                out.cz,
                bytemuck::cast_slice(&mesh.vertices),
                &mesh.indices,
                bytemuck::cast_slice(&mesh.tvertices),
                &mesh.tindices,
                mesh.y_min,
                mesh.y_max,
            )?;
            meshed += 1;
        }
    }
    log::info!(
        "live: meshed {} of {} columns in {:.1} ms (parallel one-shot)",
        meshed,
        coords.len(),
        t0.elapsed().as_secs_f32() * 1000.0
    );
    gpu.wait_idle();

    // Look slightly down from the eye toward the horizon (or a debug
    // pitch) — unless REWO_LOOK_ENTITY=1, which aims at the nearest entity
    // so the verification PNG is guaranteed to frame it.
    let eye = player_eye(&session);
    // Single-shot render: a fresh tracker sees every gesture at age 0 (use
    // REWO_FORCE_GESTURE for a specific rig time).
    let mut gestures = GestureTracker::default();
    // Headless single-frame: skins can't finish fetching in time (use
    // `mobshot --skin` for a deterministic real-skin PNG). Empty registry.
    let skins = SkinRegistry::new();
    // Resolve ONE camera lightmap for this frame at a FIXED partial of 1.0,
    // matching vanilla `GameRenderer` (lines 376-386):
    // `lightmapRenderStateExtractor.extract(lightmapRenderState, 1.0F)`. Feed
    // the identical value to both the entity-light sampler and the renderer
    // uniform, so mobs and terrain share a lightmap.
    let partial = 1.0;
    let snapshot = session.visual_effect_snapshot(partial);
    let lightmap = resolve_lightmap(
        session.day_ticks,
        session.active_dimension_type.as_ref(),
        flicker.block_factor(),
        snapshot,
        gamma,
        darkness_option,
        partial,
    );
    let draws = collect_entities(
        &session,
        &etypes,
        1.0,
        &mut gestures,
        0.0,
        &skins,
        &lightmap,
    );
    for (id, e) in session.world.entities.iter() {
        let p = e.render_pos(1.0);
        println!(
            "[rewo-entities] #{id} {} at ({:.2},{:.2},{:.2}) yaw {:.0}{}",
            etypes.name(e.type_id).unwrap_or("?"),
            p[0],
            p[1],
            p[2],
            e.yaw,
            session
                .world
                .entities
                .name_of(e.uuid)
                .map(|n| format!(" name \"{n}\""))
                .unwrap_or_default(),
        );
    }
    println!("[rewo-entities] {} tracked", draws.len());
    let mut yaw = session.player.yaw;
    let mut pitch = std::env::var("REWO_PITCH")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(10.0);
    // REWO_LOOK_AT="x,y,z": aim the camera at a fixed world point, bypassing
    // the entity search — deterministic framing for a summoned target even in
    // a scene full of other entities of the same kind.
    if let Some(pt) = std::env::var("REWO_LOOK_AT").ok().and_then(|s| {
        let mut it = s.split(',');
        Some(Vec3::new(
            it.next()?.trim().parse().ok()?,
            it.next()?.trim().parse().ok()?,
            it.next()?.trim().parse().ok()?,
        ))
    }) {
        let d = pt - Vec3::new(eye.x, eye.y, eye.z);
        let len = d.length().max(1e-4);
        yaw = (-d.x).atan2(d.z).to_degrees();
        pitch = (-d.y / len).asin().to_degrees();
        log::info!(
            "live: LOOK_AT {pt:?} from eye ({:.1},{:.1},{:.1}) -> yaw {yaw:.1} pitch {pitch:.1}",
            eye.x,
            eye.y,
            eye.z
        );
    } else if std::env::var("REWO_LOOK_ENTITY").is_ok() {
        // Aim at the nearest interesting model: a player, or (REWO_LOOK=slime)
        // the nearest slime; else the nearest anything.
        let d = |e: &EntityDraw| {
            (e.pos[0] - eye.x).powi(2) + (e.pos[1] - eye.y).powi(2) + (e.pos[2] - eye.z).powi(2)
        };
        let look = std::env::var("REWO_LOOK").ok();
        let look_kind = look
            .as_deref()
            .map(|s| rewo_gpu::mobs::kind_for_entity_name(&format!("minecraft:{s}")));
        let pref = |e: &&EntityDraw| match look_kind {
            Some(k) if k != EntityModelKind::Capsule => e.kind == k,
            _ => e.name.is_some(),
        };
        // REWO_LOOK_HIGH: among the preferred kind, take the highest one
        // (a floated summon sits above all ground clutter — deterministic).
        let high = std::env::var("REWO_LOOK_HIGH").is_ok();
        let nearest = if high {
            draws
                .iter()
                .filter(pref)
                .max_by(|a, b| a.pos[1].partial_cmp(&b.pos[1]).unwrap())
        } else {
            draws
                .iter()
                .filter(pref)
                .min_by(|a, b| d(a).partial_cmp(&d(b)).unwrap())
        }
        .or_else(|| draws.iter().min_by(|a, b| d(a).partial_cmp(&d(b)).unwrap()));
        if let Some(t) = nearest {
            let d = Vec3::new(
                t.pos[0] - eye.x,
                t.pos[1] + t.height * 0.5 - eye.y,
                t.pos[2] - eye.z,
            );
            let len = d.length().max(1e-4);
            yaw = (-d.x).atan2(d.z).to_degrees();
            pitch = (-d.y / len).asin().to_degrees();
            log::info!(
                "live: aiming at entity ({:.1},{:.1},{:.1})",
                t.pos[0],
                t.pos[1],
                t.pos[2]
            );
        }
    }
    // Block targeting: aim the raycast the same way the camera looks, so the
    // selection outline lands on whatever block the eye view frames.
    let hit = session.target_block(eye_f64(&session), look_dir(yaw, pitch), REACH);
    if let Some(h) = hit {
        log::info!("live: targeting block {:?} face {:?}", h.block, h.face);
    }
    world_renderer.set_selection(hit.map(|h| h.block));
    let (cr, cu) = camera_basis(yaw, pitch);
    // Before `set_entities`: the entity pass reads the eye as the CEM
    // `player_pos_*`, which FA aims mob eyes/heads with.
    world_renderer.set_camera(eye.to_array());
    // Day/night + effects: push the one resolved lightmap plus the sky/fog tint.
    apply_lightmap(
        &mut world_renderer,
        &lightmap,
        session.day_ticks,
        session.active_dimension_type.as_ref(),
    );
    apply_biome_sky_fog(&mut world_renderer, &session);
    world_renderer.set_celestial(celestial_state_of(session.day_ticks));
    world_renderer.set_entities(&draws, cr, cu, start.elapsed().as_secs_f32());
    world_renderer.set_hud(session.health, session.food, 0);
    world_renderer.set_text(build_text(&session, gui_px(1280, 720), 720.0, None, true));
    world_renderer.anim_tick(&mut gpu, session.ticks)?;
    let vp = eye_view_proj(eye, yaw, pitch, 1280.0 / 720.0);
    let ring = OverlayRing::default();
    let draw = OverlayDraw {
        samples_ms: &ring.data,
        head: ring.head(),
        scale_ms: 20.0,
        origin: [16.0, 16.0],
        size: [560.0, 140.0],
    };
    for _ in 0..3 {
        off.render(&gpu, Some((&mut world_renderer, vp)), &draw, CLEAR_SKY)?;
    }
    off.save_png(&gpu, out)?;
    // M16 diagnostic: which dimension the frame was actually resolved from, and
    // the three values that drive it. Printed once, not per frame — enough to
    // tell a Nether frame from an Overworld one in a headless log, without
    // asking anyone to look at the PNG.
    println!(
        "[rewo-m16] dimension: {} skybox {:?} (end_sky asset {}) ambient {:?} sky_light {:?} x{:.3}",
        session
            .active_dimension_type
            .as_ref()
            .map(|d| d.name.as_str())
            .unwrap_or("<unresolved>"),
        world_renderer.sky_mode(),
        if world_renderer.end_sky_ready() { "present" } else { "MISSING" },
        lightmap.ambient_color,
        lightmap.sky_light_color,
        lightmap.sky_factor,
    );
    let total = world_renderer.column_count();
    let gpu_drawn = world_renderer.read_draw_count(&mut gpu);
    println!(
        "[rewo-m3-live] headless: spawned at ({:.1},{:.1},{:.1}), {} columns loaded, GPU cull drew {} of {} ({} culled on GPU)",
        session.player.x,
        session.player.y,
        session.player.z,
        session.world.loaded_columns(),
        gpu_drawn,
        total,
        total as i64 - gpu_drawn as i64,
    );
    println!("[rewo-m3-live] wrote {}", out.display());
    world_renderer.destroy(&mut gpu);
    off.destroy(&mut gpu);
    Ok(())
}

// -- windowed ---------------------------------------------------------------

#[derive(Default)]
struct Keys {
    w: bool,
    a: bool,
    s: bool,
    d: bool,
    jump: bool,
    sneak: bool,
    sprint: bool,
}

impl Keys {
    fn input(&self) -> TickInput {
        TickInput {
            forward: (self.w as i32 - self.s as i32) as f32,
            strafe: (self.a as i32 - self.d as i32) as f32,
            jump: self.jump,
            sneak: self.sneak,
            sprint: self.sprint && self.w,
        }
    }
}

struct LiveState {
    window: Arc<Window>,
    gpu: Gpu,
    renderer: Renderer,
    world_renderer: WorldRenderer,
}

struct LiveApp {
    session: Option<PlaySession>,
    baked: Option<assets::BakedAssets>,
    etypes: EntityTypes,
    pool: MeshPool,
    keys: Keys,
    want_validation: bool,
    run_seconds: Option<f32>,
    fif: usize,
    state: Option<LiveState>,
    ring: OverlayRing,
    cpu: StatsAccum,
    started: Instant,
    last_frame: Option<Instant>,
    tick_accum: f32,
    logged_spawn: bool,
    uploaded_total: usize,
    flood_logged: bool,
    /// Selected hotbar slot 0..8 (number keys), for the HUD selection frame.
    hotbar_slot: u8,
    /// Dirt item id (for right-click place), resolved from the item table.
    dirt_item: Option<i32>,
    /// F3 debug overlay visible (toggled by the F3 key). Default on.
    debug: bool,
    /// Per-entity gesture state-change clocks (pose-driven rigs).
    gestures: GestureTracker,
    /// Block-light flicker (M13): ticked once per successful 20 Hz tick,
    /// mirroring vanilla's `LightmapRenderStateExtractor`.
    flicker: BlockLightFlicker,
    /// `Options.gamma` — the camera-lightmap notGamma lift weight (M13).
    gamma: f32,
    /// `Options.darknessEffectScale` — the Darkness-effect pulse scale (M13).
    darkness_option: f32,
    /// Async player-skin fetch + upload (online-mode real skins).
    skins: SkinLoader,
    /// OptiFine CEM resource pack (M9) — mob-model overrides, applied at
    /// entity-pass init.
    pack: Option<PathBuf>,
    init_error: Option<String>,
}

impl ApplicationHandler for LiveApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title("Rewo · live")
            .with_inner_size(LogicalSize::new(1280.0, 720.0));
        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                self.init_error = Some(format!("create window: {e}"));
                event_loop.exit();
                return;
            }
        };
        let init = (|| -> Result<LiveState, String> {
            let rdh = window
                .display_handle()
                .map_err(|e| format!("dh: {e}"))?
                .as_raw();
            let rwh = window
                .window_handle()
                .map_err(|e| format!("wh: {e}"))?
                .as_raw();
            let mut gpu = Gpu::new(Some((rdh, rwh)), self.want_validation)?;
            let size = window.inner_size();
            let mut renderer = Renderer::with_frames_in_flight(
                &mut gpu,
                size.width.max(1),
                size.height.max(1),
                vk::PresentModeKHR::MAILBOX,
                self.fif,
            )?;
            renderer.ensure_depth(&mut gpu)?;
            let baked = self.baked.take().ok_or("assets consumed")?;
            let mut world_renderer = WorldRenderer::new(
                &mut gpu,
                renderer.swapchain.format,
                assets::TEX_SIZE,
                &baked.layers,
            )?;
            init_entities_maybe_cem(&mut world_renderer, &mut gpu, &baked, &self.pack)?;
            init_celestial_if_present(&mut world_renderer, &mut gpu, &baked)?;
            world_renderer.set_animations(layer_animations(&baked));
            if let Some(hud) = hud_sprites(&baked) {
                world_renderer.init_hud(&mut gpu, &hud)?;
            }
            Ok(LiveState {
                window: window.clone(),
                gpu,
                renderer,
                world_renderer,
            })
        })();
        match init {
            Ok(state) => {
                let _ = state.window.set_cursor_grab(CursorGrabMode::Confined);
                state.window.set_cursor_visible(false);
                self.started = Instant::now();
                self.state = Some(state);
            }
            Err(e) => {
                self.init_error = Some(e);
                event_loop.exit();
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(LiveState { gpu, renderer, .. }) = self.state.as_mut() {
                    if size.width > 0 && size.height > 0 {
                        let _ = renderer.resize(gpu, size.width, size.height);
                        let _ = renderer.ensure_depth(gpu);
                    }
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                let p = event.state == ElementState::Pressed;
                match event.physical_key {
                    PhysicalKey::Code(KeyCode::KeyW) => self.keys.w = p,
                    PhysicalKey::Code(KeyCode::KeyA) => self.keys.a = p,
                    PhysicalKey::Code(KeyCode::KeyS) => self.keys.s = p,
                    PhysicalKey::Code(KeyCode::KeyD) => self.keys.d = p,
                    PhysicalKey::Code(KeyCode::Space) => self.keys.jump = p,
                    PhysicalKey::Code(KeyCode::ShiftLeft) => self.keys.sneak = p,
                    PhysicalKey::Code(KeyCode::ControlLeft) => self.keys.sprint = p,
                    PhysicalKey::Code(KeyCode::Escape) => event_loop.exit(),
                    // F3 toggles the debug overlay (edge-triggered on press).
                    PhysicalKey::Code(KeyCode::F3) if p => self.debug = !self.debug,
                    // Number keys 1..9 select the hotbar slot (HUD frame +
                    // sent to the server so the held item matches).
                    PhysicalKey::Code(code) if p => {
                        if let Some(n) = digit_key(code) {
                            self.hotbar_slot = n;
                            if let Some(s) = self.session.as_mut() {
                                let _ = s.select_hotbar(n);
                            }
                        }
                    }
                    _ => {}
                }
            }
            WindowEvent::MouseInput {
                state: btn, button, ..
            } if btn == ElementState::Pressed => {
                // Left-click digs the targeted block; right-click places
                // against its hit face. (Creative: dig breaks instantly.)
                if let Some(session) = self.session.as_mut() {
                    // The pick ray starts from the f64 eye (`eye_f64`), not the
                    // f32 render eye — at large coordinates the two disagree by
                    // more than a block.
                    if let Some(h) = session.target_block(
                        eye_f64(session),
                        look_dir(session.player.yaw, session.player.pitch),
                        REACH,
                    ) {
                        let [x, y, z] = h.block;
                        let face = face_index(h.face);
                        match button {
                            MouseButton::Left => {
                                let _ = session.start_dig(x, y, z, face);
                            }
                            MouseButton::Right => {
                                let _ = session.use_item_on(x, y, z, face);
                            }
                            _ => {}
                        }
                    }
                }
            }
            WindowEvent::RedrawRequested => self.frame(event_loop),
            _ => {}
        }
    }

    fn device_event(&mut self, _el: &ActiveEventLoop, _id: DeviceId, event: DeviceEvent) {
        if let (DeviceEvent::MouseMotion { delta }, Some(session)) = (&event, self.session.as_mut())
        {
            // Mouse drives the player's look (what we render AND send).
            session.player.yaw += delta.0 as f32 * 0.15;
            session.player.pitch =
                (session.player.pitch - delta.1 as f32 * 0.15).clamp(-89.0, 89.0);
        }
    }

    fn about_to_wait(&mut self, _el: &ActiveEventLoop) {
        if let Some(state) = &self.state {
            state.window.request_redraw();
        }
    }
}

impl LiveApp {
    fn frame(&mut self, event_loop: &ActiveEventLoop) {
        let now = Instant::now();
        let dt = self
            .last_frame
            .map(|l| now.duration_since(l).as_secs_f32())
            .unwrap_or(0.0)
            .min(0.1);
        self.last_frame = Some(now);
        if dt > 0.0 {
            self.cpu.push(dt * 1000.0);
            self.ring.push(dt * 1000.0);
        }

        // Fixed 20 Hz tick on an accumulator.
        let Some(session) = self.session.as_mut() else {
            return;
        };
        self.tick_accum += dt;
        let input = self.keys.input();
        let mut ran_tick = false;
        while self.tick_accum >= TICK_DT {
            self.tick_accum -= TICK_DT;
            if let Err(e) = session.tick(&input) {
                log::error!("live: tick failed: {e}");
                event_loop.exit();
                return;
            }
            // Advance the block-light flicker exactly once per successful tick.
            self.flicker.tick();
            ran_tick = true;
        }
        if let Some(reason) = session.disconnect.clone() {
            log::warn!("live: disconnected: {reason}");
            event_loop.exit();
            return;
        }
        if ran_tick && session.spawned && !self.logged_spawn {
            self.logged_spawn = true;
            log::info!(
                "live: spawned at ({:.1},{:.1},{:.1})",
                session.player.x,
                session.player.y,
                session.player.z
            );
            // Put a stack of dirt in slot 0 so right-click can place (the
            // test server is creative — a no-op elsewhere).
            if let Some(dirt) = self.dirt_item {
                let _ = session.creative_set_hotbar(0, dirt, 64);
                let _ = session.select_hotbar(0);
            }
        }

        let Some(state) = self.state.as_mut() else {
            return;
        };
        // Upload finished meshes + feed the worker pool. (Uploads are
        // async slot-ring submissions — the CPU never waits on the copy;
        // same-queue FIFO ordering keeps this frame's draws safe.)
        match pump_meshing(
            session,
            &mut state.gpu,
            &mut state.world_renderer,
            &mut self.pool,
            UPLOAD_BUDGET,
        ) {
            Ok(n) => {
                self.uploaded_total += n;
                // Log once when the pool first idles after spawn + a settle
                // margin (the chunk stream arrives over the first seconds —
                // firing on the tiny pre-stream batch would be misleading).
                if !self.flood_logged
                    && session.spawned
                    && self.uploaded_total > 0
                    && self.pool.in_flight() == 0
                    && session.dirty_len() == 0
                    && self.started.elapsed().as_secs_f32() >= 2.0
                {
                    self.flood_logged = true;
                    log::info!(
                        "live: initial mesh flood done — {} uploads for {} columns in {:.1}s",
                        self.uploaded_total,
                        session.world.loaded_columns(),
                        self.started.elapsed().as_secs_f32()
                    );
                }
            }
            Err(e) => {
                log::error!("live: remesh failed: {e}");
                event_loop.exit();
                return;
            }
        }

        // Player skins: request any newly-announced ones, upload any that
        // finished fetching (real skins on online-mode servers).
        for (uuid, info) in session.take_pending_skins() {
            self.skins.request(uuid, &info);
        }
        self.skins
            .poll_uploads(&mut state.gpu, &mut state.world_renderer);

        // Entities: frame-interpolated snapshot + camera-billboarded tags.
        let alpha = (self.tick_accum / TICK_DT).clamp(0.0, 1.0);
        let anim_time = self.started.elapsed().as_secs_f32();
        // Resolve ONE camera lightmap for this frame at a FIXED partial of 1.0
        // — not the entity-interpolation `alpha`. Vanilla `GameRenderer` (lines
        // 376-386) calls `lightmapRenderStateExtractor.extract(lightmapRenderState,
        // 1.0F)` with a hard-coded 1.0F, so the camera lightmap is resolved at
        // the tick boundary regardless of the frame's partial tick. Entity
        // position interpolation still uses `alpha` (below); only the lightmap /
        // effect snapshot is pinned to 1.0. This matches the headless path.
        let lightmap_partial = 1.0;
        let snapshot = session.visual_effect_snapshot(lightmap_partial);
        let lightmap = resolve_lightmap(
            session.day_ticks,
            session.active_dimension_type.as_ref(),
            self.flicker.block_factor(),
            snapshot,
            self.gamma,
            self.darkness_option,
            lightmap_partial,
        );
        let draws = collect_entities(
            session,
            &self.etypes,
            alpha,
            &mut self.gestures,
            anim_time,
            &self.skins.registry,
            &lightmap,
        );
        let (cr, cu) = camera_basis(session.player.yaw, session.player.pitch);
        let eye = player_eye(session);
        // Before `set_entities`: the entity pass reads the eye as the CEM
        // `player_pos_*`, which FA aims mob eyes/heads with.
        state.world_renderer.set_camera(eye.to_array());
        apply_lightmap(
            &mut state.world_renderer,
            &lightmap,
            session.day_ticks,
            session.active_dimension_type.as_ref(),
        );
        apply_biome_sky_fog(&mut state.world_renderer, session);
        state.world_renderer.set_entities(&draws, cr, cu, anim_time);
        drop(draws);

        let extent = state.renderer.swapchain.extent;
        let aspect = extent.width.max(1) as f32 / extent.height.max(1) as f32;
        // Targeted block for the selection outline.
        let hit = session.target_block(
            eye_f64(session),
            look_dir(session.player.yaw, session.player.pitch),
            REACH,
        );
        state.world_renderer.set_selection(hit.map(|h| h.block));
        // Camera + lightmap were already set above from the same eye/state this
        // frame — don't duplicate them. Celestial still tracks the world clock.
        state
            .world_renderer
            .set_celestial(celestial_state_of(session.day_ticks));
        state
            .world_renderer
            .set_hud(session.health, session.food, self.hotbar_slot);
        let px = gui_px(extent.width, extent.height);
        let fps = (!self.cpu.is_empty()).then(|| 1000.0 / self.cpu.average().max(0.001));
        state.world_renderer.set_text(build_text(
            session,
            px,
            extent.height as f32,
            fps,
            self.debug,
        ));
        if let Err(e) = state
            .world_renderer
            .anim_tick(&mut state.gpu, session.ticks)
        {
            log::error!("live: texture animation: {e}");
        }
        let vp = eye_view_proj(eye, session.player.yaw, session.player.pitch, aspect);
        let draw = OverlayDraw {
            samples_ms: &self.ring.data,
            head: self.ring.head(),
            scale_ms: 20.0,
            origin: [16.0, 16.0],
            size: [560.0, 140.0],
        };
        let LiveState {
            window,
            gpu,
            renderer,
            world_renderer,
        } = state;
        match renderer.render(gpu, Some((world_renderer, vp)), &draw, CLEAR_SKY) {
            Ok(RenderOutcome::Rendered) | Ok(RenderOutcome::Skipped) => {}
            Ok(RenderOutcome::NeedsRecreate) => {
                let size = window.inner_size();
                let _ = renderer.recreate(gpu, size.width, size.height);
                let _ = renderer.ensure_depth(gpu);
            }
            Err(e) => {
                log::error!("live: render failed: {e}");
                event_loop.exit();
            }
        }

        if let Some(limit) = self.run_seconds {
            if self.started.elapsed().as_secs_f32() >= limit {
                event_loop.exit();
            }
        }
    }
}

fn run_windowed(
    session: PlaySession,
    baked: assets::BakedAssets,
    etypes: EntityTypes,
    args: LiveArgs,
    want_validation: bool,
    dirt_item: Option<i32>,
) -> Result<(), String> {
    let event_loop = EventLoop::new().map_err(|e| format!("event loop: {e}"))?;
    event_loop.set_control_flow(ControlFlow::Poll);
    let pool = MeshPool::new(MeshTables {
        render: baked.render.clone(),
        models: baked.models.clone(),
    })?;
    let mut app = LiveApp {
        session: Some(session),
        baked: Some(baked),
        etypes,
        pool,
        keys: Keys::default(),
        want_validation,
        run_seconds: args.run_seconds,
        fif: args.fif,
        state: None,
        ring: OverlayRing::default(),
        cpu: StatsAccum::default(),
        started: Instant::now(),
        last_frame: None,
        tick_accum: 0.0,
        logged_spawn: false,
        uploaded_total: 0,
        flood_logged: false,
        hotbar_slot: 0,
        dirt_item,
        debug: true,
        gestures: GestureTracker::default(),
        flicker: BlockLightFlicker::random(),
        gamma: args.gamma,
        darkness_option: args.darkness_effect_scale,
        skins: SkinLoader::new(),
        pack: args.pack.clone(),
        init_error: None,
    };
    event_loop
        .run_app(&mut app)
        .map_err(|e| format!("event loop run: {e}"))?;
    if let Some(e) = app.init_error.take() {
        return Err(e);
    }
    let elapsed = app.started.elapsed().as_secs_f32();
    if let (Some(mut state), Some(session)) = (app.state.take(), app.session.take()) {
        println!(
            "[rewo-m3-live] windowed: {:.1}s, {} frames, avg fps {:.0}, frames-in-flight {}",
            elapsed,
            app.cpu.len(),
            app.cpu.len() as f32 / elapsed.max(0.001),
            state.renderer.frames_in_flight(),
        );
        println!(
            "[rewo-m3-live] frame time: avg {:.2}  p99 {:.2}  1% low {:.2}  0.1% low {:.2}  max {:.2} ms",
            app.cpu.average(),
            app.cpu.percentile(0.99),
            app.cpu.low_mean(0.01),
            app.cpu.low_mean(0.001),
            app.cpu.percentile(1.0),
        );
        println!(
            "[rewo-m3-live] final pos ({:.1},{:.1},{:.1}), corrections {}, columns {}",
            session.player.x,
            session.player.y,
            session.player.z,
            session.corrections,
            session.world.loaded_columns(),
        );
        println!(
            "[rewo-m3-live] mesh pool: {} column uploads over the session ({} still in flight)",
            app.uploaded_total,
            app.pool.in_flight(),
        );
        println!(
            "[rewo-m3-live] entities tracked at exit: {}",
            session.world.entities.len(),
        );
        let _ = EYE_HEIGHT;
        state.world_renderer.destroy(&mut state.gpu);
        state.renderer.destroy(&mut state.gpu);
    }
    Ok(())
}

fn client_jar_path(version: &str) -> Option<PathBuf> {
    let mut p = dirs::config_dir()?;
    p.push("EwoClient");
    p.push("shared/versions");
    p.push(version);
    p.push(format!("{version}.jar"));
    p.exists().then_some(p)
}

/// Build this frame's overlay text: an F3-style debug block (top-left, when
/// `debug`) and the last few chat messages (above the hotbar). GUI scale
/// `px`; `fps` is shown in the header when known (windowed only).
fn build_text(
    session: &PlaySession,
    px: f32,
    screen_h: f32,
    fps: Option<f32>,
    debug: bool,
) -> Vec<rewo_gpu::world::OwnedTextLine> {
    use rewo_gpu::world::OwnedTextLine;
    let white = [0.93, 0.93, 0.93];
    let mut lines = Vec::new();
    // F3 debug block (top-left). Toggled by F3 in the windowed client;
    // always on in headless so a verification PNG shows the state.
    if debug {
        let p = &session.player;
        let (bx, by, bz) = (p.x.floor() as i64, p.y.floor() as i64, p.z.floor() as i64);
        // Chunk-relative block coords (0..15) — Rust's rem_euclid keeps them
        // non-negative in the negative hemisphere, matching vanilla's F3.
        let (rbx, rbz) = (bx.rem_euclid(16), bz.rem_euclid(16));
        let (cx, cz) = (bx.div_euclid(16), bz.div_euclid(16));
        let toward = facing_axis(p.yaw, p.pitch);
        let header = match fps {
            Some(f) => format!("Rewo 26.2   {f:.0} fps"),
            None => "Rewo 26.2".to_string(),
        };
        let f3 = [
            header,
            format!("XYZ: {:.3} / {:.3} / {:.3}", p.x, p.y, p.z),
            format!(
                "Block: {bx} {by} {bz}   [{rbx} {} {rbz}]",
                by.rem_euclid(16)
            ),
            format!("Chunk: {cx} {cz}"),
            {
                // Vanilla F3's "Client Light" — sampled at the eye, the same
                // cell entity lighting uses.
                let (bl, sl) =
                    session
                        .world
                        .light_at(bx as i32, p.eye_y().floor() as i32, bz as i32);
                format!("Light: {} ({sl} sky, {bl} block)", bl.max(sl))
            },
            format!(
                "Facing: {} {}  ({:.1} / {:.1})",
                compass(p.yaw),
                toward,
                p.yaw,
                p.pitch
            ),
            format!(
                "Loaded: {} chunks   Entities: {}   {}",
                session.world.loaded_columns(),
                session.world.entities.len(),
                if p.on_ground { "grounded" } else { "airborne" },
            ),
        ];
        for (i, text) in f3.into_iter().enumerate() {
            lines.push(OwnedTextLine {
                x: 3.0 * px,
                y: (3.0 + i as f32 * 10.0) * px,
                px,
                color: white,
                alpha: 1.0,
                text,
            });
        }
    }
    // Recent chat (bottom-left, above the hotbar; oldest higher, newest low).
    let chat = &session.chat_log;
    let show = 8.min(chat.len());
    let line_h = 10.0 * px; // 9px glyph + 1px gap
    let base_y = screen_h - 40.0 * px - line_h; // above the hotbar
    for (i, msg) in chat[chat.len() - show..].iter().enumerate() {
        // Trim overlong lines; a real client wraps.
        let text: String = msg.chars().take(80).collect();
        lines.push(OwnedTextLine {
            x: 3.0 * px,
            y: base_y - (show as f32 - 1.0 - i as f32) * line_h,
            px,
            color: white,
            alpha: 1.0,
            text,
        });
    }
    lines
}

/// Auto GUI scale (vanilla: largest integer fitting a ~320×240 base).
fn gui_px(w: u32, h: u32) -> f32 {
    ((h as f32 / 240.0).min(w as f32 / 320.0))
        .floor()
        .clamp(1.0, 4.0)
}

/// Vanilla F3's "Towards …" axis hint — the dominant world axis of the look
/// direction (used alongside the compass name).
fn facing_axis(yaw_deg: f32, pitch_deg: f32) -> &'static str {
    let d = look_dir(yaw_deg, pitch_deg);
    if d[0].abs() > d[2].abs() {
        if d[0] > 0.0 {
            "(Towards +X)"
        } else {
            "(Towards -X)"
        }
    } else if d[2] > 0.0 {
        "(Towards +Z)"
    } else {
        "(Towards -Z)"
    }
}

/// Cardinal/intercardinal name for a yaw (MC: 0=south/+Z, 90=west/−X).
fn compass(yaw_deg: f32) -> &'static str {
    let a = yaw_deg.rem_euclid(360.0);
    match (a / 45.0).round() as i32 % 8 {
        0 => "S",
        1 => "SW",
        2 => "W",
        3 => "NW",
        4 => "N",
        5 => "NE",
        6 => "E",
        _ => "SE",
    }
}

/// Eye position in f64 (block-precise) — feet + the 1.62 eye height.
fn eye_f64(s: &PlaySession) -> [f64; 3] {
    [s.player.x, s.player.y + 1.62, s.player.z]
}

/// Look direction (unit) from MC-convention yaw/pitch degrees.
fn look_dir(yaw_deg: f32, pitch_deg: f32) -> [f64; 3] {
    let (yaw, pitch) = (yaw_deg.to_radians(), pitch_deg.to_radians());
    [
        (-yaw.sin() * pitch.cos()) as f64,
        (-pitch.sin()) as f64,
        (yaw.cos() * pitch.cos()) as f64,
    ]
}

/// Reach distance (creative). Survival is 3.0; the test server is creative.
const REACH: f64 = 4.5;

/// Face normal → MC face index (0 down, 1 up, 2 north −Z, 3 south +Z,
/// 4 west −X, 5 east +X).
fn face_index(n: [i32; 3]) -> u8 {
    match n {
        [0, -1, 0] => 0,
        [0, 1, 0] => 1,
        [0, 0, -1] => 2,
        [0, 0, 1] => 3,
        [-1, 0, 0] => 4,
        [1, 0, 0] => 5,
        _ => 1,
    }
}

/// Number keys 1..9 → hotbar slot 0..8.
fn digit_key(code: KeyCode) -> Option<u8> {
    Some(match code {
        KeyCode::Digit1 => 0,
        KeyCode::Digit2 => 1,
        KeyCode::Digit3 => 2,
        KeyCode::Digit4 => 3,
        KeyCode::Digit5 => 4,
        KeyCode::Digit6 => 5,
        KeyCode::Digit7 => 6,
        KeyCode::Digit8 => 7,
        KeyCode::Digit9 => 8,
        _ => return None,
    })
}

/// Per entity-type `(width, height, pushable)` for entity collision, indexed
/// by type id. Vanilla only lets living entities shove, so items/projectiles/
/// displays are excluded (see `EntityTypes::pushable`).
pub fn entity_push_table(types: &rewo_data::entity_types::EntityTypes) -> Vec<(f32, f32, bool)> {
    (0..types.len() as i32)
        .map(|id| {
            let (w, h) = types.dimensions(id);
            (w, h, types.pushable(id))
        })
        .collect()
}

/// Per-channel world-light RGB `0..1` for an entity, sampled at `(x, eye_y, z)`
/// through the shared camera `LightmapState` (M13).
///
/// Vanilla samples entity light at `BlockPos.containing(x, eyeY, z)` rather
/// than the feet, and that detail is load-bearing: the feet block is usually
/// the floor the entity stands *in* (light 0), which would render every mob
/// pitch black. Sampling at the eye and running the block/sky levels through
/// the exact same `rewo_world::lightmap::sample` the terrain shader mirrors
/// keeps a mob lit identically to the blocks around it.
fn entity_light(
    world: &rewo_world::World,
    x: f64,
    eye_y: f64,
    z: f64,
    state: &LightmapState,
) -> [f32; 3] {
    let (block, sky) = world.light_at(x.floor() as i32, eye_y.floor() as i32, z.floor() as i32);
    let mut rgb = sample(block, sky, state);
    // The shader's genuine `0/0` black-texel path yields NaN (see
    // `rewo_world::lightmap::sample`'s docs). Map ONLY nonfinite components to
    // 0 so a NaN can't poison the CPU entity-vertex transport. This is a CPU
    // vertex-safety guard, NOT a claimed vanilla rule: the production
    // terrain-store behaviour for that NaN is still to be pinned by the later
    // M13 Vulkan black-NaN readback oracle.
    for c in &mut rgb {
        if !c.is_finite() {
            *c = 0.0;
        }
    }
    rgb
}

/// The cycle state for a world-clock tick, defaulting to full daylight
/// before the first `set_time`.
fn daylight_of(day_ticks: Option<i64>) -> rewo_world::daylight::SkyLighting {
    day_ticks.map_or(
        rewo_world::daylight::SkyLighting::DAY,
        rewo_world::daylight::sky_lighting,
    )
}

/// The active dimension's fixed light attributes, plus whether the Overworld
/// day timeline applies to them (M16).
///
/// `rewo_world::daylight::SkyLighting` is a set of **multipliers** (the
/// `Timelines` tracks modify a base value), so the two layers compose:
/// `sky_factor = dimension.sky_light_factor * timeline.light_factor`, and
/// likewise per channel for the sky-light colour. Vanilla decides whether a
/// track applies from the dimension type's `timelines` tag — the Overworld's
/// `#minecraft:in_overworld` contains `minecraft:day`, while
/// `#minecraft:in_nether` / `#minecraft:in_end` contain only
/// `#minecraft:universal` (the villager schedule), which carries no `visual/*`
/// track at all.
///
/// Rewo decodes whether that holder set contains `minecraft:day` from the
/// exact 26.2 built-in timeline tag reports. It is not inferred from fixed
/// time, skybox, or the registry name.
#[derive(Clone, Copy, Debug, PartialEq)]
struct DimensionLight {
    ambient_color: [f32; 3],
    sky_light_color: [f32; 3],
    sky_light_factor: f32,
    /// `Timelines.OVERWORLD_DAY`'s tracks modify this dimension.
    day_timeline: bool,
}

impl DimensionLight {
    /// Before login (and on every serverless path) there is no dimension: the
    /// `EnvironmentAttributes` codec defaults, with the day timeline applying.
    /// This is exactly the pre-M16 behaviour.
    const UNRESOLVED: Self = Self {
        ambient_color: rewo_world::lightmap::DEFAULT_AMBIENT_COLOR,
        sky_light_color: [1.0, 1.0, 1.0],
        sky_light_factor: 1.0,
        day_timeline: true,
    };
}

fn dimension_light(def: Option<&DimensionTypeDef>) -> DimensionLight {
    match def {
        None => DimensionLight::UNRESOLVED,
        Some(d) => DimensionLight {
            ambient_color: rgb24_to_vec3(d.ambient_light_color),
            sky_light_color: rgb24_to_vec3(d.sky_light_color),
            sky_light_factor: d.sky_light_factor,
            day_timeline: d.has_day_timeline,
        },
    }
}

/// The timeline multipliers that apply in this dimension. A fixed-time
/// dimension gets the identity set (`SkyLighting::DAY` is all-ones), so a stale
/// Overworld clock can never tint the Nether or the End — including across a
/// respawn, where `day_ticks` keeps ticking but the dimension changed.
fn dimension_timeline(
    day_ticks: Option<i64>,
    dim: &DimensionLight,
) -> rewo_world::daylight::SkyLighting {
    if dim.day_timeline {
        daylight_of(day_ticks)
    } else {
        rewo_world::daylight::SkyLighting::DAY
    }
}

/// The dimension's skybox as the renderer's mode (`DimensionType.Skybox` ->
/// `LevelRenderer.addSkyPass`). An unresolved dimension keeps the Overworld
/// sky, which is the codec default for a missing `skybox` field anyway.
fn sky_mode_of(def: Option<&DimensionTypeDef>) -> SkyMode {
    match def.map(|d| d.skybox) {
        Some(Skybox::None) => SkyMode::None,
        Some(Skybox::End) => SkyMode::End,
        Some(Skybox::Overworld) | None => SkyMode::Overworld,
    }
}

/// Resolve the full camera `LightmapState` for one frame (M13).
///
/// The three independent clocks fold into one uniform: the day/night timeline
/// (`day_ticks` → sky factor + colour), the block-light flicker (already
/// advanced into `block_factor`), and the mob-effect `snapshot` (night vision +
/// darkness). `gamma` / `darkness_option` are the player's validated options.
/// Pure — the single seam the tests pin.
fn resolve_lightmap(
    day_ticks: Option<i64>,
    dimension: Option<&DimensionTypeDef>,
    block_factor: f32,
    snapshot: rewo_net::effects::VisualEffectSnapshot,
    gamma: f32,
    darkness_option: f32,
    partial: f32,
) -> LightmapState {
    let dim = dimension_light(dimension);
    let sky = dimension_timeline(day_ticks, &dim);
    let night_vision_factor = snapshot
        .night_vision_duration
        .map_or(0.0, |d| night_vision_intensity(d, partial));
    let (brightness_factor, darkness_scale) = darkness_lightmap(
        gamma,
        darkness_option,
        snapshot.darkness_blend_factor,
        snapshot.tick_count,
        partial,
    );
    LightmapState {
        // The timeline tracks are multipliers over the dimension's base.
        sky_factor: dim.sky_light_factor * sky.light_factor,
        block_factor,
        sky_light_color: std::array::from_fn(|c| dim.sky_light_color[c] * sky.light_color[c]),
        ambient_color: dim.ambient_color,
        brightness_factor,
        darkness_scale,
        night_vision_factor,
    }
}

/// Convert the CPU `LightmapState` into the GPU renderer's mirror. The two
/// structs carry identical fields (only `sky_light_color`/`sky_color` differ in
/// name), so this is a field-for-field copy.
fn to_world_lightmap(s: &LightmapState) -> WorldLightmapState {
    WorldLightmapState {
        sky_factor: s.sky_factor,
        block_factor: s.block_factor,
        sky_color: s.sky_light_color,
        ambient_color: s.ambient_color,
        brightness_factor: s.brightness_factor,
        darkness_scale: s.darkness_scale,
        night_vision_factor: s.night_vision_factor,
    }
}

/// Push one resolved lightmap into the renderer: the full lightmap uniform plus
/// the day/night sky/fog gradient tint (a separate concern from the lightmap).
/// `set_lightmap_state` already carries the sky factor + colour, so this does
/// NOT also call `set_lightmap` — no duplicate set.
fn apply_lightmap(
    wr: &mut WorldRenderer,
    state: &LightmapState,
    day_ticks: Option<i64>,
    dimension: Option<&DimensionTypeDef>,
) {
    wr.set_lightmap_state(to_world_lightmap(state));
    // The sky/fog gradient multiply is a day-timeline track too, so it is gated
    // on the same dimension test — otherwise a midnight Overworld clock would
    // black out the End's `#000000`-based sky and blue-shift its fog.
    let sky = dimension_timeline(day_ticks, &dimension_light(dimension));
    wr.set_sky_tint(sky.sky_color, sky.fog_color);
    // Set every frame, so a dimension change or respawn needs no other
    // bookkeeping and can never leave the previous world's skybox behind.
    wr.set_sky_mode(sky_mode_of(dimension));
}

/// M14: push the camera biome sky/fog base color. It composes with the existing
/// `set_sky_tint` day/night multiply inside the renderer (`sky_base * sky_tint`)
/// and is a per-frame uniform, so a biome/time change never remeshes. No biome
/// context (offline non-biome server) leaves the GPU's default fixed sky.
fn apply_biome_sky_fog(wr: &mut WorldRenderer, session: &PlaySession) {
    let eye = eye_f64(session);
    if let Some(sky) = session.world.camera_sky(eye) {
        let fog = session.world.camera_fog(eye).unwrap_or(sky);
        wr.set_sky_fog_base(argb_to_linear(sky), argb_to_linear(fog));
        return;
    }
    // No biome context (an offline / non-biome server): the positional layer
    // that normally carries the dimension base forward is absent, so read the
    // dimension's own `visual/sky_color` / `visual/fog_color` directly — but
    // ONLY when it actually sets them. The Nether sets NEITHER, and inventing a
    // base for it (black, or the Overworld's) is exactly the guess the
    // decompile does not license; `None` there leaves the GPU default.
    let def = session.active_dimension_type.as_ref();
    if let (Some(sky), Some(fog)) = (def.and_then(|d| d.sky_color), def.and_then(|d| d.fog_color)) {
        wr.set_sky_fog_base(argb_to_linear(sky), argb_to_linear(fog));
    }
}

/// Opaque ARGB int (biome sky/fog color, sRGB) → linear RGB the GPU sky base
/// wants (the SRGB attachment re-encodes on store).
fn argb_to_linear(argb: i32) -> [f32; 3] {
    let r = ((argb >> 16) & 0xFF) as f32 / 255.0;
    let g = ((argb >> 8) & 0xFF) as f32 / 255.0;
    let b = (argb & 0xFF) as f32 / 255.0;
    [srgb_to_linear(r), srgb_to_linear(g), srgb_to_linear(b)]
}

fn init_celestial_if_present(
    wr: &mut WorldRenderer,
    gpu: &mut Gpu,
    baked: &assets::BakedAssets,
) -> Result<(), String> {
    if let Some(cel) = &baked.celestial {
        wr.init_celestial(gpu, &to_gpu_celestial(cel))?;
    }
    // The End skybox texture comes from the same jar bake. Absent -> the End
    // draws no sky, which `WorldRenderer::end_sky_ready` reports honestly
    // rather than substituting an invented colour.
    if let Some(img) = &baked.end_sky {
        wr.init_end_sky(
            gpu,
            &EndSkyImage {
                rgba: &img.rgba,
                w: img.w,
                h: img.h,
            },
        )?;
    } else {
        log::warn!("live: no end_sky.png in the jar bake — the End will render no sky");
    }
    Ok(())
}

fn to_gpu_celestial(cel: &assets::CelestialTextures) -> CelestialTextures<'_> {
    fn img(i: &assets::DecodedImage) -> CelestialImage<'_> {
        CelestialImage {
            rgba: &i.rgba,
            w: i.w,
            h: i.h,
        }
    }
    CelestialTextures {
        sun: img(&cel.sun),
        moons: std::array::from_fn(|k| img(&cel.moons[k])),
    }
}

/// Exact clear-weather Overworld celestial timeline. Before the first server
/// time packet, noon matches the existing `SkyLighting::DAY` fallback.
fn celestial_state_of(day_ticks: Option<i64>) -> CelestialState {
    let c = rewo_world::celestial::celestial_at(day_ticks.unwrap_or(6000));
    let argb = c.sunrise_sunset_color;
    let ch = |sh: u32| ((argb >> sh) & 0xFF) as f32 / 255.0;
    CelestialState {
        sun_angle: c.sun_angle_rad(),
        moon_angle: c.moon_angle_rad(),
        star_angle: c.star_angle_rad(),
        star_brightness: c.star_brightness,
        moon_phase: c.moon_phase,
        sunrise_rgba: [
            srgb_to_linear(ch(16)),
            srgb_to_linear(ch(8)),
            srgb_to_linear(ch(0)),
            ch(24),
        ],
        rain_brightness: 1.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rewo_net::effects::VisualEffectSnapshot;

    #[test]
    fn stale_mesh_output_is_rejected_by_generation() {
        assert!(!mesh_output_is_stale(7, 7));
        assert!(mesh_output_is_stale(6, 7));
        assert!(mesh_output_is_stale(u64::MAX, 0));
    }

    #[test]
    fn resolve_allay_dance_gates_on_kind() {
        use rewo_world::entities::{EntityState, EntityTable};
        let mut t = EntityTable::default();
        t.add(1, EntityState::new(0, 0, 0.0, 0.0, 0.0, 0.0, 0.0));
        t.set_dancing(1, true);
        t.tick_lerp();
        // The Allay kind resolves the live dance from the counters.
        assert!(resolve_allay_dance(EntityModelKind::Allay, &t, 1, 1.0).is_some());
        // A non-Allay kind is inert even though the entity carries a dance clock
        // — the kind gate must survive the extraction.
        assert!(resolve_allay_dance(EntityModelKind::Zombie, &t, 1, 1.0).is_none());
        // No dance entry at all → None regardless of kind.
        assert!(resolve_allay_dance(EntityModelKind::Allay, &t, 999, 1.0).is_none());
    }

    /// The three built-in dimension types, with exactly the fields M16's light
    /// resolution reads, transcribed from
    /// `%APPDATA%/EwoClient/rewo/26.2/decompiled/data/minecraft/dimension_type/`.
    /// Everything else comes from `unresolved_holder` (Overworld-shaped), so
    /// each fixture states precisely what it depends on.
    fn dim(
        name: &str,
        has_fixed_time: bool,
        skybox: Skybox,
        ambient: u32,
        sky_light_color: u32,
        sky_light_factor: f32,
    ) -> DimensionTypeDef {
        DimensionTypeDef {
            name: name.into(),
            has_fixed_time,
            has_day_timeline: !has_fixed_time,
            skybox,
            ambient_light_color: ambient as i32,
            sky_light_color: sky_light_color as i32,
            sky_light_factor,
            ..DimensionTypeDef::unresolved_holder(0)
        }
    }

    /// `overworld.json`: no `has_fixed_time`, no `skybox` (→ codec default
    /// OVERWORLD), ambient `#0a0a0a`, and NO `sky_light_color` /
    /// `sky_light_factor` attribute — so both take the codec defaults.
    fn overworld() -> DimensionTypeDef {
        dim(
            "minecraft:overworld",
            false,
            Skybox::Overworld,
            0xFF0A_0A0A,
            0xFFFF_FFFF,
            1.0,
        )
    }

    /// `the_nether.json`: `has_fixed_time: true`, `skybox: "none"`, ambient
    /// `#302821`, `sky_light_color: "#7a7aff"`, `sky_light_factor: 0.0`.
    fn nether() -> DimensionTypeDef {
        dim(
            "minecraft:the_nether",
            true,
            Skybox::None,
            0xFF30_2821,
            0xFF7A_7AFF,
            0.0,
        )
    }

    /// `the_end.json`: `has_fixed_time: true`, `skybox: "end"`, ambient
    /// `#3f473f`, `sky_light_color: "#ac60cd"`, `sky_light_factor: 0.0`.
    fn the_end() -> DimensionTypeDef {
        dim(
            "minecraft:the_end",
            true,
            Skybox::End,
            0xFF3F_473F,
            0xFFAC_60CD,
            0.0,
        )
    }

    /// A snapshot with no active effects and a given player tick count.
    fn no_effects(tick_count: i32) -> VisualEffectSnapshot {
        VisualEffectSnapshot {
            night_vision_duration: None,
            darkness_blend_factor: 0.0,
            tick_count,
        }
    }

    /// `ARGB.vector3fFromRGB24`, restated: a plain `/255` of the low 24 bits.
    fn rgb24(argb: i32) -> [f32; 3] {
        [
            ((argb >> 16) & 0xFF) as f32 / 255.0,
            ((argb >> 8) & 0xFF) as f32 / 255.0,
            (argb & 0xFF) as f32 / 255.0,
        ]
    }

    #[test]
    fn neutral_state_with_gamma_half() {
        // No day/night clock, resting flicker (1.4), no effects, gamma 0.5:
        // full daylight sky, gamma flows straight through to brightness, and
        // every effect term is off.
        let s = resolve_lightmap(None, None, 1.4, no_effects(0), 0.5, 1.0, 0.0);
        assert_eq!(s.sky_factor, 1.0);
        assert_eq!(s.block_factor, 1.4);
        assert_eq!(s.sky_light_color, [1.0, 1.0, 1.0]);
        assert_eq!(s.brightness_factor, 0.5);
        assert_eq!(s.darkness_scale, 0.0);
        assert_eq!(s.night_vision_factor, 0.0);
    }

    #[test]
    fn night_vision_duration_drives_the_factor() {
        // Absent → 0.
        assert_eq!(
            resolve_lightmap(None, None, 1.4, no_effects(0), 0.5, 1.0, 0.0).night_vision_factor,
            0.0
        );
        // Infinite (`-1`) and > 200 ticks both pin to the full 1.0 seed.
        let inf = VisualEffectSnapshot {
            night_vision_duration: Some(-1),
            ..no_effects(0)
        };
        assert_eq!(
            resolve_lightmap(None, None, 1.4, inf, 0.5, 1.0, 0.0).night_vision_factor,
            1.0
        );
        let long = VisualEffectSnapshot {
            night_vision_duration: Some(400),
            ..no_effects(0)
        };
        assert_eq!(
            resolve_lightmap(None, None, 1.4, long, 0.5, 1.0, 0.0).night_vision_factor,
            1.0
        );
        // Within the last 200 ticks it pulses below 1.0 (the fade-out).
        let ending = VisualEffectSnapshot {
            night_vision_duration: Some(200),
            ..no_effects(0)
        };
        let nv = resolve_lightmap(None, None, 1.4, ending, 0.5, 1.0, 0.0).night_vision_factor;
        assert!(
            nv > 0.0 && nv < 1.0,
            "expected a pulsing NV factor, got {nv}"
        );
    }

    #[test]
    fn darkness_lowers_brightness_and_raises_scale() {
        // Partial darkness (blend 0.3) at the pulse peak (tick 0, cos = 1):
        // brightness drops below gamma, darkness scale goes positive.
        let partial_dark = VisualEffectSnapshot {
            darkness_blend_factor: 0.3,
            ..no_effects(0)
        };
        let s = resolve_lightmap(None, None, 1.4, partial_dark, 0.5, 1.0, 0.0);
        assert!(
            s.brightness_factor < 0.5,
            "brightness {} should dip below gamma",
            s.brightness_factor
        );
        assert!(
            s.brightness_factor > 0.0,
            "brightness {} should stay positive",
            s.brightness_factor
        );
        assert!(
            s.darkness_scale > 0.0,
            "darkness {} should be positive",
            s.darkness_scale
        );

        // Full darkness (blend 1.0): brightness floors to 0 (gamma - 1), and
        // the darkness subtraction maxes at 0.45 * option. This pins the
        // brightness-vs-darkness ordering: as darkness climbs, brightness sinks.
        let full_dark = VisualEffectSnapshot {
            darkness_blend_factor: 1.0,
            ..no_effects(0)
        };
        let s = resolve_lightmap(None, None, 1.4, full_dark, 0.5, 1.0, 0.0);
        assert_eq!(s.brightness_factor, 0.0);
        assert!(
            (s.darkness_scale - 0.45).abs() < 1e-4,
            "darkness {} ~ 0.45",
            s.darkness_scale
        );
        assert!(s.darkness_scale > s.brightness_factor);
    }

    #[test]
    fn day_ticks_drive_the_sky_half() {
        // Midnight (tick 18000) dims and blues the sky half, independent of
        // the block factor (a torch stays as bright).
        let s = resolve_lightmap(Some(18000), None, 1.4, no_effects(0), 0.5, 1.0, 0.0);
        assert_eq!(s.sky_factor, 0.24);
        assert_eq!(s.sky_light_color, [0.48, 0.48, 1.0]);
        assert_eq!(s.block_factor, 1.4);
    }

    #[test]
    fn world_lightmap_conversion_is_field_for_field() {
        let s = resolve_lightmap(Some(18000), None, 1.7, no_effects(0), 0.5, 1.0, 0.0);
        let w = to_world_lightmap(&s);
        assert_eq!(w.sky_factor, s.sky_factor);
        assert_eq!(w.block_factor, s.block_factor);
        assert_eq!(w.sky_color, s.sky_light_color);
        assert_eq!(w.ambient_color, s.ambient_color);
        assert_eq!(w.brightness_factor, s.brightness_factor);
        assert_eq!(w.darkness_scale, s.darkness_scale);
        assert_eq!(w.night_vision_factor, s.night_vision_factor);
    }

    /// A transition-like sequence of consecutive resolves — Overworld → Nether
    /// → End → Overworld — with the world clock *still running* underneath, as
    /// it does across a real `respawn`. Every frame must depend only on the
    /// dimension it was given: no term may carry over from the previous one.
    ///
    /// This is the failure `resolve_lightmap` exists to make impossible. It is
    /// a pure function, so the proof is that each step equals the same step
    /// computed in isolation, and that returning to the Overworld reproduces
    /// the Overworld's own value bit-for-bit.
    #[test]
    fn dimension_transitions_leave_no_stale_term() {
        let (ow, ne, en) = (overworld(), nether(), the_end());
        // The clock keeps ticking across the transitions.
        let ticks = [1000i64, 18000, 6000, 23000];
        let seq = [Some(&ow), Some(&ne), Some(&en), Some(&ow)];

        let mut states = Vec::new();
        for (t, d) in ticks.iter().zip(seq) {
            states.push(resolve_lightmap(
                Some(*t),
                d,
                1.4,
                no_effects(0),
                0.5,
                1.0,
                0.0,
            ));
        }

        // 1. Each step matches the isolated computation for its own inputs.
        for (i, (t, d)) in ticks.iter().zip(seq).enumerate() {
            let isolated = resolve_lightmap(Some(*t), d, 1.4, no_effects(0), 0.5, 1.0, 0.0);
            assert_eq!(states[i], isolated, "step {i} depends on history");
        }

        // 2. The two non-Overworld steps are exactly their registry values,
        //    even though they were preceded by a lit Overworld frame.
        assert_eq!(states[1].sky_factor, 0.0);
        assert_eq!(states[1].ambient_color, rgb24(0xFF30_2821u32 as i32));
        assert_eq!(states[1].sky_light_color, rgb24(0xFF7A_7AFFu32 as i32));
        assert_eq!(states[2].sky_factor, 0.0);
        assert_eq!(states[2].ambient_color, rgb24(0xFF3F_473Fu32 as i32));
        assert_eq!(states[2].sky_light_color, rgb24(0xFFAC_60CDu32 as i32));

        // 2b. Reject the specific "the timeline got multiplied in anyway"
        //     failure by name: step 1 sits at midnight, where the Overworld
        //     tracks are (0.24, [0.48, 0.48, 1.0]). A leaked multiply would not
        //     be zero or default — it would be a plausible-looking third
        //     colour, which is exactly why it needs its own assertion.
        for (i, base) in [(1usize, 0xFF7A_7AFFu32 as i32)] {
            let leaked: [f32; 3] = std::array::from_fn(|c| rgb24(base)[c] * [0.48, 0.48, 1.0][c]);
            assert_ne!(
                states[i].sky_light_color, leaked,
                "step {i} leaked the clock"
            );
        }

        // 3. Back in the Overworld at tick 23000 the timeline drives the sky
        //    again — the End's factor-0 must not have stuck.
        let fresh = resolve_lightmap(Some(23000), Some(&ow), 1.4, no_effects(0), 0.5, 1.0, 0.0);
        assert_eq!(states[3], fresh);
        assert!(
            states[3].sky_factor > 0.0,
            "the Overworld sky came back dead: {}",
            states[3].sky_factor
        );
        assert_eq!(states[3].ambient_color, rgb24(0xFF0A_0A0Au32 as i32));

        // 4. And the sky *mode* follows the same sequence, so a transition can
        //    never leave the previous world's skybox on screen.
        let modes: Vec<SkyMode> = seq.iter().map(|d| sky_mode_of(*d)).collect();
        assert_eq!(
            modes,
            vec![
                SkyMode::Overworld,
                SkyMode::None,
                SkyMode::End,
                SkyMode::Overworld
            ]
        );
    }

    /// No dimension resolved yet (pre-login, or any serverless path) must give
    /// exactly the pre-M16 inputs: attribute defaults, day timeline on.
    #[test]
    fn unresolved_dimension_is_the_pre_m16_default() {
        let d = dimension_light(None);
        assert_eq!(d, DimensionLight::UNRESOLVED);
        assert_eq!(d.ambient_color, [0.0, 0.0, 0.0]);
        assert_eq!(d.sky_light_color, [1.0, 1.0, 1.0]);
        assert_eq!(d.sky_light_factor, 1.0);
        assert!(d.day_timeline);
        assert_eq!(sky_mode_of(None), SkyMode::Overworld);
        // And the resolved lightmap is unchanged from the legacy call.
        let s = resolve_lightmap(Some(18000), None, 1.4, no_effects(0), 0.5, 1.0, 0.0);
        assert_eq!(s.sky_factor, 0.24);
        assert_eq!(s.sky_light_color, [0.48, 0.48, 1.0]);
        assert_eq!(s.ambient_color, [0.0, 0.0, 0.0]);
    }

    /// The Overworld keeps the day timeline, and its ambient is the dimension
    /// attribute `#0a0a0a` — NOT the codec default black the serverless paths
    /// use. The two must be distinguishable, or the field is doing nothing.
    #[test]
    fn overworld_keeps_the_day_timeline_and_gains_its_ambient() {
        let ow = overworld();
        let d = dimension_light(Some(&ow));
        assert!(d.day_timeline);
        assert_eq!(d.ambient_color, [10.0 / 255.0; 3]);
        assert_eq!(sky_mode_of(Some(&ow)), SkyMode::Overworld);

        // Noon: the timeline multiplier is 1.0 over the 1.0 base.
        let noon = resolve_lightmap(Some(6000), Some(&ow), 1.4, no_effects(0), 0.5, 1.0, 0.0);
        assert_eq!(noon.sky_factor, 1.0);
        assert_eq!(noon.sky_light_color, [1.0, 1.0, 1.0]);
        // Midnight: 1.0 * 0.24, white * (0.48, 0.48, 1.0) — the legacy values.
        let mid = resolve_lightmap(Some(18000), Some(&ow), 1.4, no_effects(0), 0.5, 1.0, 0.0);
        assert_eq!(mid.sky_factor, 0.24);
        assert_eq!(mid.sky_light_color, [0.48, 0.48, 1.0]);
        // Ambient is constant across the cycle (no timeline track keyframes it).
        assert_eq!(noon.ambient_color, mid.ambient_color);
        assert_eq!(mid.ambient_color, [10.0 / 255.0; 3]);
        assert_ne!(mid.ambient_color, [0.0; 3]);
    }

    /// The Nether and the End are fixed-time: the Overworld day timeline must
    /// not touch them, so their exact `sky_light_factor` / `sky_light_color`
    /// attributes survive at any world-clock tick.
    #[test]
    fn fixed_time_dimensions_ignore_the_overworld_clock() {
        for (def, ambient, sky, mode) in [
            (
                nether(),
                [48.0 / 255.0, 40.0 / 255.0, 33.0 / 255.0],
                [122.0 / 255.0, 122.0 / 255.0, 1.0],
                SkyMode::None,
            ),
            (
                the_end(),
                [63.0 / 255.0, 71.0 / 255.0, 63.0 / 255.0],
                [172.0 / 255.0, 96.0 / 255.0, 205.0 / 255.0],
                SkyMode::End,
            ),
        ] {
            let d = dimension_light(Some(&def));
            assert!(!d.day_timeline, "{} must be fixed-time", def.name);
            assert_eq!(sky_mode_of(Some(&def)), mode, "{} skybox", def.name);

            // Every tick of the day, and with no clock at all, resolves the same.
            let mut seen = Vec::new();
            for t in [
                None,
                Some(0),
                Some(6000),
                Some(13000),
                Some(18000),
                Some(23999),
            ] {
                let s = resolve_lightmap(t, Some(&def), 1.4, no_effects(0), 0.5, 1.0, 0.0);
                assert_eq!(s.sky_factor, 0.0, "{} sky factor at {t:?}", def.name);
                assert_eq!(s.sky_light_color, sky, "{} sky colour at {t:?}", def.name);
                assert_eq!(s.ambient_color, ambient, "{} ambient at {t:?}", def.name);
                // The sky/fog gradient multiplier is gated on the same test, so
                // a midnight clock cannot black out the End's sky either.
                let tl = dimension_timeline(t, &d);
                assert_eq!(tl, rewo_world::daylight::SkyLighting::DAY);
                seen.push(s);
            }
            assert!(seen.windows(2).all(|w| w[0] == w[1]));
        }
    }

    /// The exact leak this guards: at midnight the Overworld resolves a dim,
    /// blue sky half and a black sky gradient. Respawning into the Nether with
    /// the SAME `day_ticks` must not carry any of that across.
    #[test]
    fn overworld_midnight_does_not_leak_across_a_respawn() {
        const MIDNIGHT: Option<i64> = Some(18000);
        let ow = overworld();
        let nether = nether();
        let before = resolve_lightmap(MIDNIGHT, Some(&ow), 1.4, no_effects(0), 0.5, 1.0, 0.0);
        let after = resolve_lightmap(MIDNIGHT, Some(&nether), 1.4, no_effects(0), 0.5, 1.0, 0.0);
        assert_eq!(
            before.sky_light_color,
            [0.48, 0.48, 1.0],
            "the OW night tint"
        );
        // The Nether's own #7a7aff, not the night-multiplied version of it.
        assert_eq!(after.sky_light_color, [122.0 / 255.0, 122.0 / 255.0, 1.0]);
        assert_ne!(after.sky_light_color, before.sky_light_color);
        assert_eq!(after.sky_factor, 0.0);
        assert_ne!(after.ambient_color, before.ambient_color);

        // The gradient tint too: midnight blacks the Overworld sky, the Nether
        // (and End) must stay at the identity multiplier.
        let ow_tl = dimension_timeline(MIDNIGHT, &dimension_light(Some(&ow)));
        assert_eq!(ow_tl.sky_color, [0.0, 0.0, 0.0], "OW midnight sky is black");
        let nether_tl = dimension_timeline(MIDNIGHT, &dimension_light(Some(&nether)));
        assert_eq!(nether_tl.sky_color, [1.0, 1.0, 1.0]);
        assert_eq!(nether_tl.fog_color, [1.0, 1.0, 1.0]);
    }

    /// The ambient survives the CPU→GPU conversion, and the conversion carries
    /// every field (a missed field would default-initialize to something bland).
    #[test]
    fn world_lightmap_conversion_carries_the_ambient() {
        let end = the_end();
        let s = resolve_lightmap(Some(18000), Some(&end), 1.7, no_effects(0), 0.5, 1.0, 0.0);
        let w = to_world_lightmap(&s);
        assert_eq!(w.ambient_color, s.ambient_color);
        assert_eq!(w.ambient_color, [63.0 / 255.0, 71.0 / 255.0, 63.0 / 255.0]);
        assert_ne!(w.ambient_color, WorldLightmapState::default().ambient_color);
    }

    /// `DimensionType.Skybox` → the renderer's mode, both ways round.
    #[test]
    fn sky_mode_maps_every_skybox() {
        assert_eq!(sky_mode_of(Some(&overworld())), SkyMode::Overworld);
        assert_eq!(sky_mode_of(Some(&nether())), SkyMode::None);
        assert_eq!(sky_mode_of(Some(&the_end())), SkyMode::End);
        // An unresolved holder degrades to the codec default, not to NONE.
        let unresolved = DimensionTypeDef::unresolved_holder(9);
        assert_eq!(unresolved.skybox, Skybox::DEFAULT);
        assert_eq!(sky_mode_of(Some(&unresolved)), SkyMode::Overworld);
    }

    #[test]
    fn validate_unit_bounds() {
        assert!(validate_unit("gamma", 0.0).is_ok());
        assert!(validate_unit("gamma", 1.0).is_ok());
        assert!(validate_unit("gamma", 0.5).is_ok());
        assert!(validate_unit("gamma", -0.01).is_err());
        assert!(validate_unit("gamma", 1.01).is_err());
        assert!(validate_unit("gamma", f32::NAN).is_err());
        assert!(validate_unit("gamma", f32::INFINITY).is_err());
    }
}
