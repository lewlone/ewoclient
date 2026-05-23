//! EwoClient launcher binary entry point.
//!
//! Build-sequence steps 4 + 5 in flight: pearl dust particle system + velvet
//! folds layer. See `CLAUDE.md` for the full build sequence.

use std::sync::Arc;
use std::time::{Duration, Instant};

use clap::Parser;
use ewo_core::{Screen, Settings, Theme};
use ewo_render::backdrop::Backdrop;
use ewo_render::screens::settings::{
    AccountHover, AccountOpView, AccountRequest, AccountRowView, AccountView, KeybindRequest,
    KeybindRowView, KeybindView, ProfileHover, ProfileRequest, ProfileRowView, ProfileView,
};
use ewo_render::screens::{
    self, AboutModalState, DevOverlayState, DevSlot, FrameStats, InstancePrefs, InstanceSlot,
    LaunchingState, ModalSlot, NewInstanceModalState, Prefs, SettingsSlot, SettingsTab,
};
use ewo_render::screens::instances::Instance;
use ewo_render::skia_safe;
use ewo_render::text::HoverGlowState;
use ewo_render::{app_window, Clock, FontStore, GlBackend, VbtnState};

use auth::{AuthOp, AuthService};

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
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent};
use winit::keyboard::{Key, NamedKey};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{CursorIcon, ResizeDirection, Window, WindowId};

mod auth;
mod bundled;
mod downloads;
mod keybind;
mod launch;
mod loaders;
mod overlay_mods;
mod persistence;
mod profile;
mod runtime;
mod util;
mod versions;
mod window;

#[derive(Parser, Debug)]
#[command(name = "ewolauncher", about = "EwoClient — Velvet & Pearl")]
struct Args {
    /// Show the developer overlay (state-picker, layout-picker, tweaks panel).
    #[arg(long)]
    dev: bool,
}

const RESIZE_BORDER_LP: f64 = 8.0;
const CAPTION_HEIGHT_LP: f64 = 32.0;

/// Hard-coded URL for the in-development EwoLoader manifest. The loader
/// project lives in a sibling repo on the developer's machine and
/// doesn't yet publish a public meta endpoint, so we point straight at
/// the on-disk manifest via `file://`. Becomes a config knob (or a real
/// HTTPS URL) once the loader publishes a meta endpoint.
const DEV_EWO_LOADER_URL: &str =
    "file:///C:/Users/valtteri/Desktop/EwoLoaderV1/manifest/0.1.0/26.1.json";

