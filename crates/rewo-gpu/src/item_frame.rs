//! Item frames — where `ItemFrameRenderer.submit` puts the frame, its item and
//! a framed map (M194; `client/renderer/entity/ItemFrameRenderer.java:44-97`
//! in the 26.2 decompile). Poses only, relative to the entity's position;
//! nothing here draws.
//!
//! `EntityRenderDispatcher` translates by `renderer.getRenderOffset(state)`
//! before calling `submit`, and `submit` opens with `translate(-renderOffset)`,
//! so the two cancel and the chain below starts at the entity's position.

use glam::{Mat4, Vec3};

/// `Direction.from3DDataValue`: the facing as `add_entity`'s data (and
/// `HangingEntity.DATA_DIRECTION`) carries it — 0 down, 1 up, 2 north,
/// 3 south, 4 west, 5 east. Returns the step vector, or `None` out of range.
pub fn facing_step(facing: u8) -> Option<Vec3> {
    Some(match facing {
        0 => Vec3::new(0.0, -1.0, 0.0),
        1 => Vec3::new(0.0, 1.0, 0.0),
        2 => Vec3::new(0.0, 0.0, -1.0),
        3 => Vec3::new(0.0, 0.0, 1.0),
        4 => Vec3::new(-1.0, 0.0, 0.0),
        5 => Vec3::new(1.0, 0.0, 0.0),
        _ => return None,
    })
}

/// `(xRot, yRot)` in degrees: horizontal facings `(0, 180 - toYRot)` with
/// `toYRot` south 0, west 90, north 180, east 270; vertical ones
/// `(-90 * axisDirection.step, 180)`.
fn facing_rotation(facing: u8) -> Option<(f32, f32)> {
    Some(match facing {
        0 => (90.0, 180.0),  // down: axis step -1
        1 => (-90.0, 180.0), // up: axis step +1
        2 => (0.0, 0.0),     // north: 180 - 180
        3 => (0.0, 180.0),   // south: 180 - 0
        4 => (0.0, 90.0),    // west: 180 - 90
        5 => (0.0, -90.0),   // east: 180 - 270
        _ => return None,
    })
}

/// The chain up to the frame's own space: `translate(step * 0.46875)
/// rotX(xRot) rotY(yRot)`.
pub fn frame_base(facing: u8) -> Option<Mat4> {
    let step = facing_step(facing)?;
    let (x_rot, y_rot) = facing_rotation(facing)?;
    Some(
        Mat4::from_translation(step * 0.46875)
            * Mat4::from_rotation_x(x_rot.to_radians())
            * Mat4::from_rotation_y(y_rot.to_radians()),
    )
}

/// The frame block model's pose: `frame_base * translate(-0.5, -0.5, -0.5)`
/// (the model is authored in a 0..1 block).
pub fn frame_model_pose(facing: u8) -> Option<Mat4> {
    Some(frame_base(facing)? * Mat4::from_translation(Vec3::splat(-0.5)))
}

/// `translate(0, 0, invisible ? 0.5 : 0.4375)` — the item and map sit just
/// proud of the frame's back, or flush with the wall when the frame is
/// invisible.
fn content_depth(invisible: bool) -> Mat4 {
    Mat4::from_translation(Vec3::new(0.0, 0.0, if invisible { 0.5 } else { 0.4375 }))
}

/// The framed item's pose: `rotZ(rotation * 360/8) scale(0.5)`, then the
/// item's FIXED display transform (the caller's).
pub fn item_pose(facing: u8, rotation: u8, invisible: bool) -> Option<Mat4> {
    Some(
        frame_base(facing)?
            * content_depth(invisible)
            * Mat4::from_rotation_z((f32::from(rotation) * 45.0).to_radians())
            * Mat4::from_scale(Vec3::splat(0.5)),
    )
}

/// A framed map's pose, in the map's 0..128 units:
/// `rotZ((rotation % 4 * 2) * 360/8) rotZ(180) scale(1/128)
/// translate(-64, -64, 0) translate(0, 0, -1)`. The map turns in QUARTERS
/// where an item turns in eighths.
pub fn map_pose(facing: u8, rotation: u8, invisible: bool) -> Option<Mat4> {
    let quarter = f32::from(rotation % 4 * 2) * 45.0;
    Some(
        frame_base(facing)?
            * content_depth(invisible)
            * Mat4::from_rotation_z(quarter.to_radians())
            * Mat4::from_rotation_z(180f32.to_radians())
            * Mat4::from_scale(Vec3::splat(1.0 / 128.0))
            * Mat4::from_translation(Vec3::new(-64.0, -64.0, 0.0))
            * Mat4::from_translation(Vec3::new(0.0, 0.0, -1.0)),
    )
}

