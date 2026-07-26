//! Block-entity models, baked into the held-item shape (M25b).
//!
//! M25 decoded block entities and measured the gap they leave: ~86 blocks whose
//! models bake to no geometry, so a client without a `BlockEntityRenderer`
//! draws nothing at all. This module closes the largest slice of that — the
//! chest family, which is the most common invisible block by a wide margin.
//!
//! **Why they are baked as [`crate::held_items::HeldItemModel`]s.** That type
//! already means exactly what a block-entity model is: *quads in model units
//! 0..16, with UVs in 0..1 of their own texture*. Reusing it means the chest
//! flows through the texture pool, the entity atlas, the demand-fill upload and
//! the UV lookup that M22 built for held items, with no parallel pipeline. The
//! names are namespaced `rewo:be/…` so they cannot collide with an item.
//!
//! Ground truth: `client/model/object/chest/ChestModel.java` and
//! `client/renderer/blockentity/ChestRenderer.java`.
//!
//! ```text
//! createSingleBodyLayer(), texture 64x64:
//!   bottom  texOffs(0,19) addBox(1, 0, 1, 14, 10, 14)   PartPose.ZERO
//!   lid     texOffs(0, 0) addBox(1, 0, 0, 14,  5, 14)   offset(0, 9, 1)
//!   lock    texOffs(0, 0) addBox(7,-2,14,  2,  4,  1)   offset(0, 9, 1)
//! ```

use crate::held_items::{HeldItemModel, HeldQuad, HeldTexture, TexturePool};
use crate::item_models::DisplayTransform;

/// One cuboid of a block-entity model, in vanilla's `addBox` terms.
struct Box {
    tex: (f32, f32),
    /// `addBox(x, y, z, …)` — the min corner, in model px.
    min: [f32; 3],
    /// `(width, height, depth)`.
    dims: [f32; 3],
    /// `PartPose.offset(...)` applied to the whole box.
    ///
    /// For an animated part this is also its **pivot**: `ModelPart.render`
    /// translates by the pose offset and then rotates, so the box's own
    /// coordinates are relative to it.
    offset: [f32; 3],
    /// The animated group (see [`crate::held_items::HeldQuad::part`]).
    part: u8,
    /// A face vanilla's `addBox(..., visibleFaces)` leaves out.
    hide: Option<Facing>,
}

/// The pivot a chest's lid and lock rotate about — `PartPose.offset(0, 9, 1)`,
/// in model px. Both parts share it, which is why they share a group.
pub const CHEST_LID_PIVOT: [f32; 3] = [0.0, 9.0, 1.0];

/// The animated group a chest's lid and lock belong to.
pub const CHEST_LID_PART: u8 = 1;

/// `ChestModel.createSingleBodyLayer` — a closed single chest.
const CHEST_SINGLE: &[Box] = &[
    Box {
        tex: (0.0, 19.0),
        min: [1.0, 0.0, 1.0],
        dims: [14.0, 10.0, 14.0],
        offset: [0.0; 3],
        part: 0,
        hide: None,
    },
    Box {
        tex: (0.0, 0.0),
        min: [1.0, 0.0, 0.0],
        dims: [14.0, 5.0, 14.0],
        offset: CHEST_LID_PIVOT,
        part: CHEST_LID_PART,
        hide: None,
    },
    Box {
        tex: (0.0, 0.0),
        min: [7.0, -2.0, 14.0],
        dims: [2.0, 4.0, 1.0],
        offset: CHEST_LID_PIVOT,
        part: CHEST_LID_PART,
        hide: None,
    },
];

