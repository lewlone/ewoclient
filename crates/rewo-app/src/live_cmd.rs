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
use rewo_gpu::entities::{srgb_to_linear, EntityDraw, EntityModelKind, FontData, MobTexEntry, MobTextures};
use rewo_gpu::offscreen::Offscreen;
use rewo_gpu::overlay::OverlayDraw;
use rewo_gpu::renderer::{RenderOutcome, Renderer};
use rewo_gpu::world::WorldRenderer;
use rewo_gpu::Gpu;
use rewo_mesh::pool::{MeshPool, MeshTables};
use rewo_net::play::PlaySession;
use rewo_net::Connection;
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
    #[arg(long, default_value_t = false)]
    no_validation: bool,
}

pub fn run(args: LiveArgs) -> Result<(), String> {
    let data = GameData::load_for_version(&args.version)?;
    let jar = client_jar_path(&args.version).ok_or("client jar not found")?;
    let paths = DataPaths::for_version(&args.version).ok_or("no config dir")?;
    let baked = assets::bake(&jar, &paths.blocks_json())?;
    let solid: Vec<bool> = baked.solid.clone();
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
    let conn = Connection::connect(&args.host, args.port, &data)?;
    let session = conn.into_play(&args.host, args.port, &username, auth.as_ref(), solid, global_bits)?;
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
            run_headless(session, baked, etypes, want_validation, out, settle, dirt_item)
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
                log::info!("skin: uploaded for {uuid:032x} ({} model)", if slim { "slim" } else { "wide" });
            }
        }
    }
}

/// Tracks when each entity's gesture-driving state changed. The wire
/// carries only the *current* pose/state — vanilla clients time the rigs
/// from the transition instant, so we record it here.
#[derive(Default)]
pub(crate) struct GestureTracker {
    map: std::collections::HashMap<i32, (rewo_gpu::mobs::Gesture, f32)>,
}

impl GestureTracker {
    /// Record `wanted` for this entity at `now` seconds; returns the active
    /// gesture + its age. `head_start` pre-advances a *newly entered*
    /// gesture's clock (vanilla's SCARED `fastForward`).
    fn update(
        &mut self,
        id: i32,
        wanted: Option<rewo_gpu::mobs::Gesture>,
        now: f32,
        head_start: f32,
    ) -> Option<(rewo_gpu::mobs::Gesture, f32)> {
        match wanted {
            None => {
                self.map.remove(&id);
                None
            }
            Some(g) => {
                let start = match self.map.get(&id) {
                    Some((cur, t0)) if *cur == g => *t0,
                    _ => {
                        let t0 = now - head_start;
                        self.map.insert(id, (g, t0));
                        t0
                    }
                };
                Some((g, now - start))
            }
        }
    }
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
) -> Vec<EntityDraw<'a>> {
    let player_color = linear_rgb(0xE5, 0xB8, 0xC5); // accent rose
    let mob_color = linear_rgb(0x9A, 0x80, 0x87); // text mauve
    // Headless-only verification knob: `REWO_FORCE_LIMB=swing,amount`
    // pins every player's walk pose so a still-target PNG can prove the
    // limb-swing mechanism deterministically (a live walker's phase at
    // capture time is timing-dependent). One-shot; zero-cost when unset.
    let force_limb: Option<(f32, f32)> = std::env::var("REWO_FORCE_LIMB").ok().and_then(|s| {
        let mut it = s.split(',');
        Some((it.next()?.trim().parse().ok()?, it.next()?.trim().parse().ok()?))
    });
    // Headless-only knob: `REWO_FORCE_HEAD=<degrees>` cranks every mob's head
    // yaw to body-yaw + this offset, so a PNG can prove head-look turns the
    // head independently of the body without depending on live server AI.
    let force_head: Option<f32> = std::env::var("REWO_FORCE_HEAD").ok().and_then(|s| s.trim().parse().ok());
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
        let (w, h) = if matches!(kind, EntityModelKind::Slime | EntityModelKind::MagmaCube) {
            (1.0, 1.0) // fixed medium slime/magma (real size needs metadata)
        } else {
            etypes.dimensions(e.type_id)
        };
        let (limb_swing, limb_amount) = force_limb.unwrap_or_else(|| e.limb());
        // Gesture: wire pose/state → rig, timed from the observed change.
        let gesture = force_gesture.or_else(|| {
            let wanted = wanted_gesture(
                kind,
                session.world.entities.pose(id),
                session.world.entities.gesture_state(id),
            );
            // Entering SCARED starts the peek rig at its held ball pose —
            // vanilla `fastForward(SCARED.animationDuration())` = 2.5 s.
            let head_start = if wanted == Some(rewo_gpu::mobs::Gesture::ArmadilloScared) {
                2.5
            } else {
                0.0
            };
            gestures.update(id, wanted, now, head_start)
        });
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
            shell,
            skin_uv: player_skin.map(|ps| ps.uv),
        });
    }
    // Drop tracker entries for despawned entities (recycled server ids
    // must not inherit a stale gesture clock).
    gestures.map.retain(|id, _| session.world.entities.get(*id).is_some());
    out
}

