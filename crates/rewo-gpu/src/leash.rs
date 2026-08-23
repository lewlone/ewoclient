//! The leash rope (M170) — `LeashFeatureRenderer`, ported.
//!
//! The decode has been complete and gated since M77 (`set_entity_link` ->
//! `is_leashable` -> `set_leash_holder`, graded by `rideshot`); nothing drew the
//! rope. This is the geometry, pure and testable; the CPU inputs (which
//! entities, where their endpoints are, the light at each end) live beside the
//! session in `live_cmd`, and the Vulkan pass in `world.rs`.
//!
//! ```java
//! // LeashFeatureRenderer.prepare / addVertexPair
//! float dx = (float)(end.x - start.x), dy = ..., dz = ...;
//! float offsetFactor = Mth.invSqrt(dx*dx + dz*dz) * 0.05F / 2.0F;
//! float dxOff = dz * offsetFactor, dzOff = dx * offsetFactor;
//! for (int k = 0; k <= 24; k++) addVertexPair(.., 0.05F, .., k, false, ..);
//! for (int k = 24; k >= 0; k--) addVertexPair(.., 0.0F,  .., k, true,  ..);
//! ```
//!
//! # Things a tidy rewrite gets wrong
//!
//! * **Two passes, not one.** The forward pass (`fudge = 0.05`) and the
//!   backward pass (`fudge = 0.0`) build a two-sided ribbon: the same catenary
//!   traced down one edge and back up the other, so it is visible from either
//!   side. The vanilla primitive is a `TRIANGLE_STRIP`; Rewo expands it to a
//!   list (leashes are rare, and a list needs no per-leash draw or primitive
//!   restart).
//! * **The alternating dim is keyed to `backwards`.** `k % 2 == (backwards ? 1
//!   : 0)` dims the *even* segments on the way out and the *odd* ones on the way
//!   back, so the two edges' twist lines up rather than cancelling.
//! * **The slack curve is asymmetric in `dy`.** A rope to something *above*
//!   (`dy > 0`) sags as `dy·p^2` (slow start, so it leaves the mob near-flat);
//!   to something *below* it is `dy - dy·(1-p)^2` (the mirror). A single `p^2`
//!   for both makes an upward rope bow the wrong way.
//! * **`offset` is already in `start`.** Vanilla translates the pose by
//!   `leashState.offset` and then draws vertices relative to it; but
//!   `start = entity.pos + offset`, so in absolute world space the vertices are
//!   `start + relative`. Adding `offset` again doubles the attach displacement.
//! * **The light is interpolated per vertex, not per rope.** `Mth.lerp(progress,
//!   startLight, endLight)` — a rope from a lit barn into a dark field fades
//!   along its length. Rewo interpolates the already-resolved lightmap RGB
//!   (what `entity_light` returns) rather than the packed coords, the same
//!   approximation the entity pass makes.

/// `LEASH_RENDER_STEPS`.
pub const LEASH_STEPS: i32 = 24;
/// `LEASH_WIDTH` — the ribbon's base offset (`* 0.05 / 2` per side).
pub const LEASH_WIDTH: f32 = 0.05;

/// The rope's base tint before the per-segment dim, sRGB — vanilla
/// `r = 0.5, g = 0.4, b = 0.3`, authored in gamma space like every `setColor`.
pub const LEASH_BASE_SRGB: [f32; 3] = [0.5, 0.4, 0.3];

/// One ribbon vertex: absolute world position and a linear rgb colour with the
/// base tint, the segment dim and the interpolated lightmap already folded in.
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
#[repr(C)]
pub struct LeashVertex {
    pub pos: [f32; 3],
    pub color: [f32; 3],
}

/// `Mth.invSqrt` — the fast inverse square root with one Newton step. Ported
/// verbatim rather than replaced by `1.0 / x.sqrt()`, because it is what scales
/// the ribbon's half-width and a bit-faithful geometry is cheap here.
pub fn inv_sqrt(x: f64) -> f64 {
    let half = 0.5 * x;
    let mut l = x.to_bits() as i64;
    l = 6_910_469_410_427_058_090_i64 - (l >> 1);
    let mut y = f64::from_bits(l as u64);
    y *= 1.5 - half * y * y;
    y
}

