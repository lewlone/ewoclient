//! EwoClient launcher binary entry point.
//!
//! Build-sequence steps 4 + 5 in flight: pearl dust particle system + velvet
//! folds layer. See `CLAUDE.md` for the full build sequence.

// Release builds are a GUI app — no console window pops up when launched from
// a shortcut / Explorer. Debug builds keep the console so `cargo run` still
// shows the env_logger output.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::Arc;
use std::time::{Duration, Instant};

use clap::Parser;
use ewo_core::{Screen, Settings, Theme};
use ewo_render::backdrop::Backdrop;
use ewo_render::screens::settings::{
    AccountHover, AccountOpView, AccountRequest, AccountRowView, AccountView, KeybindRequest,
    KeybindRowView, KeybindView, LinkStatusView, ProfileHover, ProfileRequest, ProfileRowView,
    ProfileView,
};
use ewo_render::screens::{
    self, AboutModalState, DevOverlayState, DevSlot, FrameStats, FriendRowView, FriendsPrefs,
    FriendsViewState, InstancePrefs, InstanceSlot, LaunchingState, LauncherLinkModalState,
    LinkRedeemView, ModalSlot, NewInstanceModalState, Prefs, SettingsSlot, SettingsTab,
};
use ewo_render::screens::instances::Instance;
use ewo_render::skia_safe;
use ewo_render::text::HoverGlowState;
use ewo_render::{app_window, Clock, FontStore, GlBackend, VbtnState};

use auth::{AuthOp, AuthService};
use instance_ops::{delete_instance, loader_spec_for, sync_instance_config};
use window_hit::*;

/// A launch click whose JRE wasn't available — we kicked off a runtime
/// fetch and will retry once it lands.
#[derive(Debug, Clone)]
struct PendingRelaunch {
    instance_idx: usize,
    instance_name: String,
    instance_meta: String,
    /// Major version the missing JRE is for. The retry only fires when
    /// the runtime service emits `Done { major }` matching this value.
    waiting_for_major: u32,
}

/// What to do if a launch's background preparation fails.
#[derive(Debug, Clone, Copy)]
enum PrepareFail {
    /// Fall back to the synthetic launch animation (plain Launch click).
    Synthetic,
    /// Show this line + an error exit on the launching screen (retries).
    Error(&'static str),
}

/// A launch whose blocking preparation runs on a worker thread; the result
/// arrives on `rx` and the UI thread finishes the launch (JRE pick, plan,
/// spawn) — see `App::poll_prepare`.
/// The launching screen's meta line: the instance's "<LOADER> · <version>",
/// then, for a JVM instance, the Java runtime once one has been picked and
/// the heap. A native (Rewo) instance has neither.
fn launch_meta(
    inst: &ewo_render::screens::instances::Instance,
    java_major: Option<u32>,
    ram_gb: i32,
) -> String {
    if matches!(inst.loader, ewo_render::screens::instances::InstanceLoader::Native) {
        return inst.version.clone();
    }
    match java_major {
        Some(major) => format!("{} · JAVA {} · {} GB", inst.version, major, ram_gb),
        None => format!("{} · {} GB", inst.version, ram_gb),
    }
}

struct PendingPrepare {
    rx: std::sync::mpsc::Receiver<Result<launch::prepare::Prepared, String>>,
    idx: usize,
    inst_name: String,
    inst_meta: String,
    fail: PrepareFail,
}
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent};
use winit::keyboard::{Key, NamedKey};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{CursorIcon, ResizeDirection, Window, WindowId};

mod auth;
mod bundled;
mod downloads;
mod events;
mod instance_ops;
mod keybind;
mod launch;
mod launch_slot;
mod loaders;
mod overlay_mods;
mod persistence;
mod profile;
mod runtime;
mod social;
mod util;
mod versions;
mod window;
mod window_hit;

#[derive(Parser, Debug)]
#[command(name = "ewolauncher", about = "EwoClient — Velvet & Pearl")]
struct Args {
    /// Show the developer overlay (state-picker, layout-picker, tweaks panel).
    #[arg(long)]
    dev: bool,
    /// Headless dev tool: refresh the active account's Minecraft token and
    /// print the `REWO_USERNAME` / `REWO_UUID` / `REWO_ACCESS_TOKEN` env
    /// handoff lines, then exit (no window). Used by Rewo's M7 online-mode
    /// verification harness.
    #[arg(long)]
    mint_rewo_env: bool,
}

/// Base URL for the in-development EwoLoader manifests, one JSON per
/// supported Minecraft version line (26.1.json, 26.2.json, …). The loader
/// project lives in a sibling repo on the developer's machine and
/// doesn't yet publish a public meta endpoint, so we point straight at
/// the on-disk manifests via `file://`. Becomes a config knob (or a real
/// HTTPS URL) once the loader publishes a meta endpoint. Override with the
/// `EWO_LOADER_BASE` env var (a `file://` or `https://` base URL).
const DEV_EWO_LOADER_BASE: &str =
    "file:///C:/Users/valtteri/Desktop/EwoLoaderV1/manifest/0.1.0";

fn ewo_loader_base() -> String {
    std::env::var("EWO_LOADER_BASE")
        .ok()
        .map(|s| s.trim().trim_end_matches('/').to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| DEV_EWO_LOADER_BASE.to_string())
}

/// Resolve the EwoLoader manifest URL for a Minecraft version id. The
/// manifests are keyed by version *line* (major.minor), so patch releases
/// map onto their line's manifest: "26.2.1" → 26.2.json, "26.1" →
/// 26.1.json. A line with no manifest on disk (e.g. 1.21.x) fetch-fails
/// at launch and falls back to a vanilla launch — the existing non-fatal
/// loader path — so this doesn't need its own allowlist.
fn ewo_loader_manifest_url(version_id: &str) -> String {
    let line = match version_id.match_indices('.').nth(1) {
        Some((second_dot, _)) => &version_id[..second_dot],
        None => version_id,
    };
    format!("{}/{}.json", ewo_loader_base(), line)
}

/// Action triggered by clicking a sidebar menu item on the main menu.
#[derive(Copy, Clone, Debug)]
enum MenuAction {
    Navigate(Screen),
    About,
    Quit,
}

const MAIN_MENU_ACTIONS: [MenuAction; 4] = [
    MenuAction::Navigate(Screen::Instances),
    MenuAction::Navigate(Screen::Settings),
    MenuAction::About,
    MenuAction::Quit,
];

/// Seconds of no user input before the focused redraw loop throttles down.
/// Long enough that transient interactions (ripples, sheen sweeps, modal
/// entrances) finish at full rate; short enough that an idle launcher stops
/// burning the full monitor rate quickly.
const IDLE_THRESHOLD_SECS: f32 = 0.5;
/// Frame rate the focused loop drops to while idle. Ambient motion (folds,
/// caustics, breathing, drifting motes) runs on multi-second periods, so it's
/// fully smooth here; the eye can't resolve more on those layers.
const IDLE_FPS: f32 = 120.0;

/// Fallback delay (seconds after JVM spawn) before the launcher minimizes
/// itself if no window-ready marker was seen in the game log. Generous
/// enough to cover a slow cold start / heavy modpack on a spinning disk,
/// while still guaranteeing the hand-off eventually happens.
const MINIMIZE_FALLBACK_SECS: f32 = 25.0;

/// Does this game-log line signal that Minecraft's window is coming up on
/// screen? These substrings are logged right as the client initializes its
/// render backend / audio / narrator — i.e. the moment the game becomes
/// visible — so seeing one is our cue to minimize the launcher. Matched
/// case-insensitively; kept deliberately broad so vanilla and modded
/// (Fabric/loader) log formats both trip it.
fn is_window_ready_marker(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    const MARKERS: [&str; 5] = [
        "backend library: lwjgl", // client render backend init — window created
        "lwjgl version",          // same signal, alternate phrasing
        "openal initialized",     // sound engine — just after window shows
        "sound engine started",   // "
        "narrator library",       // accessibility init at client startup
    ];
    MARKERS.iter().any(|m| lower.contains(m))
}

struct App {
    window: Option<Arc<Window>>,
    backend: Option<GlBackend>,
    backdrop: Option<Backdrop>,
    fonts: Option<FontStore>,
    clock: Clock,
    theme: Theme,
    settings: Settings,
    cursor: PhysicalPosition<f64>,
    mouse_down: bool,
    /// Whether the window currently has focus. Used to throttle the
    /// redraw loop to ~10 FPS when unfocused so the launcher doesn't
    /// hammer the GPU while the user is in another app.
    focused: bool,
    /// `WindowEvent::Occluded(true)` was the most recent occlusion event —
    /// the window is fully obscured by another window (fullscreen YouTube,
    /// covered by other apps, etc.). winit fires this on platforms that
    /// can detect it; on Windows it fires reliably. Combined with
    /// `is_minimized` it tells us whether painting the offscreen surface
    /// is wasted work.
    occluded: bool,
    /// Wall-clock of the last user input (cursor move, click, scroll, key).
    /// `about_to_wait` throttles the focused redraw loop to `IDLE_FPS` once the
    /// launcher has been idle past `IDLE_THRESHOLD_SECS`, so an untouched
    /// launcher doesn't render at the full monitor rate (500 Hz on the OLED)
    /// for slow ambient motion the eye can't resolve. Any input snaps it back.
    last_activity: Instant,
    screen: Screen,
    settings_tab: SettingsTab,
    prefs: Prefs,
    instances: Vec<Instance>,
    instance_prefs: InstancePrefs,
    launching: LaunchingState,
    modal: NewInstanceModalState,
    about_modal: AboutModalState,
    /// Phase H2 launcher-link modal — the 6-digit code redemption flow.
    launcher_link_modal: LauncherLinkModalState,
    /// Phase H5 Friends screen state — text input buffer, button states.
    friends_prefs: FriendsPrefs,
    dev_overlay: Option<DevOverlayState>,
    launch_button: VbtnState,
    menu_items: [VbtnState; 4],
    /// Hover-tracked state for the main-menu "EwoClient" heading. Drives
    /// the per-glyph hover-glow stagger (CSS `bt-hover-glow`).
    heading_hover: HoverGlowState,
    /// Microsoft auth service — owns the worker thread + auth state.
    /// Polled each frame via `auth.poll()` to drain the event channel.
    auth: AuthService,
    /// ChickenedIn social state (Phase H). H1 caches per-account
    /// "is this MC UUID linked to Discord?" probes. Polled each frame
    /// the same way `auth` is.
    social: social::SocialState,
    /// Cached client-profile registry — names + the active name. Refreshed
    /// at startup and after any profile-management action (rare), so the
    /// per-frame render path never hits disk for the list.
    profiles: Vec<String>,
    active_profile: String,
    /// The active profile's keybinds (every registered action → its chord).
    /// Loaded at startup, on profile switch, and after a rebind.
    keybinds: std::collections::BTreeMap<String, keybind::KeyChord>,
    /// While `Some`, the next key press is captured as the new binding for
    /// this action id — set by the Keybinds tab, consumed by the keyboard
    /// handler. See [`keybind`].
    keybind_capture: Option<String>,
    /// Mojang version manifest service — owns disk cache + background
    /// refresh thread. Hydrated from cache at startup; refreshed on a
    /// 6-hour TTL. Feeds the new-instance modal's Version dropdown.
    versions: versions::VersionService,
    /// Real-download service — owns one worker thread per active job,
    /// reports progress via mpsc. Polled every frame.
    downloads: downloads::DownloadService,
    /// Receiver for events from the most-recent active JVM launch (Phase
    /// C). `None` while no launch is running. Drained each frame in
    /// `RedrawRequested`.
    launch_rx: Option<std::sync::mpsc::Receiver<launch::LaunchEvent>>,
    /// The running game process (PID + creation time), held so a lingering
    /// zombie (JVM that deadlocks in native teardown on exit — see
    /// `launch::reaper`) can be reaped before the next launch. Set on
    /// `Started`, cleared on exit.
    active_launch: Option<launch::reaper::Tracked>,
    /// Wall-time the running game was spawned, and whether its window-ready
    /// marker has been seen — together they tell a zombie from a game that
    /// is still starting up.
    active_launch_started_at: f32,
    active_launch_window_seen: bool,
    /// Folder id of the instance whose game is running (for its log dump).
    active_launch_instance_id: Option<String>,
    /// A launch whose blocking preparation (manifest/loader fetch, library
    /// downloads, natives extraction) is running on a worker thread.
    pending_prepare: Option<PendingPrepare>,
    /// While a launch is in flight, the wall-time deadline after which the
    /// launcher minimizes itself even if we haven't yet seen the game's
    /// window-ready log marker. `None` when no launch is pending a minimize.
    /// We defer the minimize (rather than doing it on `Started`) so the
    /// window doesn't vanish during the ~10-30s the JVM spends loading before
    /// Minecraft's own window appears — it drops out only once the game is
    /// visibly coming up (marker seen) or this fallback fires. Cleared once
    /// the minimize happens or the launch finishes.
    pending_minimize: Option<f32>,
    /// Bundled-JRE auto-fetch service. Owns one Adoptium download
    /// thread at a time. Polled each frame.
    runtime: runtime::RuntimeService,
    /// When a launch click finds no matching JRE installed, we record
    /// it here, kick off `runtime.start_fetch(major)`, and retry the
    /// launch automatically once the JRE is ready.
    pending_relaunch: Option<PendingRelaunch>,
    /// Wall-time seconds at which to clear the celebrate state. `None` when
    /// not celebrating. Set on Launch click; checked each tick.
    celebrate_until: Option<f32>,
    /// H6 (Roblox-style join). When `Some`, the in-flight / most-recent
    /// launch was initiated as a server-join (main-menu server widget or a
    /// friend's "Join"); `try_real_launch` appends the quick-play arg and
    /// the presence heartbeat reports `in_game · <addr>` while the JVM is
    /// alive. Set by `start_launch`; a normal Launch click sets it `None`.
    active_server: Option<String>,
    /// H6 — cursor is over the main-menu network widget (updated in the
    /// CursorMoved handler, read at render time to drive the hover state).
    server_widget_hover: bool,
    /// Tab the cursor is over in the top tab bar (for the hover glow), or
    /// `None`. Updated each CursorMoved.
    hovered_tab: Option<Screen>,
    /// Previous-frame screen + the wall-time the current screen was entered —
    /// drives the smooth main-menu entrance (fade + slide).
    prev_screen: Screen,
    screen_enter_at: f32,
    /// Cursor is over the "‹ Main menu" back-link (for its hover glow).
    back_link_hover: bool,
    /// Cursor over the top-right minimize / close buttons (hover state).
    min_btn_hover: bool,
    close_btn_hover: bool,
    dev: bool,
}

