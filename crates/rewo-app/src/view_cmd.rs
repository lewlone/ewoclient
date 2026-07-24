//! `rewo view` — M2 first pixels: snapshot the world (from a replay file or
//! a short live-server fetch), bake assets, mesh, and render.
//!
//! Headless (`--out x.png`) is the DoD artifact — a deterministic render of
//! a known scene with no window. Windowed is a WASD/mouse fly camera over
//! the same snapshot (`--run-seconds` auto-exits for soak use).
//!
//! Snapshot-only by design: live streaming-while-rendering arrives with the
//! M3 tick loop; here the world is fetched once, then viewed statically.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ash::vk;
use clap::Args as ClapArgs;
use glam::{Mat4, Vec3};
use raw_window_handle::{HasDisplayHandle, HasWindowHandle};
use rewo_data::{assets, DataPaths, GameData};
use rewo_gpu::offscreen::Offscreen;
use rewo_gpu::overlay::OverlayDraw;
use rewo_gpu::renderer::{RenderOutcome, Renderer};
use rewo_gpu::world::WorldRenderer;
use rewo_gpu::Gpu;
use rewo_net::{ids::Ids, record, Connection};
use rewo_world::World;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{DeviceEvent, DeviceId, ElementState, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{CursorGrabMode, Window, WindowId};

use crate::stats::{OverlayRing, StatsAccum};

/// Daytime sky, linear (sRGB #78A7FF-ish).
const CLEAR_SKY: [f32; 4] = [0.184, 0.380, 1.0, 1.0];

#[derive(ClapArgs)]
pub struct ViewArgs {
    /// Replay file to view (mutually exclusive with --host).
    #[arg(long)]
    replay: Option<PathBuf>,

    /// Live server to snapshot.
    #[arg(long)]
    host: Option<String>,
    #[arg(long, default_value_t = 25599)]
    port: u16,
    #[arg(long, default_value = "Rewo")]
    username: String,
    /// How long to stay connected collecting chunks before snapshotting.
    #[arg(long, default_value_t = 10.0)]
    fetch_seconds: f32,

    /// MC version whose data + client jar to use.
    #[arg(long, default_value = "26.2")]
    version: String,

    /// Headless: write a PNG here instead of opening a window.
    #[arg(long)]
    out: Option<PathBuf>,

    /// Camera position (default: hover above the world center).
    #[arg(long, num_args = 3, value_names = ["X", "Y", "Z"], allow_hyphen_values = true)]
    at: Vec<f32>,
    /// Camera yaw/pitch in degrees.
    #[arg(long, default_value_t = 45.0, allow_negative_numbers = true)]
    yaw: f32,
    #[arg(long, default_value_t = -35.0, allow_negative_numbers = true)]
    pitch: f32,

    /// Windowed: auto-exit after N seconds (soak mode).
    #[arg(long)]
    run_seconds: Option<f32>,

    #[arg(long, default_value_t = false)]
    no_validation: bool,
}

pub fn run(args: ViewArgs) -> Result<(), String> {
    // -- world snapshot ----------------------------------------------------
    let data = GameData::load_for_version(&args.version)?;
    let world = match (&args.replay, &args.host) {
        (Some(file), _) => {
            let ids = Ids::resolve(&data.packets)?;
            let (world, chunks) = record::replay(file, &data, &ids)?;
            log::info!("view: replayed {chunks} chunks from {}", file.display());
            world
        }
        (None, Some(host)) => {
            let conn = Connection::connect(host, args.port, &data)?;
            let (stats, world) = conn.run_session(
                host,
                args.port,
                &args.username,
                Duration::from_secs_f32(args.fetch_seconds),
            )?;
            log::info!(
                "view: snapshot from {host}:{} — {} chunks",
                args.port,
                stats.chunks
            );
            world
        }
        (None, None) => return Err("view: pass --replay FILE or --host HOST".into()),
    };
    if world.loaded_columns() == 0 {
        return Err("view: snapshot has no chunks".into());
    }

    // -- assets ------------------------------------------------------------
    let paths = DataPaths::for_version(&args.version).ok_or("no config dir")?;
    let client_jar = client_jar_path(&args.version).ok_or("client jar not found")?;
    let baked = assets::bake(&client_jar, &paths.blocks_json())?;
    // Bake sanity: the flat-world staples must have resolved to visible,
    // solid geometry. Not necessarily `Cube` — since M4 grass_block bakes as
    // a `Model` (cube + overlay element); `baked.solid` is the flag that
    // covers both (the same distinction that fixed the M5 collision bug).
    for name in [
        "minecraft:grass_block",
        "minecraft:dirt",
        "minecraft:bedrock",
    ] {
        if let Some(state) = data.blocks.default_state(name) {
            let visible = !matches!(
                baked.render.get(state as usize),
                None | Some(assets::RenderKind::Invisible)
            );
            let solid = baked.solid.get(state as usize).copied().unwrap_or(false);
            if !visible || !solid {
                return Err(format!(
                    "bake sanity: {name} did not bake as visible solid geometry"
                ));
            }
        }
    }

    // -- mesh --------------------------------------------------------------
    let t0 = Instant::now();
    let mut meshes = Vec::new();
    let mut total_verts = 0usize;
    for (cx, cz) in world.column_coords() {
        if let Some(mesh) = rewo_mesh::mesh_column(&world, &baked.render, &baked.models, cx, cz) {
            total_verts += mesh.vertices.len();
            meshes.push(mesh);
        }
    }
    log::info!(
        "view: meshed {} columns, {} vertices, in {:.1} ms",
        meshes.len(),
        total_verts,
        t0.elapsed().as_secs_f32() * 1000.0
    );

    // -- camera ------------------------------------------------------------
    let eye = if args.at.len() == 3 {
        Vec3::new(args.at[0], args.at[1], args.at[2])
    } else {
        default_eye(&world)
    };

    let want_validation = cfg!(debug_assertions) && !args.no_validation;
    match &args.out {
        Some(out) => render_headless(
            &baked,
            &meshes,
            eye,
            args.yaw,
            args.pitch,
            want_validation,
            out,
        ),
        None => render_windowed(baked, meshes, eye, args, want_validation),
    }
}

fn client_jar_path(version: &str) -> Option<PathBuf> {
    let mut p = dirs::config_dir()?;
    p.push("EwoClient");
    p.push("shared/versions");
    p.push(version);
    p.push(format!("{version}.jar"));
    p.exists().then_some(p)
}

/// Hover above the center of the loaded area, above the highest surface.
fn default_eye(world: &World) -> Vec3 {
    let coords = world.column_coords();
    let n = coords.len().max(1) as f32;
    let (sx, sz) = coords.iter().fold((0.0f32, 0.0f32), |(ax, az), (cx, cz)| {
        (ax + *cx as f32 * 16.0 + 8.0, az + *cz as f32 * 16.0 + 8.0)
    });
    let (cx, cz) = (sx / n, sz / n);
    let mut surface = world.shape.min_y;
    for y in (world.shape.min_y..world.shape.min_y + world.shape.height).rev() {
        if world.block_state_at(cx as i32, y, cz as i32) != 0 {
            surface = y;
            break;
        }
    }
    Vec3::new(cx, surface as f32 + 24.0, cz)
}

fn view_proj(eye: Vec3, yaw_deg: f32, pitch_deg: f32, aspect: f32) -> [[f32; 4]; 4] {
    let yaw = yaw_deg.to_radians();
    let pitch = pitch_deg.to_radians();
    let dir = Vec3::new(
        yaw.sin() * pitch.cos(),
        pitch.sin(),
        -yaw.cos() * pitch.cos(),
    );
    let view = Mat4::look_to_rh(eye, dir, Vec3::Y);
    let proj = Mat4::from_cols_array_2d(&rewo_gpu::world::perspective_reverse_z(
        70f32.to_radians(),
        aspect,
        0.05,
    ));
    (proj * view).to_cols_array_2d()
}

fn upload_all(
    gpu: &mut Gpu,
    world_renderer: &mut WorldRenderer,
    meshes: &[rewo_mesh::ColumnMesh],
) -> Result<(), String> {
    for mesh in meshes {
        world_renderer.upload_column(
            gpu,
            mesh.cx,
            mesh.cz,
            bytemuck::cast_slice(&mesh.vertices),
            &mesh.indices,
            bytemuck::cast_slice(&mesh.tvertices),
            &mesh.tindices,
            mesh.y_min,
            mesh.y_max,
        )?;
    }
    Ok(())
}

// -- headless ---------------------------------------------------------------

fn render_headless(
    baked: &assets::BakedAssets,
    meshes: &[rewo_mesh::ColumnMesh],
    eye: Vec3,
    yaw: f32,
    pitch: f32,
    want_validation: bool,
    out: &std::path::Path,
) -> Result<(), String> {
    let mut gpu = Gpu::new(None, want_validation)?;
    let mut off = Offscreen::new(&mut gpu, 1280, 720)?;
    let mut world_renderer =
        WorldRenderer::new(&mut gpu, off.format, assets::TEX_SIZE, &baked.layers)?;
    upload_all(&mut gpu, &mut world_renderer, meshes)?;
    world_renderer.set_camera(eye.to_array());

    let vp = view_proj(eye, yaw, pitch, 1280.0 / 720.0);
    let ring = OverlayRing::default();
    let draw = OverlayDraw {
        samples_ms: &ring.data,
        head: ring.head(),
        scale_ms: 20.0,
        origin: [16.0, 16.0],
        size: [560.0, 140.0],
    };
    // A couple of warm frames, then save.
    for _ in 0..3 {
        off.render(&gpu, Some((&mut world_renderer, vp)), &draw, CLEAR_SKY)?;
    }
    off.save_png(&gpu, out)?;
    println!(
        "[rewo-m2] headless view: {} columns drawn, {} culled, gpu {:.2} ms",
        world_renderer.drawn_last_frame,
        world_renderer.culled_last_frame,
        off.last_gpu_ms.unwrap_or(0.0)
    );
    println!("[rewo-m2] wrote {}", out.display());
    world_renderer.destroy(&mut gpu);
    off.destroy(&mut gpu);
    Ok(())
}

// -- windowed ---------------------------------------------------------------

struct FlyCamera {
    pos: Vec3,
    yaw: f32,
    pitch: f32,
    forward: bool,
    back: bool,
    left: bool,
    right: bool,
    up: bool,
    down: bool,
    fast: bool,
}

impl FlyCamera {
    fn tick(&mut self, dt: f32) {
        let speed = if self.fast { 60.0 } else { 20.0 } * dt;
        let yaw = self.yaw.to_radians();
        let fwd = Vec3::new(yaw.sin(), 0.0, -yaw.cos());
        let right = Vec3::new(-fwd.z, 0.0, fwd.x);
        let mut delta = Vec3::ZERO;
        if self.forward {
            delta += fwd;
        }
        if self.back {
            delta -= fwd;
        }
        if self.right {
            delta += right;
        }
        if self.left {
            delta -= right;
        }
        if self.up {
            delta += Vec3::Y;
        }
        if self.down {
            delta -= Vec3::Y;
        }
        self.pos += delta * speed;
    }
}

struct ViewState {
    window: Arc<Window>,
    gpu: Gpu,
    renderer: Renderer,
    world_renderer: WorldRenderer,
}

struct ViewApp {
    baked: Option<assets::BakedAssets>,
    meshes: Vec<rewo_mesh::ColumnMesh>,
    camera: FlyCamera,
    run_seconds: Option<f32>,
    want_validation: bool,
    state: Option<ViewState>,
    ring: OverlayRing,
    cpu: StatsAccum,
    started: Instant,
    last_frame: Option<Instant>,
    init_error: Option<String>,
}

impl ApplicationHandler for ViewApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title("Rewo · M2 first pixels")
            .with_inner_size(LogicalSize::new(1280.0, 720.0));
        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                self.init_error = Some(format!("create window: {e}"));
                event_loop.exit();
                return;
            }
        };
        let init = (|| -> Result<ViewState, String> {
            let rdh = window
                .display_handle()
                .map_err(|e| format!("display handle: {e}"))?
                .as_raw();
            let rwh = window
                .window_handle()
                .map_err(|e| format!("window handle: {e}"))?
                .as_raw();
            let mut gpu = Gpu::new(Some((rdh, rwh)), self.want_validation)?;
            let size = window.inner_size();
            let mut renderer = Renderer::new(
                &mut gpu,
                size.width.max(1),
                size.height.max(1),
                vk::PresentModeKHR::MAILBOX,
            )?;
            renderer.ensure_depth(&mut gpu)?;
            let baked = self.baked.take().ok_or("assets consumed")?;
            let mut world_renderer = WorldRenderer::new(
                &mut gpu,
                renderer.swapchain.format,
                assets::TEX_SIZE,
                &baked.layers,
            )?;
            upload_all(&mut gpu, &mut world_renderer, &self.meshes)?;
            Ok(ViewState {
                window: window.clone(),
                gpu,
                renderer,
                world_renderer,
            })
        })();
        match init {
            Ok(state) => {
                let _ = state
                    .window
                    .set_cursor_grab(CursorGrabMode::Confined)
                    .map(|()| state.window.set_cursor_visible(false));
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
                if let Some(ViewState { gpu, renderer, .. }) = self.state.as_mut() {
                    if size.width > 0 && size.height > 0 {
                        if let Err(e) = renderer.resize(gpu, size.width, size.height) {
                            log::error!("view: resize failed: {e}");
                        }
                        if let Err(e) = renderer.ensure_depth(gpu) {
                            log::error!("view: depth recreate failed: {e}");
                        }
                    }
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                let pressed = event.state == ElementState::Pressed;
                match event.physical_key {
                    PhysicalKey::Code(KeyCode::KeyW) => self.camera.forward = pressed,
                    PhysicalKey::Code(KeyCode::KeyS) => self.camera.back = pressed,
                    PhysicalKey::Code(KeyCode::KeyA) => self.camera.left = pressed,
                    PhysicalKey::Code(KeyCode::KeyD) => self.camera.right = pressed,
                    PhysicalKey::Code(KeyCode::Space) => self.camera.up = pressed,
                    PhysicalKey::Code(KeyCode::ControlLeft) => self.camera.down = pressed,
                    PhysicalKey::Code(KeyCode::ShiftLeft) => self.camera.fast = pressed,
                    PhysicalKey::Code(KeyCode::Escape) => event_loop.exit(),
                    _ => {}
                }
            }
            WindowEvent::RedrawRequested => self.frame(event_loop),
            _ => {}
        }
    }

    fn device_event(&mut self, _el: &ActiveEventLoop, _id: DeviceId, event: DeviceEvent) {
        if let DeviceEvent::MouseMotion { delta } = event {
            self.camera.yaw += delta.0 as f32 * 0.15;
            self.camera.pitch = (self.camera.pitch - delta.1 as f32 * 0.15).clamp(-89.0, 89.0);
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(state) = &self.state {
            state.window.request_redraw();
        }
    }
}

