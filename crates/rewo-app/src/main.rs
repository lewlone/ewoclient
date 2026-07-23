//! rewo — M0 skeleton of the Rewo native Minecraft client.
//!
//! Windowed: winit + ash swapchain, clear to Velvet wine, frame-time strip
//! chart, GPU timestamps, tracy zones. `--run-seconds N` auto-exits and
//! prints a stats block (the headless-verification soak).
//!
//! Headless: `--headless N` renders N frames offscreen (no window at all)
//! and writes a PNG — the self-check harness for machines/agents.

mod bench_cmd;
mod demo_cmd;
mod mobshot_cmd;
mod live_cmd;
mod net_cmd;
mod play_cmd;
mod skin_fetch;
mod skyshot_cmd;
mod stats;
mod view_cmd;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use ash::vk;
use clap::{Parser, Subcommand};
use raw_window_handle::{HasDisplayHandle, HasWindowHandle};
use rewo_gpu::overlay::OverlayDraw;
use rewo_gpu::renderer::{RenderOutcome, Renderer};
use rewo_gpu::{offscreen::Offscreen, Gpu};
use stats::{OverlayRing, StatsAccum};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};

/// Velvet wine `#0A0006`, converted sRGB → linear for the SRGB attachment.
const CLEAR_WINE: [f32; 4] = [0.003_035, 0.0, 0.001_821, 1.0];

#[derive(Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
enum PresentPref {
    Mailbox,
    Immediate,
    Fifo,
}

impl PresentPref {
    fn to_vk(self) -> vk::PresentModeKHR {
        match self {
            PresentPref::Mailbox => vk::PresentModeKHR::MAILBOX,
            PresentPref::Immediate => vk::PresentModeKHR::IMMEDIATE,
            PresentPref::Fifo => vk::PresentModeKHR::FIFO,
        }
    }
}

#[derive(Parser)]
#[command(name = "rewo", about = "Rewo native client — M0 renderer + M1 protocol")]
struct Args {
    #[command(subcommand)]
    command: Option<Command>,

    /// Render N frames offscreen (no window) and write a PNG.
    #[arg(long)]
    headless: Option<u32>,

    /// Windowed: exit automatically after this many seconds.
    #[arg(long)]
    run_seconds: Option<f32>,

    /// Present mode preference (falls back MAILBOX → IMMEDIATE → FIFO).
    #[arg(long, value_enum, default_value = "mailbox")]
    present: PresentPref,

    /// Output path for the headless PNG.
    #[arg(long, default_value = "rewo-m0.png")]
    out: PathBuf,

    /// Frame time (ms) mapped to full chart height.
    #[arg(long, default_value_t = 20.0)]
    scale_ms: f32,

    /// Fill the chart with a deterministic animated test pattern instead of
    /// real frame times (exercises all bar colors in the headless PNG).
    #[arg(long, default_value_t = false)]
    chart_demo: bool,

    /// Disable validation layers even in debug builds.
    #[arg(long, default_value_t = false)]
    no_validation: bool,
}

impl Args {
    fn want_validation(&self) -> bool {
        cfg!(debug_assertions) && !self.no_validation
    }
}

#[derive(Subcommand)]
enum Command {
    /// M1 protocol: connect to a server, soak, and optionally record.
    Net(net_cmd::NetArgs),
    /// M2 first pixels: snapshot a world and view it (headless or windowed).
    View(view_cmd::ViewArgs),
    /// M3 be a player: headless bot — spawn, move, build, chat; report
    /// server position corrections (the physics-parity meter).
    Play(play_cmd::PlayArgs),
    /// M3 capstone: connect + play in a real window (WASD/mouse), the live
    /// session feeding the renderer. `--out` writes the eye view headless.
    Live(live_cmd::LiveArgs),
    /// M4 showcase: render a synthetic scene of varied block models to a PNG
    /// (no server) — the model/AO/tint verification artifact.
    Demo(demo_cmd::DemoArgs),
    /// M6 benchmark: deterministic headless render of a replay world from an
    /// orbit camera; reports frame-time 1%/0.1% lows (the merge-gate metric).
    Bench(bench_cmd::BenchArgs),
    /// Mob-model verification: contact sheet of every mob (no server), or
    /// `--check` for the facelabel texture-correspondence gate.
    Mobshot(mobshot_cmd::MobshotArgs),
    /// M12 sky verification: render sun/moon/stars/sunrise + the zenith tint
    /// headless (no server) and assert their pixel properties with `--check`.
    Skyshot(skyshot_cmd::SkyshotArgs),
}

