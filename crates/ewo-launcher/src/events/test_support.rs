//! Shared helpers for the click-handler unit tests.

use ewo_render::{skia_safe, FontStore};

/// The bundled fonts, or `None` (with a `SKIP:` line) when `assets/fonts` is
/// missing. Panics instead under `REWO_REQUIRE_ASSETS`, like every other
/// asset-dependent test in the workspace.
pub(crate) fn fonts() -> Option<FontStore> {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/fonts");
    if !std::path::Path::new(dir).exists() {
        if std::env::var_os("REWO_REQUIRE_ASSETS").is_some() {
            panic!("assets/fonts missing");
        }
        eprintln!("SKIP: assets/fonts missing");
        return None;
    }
    Some(FontStore::new())
}

/// Centre of a rect: a point that is inside it and, for nested widgets,
/// inside nothing the rect doesn't cover.
pub(crate) fn center(r: &skia_safe::Rect) -> (f32, f32) {
    ((r.left + r.right) * 0.5, (r.top + r.bottom) * 0.5)
}

/// A card-local point far outside every widget of the 1180x720 launcher.
pub(crate) const NOWHERE: (f32, f32) = (-50.0, -50.0);

/// The launcher's default card size in logical pixels.
pub(crate) const CARD_W: f32 = 1180.0;
pub(crate) const CARD_H: f32 = 720.0;