/// `ChestModel.createDoubleBodyLeftLayer` — the LEFT half of a double chest.
///
/// ```text
/// visibleFaces = allOfEnumExcept(WEST)
/// bottom  texOffs(0,19) addBox(0, 0, 1, 15, 10, 14)   PartPose.ZERO
/// lid     texOffs(0, 0) addBox(0, 0, 0, 15,  5, 14)   offset(0, 9, 1)
/// lock    texOffs(0, 0) addBox(0,-2,14,  1,  4,  1)   offset(0, 9, 1)
/// ```
///
/// The dropped face is the one that meets the other half: rendering it would
/// put two coincident quads inside the seam, which z-fights.
const CHEST_LEFT: &[Box] = &[
    Box {
        tex: (0.0, 19.0),
        min: [0.0, 0.0, 1.0],
        dims: [15.0, 10.0, 14.0],
        offset: [0.0; 3],
        part: 0,
        hide: Some(Facing::West),
    },
    Box {
        tex: (0.0, 0.0),
        min: [0.0, 0.0, 0.0],
        dims: [15.0, 5.0, 14.0],
        offset: CHEST_LID_PIVOT,
        part: CHEST_LID_PART,
        hide: Some(Facing::West),
    },
    Box {
        tex: (0.0, 0.0),
        min: [0.0, -2.0, 14.0],
        dims: [1.0, 4.0, 1.0],
        offset: CHEST_LID_PIVOT,
        part: CHEST_LID_PART,
        hide: Some(Facing::West),
    },
];

/// `ChestModel.createDoubleBodyRightLayer` — the RIGHT half, dropping EAST.
const CHEST_RIGHT: &[Box] = &[
    Box {
        tex: (0.0, 19.0),
        min: [1.0, 0.0, 1.0],
        dims: [15.0, 10.0, 14.0],
        offset: [0.0; 3],
        part: 0,
        hide: Some(Facing::East),
    },
    Box {
        tex: (0.0, 0.0),
        min: [1.0, 0.0, 0.0],
        dims: [15.0, 5.0, 14.0],
        offset: CHEST_LID_PIVOT,
        part: CHEST_LID_PART,
        hide: Some(Facing::East),
    },
    Box {
        tex: (0.0, 0.0),
        min: [15.0, -2.0, 14.0],
        dims: [1.0, 4.0, 1.0],
        offset: CHEST_LID_PIVOT,
        part: CHEST_LID_PART,
        hide: Some(Facing::East),
    },
];

/// The face labels `cube_faces` returns, in order — so a `hide` can name one.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Facing {
    Down,
    Up,
    West,
    North,
    East,
    South,
}

const FACE_ORDER: [Facing; 6] = [
    Facing::Down,
    Facing::Up,
    Facing::West,
    Facing::North,
    Facing::East,
    Facing::South,
];

/// The chest variants Rewo renders, as `(model name, jar texture path)`.
///
/// `Sheets.chooseSprite(material, type)` picks these; the SINGLE row of the
/// table is what a closed single chest uses. Christmas textures are
/// deliberately absent — `SpecialDates.isExtendedChristmas()` is a wall-clock
/// check, and a renderer whose output depends on the date is not reproducible
/// in a gate.
/// Every variant is baked three times — single, left half, right half — with
/// the matching `_left` / `_right` texture, because `Sheets.chooseSprite`
/// selects the sprite by `ChestType` as well as by material.
///
/// The ender chest is the exception and ships no halves: it is always single,
/// and the jar has no `ender_left.png` to bake one from.
pub const CHESTS: &[(&str, &str, bool)] = &[
    ("rewo:be/chest", "entity/chest/normal", true),
    ("rewo:be/trapped_chest", "entity/chest/trapped", true),
    ("rewo:be/ender_chest", "entity/chest/ender", false),
    ("rewo:be/copper_chest", "entity/chest/copper", true),
    ("rewo:be/exposed_copper_chest", "entity/chest/copper_exposed", true),
    ("rewo:be/weathered_copper_chest", "entity/chest/copper_weathered", true),
    ("rewo:be/oxidized_copper_chest", "entity/chest/copper_oxidized", true),
];