/// Build the ribbon between two absolute world points as a triangle list.
///
/// `start` is the leashed entity's attach point (`entity.pos + rotated leash
/// offset`); `end` is the holder's `getRopeHoldPosition`. `slack` droops the
/// rope; `start_light` / `end_light` are the resolved lightmap RGB (`0..1`) at
/// each end. Colours come out LINEAR for the world attachment.
pub fn build_ribbon(
    start: [f64; 3],
    end: [f64; 3],
    slack: bool,
    start_light: [f32; 3],
    end_light: [f32; 3],
) -> Vec<LeashVertex> {
    let dx = (end[0] - start[0]) as f32;
    let dy = (end[1] - start[1]) as f32;
    let dz = (end[2] - start[2]) as f32;
    let offset_factor = (inv_sqrt((dx * dx + dz * dz) as f64) * 0.05 / 2.0) as f32;
    let dx_off = dz * offset_factor;
    let dz_off = dx * offset_factor;

    let mut strip: Vec<LeashVertex> = Vec::with_capacity(((LEASH_STEPS + 1) * 4) as usize);
    for k in 0..=LEASH_STEPS {
        add_vertex_pair(
            &mut strip, start, dx, dy, dz, 0.05, dx_off, dz_off, k, false, slack, start_light,
            end_light,
        );
    }
    for k in (0..=LEASH_STEPS).rev() {
        add_vertex_pair(
            &mut strip, start, dx, dy, dz, 0.0, dx_off, dz_off, k, true, slack, start_light,
            end_light,
        );
    }
    strip_to_list(&strip)
}

#[allow(clippy::too_many_arguments)]
fn add_vertex_pair(
    out: &mut Vec<LeashVertex>,
    start: [f64; 3],
    dx: f32,
    dy: f32,
    dz: f32,
    fudge: f32,
    dx_off: f32,
    dz_off: f32,
    k: i32,
    backwards: bool,
    slack: bool,
    start_light: [f32; 3],
    end_light: [f32; 3],
) {
    let progress = k as f32 / LEASH_STEPS as f32;
    let color_modifier = if k % 2 == i32::from(backwards) { 0.7 } else { 1.0 };
    let mut color = [0.0f32; 3];
    for c in 0..3 {
        let light = start_light[c] + (end_light[c] - start_light[c]) * progress;
        color[c] = crate::entities::srgb_to_linear(LEASH_BASE_SRGB[c] * color_modifier) * light;
    }
    let x = dx * progress;
    let y = if slack {
        if dy > 0.0 {
            dy * progress * progress
        } else {
            dy - dy * (1.0 - progress) * (1.0 - progress)
        }
    } else {
        dy * progress
    };
    let z = dz * progress;
    let sx = start[0] as f32;
    let sy = start[1] as f32;
    let sz = start[2] as f32;
    out.push(LeashVertex {
        pos: [sx + x - dx_off, sy + y + fudge, sz + z + dz_off],
        color,
    });
    out.push(LeashVertex {
        pos: [sx + x + dx_off, sy + y + 0.05 - fudge, sz + z - dz_off],
        color,
    });
}

