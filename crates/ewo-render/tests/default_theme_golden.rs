//! The default (Velvet, hue shift 0) theme must render the same pixels no
//! matter how the colours reach the canvas. This pins full-card renders of
//! every launcher screen and modal against a committed golden file
//! (`tests/golden/default_theme.txt`), so routing colours through `Theme`
//! tokens (or any other refactor of the colour plumbing) cannot silently
//! shift a pixel at the default theme.
//!
//! Each scene is a CPU raster of `draw_frame` at a fixed clock. Pearl dust
//! and petals are randomly seeded, so scenes run with `density = 0` (no
//! motes). Two checks per scene:
//!
//! * the FNV-1a hash of every byte must match (the byte-equality check), or,
//!   if the hash differs,
//! * a 24x16 block-mean thumbnail must match within a few levels. Skia's CPU
//!   raster pipeline can round differently between CPU generations, so a
//!   different machine may land a handful of 1-level differences that a hash
//!   cannot forgive. A theme regression is orders of magnitude larger.
//!
//! Regenerate after an *intentional* rendering change (and say why in the
//! commit): `EWO_UPDATE_GOLDEN=1 cargo test -p ewo-render --test
//! default_theme_golden`.

use std::fmt::Write as _;

use ewo_core::{motion, Screen, Settings, Theme};
use ewo_render::app_window::draw_frame;
use ewo_render::backdrop::Backdrop;
use ewo_render::screens::{
    self, AboutModalState, DevOverlayState, FrameStats, FriendsPrefs, FriendsViewState,
    InstancePrefs, LaunchError, LauncherLinkModalState, LaunchingState, LinkRedeemView,
    NewInstanceModalState, Prefs, ServerWidgetView, SettingsTab,
};
use ewo_render::skia_safe::{surfaces, ImageInfo};
use ewo_render::text::HoverGlowState;
use ewo_render::{FontStore, VbtnState};

const W: i32 = 960;
const H: i32 = 640;
const GOLDEN: &str = include_str!("golden/default_theme.txt");
/// Allowed per-channel difference between two block-mean thumbnails when the
/// full-frame hash differs.
const THUMB_TOLERANCE: i32 = 3;

struct Scene {
    name: &'static str,
    screen: Screen,
    settings_tab: SettingsTab,
    time: f32,
    about: bool,
    new_instance: bool,
    link: bool,
    dev: bool,
    /// Hover/press states lit everywhere the screen has them.
    hover: bool,
    /// New-instance modal with its empty-name error showing.
    name_error: bool,
    launch_error: Option<LaunchError>,
}

impl Scene {
    const fn new(name: &'static str, screen: Screen) -> Self {
        Self {
            name,
            screen,
            settings_tab: SettingsTab::Account,
            time: 3.7,
            about: false,
            new_instance: false,
            link: false,
            dev: false,
            hover: false,
            name_error: false,
            launch_error: None,
        }
    }
}

fn scenes() -> Vec<Scene> {
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

fn fonts() -> Option<FontStore> {
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
fn render(fonts: &FontStore, scene: &Scene, theme: &Theme, settings: &Settings) -> Vec<u8> {
    let mut surface = surfaces::raster_n32_premul((W, H)).expect("raster surface");
    let backdrop = Backdrop::new(W as f32, H as f32, settings);

    let mut about = AboutModalState::default();
    if scene.about {
        about.open();
        about.anim = 1.0;
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
    let mut instance_prefs = InstancePrefs::default();
    instance_prefs.mods_on = vec![true; 8];

    draw_frame(
        surface.canvas(),
        &backdrop,
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

    let info = ImageInfo::new_n32_premul((W, H), None);
    let stride = (W * 4) as usize;
    let mut px = vec![0u8; stride * H as usize];
    assert!(surface.read_pixels(&info, &mut px, stride, (0, 0)));
    px
}

fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325, |h, b| {
        (h ^ *b as u64).wrapping_mul(0x100000001b3)
    })
}

const TX: i32 = 24;
const TY: i32 = 16;

/// 24x16 block means of the four channels.
fn thumbnail(px: &[u8]) -> Vec<u8> {
    let (bw, bh) = ((W / TX) as usize, (H / TY) as usize);
    let mut out = Vec::new();
    for ty in 0..TY as usize {
        for tx in 0..TX as usize {
            let mut sum = [0u32; 4];
            for y in ty * bh..(ty + 1) * bh {
                for x in tx * bw..(tx + 1) * bw {
                    let i = (y * W as usize + x) * 4;
                    for c in 0..4 {
                        sum[c] += px[i + c] as u32;
                    }
                }
            }
            out.extend(sum.iter().map(|s| (s / (bw * bh) as u32) as u8));
        }
    }
    out
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len() / 2)
        .map(|i| u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).expect("hex"))
        .collect()
}

