//! Custom crosshair — config + rendering + persistence.
//!
//! Two surfaces:
//!
//! 1. **In-world crosshair**, drawn at the exact screen center every paint.
//!    Replaces vanilla's crosshair when `CrosshairConfig::enabled` is true
//!    (Java side reads the suppress flag via `nativeIsCustomCrosshairEnabled`
//!    and cancels the vanilla `Gui.extractCrosshair` mixin target).
//! 2. **Editor view**, rendered in the overlay's CROSSHAIR tab — sliders +
//!    toggles + colour swatches + a live preview at large scale. The editor
//!    mutates this same config and writes it back to disk on change.
//!
//! Config lives at `<profile>/crosshair.toml` (per client profile, same as
//! [`hud.toml`]); a missing file falls back to defaults so a fresh profile
//! always has a sensible-looking crosshair.

use std::path::PathBuf;

use skia_safe::{Canvas, Color4f, Paint, PaintStyle, Rect};

/// Three target states the crosshair adapts to, each with its own colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CrosshairState {
    /// No entity under the crosshair.
    Idle,
    /// Entity under the crosshair, but distance > `reach_distance`.
    Target,
    /// Entity under the crosshair, distance ≤ `reach_distance` (= can hit).
    Reach,
}

/// Persisted shape + colour for the custom crosshair.
///
/// All sizes are in **logical screen pixels** — the crosshair always renders
/// at the framebuffer's exact centre, so the values map directly to the
/// pixel grid you see in-game. `arm_length = 0` collapses an arm; combine
/// with `dot_enabled = true` for a dot-only crosshair.
#[derive(Clone, Copy, Debug)]
pub struct CrosshairConfig {
    /// Replace vanilla crosshair with this one. The Java mixin reads this
    /// (via `nativeIsCustomCrosshairEnabled`) and cancels vanilla's render
    /// whenever it's true; the in-world paint here always draws when true.
    pub enabled: bool,
    /// Each arm's length in pixels (0 = arm collapsed).
    pub arm_length: f32,
    /// Gap from the centre to the start of each arm.
    pub arm_gap: f32,
    /// Arm thickness.
    pub arm_thickness: f32,
    /// Show the centre dot.
    pub dot_enabled: bool,
    /// Centre-dot diameter.
    pub dot_size: f32,
    /// Draw a 1px halo behind the strokes for readability against bright
    /// backgrounds (sky, snow). Almost always wanted.
    pub outline_enabled: bool,
    /// Extra half-pixels the outline pushes outward from the stroke edges.
    pub outline_thickness: f32,
    /// Anti-alias the strokes. Off = pixel-perfect; on = sub-pixel smooth.
    pub anti_alias: bool,
    /// Colour while no entity sits under the crosshair.
    pub color_idle: [u8; 4],
    /// Colour while an entity sits under the crosshair but is out of reach.
    pub color_target: [u8; 4],
    /// Colour while the targeted entity is within `reach_distance`.
    pub color_reach: [u8; 4],
    /// Outline colour (usually a near-black with alpha for the halo).
    pub outline_color: [u8; 4],
    /// Distance threshold (blocks) below which the `Reach` colour kicks in.
    pub reach_distance: f32,
}

impl CrosshairConfig {
    pub fn defaults() -> Self {
        Self {
            enabled: false,
            arm_length: 6.0,
            arm_gap: 2.0,
            arm_thickness: 2.0,
            dot_enabled: false,
            dot_size: 2.0,
            outline_enabled: true,
            outline_thickness: 1.0,
            anti_alias: false,
            color_idle: [255, 255, 255, 255],
            // Velvet champagne — "I see you, but I can't hit you yet".
            color_target: [232, 212, 168, 255],
            // Velvet rose — "you're in range".
            color_reach: [229, 184, 197, 255],
            outline_color: [0, 0, 0, 220],
            reach_distance: 3.0,
        }
    }

    /// Map a target-distance read off `EwoHudData` into a colour state.
    /// `target_active = false` always reads as `Idle`.
    pub fn state_from_target(&self, target_active: bool, distance: f32) -> CrosshairState {
        if !target_active {
            CrosshairState::Idle
        } else if distance <= self.reach_distance {
            CrosshairState::Reach
        } else {
            CrosshairState::Target
        }
    }

    /// Resolve the active fill colour for the given state.
    pub fn color_for(&self, state: CrosshairState) -> [u8; 4] {
        match state {
            CrosshairState::Idle => self.color_idle,
            CrosshairState::Target => self.color_target,
            CrosshairState::Reach => self.color_reach,
        }
    }

    /// Sanity-clamp a freshly-loaded or just-edited config so a wild slider
    /// drag (or hand-edit of `crosshair.toml`) can't paint outside its lane.
    pub fn clamp(&mut self) {
        self.arm_length = self.arm_length.clamp(0.0, 24.0);
        self.arm_gap = self.arm_gap.clamp(0.0, 12.0);
        self.arm_thickness = self.arm_thickness.clamp(1.0, 8.0);
        self.dot_size = self.dot_size.clamp(1.0, 8.0);
        self.outline_thickness = self.outline_thickness.clamp(0.0, 3.0);
        self.reach_distance = self.reach_distance.clamp(1.5, 6.0);
    }
}