impl App {
    fn new(dev: bool) -> Self {
        // Phase F: settings live in the active client profile. `profile::load`
        // reconstructs the unified SettingsConfig + the cosmetic tokens,
        // migrating a pre-F settings.toml on first run.
        let (settings_config, settings) = profile::load();
        Self {
            window: None,
            backend: None,
            backdrop: None,
            fonts: None,
            clock: Clock::new(),
            theme: Theme::VELVET,
            settings,
            cursor: PhysicalPosition::new(0.0, 0.0),
            mouse_down: false,
            focused: true,
            occluded: false,
            last_activity: Instant::now(),
            screen: Screen::default(),
            settings_tab: SettingsTab::Graphics,
            prefs: {
                let mut p = Prefs::default();
                p.apply_config(&settings_config);
                let (mod_enabled, mod_fov) = profile::load_modules();
                p.apply_modules(&mod_enabled, mod_fov);
                // PvP-Utils config is shared with the in-game side via
                // `<profile>/pvp.toml`; load it here so the Settings tab
                // shows the user's current setup on first open.
                p.pvp = profile::load_pvp_config();
                p
            },
            // Try the persisted list first, fall back to the bundled
            // defaults if missing or malformed. Ewo manifest URLs are
            // re-derived from each instance's version on every load —
            // instances created by older launcher builds carry the
            // pre-version-keying hardcoded 26.1 URL in instances.toml,
            // which merges 26.1 mods into a 26.2 game ("Incompatible
            // mods found!" at boot). Self-heal instead of asking the
            // user to recreate the instance.
            instances: {
                let mut instances = persistence::load_instances();
                let mut healed = false;
                for inst in instances.iter_mut() {
                    if let ewo_render::screens::instances::InstanceLoader::Ewo { manifest_url } =
                        &mut inst.loader
                    {
                        let version_id =
                            inst.version.rsplit(" · ").next().unwrap_or(&inst.version);
                        let derived = ewo_loader_manifest_url(version_id);
                        if *manifest_url != derived {
                            log::info!(
                                "instances: re-keying \"{}\" loader manifest {} → {}",
                                inst.name,
                                manifest_url,
                                derived
                            );
                            *manifest_url = derived;
                            healed = true;
                        }
                    }
                }
                if healed {
                    persistence::save_instances(&instances);
                }
                instances
            },
            instance_prefs: InstancePrefs::default(),
            launching: LaunchingState::default(),
            modal: NewInstanceModalState::default(),
            about_modal: AboutModalState::default(),
            launcher_link_modal: LauncherLinkModalState::default(),
            friends_prefs: FriendsPrefs::default(),
            dev_overlay: if dev { Some(DevOverlayState::default()) } else { None },
            launch_button: VbtnState::default(),
            menu_items: [VbtnState::default(); 4],
            heading_hover: HoverGlowState::default(),
            // AuthService loads the persisted account store and kicks a
            // silent refresh for the active account itself (see `new`).
            auth: AuthService::new(),
            // SocialState starts with no probes; we trigger them each
            // frame for every signed-in account that hasn't been
            // probed yet (in `drive_social_probes`).
            social: social::SocialState::new(),
            profiles: profile::list(),
            active_profile: profile::active_name(),
            keybinds: profile::load_keybinds(),
            keybind_capture: None,
            versions: versions::VersionService::new(),
            downloads: downloads::DownloadService::new(),
            launch_rx: None,
            active_launch: None,
            active_launch_started_at: 0.0,
            active_launch_window_seen: false,
            active_launch_instance_id: None,
            pending_prepare: None,
            pending_minimize: None,
            runtime: runtime::RuntimeService::new(),
            pending_relaunch: None,
            celebrate_until: None,
            active_server: None,
            server_widget_hover: false,
            hovered_tab: None,
            // Start settled (no entrance on first paint); transitions animate.
            prev_screen: Screen::default(),
            screen_enter_at: -1.0,
            back_link_hover: false,
            min_btn_hover: false,
            close_btn_hover: false,
            dev,
        }
    }

    /// Apply a freshly-loaded profile config — Settings widgets, cosmetic
    /// tokens, vsync, and the backdrop particle density (a profile-scoped
    /// token, so the pools re-spawn).
    fn apply_loaded_config(&mut self, config: screens::SettingsConfig, settings: Settings) {
        self.prefs.apply_config(&config);
        // Modules are per-profile too — reload them for the switched-to profile.
        let (mod_enabled, mod_fov) = profile::load_modules();
        self.prefs.apply_modules(&mod_enabled, mod_fov);
        // PvP-Utils is per-profile as well — reload pvp.toml.
        self.prefs.pvp = profile::load_pvp_config();
        self.settings = settings;
        if let Some(b) = self.backend.as_ref() {
            b.set_vsync(self.prefs.vsync.on);
        }
        if let (Some(window), Some(backdrop)) = (self.window.as_ref(), self.backdrop.as_mut()) {
            let scale = window.scale_factor();
            let size = window.inner_size();
            let cw = card_content_width(size, scale);
            let ch = card_content_height(size, scale);
            backdrop.resize(cw, ch, &self.settings);
        }
    }

    /// Map the active account's chickenedin link probe state into the
    /// renderer-side `LinkStatusView`. Returns `Hidden` when there's no
    /// active account or the probe hasn't been triggered yet. When the
    /// account is MC-linked but the launcher itself hasn't been linked
    /// (Phase H2 social_token missing), returns the clickable variant.
    fn link_status_view(&self) -> LinkStatusView {
        let Some(active) = self.auth.active() else {
            return LinkStatusView::Hidden;
        };
        match self.social.link_status(&active.uuid) {
            social::LinkStatus::Unknown | social::LinkStatus::Probing => LinkStatusView::Probing,
            social::LinkStatus::Linked => {
                if self.auth.social_token(&active.uuid).is_some() {
                    LinkStatusView::Linked
                } else {
                    LinkStatusView::LinkedNeedsLauncherLink
                }
            }
            social::LinkStatus::NotLinked => LinkStatusView::NotLinked,
            social::LinkStatus::Failed(_) => LinkStatusView::Failed,
        }
    }

    /// Bounds of the active screen's Launch button (card-local). Returns
    /// `None` for screens that don't have one.
    fn launch_button_bounds(&self, card_w: f32) -> Option<skia_safe::Rect> {
        match self.screen {
            Screen::Instances => Some(screens::instances::launch_button_bounds(card_w)),
            _ => None,
        }
    }