impl ViewApp {
    fn frame(&mut self, event_loop: &ActiveEventLoop) {
        let Some(state) = self.state.as_mut() else {
            return;
        };
        let now = Instant::now();
        let dt = self
            .last_frame
            .map(|l| now.duration_since(l).as_secs_f32())
            .unwrap_or(0.0);
        self.last_frame = Some(now);
        if dt > 0.0 && dt < 1.0 {
            self.cpu.push(dt * 1000.0);
            self.ring.push(dt * 1000.0);
        }
        self.camera.tick(dt.min(0.1));

        let extent = state.renderer.swapchain.extent;
        let aspect = extent.width.max(1) as f32 / extent.height.max(1) as f32;
        state.world_renderer.set_camera(self.camera.pos.to_array());
        let vp = view_proj(self.camera.pos, self.camera.yaw, self.camera.pitch, aspect);
        let draw = OverlayDraw {
            samples_ms: &self.ring.data,
            head: self.ring.head(),
            scale_ms: 20.0,
            origin: [16.0, 16.0],
            size: [560.0, 140.0],
        };
        let ViewState {
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
                log::error!("view: render failed: {e}");
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

fn render_windowed(
    baked: assets::BakedAssets,
    meshes: Vec<rewo_mesh::ColumnMesh>,
    eye: Vec3,
    args: ViewArgs,
    want_validation: bool,
) -> Result<(), String> {
    let event_loop = EventLoop::new().map_err(|e| format!("event loop: {e}"))?;
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut app = ViewApp {
        baked: Some(baked),
        meshes,
        camera: FlyCamera {
            pos: eye,
            yaw: args.yaw,
            pitch: args.pitch,
            forward: false,
            back: false,
            left: false,
            right: false,
            up: false,
            down: false,
            fast: false,
        },
        run_seconds: args.run_seconds,
        want_validation,
        state: None,
        ring: OverlayRing::default(),
        cpu: StatsAccum::default(),
        started: Instant::now(),
        last_frame: None,
        init_error: None,
    };
    event_loop
        .run_app(&mut app)
        .map_err(|e| format!("event loop run: {e}"))?;
    if let Some(e) = app.init_error.take() {
        return Err(e);
    }
    let elapsed = app.started.elapsed().as_secs_f32();
    if let Some(mut state) = app.state.take() {
        println!(
            "[rewo-m2] windowed view: {:.1}s, {} frames, avg fps {:.0}, cpu p99 {:.2} ms, max {:.2} ms, drawn {}, culled {}",
            elapsed,
            app.cpu.ms.len(),
            app.cpu.ms.len() as f32 / elapsed.max(0.001),
            app.cpu.percentile(0.99),
            app.cpu.percentile(1.0),
            state.world_renderer.drawn_last_frame,
            state.world_renderer.culled_last_frame,
        );
        state.world_renderer.destroy(&mut state.gpu);
        state.renderer.destroy(&mut state.gpu);
    }
    Ok(())
}
