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
use ewo_render::screens::instances::Instance;
use ewo_render::screens::settings::{
    AccountHover, AccountOpView, AccountRequest, AccountRowView, AccountView, KeybindRequest,
    KeybindRowView, KeybindView, LinkStatusView, ProfileHover, ProfileRequest, ProfileRowView,
    ProfileView,
};
use ewo_render::screens::{
    self, AboutModalState, DevOverlayState, DevSlot, FrameStats, FriendRowView, FriendsPrefs,
    FriendsViewState, InstancePrefs, InstanceSlot, LauncherLinkModalState, LaunchingState,
    LinkRedeemView, ModalSlot, NewInstanceModalState, Prefs, SettingsSlot, SettingsTab,
};
use ewo_render::skia_safe;
use ewo_render::text::HoverGlowState;
use ewo_render::{app_window, Clock, FontStore, GlBackend, VbtnState};

use auth::{AuthOp, AuthService};
use instance_ops::{delete_instance, loader_spec_for, sync_instance_config};
use launch_flow::{
    launch_meta, persist_launch_log, PendingAuthLaunch, PendingPrepare, PendingRelaunch,
    PrepareFail,
};
use view_models::{format_auth_error, friend_entry_to_view, screen_name};
use window_hit::*;

use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{CursorIcon, ResizeDirection, Window, WindowId};

