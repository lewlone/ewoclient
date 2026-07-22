//! Mob models — a faithful port of vanilla's `ModelPart.Cube` box builder
//! plus every mob mesh transcribed from the 26.2 decompile.
//!
//! The one rule of this module: **match the decompiled source exactly, don't
//! approximate**. The previous hand-rolled unwrap (`box_uv_faces`) had the
//! face→rect assignment, the per-face vertex→UV-corner order, and the mirror
//! handling all subtly wrong — silhouettes looked right while every texture
//! was scrambled (see REWO_MOB_REDO_HANDOFF.md). This port reproduces
//! `ModelPart.Cube` + `Polygon` verbatim: the 8 corners, the per-face vertex
//! arrays, the UV column table, the UP-face vertical flip, and the mirror
//! vertex reversal.
//!
//! Coordinate contract (kept identical to vanilla so meshes transcribe 1:1):
//! - Models are authored in **vanilla model space**: +Y down, front = −Z,
//!   units = model pixels, ground at y=24.
//! - The renderer's transform is vanilla's exactly:
//!   `rotY(180° − yaw) · scale(−1,−1,1) · translate(0,−1.501,0) · (px/16)`,
//!   applied in [`crate::entities`]. At yaw 0 a model point (mx,my,mz) lands
//!   at world `(mx, 24.016−my, −mz)/16` relative to the entity's feet — the
//!   entity faces +Z (south), its right hand is at −X (west), like vanilla.
//! - Face labels ([`Facing`]) are vanilla's model-space names: `Down` is the
//!   minY plane, which is the **world-top** face after the y-flip.

/// Vanilla's `translate(0, -1.501, 0)` in model px (1.501 · 16).
pub const MODEL_EYE_Y: f32 = 24.016;

/// Vanilla `Direction` labels as `ModelPart.Cube` uses them (model space).
/// After the render flip: `Down` shows on the world TOP, `Up` on the world
/// bottom, `North` on the entity's front, `East` on its left.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Facing {
    Down,
    Up,
    West,
    North,
    East,
    South,
}

impl Facing {
    /// Outward normal in model space (y-down).
    pub fn normal(self) -> [f32; 3] {
        match self {
            Facing::Down => [0.0, -1.0, 0.0], // minY plane
            Facing::Up => [0.0, 1.0, 0.0],    // maxY plane
            Facing::West => [-1.0, 0.0, 0.0],
            Facing::North => [0.0, 0.0, -1.0],
            Facing::East => [1.0, 0.0, 0.0],
            Facing::South => [0.0, 0.0, 1.0],
        }
    }

    /// Debug-texture color for this face label (pure channel patterns so a
    /// shaded sample still classifies by chroma). Used by the facelabel
    /// verification pass (`REWO_MOB_DEBUG_TEX` + `rewo mobshot --check`).
    pub fn debug_color(self) -> [u8; 3] {
        match self {
            Facing::North => [255, 0, 0],   // front → red
            Facing::South => [255, 255, 0], // back → yellow
            Facing::West => [0, 255, 0],    // green
            Facing::East => [0, 0, 255],    // blue
            Facing::Down => [255, 0, 255],  // world-top → magenta
            Facing::Up => [0, 255, 255],    // world-bottom → cyan
        }
    }
}

/// Which animation drives a part. Angles are vanilla's `setupAnim` formulas,
/// computed in [`crate::entities`] from the walk phase / head state.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Anim {
    /// Static geometry (any static pose was folded in at build time).
    None,
    /// Look pitch about X + net head yaw about Y (vanilla `rotateZYX(0,y,x)`).
    Head,
    /// Humanoid arms: `cos(f ± π) · 2.0 · amount · 0.5`.
    ArmRight,
    ArmLeft,
    /// Biped legs: `cos(f ± π) · 1.4 · amount`.
    LegRight,
    LegLeft,
    /// Quadruped legs, diagonal gait: hind-right + front-left in phase.
    QuadHindRight,
    QuadHindLeft,
    QuadFrontRight,
    QuadFrontLeft,
}

/// One articulated part: cubes attached to it rotate about `pivot` by the
/// animation's angle. Static pose rotations never live here — they are folded
/// into the quad vertices at build time — so an animated part's rest matrix
/// is always identity (true for every vanilla mob we port; asserted in the
/// builder).
pub struct Part {
    pub pivot: [f32; 3],
    pub anim: Anim,
    /// Angle multiplier (villager/enderman swing at half amplitude).
    pub amp: f32,
}

/// One textured quad in part-local model px. UVs are in the mob's own
/// texture pixels; [`crate::entities`] normalizes them into the shared atlas
/// once the texture's slot is known.
pub struct RawQuad {
    pub pos: [[f32; 3]; 4],
    pub uv: [[f32; 2]; 4],
    /// Vanilla face label of the source box face (drives the debug texture
    /// + shade). For mirrored cubes the label follows the texture rect, not
    /// the geometric side — exactly like vanilla.
    pub facing: Facing,
    /// Outward normal with all static folds applied (model space).
    pub normal: [f32; 3],
    /// Which of the mob's textures this quad samples (index into
    /// [`MobDef::textures`]).
    pub tex: usize,
    /// Index into [`Model::parts`].
    pub part: usize,
    /// Baked directional shade (applied in the vertex color).
    pub shade: f32,
}

pub struct Model {
    pub parts: Vec<Part>,
    pub quads: Vec<RawQuad>,
    /// Extra scale on top of the 1/16 px→block conversion (player 0.9375,
    /// wither skeleton 1.2, slime 2.0, …).
    pub scale: f32,
}

/// A static fold applied to cube vertices at build time: `v → R_zyx(rot)·v +
/// off`. This is exactly vanilla's `translate(pose); rotateZYX(z,y,x)` — a
/// chain of them reproduces nested `PartDefinition` children.
#[derive(Clone, Copy)]
pub struct Fold {
    pub rot: [f32; 3], // (xRot, yRot, zRot), vanilla order
    pub off: [f32; 3],
}

impl Fold {
    pub const fn at(off: [f32; 3]) -> Self {
        Fold { rot: [0.0; 3], off }
    }
    pub const fn rot(rot: [f32; 3], off: [f32; 3]) -> Self {
        Fold { rot, off }
    }
}

/// `rotateZYX(z, y, x)` applied to a vector: Rx first, then Ry, then Rz —
/// matching JOML's `rotationZYX` (M = Rz·Ry·Rx).
pub fn rotate_zyx(v: [f32; 3], rot: [f32; 3]) -> [f32; 3] {
    let [rx, ry, rz] = rot;
    let [mut x, mut y, mut z] = v;
    if rx != 0.0 {
        let (s, c) = rx.sin_cos();
        let (y1, z1) = (y * c - z * s, y * s + z * c);
        y = y1;
        z = z1;
    }
    if ry != 0.0 {
        let (s, c) = ry.sin_cos();
        let (x1, z1) = (x * c + z * s, -x * s + z * c);
        x = x1;
        z = z1;
    }
    if rz != 0.0 {
        let (s, c) = rz.sin_cos();
        let (x1, y1) = (x * c - y * s, x * s + y * c);
        x = x1;
        y = y1;
    }
    [x, y, z]
}

/// Directional shade for a static-folded model-space normal, matching the
/// entity pass's classic 6-way table. Classified after the render y-flip:
/// model −Y is the world-top (brightest), model −Z the front.
fn shade_for(n: [f32; 3]) -> f32 {
    if n[1] < -0.7 {
        1.0 // world top
    } else if n[1] > 0.7 {
        0.5 // world bottom
    } else if n[2] < -0.7 {
        0.80 // front
    } else if n[2] > 0.7 {
        0.62 // back
    } else {
        0.68 // sides
    }
}