    /// Attempt a real launch for the instance at `idx`. Returns `true` if a
    /// real launch is under way (Native spawned, or a JVM launch handed to
    /// the background preparer); `false` if we should fall back right away
    /// (instance not Ready, manifest missing, …). A JVM launch finishes in
    /// [`Self::poll_prepare`] once its preparation arrives; a failure there
    /// is handled per `fail`.
    fn try_real_launch(
        &mut self,
        idx: usize,
        inst_name: &str,
        inst_meta: &str,
        time: f32,
        fail: PrepareFail,
    ) -> bool {
        // E6: apply any bundled-mod toggles made in the in-game overlay last
        // session, before we read the instance's mod state for this launch.
        if overlay_mods::apply_overrides(&mut self.instances, idx) {
            persistence::save_instances(&self.instances);
        }
        let inst = match self.instances.get(idx) {
            Some(i) => i.clone(),
            None => return false,
        };
        if inst.status != ewo_render::screens::instances::InstanceStatus::Ready {
            log::warn!(
                "launch: \"{}\" is Pending (download not done) — falling back",
                inst.name
            );
            return false;
        }
        // E6: refresh the in-game MODS view's snapshot of the bundled mods.
        overlay_mods::write_catalog(&inst);
        // F5c: resolve the active profile's keybinds for the in-game mod.
        overlay_mods::write_keybinds(&inst.id);
        // The version *string* comes from the meta, formatted as
        // "<LOADER> · <version>". Strip the loader prefix.
        let version_id = inst.version.rsplit(" · ").next().unwrap_or(&inst.version);
        // Native (Rewo) instances skip the entire JVM pipeline — no
        // manifest, no libraries, no natives, no JRE. Spawn the rewo
        // binary with the REWO_* env contract (REWO_PLAN.md §9.1). With a
        // server join active this is `rewo live` — the real playable
        // client (M3+); without one there's nothing to connect to, so the
        // bare M0 window opens as a placeholder.
        if matches!(
            inst.loader,
            ewo_render::screens::instances::InstanceLoader::Native
        ) {
            let Some(program) = launch::find_rewo_binary() else {
                log::warn!(
                    "launch: rewo binary not found next to the launcher — falling back"
                );
                return false;
            };
            let mut envs = vec![("REWO_VERSION".to_string(), version_id.to_string())];
            if let Some(account) = self.auth.active() {
                if !account.minecraft_token.is_empty() {
                    envs.push(("REWO_USERNAME".into(), account.name.clone()));
                    envs.push(("REWO_UUID".into(), account.uuid.clone()));
                    envs.push(("REWO_ACCESS_TOKEN".into(), account.minecraft_token.clone()));
                }
            }
            let mut args: Vec<String> = Vec::new();
            if let Some(addr) = self.active_server.clone() {
                let (host, port) = match addr.rsplit_once(':') {
                    Some((h, p)) => (h.to_string(), p.parse::<u16>().unwrap_or(25565)),
                    None => (addr.clone(), 25565),
                };
                envs.push(("REWO_SERVER".into(), addr.clone()));
                args = launch::native_client_args(&host, port, version_id);
                log::info!("launch: rewo live → {}", addr);
            }
            let (tx, rx) = std::sync::mpsc::channel::<launch::LaunchEvent>();
            let _ = launch::spawn_native(launch::NativePlan { program, args, envs }, tx);
            self.launch_rx = Some(rx);
            self.active_launch_instance_id = Some(inst.id.clone());
            self.launching.enter_real(time, inst_name, inst_meta);
            log::info!("launch: rewo spawned for \"{}\" ({})", inst.name, version_id);
            return true;
        }
        let manifest = match self.versions.manifest() {
            Some(m) => m,
            None => {
                log::warn!("launch: master manifest not loaded — falling back");
                return false;
            }
        };
        let entry = match manifest.entry(version_id) {
            Some(e) => e.clone(),
            None => {
                log::warn!("launch: {} not in master manifest — falling back", version_id);
                return false;
            }
        };
        // The network + disk work (manifests, library downloads, natives
        // extraction, zombie reaping) runs on a worker so the UI never
        // blocks; `poll_prepare` finishes the launch.
        let rx = launch::prepare::spawn(launch::prepare::PrepareJob {
            entry,
            version_id: version_id.to_string(),
            instance_id: inst.id.clone(),
            loader: inst.loader.clone(),
            mods: inst.mods.clone(),
        });
        self.launching.enter_real(time, inst_name, inst_meta);
        self.launching.push_real_line(
            screens::RealSeverity::Info,
            "[ewo] preparing game files…".into(),
            time,
        );
        self.pending_prepare = Some(PendingPrepare {
            rx,
            idx,
            inst_name: inst_name.to_string(),
            inst_meta: inst_meta.to_string(),
            fail,
        });
        true
    }

    /// Drain the background launch preparation, if any, and finish the
    /// launch on success. Called once per frame.
    fn poll_prepare(&mut self, time: f32) {
        let result = match self.pending_prepare.as_ref().map(|p| p.rx.try_recv()) {
            None | Some(Err(std::sync::mpsc::TryRecvError::Empty)) => return,
            Some(Ok(r)) => r,
            Some(Err(std::sync::mpsc::TryRecvError::Disconnected)) => {
                Err("launch preparation thread died".to_string())
            }
        };
        let Some(pending) = self.pending_prepare.take() else {
            return;
        };
        let prepared = match result {
            Ok(p) => p,
            Err(msg) => {
                log::warn!("launch: {} — falling back", msg);
                self.fail_launch(&pending, &msg, time);
                return;
            }
        };
        if !self.finish_launch(&pending, prepared, time) {
            self.fail_launch(&pending, "could not start the game — see log", time);
        }
    }

    fn fail_launch(&mut self, pending: &PendingPrepare, msg: &str, time: f32) {
        match pending.fail {
            PrepareFail::Synthetic => {
                log::info!("launch: falling back to synthetic for \"{}\"", pending.inst_name);
                self.launching.enter(time, &pending.inst_name, &pending.inst_meta);
            }
            PrepareFail::Error(line) => {
                self.launching.push_real_line(
                    screens::RealSeverity::Warn,
                    format!("[ewo] {} ({})", line, msg),
                    time,
                );
                self.launching.set_real_exit(Some(127), time);
            }
        }
    }

    /// UI-thread half of a JVM launch: pick a JRE (or start fetching one),
    /// build the plan and spawn. Returns `false` on a failure the caller
    /// should surface.
    fn finish_launch(
        &mut self,
        pending: &PendingPrepare,
        prepared: launch::prepare::Prepared,
        time: f32,
    ) -> bool {
        let Some(inst) = self.instances.get(pending.idx).cloned() else {
            return false;
        };
        let (inst_name, inst_meta) = (pending.inst_name.as_str(), pending.inst_meta.as_str());
        let version_id = inst.version.rsplit(" · ").next().unwrap_or(&inst.version);
        let launch::prepare::Prepared {
            pv,
            disabled_mod_ids,
        } = prepared;
        // Pick a JRE matching the per-version manifest's
        // `javaVersion.majorVersion`. Falls back to whatever's first in
        // the detected list if the manifest doesn't specify (legacy
        // 1.8.9-era).
        let required_major = pv
            .java_version
            .as_ref()
            .map(|j| j.major_version)
            .unwrap_or(8);
        let jvm_path = match launch::pick_jre(required_major) {
            Some(j) => {
                log::info!(
                    "launch: picked Java {} at {} (required ≥ {})",
                    j.major,
                    j.path.display(),
                    required_major
                );
                // Name the runtime actually chosen, now that it is known.
                self.launching.instance_meta =
                    launch_meta(&inst, Some(j.major), self.instance_prefs.ram.value as i32);
                j.path.clone()
            }
            None => {
                let installed: Vec<u32> =
                    launch::detect_jres().iter().map(|j| j.major).collect();
                log::info!(
                    "launch: no Java {} installed (have: {:?}) — fetching from Adoptium",
                    required_major,
                    installed
                );
                // Kick off the bundled-JRE download and record this launch
                // as pending. The per-frame runtime poll retries once the
                // fetch completes.
                self.launching.push_real_line(
                    screens::RealSeverity::Info,
                    format!(
                        "[ewo] Java {} not installed — fetching Eclipse Temurin from Adoptium…",
                        required_major
                    ),
                    time,
                );
                self.runtime.start_fetch(required_major);
                self.pending_relaunch = Some(PendingRelaunch {
                    instance_idx: pending.idx,
                    instance_name: inst_name.to_string(),
                    instance_meta: inst_meta.to_string(),
                    waiting_for_major: required_major,
                });
                return true;
            }
        };
        // Use the signed-in Microsoft account's profile when available;
        // fall back to offline mode (placeholder UUID + token) otherwise.
        // Online mode unlocks multiplayer + skin sync; offline mode is
        // singleplayer/LAN only.
        let profile = match self.auth.active() {
            Some(account) if !account.minecraft_token.is_empty() => {
                log::info!(
                    "launch: using signed-in profile {} (token live)",
                    account.name
                );
                launch::LaunchProfile {
                    username: account.name.clone(),
                    uuid: account.uuid.clone(),
                    access_token: account.minecraft_token.clone(),
                    user_type: "msa".to_string(),
                }
            }
            _ => {
                log::info!("launch: offline profile (no live MS token)");
                launch::LaunchProfile::offline(&inst.name)
            }
        };
        let mut plan = match launch::build(&pv, &inst.id, inst.ram, &profile, jvm_path) {
            Ok(p) => p,
            Err(e) => {
                log::warn!("launch: plan build failed: {}", e);
                return false;
            }
        };
        // Append the disabled-mods JVM arg so the loader's BundledMods
        // verification subtracts them from the expected set and the
        // discovery filter skips them. -D args go before main-class +
        // game args, which is exactly where jvm_args ends up.
        if !disabled_mod_ids.is_empty() {
            plan.jvm_args.push(format!(
                "-Dfabric.debug.disableModIds={}",
                disabled_mod_ids.join(",")
            ));
        }
        // Tell ewo-hud's mixin plugin which MC version this is so it can
        // select the right version-specific mixin set (26.2 renamed the
        // frame hook + crosshair/fire-overlay targets). Authoritative over
        // the plugin's Fabric-loader fallback probe.
        plan.jvm_args
            .push(format!("-Dewo.mc.version={}", version_id));
        // A bundled HUD native next to the launcher (see package.ps1) wins
        // over the mod's dev fallback (the repo's cargo target dir).
        if let Some(dll) = launch::find_hud_native() {
            plan.jvm_args
                .push(format!("-Dewo.hud.nativePath={}", dll.display()));
        }
        // H6: if this launch was initiated as a server-join (main-menu
        // server widget or a friend's "Join"), auto-connect on boot via the
        // modern quick-play arg. The address is `host:port`; the client
        // defaults the port to 25565 when omitted. `--quickPlayMultiplayer`
        // is the 1.20+ replacement for the removed `--server`/`--port` pair,
        // which is right for the 26.x line this launcher targets.
        if let Some(addr) = self.active_server.clone() {
            plan.game_args.push("--quickPlayMultiplayer".to_string());
            plan.game_args.push(addr.clone());
            log::info!("launch: quick-play join → {}", addr);
        }
        let (tx, rx) = std::sync::mpsc::channel::<launch::LaunchEvent>();
        let _ = launch::spawn_jvm(plan, tx);
        self.launch_rx = Some(rx);
        self.active_launch_instance_id = Some(inst.id.clone());
        self.launching.push_real_line(
            screens::RealSeverity::Info,
            "[ewo] starting jvm…".into(),
            time,
        );
        log::info!(
            "launch: real JVM spawned for \"{}\" ({})",
            inst.name, version_id
        );
        true
    }

    /// Whether a launch is already running or being prepared. When the
    /// running game is a zombie (its window closed but the process lingers)
    /// it's reaped here and the launch slot is freed; a live game is never
    /// killed — the new launch is refused instead.
    fn launch_slot_busy(&mut self, time: f32) -> bool {
        use launch_slot::{decide, SlotDecision, SlotInput};
        // Gather the input — the only place OS process state is read.
        let tracked = self.active_launch;
        let input = if self.pending_prepare.is_some() || self.pending_relaunch.is_some() {
            SlotInput::Preparing
        } else if self.launch_rx.is_none() {
            SlotInput::NeverLaunched
        } else {
            match tracked {
                // Spawned but not yet reported `Started`.
                None => SlotInput::Starting,
                Some(t) => match launch::reaper::state(&t) {
                    launch::reaper::GameState::Gone => SlotInput::Gone,
                    launch::reaper::GameState::Running { visible_window } => SlotInput::Running {
                        zombie: launch::reaper::is_zombie(
                            visible_window,
                            self.active_launch_window_seen,
                            time - self.active_launch_started_at,
                        ),
                    },
                },
            }
        };
        match decide(input) {
            SlotDecision::Busy(reason) => {
                // The running-game line names the pid, which the pure
                // decision can't carry; it is appended here.
                match (input, tracked) {
                    (SlotInput::Running { .. }, Some(t)) => log::warn!(
                        "launch: {reason} (pid {}) — ignoring launch request",
                        t.pid
                    ),
                    _ => log::warn!("launch: {reason} — ignoring launch request"),
                }
                return true;
            }
            // Nothing was ever launched: nothing to clean up either.
            SlotDecision::Free => return false,
            SlotDecision::ReapThenFree => {
                // Reap first; forgetting the record comes below.
                if let Some(t) = tracked {
                    launch::reaper::reap(&t);
                }
            }
            SlotDecision::ForgetThenFree => {}
        }
        // The game is gone (or was a zombie): drop its OS-side record and
        // clear the launch fields so the next click gets a fresh slot.
        if let Some(t) = tracked {
            launch::reaper::forget(&t);
        }
        self.launch_rx = None;
        self.active_launch = None;
        self.active_launch_instance_id = None;
        false
    }

