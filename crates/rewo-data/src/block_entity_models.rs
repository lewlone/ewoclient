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
    /// `PartPose.offsetAndRotation(...)`'s rest rotation, in radians `(x, y,
    /// z)`, applied about [`Self::offset`] before the offset translates.
    ///
    /// `ModelPart.rotate` runs Z, then Y, then X (`rotateZ` ∘ `rotateY` ∘
    /// `rotateX` in JOML post-multiply order), so a point sees X first.
    rot: [f32; 3],
    /// `CubeDeformation` — a uniform grow on every side. Vanilla's overlay
    /// layers are the same box at a positive deformation, and a *negative* one
    /// is a shrink (the decorated pot's neck uses both).
    grow: f32,
    /// `CubeListBuilder.mirror(true)` — swaps the box's x extremes and
    /// reverses each face's vertex array, so the texture reads the other way
    /// round. A model with a mirrored pair draws both from **one** rect.
    mirror: bool,
    /// The animated group (see [`crate::held_items::HeldQuad::part`]).
    part: u8,
    /// A face vanilla's `addBox(..., visibleFaces)` leaves out.
    hide: Option<Facing>,
}

/// A `Box` with every optional field at its vanilla default, so a model that
/// needs none of them reads as the `addBox` call it transcribes.
const fn plain(
    tex: (f32, f32),
    min: [f32; 3],
    dims: [f32; 3],
    offset: [f32; 3],
    part: u8,
) -> Box {
    Box {
        tex,
        min,
        dims,
        offset,
        rot: [0.0; 3],
        grow: 0.0,
        mirror: false,
        part,
        hide: None,
    }
}

/// The pivot a chest's lid and lock rotate about — `PartPose.offset(0, 9, 1)`,
/// in model px. Both parts share it, which is why they share a group.
pub const CHEST_LID_PIVOT: [f32; 3] = [0.0, 9.0, 1.0];

/// The animated group a chest's lid and lock belong to.
pub const CHEST_LID_PART: u8 = 1;

/// The shulker lid's pose offset — `PartPose.offset(0, 24, 0)`, which is also
/// the pivot its `yRot` turns about and the position its `setPos` replaces.
pub const SHULKER_LID_PIVOT: [f32; 3] = [0.0, 24.0, 0.0];

/// The animated group a shulker box's lid belongs to. Its base is group 0.
///
/// Numbered the same as the chest's because a model only ever has one animated
/// group here — the number selects *within* a model, not across them.
pub const SHULKER_LID_PART: u8 = 1;

/// `ChestModel.createSingleBodyLayer` — a closed single chest.
const CHEST_SINGLE: &[Box] = &[
    plain((0.0, 19.0), [1.0, 0.0, 1.0], [14.0, 10.0, 14.0], [0.0; 3], 0),
    plain((0.0, 0.0), [1.0, 0.0, 0.0], [14.0, 5.0, 14.0], CHEST_LID_PIVOT, CHEST_LID_PART),
    plain((0.0, 0.0), [7.0, -2.0, 14.0], [2.0, 4.0, 1.0], CHEST_LID_PIVOT, CHEST_LID_PART),
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
    Box { hide: Some(Facing::West), ..plain((0.0, 19.0), [0.0, 0.0, 1.0], [15.0, 10.0, 14.0], [0.0; 3], 0) },
    Box { hide: Some(Facing::West), ..plain((0.0, 0.0), [0.0, 0.0, 0.0], [15.0, 5.0, 14.0], CHEST_LID_PIVOT, CHEST_LID_PART) },
    Box { hide: Some(Facing::West), ..plain((0.0, 0.0), [0.0, -2.0, 14.0], [1.0, 4.0, 1.0], CHEST_LID_PIVOT, CHEST_LID_PART) },
];