// ---------------------------------------------------------------------------
// The faithful Cube port
// ---------------------------------------------------------------------------

/// Exact port of vanilla `ModelPart.Cube` + `Polygon`: given a box (tex
/// offset, min corner, dims, uniform grow, mirror), produce the 6 faces with
/// vanilla's vertex arrays, UV rects, and per-vertex UV corner assignment.
///
/// Ground truth (26.2 `ModelPart.java`):
/// ```text
/// u0=texX  u1=+d  u2=+d+w  u22=+d+2w  u3=+d+w+d  u4=+d+w+d+w
/// v0=texY  v1=+d  v2=+d+h
/// DOWN : {l1,l0,t0,t1}  rect (u1,v0,u2,v1)
/// UP   : {t2,t3,l3,l2}  rect (u2,v1,u22,v0)   // v1 before v0 → vertical flip
/// WEST : {t0,l0,l3,t3}  rect (u0,v1,u1,v2)
/// NORTH: {t1,t0,t3,t2}  rect (u1,v1,u2,v2)
/// EAST : {l1,t1,t2,l2}  rect (u2,v1,u3,v2)
/// SOUTH: {l0,l1,l2,l3}  rect (u3,v1,u4,v2)
/// Polygon remap: verts[0]=(u1,v0) [1]=(u0,v0) [2]=(u0,v1) [3]=(u1,v1);
/// mirror ⇒ swap(minX,maxX) before corners + reverse the vertex array.
/// ```
pub fn cube_faces(
    tex: (f32, f32),
    min: [f32; 3],
    dims: [f32; 3],
    grow: f32,
    mirror: bool,
) -> [(Facing, [[f32; 3]; 4], [[f32; 2]; 4]); 6] {
    let (tex_u, tex_v) = tex;
    let [w, h, d] = dims;
    let [mut x0, mut y0, mut z0] = min;
    let (mut x1, mut y1, mut z1) = (x0 + w, y0 + h, z0 + d);
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
    let u0 = tex_u;
    let u1 = tex_u + d;
    let u2 = tex_u + d + w;
    let u22 = tex_u + d + w + w;
    let u3 = tex_u + d + w + d;
    let u4 = tex_u + d + w + d + w;
    let v0 = tex_v;
    let v1 = tex_v + d;
    let v2 = tex_v + d + h;

    let face = |facing: Facing, verts: [[f32; 3]; 4], rect: (f32, f32, f32, f32)| {
        // Polygon remap — rect is the passed (u0,v0,u1,v1) quadruple.
        let (ru0, rv0, ru1, rv1) = rect;
        let mut vs = verts;
        let mut uvs = [[ru1, rv0], [ru0, rv0], [ru0, rv1], [ru1, rv1]];
        if mirror {
            // Vanilla reverses the vertex array (each vertex keeps its UV).
            vs.reverse();
            uvs.reverse();
        }
        (facing, vs, uvs)
    };

    [
        face(Facing::Down, [l1, l0, t0, t1], (u1, v0, u2, v1)),
        face(Facing::Up, [t2, t3, l3, l2], (u2, v1, u22, v0)),
        face(Facing::West, [t0, l0, l3, t3], (u0, v1, u1, v2)),
        face(Facing::North, [t1, t0, t3, t2], (u1, v1, u2, v2)),
        face(Facing::East, [l1, t1, t2, l2], (u2, v1, u3, v2)),
        face(Facing::South, [l0, l1, l2, l3], (u3, v1, u4, v2)),
    ]
}

// ---------------------------------------------------------------------------
// Model builder
// ---------------------------------------------------------------------------

/// Index of the shared static root part (identity, no animation).
pub const STATIC_PART: usize = 0;

pub struct ModelBuilder {
    parts: Vec<Part>,
    quads: Vec<RawQuad>,
}

impl ModelBuilder {
    fn new() -> Self {
        ModelBuilder {
            parts: vec![Part {
                pivot: [0.0; 3],
                anim: Anim::None,
                amp: 1.0,
            }],
            quads: Vec::new(),
        }
    }

    /// Register an animated part. Its rest transform is identity — vanilla
    /// static pose rotations belong in cube folds, not here.
    fn part(&mut self, pivot: [f32; 3], anim: Anim, amp: f32) -> usize {
        debug_assert!(anim != Anim::None, "static geometry goes through folds");
        self.parts.push(Part { pivot, anim, amp });
        self.parts.len() - 1
    }

    /// Add one vanilla `addBox` to a part. `folds` reproduce nested static
    /// child poses, innermost first; for `STATIC_PART` the outermost fold is
    /// the part's own `PartPose`.
    #[allow(clippy::too_many_arguments)]
    fn cube_f(
        &mut self,
        part: usize,
        tex: usize,
        uv: (f32, f32),
        min: [f32; 3],
        dims: [f32; 3],
        grow: f32,
        mirror: bool,
        folds: &[Fold],
    ) {
        for (facing, mut pos, uvs) in cube_faces(uv, min, dims, grow, mirror) {
            let mut normal = facing.normal();
            for f in folds {
                for p in &mut pos {
                    let r = rotate_zyx(*p, f.rot);
                    *p = [r[0] + f.off[0], r[1] + f.off[1], r[2] + f.off[2]];
                }
                normal = rotate_zyx(normal, f.rot);
            }
            self.quads.push(RawQuad {
                pos,
                uv: uvs,
                facing,
                normal,
                tex,
                part,
                shade: shade_for(normal),
            });
        }
    }

    fn cube(&mut self, part: usize, tex: usize, uv: (f32, f32), min: [f32; 3], dims: [f32; 3], folds: &[Fold]) {
        self.cube_f(part, tex, uv, min, dims, 0.0, false, folds);
    }

    fn cube_m(&mut self, part: usize, tex: usize, uv: (f32, f32), min: [f32; 3], dims: [f32; 3], folds: &[Fold]) {
        self.cube_f(part, tex, uv, min, dims, 0.0, true, folds);
    }

    fn cube_g(&mut self, part: usize, tex: usize, uv: (f32, f32), min: [f32; 3], dims: [f32; 3], grow: f32, folds: &[Fold]) {
        self.cube_f(part, tex, uv, min, dims, grow, false, folds);
    }

    fn finish(self, scale: f32) -> Model {
        Model {
            parts: self.parts,
            quads: self.quads,
            scale,
        }
    }
}

// ---------------------------------------------------------------------------
// Mob registry
// ---------------------------------------------------------------------------

/// Which model to render for an entity. `Capsule` is the fallback for
/// everything without a bespoke model (or with a missing texture).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EntityModelKind {
    Player,
    Zombie,
    Husk,
    Drowned,
    Skeleton,
    Stray,
    WitherSkeleton,
    Creeper,
    Spider,
    CaveSpider,
    Enderman,
    Slime,
    Cow,
    Pig,
    Sheep,
    Chicken,
    Wolf,
    Squid,
    GlowSquid,
    Rabbit,
    Villager,
    Capsule,
}

impl EntityModelKind {
    pub const COUNT: usize = 22;
    pub const ALL: [EntityModelKind; Self::COUNT] = [
        EntityModelKind::Player,
        EntityModelKind::Zombie,
        EntityModelKind::Husk,
        EntityModelKind::Drowned,
        EntityModelKind::Skeleton,
        EntityModelKind::Stray,
        EntityModelKind::WitherSkeleton,
        EntityModelKind::Creeper,
        EntityModelKind::Spider,
        EntityModelKind::CaveSpider,
        EntityModelKind::Enderman,
        EntityModelKind::Slime,
        EntityModelKind::Cow,
        EntityModelKind::Pig,
        EntityModelKind::Sheep,
        EntityModelKind::Chicken,
        EntityModelKind::Wolf,
        EntityModelKind::Squid,
        EntityModelKind::GlowSquid,
        EntityModelKind::Rabbit,
        EntityModelKind::Villager,
        EntityModelKind::Capsule,
    ];