fn main() {
    let mut args = Args::parse();
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_millis()
        .init();
    let tracy = tracy_client::Client::start();

    let result = match args.command.take() {
        Some(Command::Net(net_args)) => net_cmd::run(net_args),
        Some(Command::View(view_args)) => view_cmd::run(view_args),
        Some(Command::Play(play_args)) => play_cmd::run(play_args),
        Some(Command::Live(live_args)) => live_cmd::run(live_args),
        Some(Command::Demo(demo_args)) => demo_cmd::run(demo_args),
        Some(Command::Bench(bench_args)) => bench_cmd::run(bench_args),
        Some(Command::Mobshot(mobshot_args)) => mobshot_cmd::run(mobshot_args),
        Some(Command::Skyshot(skyshot_args)) => skyshot_cmd::run(skyshot_args),
        None => match args.headless {
            Some(frames) => run_headless(&args, frames),
            None => run_windowed(args, tracy),
        },
    };
    if let Err(e) = result {
        log::error!("rewo: {e}");
        std::process::exit(1);
    }
}

// ---------------------------------------------------------------- headless

fn run_headless(args: &Args, frames: u32) -> Result<(), String> {
    log::info!("rewo-m0: headless run, {frames} frames");
    let mut gpu = Gpu::new(None, args.want_validation())?;
    let mut off = Offscreen::new(&mut gpu, 1280, 720)?;

    let mut ring = OverlayRing::default();
    let mut cpu = StatsAccum::default();
    let mut gpu_ms = StatsAccum::default();
    let start = Instant::now();
    let mut last = start;

    for _ in 0..frames.max(1) {
        let now = Instant::now();
        let dt_ms = now.duration_since(last).as_secs_f32() * 1000.0;
        last = now;
        cpu.push(dt_ms);
        if args.chart_demo {
            ring.fill_demo(start.elapsed().as_secs_f32());
        } else {
            ring.push(dt_ms);
        }
        let draw = OverlayDraw {
            samples_ms: &ring.data,
            head: ring.head(),
            scale_ms: args.scale_ms,
            origin: [16.0, 16.0],
            size: [560.0, 140.0],
        };
        off.render(&gpu, None, &draw, CLEAR_WINE)?;
        if let Some(g) = off.last_gpu_ms {
            gpu_ms.push(g);
        }
    }

    off.save_png(&gpu, &args.out)?;
    log::info!("rewo-m0: wrote {}", args.out.display());
    print_summary(
        "headless",
        &gpu.device_name,
        None,
        start.elapsed().as_secs_f32(),
        &cpu,
        &gpu_ms,
    );
    off.destroy(&mut gpu);
    Ok(())
}

// ---------------------------------------------------------------- windowed

struct State {
    window: Arc<Window>,
    gpu: Gpu,
    renderer: Renderer,
}

struct WinApp {
    args: Args,
    tracy: tracy_client::Client,
    state: Option<State>,
    ring: OverlayRing,
    cpu: StatsAccum,
    gpu_ms: StatsAccum,
    started: Instant,
    last_frame: Option<Instant>,
    init_error: Option<String>,
}

impl WinApp {
    fn frame(&mut self, event_loop: &ActiveEventLoop) {
        let Some(state) = self.state.as_mut() else {
            return;
        };
        let _span = tracy_client::span!("frame");

        let now = Instant::now();
        if let Some(last) = self.last_frame {
            let dt_ms = now.duration_since(last).as_secs_f32() * 1000.0;
            self.cpu.push(dt_ms);
            if self.args.chart_demo {
                self.ring.fill_demo(self.started.elapsed().as_secs_f32());
            } else {
                self.ring.push(dt_ms);
            }
        }
        self.last_frame = Some(now);

        let draw = OverlayDraw {
            samples_ms: &self.ring.data,
            head: self.ring.head(),
            scale_ms: self.args.scale_ms,
            origin: [16.0, 16.0],
            size: [560.0, 140.0],
        };
        let State { window, gpu, renderer } = state;
        match renderer.render(gpu, None, &draw, CLEAR_WINE) {
            Ok(RenderOutcome::Rendered) | Ok(RenderOutcome::Skipped) => {}
            Ok(RenderOutcome::NeedsRecreate) => {
                let size = window.inner_size();
                if let Err(e) = renderer.recreate(gpu, size.width, size.height) {
                    log::error!("rewo: swapchain recreate failed: {e}");
                    event_loop.exit();
                }
            }
            Err(e) => {
                log::error!("rewo: render failed: {e}");
                event_loop.exit();
            }
        }
        if let Some(g) = renderer.last_gpu_ms {
            self.gpu_ms.push(g);
        }

        self.tracy.frame_mark();
        if let Some(limit) = self.args.run_seconds {
            if self.started.elapsed().as_secs_f32() >= limit {
                log::info!("rewo-m0: --run-seconds {limit} reached, exiting");
                event_loop.exit();
            }
        }
    }
}

