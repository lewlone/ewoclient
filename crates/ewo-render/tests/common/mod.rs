//! Shared scene renderer for the full-card render tests: one CPU-raster
//! `draw_frame` of a named screen or modal state.

#![allow(dead_code)]

use ewo_core::{Screen, Settings, Theme};
use ewo_render::app_window::draw_frame;
use ewo_render::backdrop::Backdrop;
use ewo_render::screens::{
    self, AboutModalState, DeleteInstanceModalState, DevOverlayState, FrameStats, FriendsPrefs,
    FriendsViewState, InstancePrefs, LaunchError, LauncherLinkModalState, LaunchingState,
    LinkRedeemView, NewInstanceModalState, Prefs, ServerWidgetView, SettingsTab,
};
use ewo_render::skia_safe::{surfaces, AlphaType, ColorType, ImageInfo};
use ewo_render::text::HoverGlowState;
use ewo_render::{FontStore, VbtnState};

pub const W: i32 = 960;
pub const H: i32 = 640;

pub struct Scene {
    pub name: &'static str,
    pub screen: Screen,
    pub settings_tab: SettingsTab,
    pub time: f32,
    pub about: bool,
    pub delete: bool,
    pub new_instance: bool,
    pub link: bool,
    pub dev: bool,
    /// Hover/press states lit everywhere the screen has them.
    pub hover: bool,
    /// New-instance modal with its empty-name error showing.
    pub name_error: bool,
    pub launch_error: Option<LaunchError>,
}

impl Scene {
    pub const fn new(name: &'static str, screen: Screen) -> Self {
        Self {
            name,
            screen,
            settings_tab: SettingsTab::Account,
            time: 3.7,
            about: false,
            delete: false,
            new_instance: false,
            link: false,
            dev: false,
            hover: false,
            name_error: false,
            launch_error: None,
        }
    }
}

pub fn scenes() -> Vec<Scene> {
    let tab = |name, tab| Scene {
        settings_tab: tab,
        ..Scene::new(name, Screen::Settings)
    };
    vec![
        Scene::new("main_menu", Screen::MainMenu),
        Scene::new("instances", Screen::Instances),
        Scene::new("friends", Screen::Friends),
        Scene::new("launching", Screen::Launching),
        Scene {
            launch_error: Some(LaunchError::Rose),
            ..Scene::new("launching_error", Screen::Launching)
        },
        tab("settings_account", SettingsTab::Account),
        tab("settings_profiles", SettingsTab::Profiles),
        tab("settings_keybinds", SettingsTab::Keybinds),
        tab("settings_modules", SettingsTab::Modules),
        tab("settings_graphics", SettingsTab::Graphics),
        tab("settings_audio", SettingsTab::Audio),
        tab("settings_paths", SettingsTab::Paths),
        tab("settings_advanced", SettingsTab::Advanced),
        Scene {
            about: true,
            ..Scene::new("about_modal", Screen::MainMenu)
        },
        Scene {
            delete: true,
            ..Scene::new("delete_instance_modal", Screen::Instances)
        },
        Scene {
            new_instance: true,
            ..Scene::new("new_instance_modal", Screen::Instances)
        },
        Scene {
            link: true,
            ..Scene::new("launcher_link_modal", Screen::Settings)
        },
        Scene {
            dev: true,
            ..Scene::new("dev_overlay", Screen::MainMenu)
        },
        Scene {
            new_instance: true,
            name_error: true,
            ..Scene::new("new_instance_modal_error", Screen::Instances)
        },
        Scene {
            hover: true,
            ..Scene::new("main_menu_hover", Screen::MainMenu)
        },
        Scene {
            hover: true,
            ..Scene::new("instances_hover", Screen::Instances)
        },
        Scene {
            hover: true,
            ..Scene::new("settings_hover", Screen::Settings)
        },
    ]
}

pub fn fonts() -> Option<FontStore> {
    if !std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/fonts")).exists() {
        if std::env::var_os("REWO_REQUIRE_ASSETS").is_some() {
            panic!("assets/fonts missing");
        }
        eprintln!("SKIP: assets/fonts missing");
        return None;
    }
    Some(FontStore::new())
}