impl Default for CrosshairConfig {
    fn default() -> Self {
        Self::defaults()
    }
}

/// Convert a saved `[u8; 4]` into a Skia `Color4f`.
pub fn color4f(rgba: [u8; 4]) -> Color4f {
    Color4f::new(
        rgba[0] as f32 / 255.0,
        rgba[1] as f32 / 255.0,
        rgba[2] as f32 / 255.0,
        rgba[3] as f32 / 255.0,
    )
}

/// Paint the crosshair at `(cx, cy)` using `config` and the resolved
/// `state` colour. Used both by the in-world paint (cx = w/2, cy = h/2) and
/// by the editor preview (cx, cy = centre of the preview rect, optional
/// `scale` to enlarge the strokes for readability at the preview size).
pub fn draw(canvas: &Canvas, cx: f32, cy: f32, config: &CrosshairConfig, state: CrosshairState) {
    draw_scaled(canvas, cx, cy, config, state, 1.0);
}

/// Same as [`draw`] but multiplies every linear dimension by `scale`. The
/// editor preview uses this to show the crosshair big enough to read.
pub fn draw_scaled(
    canvas: &Canvas,
    cx: f32,
    cy: f32,
    config: &CrosshairConfig,
    state: CrosshairState,
    scale: f32,
) {
    let fill = color4f(config.color_for(state));
    let outline = color4f(config.outline_color);

    let arm_length = config.arm_length * scale;
    let arm_gap = config.arm_gap * scale;
    let arm_thickness = config.arm_thickness * scale;
    let dot_size = config.dot_size * scale;
    let outline_inflate = config.outline_thickness * scale;

    // Paint pass: outline first (so the fill is on top), then fill.
    if config.outline_enabled && outline_inflate > 0.0 {
        paint_arms_and_dot(
            canvas,
            cx,
            cy,
            arm_length,
            arm_gap,
            arm_thickness,
            config.dot_enabled,
            dot_size,
            outline_inflate,
            outline,
            config.anti_alias,
        );
    }
    paint_arms_and_dot(
        canvas,
        cx,
        cy,
        arm_length,
        arm_gap,
        arm_thickness,
        config.dot_enabled,
        dot_size,
        0.0,
        fill,
        config.anti_alias,
    );
}

/// Internal: paint each arm (and the optional dot) as a rectangle inflated
/// by `inflate` on every side. Used twice per call — once at
/// `inflate = outline_thickness` for the halo, once at `inflate = 0` for the
/// fill.
#[allow(clippy::too_many_arguments)]
fn paint_arms_and_dot(
    canvas: &Canvas,
    cx: f32,
    cy: f32,
    arm_length: f32,
    arm_gap: f32,
    arm_thickness: f32,
    dot_enabled: bool,
    dot_size: f32,
    inflate: f32,
    color: Color4f,
    anti_alias: bool,
) {
    let mut paint = Paint::default();
    paint.set_anti_alias(anti_alias);
    paint.set_style(PaintStyle::Fill);
    paint.set_color4f(color, None);

    let half_th = arm_thickness * 0.5 + inflate;

    if arm_length > 0.0 {
        // Top arm
        let top = Rect::from_xywh(
            cx - half_th,
            cy - arm_gap - arm_length - inflate,
            arm_thickness + 2.0 * inflate,
            arm_length + inflate,
        );
        canvas.draw_rect(top, &paint);
        // Bottom arm
        let bottom = Rect::from_xywh(
            cx - half_th,
            cy + arm_gap,
            arm_thickness + 2.0 * inflate,
            arm_length + inflate,
        );
        canvas.draw_rect(bottom, &paint);
        // Left arm
        let left = Rect::from_xywh(
            cx - arm_gap - arm_length - inflate,
            cy - half_th,
            arm_length + inflate,
            arm_thickness + 2.0 * inflate,
        );
        canvas.draw_rect(left, &paint);
        // Right arm
        let right = Rect::from_xywh(
            cx + arm_gap,
            cy - half_th,
            arm_length + inflate,
            arm_thickness + 2.0 * inflate,
        );
        canvas.draw_rect(right, &paint);
    }

    if dot_enabled && dot_size > 0.0 {
        let half = dot_size * 0.5 + inflate;
        let rect = Rect::from_xywh(cx - half, cy - half, dot_size + 2.0 * inflate, dot_size + 2.0 * inflate);
        canvas.draw_rect(rect, &paint);
    }
}

// ────────────────────────────────────────────────────────────────────────
// Persistence — `<profile>/crosshair.toml`.
// ────────────────────────────────────────────────────────────────────────

/// Resolve the active-profile's `crosshair.toml` path. Returns `None` when
/// `%APPDATA%` isn't set; falls back to a `"Default"` profile name when the
/// `profiles.toml` registry isn't readable. Mirrors `hud.rs`'s path resolver.
pub fn crosshair_toml_path() -> Option<PathBuf> {
    let appdata = std::env::var_os("APPDATA")?;
    let profile = crate::hud::read_active_profile().unwrap_or_else(|| "Default".to_string());
    Some(
        PathBuf::from(appdata)
            .join("EwoClient")
            .join("profiles")
            .join(profile)
            .join("crosshair.toml"),
    )
}