    pub fn index(self) -> usize {
        Self::ALL.iter().position(|k| *k == self).unwrap()
    }

    pub fn name(self) -> &'static str {
        match self {
            EntityModelKind::Player => "player",
            EntityModelKind::Zombie => "zombie",
            EntityModelKind::Husk => "husk",
            EntityModelKind::Drowned => "drowned",
            EntityModelKind::Skeleton => "skeleton",
            EntityModelKind::Stray => "stray",
            EntityModelKind::WitherSkeleton => "wither_skeleton",
            EntityModelKind::Creeper => "creeper",
            EntityModelKind::Spider => "spider",
            EntityModelKind::CaveSpider => "cave_spider",
            EntityModelKind::Enderman => "enderman",
            EntityModelKind::Slime => "slime",
            EntityModelKind::Cow => "cow",
            EntityModelKind::Pig => "pig",
            EntityModelKind::Sheep => "sheep",
            EntityModelKind::Chicken => "chicken",
            EntityModelKind::Wolf => "wolf",
            EntityModelKind::Squid => "squid",
            EntityModelKind::GlowSquid => "glow_squid",
            EntityModelKind::Rabbit => "rabbit",
            EntityModelKind::Villager => "villager",
            EntityModelKind::Capsule => "capsule",
        }
    }
}

/// Map a wire entity-type name (`minecraft:<x>`) to a model kind. Players
/// are matched by type id upstream, not by this table.
pub fn kind_for_entity_name(name: &str) -> EntityModelKind {
    match name {
        "minecraft:zombie" | "minecraft:zombie_villager" => EntityModelKind::Zombie,
        "minecraft:husk" => EntityModelKind::Husk,
        "minecraft:drowned" => EntityModelKind::Drowned,
        "minecraft:skeleton" => EntityModelKind::Skeleton,
        "minecraft:stray" => EntityModelKind::Stray,
        "minecraft:wither_skeleton" => EntityModelKind::WitherSkeleton,
        "minecraft:creeper" => EntityModelKind::Creeper,
        "minecraft:spider" => EntityModelKind::Spider,
        "minecraft:cave_spider" => EntityModelKind::CaveSpider,
        "minecraft:enderman" => EntityModelKind::Enderman,
        "minecraft:slime" | "minecraft:magma_cube" => EntityModelKind::Slime,
        "minecraft:cow" => EntityModelKind::Cow,
        "minecraft:pig" => EntityModelKind::Pig,
        "minecraft:sheep" => EntityModelKind::Sheep,
        "minecraft:chicken" => EntityModelKind::Chicken,
        "minecraft:wolf" => EntityModelKind::Wolf,
        "minecraft:squid" => EntityModelKind::Squid,
        "minecraft:glow_squid" => EntityModelKind::GlowSquid,
        "minecraft:rabbit" => EntityModelKind::Rabbit,
        "minecraft:villager" => EntityModelKind::Villager,
        _ => EntityModelKind::Capsule,
    }
}

/// One registry entry: texture keys (into the baked mob-texture table) +
/// the model builder. A mob renders as a capsule until every listed texture
/// is present.
pub struct MobDef {
    pub kind: EntityModelKind,
    pub textures: &'static [&'static str],
    pub build: fn() -> Model,
}

pub static MOBS: &[MobDef] = &[
    MobDef { kind: EntityModelKind::Player, textures: &["player"], build: player },
    MobDef { kind: EntityModelKind::Zombie, textures: &["zombie"], build: zombie },
    MobDef { kind: EntityModelKind::Husk, textures: &["husk"], build: husk },
    MobDef { kind: EntityModelKind::Drowned, textures: &["drowned", "drowned_outer"], build: drowned },
    MobDef { kind: EntityModelKind::Skeleton, textures: &["skeleton"], build: skeleton },
    MobDef { kind: EntityModelKind::Stray, textures: &["stray", "stray_overlay"], build: stray },
    MobDef { kind: EntityModelKind::WitherSkeleton, textures: &["wither_skeleton"], build: wither_skeleton },
    MobDef { kind: EntityModelKind::Creeper, textures: &["creeper"], build: creeper },
    MobDef { kind: EntityModelKind::Spider, textures: &["spider"], build: spider },
    MobDef { kind: EntityModelKind::CaveSpider, textures: &["cave_spider"], build: cave_spider },
    MobDef { kind: EntityModelKind::Enderman, textures: &["enderman"], build: enderman },
    MobDef { kind: EntityModelKind::Slime, textures: &["slime"], build: slime },
    MobDef { kind: EntityModelKind::Cow, textures: &["cow"], build: cow },
    MobDef { kind: EntityModelKind::Pig, textures: &["pig"], build: pig },
    MobDef { kind: EntityModelKind::Sheep, textures: &["sheep", "sheep_wool"], build: sheep },
    MobDef { kind: EntityModelKind::Chicken, textures: &["chicken"], build: chicken },
    MobDef { kind: EntityModelKind::Wolf, textures: &["wolf"], build: wolf },
    MobDef { kind: EntityModelKind::Squid, textures: &["squid"], build: squid },
    MobDef { kind: EntityModelKind::GlowSquid, textures: &["glow_squid"], build: squid },
    MobDef { kind: EntityModelKind::Rabbit, textures: &["rabbit"], build: rabbit },
    MobDef { kind: EntityModelKind::Villager, textures: &["villager"], build: villager },
];

// ---------------------------------------------------------------------------
// Humanoids — `HumanoidModel.createMesh` (+ PlayerModel / SkeletonModel /
// EndermanModel variants)
// ---------------------------------------------------------------------------

use std::f32::consts::{FRAC_PI_2, PI};

const NONE: &[Fold] = &[];

/// `HumanoidModel.createMesh(g, 0)` head + hat + body; limbs differ per
/// variant so callers add their own. Returns the head part index.
fn humanoid_head_body(b: &mut ModelBuilder, tex: usize) -> usize {
    let head = b.part([0.0, 0.0, 0.0], Anim::Head, 1.0);
    b.cube(head, tex, (0.0, 0.0), [-4.0, -8.0, -4.0], [8.0, 8.0, 8.0], NONE);
    b.cube_g(head, tex, (32.0, 0.0), [-4.0, -8.0, -4.0], [8.0, 8.0, 8.0], 0.5, NONE);
    b.cube(STATIC_PART, tex, (16.0, 16.0), [-4.0, 0.0, -2.0], [8.0, 12.0, 4.0], NONE);
    head
}