impl ApplicationHandler for WinApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title("Rewo · M0 skeleton")
            .with_inner_size(LogicalSize::new(1280.0, 720.0));
        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                self.init_error = Some(format!("create window: {e}"));
                event_loop.exit();
                return;
            }
        };
        let init = (|| -> Result<State, String> {
            let rdh = window
                .display_handle()
                .map_err(|e| format!("display handle: {e}"))?
                .as_raw();
            let rwh = window
                .window_handle()
                .map_err(|e| format!("window handle: {e}"))?
                .as_raw();
            let mut gpu = Gpu::new(Some((rdh, rwh)), self.args.want_validation())?;
            let size = window.inner_size();
            let renderer = Renderer::new(
                &mut gpu,
                size.width.max(1),
                size.height.max(1),
                self.args.present.to_vk(),
            )?;
            Ok(State {
                window: window.clone(),
                gpu,
                renderer,
            })
        })();
        match init {
            Ok(state) => {
                self.started = Instant::now();
                self.state = Some(state);
            }
            Err(e) => {
                self.init_error = Some(e);
                event_loop.exit();
            }
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(State { gpu, renderer, .. }) = self.state.as_mut() {
                    if size.width > 0 && size.height > 0 {
                        if let Err(e) = renderer.resize(gpu, size.width, size.height) {
                            log::error!("rewo: resize failed: {e}");
                        }
                    }
                }
            }
            WindowEvent::RedrawRequested => self.frame(event_loop),
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(state) = &self.state {
            state.window.request_redraw();
        }
    }
}

fn run_windowed(args: Args, tracy: tracy_client::Client) -> Result<(), String> {
    let event_loop = EventLoop::new().map_err(|e| format!("event loop: {e}"))?;
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut app = WinApp {
        args,
        tracy,
        state: None,
        ring: OverlayRing::default(),
        cpu: StatsAccum::default(),
        gpu_ms: StatsAccum::default(),
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
        let present = format!("{:?}", state.renderer.present_mode());
        print_summary(
            &format!("windowed · {present}"),
            &state.gpu.device_name,
            Some(&present),
            elapsed,
            &app.cpu,
            &app.gpu_ms,
        );
        state.renderer.destroy(&mut state.gpu);
        // `state.gpu` drops here: wait_idle + device/instance teardown.
    }
    Ok(())
}

// ------------------------------------------------------------------ stats

fn print_summary(
    label: &str,
    adapter: &str,
    present: Option<&str>,
    elapsed: f32,
    cpu: &StatsAccum,
    gpu: &StatsAccum,
) {
    let frames = cpu.ms.len();
    let avg_fps = if elapsed > 0.0 {
        frames as f32 / elapsed
    } else {
        0.0
    };
    println!("[rewo-m0] mode: {label}");
    println!("[rewo-m0] adapter: {adapter}");
    if let Some(p) = present {
        println!("[rewo-m0] present mode: {p}");
    }
    println!("[rewo-m0] frames: {frames}  elapsed: {elapsed:.2}s  avg fps: {avg_fps:.1}");
    println!(
        "[rewo-m0] cpu frame ms  avg {:.3}  p50 {:.3}  p99 {:.3}  p99.9 {:.3}  max {:.3}",
        cpu.average(),
        cpu.percentile(0.50),
        cpu.percentile(0.99),
        cpu.percentile(0.999),
        cpu.percentile(1.0),
    );
    if !gpu.ms.is_empty() {
        println!(
            "[rewo-m0] gpu frame ms  avg {:.3}  p99 {:.3}  max {:.3}",
            gpu.average(),
            gpu.percentile(0.99),
            gpu.percentile(1.0),
        );
    }
}
