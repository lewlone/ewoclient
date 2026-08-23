//! A small software 3D renderer for the Minecraft player model — the
//! overlay's HOME tab shows the signed-in account's skin, drag-rotatable.
//!
//! The model is 12 axis-aligned cuboids (head / body / 2 arms / 2 legs,
//! each with a second "overlay" layer) plus a cape. Every face is a flat
//! textured quad: rotate its corners, cull the back-faces, painter-sort
//! what's left, and draw each via `Canvas::draw_vertices` against the skin
//! as an image shader. A player model never self-intersects, so per-face
//! depth sort is exact — no z-buffer needed.
//!
//! Both proportions — wide ("Steve") and slim 3px-arm ("Alex"), selected by
//! the caller via the `ewo-skin-slim` marker file the mod writes.

use skia_safe::{
    vertices::VertexMode, BlendMode, Canvas, Color, FilterMode, Image, Paint, Point, Rect,
    SamplingOptions, Shader, TileMode, Vertices,
};

/// One cuboid: geometric min/max corner (model units, inflated for overlay
/// layers), the *base* box size its UV is laid out for, and the skin-UV
/// origin. `cape` faces sample the cape texture, the rest the skin.
struct Cuboid {
    min: [f32; 3],
    max: [f32; 3],
    /// Base (un-inflated) box size — the box-UV unwrap is sized to this.
    size: [f32; 3],
    uv: (f32, f32),
}

/// The player model — six base cuboids then six overlay. Units = skin
/// pixels; the model stands on y=0 with the head-top at y=32. `slim`
/// narrows the arms to 3px (the "Alex" model).
fn body_cuboids(slim: bool) -> [Cuboid; 12] {
    let aw = if slim { 3.0 } else { 4.0 }; // arm width
    let r = -4.0 - aw; // right-arm outer edge
    let l = 4.0 + aw; // left-arm outer edge
    [
        Cuboid { min: [-4.0, 24.0, -4.0], max: [4.0, 32.0, 4.0], size: [8.0, 8.0, 8.0], uv: (0.0, 0.0) },
        Cuboid { min: [-4.0, 12.0, -2.0], max: [4.0, 24.0, 2.0], size: [8.0, 12.0, 4.0], uv: (16.0, 16.0) },
        Cuboid { min: [r, 12.0, -2.0], max: [-4.0, 24.0, 2.0], size: [aw, 12.0, 4.0], uv: (40.0, 16.0) },
        Cuboid { min: [4.0, 12.0, -2.0], max: [l, 24.0, 2.0], size: [aw, 12.0, 4.0], uv: (32.0, 48.0) },
        Cuboid { min: [-4.0, 0.0, -2.0], max: [0.0, 12.0, 2.0], size: [4.0, 12.0, 4.0], uv: (0.0, 16.0) },
        Cuboid { min: [0.0, 0.0, -2.0], max: [4.0, 12.0, 2.0], size: [4.0, 12.0, 4.0], uv: (16.0, 48.0) },
        // overlay layer — hat / jacket / sleeves / pants, slightly inflated.
        Cuboid { min: [-4.5, 23.5, -4.5], max: [4.5, 32.5, 4.5], size: [8.0, 8.0, 8.0], uv: (32.0, 0.0) },
        Cuboid { min: [-4.25, 11.75, -2.25], max: [4.25, 24.25, 2.25], size: [8.0, 12.0, 4.0], uv: (16.0, 32.0) },
        Cuboid { min: [r - 0.25, 11.75, -2.25], max: [-3.75, 24.25, 2.25], size: [aw, 12.0, 4.0], uv: (40.0, 32.0) },
        Cuboid { min: [3.75, 11.75, -2.25], max: [l + 0.25, 24.25, 2.25], size: [aw, 12.0, 4.0], uv: (48.0, 48.0) },
        Cuboid { min: [-4.25, -0.25, -2.25], max: [0.25, 12.25, 2.25], size: [4.0, 12.0, 4.0], uv: (0.0, 32.0) },
        Cuboid { min: [-0.25, -0.25, -2.25], max: [4.25, 12.25, 2.25], size: [4.0, 12.0, 4.0], uv: (0.0, 48.0) },
    ]
}