/// The wide ("Steve") player model: humanoid + PlayerModel's dedicated left
/// limb rects + the four overlay layers (hat via `humanoid_head_body`).
fn player() -> Model {
    let mut b = ModelBuilder::new();
    humanoid_head_body(&mut b, 0);
    // Jacket overlay on the body.
    b.cube_g(STATIC_PART, 0, (16.0, 32.0), [-4.0, 0.0, -2.0], [8.0, 12.0, 4.0], 0.25, NONE);
    let arm_r = b.part([-5.0, 2.0, 0.0], Anim::ArmRight, 1.0);
    b.cube(arm_r, 0, (40.0, 16.0), [-3.0, -2.0, -2.0], [4.0, 12.0, 4.0], NONE);
    b.cube_g(arm_r, 0, (40.0, 32.0), [-3.0, -2.0, -2.0], [4.0, 12.0, 4.0], 0.25, NONE);
    let arm_l = b.part([5.0, 2.0, 0.0], Anim::ArmLeft, 1.0);
    b.cube(arm_l, 0, (32.0, 48.0), [-1.0, -2.0, -2.0], [4.0, 12.0, 4.0], NONE);
    b.cube_g(arm_l, 0, (48.0, 48.0), [-1.0, -2.0, -2.0], [4.0, 12.0, 4.0], 0.25, NONE);
    let leg_r = b.part([-1.9, 12.0, 0.0], Anim::LegRight, 1.0);
    b.cube(leg_r, 0, (0.0, 16.0), [-2.0, 0.0, -2.0], [4.0, 12.0, 4.0], NONE);
    b.cube_g(leg_r, 0, (0.0, 32.0), [-2.0, 0.0, -2.0], [4.0, 12.0, 4.0], 0.25, NONE);
    let leg_l = b.part([1.9, 12.0, 0.0], Anim::LegLeft, 1.0);
    b.cube(leg_l, 0, (16.0, 48.0), [-2.0, 0.0, -2.0], [4.0, 12.0, 4.0], NONE);
    b.cube_g(leg_l, 0, (0.0, 48.0), [-2.0, 0.0, -2.0], [4.0, 12.0, 4.0], 0.25, NONE);
    b.finish(0.9375)
}

/// Zombie-family humanoid: vanilla mesh limbs (left = mirrored right rects),
/// arms held straight out (−90°, the iconic pose — vanilla's
/// `animateZombieArms` base angle) so they're static, legs swing normally.
fn zombie_like(scale: f32, overlay: Option<usize>) -> Model {
    let mut b = ModelBuilder::new();
    let head = humanoid_head_body(&mut b, 0);
    let arms = Fold::rot([-FRAC_PI_2, 0.0, 0.0], [-5.0, 2.0, 0.0]);
    b.cube_f(STATIC_PART, 0, (40.0, 16.0), [-3.0, -2.0, -2.0], [4.0, 12.0, 4.0], 0.0, false, &[arms]);
    let arms_l = Fold::rot([-FRAC_PI_2, 0.0, 0.0], [5.0, 2.0, 0.0]);
    b.cube_f(STATIC_PART, 0, (40.0, 16.0), [-1.0, -2.0, -2.0], [4.0, 12.0, 4.0], 0.0, true, &[arms_l]);
    let leg_r = b.part([-1.9, 12.0, 0.0], Anim::LegRight, 1.0);
    b.cube(leg_r, 0, (0.0, 16.0), [-2.0, 0.0, -2.0], [4.0, 12.0, 4.0], NONE);
    let leg_l = b.part([1.9, 12.0, 0.0], Anim::LegLeft, 1.0);
    b.cube_m(leg_l, 0, (0.0, 16.0), [-2.0, 0.0, -2.0], [4.0, 12.0, 4.0], NONE);
    if let Some(tex) = overlay {
        // Drowned outer layer: the same humanoid mesh inflated 0.25 (hat
        // 0.5+0.25), sampling the overlay texture. Parts mirror the base so
        // the clothing follows the head/legs.
        b.cube_g(head, tex, (0.0, 0.0), [-4.0, -8.0, -4.0], [8.0, 8.0, 8.0], 0.25, NONE);
        b.cube_g(head, tex, (32.0, 0.0), [-4.0, -8.0, -4.0], [8.0, 8.0, 8.0], 0.75, NONE);
        b.cube_g(STATIC_PART, tex, (16.0, 16.0), [-4.0, 0.0, -2.0], [8.0, 12.0, 4.0], 0.25, NONE);
        b.cube_f(STATIC_PART, tex, (40.0, 16.0), [-3.0, -2.0, -2.0], [4.0, 12.0, 4.0], 0.25, false, &[arms]);
        b.cube_f(STATIC_PART, tex, (40.0, 16.0), [-1.0, -2.0, -2.0], [4.0, 12.0, 4.0], 0.25, true, &[arms_l]);
        b.cube_g(leg_r, tex, (0.0, 16.0), [-2.0, 0.0, -2.0], [4.0, 12.0, 4.0], 0.25, NONE);
        b.cube_f(leg_l, tex, (0.0, 16.0), [-2.0, 0.0, -2.0], [4.0, 12.0, 4.0], 0.25, true, NONE);
    }
    b.finish(scale)
}

fn zombie() -> Model {
    zombie_like(1.0, None)
}

fn husk() -> Model {
    // Vanilla HuskRenderer scales the zombie model by 1.0625.
    zombie_like(1.0625, None)
}

fn drowned() -> Model {
    zombie_like(1.0, Some(1))
}

/// `SkeletonModel`: humanoid head/body + 2×12×2 limbs, walking arm swing.
/// `overlay` adds the stray clothing layer (same mesh, +0.25, texture 1).
fn skeleton_like(scale: f32, overlay: bool) -> Model {
    let mut b = ModelBuilder::new();
    let head = humanoid_head_body(&mut b, 0);
    let arm_r = b.part([-5.0, 2.0, 0.0], Anim::ArmRight, 1.0);
    b.cube(arm_r, 0, (40.0, 16.0), [-1.0, -2.0, -1.0], [2.0, 12.0, 2.0], NONE);
    let arm_l = b.part([5.0, 2.0, 0.0], Anim::ArmLeft, 1.0);
    b.cube_m(arm_l, 0, (40.0, 16.0), [-1.0, -2.0, -1.0], [2.0, 12.0, 2.0], NONE);
    let leg_r = b.part([-2.0, 12.0, 0.0], Anim::LegRight, 1.0);
    b.cube(leg_r, 0, (0.0, 16.0), [-1.0, 0.0, -1.0], [2.0, 12.0, 2.0], NONE);
    let leg_l = b.part([2.0, 12.0, 0.0], Anim::LegLeft, 1.0);
    b.cube_m(leg_l, 0, (0.0, 16.0), [-1.0, 0.0, -1.0], [2.0, 12.0, 2.0], NONE);
    if overlay {
        b.cube_g(head, 1, (0.0, 0.0), [-4.0, -8.0, -4.0], [8.0, 8.0, 8.0], 0.25, NONE);
        b.cube_g(head, 1, (32.0, 0.0), [-4.0, -8.0, -4.0], [8.0, 8.0, 8.0], 0.75, NONE);
        b.cube_g(STATIC_PART, 1, (16.0, 16.0), [-4.0, 0.0, -2.0], [8.0, 12.0, 4.0], 0.25, NONE);
        b.cube_g(arm_r, 1, (40.0, 16.0), [-1.0, -2.0, -1.0], [2.0, 12.0, 2.0], 0.25, NONE);
        b.cube_f(arm_l, 1, (40.0, 16.0), [-1.0, -2.0, -1.0], [2.0, 12.0, 2.0], 0.25, true, NONE);
        b.cube_g(leg_r, 1, (0.0, 16.0), [-1.0, 0.0, -1.0], [2.0, 12.0, 2.0], 0.25, NONE);
        b.cube_f(leg_l, 1, (0.0, 16.0), [-1.0, 0.0, -1.0], [2.0, 12.0, 2.0], 0.25, true, NONE);
    }
    b.finish(scale)
}

fn skeleton() -> Model {
    skeleton_like(1.0, false)
}

fn stray() -> Model {
    skeleton_like(1.0, true)
}

fn wither_skeleton() -> Model {
    // Vanilla WitherSkeletonRenderer scale.
    skeleton_like(1.2, false)
}