/// The name suffix for a chest half. Appended to the single model's name and
/// to its texture path alike, which is what keeps the two in step.
pub const LEFT_SUFFIX: &str = "_left";
pub const RIGHT_SUFFIX: &str = "_right";

/// Vanilla `Direction` ordinals for the six faces, in the order
/// [`rewo_gpu::mobs::cube_faces`] returns them.
///
/// Duplicated from the mob port rather than imported: `rewo-data` does not
/// depend on `rewo-gpu`, and this is a six-entry constant.
const FACE_DIRS: [u8; 6] = [0, 1, 2, 5, 4, 3];

/// `ShulkerModel.createShellMesh` — a closed shulker box, texture 64x64.
///
/// ```text
/// lid   texOffs(0, 0)  addBox(-8,-16,-8, 16,12,16)  PartPose.offset(0,24,0)
/// base  texOffs(0,28)  addBox(-8, -8,-8, 16, 8,16)  PartPose.offset(0,24,0)
/// ```
///
/// The negative y is not a mistake: `ShulkerBoxRenderer`'s transform ends in
/// `scale(1, -1, -1)`, so the model is authored upside down and the renderer
/// flips it. Closed is the rest pose — `setupAnim(0)` computes
/// `lid.setPos(0, 16 + sin(PI/2)*8, 0)`, which is the (0, 24, 0) it already
/// has — so a shut box needs no animation at all.
const SHULKER_BOX: &[Box] = &[
    Box {
        tex: (0.0, 0.0),
        min: [-8.0, -16.0, -8.0],
        dims: [16.0, 12.0, 16.0],
        offset: [0.0, 24.0, 0.0],
        part: 0,
        hide: None,
    },
    Box {
        tex: (0.0, 28.0),
        min: [-8.0, -8.0, -8.0],
        dims: [16.0, 8.0, 16.0],
        offset: [0.0, 24.0, 0.0],
        part: 0,
        hide: None,
    },
];

/// `ColorCollection.NAMES`, the dye order shulker box textures are named in.
pub const DYE_COLORS: &[&str] = &[
    "white", "orange", "magenta", "light_blue", "yellow", "lime", "pink", "gray",
    "light_gray", "cyan", "purple", "blue", "brown", "green", "red", "black",
];

/// The undyed shulker box's model name and texture.
///
/// `ShulkerBoxRenderer.submit` uses `Sheets.DEFAULT_SHULKER_TEXTURE_LOCATION`
/// when `getColor()` is null, and `getShulkerBoxSprite(color)` otherwise.
pub const SHULKER_DEFAULT: (&str, &str) = ("rewo:be/shulker_box", "entity/shulker/shulker");

/// Bake the shulker-box models — the undyed one and the sixteen dyed.
pub fn bake_shulker_boxes(
    pool: &mut TexturePool,
    load: &mut dyn FnMut(&str) -> Option<HeldTexture>,
) -> Vec<(String, HeldItemModel)> {
    let mut variants: Vec<(String, String)> = vec![(
        SHULKER_DEFAULT.0.to_string(),
        SHULKER_DEFAULT.1.to_string(),
    )];
    for c in DYE_COLORS {
        variants.push((
            format!("rewo:be/{c}_shulker_box"),
            format!("entity/shulker/shulker_{c}"),
        ));
    }
    let mut out = Vec::new();
    for (name, tex_name) in variants {
        let Some(tex) = pool.intern(&tex_name, || load(&tex_name)) else {
            continue;
        };
        out.push((
            name,
            HeldItemModel {
                quads: chest_quads(SHULKER_BOX, tex),
                right: DisplayTransform::default(),
                left: DisplayTransform::default(),
                ground: DisplayTransform::default(),
                from_block: false,
            },
        ));
    }
    out
}

