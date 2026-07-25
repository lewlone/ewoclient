//! Held-item models as the renderer sees them (M22).
//!
//! Mirrors `rewo_data::held_items` so `rewo-gpu` keeps no `rewo-data`
//! dependency — the same pattern [`crate::mobs::SwingKind`],
//! [`crate::mobs::AllayDance`] and [`crate::mobs::MobCombat`] already follow.
//! The app converts across the seam; the shapes are deliberately identical so
//! that conversion stays mechanical and reviewable.

use std::collections::HashMap;

/// A `display` entry, already through vanilla's `ItemTransform.Deserializer`
/// (translation × 0.0625 then clamped ±5, scale clamped ±4).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DisplayTransform {
    /// Degrees, XYZ.
    pub rotation: [f32; 3],
    /// Block units.
    pub translation: [f32; 3],
    pub scale: [f32; 3],
}

impl Default for DisplayTransform {
    fn default() -> Self {
        Self {
            rotation: [0.0; 3],
            translation: [0.0; 3],
            scale: [1.0; 3],
        }
    }
}

/// One quad of a held item: corners in model units `0..16`, UVs in `0..1` of
/// [`Self::tex`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HeldQuad {
    pub verts: [[f32; 3]; 4],
    pub uv: [[f32; 2]; 4],
    pub tex: u16,
    /// Vanilla `Direction` ordinal, for directional shading.
    pub dir: u8,
}

/// A baked held item.
#[derive(Clone, Debug, PartialEq)]
pub struct HeldItemModel {
    pub quads: Vec<HeldQuad>,
    pub right: DisplayTransform,
    pub left: DisplayTransform,
    pub from_block: bool,
}

/// One texture a held item samples.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeldTexture {
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
}

/// Every baked held item plus the textures they reference.
#[derive(Clone, Debug, Default)]
pub struct HeldItems {
    pub models: HashMap<String, HeldItemModel>,
    pub textures: Vec<HeldTexture>,
}

/// `ItemInHandLayer.submitArmWithItem`'s hand offset, in block units.
///
/// ```text
/// offsetX = baby ? 0.0 : 1.0
/// offsetY = baby ? 1.0 : 2.0
/// offsetZ = baby ? -4.5 : -10.0
/// translate((left ? -1 : 1) * offsetX/16, offsetY/16, offsetZ/16)
/// ```
pub fn hand_offset(left: bool, baby: bool) -> [f32; 3] {
    let (ox, oy, oz) = if baby {
        (0.0f32, 1.0f32, -4.5f32)
    } else {
        (1.0, 2.0, -10.0)
    };
    [
        (if left { -1.0 } else { 1.0 }) * ox / 16.0,
        oy / 16.0,
        oz / 16.0,
    ]
}

/// Apply `ItemTransform.apply(applyLeftHandFix, pose)` to a point.
///
/// A `PoseStack` transforms the coordinate system, so the point runs through
/// the chain in reverse order of the calls: centre by −0.5, scale, rotate,
/// then translate. The left-hand fix negates the x translation and the y and z
/// rotations — **not** the x rotation, which is a detail worth not
/// symmetrising by accident.
pub fn apply_display(t: &DisplayTransform, left: bool, p: [f32; 3]) -> [f32; 3] {
    let (tx, ry, rz) = if left {
        (-t.translation[0], -t.rotation[1], -t.rotation[2])
    } else {
        (t.translation[0], t.rotation[1], t.rotation[2])
    };
    // translate(-0.5) — the model occupies 0..1 block units.
    let mut v = [p[0] - 0.5, p[1] - 0.5, p[2] - 0.5];
    v = [v[0] * t.scale[0], v[1] * t.scale[1], v[2] * t.scale[2]];
    v = rotate_xyz(v, [t.rotation[0], ry, rz]);
    [v[0] + tx, v[1] + t.translation[1], v[2] + t.translation[2]]
}

/// JOML's `Quaternionf.rotationXYZ(x, y, z)` applied to a vector — the
/// rotation is X first, then Y, then Z (`M = Rz·Ry·Rx`), which is the same
/// convention [`crate::mobs::rotate_zyx`] implements for model parts.
pub fn rotate_xyz(v: [f32; 3], deg: [f32; 3]) -> [f32; 3] {
    const D: f32 = std::f32::consts::PI / 180.0;
    crate::mobs::rotate_zyx(v, [deg[0] * D, deg[1] * D, deg[2] * D])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_identity_transform_only_centres() {
        let t = DisplayTransform::default();
        // `NO_TRANSFORM` is exactly `translate(-0.5, -0.5, -0.5)`.
        assert_eq!(apply_display(&t, false, [0.5, 0.5, 0.5]), [0.0, 0.0, 0.0]);
        assert_eq!(apply_display(&t, false, [1.0, 1.0, 1.0]), [0.5, 0.5, 0.5]);
    }

    #[test]
    fn the_left_hand_fix_negates_x_translation_and_yz_rotation_only() {
        let t = DisplayTransform {
            rotation: [90.0, 0.0, 0.0],
            translation: [0.25, 0.0, 0.0],
            scale: [1.0; 3],
        };
        let r = apply_display(&t, false, [0.5, 0.5, 0.5]);
        let l = apply_display(&t, true, [0.5, 0.5, 0.5]);
        // The centred point is the origin, so only the translation shows — and
        // it mirrors. The x rotation is NOT negated, but with a centred point
        // that is invisible, so this pins the translation half.
        assert!((r[0] - 0.25).abs() < 1e-6, "{r:?}");
        assert!((l[0] + 0.25).abs() < 1e-6, "{l:?}");
    }

    #[test]
    fn scale_applies_before_rotation() {
        // A 90 deg Z rotation maps +x to +y; scaling x by 2 first must show up
        // on the y axis afterwards, which distinguishes S-then-R from R-then-S.
        let t = DisplayTransform {
            rotation: [0.0, 0.0, 90.0],
            translation: [0.0; 3],
            scale: [2.0, 1.0, 1.0],
        };
        let p = apply_display(&t, false, [1.0, 0.5, 0.5]); // centred -> (0.5,0,0)
        assert!((p[0]).abs() < 1e-5, "{p:?}");
        assert!((p[1] - 1.0).abs() < 1e-5, "x scaled by 2 then rotated: {p:?}");
    }

    #[test]
    fn the_hand_offset_matches_the_layer_and_mirrors_for_the_left() {
        assert_eq!(hand_offset(false, false), [1.0 / 16.0, 2.0 / 16.0, -10.0 / 16.0]);
        assert_eq!(hand_offset(true, false), [-1.0 / 16.0, 2.0 / 16.0, -10.0 / 16.0]);
        // A baby's offsets differ on all three axes, and x is zero so it does
        // not mirror.
        assert_eq!(hand_offset(false, true), [0.0, 1.0 / 16.0, -4.5 / 16.0]);
        assert_eq!(hand_offset(true, true), [0.0, 1.0 / 16.0, -4.5 / 16.0]);
    }
}