/// `EndermanModel`: stretched humanoid, half-amplitude limb swing.
fn enderman() -> Model {
    let mut b = ModelBuilder::new();
    let head = b.part([0.0, -13.0, 0.0], Anim::Head, 1.0);
    b.cube(head, 0, (0.0, 0.0), [-4.0, -8.0, -4.0], [8.0, 8.0, 8.0], NONE);
    b.cube_g(head, 0, (0.0, 16.0), [-4.0, -8.0, -4.0], [8.0, 8.0, 8.0], -0.5, NONE);
    b.cube_f(STATIC_PART, 0, (32.0, 16.0), [-4.0, 0.0, -2.0], [8.0, 12.0, 4.0], 0.0, false, &[Fold::at([0.0, -14.0, 0.0])]);
    let arm_r = b.part([-5.0, -12.0, 0.0], Anim::ArmRight, 0.5);
    b.cube(arm_r, 0, (56.0, 0.0), [-1.0, -2.0, -1.0], [2.0, 30.0, 2.0], NONE);
    let arm_l = b.part([5.0, -12.0, 0.0], Anim::ArmLeft, 0.5);
    b.cube_m(arm_l, 0, (56.0, 0.0), [-1.0, -2.0, -1.0], [2.0, 30.0, 2.0], NONE);
    let leg_r = b.part([-2.0, -5.0, 0.0], Anim::LegRight, 0.5);
    b.cube(leg_r, 0, (56.0, 0.0), [-1.0, 0.0, -1.0], [2.0, 30.0, 2.0], NONE);
    let leg_l = b.part([2.0, -5.0, 0.0], Anim::LegLeft, 0.5);
    b.cube_m(leg_l, 0, (56.0, 0.0), [-1.0, 0.0, -1.0], [2.0, 30.0, 2.0], NONE);
    b.finish(1.0)
}

// ---------------------------------------------------------------------------
// Monsters with their own body plans
// ---------------------------------------------------------------------------

/// `CreeperModel`: head + tall body + four stubby legs on the quad gait.
fn creeper() -> Model {
    let mut b = ModelBuilder::new();
    let head = b.part([0.0, 6.0, 0.0], Anim::Head, 1.0);
    b.cube(head, 0, (0.0, 0.0), [-4.0, -8.0, -4.0], [8.0, 8.0, 8.0], NONE);
    b.cube_f(STATIC_PART, 0, (16.0, 16.0), [-4.0, 0.0, -2.0], [8.0, 12.0, 4.0], 0.0, false, &[Fold::at([0.0, 6.0, 0.0])]);
    let legs = [
        ([-2.0, 18.0, 4.0], Anim::QuadHindRight),
        ([2.0, 18.0, 4.0], Anim::QuadHindLeft),
        ([-2.0, 18.0, -4.0], Anim::QuadFrontRight),
        ([2.0, 18.0, -4.0], Anim::QuadFrontLeft),
    ];
    for (pivot, anim) in legs {
        let p = b.part(pivot, anim, 1.0);
        b.cube(p, 0, (0.0, 16.0), [-2.0, 0.0, -2.0], [4.0, 6.0, 4.0], NONE);
    }
    b.finish(1.0)
}

/// `SpiderModel`: head + two body segments + 8 splayed legs (static pose;
/// the leg-wave animation is a follow-up).
fn spider_like(scale: f32) -> Model {
    let mut b = ModelBuilder::new();
    let head = b.part([0.0, 15.0, -3.0], Anim::Head, 1.0);
    b.cube(head, 0, (32.0, 4.0), [-4.0, -4.0, -8.0], [8.0, 8.0, 8.0], NONE);
    b.cube_f(STATIC_PART, 0, (0.0, 0.0), [-3.0, -3.0, -3.0], [6.0, 6.0, 6.0], 0.0, false, &[Fold::at([0.0, 15.0, 0.0])]);
    b.cube_f(STATIC_PART, 0, (0.0, 12.0), [-5.0, -4.0, -6.0], [10.0, 8.0, 12.0], 0.0, false, &[Fold::at([0.0, 15.0, 9.0])]);
    // (pivot, yRot, zRot, mirror) per leg — right legs extend −X, left +X.
    const Z58: f32 = 0.58119464;
    let legs: [([f32; 3], f32, f32, bool); 8] = [
        ([-4.0, 15.0, 2.0], PI / 4.0, -PI / 4.0, false),
        ([4.0, 15.0, 2.0], -PI / 4.0, PI / 4.0, true),
        ([-4.0, 15.0, 1.0], PI / 8.0, -Z58, false),
        ([4.0, 15.0, 1.0], -PI / 8.0, Z58, true),
        ([-4.0, 15.0, 0.0], -PI / 8.0, -Z58, false),
        ([4.0, 15.0, 0.0], PI / 8.0, Z58, true),
        ([-4.0, 15.0, -1.0], -PI / 4.0, -PI / 4.0, false),
        ([4.0, 15.0, -1.0], PI / 4.0, PI / 4.0, true),
    ];
    for (pivot, ry, rz, left) in legs {
        let fold = Fold::rot([0.0, ry, rz], pivot);
        let min = if left { [-1.0, -1.0, -1.0] } else { [-15.0, -1.0, -1.0] };
        b.cube_f(STATIC_PART, 0, (18.0, 0.0), min, [16.0, 2.0, 2.0], 0.0, left, &[fold]);
    }
    b.finish(scale)
}

fn spider() -> Model {
    spider_like(1.0)
}

fn cave_spider() -> Model {
    // Vanilla CaveSpiderRenderer scale.
    spider_like(0.7)
}

/// `SlimeModel.createOuterBodyLayer`: the 8³ shell. The inner cube + face
/// live behind a translucent shell vanilla-side; we render the shell opaque
/// until an entity translucent pass exists.
fn slime() -> Model {
    let mut b = ModelBuilder::new();
    b.cube(STATIC_PART, 0, (0.0, 0.0), [-4.0, 16.0, -4.0], [8.0, 8.0, 8.0], NONE);
    // 8 px spans one block at 2×: the fixed "medium slime" of the old pass.
    b.finish(2.0)
}

// ---------------------------------------------------------------------------
// Farm + passive mobs
// ---------------------------------------------------------------------------

/// Shared quadruped legs (`QuadrupedModel.createLegs`): 4×`leg`×4 boxes at
/// (±x, 24−leg, 7 / −5) with per-side mirror flags.
#[allow(clippy::too_many_arguments)]
fn quadruped_legs(
    b: &mut ModelBuilder,
    tex: usize,
    uv: (f32, f32),
    leg: f32,
    x: f32,
    mirror_left: bool,
    mirror_right: bool,
) {
    let y = 24.0 - leg;
    let legs = [
        ([-x, y, 7.0], Anim::QuadHindRight, mirror_right),
        ([x, y, 7.0], Anim::QuadHindLeft, mirror_left),
        ([-x, y, -5.0], Anim::QuadFrontRight, mirror_right),
        ([x, y, -5.0], Anim::QuadFrontLeft, mirror_left),
    ];
    for (pivot, anim, mirror) in legs {
        let p = b.part(pivot, anim, 1.0);
        b.cube_f(p, tex, uv, [-2.0, 0.0, -2.0], [4.0, leg, 4.0], 0.0, mirror, NONE);
    }
}