/// Bake the chest models, interning their textures into `pool`.
///
/// A variant whose texture is missing from the jar is **skipped**, not drawn
/// untextured — the same rule the item bake applies. `load` is given a
/// jar-relative texture path without the `.png`.
pub fn bake_chests(
    pool: &mut TexturePool,
    load: &mut dyn FnMut(&str) -> Option<HeldTexture>,
) -> Vec<(String, HeldItemModel)> {
    let mut out = Vec::new();
    for (name, tex_path, has_halves) in CHESTS {
        let mut variants: Vec<(String, String, &[Box])> =
            vec![((*name).to_string(), (*tex_path).to_string(), CHEST_SINGLE)];
        if *has_halves {
            variants.push((
                format!("{name}{LEFT_SUFFIX}"),
                format!("{tex_path}{LEFT_SUFFIX}"),
                CHEST_LEFT,
            ));
            variants.push((
                format!("{name}{RIGHT_SUFFIX}"),
                format!("{tex_path}{RIGHT_SUFFIX}"),
                CHEST_RIGHT,
            ));
        }
        for (model_name, tex_name, boxes) in variants {
        let Some(tex) = pool.intern(&tex_name, || load(&tex_name)) else {
            continue;
        };
        out.push((
            model_name,
            HeldItemModel {
                quads: chest_quads(boxes, tex),
                // A block entity is placed by its own transform, never by a
                // display context — these exist only because the shape is
                // shared with held items, and are the identity so that a
                // mis-wired caller renders it at the block origin rather than
                // somewhere plausible-looking.
                right: DisplayTransform::default(),
                left: DisplayTransform::default(),
                ground: DisplayTransform::default(),
                from_block: false,
            },
        ));
        }
    }
    out
}

/// The three cuboids, unwrapped against the 64×64 chest texture.
fn chest_quads(boxes: &[Box], tex: u16) -> Vec<HeldQuad> {
    const ATLAS: f32 = 64.0;
    let mut quads = Vec::with_capacity(boxes.len() * 6);
    for b in boxes {
        for (i, (verts, uv)) in cube_faces(b.tex, b.min, b.dims).into_iter().enumerate() {
            // `addBox(..., visibleFaces)` — the seam face of a double half is
            // simply not built.
            if b.hide == Some(FACE_ORDER[i]) {
                continue;
            }
            let mut v = [[0f32; 3]; 4];
            for (k, c) in verts.iter().enumerate() {
                // `PartPose.offset` translates the whole box; the model is
                // y-up here (unlike an entity model, which is rendered through
                // a `scale(-1,-1,1)`), because `BlockEntityRenderer` has no
                // such flip.
                v[k] = [
                    c[0] + b.offset[0],
                    c[1] + b.offset[1],
                    c[2] + b.offset[2],
                ];
            }
            let mut t = [[0f32; 2]; 4];
            for (k, c) in uv.iter().enumerate() {
                t[k] = [c[0] / ATLAS, c[1] / ATLAS];
            }
            quads.push(HeldQuad {
                verts: v,
                uv: t,
                tex,
                part: b.part,
                dir: FACE_DIRS[i],
            });
        }
    }
    quads
}