/// The cape — a thin slab behind the upper body. Box-UV from (0,0) of the
/// 64×32 cape texture; 10w × 16h × 1d.
const CAPE: Cuboid = Cuboid {
    min: [-5.0, 8.0, 2.2],
    max: [5.0, 24.0, 3.2],
    size: [10.0, 16.0, 1.0],
    uv: (0.0, 0.0),
};

/// A face before projection: its 4 corners, matching UVs (texture pixels),
/// outward normal (an axis), a base shade, and which texture it samples.
struct Face {
    pos: [[f32; 3]; 4],
    uv: [Point; 4],
    normal: [f32; 3],
    shade: f32,
    cape: bool,
}

/// Build the 6 faces of a cuboid with standard Minecraft box-UV. `tex_h`
/// scales the V axis so the same routine serves the 64-tall skin and the
/// 32-tall cape texture (UVs are stored in absolute texture pixels).
fn cuboid_faces(c: &Cuboid, cape: bool) -> [Face; 6] {
    let [x0, y0, z0] = c.min;
    let [x1, y1, z1] = c.max;
    let [sw, sh, sd] = c.size;
    let (u, v) = c.uv;
    let p = |a, b, c| [a, b, c];
    let t = |a: f32, b: f32| Point::new(a, b);

    // Box-UV sub-rect origins.
    let (tu, tv) = (u + sd, v); // top
    let (du, dv) = (u + sd + sw, v); // bottom
    let (ru, rv) = (u, v + sd); // right (the -X side)
    let (fu, fv) = (u + sd, v + sd); // front (+Z)
    let (lu, lv) = (u + sd + sw, v + sd); // left (+X side)
    let (bu, bv) = (u + 2.0 * sd + sw, v + sd); // back (-Z)

    [
        // Front (+Z) — chest / face.
        Face {
            pos: [p(x0, y1, z1), p(x1, y1, z1), p(x1, y0, z1), p(x0, y0, z1)],
            uv: [t(fu, fv), t(fu + sw, fv), t(fu + sw, fv + sh), t(fu, fv + sh)],
            normal: [0.0, 0.0, 1.0],
            shade: 0.80,
            cape,
        },
        // Back (-Z).
        Face {
            pos: [p(x1, y1, z0), p(x0, y1, z0), p(x0, y0, z0), p(x1, y0, z0)],
            uv: [t(bu, bv), t(bu + sw, bv), t(bu + sw, bv + sh), t(bu, bv + sh)],
            normal: [0.0, 0.0, -1.0],
            shade: 0.62,
            cape,
        },
        // Top (+Y).
        Face {
            pos: [p(x0, y1, z0), p(x1, y1, z0), p(x1, y1, z1), p(x0, y1, z1)],
            uv: [t(tu, tv), t(tu + sw, tv), t(tu + sw, tv + sd), t(tu, tv + sd)],
            normal: [0.0, 1.0, 0.0],
            shade: 1.0,
            cape,
        },
        // Bottom (-Y).
        Face {
            pos: [p(x0, y0, z1), p(x1, y0, z1), p(x1, y0, z0), p(x0, y0, z0)],
            uv: [t(du, dv), t(du + sw, dv), t(du + sw, dv + sd), t(du, dv + sd)],
            normal: [0.0, -1.0, 0.0],
            shade: 0.50,
            cape,
        },
        // Right (-X) — the player's right side.
        Face {
            pos: [p(x0, y1, z0), p(x0, y1, z1), p(x0, y0, z1), p(x0, y0, z0)],
            uv: [t(ru, rv), t(ru + sd, rv), t(ru + sd, rv + sh), t(ru, rv + sh)],
            normal: [-1.0, 0.0, 0.0],
            shade: 0.68,
            cape,
        },
        // Left (+X).
        Face {
            pos: [p(x1, y1, z1), p(x1, y1, z0), p(x1, y0, z0), p(x1, y0, z1)],
            uv: [t(lu, lv), t(lu + sd, lv), t(lu + sd, lv + sh), t(lu, lv + sh)],
            normal: [1.0, 0.0, 0.0],
            shade: 0.68,
            cape,
        },
    ]
}

