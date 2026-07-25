//! `ItemModelGenerator` — the extruded-sprite geometry for `builtin/generated`
//! item models (M22).
//!
//! A generated item is not a model in the file; the client *builds* one from
//! the sprite's alpha. Two full-size quads face front and back across a 1/16
//! slab, and every texel edge where an opaque texel meets a transparent one (or
//! the sprite border) gets a thin side quad, so the item reads as a cut-out
//! slab rather than a billboard.
//!
//! ```text
//! MIN_Z 7.5, MAX_Z 8.5          — the slab, in 1/16 model units
//! SOUTH_FACE_UVS (0,0,16,16)    — front
//! NORTH_FACE_UVS (16,0,0,16)    — back, u mirrored
//! UV_SHRINK 0.1                 — side quads inset, to avoid bleeding
//! ```
//!
//! Two details are easy to get backwards and are transcribed deliberately:
//! `SideDirection::Left` maps to `Direction.EAST` and `Right` to `WEST` (the
//! names describe the *sprite* edge, not the world axis), and `isTransparent`
//! returns **true** out of bounds — so the sprite's border always extrudes.

/// The four sprite-edge directions `getSideFaces` scans for.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum SideDirection {
    Up,
    Down,
    Left,
    Right,
}

impl SideDirection {
    /// The `(stepX, stepY)` of the vanilla `Direction` this maps to, in sprite
    /// space (y grows downward). `checkTransition` samples
    /// `(x - stepX, y - stepY)`.
    const fn step(self) -> (i32, i32) {
        match self {
            // Direction.UP = (0, 1, 0) → stepY 1 → samples the texel above.
            SideDirection::Up => (0, 1),
            // Direction.DOWN = (0, -1, 0).
            SideDirection::Down => (0, -1),
            // LEFT is Direction.EAST = (1, 0, 0) — not west. The name is the
            // sprite edge; the world direction is its opposite.
            SideDirection::Left => (1, 0),
            // RIGHT is Direction.WEST = (-1, 0, 0).
            SideDirection::Right => (-1, 0),
        }
    }

    /// `isHorizontal()` — UP and DOWN, which flip the v range.
    const fn is_horizontal(self) -> bool {
        matches!(self, SideDirection::Up | SideDirection::Down)
    }

    /// The vanilla `Direction` ordinal used for face shading:
    /// down 0, up 1, north 2, south 3, west 4, east 5.
    pub const fn face_index(self) -> u8 {
        match self {
            SideDirection::Up => 1,
            SideDirection::Down => 0,
            SideDirection::Left => 5,  // EAST
            SideDirection::Right => 4, // WEST
        }
    }
}

/// One generated quad, in model units (0..16) with sprite-relative UVs (0..16).
///
/// Kept independent of the block bake's `Quad` because a generated item has no
/// cull face, no tint source and one texture — carrying those would invite a
/// caller to treat it as a block face.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ItemQuad {
    /// Corner positions, model units.
    pub verts: [[f32; 3]; 4],
    /// Sprite-relative UVs in 0..16, matching `verts`.
    pub uv: [[f32; 2]; 4],
    /// Vanilla `Direction` ordinal, for directional shading.
    pub dir: u8,
    /// Which `layerN` of the item model this quad came from.
    pub layer: u8,
}

const MIN_Z: f32 = 7.5;
const MAX_Z: f32 = 8.5;
const UV_SHRINK: f32 = 0.1;

/// A sprite's opacity mask: `true` where the texel is **transparent**.
///
/// Vanilla asks `SpriteContents.isTransparent`, which is alpha == 0 for the
/// standard `AlphaDiscard` sprite; the caller builds this from the decoded PNG
/// so this module never touches image decoding.
pub struct SpriteMask {
    pub width: u32,
    pub height: u32,
    /// Row-major, `width * height`.
    pub transparent: Vec<bool>,
}

impl SpriteMask {
    /// `isTransparent(sprite, frame, x, y, w, h)` — **out of bounds is
    /// transparent**, which is what makes the sprite border extrude.
    fn is_transparent(&self, x: i32, y: i32) -> bool {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return true;
        }
        self.transparent[(y as u32 * self.width + x as u32) as usize]
    }

    /// Every texel is transparent — a fully blank sprite generates only the
    /// two full-size faces, exactly as vanilla does.
    pub fn is_blank(&self) -> bool {
        self.transparent.iter().all(|t| *t)
    }
}

/// `bakeExtrudedSprite` for one layer: the front and back faces plus every
/// side face the sprite's alpha calls for.
pub fn extrude(mask: &SpriteMask, layer: u8) -> Vec<ItemQuad> {
    let mut out = Vec::new();
    // The two full-size faces. `from (0,0,7.5)`, `to (16,16,8.5)`;
    // SOUTH gets UVs (0,0)-(16,16) and NORTH the u-mirrored (16,0)-(0,16).
    out.push(face_quad([0.0, 0.0], [16.0, 16.0], MAX_Z, [0.0, 0.0, 16.0, 16.0], 3, layer));
    out.push(face_quad([0.0, 0.0], [16.0, 16.0], MIN_Z, [16.0, 0.0, 0.0, 16.0], 2, layer));
    out.extend(side_faces(mask, layer));
    out
}