/// `ChestModel.createDoubleBodyRightLayer` — the RIGHT half, dropping EAST.
const CHEST_RIGHT: &[Box] = &[
    Box { hide: Some(Facing::East), ..plain((0.0, 19.0), [1.0, 0.0, 1.0], [15.0, 10.0, 14.0], [0.0; 3], 0) },
    Box { hide: Some(Facing::East), ..plain((0.0, 0.0), [1.0, 0.0, 0.0], [15.0, 5.0, 14.0], CHEST_LID_PIVOT, CHEST_LID_PART) },
    Box { hide: Some(Facing::East), ..plain((0.0, 0.0), [15.0, -2.0, 14.0], [1.0, 4.0, 1.0], CHEST_LID_PIVOT, CHEST_LID_PART) },
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
/// `lid.setPos(0, 24 - 0*8, 0)`, which is the (0, 24, 0) it already has — so a
/// shut box is the baked geometry untouched.
///
/// The lid is its own animated group (M26): `setupAnim(progress)` both moves
/// and spins it, which is why the group's transform is a matrix rather than an
/// angle. See [`crate::be_transform::shulker_lid`].
const SHULKER_BOX: &[Box] = &[
    plain((0.0, 0.0), [-8.0, -16.0, -8.0], [16.0, 12.0, 16.0], SHULKER_LID_PIVOT, SHULKER_LID_PART),
    plain((0.0, 28.0), [-8.0, -8.0, -8.0], [16.0, 8.0, 16.0], [0.0, 24.0, 0.0], 0),
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

// ------------------------------------------------------------------ skulls
//
// `SkullBlockRenderer` (M28). Seven types across fourteen blocks — a ground
// and a wall variant each.
//
// **These are entity models, not block-entity ones**, and the difference is
// load-bearing: `SkullModelBase` is authored y-down like a mob, and both skull
// transforms end in `scale(-1, -1, 1)` to put it the right way up. A chest has
// no such flip. Getting this backwards renders a skull upside down and
// mirrored, which reads as a texture bug.

/// `SkullModel.createHeadModel` — the one box every skull shares.
///
/// `texOffs(0, 0) addBox(-4, -8, -4, 8, 8, 8)` at `PartPose.ZERO`, so the head
/// hangs *below* its origin: in the flipped space that puts it sitting on the
/// block.
const SKULL_HEAD: Box = plain((0.0, 0.0), [-4.0, -8.0, -4.0], [8.0, 8.0, 8.0], [0.0; 3], 0);

/// `createMobHeadLayer` — the head alone, on a 64×32 sheet.
const SKULL_MOB: &[Box] = &[SKULL_HEAD];

/// `createHumanoidHeadLayer` — the head plus the `hat` overlay, 64×64.
///
/// The hat is the same box at `CubeDeformation(0.25)`, which is what makes a
/// player head's second skin layer stand a quarter-pixel proud of the first.
const SKULL_HUMANOID: &[Box] = &[
    SKULL_HEAD,
    Box {
        grow: 0.25,
        ..plain((32.0, 0.0), [-4.0, -8.0, -4.0], [8.0, 8.0, 8.0], [0.0; 3], 0)
    },
];

/// `AbstractPiglinModel.addHead` — head, snout, two tusks and two ears, 64×64.
///
/// The ears carry a rest rotation of ∓30° about Z, which is the first model
/// here to need one.
const SKULL_PIGLIN: &[Box] = &[
    plain((0.0, 0.0), [-5.0, -8.0, -4.0], [10.0, 8.0, 8.0], [0.0; 3], 0),
    plain((31.0, 1.0), [-2.0, -4.0, -5.0], [4.0, 4.0, 1.0], [0.0; 3], 0),
    plain((2.0, 4.0), [2.0, -2.0, -5.0], [1.0, 2.0, 1.0], [0.0; 3], 0),
    plain((2.0, 0.0), [-3.0, -2.0, -5.0], [1.0, 2.0, 1.0], [0.0; 3], 0),
    Box {
        rot: [0.0, 0.0, -std::f32::consts::FRAC_PI_6],
        ..plain((51.0, 6.0), [0.0, 0.0, -2.0], [1.0, 5.0, 4.0], [4.5, -6.0, 0.0], 0)
    },
    Box {
        rot: [0.0, 0.0, std::f32::consts::FRAC_PI_6],
        ..plain((39.0, 6.0), [-1.0, 0.0, -2.0], [1.0, 5.0, 4.0], [-4.5, -6.0, 0.0], 0)
    },
];

/// `DragonHeadModel.createHeadLayer` — 256×256, and the only skull with a
/// mirrored pair.
///
/// `mirror(true)` covers the left scale and nostril, then `mirror(false)` the
/// right: both sides come from **one** texture rect read in opposite
/// directions, which is why the mirror flag exists rather than a second rect.
///
/// The head's pose is `offset(0, -7.986666, 0).scaled(0.75)` and the jaw hangs
/// off it at `offset(0, 4, -8)`. The scale is folded into the box coordinates
/// here — the model has no animated group, so nothing needs the pose to stay
/// separable — and the jaw's own offset is pre-multiplied by the same 0.75 for
/// the same reason.
const DRAGON_SCALE: f32 = 0.75;
const DRAGON_HEAD_Y: f32 = -7.986_666;

/// `DragonHeadModel`'s boxes, before the head pose's scale and offset. Applied
/// by [`dragon_skull`], which is a function rather than a const because the
/// pose has to multiply through.
const DRAGON_RAW: &[Box] = &[
    plain((176.0, 44.0), [-6.0, -1.0, -24.0], [12.0, 5.0, 16.0], [0.0; 3], 0),
    plain((112.0, 30.0), [-8.0, -8.0, -10.0], [16.0, 16.0, 16.0], [0.0; 3], 0),
    Box {
        mirror: true,
        ..plain((0.0, 0.0), [-5.0, -12.0, -4.0], [2.0, 4.0, 6.0], [0.0; 3], 0)
    },
    Box {
        mirror: true,
        ..plain((112.0, 0.0), [-5.0, -3.0, -22.0], [2.0, 2.0, 4.0], [0.0; 3], 0)
    },
    plain((0.0, 0.0), [3.0, -12.0, -4.0], [2.0, 4.0, 6.0], [0.0; 3], 0),
    plain((112.0, 0.0), [3.0, -3.0, -22.0], [2.0, 2.0, 4.0], [0.0; 3], 0),
    // The jaw, whose own pose offset rides inside the head's scale.
    plain((176.0, 65.0), [-6.0, 0.0, -16.0], [12.0, 4.0, 16.0], [0.0, 4.0, -8.0], 0),
];

/// The dragon skull's boxes with the head pose applied.
fn dragon_skull() -> Vec<Box> {
    DRAGON_RAW
        .iter()
        .map(|b| Box {
            tex: b.tex,
            min: [
                b.min[0] * DRAGON_SCALE,
                b.min[1] * DRAGON_SCALE,
                b.min[2] * DRAGON_SCALE,
            ],
            dims: [
                b.dims[0] * DRAGON_SCALE,
                b.dims[1] * DRAGON_SCALE,
                b.dims[2] * DRAGON_SCALE,
            ],
            offset: [
                b.offset[0] * DRAGON_SCALE,
                b.offset[1] * DRAGON_SCALE + DRAGON_HEAD_Y,
                b.offset[2] * DRAGON_SCALE,
            ],
            rot: b.rot,
            // The deformation is in *unscaled* px in vanilla, but every dragon
            // box has none, so scaling it is moot and left alone.
            grow: b.grow,
            mirror: b.mirror,
            part: b.part,
            hide: b.hide,
        })
        .collect()
}

/// The skull types, as `(model name, jar texture, texture size, boxes)`.
///
/// The player head uses the jar's default wide skin. A real player head
/// carries a profile in its NBT and vanilla fetches that skin — deliberately
/// not done here, for the same reason M7c's fetch is a runtime pool: it is a
/// network round trip, and a head whose appearance depends on one is not
/// reproducible in a gate.
pub const SKULL_TEXTURES: &[(&str, &str)] = &[
    ("rewo:be/skeleton_skull", "entity/skeleton/skeleton"),
    (
        "rewo:be/wither_skeleton_skull",
        "entity/skeleton/wither_skeleton",
    ),
    ("rewo:be/zombie_head", "entity/zombie/zombie"),
    ("rewo:be/creeper_head", "entity/creeper/creeper"),
    ("rewo:be/player_head", "entity/player/wide/steve"),
    ("rewo:be/piglin_head", "entity/piglin/piglin"),
    ("rewo:be/dragon_head", "entity/enderdragon/dragon"),
];

/// Bake the seven skull models.
pub fn bake_skulls(
    pool: &mut TexturePool,
    load: &mut dyn FnMut(&str) -> Option<HeldTexture>,
) -> Vec<(String, HeldItemModel)> {
    let dragon = dragon_skull();
    let mut out = Vec::new();
    for (name, tex_path) in SKULL_TEXTURES {
        let (boxes, size): (&[Box], (f32, f32)) = match *name {
            // `createMobHeadLayer` — head only, on a half-height sheet.
            "rewo:be/skeleton_skull"
            | "rewo:be/wither_skeleton_skull"
            | "rewo:be/creeper_head" => (SKULL_MOB, (64.0, 32.0)),
            "rewo:be/piglin_head" => (SKULL_PIGLIN, TEX_64),
            "rewo:be/dragon_head" => (&dragon, (256.0, 256.0)),
            // `createHumanoidHeadLayer` — head + hat, full sheet.
            _ => (SKULL_HUMANOID, TEX_64),
        };
        let Some(tex) = pool.intern(tex_path, || load(tex_path)) else {
            continue;
        };
        out.push((
            (*name).to_string(),
            HeldItemModel {
                quads: model_quads(boxes, tex, size),
                right: DisplayTransform::default(),
                left: DisplayTransform::default(),
                ground: DisplayTransform::default(),
                from_block: false,
            },
        ));
    }
    out
}

// ----------------------------------------------------------------- conduit
//
// `ConduitRenderer` (M28). Four models in vanilla — an inactive shell, an
// active cage, a wind shroud and an eye — of which this ships the shell.

/// `ConduitRenderer.createShellLayer` — the dormant conduit.
///
/// `texOffs(0, 0) addBox(-3, -3, -3, 6, 6, 6)` on a **32×16** sheet, centred
/// on its own origin (the renderer translates to the block centre), so unlike
/// a skull this one is symmetric about zero and needs no flip.
const CONDUIT_SHELL: &[Box] =
    &[plain((0.0, 0.0), [-3.0, -3.0, -3.0], [6.0, 6.0, 6.0], [0.0; 3], 0)];

/// The conduit shell's model name and jar texture.
pub const CONDUIT: (&str, &str) = ("rewo:be/conduit", "entity/conduit/base");

/// Bake the conduit shell.
pub fn bake_conduit(
    pool: &mut TexturePool,
    load: &mut dyn FnMut(&str) -> Option<HeldTexture>,
) -> Vec<(String, HeldItemModel)> {
    let Some(tex) = pool.intern(CONDUIT.1, || load(CONDUIT.1)) else {
        return Vec::new();
    };
    vec![(
        CONDUIT.0.to_string(),
        HeldItemModel {
            quads: model_quads(CONDUIT_SHELL, tex, (32.0, 16.0)),
            right: DisplayTransform::default(),
            left: DisplayTransform::default(),
            ground: DisplayTransform::default(),
            from_block: false,
        },
    )]
}

/// The default texture size for the chest and shulker models — 64×64.
const TEX_64: (f32, f32) = (64.0, 64.0);

/// Unwrap a model's cuboids against its own texture.
///
/// `tex_size` is the `LayerDefinition.create(mesh, w, h)` pair, which is **not
/// always square and not always 64**: a mob head sheet is 64×32 and a dragon
/// skull is 256×256. Hard-coding 64 was fine while only chests and shulker
/// boxes existed and is exactly the kind of assumption that renders a new model
/// with plausible-looking garbage UVs.
fn model_quads(boxes: &[Box], tex: u16, tex_size: (f32, f32)) -> Vec<HeldQuad> {
    let (atlas_w, atlas_h) = tex_size;
    let mut quads = Vec::with_capacity(boxes.len() * 6);
    for b in boxes {
        // `ModelPart.rotate` — Z, then Y, then X in JOML's post-multiply
        // order, so a point is turned by X first.
        let [rx, ry, rz] = b.rot;
        let (sx, cx) = rx.sin_cos();
        let (sy, cy) = ry.sin_cos();
        let (sz, cz) = rz.sin_cos();
        let rotate = |p: [f32; 3]| -> [f32; 3] {
            let p = [p[0], p[1] * cx - p[2] * sx, p[1] * sx + p[2] * cx];
            let p = [p[0] * cy + p[2] * sy, p[1], -p[0] * sy + p[2] * cy];
            [p[0] * cz - p[1] * sz, p[0] * sz + p[1] * cz, p[2]]
        };
        for (i, (verts, uv)) in cube_faces(b.tex, b.min, b.dims, b.grow, b.mirror)
            .into_iter()
            .enumerate()
        {
            // `addBox(..., visibleFaces)` — the seam face of a double half is
            // simply not built.
            if b.hide == Some(FACE_ORDER[i]) {
                continue;
            }
            let mut v = [[0f32; 3]; 4];
            for (k, c) in verts.iter().enumerate() {
                // Rotate about the pose origin, then translate by it. The
                // model is y-up here (unlike an entity model, which is
                // rendered through a `scale(-1,-1,1)`) — except where the
                // renderer's own transform supplies that flip, which is why
                // the skull models below are authored the entity way up.
                let r = rotate(*c);
                v[k] = [
                    r[0] + b.offset[0],
                    r[1] + b.offset[1],
                    r[2] + b.offset[2],
                ];
            }
            let mut t = [[0f32; 2]; 4];
            for (k, c) in uv.iter().enumerate() {
                t[k] = [c[0] / atlas_w, c[1] / atlas_h];
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

/// The chest and shulker models, which are all 64×64.
fn chest_quads(boxes: &[Box], tex: u16) -> Vec<HeldQuad> {
    model_quads(boxes, tex, TEX_64)
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
    grow: f32,
    mirror: bool,
) -> [([[f32; 3]; 4], [[f32; 2]; 4]); 6] {
    let (tex_u, tex_v) = tex;
    let [w, h, d] = dims;
    let [mut x0, mut y0, mut z0] = min;
    let (mut x1, mut y1, mut z1) = (x0 + w, y0 + h, z0 + d);
    // `CubeDeformation` grows every side, so the box keeps its centre. A
    // negative value shrinks it, which is what an inset detail uses.
    x0 -= grow;
    y0 -= grow;
    z0 -= grow;
    x1 += grow;
    y1 += grow;
    z1 += grow;
    if mirror {
        std::mem::swap(&mut x0, &mut x1);
    }
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
        let mut vs = verts;
        let mut uvs = [[c, b], [a, b], [a, e], [c, e]];
        if mirror {
            // Vanilla reverses the vertex array; each vertex keeps its UV, so
            // the rect reads the other way round across the face.
            vs.reverse();
            uvs.reverse();
        }
        (vs, uvs)
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