    /// Shared launch entry point for the Launch button and the H6 server-join
    /// paths (main-menu server widget + friend "Join"). `server` is the
    /// `host:port` to auto-connect to, or `None` for a plain launch.
    ///
    /// Resolves the instance's display name + meta, gates out a `Pending`
    /// (mid-download) or missing instance, stamps `last_played`, then runs
    /// the real launch (falling back to the synthetic animation if anything
    /// upstream is missing) and switches to the Launching screen with the
    /// celebrate burst.
    fn start_launch(&mut self, idx: usize, server: Option<String>, time: f32) {
        use ewo_render::screens::instances::InstanceStatus;
        // A still-downloading instance can't launch; clicking Launch on one
        // whose download failed (or never started this session) retries it.
        if matches!(self.instances.get(idx), Some(i) if i.status == InstanceStatus::Pending) {
            self.retry_download(idx);
        }
        // Resolve name + meta and gate Pending/missing instances. The
        // immutable borrow ends here so the rest can mutate `self`.
        let (inst_name, inst_meta) = match self.instances.get(idx) {
            Some(i) if i.status != InstanceStatus::Pending => (
                i.name.clone(),
                launch_meta(i, None, self.instance_prefs.ram.value as i32),
            ),
            _ => {
                log::warn!(
                    "launch: instance #{} not launchable (missing or still downloading)",
                    idx
                );
                return;
            }
        };
        // One game at a time: a live game is never killed by a second
        // launch click — the click is ignored. Only a zombie is reaped.
        if self.launch_slot_busy(time) {
            return;
        }
        // Dev affordance: `EWO_DEV_SERVER=host:port` points plain Launch
        // clicks at a server without going through a join flow — e.g. the
        // local offline Rewo test server (127.0.0.1:25599) for eyeballing
        // the Native → `rewo live` path. Join flows (Some) always win.
        let server = server.or_else(|| {
            std::env::var("EWO_DEV_SERVER").ok().filter(|s| !s.is_empty())
        });
        if let Some(addr) = &server {
            log::info!("launch: server-join → {} (instance \"{}\")", addr, inst_name);
        } else {
            log::info!("vbtn: Launch clicked → launching \"{}\"", inst_name);
        }
        self.active_server = server;

        // Stamp last_played + persist so the timestamp survives a restart.
        if let Some(inst) = self.instances.get_mut(idx) {
            inst.last_played = "just now".to_string();
            inst.last_played_at = screens::instances::current_unix_seconds();
        }
        persistence::save_instances(&self.instances);

        // A previous launcher session may have left a zombie game process
        // recorded on disk; reap it (only if it's ours, still that exact
        // process, and windowless — a live game is left alone).
        let reaped = launch::reaper::reap_recorded_zombies();
        if reaped > 0 {
            log::info!("launch: reaped {} zombie game process(es) from an earlier run", reaped);
        }

        // Real launch fires only when the instance is Ready + the manifest
        // resolves; otherwise fall back to the synthetic animation so the
        // user still gets feedback.
        let real_launched =
            self.try_real_launch(idx, &inst_name, &inst_meta, time, PrepareFail::Synthetic);
        if !real_launched {
            log::info!("launch: falling back to synthetic for \"{}\"", inst_name);
            self.launching.enter(time, &inst_name, &inst_meta);
        }
        self.screen = Screen::Launching;
        self.launch_button = VbtnState::default();
        self.prefs.close_dropdowns();
        self.instance_prefs.close_dropdowns();
        if let Some(bd) = self.backdrop.as_mut() {
            bd.celebrate(true);
        }
        self.celebrate_until = Some(time + 4.5);
    }