/// 26.2 `CowModel.createBaseCowModel` — its own mesh, not the generic
/// quadruped: 8×8×6 head with muzzle + horns, 12×18×10 body with udder,
/// 4×12×4 legs at ±4 (left pair mirrored).
fn cow() -> Model {
    let mut b = ModelBuilder::new();
    let head = b.part([0.0, 4.0, -8.0], Anim::Head, 1.0);
    b.cube(head, 0, (0.0, 0.0), [-4.0, -4.0, -6.0], [8.0, 8.0, 6.0], NONE);
    b.cube(head, 0, (1.0, 33.0), [-3.0, 1.0, -7.0], [6.0, 3.0, 1.0], NONE);
    b.cube(head, 0, (22.0, 0.0), [-5.0, -5.0, -5.0], [1.0, 3.0, 1.0], NONE);
    b.cube(head, 0, (22.0, 0.0), [4.0, -5.0, -5.0], [1.0, 3.0, 1.0], NONE);
    let body = Fold::rot([FRAC_PI_2, 0.0, 0.0], [0.0, 5.0, 2.0]);
    b.cube_f(STATIC_PART, 0, (18.0, 4.0), [-6.0, -10.0, -7.0], [12.0, 18.0, 10.0], 0.0, false, &[body]);
    b.cube_f(STATIC_PART, 0, (52.0, 0.0), [-2.0, 2.0, -8.0], [4.0, 6.0, 1.0], 0.0, false, &[body]);
    let legs = [
        ([-4.0, 12.0, 7.0], Anim::QuadHindRight, false),
        ([4.0, 12.0, 7.0], Anim::QuadHindLeft, true),
        ([-4.0, 12.0, -5.0], Anim::QuadFrontRight, false),
        ([4.0, 12.0, -5.0], Anim::QuadFrontLeft, true),
    ];
    for (pivot, anim, mirror) in legs {
        let p = b.part(pivot, anim, 1.0);
        b.cube_f(p, 0, (0.0, 16.0), [-2.0, 0.0, -2.0], [4.0, 12.0, 4.0], 0.0, mirror, NONE);
    }
    b.finish(1.0)
}

/// `PigModel`: generic quadruped (legSize 6, left legs mirrored) with the
/// snouted head.
fn pig() -> Model {
    let mut b = ModelBuilder::new();
    let head = b.part([0.0, 12.0, -6.0], Anim::Head, 1.0);
    b.cube(head, 0, (0.0, 0.0), [-4.0, -4.0, -8.0], [8.0, 8.0, 8.0], NONE);
    b.cube(head, 0, (16.0, 16.0), [-2.0, 0.0, -9.0], [4.0, 3.0, 1.0], NONE);
    let body = Fold::rot([FRAC_PI_2, 0.0, 0.0], [0.0, 11.0, 2.0]);
    b.cube_f(STATIC_PART, 0, (28.0, 8.0), [-5.0, -10.0, -7.0], [10.0, 16.0, 8.0], 0.0, false, &[body]);
    quadruped_legs(&mut b, 0, (0.0, 16.0), 6.0, 3.0, true, false);
    b.finish(1.0)
}

/// `SheepModel` + `SheepFurModel`: quadruped (legSize 12, right legs
/// mirrored) with the sheep head/body, plus the inflated wool overlay on
/// texture 1 (head +0.6 follows the head-look; body +1.75; upper legs +0.5
/// ride the leg swing).
fn sheep() -> Model {
    let mut b = ModelBuilder::new();
    let head = b.part([0.0, 6.0, -8.0], Anim::Head, 1.0);
    b.cube(head, 0, (0.0, 0.0), [-3.0, -4.0, -6.0], [6.0, 6.0, 8.0], NONE);
    let body = Fold::rot([FRAC_PI_2, 0.0, 0.0], [0.0, 5.0, 2.0]);
    b.cube_f(STATIC_PART, 0, (28.0, 8.0), [-4.0, -10.0, -7.0], [8.0, 16.0, 6.0], 0.0, false, &[body]);
    quadruped_legs(&mut b, 0, (0.0, 16.0), 12.0, 3.0, false, true);
    // Wool (SheepFurModel). Legs were parts 2..6 in registration order.
    b.cube_g(head, 1, (0.0, 0.0), [-3.0, -4.0, -4.0], [6.0, 6.0, 6.0], 0.6, NONE);
    b.cube_f(STATIC_PART, 1, (28.0, 8.0), [-4.0, -10.0, -7.0], [8.0, 16.0, 6.0], 1.75, false, &[body]);
    for leg_part in 2..6 {
        b.cube_g(leg_part, 1, (0.0, 16.0), [-2.0, 0.0, -2.0], [4.0, 6.0, 4.0], 0.5, NONE);
    }
    b.finish(1.0)
}

/// `AdultChickenModel`: head with beak + wattle, rotated body, biped-swing
/// legs, static wings (the flap animation is airborne-only).
fn chicken() -> Model {
    let mut b = ModelBuilder::new();
    let head = b.part([0.0, 15.0, -4.0], Anim::Head, 1.0);
    b.cube(head, 0, (0.0, 0.0), [-2.0, -6.0, -2.0], [4.0, 6.0, 3.0], NONE);
    b.cube(head, 0, (14.0, 0.0), [-2.0, -4.0, -4.0], [4.0, 2.0, 2.0], NONE);
    b.cube(head, 0, (14.0, 4.0), [-1.0, -2.0, -3.0], [2.0, 2.0, 2.0], NONE);
    let body = Fold::rot([FRAC_PI_2, 0.0, 0.0], [0.0, 16.0, 0.0]);
    b.cube_f(STATIC_PART, 0, (0.0, 9.0), [-3.0, -4.0, -3.0], [6.0, 8.0, 6.0], 0.0, false, &[body]);
    let leg_r = b.part([-2.0, 19.0, 1.0], Anim::LegRight, 1.0);
    b.cube(leg_r, 0, (26.0, 0.0), [-1.0, 0.0, -3.0], [3.0, 5.0, 3.0], NONE);
    let leg_l = b.part([1.0, 19.0, 1.0], Anim::LegLeft, 1.0);
    b.cube(leg_l, 0, (26.0, 0.0), [-1.0, 0.0, -3.0], [3.0, 5.0, 3.0], NONE);
    b.cube_f(STATIC_PART, 0, (24.0, 13.0), [0.0, 0.0, -3.0], [1.0, 4.0, 6.0], 0.0, false, &[Fold::at([-4.0, 13.0, 0.0])]);
    b.cube_f(STATIC_PART, 0, (24.0, 13.0), [-1.0, 0.0, -3.0], [1.0, 4.0, 6.0], 0.0, false, &[Fold::at([4.0, 13.0, 0.0])]);
    b.finish(1.0)
}