/// Load `crosshair.toml` for the active profile. Any missing field falls
/// back to the corresponding default — so a hand-edit that drops half the
/// keys still works, and earlier-schema files survive forward.
pub fn load() -> CrosshairConfig {
    let mut cfg = CrosshairConfig::defaults();
    let Some(path) = crosshair_toml_path() else {
        return cfg;
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return cfg;
    };
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with('[') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim();
        match key {
            "enabled" => cfg.enabled = value == "true",
            "arm_length" => cfg.arm_length = parse_f32(value, cfg.arm_length),
            "arm_gap" => cfg.arm_gap = parse_f32(value, cfg.arm_gap),
            "arm_thickness" => cfg.arm_thickness = parse_f32(value, cfg.arm_thickness),
            "dot_enabled" => cfg.dot_enabled = value == "true",
            "dot_size" => cfg.dot_size = parse_f32(value, cfg.dot_size),
            "outline_enabled" => cfg.outline_enabled = value == "true",
            "outline_thickness" => cfg.outline_thickness = parse_f32(value, cfg.outline_thickness),
            "anti_alias" => cfg.anti_alias = value == "true",
            "color_idle" => cfg.color_idle = parse_color(value, cfg.color_idle),
            "color_target" => cfg.color_target = parse_color(value, cfg.color_target),
            "color_reach" => cfg.color_reach = parse_color(value, cfg.color_reach),
            "outline_color" => cfg.outline_color = parse_color(value, cfg.outline_color),
            "reach_distance" => cfg.reach_distance = parse_f32(value, cfg.reach_distance),
            _ => {}
        }
    }
    cfg.clamp();
    cfg
}

/// Write `crosshair.toml` for the active profile. Silent on I/O failure
/// (the file path is best-effort: missing `%APPDATA%` or a profile-write
/// race shouldn't crash the running game).
pub fn save(cfg: &CrosshairConfig) {
    let Some(path) = crosshair_toml_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let s = format!(
        "# EwoClient custom crosshair — written by the in-game editor.\n\
         enabled = {enabled}\n\
         arm_length = {arm_length:.2}\n\
         arm_gap = {arm_gap:.2}\n\
         arm_thickness = {arm_thickness:.2}\n\
         dot_enabled = {dot_enabled}\n\
         dot_size = {dot_size:.2}\n\
         outline_enabled = {outline_enabled}\n\
         outline_thickness = {outline_thickness:.2}\n\
         anti_alias = {anti_alias}\n\
         color_idle = \"{ci_r},{ci_g},{ci_b},{ci_a}\"\n\
         color_target = \"{ct_r},{ct_g},{ct_b},{ct_a}\"\n\
         color_reach = \"{cr_r},{cr_g},{cr_b},{cr_a}\"\n\
         outline_color = \"{oc_r},{oc_g},{oc_b},{oc_a}\"\n\
         reach_distance = {reach_distance:.3}\n",
        enabled = cfg.enabled,
        arm_length = cfg.arm_length,
        arm_gap = cfg.arm_gap,
        arm_thickness = cfg.arm_thickness,
        dot_enabled = cfg.dot_enabled,
        dot_size = cfg.dot_size,
        outline_enabled = cfg.outline_enabled,
        outline_thickness = cfg.outline_thickness,
        anti_alias = cfg.anti_alias,
        ci_r = cfg.color_idle[0],
        ci_g = cfg.color_idle[1],
        ci_b = cfg.color_idle[2],
        ci_a = cfg.color_idle[3],
        ct_r = cfg.color_target[0],
        ct_g = cfg.color_target[1],
        ct_b = cfg.color_target[2],
        ct_a = cfg.color_target[3],
        cr_r = cfg.color_reach[0],
        cr_g = cfg.color_reach[1],
        cr_b = cfg.color_reach[2],
        cr_a = cfg.color_reach[3],
        oc_r = cfg.outline_color[0],
        oc_g = cfg.outline_color[1],
        oc_b = cfg.outline_color[2],
        oc_a = cfg.outline_color[3],
        reach_distance = cfg.reach_distance,
    );
    let _ = std::fs::write(&path, s);
}

fn parse_f32(value: &str, fallback: f32) -> f32 {
    value.trim().trim_matches('"').parse().unwrap_or(fallback)
}

fn parse_color(value: &str, fallback: [u8; 4]) -> [u8; 4] {
    let raw = value.trim().trim_matches('"');
    let parts: Vec<&str> = raw.split(',').collect();
    if parts.len() != 4 {
        return fallback;
    }
    let mut out = [0u8; 4];
    for (i, part) in parts.iter().enumerate() {
        match part.trim().parse::<u32>() {
            Ok(v) => out[i] = v.min(255) as u8,
            Err(_) => return fallback,
        }
    }
    out
}