    /// (Re)start the download job for a Pending instance whose job failed or
    /// never ran this session. No-op while a job for it is in flight.
    fn retry_download(&mut self, idx: usize) {
        let Some(inst) = self.instances.get(idx) else {
            return;
        };
        let version_id = inst.version.rsplit(" · ").next().unwrap_or(&inst.version);
        let loader_spec = loader_spec_for(&inst.loader);
        let Some(manifest) = self.versions.manifest() else {
            log::warn!("downloads: master manifest not loaded — can't retry {}", version_id);
            return;
        };
        let Some(entry) = manifest.entry(version_id).cloned() else {
            log::warn!("downloads: {} not in master manifest — can't retry", version_id);
            return;
        };
        self.downloads.start(entry, loader_spec);
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let attrs = Window::default_attributes()
            .with_title("EwoClient")
            .with_decorations(false)
            // Per-pixel alpha: the rounded card floats with transparent corners
            // (the desktop shows through) instead of an opaque black margin +
            // painted bevel.
            .with_transparent(true)
            .with_inner_size(LogicalSize::new(1180.0, 720.0))
            .with_min_inner_size(LogicalSize::new(800.0, 520.0));

        // Windows: create the window WITHOUT a redirection bitmap. The render
        // backend presents through a DirectComposition swapchain (see
        // `ewo-render::gl_backend`), so there must be no opaque GDI/GL
        // redirection surface behind it — otherwise the transparent corners
        // composite against that surface (black) instead of the desktop. This
        // MUST be set at creation time; it can't be added afterwards.
        #[cfg(target_os = "windows")]
        let attrs = {
            use winit::platform::windows::WindowAttributesExtWindows;
            attrs.with_no_redirection_bitmap(true)
        };

        let win = Arc::new(
            event_loop
                .create_window(attrs)
                .expect("failed to create window"),
        );

        window::configure(&win);

        // Warm the JRE-detection cache on a background thread. `pick_jre` on
        // the Launch path calls `detect_all`, which shells `java -version` at
        // every installed JDK — a 1-3s scan that, run cold on the UI thread at
        // click time, froze the whole render loop. Kicking it off at window
        // creation means the `OnceLock` cache is populated seconds before the
        // user can reach the Launch button, so the click resolves instantly.
        // (CREATE_NO_WINDOW — see `launch::no_window` — keeps the scan's
        // subprocesses from flashing consoles here too.)
        std::thread::Builder::new()
            .name("ewo-jre-warm".into())
            .spawn(|| {
                let n = launch::detect_jres().len();
                log::info!("jre warm: {n} runtime(s) detected (cache primed)");
            })
            .expect("spawn jre-warm thread");

        let backend = GlBackend::new(event_loop, win.clone());
        // Backdrop particle pools + layout sizes live in logical pixels —
        // the same coord space `draw_frame` paints in. Inputs to
        // `card_content_size` are logical, so divide physical by the DPI
        // scale factor up-front. On 100% scale they're equal; on HiDPI
        // (e.g. 125% on 1440p) they differ — and a physical-sized backdrop
        // would put the particles in a coord space the renderer doesn't
        // share, throwing off both density + bounds.
        let size = win.inner_size();
        let scale = win.scale_factor() as f32;
        let logical_w = ((size.width as f32) / scale) as u32;
        let logical_h = ((size.height as f32) / scale) as u32;
        let (card_w, card_h) = app_window::card_content_size(logical_w, logical_h);
        let backdrop = Backdrop::new(card_w, card_h, &self.settings);
        let fonts = FontStore::new();

        win.request_redraw();

        self.window = Some(win);
        self.backend = Some(backend);
        self.backdrop = Some(backdrop);
        self.fonts = Some(fonts);

        // Initialize per-mod toggle state from the now-built instance list.
        self.instance_prefs.sync_mods(&self.instances);

        // Apply the persisted VSync preference to the GL backend that
        // just came online. The backend defaults to vsync-on, so this is
        // only meaningful when the user had it off last session.
        if let Some(b) = self.backend.as_ref() {
            b.set_vsync(self.prefs.vsync.on);
        }

        if self.dev {
            log::info!("dev overlay enabled");
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _id: WindowId,
        event: WindowEvent,
    ) {
        let Some(window) = self.window.as_ref().cloned() else {
            return;
        };

        // Any user input resets the idle clock — so the frame-pacing throttle
        // in `about_to_wait` only kicks in when the launcher is genuinely idle.
        // (Non-binding patterns, so `event` is not consumed by this peek.)
        if matches!(
            event,
            WindowEvent::CursorMoved { .. }
                | WindowEvent::MouseInput { .. }
                | WindowEvent::MouseWheel { .. }
                | WindowEvent::KeyboardInput { .. }
                | WindowEvent::CursorEntered { .. }
                | WindowEvent::Touch(..)
        ) {
            self.last_activity = Instant::now();
        }

        match event {
            WindowEvent::CloseRequested => event_loop.exit(),

            WindowEvent::Focused(focused) => {
                self.focused = focused;
                if focused {
                    // Coming back from unfocused — kick off a fresh redraw
                    // so animations resume immediately, at full rate briefly.
                    self.last_activity = Instant::now();
                    window.request_redraw();
                }
            }

            WindowEvent::Occluded(occluded) => {
                // The window became fully obscured (or stopped being
                // obscured). On Windows this fires reliably when another
                // fullscreen app takes over, when the user minimises, or
                // when our window is fully hidden behind others. We use
                // it together with `focused` to gate the render+swap
                // path: presenting a frame when the OS won't show it is
                // wasted work AND a known leak path (driver-side
                // present queue accumulates).
                self.occluded = occluded;
                if !occluded {
                    window.request_redraw();
                }
            }

            WindowEvent::KeyboardInput {
                event:
                    KeyEvent {
                        state: ElementState::Pressed,
                        logical_key,
                        physical_key,
                        text,
                        ..
                    },
                ..
            } => {
                self.on_key_pressed(&window, logical_key, physical_key, text);
            }

            WindowEvent::MouseWheel { delta, .. } => {
                self.on_mouse_wheel(&window, delta);
            }

            WindowEvent::Resized(size) => {
                if let Some(backend) = self.backend.as_mut() {
                    backend.resize(size.width, size.height);
                }
                if let Some(backdrop) = self.backdrop.as_mut() {
                    // Backdrop is sized in logical pixels — the same space
                    // `draw_frame` paints in after the canvas scale. Convert
                    // physical→logical via the current scale factor.
                    let scale = window.scale_factor() as f32;
                    let logical_w = ((size.width as f32) / scale) as u32;
                    let logical_h = ((size.height as f32) / scale) as u32;
                    let (cw, ch) = app_window::card_content_size(logical_w, logical_h);
                    backdrop.resize(cw, ch, &self.settings);
                }
                window.request_redraw();
            }

            WindowEvent::CursorMoved { position, .. } => {
                self.on_cursor_moved(&window, position);
            }

            WindowEvent::MouseInput { state, button: MouseButton::Left, .. } => {
                self.on_left_mouse(event_loop, &window, state);
            }

            WindowEvent::RedrawRequested => {
                self.on_redraw(&window);
            }

            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        // Frame pacing. `request_redraw` only marks the window dirty; this
        // ControlFlow decides *when* the loop wakes to service that redraw, so
        // a WaitUntil here caps the effective frame rate even though the
        // redraw is always pending.
        //
        //  1. Unfocused → ~10 FPS — GPU stays idle in another window.
        //  2. Focused + idle (no input past IDLE_THRESHOLD, and not mid-launch)
        //     → IDLE_FPS. Ambient motion is far slower than that, so it reads
        //     identical while cutting GPU/heat/power on an untouched launcher.
        //     Applies regardless of vsync — vsync alone caps at the monitor
        //     rate (up to 500 Hz on the OLED), which is overkill at idle.
        //  3. Focused + active, VSync off, `max_fps < 240` → cap to max_fps.
        //  4. Focused + active otherwise → Poll (vsync caps at refresh; the
        //     dev-overlay uncapped path validates the 500fps OLED target).
        if !self.focused {
            event_loop.set_control_flow(ControlFlow::WaitUntil(
                Instant::now() + Duration::from_millis(100),
            ));
            if let Some(w) = self.window.as_ref() {
                w.request_redraw();
            }
            return;
        }

        let idle = self.last_activity.elapsed().as_secs_f32() >= IDLE_THRESHOLD_SECS
            && self.screen != Screen::Launching;

        let max_fps = self.prefs.max_fps.value;
        let cap_fps: Option<f32> = if idle {
            Some(IDLE_FPS)
        } else if !self.prefs.vsync.on && max_fps > 0.0 && max_fps < 240.0 {
            Some(max_fps)
        } else {
            None
        };

        match cap_fps {
            Some(fps) => {
                let target_ns = (1_000_000_000.0 / fps).max(1.0) as u64;
                event_loop.set_control_flow(ControlFlow::WaitUntil(
                    Instant::now() + Duration::from_nanos(target_ns),
                ));
                if let Some(w) = self.window.as_ref() {
                    w.request_redraw();
                }
            }
            None => event_loop.set_control_flow(ControlFlow::Poll),
        }
    }
}

/// Phase H5: map a `social::FriendEntry` (raw bot payload) to a
/// renderer-ready `FriendRowView`. The launcher doesn't yet know MC
/// display names (no Mojang UUID→name resolver), so we fall back to
/// "Player <last-8-of-uuid>" — good enough for v1, polish later.
fn friend_entry_to_view(entry: &social::FriendEntry) -> FriendRowView {
    let display_name = entry
        .minecraft_uuid
        .as_deref()
        .map(|u| {
            let cleaned: String = u.chars().filter(|c| c.is_ascii_hexdigit()).collect();
            if cleaned.len() >= 8 {
                format!("Player {}", &cleaned[cleaned.len() - 8..])
            } else {
                format!("Player {}", u)
            }
        })
        .unwrap_or_else(|| "Unknown".into());

    let (presence, online) = match entry.presence.as_ref() {
        None => ("offline".to_string(), false),
        Some(p) => {
            let where_at = match p.location.as_str() {
                "launcher" => p
                    .screen
                    .as_deref()
                    .map(|s| format!("in launcher · {}", s))
                    .unwrap_or_else(|| "in launcher".into()),
                "in_game" => p
                    .server_addr
                    .as_deref()
                    .map(|s| format!("in-game · {}", s))
                    .unwrap_or_else(|| "in-game".into()),
                other => other.to_string(),
            };
            (where_at, true)
        }
    };

    // H6 — joinable address: only when the friend is actually in-game.
    let server_addr = entry.presence.as_ref().and_then(|p| {
        if p.location == "in_game" {
            p.server_addr.clone()
        } else {
            None
        }
    });

    FriendRowView {
        display_name,
        presence,
        online,
        discord_id: entry.discord_id.clone(),
        server_addr,
    }
}

/// Stable lowercase name for a launcher screen — used as the `screen`
/// field on Phase H3 presence heartbeats. Friends see this verbatim
/// (e.g. "in_launcher · settings"), so keep the strings short.
fn screen_name(screen: Screen) -> &'static str {
    match screen {
        Screen::MainMenu => "main_menu",
        Screen::Instances => "instances",
        Screen::Friends => "friends",
        Screen::Settings => "settings",
        Screen::Launching => "launching",
    }
}

/// Drive any slider on the active Settings tab with the current cursor +
/// mouse-button state. Continuous-drive semantics: on rising edge inside
/// bounds the slider starts dragging; while dragging, value tracks `mouse.x`
/// even if the cursor leaves the widget bounds; on falling edge dragging
/// ends. See `VsliderState::drive` for the per-widget contract.
///
/// Also updates dropdown menu hover state for any open dropdown so cursor
/// motion lights up the row under the cursor.
fn drive_settings_sliders(
    prefs: &mut Prefs,
    tab: SettingsTab,
    fonts: Option<&FontStore>,
    mouse: (f32, f32),
    mouse_down: bool,
    card_w: f32,
    card_h: f32,
    account_uuids: &[String],
    profile_names: &[String],
    link_status: LinkStatusView,
) -> bool {
    let Some(fonts) = fonts else {
        return false;
    };
    // Account tab — custom layout. Update row / remove / add-button hover.
    if tab == SettingsTab::Account {
        let layout =
            screens::settings::account_tab_layout(fonts, card_w, account_uuids.len(), link_status);
        prefs.account_hover = None;
        for rl in &layout.rows {
            if rect_contains(&rl.remove, mouse) {
                prefs.account_hover = Some(AccountHover::Remove(rl.index));
            } else if rect_contains(&rl.row, mouse) {
                prefs.account_hover = Some(AccountHover::Row(rl.index));
            }
        }
        prefs.account_add.handle(mouse, layout.add_button, false);
        return false;
    }
    // Profiles tab — update row / delete / button hover.
    if tab == SettingsTab::Profiles {
        let layout = screens::settings::profiles_tab_layout(fonts, card_w, profile_names.len());
        let can_delete = profile_names.len() > 1;
        prefs.profile_hover = None;
        for rl in &layout.rows {
            if can_delete && rect_contains(&rl.delete, mouse) {
                prefs.profile_hover = Some(ProfileHover::Delete(rl.index));
            } else if rect_contains(&rl.rename, mouse) {
                prefs.profile_hover = Some(ProfileHover::Rename(rl.index));
            } else if rect_contains(&rl.row, mouse) {
                prefs.profile_hover = Some(ProfileHover::Row(rl.index));
            }
        }
        prefs.profile_new.handle(mouse, layout.new_button, false);
        prefs.profile_dup.handle(mouse, layout.dup_button, false);
        return false;
    }
    // Keybinds tab — update chord + reset hover within the scrolled list.
    if tab == SettingsTab::Keybinds {
        let layout = screens::settings::keybinds_tab_layout(
            fonts,
            card_w,
            card_h,
            keybind::REGISTRY.len(),
            prefs.settings_scroll,
        );
        let max = (layout.content_h - layout.list_region.height()).max(0.0);
        prefs.settings_scroll = prefs.settings_scroll.clamp(0.0, max);
        let in_list = rect_contains(&layout.list_region, mouse);
        prefs.keybind_hover = None;
        if in_list {
            for rl in &layout.rows {
                if rect_contains(&rl.chord, mouse) {
                    prefs.keybind_hover = Some(rl.index);
                }
            }
        }
        let probe = if in_list { mouse } else { (-1.0, -1.0) };
        prefs.keybind_reset.handle(probe, layout.reset_button, false);
        return false;
    }
    // Modules tab — toggle hover + the FOV slider, within the scrolled list.
    if tab == SettingsTab::Modules {
        let layout = screens::settings::modules_tab_layout(
            fonts,
            card_w,
            card_h,
            prefs.settings_scroll,
        );
        let max = (layout.content_h - layout.list_region.height()).max(0.0);
        prefs.settings_scroll = prefs.settings_scroll.clamp(0.0, max);
        let in_list = rect_contains(&layout.list_region, mouse);
        let probe = if in_list { mouse } else { (-1.0, -1.0) };
        for rl in &layout.rows {
            if let Some(toggle) = prefs.module_toggles.get_mut(rl.index) {
                toggle.handle(probe, rl.toggle, false);
            }
            if let Some(slider) = rl.slider {
                if prefs.module_fov.drive(mouse, slider, mouse_down) {
                    prefs.modules_changed = true;
                }
            }
        }
        return false;
    }
    // PvP-Utils tab — slider drag and release. Press dispatch (toggles + chip
    // cycles + drag start) lives in the MouseInput Pressed branch.
    if tab == SettingsTab::PvpUtils {
        let layout = screens::settings::pvp_tab_layout(
            fonts,
            card_w,
            card_h,
            prefs.settings_scroll,
        );
        let max = (layout.content_h - layout.list_region.height()).max(0.0);
        prefs.settings_scroll = prefs.settings_scroll.clamp(0.0, max);
        if prefs.pvp_drag.is_some() {
            screens::settings::drive_pvp_drag(prefs, fonts, card_w, card_h, mouse.0);
            if !mouse_down {
                screens::settings::end_pvp_drag(prefs);
            }
        }
        return false;
    }
    let mut changed = false;
    for (slot, rect) in screens::settings::widget_bounds(tab, fonts, card_w, card_h) {
        match slot {
            SettingsSlot::MaxFps => {
                if prefs.max_fps.drive(mouse, rect, mouse_down) {
                    changed = true;
                }
            }
            SettingsSlot::Master => {
                if prefs.master.drive(mouse, rect, mouse_down) {
                    changed = true;
                }
            }
            SettingsSlot::Music => {
                if prefs.music.drive(mouse, rect, mouse_down) {
                    changed = true;
                }
            }
            SettingsSlot::Effects => {
                if prefs.effects.drive(mouse, rect, mouse_down) {
                    changed = true;
                }
            }
            // Path fields and Reset button drive their own hover state.
            SettingsSlot::GameDir => {
                prefs.game_dir.drive_hover(mouse, rect);
            }
            SettingsSlot::Downloads => {
                prefs.downloads.drive_hover(mouse, rect);
            }
            SettingsSlot::ResetPrefs => {
                prefs.reset_prefs.handle(mouse, rect, false);
            }
            // Toggles + dropdowns react to discrete press events, not motion.
            SettingsSlot::Vsync
            | SettingsSlot::AmbientHum
            | SettingsSlot::AutoBackup
            | SettingsSlot::Telemetry
            | SettingsSlot::WindowMode
            | SettingsSlot::Theme
            | SettingsSlot::LogLevel => {}
        }
    }

    // Update menu-row hover for any open dropdown.
    if let Some(slot) = prefs.open_dropdown() {
        if let Some(opts) = screens::settings::dropdown_options(slot) {
            if let Some(head) =
                screens::settings::dropdown_head_for_slot(slot, fonts, card_w, card_h)
            {
                let (menu_bounds, _flip) =
                    ewo_render::widgets::menu_layout(head, opts.len(), card_h);
                if let Some(state) = prefs.dropdown_state_mut(slot) {
                    state.update_menu_hover(mouse, menu_bounds, opts.len());
                }
            }
        }
    }

    changed
}

/// Route a press event on the Settings screen. Returns `true` when the
/// press hit a Settings widget (and so should be considered "handled" and
/// not fall through to the launch-button or window-drag handlers).
///
/// Order of resolution:
///   1. If a dropdown is open and the press is inside its menu → commit row
///      and consume the press.
///   2. If a dropdown is open and the press is *outside* both its head and
///      its menu → close the menu and consume the press.
///   3. Otherwise iterate the active tab's widgets:
///      - dropdown head → toggle open/closed (closing any other open one)
///      - toggle → flip
///      - slider → no-op (already handled by `drive_settings_sliders`)
fn handle_settings_press(
    prefs: &mut Prefs,
    tab: SettingsTab,
    fonts: Option<&FontStore>,
    mouse: (f32, f32),
    card_w: f32,
    card_h: f32,
    account_uuids: &[String],
    profile_names: &[String],
    link_status: LinkStatusView,
) -> (bool, bool) {
    let Some(fonts) = fonts else {
        return (false, false);
    };

    // Account tab — custom layout, handled outside the row-grid dispatch.
    // Remove buttons are nested inside row rects, so test them first.
    if tab == SettingsTab::Account {
        let layout =
            screens::settings::account_tab_layout(fonts, card_w, account_uuids.len(), link_status);
        for rl in &layout.rows {
            if rect_contains(&rl.remove, mouse) {
                if let Some(uuid) = account_uuids.get(rl.index) {
                    log::info!("account: remove {}", uuid);
                    prefs.account_request = Some(AccountRequest::Remove(uuid.clone()));
                }
                return (true, false);
            }
        }
        for rl in &layout.rows {
            if rect_contains(&rl.row, mouse) {
                if let Some(uuid) = account_uuids.get(rl.index) {
                    prefs.account_request = Some(AccountRequest::SetActive(uuid.clone()));
                }
                return (true, false);
            }
        }
        // Phase H2: clickable link-status line (only when status is
        // `LinkedNeedsLauncherLink`, in which case `link_line_bounds`
        // is `Some`).
        if let Some(bounds) = layout.link_line_bounds {
            if rect_contains(&bounds, mouse) {
                log::info!("account: link line clicked → open launcher-link modal");
                prefs.account_request = Some(AccountRequest::OpenLauncherLink);
                return (true, false);
            }
        }
        if prefs.account_add.handle(mouse, layout.add_button, true) {
            log::info!("account: add-account button clicked");
            prefs.account_request = Some(AccountRequest::Add);
            return (true, false);
        }
        return (false, false);
    }

    // Profiles tab — custom layout. Delete buttons nest inside rows, so
    // test them first; then row clicks (switch); then the action buttons.
    if tab == SettingsTab::Profiles {
        let layout = screens::settings::profiles_tab_layout(fonts, card_w, profile_names.len());
        // A click anywhere while renaming commits the in-progress rename.
        if let Some(idx) = prefs.profile_renaming {
            prefs.profile_request = Some(ProfileRequest::Rename {
                index: idx,
                new_name: prefs.profile_rename_buffer.clone(),
            });
            prefs.profile_renaming = None;
            return (true, false);
        }
        let can_delete = profile_names.len() > 1;
        // Rename buttons nest inside rows — test them before row clicks.
        for rl in &layout.rows {
            if rect_contains(&rl.rename, mouse) {
                if let Some(name) = profile_names.get(rl.index) {
                    log::info!("profile: rename \"{}\" — entering edit", name);
                    prefs.profile_renaming = Some(rl.index);
                    prefs.profile_rename_buffer = name.clone();
                    prefs.profile_rename_focus_time = 0.0;
                }
                return (true, false);
            }
        }
        if can_delete {
            for rl in &layout.rows {
                if rect_contains(&rl.delete, mouse) {
                    if let Some(name) = profile_names.get(rl.index) {
                        log::info!("profile: delete \"{}\"", name);
                        prefs.profile_request = Some(ProfileRequest::Delete(name.clone()));
                    }
                    return (true, false);
                }
            }
        }
        for rl in &layout.rows {
            if rect_contains(&rl.row, mouse) {
                if let Some(name) = profile_names.get(rl.index) {
                    prefs.profile_request = Some(ProfileRequest::Switch(name.clone()));
                }
                return (true, false);
            }
        }
        if prefs.profile_new.handle(mouse, layout.new_button, true) {
            prefs.profile_request = Some(ProfileRequest::New);
            return (true, false);
        }
        if prefs.profile_dup.handle(mouse, layout.dup_button, true) {
            prefs.profile_request = Some(ProfileRequest::Duplicate);
            return (true, false);
        }
        return (false, false);
    }

    // Keybinds tab — custom layout. A chord button arms a rebind; the reset
    // button restores every registry default. Only acts inside the viewport.
    if tab == SettingsTab::Keybinds {
        let layout = screens::settings::keybinds_tab_layout(
            fonts,
            card_w,
            card_h,
            keybind::REGISTRY.len(),
            prefs.settings_scroll,
        );
        if rect_contains(&layout.list_region, mouse) {
            for rl in &layout.rows {
                if rect_contains(&rl.chord, mouse) {
                    prefs.keybind_request = Some(KeybindRequest::Capture(rl.index));
                    return (true, false);
                }
            }
            if prefs.keybind_reset.handle(mouse, layout.reset_button, true) {
                prefs.keybind_request = Some(KeybindRequest::ResetAll);
                return (true, false);
            }
        }
        return (false, false);
    }

    // Modules tab — a toggle click flips the module; a slider press is
    // consumed here (the drag itself runs in `drive_settings_sliders`).
    if tab == SettingsTab::Modules {
        let layout = screens::settings::modules_tab_layout(
            fonts,
            card_w,
            card_h,
            prefs.settings_scroll,
        );
        if rect_contains(&layout.list_region, mouse) {
            for rl in &layout.rows {
                if rect_contains(&rl.toggle, mouse) {
                    if let Some(toggle) = prefs.module_toggles.get_mut(rl.index) {
                        if toggle.handle(mouse, rl.toggle, true) {
                            prefs.modules_changed = true;
                        }
                    }
                    return (true, false);
                }
                if let Some(slider) = rl.slider {
                    if rect_contains(&slider, mouse) {
                        return (true, false);
                    }
                }
            }
        }
        return (false, false);
    }

    // PvP-Utils tab — full press dispatch lives in `pvp_tab_press`. It returns
    // `true` when the press was consumed (toggle/cycle/drag start). The drag
    // itself runs through `drive_settings_sliders`.
    if tab == SettingsTab::PvpUtils {
        let consumed = screens::settings::pvp_tab_press(prefs, fonts, card_w, card_h, mouse.0, mouse.1);
        return (consumed, false);
    }

    let mut changed = false;

    // (1) and (2): handle any open dropdown menu first.
    if let Some(open_slot) = prefs.open_dropdown() {
        if let Some(opts) = screens::settings::dropdown_options(open_slot) {
            if let Some(head) =
                screens::settings::dropdown_head_for_slot(open_slot, fonts, card_w, card_h)
            {
                let (menu_bounds, _flip) =
                    ewo_render::widgets::menu_layout(head, opts.len(), card_h);
                let in_menu = rect_contains(&menu_bounds, mouse);
                let in_head = rect_contains(&head, mouse);
                if in_menu {
                    if let Some(state) = prefs.dropdown_state_mut(open_slot) {
                        if let Some(idx) = state.handle_menu(mouse, menu_bounds, opts.len(), true)
                        {
                            log::info!("dropdown {:?} → {} ({})", open_slot, idx, opts[idx]);
                            changed = true;
                        }
                    }
                    return (true, changed);
                }
                if !in_head {
                    if let Some(state) = prefs.dropdown_state_mut(open_slot) {
                        state.close();
                    }
                    // Don't return — allow the press to fall through so the
                    // user can click another widget while dismissing.
                }
            }
        }
    }

    // (3) Normal widget dispatch.
    for (slot, rect) in screens::settings::widget_bounds(tab, fonts, card_w, card_h) {
        if !rect_contains(&rect, mouse) {
            continue;
        }
        match slot {
            SettingsSlot::Vsync => {
                if prefs.vsync.handle(mouse, rect, true) {
                    log::info!("vsync: {}", prefs.vsync.on);
                    changed = true;
                }
            }
            SettingsSlot::AmbientHum => {
                if prefs.ambient_hum.handle(mouse, rect, true) {
                    log::info!("ambient_hum: {}", prefs.ambient_hum.on);
                    changed = true;
                }
            }
            SettingsSlot::AutoBackup => {
                if prefs.auto_backup.handle(mouse, rect, true) {
                    log::info!("auto_backup: {}", prefs.auto_backup.on);
                    changed = true;
                }
            }
            // UI-only: there is no telemetry backend and nothing reads this
            // flag — the launcher sends no telemetry either way.
            SettingsSlot::Telemetry => {
                if prefs.telemetry.handle(mouse, rect, true) {
                    log::info!("telemetry: {}", prefs.telemetry.on);
                    changed = true;
                }
            }
            SettingsSlot::WindowMode | SettingsSlot::Theme | SettingsSlot::LogLevel => {
                // Close other dropdowns first so only one is ever open.
                close_other_dropdowns(prefs, slot);
                if let Some(state) = prefs.dropdown_state_mut(slot) {
                    if state.handle_head(mouse, rect, true) {
                        log::info!("dropdown {:?}: open={}", slot, state.open);
                    }
                }
            }
            SettingsSlot::GameDir => {
                if prefs.game_dir.handle_press(mouse, rect) {
                    log::info!("game_dir: Browse clicked");
                }
            }
            SettingsSlot::Downloads => {
                if prefs.downloads.handle_press(mouse, rect) {
                    log::info!("downloads: Browse clicked");
                }
            }
            SettingsSlot::ResetPrefs => {
                if prefs.reset_prefs.handle(mouse, rect, true) {
                    log::info!("reset_prefs: clicked → flagging for reset");
                    prefs.reset_requested = true;
                }
            }
            // Sliders driven by drive_settings_sliders — head click consumed there.
            SettingsSlot::MaxFps
            | SettingsSlot::Master
            | SettingsSlot::Music
            | SettingsSlot::Effects => {}
        }
        return (true, changed);
    }
    (false, changed)
}

/// Drive the Instances detail's slider drags + dropdown menu hover.
/// Mirror of `drive_settings_sliders` for the Instances screen. Returns
/// `true` when a slider value changed so the caller can mirror the
/// change into the underlying instance + persist.
#[allow(clippy::too_many_arguments)]
fn drive_instance_widgets(
    prefs: &mut InstancePrefs,
    instances: &[Instance],
    fonts: Option<&FontStore>,
    mouse: (f32, f32),
    mouse_down: bool,
    card_w: f32,
    card_h: f32,
) -> bool {
    let Some(fonts) = fonts else {
        return false;
    };
    let mut value_changed = false;
    for (slot, rect) in
        screens::instances::widget_bounds(card_w, card_h, fonts, prefs, instances)
    {
        match slot {
            InstanceSlot::Ram => {
                if prefs.ram.drive(mouse, rect, mouse_down) {
                    value_changed = true;
                }
            }
            InstanceSlot::RenderDist => {
                if prefs.render_dist.drive(mouse, rect, mouse_down) {
                    value_changed = true;
                }
            }
            InstanceSlot::JavaRuntime | InstanceSlot::ModToggle(_) => {}
        }
    }

    // Update menu hover for any open dropdown.
    if let Some(slot) = prefs.open_dropdown() {
        if let Some(opts) = screens::instances::dropdown_options(slot) {
            if let Some(head) = screens::instances::dropdown_head_for_slot(
                slot, fonts, card_w, card_h, prefs, instances,
            ) {
                let (menu_bounds, _flip) =
                    ewo_render::widgets::menu_layout(head, opts.len(), card_h);
                if let Some(state) = prefs.dropdown_state_mut(slot) {
                    state.update_menu_hover(mouse, menu_bounds, opts.len());
                }
            }
        }
    }

    value_changed
}

/// Route a press event on the Instances screen — mirror of
/// `handle_settings_press`. Returns `true` when the press hit a widget.
#[allow(clippy::too_many_arguments)]
fn handle_instances_press(
    prefs: &mut InstancePrefs,
    instances: &mut Vec<Instance>,
    fonts: Option<&FontStore>,
    mouse: (f32, f32),
    card_w: f32,
    card_h: f32,
) -> bool {
    let Some(fonts) = fonts else {
        return false;
    };

    // (1) and (2): handle any open dropdown menu first.
    if let Some(open_slot) = prefs.open_dropdown() {
        if let Some(opts) = screens::instances::dropdown_options(open_slot) {
            if let Some(head) = screens::instances::dropdown_head_for_slot(
                open_slot, fonts, card_w, card_h, prefs, instances,
            ) {
                let (menu_bounds, _flip) =
                    ewo_render::widgets::menu_layout(head, opts.len(), card_h);
                let in_menu = rect_contains(&menu_bounds, mouse);
                let in_head = rect_contains(&head, mouse);
                if in_menu {
                    let mut commit = false;
                    if let Some(state) = prefs.dropdown_state_mut(open_slot) {
                        if let Some(idx) =
                            state.handle_menu(mouse, menu_bounds, opts.len(), true)
                        {
                            log::info!("instance dropdown {:?} → {} ({})", open_slot, idx, opts[idx]);
                            commit = true;
                        }
                    }
                    if commit {
                        sync_instance_config(instances, prefs);
                        persistence::save_instances(instances);
                    }
                    return true;
                }
                if !in_head {
                    if let Some(state) = prefs.dropdown_state_mut(open_slot) {
                        state.close();
                    }
                }
            }
        }
    }

    // (3) Normal widget dispatch.
    for (slot, rect) in
        screens::instances::widget_bounds(card_w, card_h, fonts, prefs, instances)
    {
        if !rect_contains(&rect, mouse) {
            continue;
        }
        match slot {
            InstanceSlot::JavaRuntime => {
                if let Some(state) = prefs.dropdown_state_mut(slot) {
                    if state.handle_head(mouse, rect, true) {
                        log::info!("instance dropdown {:?}: open={}", slot, state.open);
                    }
                }
            }
            InstanceSlot::ModToggle(i) => {
                if let Some(flag) = prefs.mods_on.get_mut(i) {
                    *flag = !*flag;
                    log::info!("mod[{}]: {}", i, *flag);
                    // Mirror the toggle into the underlying instance so
                    // the change persists and is reflected on the next
                    // selection-reset.
                    let new_value = *flag;
                    if let Some(inst) = instances.get_mut(prefs.selected) {
                        if let Some(m) = inst.mods.get_mut(i) {
                            m.on = new_value;
                        }
                    }
                    persistence::save_instances(instances);
                }
            }
            // Sliders consumed by drive_instance_widgets.
            InstanceSlot::Ram | InstanceSlot::RenderDist => {}
        }
        return true;
    }
    false
}

/// Drive the dev overlay's slider drags + ghost button hover states.
fn drive_dev_overlay(
    overlay: &mut DevOverlayState,
    mouse: (f32, f32),
    mouse_down: bool,
    card_w: f32,
    card_h: f32,
) {
    for (slot, rect) in screens::dev_overlay::widget_bounds(card_w, card_h) {
        match slot {
            DevSlot::Reset => {
                overlay.reset_btn.handle(mouse, rect, false);
            }
            DevSlot::VsyncToggle => {
                overlay.vsync_btn.handle(mouse, rect, false);
            }
            DevSlot::SimError => {
                overlay.sim_error_btn.handle(mouse, rect, false);
            }
            slot => {
                if let Some(state) = screens::dev_overlay::slider_state_mut(overlay, slot) {
                    state.drive(mouse, rect, mouse_down);
                }
            }
        }
    }
}

/// Route a press inside the dev overlay panel. Returns whether the vsync
/// state changed this frame — caller uses that to call `GlBackend::set_vsync`.
fn handle_dev_overlay_press(
    overlay: &mut DevOverlayState,
    mouse: (f32, f32),
    card_w: f32,
    card_h: f32,
) -> bool {
    for (slot, rect) in screens::dev_overlay::widget_bounds(card_w, card_h) {
        if !rect_contains(&rect, mouse) {
            continue;
        }
        match slot {
            DevSlot::Reset => {
                if overlay.reset_btn.handle(mouse, rect, true) {
                    log::info!("dev: reset to defaults");
                    overlay.reset_to_defaults();
                }
            }
            DevSlot::VsyncToggle => {
                if overlay.vsync_btn.handle(mouse, rect, true) {
                    overlay.vsync = !overlay.vsync;
                    log::info!("dev: vsync = {}", overlay.vsync);
                    return true;
                }
            }
            DevSlot::SimError => {
                if overlay.sim_error_btn.handle(mouse, rect, true) {
                    overlay.cycle_sim_error();
                    log::info!("dev: sim_error = {:?}", overlay.sim_error);
                }
            }
            // Sliders consumed by drive_dev_overlay.
            _ => {}
        }
        return false;
    }
    false
}

/// Drive the modal's slider drag + dropdown menu hover + button hover
/// states from the current cursor + mouse-button state. Mirrors
/// `drive_settings_sliders` for the modal's three interactive controls.
fn drive_modal_widgets(
    modal: &mut NewInstanceModalState,
    fonts: Option<&FontStore>,
    mouse: (f32, f32),
    mouse_down: bool,
    card_w: f32,
    card_h: f32,
) {
    let Some(fonts) = fonts else {
        return;
    };
    for (slot, rect) in screens::new_instance_modal::widget_bounds(card_w, card_h, fonts) {
        match slot {
            ModalSlot::Ram => {
                modal.ram.drive(mouse, rect, mouse_down);
            }
            ModalSlot::Cancel => {
                modal.cancel_btn.handle(mouse, rect, false);
            }
            ModalSlot::Create => {
                modal.create_btn.update(mouse, rect, mouse_down, 0.0);
            }
            ModalSlot::Version | ModalSlot::Loader => {}
        }
    }
    // Update menu hover for any open dropdown. Extract just the option
    // count up-front so the `Vec<&str>` borrow ends before we ask for a
    // `&mut VdropState` on the same modal.
    if let Some(slot) = modal.open_dropdown() {
        let opt_count = modal.dropdown_options(slot).map(|v| v.len());
        if let Some(opt_count) = opt_count {
            if let Some(head) = screens::new_instance_modal::widget_bounds(card_w, card_h, fonts)
                .into_iter()
                .find_map(|(s, r)| if s == slot { Some(r) } else { None })
            {
                let (menu_bounds, _flip) =
                    ewo_render::widgets::menu_layout(head, opt_count, card_h);
                if let Some(state) = modal.dropdown_state_mut(slot) {
                    state.update_menu_hover(mouse, menu_bounds, opt_count);
                }
            }
        }
    }
    let _ = fonts;
}

/// Route a press inside the open modal. Returns `true` if the press was
/// consumed (always — the modal absorbs all clicks while open). Calls
/// `modal.close()` when the press lands on the shroud, Cancel button, or
/// a successfully-validated Create button. Create with empty name sets
/// `modal.name_error = true` and keeps the modal open so the user sees
/// the inline error.
#[allow(clippy::too_many_arguments)]
#[allow(clippy::too_many_arguments)]
fn handle_modal_press(
    modal: &mut NewInstanceModalState,
    instances: &mut Vec<Instance>,
    instance_prefs: &mut InstancePrefs,
    versions: &versions::VersionService,
    downloads: &mut downloads::DownloadService,
    fonts: Option<&FontStore>,
    mouse: (f32, f32),
    card_w: f32,
    card_h: f32,
    time: f32,
) -> bool {
    let Some(fonts) = fonts else {
        return true;
    };

    // (0) Name input focus hit-test. Compute the input rect from the
    // shared layout. If the click lands inside, focus it; otherwise
    // unfocus before any other widget dispatch.
    let layout = screens::new_instance_modal::compute_layout(card_w, card_h, fonts);
    if rect_contains(&layout.name_input, mouse) {
        modal.focus_name(true);
        return true;
    } else {
        modal.focus_name(false);
    }

    // (1) Open dropdown menu hit-test first. Snapshot the option strings
    // into an owned `Vec<String>` so the borrow on `modal` ends before
    // we ask for a `&mut VdropState` on the same modal below.
    if let Some(open_slot) = modal.open_dropdown() {
        let opts: Option<Vec<String>> = modal
            .dropdown_options(open_slot)
            .map(|v| v.iter().map(|s| (*s).to_string()).collect());
        if let Some(opts) = opts {
            if let Some(head) =
                screens::new_instance_modal::widget_bounds(card_w, card_h, fonts)
                    .into_iter()
                    .find_map(|(s, r)| if s == open_slot { Some(r) } else { None })
            {
                let (menu_bounds, _flip) =
                    ewo_render::widgets::menu_layout(head, opts.len(), card_h);
                let in_menu = rect_contains(&menu_bounds, mouse);
                let in_head = rect_contains(&head, mouse);
                if in_menu {
                    if let Some(state) = modal.dropdown_state_mut(open_slot) {
                        if let Some(idx) =
                            state.handle_menu(mouse, menu_bounds, opts.len(), true)
                        {
                            log::info!(
                                "modal dropdown {:?} → {} ({})",
                                open_slot, idx, opts[idx]
                            );
                        }
                    }
                    return true;
                }
                if !in_head {
                    if let Some(state) = modal.dropdown_state_mut(open_slot) {
                        state.close();
                    }
                }
            }
        }
    }

    // (2) Form widget dispatch.
    for (slot, rect) in screens::new_instance_modal::widget_bounds(card_w, card_h, fonts) {
        if !rect_contains(&rect, mouse) {
            continue;
        }
        match slot {
            ModalSlot::Version | ModalSlot::Loader => {
                close_other_modal_dropdowns(modal, slot);
                if let Some(state) = modal.dropdown_state_mut(slot) {
                    if state.handle_head(mouse, rect, true) {
                        log::info!("modal dropdown {:?}: open={}", slot, state.open);
                    }
                }
                return true;
            }
            ModalSlot::Cancel => {
                if modal.cancel_btn.handle(mouse, rect, true) {
                    log::info!("modal: Cancel clicked");
                    modal.close();
                }
                return true;
            }
            ModalSlot::Create => {
                modal.create_btn.update(mouse, rect, true, 0.0);
                if let Some(form) = try_submit_modal(modal) {
                    commit_new_instance(instances, instance_prefs, versions, downloads, form, time);
                    modal.close();
                } else {
                    // Empty name → keep modal open; `modal.name_error` is
                    // now `true`, the renderer surfaces the inline message.
                    log::info!("modal: Create blocked — name required");
                }
                return true;
            }
            // Sliders consumed by drive_modal_widgets.
            ModalSlot::Ram => return true,
        }
    }

    // (3) Anywhere else inside the card body → consume but no-op.
    let card = screens::new_instance_modal::card_rect(card_w, card_h);
    if rect_contains(&card, mouse) {
        return true;
    }

    // (4) Shroud click outside the card → dismiss.
    if screens::new_instance_modal::shroud_consumes(mouse, card_w, card_h) {
        log::info!("modal: shroud click → closing");
        modal.close();
        return true;
    }

    let _ = fonts;
    true
}

/// `modal.try_submit()` plus name validation: a name with path separators,
/// `..`, a reserved device name or control characters is rejected the same
/// way a blank one is (inline error, modal stays open).
fn try_submit_modal(
    modal: &mut NewInstanceModalState,
) -> Option<screens::new_instance_modal::NewInstanceForm> {
    let mut form = modal.try_submit()?;
    match persistence::validate_instance_name(&form.name) {
        Ok(name) => {
            form.name = name.to_string();
            Some(form)
        }
        Err(e) => {
            log::info!("modal: Create blocked — invalid name ({:?})", e);
            modal.name_error = true;
            None
        }
    }
}

fn close_other_modal_dropdowns(modal: &mut NewInstanceModalState, keep: ModalSlot) {
    if keep != ModalSlot::Version {
        modal.version.close();
    }
    if keep != ModalSlot::Loader {
        modal.loader.close();
    }
}

/// Insert a new instance at the front of the launcher's list (so it
/// shows first under the default "newest first" sort) and select it.
/// `time` is the current wall-clock seconds; used to drive the row
/// drop-in animation. Called by the Create button + Enter-key paths.
fn commit_new_instance(
    instances: &mut Vec<Instance>,
    prefs: &mut InstancePrefs,
    versions: &versions::VersionService,
    downloads: &mut downloads::DownloadService,
    form: screens::new_instance_modal::NewInstanceForm,
    time: f32,
) {
    let version_meta = format!("{} · {}", form.loader.to_uppercase(), form.version);
    // Map the modal's loader-string back to the typed `InstanceLoader`.
    // "Ewo (development)" → Ewo with the version-line-keyed dev manifest
    // URL (26.2 instances get 26.2.json, etc.); anything else → Vanilla.
    // Other loaders will land here once the dropdown grows back.
    let loader = if form.loader.starts_with("Ewo") {
        ewo_render::screens::instances::InstanceLoader::Ewo {
            manifest_url: ewo_loader_manifest_url(&form.version),
        }
    } else if form.loader.starts_with("Native") {
        // Rewo — spawns the native client at launch. Downloads still run
        // the vanilla profile: Rewo bakes textures/data from the client jar.
        ewo_render::screens::instances::InstanceLoader::Native
    } else {
        ewo_render::screens::instances::InstanceLoader::Vanilla
    };
    // Derived before `loader` is moved into `Instance::with_loader` below.
    // The job needs the manifest URL up front so it can fetch + merge
    // before counting bytes for the progress bar.
    let loader_spec = loader_spec_for(&loader);
    // Seed the instance's mods list from the bundled catalog so the
    // Instances UI shows real toggles immediately. Only Ewo instances get
    // mods — vanilla launches don't run any mods so the list stays empty.
    let seeded_mods = match &loader {
        ewo_render::screens::instances::InstanceLoader::Vanilla
        | ewo_render::screens::instances::InstanceLoader::Native => Vec::new(),
        ewo_render::screens::instances::InstanceLoader::Ewo { .. } => {
            bundled::seed_instance_mods()
        }
    };
    let mut new_inst = Instance::new(
        form.name.clone(),
        version_meta,
        "just now".to_string(),
        seeded_mods,
    )
    .with_config(form.ram, 16, 0)
    .with_loader(loader);
    // A fresh folder id — never the name, so a new instance can't inherit
    // a deleted same-named instance's folder.
    new_inst.id = persistence::new_instance_id(&form.name, instances);
    // Stamp the new world as "just played" so it leads the list in both
    // newest-first and recently-played sorts until the user launches
    // anything else. New instances are Pending until the download job
    // finishes.
    new_inst.last_played_at = screens::instances::current_unix_seconds();
    new_inst.status = ewo_render::screens::instances::InstanceStatus::Pending;
    log::info!(
        "instances: created \"{}\" ({} · {} · {} GB)",
        form.name, form.version, form.loader, form.ram
    );
    instances.insert(0, new_inst);
    // Selection is by underlying index → newly inserted is at 0; previously
    // selected indices shift +1, so update prefs.selected to track it.
    prefs.selected = prefs.selected.saturating_add(1);
    prefs.select(instances, 0);
    prefs.list_scroll = 0.0;
    prefs.created_at = Some(time);
    prefs.selected_at = Some(time);
    persistence::save_instances(instances);

    // Kick off the download job for the selected version. If the master
    // manifest hasn't loaded yet (very-first launch + offline), we just
    // log + leave the instance Pending; user can retry once the
    // manifest fetches. `loader_spec` was derived earlier (before `loader`
    // got moved into the Instance) and feeds the loader manifest URL
    // through so the job can fetch + merge it up front and include
    // loader-added libraries (EwoLoader fat jar + bundled mods) in the
    // progress bar's byte total.
    if let Some(manifest) = versions.manifest() {
        if let Some(entry) = manifest.entry(&form.version) {
            downloads.start(entry.clone(), loader_spec);
        } else {
            log::warn!(
                "instances: version {} not in master manifest — download skipped",
                form.version
            );
        }
    } else {
        log::warn!(
            "instances: master manifest not yet loaded — download deferred for {}",
            form.version
        );
    }
}

fn close_other_dropdowns(prefs: &mut Prefs, keep: SettingsSlot) {
    if keep != SettingsSlot::WindowMode {
        prefs.window_mode.close();
    }
    if keep != SettingsSlot::Theme {
        prefs.theme.close();
    }
    if keep != SettingsSlot::LogLevel {
        prefs.log_level.close();
    }
}

fn update_cursor_icon(
    window: &Window,
    pos: &PhysicalPosition<f64>,
    size: PhysicalSize<u32>,
    scale: f64,
) {
    use winit::window::CursorIcon::*;
    let icon = match hit_test(*pos, size, scale) {
        Some(Zone::Resize(ResizeDirection::North)) => NResize,
        Some(Zone::Resize(ResizeDirection::South)) => SResize,
        Some(Zone::Resize(ResizeDirection::East)) => EResize,
        Some(Zone::Resize(ResizeDirection::West)) => WResize,
        Some(Zone::Resize(ResizeDirection::NorthEast)) => NeResize,
        Some(Zone::Resize(ResizeDirection::NorthWest)) => NwResize,
        Some(Zone::Resize(ResizeDirection::SouthEast)) => SeResize,
        Some(Zone::Resize(ResizeDirection::SouthWest)) => SwResize,
        Some(Zone::Caption) | None => Default,
    };
    window.set_cursor(icon);
}

fn main() {
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info"),
    )
    .init();