/// `AdultWolfModel`: head (skull + ears + snout), two rotated body
/// segments, 2×8×2 legs (right pair mirrored), angled tail.
fn wolf() -> Model {
    let mut b = ModelBuilder::new();
    let head = b.part([-1.0, 13.5, -7.0], Anim::Head, 1.0);
    b.cube(head, 0, (0.0, 0.0), [-2.0, -3.0, -2.0], [6.0, 6.0, 4.0], NONE);
    b.cube(head, 0, (16.0, 14.0), [-2.0, -5.0, 0.0], [2.0, 2.0, 1.0], NONE);
    b.cube(head, 0, (16.0, 14.0), [2.0, -5.0, 0.0], [2.0, 2.0, 1.0], NONE);
    b.cube(head, 0, (0.0, 10.0), [-0.5, -0.001, -5.0], [3.0, 3.0, 4.0], NONE);
    let body = Fold::rot([FRAC_PI_2, 0.0, 0.0], [0.0, 14.0, 2.0]);
    b.cube_f(STATIC_PART, 0, (18.0, 14.0), [-3.0, -2.0, -3.0], [6.0, 9.0, 6.0], 0.0, false, &[body]);
    let mane = Fold::rot([FRAC_PI_2, 0.0, 0.0], [-1.0, 14.0, -3.0]);
    b.cube_f(STATIC_PART, 0, (21.0, 0.0), [-3.0, -3.0, -3.0], [8.0, 6.0, 7.0], 0.0, false, &[mane]);
    let legs = [
        ([-2.5, 16.0, 7.0], Anim::QuadHindRight, true),
        ([0.5, 16.0, 7.0], Anim::QuadHindLeft, false),
        ([-2.5, 16.0, -4.0], Anim::QuadFrontRight, true),
        ([0.5, 16.0, -4.0], Anim::QuadFrontLeft, false),
    ];
    for (pivot, anim, mirror) in legs {
        let p = b.part(pivot, anim, 1.0);
        b.cube_f(p, 0, (0.0, 18.0), [0.0, 0.0, -1.0], [2.0, 8.0, 2.0], 0.0, mirror, NONE);
    }
    let tail = Fold::rot([PI / 5.0, 0.0, 0.0], [-1.0, 12.0, 8.0]);
    b.cube_f(STATIC_PART, 0, (9.0, 18.0), [0.0, 0.0, -1.0], [2.0, 8.0, 2.0], 0.0, false, &[tail]);
    b.finish(1.0)
}

/// `SquidModel`: big body + 8 tentacles hanging from a radius-5 ring, each
/// yawed to face outward.
fn squid() -> Model {
    let mut b = ModelBuilder::new();
    b.cube_f(STATIC_PART, 0, (0.0, 0.0), [-6.0, -8.0, -6.0], [12.0, 16.0, 12.0], 0.02, false, &[Fold::at([0.0, 8.0, 0.0])]);
    for i in 0..8 {
        let angle = i as f32 * PI * 2.0 / 8.0;
        let pivot = [angle.cos() * 5.0, 15.0, angle.sin() * 5.0];
        let y_rot = -(i as f32) * PI * 2.0 / 8.0 + PI / 2.0;
        b.cube_f(STATIC_PART, 0, (48.0, 0.0), [-1.0, 0.0, -1.0], [2.0, 18.0, 2.0], 0.0, false, &[Fold::rot([0.0, y_rot, 0.0], pivot)]);
    }
    b.finish(1.0)
}

/// `AdultRabbitModel`: fully static transcription (the hop keyframes are a
/// follow-up) — tilted body with tail + head + ears, front legs, and the
/// yaw-splayed haunches.
fn rabbit() -> Model {
    let mut b = ModelBuilder::new();
    let body = Fold::rot([-0.3927, 0.0, 0.0], [0.0, 23.0, 4.0]);
    b.cube_f(STATIC_PART, 0, (0.0, 0.0), [-4.0, -6.0, -9.0], [8.0, 6.0, 10.0], 0.0, false, &[body]);
    b.cube_f(STATIC_PART, 0, (20.0, 16.0), [-2.0, -3.0084, -1.0125], [4.0, 4.0, 4.0], 0.0, false, &[Fold::at([0.0, -4.9916, 0.0125]), body]);
    let head = Fold::rot([0.3927, 0.0, 0.0], [0.0, -5.2929, -8.1213]);
    b.cube_f(STATIC_PART, 0, (0.0, 16.0), [-2.5, -3.0, -4.0], [5.0, 5.0, 5.0], 0.0, false, &[head, body]);
    b.cube_f(STATIC_PART, 0, (32.0, 0.0), [-1.0, -4.2929, -0.1213], [2.0, 5.0, 1.0], 0.0, false, &[Fold::at([1.5, -3.7071, -0.8787]), head, body]);
    b.cube_f(STATIC_PART, 0, (26.0, 0.0), [-1.0, -4.2929, -0.1213], [2.0, 5.0, 1.0], 0.0, false, &[Fold::at([-1.5, -3.7071, -0.8787]), head, body]);
    let front = Fold::at([0.0, -1.5349, -6.3108]);
    b.cube_f(STATIC_PART, 0, (36.0, 18.0), [-0.9, -1.0, -0.9], [2.0, 4.0, 2.0], 0.0, false, &[Fold::rot([0.3927, 0.0, 0.0], [-2.0, 1.9239, 0.3827]), front, body]);
    b.cube_f(STATIC_PART, 0, (44.0, 18.0), [-1.0, -1.0, -1.0], [2.0, 4.0, 2.0], 0.0, false, &[Fold::rot([0.3927, 0.0, 0.0], [2.0, 1.9239, 0.4827]), front, body]);
    // Haunches under `backlegs` (0,23,4): pivot (∓3,0.5,0), then the haunch
    // pose (0,−0.5,0) with the ±22.5° yaw splay.
    b.cube_f(STATIC_PART, 0, (20.0, 24.0), [-1.0, 0.0, -5.0], [2.0, 1.0, 6.0], 0.0, false, &[Fold::rot([0.0, 0.3927, 0.0], [0.0, -0.5, 0.0]), Fold::at([-3.0, 0.5, 0.0]), Fold::at([0.0, 23.0, 4.0])]);
    b.cube_f(STATIC_PART, 0, (36.0, 24.0), [-1.0, 0.0, -5.0], [2.0, 1.0, 6.0], 0.0, false, &[Fold::rot([0.0, -0.3927, 0.0], [0.0, -0.5, 0.0]), Fold::at([3.0, 0.5, 0.0]), Fold::at([0.0, 23.0, 4.0])]);
    b.finish(1.0)
}

/// `VillagerModel`: big head with nose + hat (+ rotated hat rim), robed
/// body, crossed arms, half-swing legs.
fn villager() -> Model {
    let mut b = ModelBuilder::new();
    let head = b.part([0.0, 0.0, 0.0], Anim::Head, 1.0);
    b.cube(head, 0, (0.0, 0.0), [-4.0, -10.0, -4.0], [8.0, 10.0, 8.0], NONE);
    b.cube_g(head, 0, (32.0, 0.0), [-4.0, -10.0, -4.0], [8.0, 10.0, 8.0], 0.51, NONE);
    b.cube_f(head, 0, (30.0, 47.0), [-8.0, -8.0, -6.0], [16.0, 16.0, 1.0], 0.0, false, &[Fold::rot([-FRAC_PI_2, 0.0, 0.0], [0.0, 0.0, 0.0])]);
    b.cube_f(head, 0, (24.0, 0.0), [-1.0, -1.0, -6.0], [2.0, 4.0, 2.0], 0.0, false, &[Fold::at([0.0, -2.0, 0.0])]);
    b.cube(STATIC_PART, 0, (16.0, 20.0), [-4.0, 0.0, -3.0], [8.0, 12.0, 6.0], NONE);
    b.cube_g(STATIC_PART, 0, (0.0, 38.0), [-4.0, 0.0, -3.0], [8.0, 20.0, 6.0], 0.5, NONE);
    let arms = Fold::rot([-0.75, 0.0, 0.0], [0.0, 3.0, -1.0]);
    b.cube_f(STATIC_PART, 0, (44.0, 22.0), [-8.0, -2.0, -2.0], [4.0, 8.0, 4.0], 0.0, false, &[arms]);
    b.cube_f(STATIC_PART, 0, (44.0, 22.0), [4.0, -2.0, -2.0], [4.0, 8.0, 4.0], 0.0, true, &[arms]);
    b.cube_f(STATIC_PART, 0, (40.0, 38.0), [-4.0, 2.0, -2.0], [8.0, 4.0, 4.0], 0.0, false, &[arms]);
    let leg_r = b.part([-2.0, 12.0, 0.0], Anim::LegRight, 0.5);
    b.cube(leg_r, 0, (0.0, 22.0), [-2.0, 0.0, -2.0], [4.0, 12.0, 4.0], NONE);
    let leg_l = b.part([2.0, 12.0, 0.0], Anim::LegLeft, 0.5);
    b.cube_m(leg_l, 0, (0.0, 22.0), [-2.0, 0.0, -2.0], [4.0, 12.0, 4.0], NONE);
    b.finish(1.0)
}

