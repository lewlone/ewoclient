//! rewo — M0 skeleton of the Rewo native Minecraft client.
//!
//! Windowed: winit + ash swapchain, clear to Velvet wine, frame-time strip
//! chart, GPU timestamps, tracy zones. `--run-seconds N` auto-exits and
//! prints a stats block (the headless-verification soak).
//!
//! Headless: `--headless N` renders N frames offscreen (no window at all)
//! and writes a PNG — the self-check harness for machines/agents.

mod abilityshot_cmd;
mod audio_backend;
mod bordershot_cmd;
mod capture;
mod captureshot_cmd;
mod bench_cmd;
mod attributeshot_cmd;
mod danceshot_cmd;
mod deathshot_cmd;
mod serverlinkshot_cmd;
mod rideshot_cmd;
mod healthbarshot_cmd;
mod breakshot_cmd;
mod hurtshot_cmd;
mod labelshot_cmd;
mod itemshot_cmd;
mod demo_cmd;
mod dimension_check;
mod dimension_json;
mod dimensioncheck_cmd;
mod blockentityshot_cmd;
mod eventshot_cmd;
mod lightmapshot_cmd;
mod meshshot_cmd;
mod tintshot_cmd;
mod locatorshot_cmd;
mod sidebarshot_cmd;
mod titleshot_cmd;
mod bookshot_cmd;
mod gaugeshot_cmd;
mod leashshot_cmd;
mod mobshot_cmd;
mod mobtexshot_cmd;
mod modules;
mod live_cmd;
mod net_cmd;
mod play_cmd;
mod capeshot_cmd;
mod handshot_cmd;
mod containershot_cmd;
mod inventoryshot_cmd;
mod particleshot_cmd;
mod portalshot_cmd;
mod skin_fetch;
mod uri_open;
mod skyshot_cmd;
mod soundshot_cmd;
mod stats;
mod witness_names;
mod stats_view;
mod statshot_cmd;
mod swingshot_cmd;
mod tab_list_view;
mod tablistshot_cmd;
mod view_cmd;
mod hudshot_cmd;
mod weathershot_cmd;

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
    /// Real-texture, multi-entity mob gate: many mobs in ONE `set_entities`,
    /// each rendered pixel checked against the colours its own jar sheet can
    /// produce. The complement of `mobshot --check`, which substitutes debug
    /// colours and renders one entity per frame.
    Mobtexshot(mobtexshot_cmd::MobtexshotArgs),
    /// M12 sky verification: render sun/moon/stars/sunrise + the zenith tint
    /// headless (no server) and assert their pixel properties with `--check`.
    Skyshot(skyshot_cmd::SkyshotArgs),
    /// M13 lightmap verification: render terrain, water and entity cases
    /// through the production Vulkan paths (no server) and assert their pixel
    /// properties against independent CPU expectations with `--check`.
    Lightmapshot(lightmapshot_cmd::LightmapshotArgs),
    /// M14 biome-tint verification: build a deterministic multi-biome scene,
    /// mesh it through the production `mesh_column`, render terrain + sky/fog
    /// through Vulkan (no server), and assert grass/foliage/water tint + sky/fog
    /// against independent expectations with `--check`.
    Tintshot(tintshot_cmd::TintshotArgs),
    /// M15 geometry oracle: compare production greedy rectangles with the
    /// frozen unit-face reference and pin every merge boundary (no server).
    Meshshot(meshshot_cmd::MeshshotArgs),
    /// M16 dimension oracle: grade a captured 26.2 `dimension_type` registry
    /// against the bundled built-ins and the decompiled JSON, and prove every
    /// entry binds to the world shape, sky channel and mesh shade (no server).
    Dimensioncheck(dimensioncheck_cmd::DimensioncheckArgs),
    /// M17 entity-event oracle: drive raw `ClientboundEntityEventPacket` bodies
    /// through the real dispatch → receipt-tick → `resolve_mob_anim` → rig-oracle
    /// path and assert the warden attack/sonic and armadillo peek animations
    /// against independent decompiled literals with `--check` (no server, no GPU).
    Eventshot(eventshot_cmd::EventshotArgs),
    /// M75 abilities oracle: drive raw `ClientboundPlayerAbilitiesPacket` bodies
    /// and `CommonPlayerSpawnInfo` gamemode fields through the real decoders,
    /// the real `GameType.updatePlayerAbilities` binding, the real
    /// `LocalPlayer.aiStep` flight controller and the real `physics::tick_with`,
    /// and assert the flags byte, the flight constants, the double-tap window
    /// and the mode transitions with `--check` (no server, no GPU).
    Abilityshot(abilityshot_cmd::AbilityshotArgs),
    /// M79 title-overlay + HUD-gauge oracle: drive raw `set_title_text`,
    /// `set_subtitle_text`, `set_action_bar_text`, `set_titles_animation`,
    /// `clear_titles`, `set_experience` and `cooldown` bodies through the real
    /// router and the real line builders, then render the result offscreen and
    /// assert the pixels against a synthetic magenta subject with `--check`
    /// (no server; Vulkan required).
    Titleshot(titleshot_cmd::TitleshotArgs),
    /// M168 survival-HUD oracle: drive raw `set_entity_data`,
    /// `update_mob_effect`, `update_attributes` and `set_health`-shaped
    /// inputs through the real decoders, the real `LocalPlayer.hurtTo` /
    /// `Hud` blink clock and the SAME `survival_inputs_from` the frame calls,
    /// grade `survival_hud::layout` against literals transcribed from
    /// `Hud.java`, then render hearts, armour, food, air, vehicle hearts,
    /// effect icons and the jump bar offscreen and assert the pixels against
    /// the jar's own sprite bytes with `--check` (no server; Vulkan required).
    /// The written-book reader gate (M172).
    Bookshot(bookshot_cmd::BookshotArgs),
    Gaugeshot(gaugeshot_cmd::GaugeshotArgs),
    /// The leash rope gate (M170).
    Leashshot(leashshot_cmd::LeashshotArgs),
    /// M132 scoreboard-sidebar oracle: drive raw `set_objective`, `set_score`
    /// and `set_display_objective` bodies through the real parsers and the
    /// real `Scoreboard`, resolve the panel with the SAME `resolve_sidebar`
    /// the windowed frame calls, then render its fills and text offscreen and
    /// assert the geometry against literals transcribed from
    /// `displayScoreboardSidebar` with `--check` (no server; Vulkan required).
    Sidebarshot(sidebarshot_cmd::SidebarshotArgs),
    /// M151 tab-list oracle: rows, bands, ping icons, header/footer, score column.
    Tablistshot(tablistshot_cmd::TablistshotArgs),
    /// M83's locator-bar oracle: the `waypoint` packet and the HUD strip.
    Locatorshot(locatorshot_cmd::LocatorshotArgs),
    /// M82 screen-framework + death-screen oracle: drive a raw
    /// `player_combat_kill` body through the real router with **no entity
    /// table at all**, grade the widget model's hit rects, focus, sprite table
    /// and one-second guard against the decompile, then render the screen
    /// offscreen over a pure-green clear and assert the pixels with `--check`
    /// (no server; Vulkan required).
    Deathshot(deathshot_cmd::DeathshotArgs),
    /// M84: the statistics screen + `award_stats` oracle.
    Statshot(statshot_cmd::StatshotArgs),
    /// M85 `server_links` + pause/disconnect-screen oracle: drive a raw
    /// `server_links` body through the real `route_session`, grade the
    /// `Either` flag, the `ByIdMap.ZERO` enum and the per-entry URL filter,
    /// then the three screens' layouts against the decompile, then render them
    /// offscreen over a pure-magenta clear and assert the nine-slice, the
    /// tiled menu background and the reserved cells with `--check` (no server;
    /// Vulkan required).
    Serverlinkshot(serverlinkshot_cmd::ServerLinkshotArgs),
    /// M25 block-entity oracle: drive a synthesised level-chunk payload and a
    /// `block_entity_data` body through the real decoders, prove the fail-closed
    /// type registry, and re-measure the invisible-block gap from the client
    /// jar's own model parent chains with `--check` (no server, no GPU).
    Blockentityshot(blockentityshot_cmd::BlockentityshotArgs),
    /// M32b end-portal shader oracle: render the production end-portal /
    /// gateway pass offscreen through Vulkan (no server) and assert its pixels
    /// against an independent CPU prediction with `--check` — the analytic
    /// uniform-texture sum, screen-space sampling, and the column-major layer
    /// matrix.
    Portalshot(portalshot_cmd::PortalshotArgs),
    /// M33 weather + cloud oracle: grade the `game_event` wire, the
    /// precipitation rule and the cloud mesh on the CPU, then render both
    /// production passes offscreen and assert their pixels with `--check`.
    Weathershot(weathershot_cmd::WeathershotArgs),
    /// M52b Velvet UI oracle: grade the HUD layout chain, anchors, glyph
    /// metrics and the in-world shadow stack against the transcribed
    /// `ewo-jni/src/hud.rs` constants. CPU-only, serverless.
    Hudshot(hudshot_cmd::HudshotArgs),
    /// M34 inventory + hotbar-icon oracle: drive the three inventory packets
    /// through the real router, grade the `display.gui` placement and the GUI
    /// diffuse on the CPU, then render real baked items into real hotbar slots
    /// offscreen and assert their pixels with `--check` (no server).
    Inventoryshot(inventoryshot_cmd::InventoryshotArgs),
    /// M87 — the container-screen gate.
    Containershot(containershot_cmd::ContainershotArgs),
    /// M38 first-person hand oracle: grade the two first-person display
    /// transforms on the real jar, the pose chain against a derivation from the
    /// decompile, and the pass's pixels — with a synthetic magenta texture, so
    /// the detector cannot match anything but the hand.
    Handshot(handshot_cmd::HandshotArgs),
    /// M60 vanilla-cape oracle: the cube and its 64x32 UV space, the
    /// `Rx·Rz·Ry` composition the `PartPose` cancels into, the lagging cloak
    /// anchor and the three angles, `CapeLayer`'s four gates against real jar
    /// equipment, and the pass's pixels — with a marker-coloured cape, so the
    /// detector cannot match the player it hangs on.
    Capeshot(capeshot_cmd::CapeshotArgs),
    /// M18 Allay-dance oracle: drive raw `set_entity_data` bodies through the
    /// real packet routing → kind-aware DANCING/BABY disambiguation → client
    /// counter lifecycle → `AllayRoot`/`AllayHead` pose oracle, asserting the
    /// dance transforms against independent decompiled formulas with `--check`
    /// (no server, no GPU).
    Danceshot(danceshot_cmd::DanceshotArgs),
    /// M72 passenger-positioning oracle: drive raw `set_passengers` bodies
    /// through the real router → the riding graph → `tick_lerp`'s
    /// `positionRider` derivation, asserting every rider's position **relative
    /// to its vehicle** across sub-tick fractions and through every
    /// per-vehicle override, with `--check` (no server, no GPU).
    Rideshot(rideshot_cmd::RideshotArgs),
    /// M52 entity-attribute oracle: drive raw `update_attributes` bodies
    /// through the real packet routing → `handleUpdateAttributes` receipt gates
    /// → `AttributeInstance.calculateValue` → `RangedAttribute.sanitizeValue`,
    /// asserting the wire encoding, the operation order, the clamp and the
    /// fail-closed default resolution against independent decompiled literals
    /// with `--check` (no server, no GPU).
    Attributeshot(attributeshot_cmd::AttributeshotArgs),
    /// M59: the floating health bar. The first Rewo feature with **no vanilla
    /// oracle** — vanilla renders no health bar over any entity — so this gate
    /// grades the render against `REWO_HEALTH_BAR_SPEC.md`, a written design
    /// decision, rather than against a decompile reading.
    Healthbarshot(healthbarshot_cmd::HealthbarshotArgs),
    /// M70: entity-label visibility — the one predicate that decides whether a
    /// nametag or a health bar is drawn at all. The renderer ladder, the sneak
    /// cut-off, invisibility, the camera entity, `isVehicle`, F1 and the four
    /// `Team.Visibility` arms, plus the property that both labels agree.
    Labelshot(labelshot_cmd::LabelshotArgs),
    /// M21: the combat damage response — hurt clock + the red flash. M81
    /// adds `hurt_animation` and the camera tilt it steers.
    Hurtshot(hurtshot_cmd::HurtshotArgs),
    /// M81: the block-break crack overlay — `block_destruction`'s two
    /// indexes, the block's own decal geometry, and the multiply blend.
    Breakshot(breakshot_cmd::BreakshotArgs),
    /// M22: held items — both geometry paths, placement and suppression.
    Itemshot(itemshot_cmd::ItemshotArgs),
    /// M19 combat-swing oracle: drive raw `ClientboundAnimatePacket` bodies
    /// through the real dispatch → `LivingEntity` swing clock →
    /// `resolve_attack_anim` → `HumanoidModel.setupAttackAnimation` pose oracle,
    /// with the equipment + main-arm packets that decide a swing's duration and
    /// animation type, asserting every value against independent decompiled
    /// transcriptions with `--check` (no server, no GPU).
    Swingshot(swingshot_cmd::SwingshotArgs),
    /// M37 particle oracle: drive raw `level_particles` / `level_event` bodies
    /// through the production decoders, grade the spawn fan-out, and assert
    /// seeded particle trajectories bit-for-bit against vectors emitted by a
    /// Java harness copied verbatim from the decompile (`--check`, no server,
    /// no GPU).
    Particleshot(particleshot_cmd::ParticleshotArgs),
    /// The sound oracle (`REWO_AUDIO_PLAN.md` §4): drive raw sound and
    /// `level_event` bodies through the production decoders, grade the seeded
    /// variant pick and the redirect's asymmetric field mix against the LCG,
    /// and assert `SoundEngine::play`'s exact eight-call sequence through a
    /// `RecordingDevice` (`--check`, no server, no GPU, **no device**).
    ///
    /// Under `--features audio` it also grades the quantisation against literal
    /// vectors, real Ogg Vorbis from the asset store, and the production `Mixer`
    /// through a `NullSink`. **A green run is not evidence that this client
    /// makes any sound** — see the module doc.
    Soundshot(soundshot_cmd::SoundshotArgs),
    /// M51c screenshot-capture oracle: render through a **BGRA** `Offscreen` —
    /// the live swapchain's format, which no other gate exercises — and grade
    /// the saved PNG's channel order, opacity and row order, then drive
    /// production `capture::grab` end to end and pin vanilla's filename pattern
    /// and dedup ladder with `--check` (no server, no client jar).
    Captureshot(captureshot_cmd::CaptureshotArgs),
    /// M80's world-border oracle: the lerp state machine, the six packets, the
    /// collision push, and a pixel read-back of the wall.
    ///
    /// Four layers in one gate because the halves fail differently — the
    /// arithmetic is graded against an independent transcription, the physics
    /// as a measured displacement (a correction count cannot see a movement
    /// the client fails to *stop*), and the wall against a CPU prediction over
    /// a black clear.
    Bordershot(bordershot_cmd::BordershotArgs),
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
        Some(Command::Mobtexshot(mt_args)) => mobtexshot_cmd::run(mt_args),
        Some(Command::Skyshot(skyshot_args)) => skyshot_cmd::run(skyshot_args),
        Some(Command::Lightmapshot(lm_args)) => lightmapshot_cmd::run(lm_args),
        Some(Command::Tintshot(ts_args)) => tintshot_cmd::run(ts_args),
        Some(Command::Meshshot(ms_args)) => meshshot_cmd::run(ms_args),
        Some(Command::Dimensioncheck(dc_args)) => dimensioncheck_cmd::run(dc_args),
        Some(Command::Eventshot(ev_args)) => eventshot_cmd::run(ev_args),
        Some(Command::Abilityshot(ab_args)) => abilityshot_cmd::run(ab_args),
        Some(Command::Titleshot(t_args)) => titleshot_cmd::run(t_args),
        Some(Command::Bookshot(b_args)) => bookshot_cmd::run(b_args),
        Some(Command::Gaugeshot(g_args)) => gaugeshot_cmd::run(g_args),
        Some(Command::Leashshot(l_args)) => leashshot_cmd::run(l_args),
        Some(Command::Sidebarshot(sb_args)) => sidebarshot_cmd::run(sb_args),
        Some(Command::Tablistshot(tl_args)) => tablistshot_cmd::run(tl_args),
        Some(Command::Locatorshot(l_args)) => locatorshot_cmd::run(l_args),
        Some(Command::Deathshot(d_args)) => deathshot_cmd::run(d_args),
        Some(Command::Statshot(s_args)) => statshot_cmd::run(s_args),
        Some(Command::Serverlinkshot(sl_args)) => serverlinkshot_cmd::run(sl_args),
        Some(Command::Blockentityshot(be_args)) => blockentityshot_cmd::run(be_args),
        Some(Command::Portalshot(ps_args)) => portalshot_cmd::run(ps_args),
        Some(Command::Inventoryshot(iv_args)) => inventoryshot_cmd::run(iv_args),
        Some(Command::Containershot(cs_args)) => containershot_cmd::run(cs_args),
        Some(Command::Handshot(h_args)) => handshot_cmd::run(h_args),
        Some(Command::Capeshot(cape_args)) => capeshot_cmd::run(cape_args),
        Some(Command::Weathershot(ws_args)) => weathershot_cmd::run(ws_args),
        Some(Command::Hudshot(hs_args)) => hudshot_cmd::run(hs_args),
        Some(Command::Danceshot(dance_args)) => danceshot_cmd::run(dance_args),
        Some(Command::Rideshot(ride_args)) => rideshot_cmd::run(ride_args),
        Some(Command::Attributeshot(attr_args)) => attributeshot_cmd::run(attr_args),
        Some(Command::Healthbarshot(hb_args)) => healthbarshot_cmd::run(hb_args),
        Some(Command::Labelshot(label_args)) => labelshot_cmd::run(label_args),
        Some(Command::Hurtshot(hurt_args)) => hurtshot_cmd::run(hurt_args),
        Some(Command::Breakshot(a)) => breakshot_cmd::run(a),
        Some(Command::Itemshot(item_args)) => itemshot_cmd::run(item_args),
        Some(Command::Swingshot(sw_args)) => swingshot_cmd::run(sw_args),
        Some(Command::Particleshot(pt_args)) => particleshot_cmd::run(pt_args),
        Some(Command::Soundshot(snd_args)) => soundshot_cmd::run(snd_args),
        Some(Command::Captureshot(cap_args)) => captureshot_cmd::run(cap_args),
        Some(Command::Bordershot(b_args)) => bordershot_cmd::run(b_args),
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