    let args = Args::parse();

    if args.mint_rewo_env {
        std::process::exit(mint_rewo_env());
    }
    let event_loop = EventLoop::new().expect("failed to create event loop");
    let mut app = App::new(args.dev);
    event_loop.run_app(&mut app).expect("event loop error");

    // Terminate immediately instead of unwinding `app`. The GL context +
    // transparent-window teardown can deadlock the main thread on some drivers
    // — the window would hang as "Not Responding" on quit (from "Quit to
    // desktop" or the taskbar close). Nothing in our Drop path needs to run
    // (settings / instances persist on change, not on exit), so we let the OS
    // reclaim the window, GL context, and detached background threads.
    std::process::exit(0);
}

/// `--mint-rewo-env`: refresh the active account's Minecraft token from
/// the persisted MS refresh token and print the Rewo env handoff, one
/// `KEY=value` per line on stdout (everything else goes to stderr via the
/// logger). Returns a process exit code.
fn mint_rewo_env() -> i32 {
    let store = auth::persistence::load_store();
    let Some(account) = store.active_account() else {
        log::error!("mint-rewo-env: no active account in auth.toml — sign in via the launcher first");
        return 1;
    };
    log::info!("mint-rewo-env: refreshing Minecraft token for {}…", account.name);
    match auth::chain::run_chain_from_refresh(&account.ms_refresh_token, |stage| {
        log::info!("mint-rewo-env: {}", stage.label());
    }) {
        Ok(fresh) => {
            println!("REWO_USERNAME={}", fresh.name);
            println!("REWO_UUID={}", fresh.uuid);
            println!("REWO_ACCESS_TOKEN={}", fresh.minecraft_token);
            0
        }
        Err(e) => {
            log::error!("mint-rewo-env: auth chain failed: {e:?}");
            1
        }
    }
}