/// A full-size front/back quad at `z`.
fn face_quad(
    min: [f32; 2],
    max: [f32; 2],
    z: f32,
    uvs: [f32; 4],
    dir: u8,
    layer: u8,
) -> ItemQuad {
    let [u0, v0, u1, v1] = uvs;
    // Wound so the quad faces its `dir`; the renderer does not cull, but the
    // winding keeps the two faces distinguishable for the oracle.
    let front = dir == 3;
    let (a, b) = if front { (min[0], max[0]) } else { (max[0], min[0]) };
    let (ua, ub) = if front { (u0, u1) } else { (u0, u1) };
    ItemQuad {
        verts: [
            [a, min[1], z],
            [b, min[1], z],
            [b, max[1], z],
            [a, max[1], z],
        ],
        uv: [[ua, v1], [ub, v1], [ub, v0], [ua, v0]],
        dir,
        layer,
    }
}

/// `getSideFaces` + `bakeSideFaces`.
fn side_faces(mask: &SpriteMask, layer: u8) -> Vec<ItemQuad> {
    let (w, h) = (mask.width, mask.height);
    let x_scale = 16.0 / w as f32;
    let y_scale = 16.0 / h as f32;
    // Deduplicated like vanilla's HashSet; a Vec + contains would be O(n²) on
    // a 64² sprite, so the key is packed into a sortable tuple.
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();

    for y in 0..h as i32 {
        for x in 0..w as i32 {
            if mask.is_transparent(x, y) {
                continue; // vanilla only scans from an OPAQUE texel
            }
            for facing in [
                SideDirection::Up,
                SideDirection::Down,
                SideDirection::Left,
                SideDirection::Right,
            ] {
                let (sx, sy) = facing.step();
                if !mask.is_transparent(x - sx, y - sy) {
                    continue;
                }
                if !seen.insert((facing, x, y)) {
                    continue;
                }
                out.push(side_quad(facing, x as f32, y as f32, x_scale, y_scale, layer));
            }
        }
    }
    out
}