mod auth;
mod bundled;
mod downloads;
mod events;
mod instance_ops;
mod keybind;
mod launch;
mod launch_flow;
mod launch_slot;
mod loaders;
mod overlay_mods;
mod persistence;
mod profile;
mod runtime;
mod social;
mod util;
mod versions;
mod view_models;
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
    /// Re-reads the OS "show animations" setting (non-negotiable #6); see
    /// `ewo_core::motion` and `window::prefers_reduced_motion`.
    motion_watch: ewo_core::motion::Watcher,
    /// Force the next frame to re-read it (set on focus gain).
    motion_recheck: bool,
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
    /// Frogsy social state (Phase H): link redemption, presence, friends,
    /// network status. Off unless `EWO_BOT_API_BASE` is configured.
    /// Polled each frame the same way `auth` is.
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
    /// A launch waiting on a Minecraft-token refresh. See
    /// [`PendingAuthLaunch`].
    pending_auth: Option<PendingAuthLaunch>,
    /// After a bundled JRE lands, the JRE re-scan (`java -version` per
    /// candidate, `where java`) runs on a worker; it reports the major it
    /// was for here, and the pending relaunch retries then.
    jre_rescan_rx: Option<std::sync::mpsc::Receiver<u32>>,
    /// The Java-runtime dropdown's live option list (Auto first), mirrored
    /// into `instance_prefs.jre_runtime_labels` each frame. Starts as just
    /// the Auto entry; the `ewo-jre-warm` thread delivers the detected
    /// list through `jre_labels_rx`, and a finished bundled-JRE rescan
    /// refreshes it.
    jre_labels: Vec<String>,
    /// Delivery channel for the warm-up scan's label list. Drained once
    /// (the scan is cached in a `OnceLock`); `None` after delivery.
    jre_labels_rx: Option<std::sync::mpsc::Receiver<Vec<String>>>,
    /// Last 10 % bucket of the runtime download logged to the launch log,
    /// so each bucket is logged once.
    runtime_log_bucket: u32,
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
            motion_watch: ewo_core::motion::Watcher::new(),
            motion_recheck: false,
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
            // defaults if missing or malformed. An Ewo instance's stored
            // loader value is never used to fetch: the manifest URL is
            // resolved from the version and this machine's loader base at
            // launch (`loaders::base`), so there is nothing to re-key here.
            instances: persistence::load_instances(),
            instance_prefs: InstancePrefs::default(),
            launching: LaunchingState::default(),
            modal: NewInstanceModalState::default(),
            about_modal: AboutModalState::default(),
            launcher_link_modal: LauncherLinkModalState::default(),
            friends_prefs: FriendsPrefs::default(),
            dev_overlay: if dev {
                Some(DevOverlayState::default())
            } else {
                None
            },
            launch_button: VbtnState::default(),
            menu_items: [VbtnState::default(); 4],
            heading_hover: HoverGlowState::default(),
            // AuthService loads the persisted account store and kicks a
            // silent refresh for the active account itself (see `new`).
            auth: AuthService::new(),
            // SocialState reads `EWO_BOT_API_BASE`; unset = disabled.
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
            pending_auth: None,
            jre_rescan_rx: None,
            jre_labels: screens::instances::java_runtime_options(&[]),
            jre_labels_rx: None,
            runtime_log_bucket: 0,
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

    /// Map the active account's Frogsy link state into the renderer-side
    /// `LinkStatusView`. `Hidden` when social isn't configured or there's
    /// no active account; otherwise the stored `social_token` alone
    /// decides (no network probe, see `social`'s module docs): linked, or
    /// the clickable line that opens the launcher-link modal.
    fn link_status_view(&self) -> LinkStatusView {
        if !self.social.enabled() {
            return LinkStatusView::Hidden;
        }
        let Some(active) = self.auth.active() else {
            return LinkStatusView::Hidden;
        };
        if self.auth.social_token(&active.uuid).is_some() {
            LinkStatusView::Linked
        } else {
            LinkStatusView::LinkedNeedsLauncherLink
        }
    }

    /// The active account's `social_token`, if it has one.
    fn active_social_token(&self) -> Option<String> {
        let active = self.auth.active()?;
        self.auth.social_token(&active.uuid).map(str::to_string)
    }

    /// Card-local bounds of the main-menu network widget, grown by the
    /// online-name strip when there are names to show. One source for the
    /// hover and click rects so they match what `draw_main_menu` paints.
    fn server_widget_rect(&self, card_w: f32, card_h: f32) -> skia_safe::Rect {
        let rows = self.social.server_status().map_or(0, |s| {
            screens::server_widget_name_rows(s.online, s.players.len())
        });
        screens::server_widget_bounds(card_w, card_h, rows)
    }

    /// Arriving at Friends refetches the list immediately rather than
    /// waiting for the next 30 s cadence tick. Goes through the same gate
    /// as every social request: with social disabled, or no linked
    /// account, `refresh_friends_now` issues nothing.
    fn refresh_friends_on_arrival(&mut self) {
        if let Some(token) = self.active_social_token() {
            self.social.refresh_friends_now(&token);
        }
    }

    /// The main-menu network widget exists only while its status poll can
    /// run: social configured and the active account linked.
    fn server_widget_visible(&self) -> bool {
        self.social.enabled() && self.active_social_token().is_some()
    }

    /// Bounds of the active screen's Launch button (card-local). Returns
    /// `None` for screens that don't have one.
    fn launch_button_bounds(&self, card_w: f32) -> Option<skia_safe::Rect> {
        match self.screen {
            Screen::Instances => Some(screens::instances::launch_button_bounds(card_w)),
            _ => None,
        }
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

        // Runtime taskbar/alt-tab icon for the borderless window (the
        // embedded .ico serves Explorer only). None → OS default icon;
        // a missing asset never blocks startup.
        let attrs = match window::icon::load_app_icon() {
            Some(icon) => attrs.with_window_icon(Some(icon)),
            None => {
                log::warn!("app icon not found — taskbar shows the default icon");
                attrs
            }
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
        // The scan's side product, the label strings for the per-instance
        // Java-runtime dropdown, comes back over a channel so the UI never
        // calls `labels()` synchronously either.
        let (jre_tx, jre_rx) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("ewo-jre-warm".into())
            .spawn(move || {
                let n = launch::detect_jres().len();
                log::info!("jre warm: {n} runtime(s) detected (cache primed)");
                let _ = jre_tx.send(launch::jre::labels());
            })
            .expect("spawn jre-warm thread");
        self.jre_labels_rx = Some(jre_rx);

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

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
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
                    // The user may have flipped Windows "Show animations"
                    // while we were in the background.
                    self.motion_recheck = true;
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

            WindowEvent::MouseInput {
                state,
                button: MouseButton::Left,
                ..
            } => {
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
        // GPU device lost: `GlBackend::render` can draw nothing until its
        // recreation backoff (up to 30 s) elapses, so polling (or even the
        // 100 ms unfocused cadence) would only burn a core. Sleep until the
        // next attempt; input events still wake the loop.
        if let Some(retry_at) = self.backend.as_ref().and_then(|b| b.retry_deadline()) {
            if retry_at > Instant::now() {
                event_loop.set_control_flow(ControlFlow::WaitUntil(retry_at));
                return;
            }
        }

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
        prefs
            .keybind_reset
            .handle(probe, layout.reset_button, false);
        return false;
    }
    // Modules tab — toggle hover + the FOV slider, within the scrolled list.
    if tab == SettingsTab::Modules {
        let layout =
            screens::settings::modules_tab_layout(fonts, card_w, card_h, prefs.settings_scroll);
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
        let layout =
            screens::settings::pvp_tab_layout(fonts, card_w, card_h, prefs.settings_scroll);
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
        let layout =
            screens::settings::modules_tab_layout(fonts, card_w, card_h, prefs.settings_scroll);
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
        let consumed =
            screens::settings::pvp_tab_press(prefs, fonts, card_w, card_h, mouse.0, mouse.1);
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
                        if let Some(idx) = state.handle_menu(mouse, menu_bounds, opts.len(), true) {
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
    for (slot, rect) in screens::instances::widget_bounds(card_w, card_h, fonts, prefs, instances) {
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
        // Immutable pass first (the options borrow prefs), then the
        // mutable hover update.
        let prep = screens::instances::dropdown_options(slot, prefs).and_then(|opts| {
            screens::instances::dropdown_head_for_slot(
                slot, fonts, card_w, card_h, prefs, instances,
            )
            .map(|head| {
                let (menu_bounds, _flip) =
                    ewo_render::widgets::menu_layout(head, opts.len(), card_h);
                (menu_bounds, opts.len())
            })
        });
        if let Some((menu_bounds, n)) = prep {
            if let Some(state) = prefs.dropdown_state_mut(slot) {
                state.update_menu_hover(mouse, menu_bounds, n);
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
    instances: &mut [Instance],
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
        // Immutable pass: head geometry + row count end before the
        // mutable state updates below.
        let prep = screens::instances::dropdown_options(open_slot, prefs).and_then(|opts| {
            screens::instances::dropdown_head_for_slot(
                open_slot, fonts, card_w, card_h, prefs, instances,
            )
            .map(|head| {
                let (menu_bounds, _flip) =
                    ewo_render::widgets::menu_layout(head, opts.len(), card_h);
                (head, menu_bounds, opts.len())
            })
        });
        if let Some((head, menu_bounds, n)) = prep {
            let in_menu = rect_contains(&menu_bounds, mouse);
            let in_head = rect_contains(&head, mouse);
            if in_menu {
                let mut commit = false;
                if let Some(state) = prefs.dropdown_state_mut(open_slot) {
                    if let Some(idx) = state.handle_menu(mouse, menu_bounds, n, true) {
                        log::info!(
                            "instance dropdown {:?} → {} ({})",
                            open_slot,
                            idx,
                            prefs
                                .jre_runtime_labels
                                .get(idx)
                                .map_or("?", String::as_str)
                        );
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

    // (3) Normal widget dispatch.
    for (slot, rect) in screens::instances::widget_bounds(card_w, card_h, fonts, prefs, instances) {
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
            DevSlot::SimError if overlay.sim_error_btn.handle(mouse, rect, true) => {
                overlay.cycle_sim_error();
                log::info!("dev: sim_error = {:?}", overlay.sim_error);
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
            if let Some(head) = screens::new_instance_modal::widget_bounds(card_w, card_h, fonts)
                .into_iter()
                .find_map(|(s, r)| if s == open_slot { Some(r) } else { None })
            {
                let (menu_bounds, _flip) =
                    ewo_render::widgets::menu_layout(head, opts.len(), card_h);
                let in_menu = rect_contains(&menu_bounds, mouse);
                let in_head = rect_contains(&head, mouse);
                if in_menu {
                    if let Some(state) = modal.dropdown_state_mut(open_slot) {
                        if let Some(idx) = state.handle_menu(mouse, menu_bounds, opts.len(), true) {
                            log::info!("modal dropdown {:?} → {} ({})", open_slot, idx, opts[idx]);
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
            manifest_url: loaders::base::stored_marker(&form.version),
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
    let loader_spec = loader_spec_for(&loader, &form.version);
    // Seed the instance's mods list from the bundled catalog so the
    // Instances UI shows real toggles immediately. Only Ewo instances get
    // mods — vanilla launches don't run any mods so the list stays empty.
    let seeded_mods = match &loader {
        ewo_render::screens::instances::InstanceLoader::Vanilla
        | ewo_render::screens::instances::InstanceLoader::Native => Vec::new(),
        ewo_render::screens::instances::InstanceLoader::Ewo { .. } => bundled::seed_instance_mods(),
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
        form.name,
        form.version,
        form.loader,
        form.ram
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
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let args = Args::parse();

    if args.mint_rewo_env {
        std::process::exit(mint_rewo_env());
    }
    // Reduced motion must be known before the first frame is built.
    let reduced = window::prefers_reduced_motion();
    ewo_core::motion::set_reduced(reduced);
    log::info!("reduced motion: {}", if reduced { "on" } else { "off" });
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
        log::error!(
            "mint-rewo-env: no active account in auth.toml — sign in via the launcher first"
        );
        return 1;
    };
    log::info!(
        "mint-rewo-env: refreshing Minecraft token for {}…",
        account.name
    );
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