/// A pose as the row-major 3x4 affine
/// [`crate::entities::BlockEntityDraw::transform`] takes.
pub fn affine(m: Mat4) -> [[f32; 4]; 3] {
    let r = |i: usize| [m.x_axis[i], m.y_axis[i], m.z_axis[i], m.w_axis[i]];
    [r(0), r(1), r(2)]
}

/// The framed item's transform for
/// [`crate::entities::BlockEntityDraw::transform`]: [`item_pose`] as an
/// [`affine`]. The item's FIXED display transform comes first, applied by the
/// emitter from the model it draws (`BlockEntityDraw::fixed_display`).
pub fn item_transform(facing: u8, rotation: u8, invisible: bool) -> Option<[[f32; 4]; 3]> {
    Some(affine(item_pose(facing, rotation, invisible)?))
}

/// The frame model's transform, as [`affine`] of [`frame_model_pose`].
pub fn frame_transform(facing: u8) -> Option<[[f32; 4]; 3]> {
    Some(affine(frame_model_pose(facing)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `affine` is row-major: row r is `[m[0][r], m[1][r], m[2][r], m[3][r]]`
    /// in column terms, so applying it by hand equals `transform_point3`.
    #[test]
    fn affine_rows_apply_like_the_matrix() {
        let m = frame_model_pose(5).unwrap() * Mat4::from_scale(Vec3::new(1.0, 2.0, 3.0));
        let a = affine(m);
        let p = Vec3::new(0.25, -0.5, 0.75);
        let by_hand = Vec3::new(
            a[0][0] * p.x + a[0][1] * p.y + a[0][2] * p.z + a[0][3],
            a[1][0] * p.x + a[1][1] * p.y + a[1][2] * p.z + a[1][3],
            a[2][0] * p.x + a[2][1] * p.y + a[2][2] * p.z + a[2][3],
        );
        assert!(close(by_hand, m.transform_point3(p)));
    }

    fn close(a: Vec3, b: Vec3) -> bool {
        (a - b).abs().max_element() < 1e-5
    }

    /// For every facing the item's centre is `step * (0.46875 - 0.4375)` =
    /// `step / 32` from the entity: the base pushes it toward the wall and
    /// local +z (which points INTO the wall, -step) brings it back out. A
    /// map's `translate(0, 0, -1)` in map units is therefore 1/128 further
    /// OUT along the facing, in front of where an item would sit.
    #[test]
    fn contents_sit_just_off_the_wall_for_every_facing() {
        for facing in 0..6u8 {
            let step = facing_step(facing).unwrap();
            let item = item_pose(facing, 0, false)
                .unwrap()
                .transform_point3(Vec3::ZERO);
            assert!(close(item, step / 32.0), "facing {facing}: {item:?}");
            let map = map_pose(facing, 0, false)
                .unwrap()
                .transform_point3(Vec3::new(64.0, 64.0, 0.0));
            let want = step * (1.0 / 32.0 + 1.0 / 128.0);
            assert!(close(map, want), "facing {facing}: map {map:?} vs {want:?}");
            // Invisible frames put the contents flush with the wall.
            let flush = item_pose(facing, 0, true)
                .unwrap()
                .transform_point3(Vec3::ZERO);
            assert!(close(flush, step * (0.46875 - 0.5)), "facing {facing}");
        }
        assert!(facing_step(6).is_none() && item_pose(9, 0, false).is_none());
    }

    /// The frame model's block centre (0.5, 0.5, 0.5) lands on the base
    /// origin, `step * 0.46875`.
    #[test]
    fn the_frame_models_centre_is_the_base_origin() {
        for facing in 0..6u8 {
            let c = frame_model_pose(facing)
                .unwrap()
                .transform_point3(Vec3::splat(0.5));
            assert!(close(c, facing_step(facing).unwrap() * 0.46875), "{facing}");
        }
    }

    /// An item turns in eighths, a map in quarters: rotation 1 turns the item
    /// 45 degrees and leaves the map where rotation 0 had it.
    #[test]
    fn a_map_turns_in_quarters_and_an_item_in_eighths() {
        let corner = Vec3::new(0.0, 0.0, 0.0);
        let m0 = map_pose(2, 0, false).unwrap().transform_point3(corner);
        let m1 = map_pose(2, 1, false).unwrap().transform_point3(corner);
        let m2 = map_pose(2, 2, false).unwrap().transform_point3(corner);
        assert!(!close(m0, m1), "rotation 1 is a quarter turn for a map");
        assert!(
            close(map_pose(2, 4, false).unwrap().transform_point3(corner), m0),
            "4 % 4"
        );
        assert!(!close(m1, m2));
        let x = Vec3::new(1.0, 0.0, 0.0);
        let i0 = item_pose(2, 0, false).unwrap().transform_vector3(x);
        let i1 = item_pose(2, 1, false).unwrap().transform_vector3(x);
        assert!((i0.angle_between(i1).to_degrees() - 45.0).abs() < 1e-3);
    }
}