/// One side quad — the body of `bakeSideFaces`'s loop.
fn side_quad(
    facing: SideDirection,
    x: f32,
    y: f32,
    x_scale: f32,
    y_scale: f32,
    layer: u8,
) -> ItemQuad {
    let u0 = x + UV_SHRINK;
    let u1 = x + 1.0 - UV_SHRINK;
    // UP/DOWN keep v ascending; LEFT/RIGHT invert it.
    let (v0, v1) = if facing.is_horizontal() {
        (y + UV_SHRINK, y + 1.0 - UV_SHRINK)
    } else {
        (y + 1.0 - UV_SHRINK, y + UV_SHRINK)
    };

    let (mut sx, mut sy, mut ex, mut ey) = (x, y, x, y);
    match facing {
        SideDirection::Up => ex += 1.0,
        SideDirection::Down => {
            ex += 1.0;
            sy += 1.0;
            ey += 1.0;
        }
        SideDirection::Left => ey += 1.0,
        SideDirection::Right => {
            sx += 1.0;
            ex += 1.0;
            ey += 1.0;
        }
    }
    sx *= x_scale;
    ex *= x_scale;
    sy *= y_scale;
    ey *= y_scale;
    // Sprite space has y downward; model space has it upward.
    sy = 16.0 - sy;
    ey = 16.0 - ey;

    let (from, to) = match facing {
        SideDirection::Up => ([sx, sy, MIN_Z], [ex, sy, MAX_Z]),
        SideDirection::Down => ([sx, ey, MIN_Z], [ex, ey, MAX_Z]),
        SideDirection::Left => ([sx, sy, MIN_Z], [sx, ey, MAX_Z]),
        SideDirection::Right => ([ex, sy, MIN_Z], [ex, ey, MAX_Z]),
    };
    // The quad spans `from`..`to`; one of x/y is degenerate, z always spans the
    // slab, so the four corners are the two spanning axes' combinations.
    let verts = if facing.is_horizontal() {
        // Spans x and z at a fixed y.
        [
            [from[0], from[1], from[2]],
            [to[0], from[1], from[2]],
            [to[0], to[1], to[2]],
            [from[0], to[1], to[2]],
        ]
    } else {
        // Spans y and z at a fixed x.
        [
            [from[0], from[1], from[2]],
            [from[0], to[1], from[2]],
            [to[0], to[1], to[2]],
            [to[0], from[1], to[2]],
        ]
    };
    ItemQuad {
        verts,
        uv: [
            [u0 * x_scale, v0 * y_scale],
            [u1 * x_scale, v0 * y_scale],
            [u1 * x_scale, v1 * y_scale],
            [u0 * x_scale, v1 * y_scale],
        ],
        dir: facing.face_index(),
        layer,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A mask from a row-major picture, `#` opaque and `.` transparent.
    fn mask(rows: &[&str]) -> SpriteMask {
        let height = rows.len() as u32;
        let width = rows[0].len() as u32;
        let mut transparent = Vec::with_capacity((width * height) as usize);
        for r in rows {
            assert_eq!(r.len() as u32, width, "ragged mask");
            for ch in r.chars() {
                transparent.push(ch == '.');
            }
        }
        SpriteMask {
            width,
            height,
            transparent,
        }
    }

    #[test]
    fn a_blank_sprite_generates_only_the_two_faces() {
        let m = mask(&["....", "....", "....", "...."]);
        assert!(m.is_blank());
        let q = extrude(&m, 0);
        assert_eq!(q.len(), 2, "front + back only");
        // Front at MAX_Z facing south (3), back at MIN_Z facing north (2).
        assert_eq!(q[0].dir, 3);
        assert_eq!(q[0].verts[0][2], MAX_Z);
        assert_eq!(q[1].dir, 2);
        assert_eq!(q[1].verts[0][2], MIN_Z);
    }

    #[test]
    fn a_single_opaque_texel_extrudes_on_all_four_sides() {
        // One opaque texel surrounded by transparency: each of its four edges
        // borders a transparent neighbour, so all four sides extrude.
        let m = mask(&["....", ".#..", "....", "...."]);
        let q = extrude(&m, 0);
        assert_eq!(q.len(), 2 + 4, "two faces plus four sides, got {}", q.len());
        let dirs: std::collections::HashSet<u8> = q[2..].iter().map(|x| x.dir).collect();
        // up(1), down(0), east(5), west(4).
        assert_eq!(dirs, [0u8, 1, 4, 5].into_iter().collect());
    }

    #[test]
    fn the_sprite_border_always_extrudes() {
        // A fully opaque sprite has no internal transitions, but every texel on
        // the border sees out-of-bounds as transparent.
        let m = mask(&["##", "##"]);
        let q = extrude(&m, 0);
        // 2 faces + 2 per side × 4 sides = 10.
        assert_eq!(q.len(), 10, "got {}", q.len());
    }

    #[test]
    fn an_interior_texel_of_a_solid_block_extrudes_nothing() {
        let m = mask(&["###", "###", "###"]);
        let q = extrude(&m, 0);
        // Only the 3-wide border edges: 3 per side × 4 = 12, plus 2 faces.
        assert_eq!(q.len(), 14, "got {}", q.len());
    }

    #[test]
    fn side_quads_span_the_slab_and_are_inset() {
        let m = mask(&["#"]);
        let q = extrude(&m, 0);
        for s in &q[2..] {
            let zs: Vec<f32> = s.verts.iter().map(|v| v[2]).collect();
            assert!(zs.contains(&MIN_Z) && zs.contains(&MAX_Z), "spans the slab");
            // UV_SHRINK insets both u ends by 0.1 of a texel (×16 scale = 1.6).
            let us: Vec<f32> = s.uv.iter().map(|u| u[0]).collect();
            let lo = us.iter().cloned().fold(f32::INFINITY, f32::min);
            let hi = us.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
            assert!((lo - 1.6).abs() < 1e-4, "u lo {lo}");
            assert!((hi - 14.4).abs() < 1e-4, "u hi {hi}");
        }
    }

    #[test]
    fn left_is_east_and_right_is_west() {
        // The names describe the sprite edge; the world direction is opposite.
        assert_eq!(SideDirection::Left.face_index(), 5, "EAST");
        assert_eq!(SideDirection::Right.face_index(), 4, "WEST");
        assert_eq!(SideDirection::Up.face_index(), 1);
        assert_eq!(SideDirection::Down.face_index(), 0);
    }

    #[test]
    fn a_non_16_sprite_scales_into_model_units() {
        // A 32² sprite still spans 0..16 model units, so each texel is 0.5.
        let rows: Vec<String> = (0..32).map(|_| ".".repeat(32)).collect();
        let mut refs: Vec<&str> = rows.iter().map(|s| s.as_str()).collect();
        refs[16] = "................#...............";
        let m = mask(&refs);
        let q = extrude(&m, 0);
        let sides = &q[2..];
        assert_eq!(sides.len(), 4);
        for s in sides {
            for v in s.verts {
                assert!((0.0..=16.0).contains(&v[0]), "x {} in model units", v[0]);
                assert!((0.0..=16.0).contains(&v[1]), "y {} in model units", v[1]);
            }
        }
    }
}