fn golden_entry(name: &str) -> Option<(u64, Vec<u8>)> {
    GOLDEN.lines().find_map(|l| {
        let mut it = l.split_whitespace();
        (it.next()? == name).then(|| {
            let hash = u64::from_str_radix(it.next().unwrap(), 16).unwrap();
            (hash, unhex(it.next().unwrap()))
        })
    })
}

/// Settings the golden was recorded with: defaults, minus the random motes.
pub fn golden_settings() -> Settings {
    Settings {
        density: 0.0,
        ..Settings::default()
    }
}

#[test]
fn default_theme_pixels_match_golden() {
    let Some(fonts) = fonts() else { return };
    motion::set_reduced(false);
    let settings = golden_settings();
    let theme = Theme::VELVET;

    if std::env::var_os("EWO_UPDATE_GOLDEN").is_some() {
        let mut out = String::new();
        for scene in scenes() {
            let px = render(&fonts, &scene, &theme, &settings);
            let _ = writeln!(
                out,
                "{} {:016x} {}",
                scene.name,
                fnv1a(&px),
                hex(&thumbnail(&px))
            );
        }
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/golden/default_theme.txt"
        );
        std::fs::write(path, out).expect("write golden");
        eprintln!("golden rewritten: {path}");
        return;
    }

    let mut failures = Vec::new();
    for scene in scenes() {
        let (want_hash, want_thumb) = golden_entry(scene.name).unwrap_or_else(|| {
            panic!(
                "no golden for scene `{}`; run with EWO_UPDATE_GOLDEN=1",
                scene.name
            )
        });
        let px = render(&fonts, &scene, &theme, &settings);
        if fnv1a(&px) == want_hash {
            continue;
        }
        let got = thumbnail(&px);
        let worst = got
            .iter()
            .zip(&want_thumb)
            .map(|(a, b)| (*a as i32 - *b as i32).abs())
            .max()
            .unwrap_or(0);
        if worst > THUMB_TOLERANCE {
            failures.push(format!("{} (block mean off by {worst})", scene.name));
        } else {
            eprintln!(
                "note: `{}` hash differs but block means are within {worst} levels \
                 (CPU raster rounding)",
                scene.name
            );
        }
    }
    assert!(
        failures.is_empty(),
        "default-theme pixels changed: {}",
        failures.join(", ")
    );
}

/// The frames really do contain the screens (a blank golden would pass
/// vacuously): every scene must differ from every other.
#[test]
fn scenes_are_distinct() {
    let Some(fonts) = fonts() else { return };
    motion::set_reduced(false);
    let settings = golden_settings();
    let theme = Theme::VELVET;
    let mut seen = std::collections::HashMap::new();
    for scene in scenes() {
        let h = fnv1a(&render(&fonts, &scene, &theme, &settings));
        if let Some(prev) = seen.insert(h, scene.name) {
            panic!("scenes `{prev}` and `{}` render identically", scene.name);
        }
    }
}