/// A `TRIANGLE_STRIP` `[v0, v1, v2, …]` expanded to an explicit list
/// `(v0,v1,v2), (v1,v2,v3), …`. Winding alternates, which is why the pass runs
/// with culling off, exactly as the strip would.
fn strip_to_list(strip: &[LeashVertex]) -> Vec<LeashVertex> {
    if strip.len() < 3 {
        return Vec::new();
    }
    let mut out = Vec::with_capacity((strip.len() - 2) * 3);
    for i in 0..strip.len() - 2 {
        out.push(strip[i]);
        out.push(strip[i + 1]);
        out.push(strip[i + 2]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIT: [f32; 3] = [1.0, 1.0, 1.0];

    #[test]
    fn inv_sqrt_is_close_to_the_real_thing() {
        for x in [0.25f64, 1.0, 4.0, 9.0, 100.0, 0.05] {
            let got = inv_sqrt(x);
            let want = 1.0 / x.sqrt();
            assert!((got - want).abs() / want < 2e-3, "invSqrt({x}) = {got}, want {want}");
        }
    }

    #[test]
    fn the_strip_is_two_sided_and_expands_to_a_list() {
        // 2 passes × (24+1) steps × 2 verts = 100 strip verts -> 98 triangles.
        let v = build_ribbon([0.0, 1.0, 0.0], [3.0, 1.0, 0.0], false, LIT, LIT);
        assert_eq!(v.len(), 98 * 3, "98 triangles");
    }

    #[test]
    fn a_taut_rope_is_a_straight_line_in_y() {
        // Level, non-slack: the two edges sit at y and y + 0.05, so every
        // vertex is in [y - eps, y + 0.05 + eps]. The lower bound is TIGHT on
        // purpose: dropping the `0.05 -` on the second edge sinks half the
        // vertices to y - 0.05, which a loose `abs() <= 0.05` tolerance hides.
        let v = build_ribbon([0.0, 5.0, 0.0], [4.0, 5.0, 0.0], false, LIT, LIT);
        for vert in &v {
            assert!(
                vert.pos[1] >= 5.0 - 1e-4 && vert.pos[1] <= 5.0 + 0.05 + 1e-4,
                "y {} strayed outside [5.0, 5.05]",
                vert.pos[1]
            );
        }
    }

    #[test]
    fn the_ribbon_has_perpendicular_width() {
        // A rope along +X carries its two edges apart in Z (`dz_off = dx *
        // offsetFactor`), so the ribbon has real thickness — this is what a
        // zero `offset_factor` collapses, and the gate cannot see it because
        // for this rope the width is along the camera's own view axis.
        let v = build_ribbon([-2.0, 0.0, 0.0], [2.0, 0.0, 0.0], false, LIT, LIT);
        let zmin = v.iter().map(|vt| vt.pos[2]).fold(f32::MAX, f32::min);
        let zmax = v.iter().map(|vt| vt.pos[2]).fold(f32::MIN, f32::max);
        assert!(zmax - zmin > 0.01, "ribbon has no perpendicular width: z span {}", zmax - zmin);
    }

    #[test]
    fn slack_sags_below_the_endpoints() {
        // A rope to a point BELOW: the curve is dy - dy(1-p)^2 and must dip past
        // the straight interpolation between the endpoints.
        let start = [0.0f64, 10.0, 0.0];
        let end = [4.0f64, 6.0, 0.0]; // dy = -4
        let slack = build_ribbon(start, end, true, LIT, LIT);
        let taut = build_ribbon(start, end, false, LIT, LIT);
        // Both share their x positions (x = dx·progress), so pick the vertex
        // nearest the midpoint x = 2.0 in each and compare y there — the global
        // minima are equal, because both curves meet at the lower endpoint.
        let near_mid = |v: &[LeashVertex]| {
            v.iter()
                .min_by(|a, b| {
                    (a.pos[0] - 2.0)
                        .abs()
                        .partial_cmp(&(b.pos[0] - 2.0).abs())
                        .unwrap()
                })
                .unwrap()
                .pos[1]
        };
        let mid_taut = near_mid(&taut);
        let mid_slack = near_mid(&slack);
        assert!(mid_slack < mid_taut, "slack {mid_slack} not below taut {mid_taut} at midpoint");
    }

    #[test]
    fn an_upward_rope_sags_the_other_way() {
        // dy > 0 uses dy·p^2, so near the mob (p small) the rope stays low and
        // rises late — the min y is close to the start, not the end.
        let v = build_ribbon([0.0, 2.0, 0.0], [0.0, 8.0, 4.0], true, LIT, LIT);
        let min_y = v.iter().map(|vt| vt.pos[1]).fold(f32::MAX, f32::min);
        assert!((min_y - 2.0).abs() < 0.1, "upward rope should hug the start y, got {min_y}");
    }

    #[test]
    fn the_base_is_brown_and_alternating_segments_dim() {
        // At full light the linear colour is srgb_to_linear(base·mod). r > g > b
        // (brown), and some verts carry the 0.7 dim.
        let v = build_ribbon([0.0, 1.0, 0.0], [3.0, 1.0, 0.0], false, LIT, LIT);
        let bright = crate::entities::srgb_to_linear(0.5);
        let dim = crate::entities::srgb_to_linear(0.5 * 0.7);
        let reds: Vec<f32> = v.iter().map(|vt| vt.color[0]).collect();
        assert!(reds.iter().any(|r| (r - bright).abs() < 1e-4), "a full-strength segment");
        assert!(reds.iter().any(|r| (r - dim).abs() < 1e-4), "a dimmed segment");
        for vt in &v {
            assert!(vt.color[0] >= vt.color[1] && vt.color[1] >= vt.color[2], "brown r>=g>=b");
        }
    }

    #[test]
    fn light_fades_along_the_rope() {
        // Lit at the start, dark at the end: the near and far reds differ.
        let v = build_ribbon(
            [0.0, 1.0, 0.0],
            [6.0, 1.0, 0.0],
            false,
            [1.0, 1.0, 1.0],
            [0.0, 0.0, 0.0],
        );
        let near = v.first().unwrap().color[0];
        let far = v
            .iter()
            .max_by(|a, b| a.pos[0].partial_cmp(&b.pos[0]).unwrap())
            .unwrap()
            .color[0];
        assert!(near > far + 0.1, "near {near} should be brighter than far {far}");
    }
}