/// Borrow the baked mob-texture table into the entity pass's view type.
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

pub(crate) fn layer_animations(baked: &assets::BakedAssets) -> Vec<rewo_gpu::world::LayerAnimation> {
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
fn eye_view_proj(
    eye: Vec3,
    yaw_deg: f32,
    pitch_deg: f32,
    aspect: f32,
) -> [[f32; 4]; 4] {
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
            if !pool.submit(&session.world, cx, cz) {
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
) -> Result<(), String> {
    let _ = dirt_item;
    let mut gpu = Gpu::new(None, want_validation)?;
    let mut off = Offscreen::new(&mut gpu, 1280, 720)?;
    let mut world_renderer =
        WorldRenderer::new(&mut gpu, off.format, assets::TEX_SIZE, &baked.layers)?;
    world_renderer.init_entities(&mut gpu, font_data(&baked), entity_textures(&baked))?;
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
    while start.elapsed().as_secs_f32() < settle_seconds {
        let deadline = start + Duration::from_millis(50) * (tick as u32 + 1);
        session.tick(&idle)?;
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
    let outputs =
        rewo_mesh::pool::mesh_all(&session.world, &baked.render, &baked.models, &coords);
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
    let draws = collect_entities(&session, &etypes, 1.0, &mut gestures, 0.0, &skins);
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
            eye.x, eye.y, eye.z
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
            log::info!("live: aiming at entity ({:.1},{:.1},{:.1})", t.pos[0], t.pos[1], t.pos[2]);
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
    world_renderer.set_entities(&draws, cr, cu, start.elapsed().as_secs_f32());
    world_renderer.set_camera(eye.to_array());
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
    /// Async player-skin fetch + upload (online-mode real skins).
    skins: SkinLoader,
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
            let rdh = window.display_handle().map_err(|e| format!("dh: {e}"))?.as_raw();
            let rwh = window.window_handle().map_err(|e| format!("wh: {e}"))?.as_raw();
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
            world_renderer.init_entities(&mut gpu, font_data(&baked), entity_textures(&baked))?;
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
            WindowEvent::MouseInput { state: btn, button, .. }
                if btn == ElementState::Pressed =>
            {
                // Left-click digs the targeted block; right-click places
                // against its hit face. (Creative: dig breaks instantly.)
                if let Some(session) = self.session.as_mut() {
                    let eye = player_eye(session);
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
        if let (DeviceEvent::MouseMotion { delta }, Some(session)) =
            (&event, self.session.as_mut())
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
        self.skins.poll_uploads(&mut state.gpu, &mut state.world_renderer);

        // Entities: frame-interpolated snapshot + camera-billboarded tags.
        let alpha = (self.tick_accum / TICK_DT).clamp(0.0, 1.0);
        let anim_time = self.started.elapsed().as_secs_f32();
        let draws = collect_entities(session, &self.etypes, alpha, &mut self.gestures, anim_time, &self.skins.registry);
        let (cr, cu) = camera_basis(session.player.yaw, session.player.pitch);
        state.world_renderer.set_entities(&draws, cr, cu, anim_time);
        drop(draws);

        let extent = state.renderer.swapchain.extent;
        let aspect = extent.width.max(1) as f32 / extent.height.max(1) as f32;
        let eye = player_eye(session);
        // Targeted block for the selection outline.
        let hit = session.target_block(
        eye_f64(session),
            look_dir(session.player.yaw, session.player.pitch),
            REACH,
        );
        state.world_renderer.set_selection(hit.map(|h| h.block));
        state.world_renderer.set_camera(eye.to_array());
        state
            .world_renderer
            .set_hud(session.health, session.food, self.hotbar_slot);
        let px = gui_px(extent.width, extent.height);
        let fps = (!self.cpu.is_empty()).then(|| 1000.0 / self.cpu.average().max(0.001));
        state
            .world_renderer
            .set_text(build_text(session, px, extent.height as f32, fps, self.debug));
        if let Err(e) = state.world_renderer.anim_tick(&mut state.gpu, session.ticks) {
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
        skins: SkinLoader::new(),
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
            format!("Block: {bx} {by} {bz}   [{rbx} {} {rbz}]", by.rem_euclid(16)),
            format!("Chunk: {cx} {cz}"),
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
    ((h as f32 / 240.0).min(w as f32 / 320.0)).floor().clamp(1.0, 4.0)
}

/// Vanilla F3's "Towards …" axis hint — the dominant world axis of the look
/// direction (used alongside the compass name).
fn facing_axis(yaw_deg: f32, pitch_deg: f32) -> &'static str {
    let d = look_dir(yaw_deg, pitch_deg);
    if d[0].abs() > d[2].abs() {
        if d[0] > 0.0 { "(Towards +X)" } else { "(Towards -X)" }
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