/// Dump the launching screen's in-memory log to
/// `<config>/EwoClient/instances/<id>/logs/<timestamp>.log`.
/// Best-effort — failures log a warning but don't surface. Each line is
/// prefixed with its severity tag so stderr lines stay distinguishable
/// from stdout when grepping.
fn persist_launch_log(
    instance_id: &str,
    instance_name: &str,
    lines: &[ewo_render::screens::launching::RealLogLine],
    exit_code: Option<i32>,
) {
    use std::io::Write;
    if lines.is_empty() {
        return;
    }
    let Some(mut path) = downloads::paths::instance_dir(instance_id) else {
        log::warn!("logs: instance dir unresolvable for {}", instance_name);
        return;
    };
    path.push("logs");
    if let Err(e) = std::fs::create_dir_all(&path) {
        log::warn!("logs: mkdir {} failed: {}", path.display(), e);
        return;
    }
    // Filename uses a sortable timestamp so newest logs sort to the
    // bottom alphabetically.
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    path.push(format!("launch_{}.log", ts));
    let file = match std::fs::File::create(&path) {
        Ok(f) => f,
        Err(e) => {
            log::warn!("logs: create {} failed: {}", path.display(), e);
            return;
        }
    };
    let mut w = std::io::BufWriter::new(file);
    let _ = writeln!(
        w,
        "# EwoClient launch log — instance \"{}\" — exit code {:?}",
        instance_name, exit_code
    );
    let _ = writeln!(w, "# {} lines\n", lines.len());
    for line in lines {
        let tag = match line.severity {
            ewo_render::screens::RealSeverity::Info => "OUT",
            ewo_render::screens::RealSeverity::Warn => "ERR",
        };
        let _ = writeln!(w, "[{}] {:.2}s {}", tag, line.at, line.text);
    }
    log::info!("logs: wrote {} lines to {}", lines.len(), path.display());
}

