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

use ewo_core::{motion, Settings, Theme};

mod common;
use common::{fonts, render, scenes, H, W};

const GOLDEN: &str = include_str!("golden/default_theme.txt");
/// Allowed per-channel difference between two block-mean thumbnails when the
/// full-frame hash differs.
const THUMB_TOLERANCE: i32 = 3;

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