/// Rotate a vector by `yaw` (around Y) then `pitch` (around X).
fn rotate(v: [f32; 3], yaw: f32, pitch: f32) -> [f32; 3] {
    let (sy, cy) = yaw.sin_cos();
    let (sp, cp) = pitch.sin_cos();
    let x1 = v[0] * cy + v[2] * sy;
    let z1 = -v[0] * sy + v[2] * cy;
    let y1 = v[1];
    let y2 = y1 * cp - z1 * sp;
    let z2 = y1 * sp + z1 * cp;
    [x1, y2, z2]
}

/// One face ready to draw — screen-space corners, depth, shade, texture.
struct Projected {
    pts: [Point; 4],
    uv: [Point; 4],
    depth: f32,
    shade: f32,
    cape: bool,
}

/// Draw the player model into `rect`, rotated by `yaw` radians. No-ops if
/// the skin image is absent. The cape is drawn only if `cape` is present;
/// `slim` selects the 3px-arm ("Alex") model.
pub fn draw_skin(
    canvas: &Canvas,
    rect: Rect,
    skin: Option<&Image>,
    cape: Option<&Image>,
    yaw: f32,
    slim: bool,
) {
    let Some(skin) = skin else {
        return;
    };

    // A slight downward tilt — a gentle 3/4 view, like a profile-page render.
    let pitch = -0.16_f32;
    // Model centre of mass ≈ (0, 16, 0); fit its 32-unit height into `rect`.
    let centre = [0.0, 16.0, 0.0];
    let scale = (rect.height() * 0.84) / 32.0;
    let cx = rect.left + rect.width() * 0.5;
    let cy = rect.top + rect.height() * 0.5;

    let mut projected: Vec<Projected> = Vec::with_capacity(84);
    let mut consider = |face: &Face| {
        let n = rotate(face.normal, yaw, pitch);
        if n[2] <= 0.0 {
            return; // back-face — points away from the camera at +Z.
        }
        let mut pts = [Point::default(); 4];
        let mut depth = 0.0;
        for i in 0..4 {
            let r = rotate(
                [
                    face.pos[i][0] - centre[0],
                    face.pos[i][1] - centre[1],
                    face.pos[i][2] - centre[2],
                ],
                yaw,
                pitch,
            );
            pts[i] = Point::new(cx + r[0] * scale, cy - r[1] * scale);
            depth += r[2];
        }
        projected.push(Projected {
            pts,
            uv: face.uv,
            depth: depth * 0.25,
            shade: face.shade,
            cape: face.cape,
        });
    };

    for c in &body_cuboids(slim) {
        for face in cuboid_faces(c, false) {
            consider(&face);
        }
    }
    if cape.is_some() {
        for face in cuboid_faces(&CAPE, true) {
            consider(&face);
        }
    }

    // Painter's order — far (small depth) first, near last.
    projected.sort_by(|a, b| a.depth.total_cmp(&b.depth));

    let skin_shader = skin.to_shader(
        (TileMode::Decal, TileMode::Decal),
        SamplingOptions::from(FilterMode::Nearest),
        None,
    );
    let cape_shader: Option<Shader> = cape.and_then(|img| {
        img.to_shader(
            (TileMode::Decal, TileMode::Decal),
            SamplingOptions::from(FilterMode::Nearest),
            None,
        )
    });

    for f in &projected {
        let shader = if f.cape { &cape_shader } else { &skin_shader };
        let Some(shader) = shader else {
            continue;
        };
        let mut paint = Paint::default();
        paint.set_anti_alias(true);
        paint.set_shader(Some(shader.clone()));

        // Per-face shade via vertex colours that modulate the texture.
        let s = (f.shade.clamp(0.0, 1.0) * 255.0) as u8;
        let colors = [Color::from_argb(0xFF, s, s, s); 4];
        let verts = Vertices::new_copy(
            VertexMode::TriangleFan,
            &f.pts[..],
            &f.uv[..],
            &colors[..],
            None,
        );
        canvas.draw_vertices(&verts, BlendMode::Modulate, &paint);
    }
}