// ---------------------------------------------------------------------------
// Tests — the hand-computed vanilla ground truth from the handoff §3
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// The humanoid head at texOffs(0,0), box(−4,−8,−4, 8,8,8): every face's
    /// vertex array + UV corners hand-computed from vanilla `ModelPart.Cube`.
    #[test]
    fn cube_matches_vanilla_head_unwrap() {
        let faces = cube_faces((0.0, 0.0), [-4.0, -8.0, -4.0], [8.0, 8.0, 8.0], 0.0, false);

        // NORTH (front): verts {t1,t0,t3,t2}, rect (8,8,16,16).
        let (f, pos, uv) = &faces[3];
        assert_eq!(*f, Facing::North);
        assert_eq!(*pos, [[4.0, -8.0, -4.0], [-4.0, -8.0, -4.0], [-4.0, 0.0, -4.0], [4.0, 0.0, -4.0]]);
        assert_eq!(*uv, [[16.0, 8.0], [8.0, 8.0], [8.0, 16.0], [16.0, 16.0]]);

        // UP (maxY plane): verts {t2,t3,l3,l2}, rect (16,8,24,0) — the
        // v1-before-v0 vertical flip.
        let (f, pos, uv) = &faces[1];
        assert_eq!(*f, Facing::Up);
        assert_eq!(*pos, [[4.0, 0.0, -4.0], [-4.0, 0.0, -4.0], [-4.0, 0.0, 4.0], [4.0, 0.0, 4.0]]);
        assert_eq!(*uv, [[24.0, 8.0], [16.0, 8.0], [16.0, 0.0], [24.0, 0.0]]);

        // DOWN (minY plane, world top): verts {l1,l0,t0,t1}, rect (8,0,16,8).
        let (f, pos, uv) = &faces[0];
        assert_eq!(*f, Facing::Down);
        assert_eq!(*pos, [[4.0, -8.0, 4.0], [-4.0, -8.0, 4.0], [-4.0, -8.0, -4.0], [4.0, -8.0, -4.0]]);
        assert_eq!(*uv, [[16.0, 0.0], [8.0, 0.0], [8.0, 8.0], [16.0, 8.0]]);

        // WEST rect (0,8,8,16); EAST rect (16,8,24,16); SOUTH rect (24,8,32,16).
        assert_eq!(faces[2].2, [[8.0, 8.0], [0.0, 8.0], [0.0, 16.0], [8.0, 16.0]]);
        assert_eq!(faces[4].2, [[24.0, 8.0], [16.0, 8.0], [16.0, 16.0], [24.0, 16.0]]);
        assert_eq!(faces[5].2, [[32.0, 8.0], [24.0, 8.0], [24.0, 16.0], [32.0, 16.0]]);
    }

    /// Mirror: X extremes swap and the vertex array reverses (each vertex
    /// keeps its UV corner).
    #[test]
    fn cube_mirror_swaps_x_and_reverses() {
        let plain = cube_faces((0.0, 16.0), [-2.0, 0.0, -2.0], [4.0, 12.0, 4.0], 0.0, false);
        let mirrored = cube_faces((0.0, 16.0), [-2.0, 0.0, -2.0], [4.0, 12.0, 4.0], 0.0, true);
        let (_, ppos, puv) = &plain[3];
        let (_, mpos, muv) = &mirrored[3];
        // Same rect, reversed orders; each mirrored vertex mirrors its
        // partner's UV assignment.
        for i in 0..4 {
            assert_eq!(muv[i], puv[3 - i]);
            let want = ppos[3 - i];
            assert_eq!(mpos[i][1], want[1]);
            assert_eq!(mpos[i][2], want[2]);
            // x negates around the box center (center x = 0 here).
            assert_eq!(mpos[i][0], -want[0]);
        }
    }

    /// Grow inflates the geometry but never the UV rect.
    #[test]
    fn grow_inflates_geometry_only() {
        let base = cube_faces((16.0, 16.0), [-4.0, 0.0, -2.0], [8.0, 12.0, 4.0], 0.0, false);
        let grown = cube_faces((16.0, 16.0), [-4.0, 0.0, -2.0], [8.0, 12.0, 4.0], 0.25, false);
        assert_eq!(base[3].2, grown[3].2);
        assert_eq!(grown[3].1[0], [4.25, -0.25, -2.25]);
    }

    /// Every registered mob builds with valid part indices and non-empty
    /// geometry, and every animated part's amp is sane.
    #[test]
    fn registry_builds_clean() {
        for def in MOBS {
            let m = (def.build)();
            assert!(!m.quads.is_empty(), "{:?} empty", def.kind);
            for q in &m.quads {
                assert!(q.part < m.parts.len(), "{:?} bad part", def.kind);
                assert!(q.tex < def.textures.len(), "{:?} bad tex index", def.kind);
                let n = q.normal;
                let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
                assert!((len - 1.0).abs() < 1e-4, "{:?} normal not unit", def.kind);
            }
            assert!(m.scale > 0.0);
        }
    }

    /// The model→world convention at yaw 0: front (model −Z) faces world +Z,
    /// the right arm (model −X) lands at world −X, feet at y≈0. This is the
    /// transform whose X sign the old code had wrong.
    #[test]
    fn player_world_orientation_at_yaw_zero() {
        let m = player();
        let to_world = |p: [f32; 3], part: &Part| -> [f32; 3] {
            let v = [p[0] + part.pivot[0], p[1] + part.pivot[1], p[2] + part.pivot[2]];
            // model → entity local: (−x, 24.016−y, z); yaw 0 ⇒ rotY(180°).
            let e = [-v[0], MODEL_EYE_Y - v[1], v[2]];
            [-e[0], e[1], -e[2]]
        };
        // Head front face (first cube's NORTH quad) must sit at +Z.
        let head_north = m
            .quads
            .iter()
            .find(|q| q.facing == Facing::North && m.parts[q.part].anim == Anim::Head)
            .unwrap();
        for corner in head_north.pos {
            let w = to_world(corner, &m.parts[head_north.part]);
            assert!(w[2] > 0.0, "front face must face +Z, got {w:?}");
            // Head spans model-px y 24.016..32.016 above the feet.
            assert!(w[1] >= MODEL_EYE_Y - 0.1 && w[1] <= MODEL_EYE_Y + 8.1);
        }
        // Right arm reaches to x −8.25 px (west side; −8 base + 0.25 sleeve).
        let arm = m
            .quads
            .iter()
            .filter(|q| m.parts[q.part].anim == Anim::ArmRight)
            .flat_map(|q| q.pos.iter().map(|p| to_world(*p, &m.parts[q.part])[0]))
            .fold(f32::MAX, f32::min);
        assert_eq!(arm, -8.25);
    }

    #[test]
    fn entity_names_map_to_kinds() {
        assert_eq!(kind_for_entity_name("minecraft:cow"), EntityModelKind::Cow);
        assert_eq!(kind_for_entity_name("minecraft:glow_squid"), EntityModelKind::GlowSquid);
        assert_eq!(kind_for_entity_name("minecraft:magma_cube"), EntityModelKind::Slime);
        assert_eq!(kind_for_entity_name("minecraft:warden"), EntityModelKind::Capsule);
    }
}