/// `ModelPart.Cube` + `Polygon`, for the block-entity bake.
///
/// A second transcription of the same vanilla source `rewo_gpu::mobs` ports,
/// kept here because `rewo-data` does not depend on `rewo-gpu` — the same
/// crate-boundary rule the `HeldQuad` / `DisplayTransform` mirrors follow. No
/// grow and no mirror: a chest uses neither.
fn cube_faces(
    tex: (f32, f32),
    min: [f32; 3],
    dims: [f32; 3],
) -> [([[f32; 3]; 4], [[f32; 2]; 4]); 6] {
    let (tex_u, tex_v) = tex;
    let [w, h, d] = dims;
    let [x0, y0, z0] = min;
    let (x1, y1, z1) = (x0 + w, y0 + h, z0 + d);
    let t0 = [x0, y0, z0];
    let t1 = [x1, y0, z0];
    let t2 = [x1, y1, z0];
    let t3 = [x0, y1, z0];
    let l0 = [x0, y0, z1];
    let l1 = [x1, y0, z1];
    let l2 = [x1, y1, z1];
    let l3 = [x0, y1, z1];
    let (u0, u1, u2) = (tex_u, tex_u + d, tex_u + d + w);
    let u22 = tex_u + d + w + w;
    let u3 = tex_u + d + w + d;
    let u4 = tex_u + d + w + d + w;
    let (v0, v1, v2) = (tex_v, tex_v + d, tex_v + d + h);

    // `Polygon` remaps a `(u0,v0,u1,v1)` rect onto its four vertices as
    // [0]=(u1,v0) [1]=(u0,v0) [2]=(u0,v1) [3]=(u1,v1).
    let face = |verts: [[f32; 3]; 4], r: (f32, f32, f32, f32)| {
        let (a, b, c, e) = r;
        (verts, [[c, b], [a, b], [a, e], [c, e]])
    };
    [
        face([l1, l0, t0, t1], (u1, v0, u2, v1)),  // DOWN
        face([t2, t3, l3, l2], (u2, v1, u22, v0)), // UP — v1 before v0, flipped
        face([t0, l0, l3, t3], (u0, v1, u1, v2)),  // WEST
        face([t1, t0, t3, t2], (u1, v1, u2, v2)),  // NORTH
        face([l1, t1, t2, l2], (u2, v1, u3, v2)),  // EAST
        face([l0, l1, l2, l3], (u3, v1, u4, v2)),  // SOUTH
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_chest_bakes_three_cuboids_of_six_faces() {
        let quads = chest_quads(CHEST_SINGLE, 0);
        assert_eq!(quads.len(), 18, "3 boxes x 6 faces");
    }

    #[test]
    fn the_chest_occupies_the_block_it_sits_in() {
        // The bottom spans 1..15 in x/z and 0..10 in y; the lid sits on top of
        // it (offset y 9, height 5) so the whole model reaches y 14 and never
        // leaves the 0..16 block.
        let quads = chest_quads(CHEST_SINGLE, 0);
        let mut lo = [f32::MAX; 3];
        let mut hi = [f32::MIN; 3];
        for q in &quads {
            for v in &q.verts {
                for k in 0..3 {
                    lo[k] = lo[k].min(v[k]);
                    hi[k] = hi[k].max(v[k]);
                }
            }
        }
        // The lid box is addBox(1,0,0,…) but PartPose.offset(0,9,1) shifts it,
        // so nothing reaches z 0; and the lock ends flush with the block face
        // at z 16, which is what makes the latch visible from outside.
        assert_eq!(lo, [1.0, 0.0, 1.0], "min corner");
        assert_eq!(hi, [15.0, 14.0, 16.0], "max corner");
        for k in 0..3 {
            assert!(lo[k] >= 0.0 && hi[k] <= 16.0, "axis {k} leaves the block");
        }
    }

    #[test]
    fn every_uv_is_inside_the_sixty_four_square_texture() {
        for q in chest_quads(CHEST_SINGLE, 0) {
            for uv in q.uv {
                assert!(
                    (0.0..=1.0).contains(&uv[0]) && (0.0..=1.0).contains(&uv[1]),
                    "uv {uv:?} outside the texture"
                );
            }
        }
    }

    #[test]
    fn the_lock_sits_proud_of_the_front_face() {
        // addBox(7,-2,14, 2,4,1) with offset(0,9,1) puts the lock at z 15..16
        // — proud of the lid, which ends at 15. That is the visible latch.
        let quads = chest_quads(CHEST_SINGLE, 0);
        let max_z = quads
            .iter()
            .flat_map(|q| q.verts.iter())
            .fold(f32::MIN, |m, v| m.max(v[2]));
        assert_eq!(max_z, 16.0);
    }
}
