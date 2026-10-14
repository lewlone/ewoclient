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
use cli::{mint_rewo_env, Args};
use instance_ops::{delete_instance, loader_spec_for, sync_instance_config};
use launch_flow::{
    persist_launch_log, PendingAuthLaunch, PendingPrepare, PendingRelaunch, PrepareFail,
};
use view_models::{format_auth_error, friend_entry_to_view, screen_name};
use window_hit::*;

use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event::{ElementState, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{CursorIcon, Window, WindowId};

mod app;
mod auth;
mod bundled;
mod cli;
mod downloads;
mod events;
mod frame_pacing;
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
mod window_init;

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
    /// Wall-time the running game was spawned, whether a visible window was
    /// ever **observed** for it (`launch::reaper::state`, polled while it
    /// runs), whether its log merely claimed one is coming (a hint), and when
    /// it was last polled. Together they tell a zombie from a game that is
    /// still starting up (see `launch::reaper::is_zombie`).
    active_launch_started_at: f32,
    active_launch_window_seen: bool,
    active_launch_marker_seen: bool,
    active_launch_polled_at: f32,
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

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        self.create_window(event_loop);
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
        self.pace_frames(event_loop);
    }
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
