//! Right-handed view matrices, built exactly as glam 0.29's `Mat4::look_to_rh`
//! / `Mat4::look_at_rh` built them.
//!
//! glam 0.31 moved the view constructors off `Mat4` into
//! `glam::camera::rh::view`, and since 0.30 `look_to` no longer normalises
//! `dir` itself (it only asserts, under `glam-assert`). The callers here were
//! written against the normalising version and some pass an unnormalised
//! direction, so `look_to_rh` restores the normalisation; `look_at_mat4`
//! still normalises `center - eye`, which is what 0.29 did.

use glam::{Mat4, Vec3};

/// Right-handed view matrix from an eye position, a facing direction (any
/// length) and a normalised up vector.
pub fn look_to_rh(eye: Vec3, dir: Vec3, up: Vec3) -> Mat4 {
    glam::camera::rh::view::look_to_mat4(eye, dir.normalize(), up)
}

/// Right-handed view matrix from an eye position, a focal point and a
/// normalised up vector.
pub fn look_at_rh(eye: Vec3, center: Vec3, up: Vec3) -> Mat4 {
    glam::camera::rh::view::look_at_mat4(eye, center, up)
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec4;

    /// glam 0.29.3's `Mat4::look_to_rh`, transcribed.
    fn glam_0_29_look_to_rh(eye: Vec3, dir: Vec3, up: Vec3) -> Mat4 {
        let f = dir.normalize();
        let s = f.cross(up).normalize();
        let u = s.cross(f);
        Mat4::from_cols(
            Vec4::new(s.x, u.x, -f.x, 0.0),
            Vec4::new(s.y, u.y, -f.y, 0.0),
            Vec4::new(s.z, u.z, -f.z, 0.0),
            Vec4::new(-eye.dot(s), -eye.dot(u), eye.dot(f), 1.0),
        )
    }

    #[test]
    fn matches_glam_0_29_bit_for_bit() {
        let cases = [
            (
                Vec3::new(1.5, 70.25, -3.0),
                Vec3::new(0.3, -0.2, 0.9),
                Vec3::Y,
            ),
            (Vec3::new(0.0, 0.0, -4.0), Vec3::Z, Vec3::Y),
            (
                Vec3::new(8.0, 90.0, 8.0),
                Vec3::new(0.0, -1.0, 0.0),
                Vec3::Z,
            ),
            (Vec3::ZERO, Vec3::new(-2.5, 0.75, 4.0), Vec3::Y),
        ];
        for (eye, dir, up) in cases {
            let old = glam_0_29_look_to_rh(eye, dir, up).to_cols_array();
            let new = look_to_rh(eye, dir, up).to_cols_array();
            assert_eq!(
                old.map(f32::to_bits),
                new.map(f32::to_bits),
                "look_to {dir}"
            );
            let center = eye + dir * 3.0;
            let old_at = glam_0_29_look_to_rh(eye, center - eye, up).to_cols_array();
            let new_at = look_at_rh(eye, center, up).to_cols_array();
            assert_eq!(
                old_at.map(f32::to_bits),
                new_at.map(f32::to_bits),
                "look_at {dir}"
            );
        }
    }
}