/// Render an `AuthError` into a user-facing string for the Account tab's
/// detail line. The XSTS variants get the spec-defined messages
/// (region-blocked etc.); other errors get a clean fallback.
///
/// `Other` errors are pattern-matched for known patterns so the user
/// sees something readable instead of raw HTTP-response JSON.
fn format_auth_error(err: &auth::AuthError) -> String {
    match err {
        auth::AuthError::UserCancelled => "sign-in was cancelled.".to_string(),
        auth::AuthError::Network(msg) => format!("network error: {}", msg),
        auth::AuthError::XstsBlocked(b) => b.user_message().to_string(),
        auth::AuthError::NoMinecraftLicense => {
            "this Microsoft account doesn't appear to own Minecraft Java edition.".to_string()
        }
        auth::AuthError::Other(msg) => friendly_other_error(msg),
    }
}

/// Best-effort transform of raw HTTP-response error strings into a clean
/// one-liner. Returns the raw message unchanged if no pattern matches.
fn friendly_other_error(msg: &str) -> String {
    if msg.contains("Invalid app registration") {
        return "this Entra app isn't yet approved by Mojang's Minecraft Launcher \
            Program. apply at aka.ms/AppRegInfo, or use Phase B (no auth) for now."
            .to_string();
    }
    if msg.contains("AADSTS") {
        // Pull just the AADSTS code + short message instead of the full body.
        if let Some(idx) = msg.find("AADSTS") {
            let tail = &msg[idx..];
            let line = tail.split('\n').next().unwrap_or(tail);
            return format!("Microsoft auth: {}", line);
        }
    }
    // Fallback: trim whitespace + cap length so the panel doesn't overflow.
    let trimmed = msg.trim();
    if trimmed.len() > 240 {
        format!("{}…", &trimmed[..240])
    } else {
        trimmed.to_string()
    }
}