// Card inset (logical px). Mirrors `app_window::CARD_INSET`. Used to convert
// cursor positions from window-local to card-local for widget hit-testing.
const CARD_INSET_LP: f64 = 28.0;

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
    screen: Screen,
    settings_tab: SettingsTab,
    prefs: Prefs,
    instances: Vec<Instance>,
    instance_prefs: InstancePrefs,
    launching: LaunchingState,
    modal: NewInstanceModalState,
    about_modal: AboutModalState,
    dev_overlay: Option<DevOverlayState>,
    launch_button: VbtnState,
    menu_items: [VbtnState; 4],
    /// Hover-tracked state for the main-menu "EwoClient" heading. Drives
    /// the per-glyph hover-glow stagger (CSS `bt-hover-glow`).
    heading_hover: HoverGlowState,
    /// Microsoft auth service — owns the worker thread + auth state.
    /// Polled each frame via `auth.poll()` to drain the event channel.
    auth: AuthService,
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
            // defaults if missing or malformed.
            instances: persistence::load_instances(),
            instance_prefs: InstancePrefs::default(),
            launching: LaunchingState::default(),
            modal: NewInstanceModalState::default(),
            about_modal: AboutModalState::default(),
            dev_overlay: if dev { Some(DevOverlayState::default()) } else { None },
            launch_button: VbtnState::default(),
            menu_items: [VbtnState::default(); 4],
            heading_hover: HoverGlowState::default(),
            // AuthService loads the persisted account store and kicks a
            // silent refresh for the active account itself (see `new`).
            auth: AuthService::new(),
            profiles: profile::list(),
            active_profile: profile::active_name(),
            keybinds: profile::load_keybinds(),
            keybind_capture: None,
            versions: versions::VersionService::new(),
            downloads: downloads::DownloadService::new(),
            launch_rx: None,
            runtime: runtime::RuntimeService::new(),
            pending_relaunch: None,
            celebrate_until: None,
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

    /// Bounds of the active screen's Launch button (card-local). Returns
    /// `None` for screens that don't have one.
    fn launch_button_bounds(&self, card_w: f32) -> Option<skia_safe::Rect> {
        match self.screen {
            Screen::Instances => Some(screens::instances::launch_button_bounds(card_w)),
            _ => None,
        }
    }

    /// Attempt a real JVM launch for the instance at `idx`. Returns
    /// `true` if a real launch started; `false` if we should fall back
    /// to the synthetic path (e.g. instance not Ready, manifest missing
    /// from cache, plan-build failed). On success: stores the receiver
    /// in `self.launch_rx`, transitions `self.launching` into real
    /// mode, and the per-frame poll picks up subsequent events.
    fn try_real_launch(&mut self, idx: usize, inst_name: &str, inst_meta: &str, time: f32) -> bool {
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
        overlay_mods::write_keybinds(&inst.name);
        // The version *string* comes from the meta, formatted as
        // "<LOADER> · <version>". Strip the loader prefix.
        let version_id = inst.version.rsplit(" · ").next().unwrap_or(&inst.version);
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
        // Per-version manifest must be on disk (Phase B). If somehow
        // it isn't, refuse to launch — caller falls back to synthetic.
        let vanilla_pv = match versions::per_version_fetch::get_or_fetch(&entry) {
            Ok(p) => p,
            Err(e) => {
                log::warn!("launch: per-version fetch failed: {}", e);
                return false;
            }
        };
        // Phase D: layer the instance's loader on top of vanilla, if any.
        // Loader-fetch failures are non-fatal — we log + fall back to
        // launching the vanilla profile so the user isn't blocked by a
        // flaky local manifest.
        let mut pv = match &inst.loader {
            ewo_render::screens::instances::InstanceLoader::Vanilla => vanilla_pv,
            ewo_render::screens::instances::InstanceLoader::Ewo { manifest_url } => {
                match loaders::get_or_fetch("ewo", manifest_url) {
                    Ok(loader_manifest) => {
                        log::info!(
                            "launch: merging EwoLoader manifest \"{}\" on top of {}",
                            loader_manifest.id, version_id
                        );
                        loaders::merge(&vanilla_pv, &loader_manifest)
                    }
                    Err(e) => {
                        log::warn!(
                            "launch: EwoLoader fetch failed ({}) — launching vanilla {}",
                            e, version_id
                        );
                        vanilla_pv
                    }
                }
            }
        };
        // Phase D follow-on: download any library the merge added that
        // wasn't in the vanilla `PerVersion` Phase B saw at instance-setup
        // time (the EwoLoader fat jar + bundled mods). Idempotent —
        // `ensure_libraries` skips files already on disk. Runs against the
        // *full* merged library set so disabled mods stay downloaded — the
        // user re-enabling a mod doesn't trigger a re-download.
        if let Err(e) = downloads::ensure_libraries(&pv) {
            log::warn!("launch: loader library fetch failed: {} — falling back", e);
            return false;
        }
        // Per-instance mod toggles: strip libraries the user disabled from
        // the merged classpath. The corresponding mod ids also feed the
        // -Dfabric.debug.disableModIds JVM arg below so the loader's
        // BundledMods verification skips them. Order matters — strip must
        // happen after ensure_libraries (we still want disabled mods on
        // disk for cheap re-enable) but before launch::build (which reads
        // pv.libraries to assemble the classpath).
        let disabled_mod_ids = bundled::disabled_mod_ids(&inst.mods);
        if !disabled_mod_ids.is_empty() {
            use std::collections::HashSet;
            let disabled_libs: HashSet<&str> =
                bundled::library_names_for_disabled(&disabled_mod_ids)
                    .into_iter()
                    .collect();
            let before = pv.libraries.len();
            pv.libraries
                .retain(|l| !disabled_libs.contains(l.name.as_str()));
            log::info!(
                "launch: disabling {} mod(s) [{}] — stripped {} libraries from classpath",
                disabled_mod_ids.len(),
                disabled_mod_ids.join(","),
                before - pv.libraries.len()
            );
        }
        if let Err(e) = launch::extract_all(&pv, &inst.name) {
            log::warn!("launch: native extraction failed: {} — falling back", e);
            return false;
        }
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
                // Kick off the bundled-JRE download, switch the
                // launching screen into "downloading runtime" mode, and
                // record this launch as pending. The per-frame runtime
                // poll will retry once the fetch completes.
                self.launching.enter_real(time, inst_name, inst_meta);
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
                    instance_idx: idx,
                    instance_name: inst_name.to_string(),
                    instance_meta: inst_meta.to_string(),
                    waiting_for_major: required_major,
                });
                // Treat this as a "real" launch path so the caller
                // doesn't fall back to synthetic — we've already
                // populated the launching screen with our own status.
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
        let mut plan = match launch::build(&pv, &inst.name, inst.ram, &profile, jvm_path) {
            Ok(p) => p,
            Err(e) => {
                log::warn!("launch: plan build failed: {} — falling back", e);
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
        let (tx, rx) = std::sync::mpsc::channel::<launch::LaunchEvent>();
        let _ = launch::spawn_jvm(plan, tx);
        self.launch_rx = Some(rx);
        self.launching.enter_real(time, inst_name, inst_meta);
        log::info!(
            "launch: real JVM spawned for \"{}\" ({})",
            inst.name, version_id
        );
        true
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let attrs = Window::default_attributes()
            .with_title("EwoClient")
            .with_decorations(false)
            .with_inner_size(LogicalSize::new(1180.0, 720.0))
            .with_min_inner_size(LogicalSize::new(800.0, 520.0));

        let win = Arc::new(
            event_loop
                .create_window(attrs)
                .expect("failed to create window"),
        );

        window::configure(&win);

        let backend = GlBackend::new(event_loop, win.clone());
        let size = win.inner_size();
        let (card_w, card_h) = app_window::card_content_size(size.width, size.height);
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

        match event {
            WindowEvent::CloseRequested => event_loop.exit(),

            WindowEvent::Focused(focused) => {
                self.focused = focused;
                if focused {
                    // Coming back from unfocused — kick off a fresh redraw
                    // so animations resume immediately.
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
                // Keybind capture — the Keybinds tab armed a rebind, so the
                // next key press becomes the new binding. Esc cancels; a key
                // GLFW can't name leaves the rebind armed.
                if let Some(action_id) = self.keybind_capture.clone() {
                    if logical_key == Key::Named(NamedKey::Escape) {
                        log::info!("keybind: capture cancelled");
                        self.keybind_capture = None;
                    } else if let winit::keyboard::PhysicalKey::Code(code) = physical_key {
                        if let Some(chord) = keybind::KeyChord::from_winit(code) {
                            log::info!("keybind: {} → {}", action_id, chord.label());
                            self.keybinds.insert(action_id, chord);
                            profile::save_keybinds(&self.keybinds);
                            self.keybind_capture = None;
                        }
                    }
                    window.request_redraw();
                    return;
                }
                if self.instance_prefs.renaming {
                    // Inline rename for the selected instance.
                    const RENAME_MAX_LEN: usize = 48;
                    if logical_key == Key::Named(NamedKey::Escape) {
                        log::info!("rename: Esc → cancelled");
                        self.instance_prefs.renaming = false;
                        self.instance_prefs.rename_buffer.clear();
                    } else if logical_key == Key::Named(NamedKey::Enter) {
                        let trimmed = self.instance_prefs.rename_buffer.trim();
                        if !trimmed.is_empty() {
                            if let Some(inst) =
                                self.instances.get_mut(self.instance_prefs.selected)
                            {
                                log::info!(
                                    "rename: \"{}\" → \"{}\"",
                                    inst.name, trimmed
                                );
                                inst.name = trimmed.to_string();
                            }
                            persistence::save_instances(&self.instances);
                        } else {
                            log::info!("rename: empty → cancelled");
                        }
                        self.instance_prefs.renaming = false;
                        self.instance_prefs.rename_buffer.clear();
                    } else if logical_key == Key::Named(NamedKey::Backspace) {
                        self.instance_prefs.rename_buffer.pop();
                        self.instance_prefs.rename_focus_time = 0.0;
                    } else if let Some(t) = text {
                        for ch in t.chars() {
                            if !ch.is_control()
                                && self.instance_prefs.rename_buffer.chars().count() < RENAME_MAX_LEN
                            {
                                self.instance_prefs.rename_buffer.push(ch);
                                self.instance_prefs.rename_focus_time = 0.0;
                            }
                        }
                    }
                } else if self.prefs.profile_renaming.is_some() {
                    // Inline rename for a client profile (Settings → Profiles).
                    if logical_key == Key::Named(NamedKey::Escape) {
                        log::info!("profile rename: Esc → cancelled");
                        self.prefs.profile_renaming = None;
                        self.prefs.profile_rename_buffer.clear();
                    } else if logical_key == Key::Named(NamedKey::Enter) {
                        if let Some(idx) = self.prefs.profile_renaming {
                            self.prefs.profile_request = Some(ProfileRequest::Rename {
                                index: idx,
                                new_name: self.prefs.profile_rename_buffer.clone(),
                            });
                        }
                        self.prefs.profile_renaming = None;
                    } else if logical_key == Key::Named(NamedKey::Backspace) {
                        self.prefs.profile_rename_buffer.pop();
                        self.prefs.profile_rename_focus_time = 0.0;
                    } else if let Some(t) = text {
                        for ch in t.chars() {
                            // Skip control + path-unsafe chars — the name
                            // becomes a directory under `profiles/`.
                            if !ch.is_control()
                                && !"/\\:*?\"<>|".contains(ch)
                                && self.prefs.profile_rename_buffer.chars().count()
                                    < profile::MAX_NAME_LEN
                            {
                                self.prefs.profile_rename_buffer.push(ch);
                                self.prefs.profile_rename_focus_time = 0.0;
                            }
                        }
                    }
                } else if logical_key == Key::Named(NamedKey::Escape) && self.about_modal.open {
                    log::info!("about: Esc → closing");
                    self.about_modal.close();
                } else if logical_key == Key::Named(NamedKey::Escape) && self.modal.open {
                    log::info!("modal: Esc → closing");
                    self.modal.close();
                } else if self.modal.open && self.modal.name_focused {
                    // Name field text input — append printable chars, pop on
                    // backspace. Only when the name field is the active
                    // focus target (set by clicking the input).
                    const NAME_MAX_LEN: usize = 48;
                    if logical_key == Key::Named(NamedKey::Backspace) {
                        self.modal.name.pop();
                        self.modal.name_focus_time = 0.0; // restart caret blink
                        if !self.modal.name.is_empty() {
                            self.modal.name_error = false;
                        }
                    } else if logical_key == Key::Named(NamedKey::Enter) {
                        // Enter commits — same as clicking Create.
                        if let Some(form) = self.modal.try_submit() {
                            commit_new_instance(
                                &mut self.instances,
                                &mut self.instance_prefs,
                                &self.versions,
                                &mut self.downloads,
                                form,
                                self.clock.elapsed,
                            );
                            self.modal.close();
                        }
                    } else if let Some(t) = text {
                        for ch in t.chars() {
                            if !ch.is_control() && self.modal.name.chars().count() < NAME_MAX_LEN {
                                self.modal.name.push(ch);
                                self.modal.name_focus_time = 0.0;
                                self.modal.name_error = false;
                            }
                        }
                    }
                }
            }

            WindowEvent::MouseWheel { delta, .. } => {
                let scale = window.scale_factor();
                let card_pos = cursor_card_local(self.cursor, scale);
                let size = window.inner_size();
                let card_w = card_content_width(size, scale);
                let card_h = card_content_height(size, scale);

                // Convert delta to logical pixels. `LineDelta` lines are
                // platform-dependent — multiply by 32px to get a roughly
                // right wheel-tick-to-pixels mapping. `PixelDelta` is
                // already in physical pixels — divide by scale.
                let dy: f32 = match delta {
                    MouseScrollDelta::LineDelta(_, y) => -y * 32.0,
                    MouseScrollDelta::PixelDelta(p) => -(p.y as f32 / scale as f32),
                };

                // Routing priority:
                //  1. If a dropdown menu is open *anywhere*, route the
                //     wheel to it (so users can scroll the open list).
                //  2. If the modal is open, absorb the wheel even when no
                //     menu is open — prevents the underlying Instances
                //     panel from scrolling beneath a modal.
                //  3. Otherwise, scroll the active screen's primary
                //     scrollable region (currently only the Instances
                //     detail panel).
                let fonts = self.fonts.as_ref();
                let mut handled = false;

                // About modal absorbs wheel events outright — there's nothing
                // scrollable inside it, but we don't want the Instances panel
                // to scroll behind the dialog.
                if self.about_modal.open {
                    let _ = dy;
                    return;
                }

                if let Some(fonts) = fonts {
                    // Modal dropdown takes precedence when modal is open.
                    if self.modal.open {
                        if let Some(slot) = self.modal.open_dropdown() {
                            // Extract just the count so the `Vec<&str>` borrow on
                            // `self.modal` ends before the `&mut VdropState`.
                            let opt_count = self.modal.dropdown_options(slot).map(|v| v.len());
                            if let Some(opt_count) = opt_count {
                                let layout = screens::new_instance_modal::compute_layout(
                                    card_w, card_h, fonts,
                                );
                                let head = match slot {
                                    ModalSlot::Version => layout.version_head,
                                    ModalSlot::Loader => layout.loader_head,
                                    _ => layout.version_head,
                                };
                                let (menu_bounds, _flip) =
                                    ewo_render::widgets::menu_layout(head, opt_count, card_h);
                                if let Some(state) = self.modal.dropdown_state_mut(slot) {
                                    state.scroll_by(dy, menu_bounds, opt_count);
                                }
                                handled = true;
                            }
                        }
                    } else {
                        // Settings dropdown
                        if !handled && self.screen == Screen::Settings {
                            if let Some(slot) = self.prefs.open_dropdown() {
                                if let Some(opts) = screens::settings::dropdown_options(slot) {
                                    if let Some(head) = screens::settings::dropdown_head_for_slot(
                                        slot, fonts, card_w, card_h,
                                    ) {
                                        let (menu_bounds, _flip) =
                                            ewo_render::widgets::menu_layout(
                                                head,
                                                opts.len(),
                                                card_h,
                                            );
                                        if let Some(state) = self.prefs.dropdown_state_mut(slot) {
                                            state.scroll_by(dy, menu_bounds, opts.len());
                                        }
                                        handled = true;
                                    }
                                }
                            }
                        }
                        // Instances dropdown
                        if !handled && self.screen == Screen::Instances {
                            if let Some(slot) = self.instance_prefs.open_dropdown() {
                                if let Some(opts) = screens::instances::dropdown_options(slot) {
                                    if let Some(head) = screens::instances::dropdown_head_for_slot(
                                        slot,
                                        fonts,
                                        card_w,
                                        card_h,
                                        &self.instance_prefs,
                                        &self.instances,
                                    ) {
                                        let (menu_bounds, _flip) =
                                            ewo_render::widgets::menu_layout(
                                                head,
                                                opts.len(),
                                                card_h,
                                            );
                                        if let Some(state) =
                                            self.instance_prefs.dropdown_state_mut(slot)
                                        {
                                            state.scroll_by(dy, menu_bounds, opts.len());
                                        }
                                        handled = true;
                                    }
                                }
                            }
                        }
                    }
                }

                // Modal absorbs all remaining wheel events so the
                // background doesn't scroll beneath it.
                if !handled && self.modal.open {
                    handled = true;
                }

                // Fall-through: Worlds list (left) or Instances detail
                // panel (right) scroll, depending on which side the cursor
                // is on.
                if !handled && self.screen == Screen::Instances {
                    if let Some(fonts) = fonts {
                        if card_pos.0 < 320.0 {
                            // Cursor is over the Worlds list column.
                            let max_scroll = screens::instances::list_max_scroll(
                                card_h,
                                fonts,
                                &self.instances,
                            );
                            self.instance_prefs.list_scroll =
                                (self.instance_prefs.list_scroll + dy).clamp(0.0, max_scroll);
                        } else {
                            let panel =
                                screens::instances::detail_panel_bounds(card_w, card_h);
                            if rect_contains(&panel, card_pos) {
                                let max_scroll = screens::instances::detail_max_scroll(
                                    card_w,
                                    card_h,
                                    fonts,
                                    &self.instance_prefs,
                                    &self.instances,
                                );
                                self.instance_prefs.detail_scroll = (self.instance_prefs.detail_scroll + dy)
                                    .clamp(0.0, max_scroll);
                            }
                        }
                    }
                }

                // Settings — scroll the Keybinds / Modules tab list.
                if !handled && self.screen == Screen::Settings {
                    if let Some(fonts) = fonts {
                        let (content_h, visible_h) = match self.settings_tab {
                            SettingsTab::Keybinds => {
                                let l = screens::settings::keybinds_tab_layout(
                                    fonts,
                                    card_w,
                                    card_h,
                                    keybind::REGISTRY.len(),
                                    self.prefs.settings_scroll,
                                );
                                (l.content_h, l.list_region.height())
                            }
                            SettingsTab::Modules => {
                                let l = screens::settings::modules_tab_layout(
                                    fonts,
                                    card_w,
                                    card_h,
                                    self.prefs.settings_scroll,
                                );
                                (l.content_h, l.list_region.height())
                            }
                            SettingsTab::PvpUtils => {
                                let l = screens::settings::pvp_tab_layout(
                                    fonts,
                                    card_w,
                                    card_h,
                                    self.prefs.settings_scroll,
                                );
                                (l.content_h, l.list_region.height())
                            }
                            _ => (0.0, 1.0),
                        };
                        let max = (content_h - visible_h).max(0.0);
                        self.prefs.settings_scroll =
                            (self.prefs.settings_scroll + dy).clamp(0.0, max);
                    }
                }
            }

            WindowEvent::Resized(size) => {
                if let Some(backend) = self.backend.as_mut() {
                    backend.resize(size.width, size.height);
                }
                if let Some(backdrop) = self.backdrop.as_mut() {
                    let (cw, ch) = app_window::card_content_size(size.width, size.height);
                    backdrop.resize(cw, ch, &self.settings);
                }
                window.request_redraw();
            }

            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = position;
                let scale = window.scale_factor();
                let card_pos = cursor_card_local(position, scale);
                let size = window.inner_size();
                let card_w = card_content_width(size, scale);
                let card_h = card_content_height(size, scale);
                let time = self.clock.elapsed;

                // When a modal is open it absorbs all hover state so the
                // background screen doesn't react under it. Otherwise drive
                // the active screen's widget + list/button hovers normally.
                if self.about_modal.open {
                    // About modal absorbs hover; clear background state so
                    // glows/highlights don't bleed through.
                    self.instance_prefs.list_hover = None;
                    self.instance_prefs.delete_hover = None;
                    self.instance_prefs.rename_hover = false;
                    self.instance_prefs.add_hover = false;
                    self.instance_prefs.sort_hover = false;
                } else if self.modal.open {
                    drive_modal_widgets(
                        &mut self.modal,
                        self.fonts.as_ref(),
                        card_pos,
                        self.mouse_down,
                        card_w,
                        card_h,
                    );
                    // Clear background hover state so the modal feels modal.
                    self.instance_prefs.list_hover = None;
                    self.instance_prefs.add_hover = false;
                    self.launch_button = VbtnState::default();
                    for s in self.menu_items.iter_mut() {
                        *s = VbtnState::default();
                    }
                } else if self.screen == Screen::Settings {
                    let account_uuids = self.auth.account_uuids();
                    let changed = drive_settings_sliders(
                        &mut self.prefs,
                        self.settings_tab,
                        self.fonts.as_ref(),
                        card_pos,
                        self.mouse_down,
                        card_w,
                        card_h,
                        &account_uuids,
                        &self.profiles,
                    );
                    if changed {
                        profile::save(&self.prefs.to_config(), &self.settings);
                        if let Some(b) = self.backend.as_ref() {
                            b.set_vsync(self.prefs.vsync.on);
                        }
                    }
                    self.instance_prefs.list_hover = None;
                    self.instance_prefs.add_hover = false;
                } else if self.screen == Screen::Instances {
                    let changed = drive_instance_widgets(
                        &mut self.instance_prefs,
                        &self.instances,
                        self.fonts.as_ref(),
                        card_pos,
                        self.mouse_down,
                        card_w,
                        card_h,
                    );
                    if changed {
                        sync_instance_config(&mut self.instances, &self.instance_prefs);
                        persistence::save_instances(&self.instances);
                    }
                    // List-row + "+" button + sort button + × delete hover
                    // + ✎ rename hover.
                    if let Some(fonts) = self.fonts.as_ref() {
                        self.instance_prefs.rename_hover =
                            screens::instances::rename_button_bounds(
                                card_w, card_h, fonts, &self.instances, &self.instance_prefs,
                            )
                            .map(|r| rect_contains(&r, card_pos))
                            .unwrap_or(false);
                        let mut hover: Option<usize> = None;
                        let mut delete_hover: Option<usize> = None;
                        for (i, rect) in screens::instances::list_row_bounds(
                            card_h,
                            fonts,
                            &self.instances,
                            &self.instance_prefs,
                        )
                        .iter()
                        .enumerate()
                        {
                            if rect_contains(rect, card_pos) {
                                hover = Some(i);
                            }
                            let del = screens::instances::delete_button_bounds(*rect);
                            if rect_contains(&del, card_pos) {
                                delete_hover = Some(i);
                            }
                        }
                        self.instance_prefs.list_hover = hover;
                        self.instance_prefs.delete_hover = delete_hover;
                        self.instance_prefs.add_hover = rect_contains(
                            &screens::instances::add_button_bounds(),
                            card_pos,
                        );
                        self.instance_prefs.sort_hover = rect_contains(
                            &screens::instances::sort_button_bounds(fonts),
                            card_pos,
                        );
                    }
                } else {
                    // Clear lingering hover state when off the Instances screen.
                    self.instance_prefs.list_hover = None;
                    self.instance_prefs.add_hover = false;
                }
                if let Some(overlay) = self.dev_overlay.as_mut() {
                    drive_dev_overlay(overlay, card_pos, self.mouse_down, card_w, card_h);
                }

                // Update launch-button + main-menu hover state. Modal-open
                // suppresses these so background buttons don't react under
                // a modal (new-instance or About).
                let any_modal_open = self.modal.open || self.about_modal.open;
                if any_modal_open {
                    self.launch_button = VbtnState::default();
                } else if let Some(b) = self.launch_button_bounds(card_w) {
                    self.launch_button.update(card_pos, b, self.mouse_down, time);
                } else {
                    self.launch_button = VbtnState::default();
                }

                let mut hovering_menu = false;
                if !any_modal_open && self.screen == Screen::MainMenu {
                    if let Some(fonts) = self.fonts.as_ref() {
                        let bounds = screens::main_menu::menu_item_bounds(card_w, card_h, fonts);
                        for (i, b) in bounds.iter().enumerate() {
                            self.menu_items[i].update(card_pos, *b, self.mouse_down, time);
                            if self.menu_items[i].hover {
                                hovering_menu = true;
                            }
                        }
                        // Heading hover-glow — fires per-glyph stagger when
                        // the cursor enters/exits the EwoClient title bbox.
                        let heading = screens::main_menu::heading_bounds(fonts);
                        let over_heading = rect_contains(&heading, card_pos);
                        self.heading_hover.update(over_heading, time);
                    }
                } else {
                    for s in self.menu_items.iter_mut() {
                        *s = VbtnState::default();
                    }
                    // Clear heading-hover when off the main menu so the
                    // glow doesn't survive a screen change.
                    self.heading_hover.update(false, time);
                }

                // About modal — drive the Close button hover so the ghost
                // glow tracks the cursor without needing a click.
                if self.about_modal.open {
                    let close_rect =
                        screens::about_modal::close_button_bounds(card_w, card_h);
                    self.about_modal.close_btn.handle(card_pos, close_rect, false);
                }

                // Hover priority: tab bar → menu items → launch button → window zones.
                let hovering_tab = if let Some(fonts) = self.fonts.as_ref() {
                    screens::tab_bounds(card_w, fonts)
                        .iter()
                        .any(|(_, r)| rect_contains(r, card_pos))
                } else {
                    false
                };

                let hovering_settings_tab = if self.screen == Screen::Settings {
                    if let Some(fonts) = self.fonts.as_ref() {
                        screens::settings::sidebar_tab_bounds(fonts)
                            .iter()
                            .any(|(_, r)| rect_contains(r, card_pos))
                    } else {
                        false
                    }
                } else {
                    false
                };

                let hovering_settings_widget = if self.screen == Screen::Settings {
                    if let Some(fonts) = self.fonts.as_ref() {
                        screens::settings::widget_bounds(
                            self.settings_tab, fonts, card_w, card_h,
                        )
                        .iter()
                        .any(|(_, r)| rect_contains(r, card_pos))
                    } else {
                        false
                    }
                } else {
                    false
                };

                let hovering_instance_widget = if self.screen == Screen::Instances {
                    if let Some(fonts) = self.fonts.as_ref() {
                        let widgets = screens::instances::widget_bounds(
                            card_w, card_h, fonts, &self.instance_prefs, &self.instances,
                        );
                        let on_widget = widgets
                            .iter()
                            .any(|(_, r)| rect_contains(r, card_pos));
                        let row_rects = screens::instances::list_row_bounds(
                            card_h, fonts, &self.instances, &self.instance_prefs,
                        );
                        let on_list_row =
                            row_rects.iter().any(|r| rect_contains(r, card_pos));
                        let on_add = rect_contains(
                            &screens::instances::add_button_bounds(),
                            card_pos,
                        );
                        let on_sort = rect_contains(
                            &screens::instances::sort_button_bounds(fonts),
                            card_pos,
                        );
                        // × buttons are inside row rects, but we want the
                        // pointer cursor regardless — `on_list_row` already
                        // covers them.
                        let on_rename = screens::instances::rename_button_bounds(
                            card_w, card_h, fonts, &self.instances, &self.instance_prefs,
                        )
                        .map(|r| rect_contains(&r, card_pos))
                        .unwrap_or(false);
                        on_widget || on_list_row || on_add || on_sort || on_rename
                    } else {
                        false
                    }
                } else {
                    false
                };

                // Modal hover: when the modal is open, the cursor flips
                // based on which control it's over. Name field → text
                // I-beam; buttons / dropdowns / slider → pointer; anywhere
                // else (including the shroud) → default arrow.
                let modal_cursor = if self.modal.open {
                    if let Some(fonts) = self.fonts.as_ref() {
                        let layout = screens::new_instance_modal::compute_layout(
                            card_w, card_h, fonts,
                        );
                        if rect_contains(&layout.name_input, card_pos) {
                            Some(CursorIcon::Text)
                        } else {
                            let widgets = screens::new_instance_modal::widget_bounds(
                                card_w, card_h, fonts,
                            );
                            if widgets.iter().any(|(_, r)| rect_contains(r, card_pos)) {
                                Some(CursorIcon::Pointer)
                            } else {
                                Some(CursorIcon::Default)
                            }
                        }
                    } else {
                        Some(CursorIcon::Default)
                    }
                } else {
                    None
                };

                if let Some(icon) = modal_cursor {
                    window.set_cursor(icon);
                } else if hovering_tab
                    || hovering_menu
                    || hovering_settings_tab
                    || hovering_settings_widget
                    || hovering_instance_widget
                    || self.launch_button.hover
                {
                    window.set_cursor(CursorIcon::Pointer);
                } else {
                    update_cursor_icon(&window, &self.cursor, size, scale);
                }
            }

            WindowEvent::MouseInput { state, button: MouseButton::Left, .. } => {
                let pressed = matches!(state, ElementState::Pressed);
                self.mouse_down = pressed;

                let scale = window.scale_factor();
                let card_pos = cursor_card_local(self.cursor, scale);
                let size = window.inner_size();
                let card_w = card_content_width(size, scale);
                let card_h = card_content_height(size, scale);
                let time = self.clock.elapsed;

                // Step -1: dev overlay (when --dev) — sits above everything,
                // including the modal. Absorbs input when cursor is over it.
                if let Some(overlay) = self.dev_overlay.as_mut() {
                    let panel = screens::dev_overlay::panel_bounds(card_w, card_h);
                    drive_dev_overlay(overlay, card_pos, pressed, card_w, card_h);
                    if rect_contains(&panel, card_pos) {
                        if pressed {
                            let vsync_changed =
                                handle_dev_overlay_press(overlay, card_pos, card_w, card_h);
                            if vsync_changed {
                                if let Some(backend) = self.backend.as_ref() {
                                    backend.set_vsync(overlay.vsync);
                                }
                            }
                        }
                        return;
                    }
                }

                // Step 0a: About modal — when open, absorbs all input. Close
                // button click closes; shroud click closes; press anywhere
                // else inside the card is a no-op so the modal can't be
                // dismissed by misclicks on the card itself.
                if self.about_modal.open {
                    let close_rect =
                        screens::about_modal::close_button_bounds(card_w, card_h);
                    let close_clicked =
                        self.about_modal.close_btn.handle(card_pos, close_rect, pressed);
                    if close_clicked {
                        log::info!("about: Close clicked");
                        self.about_modal.close();
                    } else if pressed
                        && screens::about_modal::shroud_consumes(card_pos, card_w, card_h)
                    {
                        log::info!("about: shroud click → closing");
                        self.about_modal.close();
                    }
                    return;
                }

                // Step 0: modal — when open, the modal absorbs all input.
                // `drive_modal_widgets` runs first so slider drags start on
                // the rising edge inside bounds; `handle_modal_press` runs
                // after to consume button / dropdown / shroud clicks.
                if self.modal.open {
                    drive_modal_widgets(
                        &mut self.modal,
                        self.fonts.as_ref(),
                        card_pos,
                        pressed,
                        card_w,
                        card_h,
                    );
                    if pressed {
                        handle_modal_press(
                            &mut self.modal,
                            &mut self.instances,
                            &mut self.instance_prefs,
                            &self.versions,
                            &mut self.downloads,
                            self.fonts.as_ref(),
                            card_pos,
                            card_w,
                            card_h,
                            self.clock.elapsed,
                        );
                    }
                    return;
                }

                // Step 1: tab bar hit-test (priority over everything else).
                let mut handled = false;
                if pressed {
                    if let Some(fonts) = self.fonts.as_ref() {
                        for (target, rect) in screens::tab_bounds(card_w, fonts) {
                            if rect_contains(&rect, card_pos) {
                                if self.screen != target {
                                    log::info!("nav: {:?} → {:?}", self.screen, target);
                                    self.screen = target;
                                    self.launch_button = VbtnState::default();
                                    for s in self.menu_items.iter_mut() {
                                        *s = VbtnState::default();
                                    }
                                    self.prefs.close_dropdowns();
                                    self.instance_prefs.close_dropdowns();
                                    self.keybind_capture = None;
                                    self.prefs.keybind_request = None;
                                    self.prefs.profile_renaming = None;
                                    self.prefs.profile_rename_buffer.clear();
                                    self.modal.close();
                                    // Trigger the tab fade-in when arriving at
                                    // Settings, so the active tab's content
                                    // greets the user with the same animation
                                    // it plays when they switch tabs.
                                    if target == Screen::Settings {
                                        self.prefs.tab_changed_at = Some(time);
                                        self.prefs.settings_scroll = 0.0;
                                    }
                                    // Demo affordance: clicking the LAUNCHING
                                    // tab without an active launch kicks off
                                    // a fresh synthetic one so the screen is
                                    // never empty.
                                    if target == Screen::Launching
                                        && self.launching.start_time.is_none()
                                    {
                                        let (inst_name, inst_meta) = self
                                            .instances
                                            .get(self.instance_prefs.selected)
                                            .map(|i| {
                                                (
                                                    i.name.clone(),
                                                    format!(
                                                        "{} · ADOPTIUM 21 · {} GB",
                                                        i.version,
                                                        self.instance_prefs.ram.value as i32,
                                                    ),
                                                )
                                            })
                                            .unwrap_or_else(|| {
                                                (
                                                    "Velvet Hours".to_string(),
                                                    "VANILLA · 1.21 · ADOPTIUM 21".to_string(),
                                                )
                                            });
                                        self.launching.enter(time, &inst_name, &inst_meta);
                                    }
                                }
                                handled = true;
                                break;
                            }
                        }
                    }
                }

                // Step 2: main-menu sidebar items.
                if !handled && self.screen == Screen::MainMenu {
                    if let Some(fonts) = self.fonts.as_ref() {
                        let bounds = screens::main_menu::menu_item_bounds(card_w, card_h, fonts);
                        for (i, b) in bounds.iter().enumerate() {
                            let clicked =
                                self.menu_items[i].update(card_pos, *b, pressed, time);
                            if clicked {
                                match MAIN_MENU_ACTIONS[i] {
                                    MenuAction::Navigate(target) => {
                                        log::info!("nav (menu): {:?} → {:?}", self.screen, target);
                                        self.screen = target;
                                    }
                                    MenuAction::About => {
                                        log::info!("about: clicked → opening About modal");
                                        self.about_modal.open();
                                    }
                                    MenuAction::Quit => {
                                        log::info!("quit: closing app");
                                        event_loop.exit();
                                    }
                                }
                            }
                            if self.menu_items[i].hover && pressed {
                                handled = true;
                            }
                        }
                    }
                }

                // Step 2.5: settings sidebar tab switch.
                if !handled && pressed && self.screen == Screen::Settings {
                    if let Some(fonts) = self.fonts.as_ref() {
                        for (tab, rect) in screens::settings::sidebar_tab_bounds(fonts) {
                            if rect_contains(&rect, card_pos) {
                                if self.settings_tab != tab {
                                    log::info!(
                                        "settings: {:?} → {:?}",
                                        self.settings_tab, tab
                                    );
                                    self.settings_tab = tab;
                                    self.prefs.close_dropdowns();
                                    // A tab switch abandons a pending keybind
                                    // capture or an in-progress profile rename.
                                    self.keybind_capture = None;
                                    self.prefs.keybind_request = None;
                                    self.prefs.profile_renaming = None;
                                    self.prefs.profile_rename_buffer.clear();
                                    self.prefs.tab_changed_at =
                                        Some(self.clock.elapsed);
                                    self.prefs.settings_scroll = 0.0;
                                }
                                handled = true;
                                break;
                            }
                        }
                    }
                }

                // Step 2.7: settings widget interaction. Toggles flip on the
                // press edge; sliders begin a drag (drag continues in
                // CursorMoved via `drive_settings_sliders`); dropdown heads
                // toggle the menu open/closed; clicks on open menu rows
                // commit the selection; clicks elsewhere close the menu.
                if self.screen == Screen::Settings {
                    let account_uuids = self.auth.account_uuids();
                    let mut changed = drive_settings_sliders(
                        &mut self.prefs,
                        self.settings_tab,
                        self.fonts.as_ref(),
                        card_pos,
                        pressed,
                        card_w,
                        card_h,
                        &account_uuids,
                        &self.profiles,
                    );
                    if !handled && pressed {
                        let (h, c) = handle_settings_press(
                            &mut self.prefs,
                            self.settings_tab,
                            self.fonts.as_ref(),
                            card_pos,
                            card_w,
                            card_h,
                            &account_uuids,
                            &self.profiles,
                        );
                        handled = h;
                        changed = changed || c;
                    }
                    if changed {
                        profile::save(&self.prefs.to_config(), &self.settings);
                        if let Some(b) = self.backend.as_ref() {
                            b.set_vsync(self.prefs.vsync.on);
                        }
                    }
                }

                // Step 2.75: instances list "+" button — opens the
                // new-instance modal.
                if !handled && pressed && self.screen == Screen::Instances {
                    let plus_rect = screens::instances::add_button_bounds();
                    if rect_contains(&plus_rect, card_pos) {
                        log::info!("instances: + clicked → opening new-instance modal");
                        self.modal.open();
                        handled = true;
                    }
                }

                // Step 2.754: ✎ rename icon — enters rename mode for
                // the currently-selected instance.
                if !handled && pressed && self.screen == Screen::Instances {
                    if let Some(fonts) = self.fonts.as_ref() {
                        if let Some(r) = screens::instances::rename_button_bounds(
                            card_w,
                            card_h,
                            fonts,
                            &self.instances,
                            &self.instance_prefs,
                        ) {
                            if rect_contains(&r, card_pos) {
                                if let Some(inst) = self
                                    .instances
                                    .get(self.instance_prefs.selected)
                                {
                                    self.instance_prefs.renaming = true;
                                    self.instance_prefs.rename_buffer = inst.name.clone();
                                    self.instance_prefs.rename_focus_time = 0.0;
                                    log::info!("rename: editing \"{}\"", inst.name);
                                }
                                handled = true;
                            }
                        }
                    }
                }

                // Step 2.755: × delete button — must run before
                // click-to-select since × sits inside the row's hit-rect.
                if !handled && pressed && self.screen == Screen::Instances {
                    if let Some(fonts) = self.fonts.as_ref() {
                        let order = screens::instances::display_order(
                            &self.instances,
                            self.instance_prefs.sort_mode,
                        );
                        let row_rects = screens::instances::list_row_bounds(
                            card_h, fonts, &self.instances, &self.instance_prefs,
                        );
                        for (display_idx, row_rect) in row_rects.iter().enumerate() {
                            let del_rect = screens::instances::delete_button_bounds(*row_rect);
                            if rect_contains(&del_rect, card_pos) {
                                let underlying = order[display_idx];
                                delete_instance(
                                    &mut self.instances,
                                    &mut self.instance_prefs,
                                    underlying,
                                    self.clock.elapsed,
                                );
                                handled = true;
                                break;
                            }
                        }
                    }
                }

                // Step 2.76: instance list rows — click-to-select. Click
                // dispatch uses display order (visual position), then maps
                // back to the underlying index via `display_order`.
                if !handled && pressed && self.screen == Screen::Instances {
                    if let Some(fonts) = self.fonts.as_ref() {
                        let order = screens::instances::display_order(
                            &self.instances,
                            self.instance_prefs.sort_mode,
                        );
                        for (display_idx, rect) in screens::instances::list_row_bounds(
                            card_h, fonts, &self.instances, &self.instance_prefs,
                        )
                        .iter()
                        .enumerate()
                        {
                            if rect_contains(rect, card_pos) {
                                let underlying = order[display_idx];
                                if underlying != self.instance_prefs.selected {
                                    log::info!(
                                        "instances: select {} → {}",
                                        self.instance_prefs.selected, underlying
                                    );
                                    self.instance_prefs.select(&self.instances, underlying);
                                    self.instance_prefs.selected_at =
                                        Some(self.clock.elapsed);
                                }
                                handled = true;
                                break;
                            }
                        }
                    }
                }

                // Step 2.77: sort label cycle.
                if !handled && pressed && self.screen == Screen::Instances {
                    if let Some(fonts) = self.fonts.as_ref() {
                        let r = screens::instances::sort_button_bounds(fonts);
                        if rect_contains(&r, card_pos) {
                            self.instance_prefs.sort_mode =
                                self.instance_prefs.sort_mode.cycle();
                            log::info!(
                                "instances: sort → {}",
                                self.instance_prefs.sort_mode.label()
                            );
                            handled = true;
                        }
                    }
                }

                // Step 2.8: instances detail widget interaction. Sliders for
                // RAM / render distance, plus the Java runtime dropdown.
                if self.screen == Screen::Instances {
                    let changed = drive_instance_widgets(
                        &mut self.instance_prefs,
                        &self.instances,
                        self.fonts.as_ref(),
                        card_pos,
                        pressed,
                        card_w,
                        card_h,
                    );
                    if changed {
                        sync_instance_config(&mut self.instances, &self.instance_prefs);
                        persistence::save_instances(&self.instances);
                    }
                    if !handled && pressed {
                        handled = handle_instances_press(
                            &mut self.instance_prefs,
                            &mut self.instances,
                            self.fonts.as_ref(),
                            card_pos,
                            card_w,
                            card_h,
                        );
                    }
                }

                // Step 2.9: Launching screen's Retry/Back buttons. Only
                // active when the JVM has exited non-zero. Retry rebuilds
                // the LaunchPlan and respawns; Back returns to Instances.
                if !handled
                    && pressed
                    && self.screen == Screen::Launching
                    && self.launching.ended_in_error()
                {
                    let retry_rect =
                        screens::launching::retry_button_bounds(card_w, card_h);
                    let back_rect =
                        screens::launching::cancel_button_bounds(card_w, card_h);
                    if rect_contains(&retry_rect, card_pos) {
                        log::info!("launching: Retry clicked");
                        let inst_name = self.launching.instance_name.clone();
                        let inst_meta = self.launching.instance_meta.clone();
                        self.launching.reset_for_retry();
                        let ok = self.try_real_launch(
                            self.instance_prefs.selected,
                            &inst_name,
                            &inst_meta,
                            time,
                        );
                        if !ok {
                            // try_real_launch can return false silently
                            // when something's missing — surface that to
                            // the user as an error rather than a blank
                            // screen.
                            self.launching.push_real_line(
                                screens::RealSeverity::Warn,
                                "[ewo] retry could not start launch — see logs above".into(),
                                time,
                            );
                            self.launching.set_real_exit(Some(127), time);
                        }
                        handled = true;
                    } else if rect_contains(&back_rect, card_pos) {
                        log::info!("launching: Back clicked");
                        self.launching.exit();
                        self.screen = Screen::Instances;
                        handled = true;
                    }
                }

                // Step 3: active screen's launch button.
                if !handled {
                    if let Some(b) = self.launch_button_bounds(card_w) {
                        let clicked = self.launch_button.update(card_pos, b, pressed, time);
                        // Gate: don't launch a Pending instance — its
                        // download isn't done. The hover state still
                        // updates (so the button visually responds to
                        // the click) but the launch logic skips.
                        let pending = self
                            .instances
                            .get(self.instance_prefs.selected)
                            .map(|i| {
                                i.status
                                    == ewo_render::screens::instances::InstanceStatus::Pending
                            })
                            .unwrap_or(false);
                        if clicked && !pending {
                            // Pull the actual selected instance's name +
                            // version line so the Launching screen reflects
                            // what the user is launching.
                            let (inst_name, inst_meta) = self
                                .instances
                                .get(self.instance_prefs.selected)
                                .map(|i| {
                                    (
                                        i.name.clone(),
                                        format!(
                                            "{} · ADOPTIUM 21 · {} GB",
                                            i.version,
                                            self.instance_prefs.ram.value as i32,
                                        ),
                                    )
                                })
                                .unwrap_or_else(|| {
                                    (
                                        "Velvet Hours".to_string(),
                                        "VANILLA · 1.21 · ADOPTIUM 21".to_string(),
                                    )
                                });
                            log::info!(
                                "vbtn: Launch clicked → launching \"{}\"",
                                inst_name
                            );
                            // Update last_played on the launched instance
                            // and persist immediately so the timestamp
                            // survives a restart.
                            if let Some(inst) =
                                self.instances.get_mut(self.instance_prefs.selected)
                            {
                                inst.last_played = "just now".to_string();
                                inst.last_played_at =
                                    screens::instances::current_unix_seconds();
                            }
                            persistence::save_instances(&self.instances);
                            // Decide synthetic vs real launch. Real
                            // launch fires only when the instance is
                            // `Ready` (artifacts on disk + verified)
                            // AND we can resolve the per-version manifest
                            // from cache. Anything else falls back to
                            // synthetic so the user still gets feedback.
                            let real_launched = self.try_real_launch(
                                self.instance_prefs.selected,
                                &inst_name,
                                &inst_meta,
                                time,
                            );
                            if !real_launched {
                                log::info!(
                                    "vbtn: Launch falling back to synthetic for \"{}\"",
                                    inst_name
                                );
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
                        if self.launch_button.hover && pressed {
                            handled = true;
                        }
                    }
                }

                // Step 4: window drag/resize fallback.
                if pressed && !handled {
                    let zone = hit_test(self.cursor, size, scale);
                    match zone {
                        Some(Zone::Caption) => {
                            let _ = window.drag_window();
                        }
                        Some(Zone::Resize(dir)) => {
                            let _ = window.drag_resize_window(dir);
                        }
                        None => {
                            if let Some(b) = self.backdrop.as_mut() {
                                b.disturb();
                            }
                        }
                    }
                }
            }

            WindowEvent::RedrawRequested => {
                self.clock.tick();
                let time = self.clock.elapsed;
                let dt = self.clock.dt;
                let screen = self.screen;

                // Auto-end celebrate after the configured duration.
                if let Some(end) = self.celebrate_until {
                    if time >= end {
                        if let Some(b) = self.backdrop.as_mut() {
                            b.celebrate(false);
                        }
                        self.celebrate_until = None;
                    }
                }

                if let Some(backdrop) = self.backdrop.as_mut() {
                    backdrop.update(dt);
                }

                // Tick widget hover animations.
                self.launch_button.tick(dt);
                for s in self.menu_items.iter_mut() {
                    s.tick(dt);
                }
                self.prefs.tick(dt);
                self.instance_prefs.tick(dt);
                if self.instance_prefs.renaming {
                    self.instance_prefs.rename_focus_time += dt;
                }
                if self.prefs.profile_renaming.is_some() {
                    self.prefs.profile_rename_focus_time += dt;
                }
                self.modal.tick(dt);
                self.about_modal.tick(dt);
                self.auth.poll();
                self.versions.poll();
                self.downloads.poll();

                // Sync per-instance download progress for the list-row
                // badge. Only Pending instances are interesting; we
                // clear the map first so completed downloads stop
                // showing a stale percentage.
                self.instance_prefs.download_pct.clear();
                for inst in self.instances.iter() {
                    if inst.status
                        != ewo_render::screens::instances::InstanceStatus::Pending
                    {
                        continue;
                    }
                    let v = inst.version.rsplit(" · ").next().unwrap_or(&inst.version);
                    if let Some(status) = self.downloads.status(v) {
                        if let Some(total) = status.total {
                            if total > 0 {
                                let pct = ((status.downloaded as f64 / total as f64) * 100.0)
                                    .clamp(0.0, 99.0)
                                    as u32;
                                self.instance_prefs
                                    .download_pct
                                    .insert(inst.name.clone(), pct);
                            }
                        }
                    }
                }

                // Drain runtime (bundled-JRE) events. Surface progress
                // as Info lines on the launching screen; on Done, kick
                // the JRE-detector cache + retry the pending launch.
                let runtime_events = self.runtime.poll();
                for ev in runtime_events {
                    match ev {
                        runtime::RuntimeEvent::Resolved { major, info } => {
                            self.launching.push_real_line(
                                screens::RealSeverity::Info,
                                format!(
                                    "[ewo] resolved Java {} → {} ({:.1} MB)",
                                    major,
                                    info.release_name,
                                    info.size as f32 / 1_048_576.0
                                ),
                                time,
                            );
                        }
                        runtime::RuntimeEvent::Progress { downloaded, total } => {
                            // Drive the real pbar override (visible
                            // immediately as a smooth fill), and log a
                            // line every 10% step so the user sees
                            // discrete checkpoints in the log panel too.
                            if total > 0 {
                                let frac = downloaded as f32 / total as f32;
                                self.launching.set_real_progress(Some(frac));
                                let pct = frac * 100.0;
                                let bucket = (pct as u32) / 10 * 10;
                                if bucket > 0 && bucket % 10 == 0 {
                                    let line = format!(
                                        "[ewo] downloading runtime: {:>3}% ({} / {} MB)",
                                        bucket,
                                        downloaded / 1_048_576,
                                        total / 1_048_576,
                                    );
                                    self.launching.push_real_line(
                                        screens::RealSeverity::Info,
                                        line,
                                        time,
                                    );
                                }
                            }
                        }
                        runtime::RuntimeEvent::Done { major, jre_dir } => {
                            // Clear the pbar override — synthetic curve
                            // takes back over while the JVM boots.
                            self.launching.set_real_progress(None);
                            self.launching.push_real_line(
                                screens::RealSeverity::Info,
                                format!(
                                    "[ewo] Java {} extracted to {} — retrying launch…",
                                    major,
                                    jre_dir.display()
                                ),
                                time,
                            );
                            launch::jre::invalidate_cache();
                            // If the launch we deferred is for this
                            // major, retry it now.
                            if let Some(p) = self.pending_relaunch.clone() {
                                if p.waiting_for_major == major {
                                    self.pending_relaunch = None;
                                    let ok = self.try_real_launch(
                                        p.instance_idx,
                                        &p.instance_name,
                                        &p.instance_meta,
                                        time,
                                    );
                                    if !ok {
                                        self.launching.push_real_line(
                                            screens::RealSeverity::Warn,
                                            "[ewo] retry failed after JRE install".into(),
                                            time,
                                        );
                                        self.launching.set_real_exit(Some(127), time);
                                    }
                                }
                            }
                        }
                        runtime::RuntimeEvent::Failed { major, message } => {
                            log::warn!("runtime: Java {} fetch failed: {}", major, message);
                            self.launching.set_real_progress(None);
                            self.launching.push_real_line(
                                screens::RealSeverity::Warn,
                                format!("[ewo] Java {} fetch failed: {}", major, message),
                                time,
                            );
                            self.launching.set_real_exit(Some(127), time);
                            self.pending_relaunch = None;
                        }
                    }
                }

                // Drain JVM launch events into the launching screen.
                // Each line, every stage transition, and the exit code
                // arrives via this channel. When the JVM exits we drop
                // the receiver — the next launch will create a fresh one.
                let mut launch_finished = false;
                if let Some(rx) = self.launch_rx.as_ref() {
                    while let Ok(event) = rx.try_recv() {
                        match event {
                            launch::LaunchEvent::Started => {
                                log::info!("launch: JVM started");
                            }
                            launch::LaunchEvent::Line { severity, text } => {
                                let sev = match severity {
                                    launch::Severity::Info => screens::RealSeverity::Info,
                                    launch::Severity::Warn => screens::RealSeverity::Warn,
                                };
                                self.launching.push_real_line(sev, text, time);
                            }
                            launch::LaunchEvent::Exited(code) => {
                                log::info!("launch: JVM exited code={:?}", code);
                                self.launching.set_real_exit(code, time);
                                // Dump the in-memory log to disk so the
                                // user can grab it later (especially on
                                // a crash). Best-effort: errors don't
                                // surface to the UI.
                                persist_launch_log(
                                    &self.launching.instance_name,
                                    self.launching
                                        .real_log
                                        .as_deref()
                                        .unwrap_or(&[]),
                                    code,
                                );
                                launch_finished = true;
                            }
                            launch::LaunchEvent::SpawnFailed(msg) => {
                                log::warn!("launch: spawn failed: {}", msg);
                                self.launching.push_real_line(
                                    screens::RealSeverity::Warn,
                                    format!("[ewo] spawn failed: {}", msg),
                                    time,
                                );
                                self.launching.set_real_exit(Some(127), time);
                                launch_finished = true;
                            }
                        }
                    }
                }
                if launch_finished {
                    self.launch_rx = None;
                }

                // Flip any instances whose download job just finished from
                // `Pending` to `Ready` and persist. We match on the
                // version *string* of the most-recent job — same instance
                // can appear multiple times with different IDs, but the
                // status flips per-instance.
                let mut completed_versions: Vec<String> = Vec::new();
                for (vid, status) in self.downloads.iter_statuses() {
                    if status.done && status.error.is_none() {
                        completed_versions.push(vid.clone());
                    }
                }
                if !completed_versions.is_empty() {
                    let mut any_changed = false;
                    for inst in self.instances.iter_mut() {
                        if inst.status == ewo_render::screens::instances::InstanceStatus::Ready {
                            continue;
                        }
                        // Match by the version-string suffix on the meta
                        // (commit_new_instance writes "<LOADER> · <version>").
                        let v = match inst.version.rsplit(" · ").next() {
                            Some(s) => s,
                            None => &inst.version,
                        };
                        if completed_versions.iter().any(|w| w == v) {
                            inst.status = ewo_render::screens::instances::InstanceStatus::Ready;
                            any_changed = true;
                            log::info!(
                                "instances: \"{}\" → Ready (version {})",
                                inst.name, v
                            );
                        }
                    }
                    if any_changed {
                        persistence::save_instances(&self.instances);
                    }
                }

                // Sync the live version manifest into the new-instance
                // modal's dropdown source. Filter to releases by default;
                // a "Show snapshots" toggle could later flip the second
                // arg. List goes from newest → oldest (Mojang's order).
                if let Some(manifest) = self.versions.manifest() {
                    let want: Vec<String> = manifest
                        .filtered_for_dropdown(false)
                        .iter()
                        .map(|e| e.id.clone())
                        .collect();
                    if want != self.modal.mc_versions {
                        self.modal.apply_versions(want);
                    }
                }

                // Account-tab actions — the press handler records one
                // request; dispatch it here, where we own `&mut auth`.
                if let Some(req) = self.prefs.account_request.take() {
                    match req {
                        AccountRequest::Add => {
                            log::info!("auth: add account -> interactive sign-in");
                            self.auth.start_interactive();
                        }
                        AccountRequest::SetActive(uuid) => {
                            self.auth.set_active(&uuid);
                        }
                        AccountRequest::Remove(uuid) => {
                            self.auth.remove(&uuid);
                        }
                    }
                }

                // Profile-tab actions — switch / new / duplicate / delete.
                if let Some(req) = self.prefs.profile_request.take() {
                    let applied = match req {
                        ProfileRequest::Switch(name) => profile::switch(&name),
                        ProfileRequest::New => {
                            let (_n, c, s) = profile::create();
                            Some((c, s))
                        }
                        ProfileRequest::Duplicate => {
                            profile::duplicate(&self.active_profile).map(|(_n, c, s)| (c, s))
                        }
                        ProfileRequest::Delete(name) => profile::delete(&name),
                        ProfileRequest::Rename { index, new_name } => {
                            if let Some(old) = self.profiles.get(index).cloned() {
                                profile::rename(&old, &new_name);
                            }
                            self.prefs.profile_renaming = None;
                            self.prefs.profile_rename_buffer.clear();
                            None // a rename doesn't change the active config
                        }
                    };
                    if let Some((config, settings)) = applied {
                        self.apply_loaded_config(config, settings);
                    }
                    self.profiles = profile::list();
                    self.active_profile = profile::active_name();
                    // Keybinds are profile-scoped — the switched-to profile
                    // carries its own set.
                    self.keybinds = profile::load_keybinds();
                }

                // Keybinds-tab actions — arm a rebind or reset to defaults.
                if let Some(req) = self.prefs.keybind_request.take() {
                    match req {
                        KeybindRequest::Capture(idx) => {
                            if let Some(action) = keybind::REGISTRY.get(idx) {
                                log::info!("keybind: capturing for {}", action.id);
                                self.keybind_capture = Some(action.id.to_string());
                            }
                        }
                        KeybindRequest::ResetAll => {
                            for a in keybind::REGISTRY.iter() {
                                self.keybinds.insert(a.id.to_string(), a.default);
                            }
                            self.keybind_capture = None;
                            profile::save_keybinds(&self.keybinds);
                            log::info!("keybind: reset all to defaults");
                        }
                    }
                }

                // Reset preferences — wipe to bundled defaults, persist,
                // and resync the GL backend's vsync to match.
                if self.prefs.reset_requested {
                    self.prefs.reset_requested = false;
                    self.prefs
                        .apply_config(&screens::SettingsConfig::default());
                    profile::save(&self.prefs.to_config(), &self.settings);
                    if let Some(b) = self.backend.as_ref() {
                        b.set_vsync(self.prefs.vsync.on);
                    }
                    log::info!("reset_prefs: applied defaults");
                }

                // Modules tab — persist `modules.toml` when an edit landed.
                if self.prefs.modules_changed {
                    self.prefs.modules_changed = false;
                    let (enabled, fov) = self.prefs.modules_snapshot();
                    profile::save_modules(&enabled, fov);
                }
                // PvP-Utils tab — persist `pvp.toml` when an edit landed. The
                // in-game mod polls the file's mtime each frame and reloads,
                // so a running game picks the change up immediately.
                if self.prefs.pvp_changed {
                    self.prefs.pvp_changed = false;
                    profile::save_pvp_config(&self.prefs.pvp);
                }
                if let Some(overlay) = self.dev_overlay.as_mut() {
                    overlay.tick(dt);
                    let density_changed = overlay.apply_to_settings(&mut self.settings);
                    if density_changed {
                        if let (Some(window), Some(backdrop)) =
                            (self.window.as_ref(), self.backdrop.as_mut())
                        {
                            let size = window.inner_size();
                            let (cw, ch) = app_window::card_content_size(size.width, size.height);
                            backdrop.resize(cw, ch, &self.settings);
                        }
                    }
                    // Mirror dev overlay's sim_error into the launching
                    // state so the pbar variant matches what the dev pill
                    // shows. Auto-starts a synthetic launch if needed so
                    // the error has a bar to render against.
                    if overlay.sim_error != self.launching.error {
                        match overlay.sim_error {
                            Some(variant) => {
                                if self.launching.start_time.is_none() {
                                    let (n, m) = self
                                        .instances
                                        .get(self.instance_prefs.selected)
                                        .map(|i| {
                                            (
                                                i.name.clone(),
                                                format!(
                                                    "{} · ADOPTIUM 21 · {} GB",
                                                    i.version,
                                                    self.instance_prefs.ram.value as i32,
                                                ),
                                            )
                                        })
                                        .unwrap_or_else(|| {
                                            (
                                                "Velvet Hours".to_string(),
                                                "VANILLA · 1.21 · ADOPTIUM 21".to_string(),
                                            )
                                        });
                                    self.launching.enter(time, &n, &m);
                                }
                                self.launching.trigger_error(variant, time);
                            }
                            None => self.launching.clear_error(),
                        }
                    }
                }
                if self.screen == Screen::Launching {
                    self.launching.tick(time, dt);
                    if self.launching.should_handoff(time) {
                        log::info!("launching: handoff complete → returning to Instances");
                        self.launching.exit();
                        self.screen = Screen::Instances;
                    }
                }

                let backdrop_ref = self.backdrop.as_ref();
                let fonts_ref = self.fonts.as_ref();
                let launch_button = self.launch_button;
                let menu_items = self.menu_items;
                let settings_tab = self.settings_tab;
                let theme = &self.theme;
                let settings = &self.settings;
                let prefs = &self.prefs;
                let instance_prefs = &self.instance_prefs;
                let launching_state = &self.launching;
                let modal = &self.modal;
                let about_modal = &self.about_modal;
                let dev_overlay = self.dev_overlay.as_ref();
                let heading_hover = self.heading_hover;
                // Build the Account-tab view from the auth store. The row
                // Vec + the error string are stack locals that AccountView
                // borrows — keeps `ewo-render` ignorant of auth types.
                let active_uuid: Option<String> = self.auth.active().map(|a| a.uuid.clone());
                let account_rows: Vec<AccountRowView<'_>> = self
                    .auth
                    .accounts()
                    .iter()
                    .map(|a| AccountRowView {
                        name: &a.name,
                        uuid: &a.uuid,
                        active: active_uuid.as_deref() == Some(a.uuid.as_str()),
                    })
                    .collect();
                let err_msg: Option<String> = if let AuthOp::Failed(err) = self.auth.op() {
                    Some(format_auth_error(err))
                } else {
                    None
                };
                let account_op = match self.auth.op() {
                    AuthOp::Idle => AccountOpView::Idle,
                    AuthOp::Working(stage) => AccountOpView::Working { stage: *stage },
                    AuthOp::Failed(_) => AccountOpView::Failed {
                        message: err_msg.as_deref().unwrap_or("auth failed"),
                    },
                };
                let account_view = AccountView {
                    accounts: &account_rows,
                    op: account_op,
                };
                let profile_rows: Vec<ProfileRowView<'_>> = self
                    .profiles
                    .iter()
                    .map(|n| ProfileRowView {
                        name: n,
                        active: *n == self.active_profile,
                    })
                    .collect();
                let profile_view = ProfileView {
                    profiles: &profile_rows,
                };
                // Keybinds-tab view — registry actions resolved against the
                // active profile's bindings. The chord labels are stack
                // locals the KeybindRowViews borrow.
                let keybind_chord_labels: Vec<String> = keybind::REGISTRY
                    .iter()
                    .map(|a| {
                        self.keybinds
                            .get(a.id)
                            .copied()
                            .unwrap_or(a.default)
                            .label()
                    })
                    .collect();
                let keybind_rows: Vec<KeybindRowView<'_>> = keybind::REGISTRY
                    .iter()
                    .zip(&keybind_chord_labels)
                    .map(|(a, label)| KeybindRowView {
                        action_label: a.label,
                        module: a.module,
                        chord_label: label,
                        capturing: self.keybind_capture.as_deref() == Some(a.id),
                    })
                    .collect();
                let keybind_view = KeybindView { rows: &keybind_rows };
                let frame_stats = FrameStats {
                    fps: self.clock.avg_fps(),
                    frame_ms: self.clock.avg_dt() * 1000.0,
                    worst_ms: self.clock.worst_dt() * 1000.0,
                };
                let instances = self.instances.as_slice();
                if let (Some(backend), Some(backdrop), Some(fonts)) =
                    (self.backend.as_mut(), backdrop_ref, fonts_ref)
                {
                    backend.render(|canvas, w, h| {
                        app_window::draw_frame(
                            canvas, backdrop, fonts, w, h, time, theme, settings,
                            screen, &launch_button, &menu_items, settings_tab, prefs,
                            instance_prefs, launching_state, modal, about_modal,
                            dev_overlay, frame_stats, instances, heading_hover,
                            account_view, profile_view, keybind_view,
                        );
                    });
                }
                // Only chain redraws while focused. When unfocused, the
                // event loop's `WaitUntil` (set in `about_to_wait`) will
                // wake us at ~10 FPS so subtle animations still tick but
                // we stop hammering the GPU.
                if self.focused {
                    window.request_redraw();
                }
            }

            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        // Three modes:
        //  1. Unfocused → ~10 FPS via WaitUntil — GPU stays idle while
        //     the user is in another window.
        //  2. Focused, VSync on → Poll. Skia + GL surface caps at the
        //     display refresh, no extra throttling needed.
        //  3. Focused, VSync off → if `max_fps < 240` cap via WaitUntil
        //     to that target; else Poll for uncapped (the dev-overlay
        //     path that validates the 500fps OLED target).
        if !self.focused {
            event_loop.set_control_flow(ControlFlow::WaitUntil(
                Instant::now() + Duration::from_millis(100),
            ));
            if let Some(w) = self.window.as_ref() {
                w.request_redraw();
            }
            return;
        }

        let max_fps = self.prefs.max_fps.value;
        if !self.prefs.vsync.on && max_fps < 240.0 && max_fps > 0.0 {
            let target_ns = (1_000_000_000.0 / max_fps).max(1.0) as u64;
            event_loop.set_control_flow(ControlFlow::WaitUntil(
                Instant::now() + Duration::from_nanos(target_ns),
            ));
            if let Some(w) = self.window.as_ref() {
                w.request_redraw();
            }
        } else {
            event_loop.set_control_flow(ControlFlow::Poll);
        }
    }
}

#[derive(Copy, Clone, Debug)]
enum Zone {
    Caption,
    Resize(ResizeDirection),
}

fn hit_test(
    pos: PhysicalPosition<f64>,
    size: PhysicalSize<u32>,
    scale: f64,
) -> Option<Zone> {
    let border = RESIZE_BORDER_LP * scale;
    let caption = CAPTION_HEIGHT_LP * scale;
    let (x, y) = (pos.x, pos.y);
    let (w, h) = (size.width as f64, size.height as f64);

    let on_top = y >= 0.0 && y < border;
    let on_bottom = y > h - border;
    let on_left = x >= 0.0 && x < border;
    let on_right = x > w - border;

    use ResizeDirection::*;
    let dir = match (on_top, on_bottom, on_left, on_right) {
        (true, _, true, _) => Some(NorthWest),
        (true, _, _, true) => Some(NorthEast),
        (_, true, true, _) => Some(SouthWest),
        (_, true, _, true) => Some(SouthEast),
        (true, _, _, _) => Some(North),
        (_, true, _, _) => Some(South),
        (_, _, true, _) => Some(West),
        (_, _, _, true) => Some(East),
        _ => None,
    };
    if let Some(d) = dir {
        return Some(Zone::Resize(d));
    }
    if y >= 0.0 && y < caption {
        return Some(Zone::Caption);
    }
    None
}

/// Convert a window-local cursor position (physical px) into card-local
/// (logical px), matching the coord space widget code uses.
fn cursor_card_local(cursor: PhysicalPosition<f64>, scale: f64) -> (f32, f32) {
    let lp_x = cursor.x / scale;
    let lp_y = cursor.y / scale;
    ((lp_x - CARD_INSET_LP) as f32, (lp_y - CARD_INSET_LP) as f32)
}

/// Card content width in card-local logical pixels (window minus 2× card inset).
fn card_content_width(size: PhysicalSize<u32>, scale: f64) -> f32 {
    let logical_w = size.width as f64 / scale;
    (logical_w - 2.0 * CARD_INSET_LP) as f32
}

/// Card content height in card-local logical pixels.
fn card_content_height(size: PhysicalSize<u32>, scale: f64) -> f32 {
    let logical_h = size.height as f64 / scale;
    (logical_h - 2.0 * CARD_INSET_LP) as f32
}

fn rect_contains(rect: &skia_safe::Rect, p: (f32, f32)) -> bool {
    p.0 >= rect.left && p.0 <= rect.right && p.1 >= rect.top && p.1 <= rect.bottom
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
) -> bool {
    let Some(fonts) = fonts else {
        return false;
    };
    // Account tab — custom layout. Update row / remove / add-button hover.
    if tab == SettingsTab::Account {
        let layout = screens::settings::account_tab_layout(fonts, card_w, account_uuids.len());
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
) -> (bool, bool) {
    let Some(fonts) = fonts else {
        return (false, false);
    };

    // Account tab — custom layout, handled outside the row-grid dispatch.
    // Remove buttons are nested inside row rects, so test them first.
    if tab == SettingsTab::Account {
        let layout = screens::settings::account_tab_layout(fonts, card_w, account_uuids.len());
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
                if let Some(form) = modal.try_submit() {
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

fn close_other_modal_dropdowns(modal: &mut NewInstanceModalState, keep: ModalSlot) {
    if keep != ModalSlot::Version {
        modal.version.close();
    }
    if keep != ModalSlot::Loader {
        modal.loader.close();
    }
}

/// Remove an instance by underlying index. Adjusts `prefs.selected` so
/// it still points at a valid instance (or the last one, if the user
/// deleted the currently-selected one) and persists. Refuses to delete
/// the last remaining instance — there must always be at least one.
fn delete_instance(
    instances: &mut Vec<Instance>,
    prefs: &mut InstancePrefs,
    underlying_idx: usize,
    time: f32,
) {
    if underlying_idx >= instances.len() || instances.len() <= 1 {
        log::info!("delete: refused (idx={} len={})", underlying_idx, instances.len());
        return;
    }
    let removed_name = instances[underlying_idx].name.clone();
    instances.remove(underlying_idx);

    // Re-anchor selection. If we removed something below the cursor,
    // shift back. If we removed the cursor itself, clamp to the new last
    // index.
    if prefs.selected > underlying_idx {
        prefs.selected -= 1;
    } else if prefs.selected == underlying_idx {
        prefs.selected = prefs.selected.min(instances.len().saturating_sub(1));
    }
    prefs.sync_from_instance(instances);
    prefs.detail_scroll = 0.0;
    prefs.selected_at = Some(time); // play the detail-panel fade for the new view
    prefs.delete_hover = None;
    prefs.list_hover = None;

    log::info!("delete: removed \"{}\"", removed_name);
    persistence::save_instances(instances);
}

/// Mirror the prefs slider/dropdown values into the currently-selected
/// instance. Called whenever those widgets fire a change event so the
/// per-instance config follows the user's edits.
fn sync_instance_config(instances: &mut Vec<Instance>, prefs: &InstancePrefs) {
    if let Some(inst) = instances.get_mut(prefs.selected) {
        inst.ram = prefs.ram.value as u32;
        inst.render_distance = prefs.render_dist.value as u32;
        inst.java_runtime = prefs.java_runtime.selected;
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
    // "Ewo (development)" → Ewo with the hard-coded dev manifest URL;
    // anything else → Vanilla. Other loaders will land here once the
    // dropdown grows back.
    let loader = if form.loader.starts_with("Ewo") {
        ewo_render::screens::instances::InstanceLoader::Ewo {
            manifest_url: DEV_EWO_LOADER_URL.to_string(),
        }
    } else {
        ewo_render::screens::instances::InstanceLoader::Vanilla
    };
    // Derived before `loader` is moved into `Instance::with_loader` below.
    // The job needs the manifest URL up front so it can fetch + merge
    // before counting bytes for the progress bar.
    let loader_spec = match &loader {
        ewo_render::screens::instances::InstanceLoader::Vanilla => None,
        ewo_render::screens::instances::InstanceLoader::Ewo { manifest_url } => {
            Some(loaders::LoaderSpec {
                id: "ewo".to_string(),
                url: manifest_url.clone(),
            })
        }
    };
    // Seed the instance's mods list from the bundled catalog so the
    // Instances UI shows real toggles immediately. Only Ewo instances get
    // mods — vanilla launches don't run any mods so the list stays empty.
    let seeded_mods = match &loader {
        ewo_render::screens::instances::InstanceLoader::Vanilla => Vec::new(),
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

    let event_loop = EventLoop::new().expect("failed to create event loop");
    let mut app = App::new(args.dev);
    event_loop.run_app(&mut app).expect("event loop error");
}

/// Dump the launching screen's in-memory log to
/// `<config>/EwoClient/instances/<name>/logs/<timestamp>.log`.
/// Best-effort — failures log a warning but don't surface. Each line is
/// prefixed with its severity tag so stderr lines stay distinguishable
/// from stdout when grepping.
fn persist_launch_log(
    instance_name: &str,
    lines: &[ewo_render::screens::launching::RealLogLine],
    exit_code: Option<i32>,
) {
    use std::io::Write;
    if lines.is_empty() {
        return;
    }
    let Some(mut path) = downloads::paths::instance_dir(instance_name) else {
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