/// Premultiplied RGBA bytes of one full `draw_frame`.
pub fn render(fonts: &FontStore, scene: &Scene, theme: &Theme, settings: &Settings) -> Vec<u8> {
    let backdrop = Backdrop::new(W as f32, H as f32, settings);
    render_scaled(fonts, scene, theme, settings, 1.0, &backdrop)
}

/// Like [`render`] on a display with `scale` device pixels per logical pixel,
/// the way the launcher does it: the surface is `W x H` logical times `scale`
/// physical pixels, the canvas carries a `scale` CTM, and `draw_frame` gets
/// the logical size. Returns the physical pixels.
pub fn render_scaled(
    fonts: &FontStore,
    scene: &Scene,
    theme: &Theme,
    settings: &Settings,
    scale: f32,
    backdrop: &Backdrop,
) -> Vec<u8> {
    let (pw, ph) = physical(scale);
    let mut surface = surfaces::raster_n32_premul((pw, ph)).expect("raster surface");
    if scale != 1.0 {
        surface.canvas().scale((scale, scale));
    }

    let mut about = AboutModalState::default();
    if scene.about {
        about.open();
        about.anim = 1.0;
    }
    let mut delete = DeleteInstanceModalState::default();
    if scene.delete {
        delete.open(
            "velvet-hours-1a2b3c",
            "Velvet Hours",
            "instances/velvet-hours-1a2b3c",
        );
        delete.anim = 1.0;
    }
    let mut modal = NewInstanceModalState::default();
    if scene.new_instance {
        modal.open();
        modal.anim = 1.0;
        modal.name_error = scene.name_error;
    }
    let lit = VbtnState {
        hover: scene.hover,
        hover_anim: if scene.hover { 1.0 } else { 0.0 },
        ..Default::default()
    };
    let mut link = LauncherLinkModalState::default();
    if scene.link {
        link.open();
        link.anim = 1.0;
    }
    let mut launching = LaunchingState::default();
    if scene.screen == Screen::Launching {
        launching.enter(0.5, "Velvet Hours", "VANILLA · 1.21 · ADOPTIUM 21");
        if let Some(variant) = scene.launch_error {
            launching.trigger_error(variant, 3.0);
        }
    }
    let dev = scene.dev.then(|| DevOverlayState::from_settings(settings));
    let instances = screens::instances::default_instances();
    let instance_prefs = InstancePrefs {
        mods_on: vec![true; 8],
        ..InstancePrefs::default()
    };

    draw_frame(
        surface.canvas(),
        backdrop,
        fonts,
        W as u32,
        H as u32,
        scene.time,
        theme,
        settings,
        scene.screen,
        scene.hover.then_some(Screen::Instances),
        &lit,
        &[lit; 4],
        scene.settings_tab,
        &Prefs::default(),
        &instance_prefs,
        &launching,
        &modal,
        &about,
        &delete,
        &link,
        LinkRedeemView::Idle,
        dev.as_ref(),
        FrameStats::default(),
        &instances,
        HoverGlowState::default(),
        screens::settings::AccountView {
            accounts: &[],
            op: screens::settings::AccountOpView::Idle,
            link_status: screens::settings::LinkStatusView::LinkedNeedsLauncherLink,
        },
        screens::settings::ProfileView { profiles: &[] },
        screens::settings::KeybindView { rows: &[] },
        &FriendsPrefs::default(),
        FriendsViewState::NotLinked,
        ServerWidgetView::default(),
        1.0,
        scene.hover,
        scene.hover,
        scene.hover,
    );

    // Explicit RGBA: `n32` is BGRA on Windows and RGBA on Linux, which would
    // make the golden hashes platform-specific.
    let info = ImageInfo::new((pw, ph), ColorType::RGBA8888, AlphaType::Premul, None);
    let stride = (pw * 4) as usize;
    let mut px = vec![0u8; stride * ph as usize];
    assert!(surface.read_pixels(&info, &mut px, stride, (0, 0)));
    px
}

/// Physical size of the `W x H` logical card at `scale`.
pub fn physical(scale: f32) -> (i32, i32) {
    (
        (W as f32 * scale).round() as i32,
        (H as f32 * scale).round() as i32,
    )
}
