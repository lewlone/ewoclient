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
    ///
    /// Zero-area faces of plate boxes (fins/wings authored with a 0 dim)
    /// are skipped — they rasterize nothing, and their UV rects are
    /// degenerate (vanilla even gives some negative offsets there).
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
        let [w, h, d] = dims;
        for (facing, mut pos, uvs) in cube_faces(uv, min, dims, grow, mirror) {
            let area = match facing {
                Facing::Down | Facing::Up => w * d,
                Facing::West | Facing::East => d * h,
                Facing::North | Facing::South => w * h,
            };
            if area == 0.0 {
                continue;
            }
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
///
/// Keep `ALL` in the same order as the variants — `index()` is derived
/// from it and the mobshot sheet iterates it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EntityModelKind {
    Player,
    Zombie,
    ZombieVillager,
    Husk,
    Drowned,
    Skeleton,
    Stray,
    Bogged,
    Parched,
    WitherSkeleton,
    Creeper,
    Spider,
    CaveSpider,
    Enderman,
    Slime,
    MagmaCube,
    Cow,
    Mooshroom,
    Pig,
    Sheep,
    Chicken,
    Wolf,
    Squid,
    GlowSquid,
    Rabbit,
    Villager,
    WanderingTrader,
    Witch,
    Pillager,
    Vindicator,
    Evoker,
    Illusioner,
    Vex,
    Phantom,
    Guardian,
    ElderGuardian,
    Shulker,
    Silverfish,
    Endermite,
    Blaze,
    Ghast,
    Piglin,
    PiglinBrute,
    ZombifiedPiglin,
    Hoglin,
    Zoglin,
    Strider,
    Bat,
    Cat,
    Ocelot,
    Fox,
    Goat,
    Bee,
    Frog,
    Tadpole,
    Armadillo,
    Axolotl,
    Dolphin,
    Turtle,
    Cod,
    Salmon,
    Pufferfish,
    TropicalFish,
    Panda,
    PolarBear,
    Camel,
    Llama,
    TraderLlama,
    Parrot,
    Horse,
    Donkey,
    Mule,
    SkeletonHorse,
    ZombieHorse,
    SnowGolem,
    IronGolem,
    Allay,
    Warden,
    Sniffer,
    Breeze,
    Creaking,
    Ravager,
    Wither,
    EnderDragon,
    HappyGhast,
    CopperGolem,
    Nautilus,
    ZombieNautilus,
    Capsule,
}

impl EntityModelKind {
    pub const COUNT: usize = 89;
    pub const ALL: [EntityModelKind; Self::COUNT] = [
        EntityModelKind::Player,
        EntityModelKind::Zombie,
        EntityModelKind::ZombieVillager,
        EntityModelKind::Husk,
        EntityModelKind::Drowned,
        EntityModelKind::Skeleton,
        EntityModelKind::Stray,
        EntityModelKind::Bogged,
        EntityModelKind::Parched,
        EntityModelKind::WitherSkeleton,
        EntityModelKind::Creeper,
        EntityModelKind::Spider,
        EntityModelKind::CaveSpider,
        EntityModelKind::Enderman,
        EntityModelKind::Slime,
        EntityModelKind::MagmaCube,
        EntityModelKind::Cow,
        EntityModelKind::Mooshroom,
        EntityModelKind::Pig,
        EntityModelKind::Sheep,
        EntityModelKind::Chicken,
        EntityModelKind::Wolf,
        EntityModelKind::Squid,
        EntityModelKind::GlowSquid,
        EntityModelKind::Rabbit,
        EntityModelKind::Villager,
        EntityModelKind::WanderingTrader,
        EntityModelKind::Witch,
        EntityModelKind::Pillager,
        EntityModelKind::Vindicator,
        EntityModelKind::Evoker,
        EntityModelKind::Illusioner,
        EntityModelKind::Vex,
        EntityModelKind::Phantom,
        EntityModelKind::Guardian,
        EntityModelKind::ElderGuardian,
        EntityModelKind::Shulker,
        EntityModelKind::Silverfish,
        EntityModelKind::Endermite,
        EntityModelKind::Blaze,
        EntityModelKind::Ghast,
        EntityModelKind::Piglin,
        EntityModelKind::PiglinBrute,
        EntityModelKind::ZombifiedPiglin,
        EntityModelKind::Hoglin,
        EntityModelKind::Zoglin,
        EntityModelKind::Strider,
        EntityModelKind::Bat,
        EntityModelKind::Cat,
        EntityModelKind::Ocelot,
        EntityModelKind::Fox,
        EntityModelKind::Goat,
        EntityModelKind::Bee,
        EntityModelKind::Frog,
        EntityModelKind::Tadpole,
        EntityModelKind::Armadillo,
        EntityModelKind::Axolotl,
        EntityModelKind::Dolphin,
        EntityModelKind::Turtle,
        EntityModelKind::Cod,
        EntityModelKind::Salmon,
        EntityModelKind::Pufferfish,
        EntityModelKind::TropicalFish,
        EntityModelKind::Panda,
        EntityModelKind::PolarBear,
        EntityModelKind::Camel,
        EntityModelKind::Llama,
        EntityModelKind::TraderLlama,
        EntityModelKind::Parrot,
        EntityModelKind::Horse,
        EntityModelKind::Donkey,
        EntityModelKind::Mule,
        EntityModelKind::SkeletonHorse,
        EntityModelKind::ZombieHorse,
        EntityModelKind::SnowGolem,
        EntityModelKind::IronGolem,
        EntityModelKind::Allay,
        EntityModelKind::Warden,
        EntityModelKind::Sniffer,
        EntityModelKind::Breeze,
        EntityModelKind::Creaking,
        EntityModelKind::Ravager,
        EntityModelKind::Wither,
        EntityModelKind::EnderDragon,
        EntityModelKind::HappyGhast,
        EntityModelKind::CopperGolem,
        EntityModelKind::Nautilus,
        EntityModelKind::ZombieNautilus,
        EntityModelKind::Capsule,
    ];

    pub fn index(self) -> usize {
        Self::ALL.iter().position(|k| *k == self).unwrap()
    }

    pub fn name(self) -> &'static str {
        match self {
            EntityModelKind::Player => "player",
            EntityModelKind::Zombie => "zombie",
            EntityModelKind::ZombieVillager => "zombie_villager",
            EntityModelKind::Husk => "husk",
            EntityModelKind::Drowned => "drowned",
            EntityModelKind::Skeleton => "skeleton",
            EntityModelKind::Stray => "stray",
            EntityModelKind::Bogged => "bogged",
            EntityModelKind::Parched => "parched",
            EntityModelKind::WitherSkeleton => "wither_skeleton",
            EntityModelKind::Creeper => "creeper",
            EntityModelKind::Spider => "spider",
            EntityModelKind::CaveSpider => "cave_spider",
            EntityModelKind::Enderman => "enderman",
            EntityModelKind::Slime => "slime",
            EntityModelKind::MagmaCube => "magma_cube",
            EntityModelKind::Cow => "cow",
            EntityModelKind::Mooshroom => "mooshroom",
            EntityModelKind::Pig => "pig",
            EntityModelKind::Sheep => "sheep",
            EntityModelKind::Chicken => "chicken",
            EntityModelKind::Wolf => "wolf",
            EntityModelKind::Squid => "squid",
            EntityModelKind::GlowSquid => "glow_squid",
            EntityModelKind::Rabbit => "rabbit",
            EntityModelKind::Villager => "villager",
            EntityModelKind::WanderingTrader => "wandering_trader",
            EntityModelKind::Witch => "witch",
            EntityModelKind::Pillager => "pillager",
            EntityModelKind::Vindicator => "vindicator",
            EntityModelKind::Evoker => "evoker",
            EntityModelKind::Illusioner => "illusioner",
            EntityModelKind::Vex => "vex",
            EntityModelKind::Phantom => "phantom",
            EntityModelKind::Guardian => "guardian",
            EntityModelKind::ElderGuardian => "elder_guardian",
            EntityModelKind::Shulker => "shulker",
            EntityModelKind::Silverfish => "silverfish",
            EntityModelKind::Endermite => "endermite",
            EntityModelKind::Blaze => "blaze",
            EntityModelKind::Ghast => "ghast",
            EntityModelKind::Piglin => "piglin",
            EntityModelKind::PiglinBrute => "piglin_brute",
            EntityModelKind::ZombifiedPiglin => "zombified_piglin",
            EntityModelKind::Hoglin => "hoglin",
            EntityModelKind::Zoglin => "zoglin",
            EntityModelKind::Strider => "strider",
            EntityModelKind::Bat => "bat",
            EntityModelKind::Cat => "cat",
            EntityModelKind::Ocelot => "ocelot",
            EntityModelKind::Fox => "fox",
            EntityModelKind::Goat => "goat",
            EntityModelKind::Bee => "bee",
            EntityModelKind::Frog => "frog",
            EntityModelKind::Tadpole => "tadpole",
            EntityModelKind::Armadillo => "armadillo",
            EntityModelKind::Axolotl => "axolotl",
            EntityModelKind::Dolphin => "dolphin",
            EntityModelKind::Turtle => "turtle",
            EntityModelKind::Cod => "cod",
            EntityModelKind::Salmon => "salmon",
            EntityModelKind::Pufferfish => "pufferfish",
            EntityModelKind::TropicalFish => "tropical_fish",
            EntityModelKind::Panda => "panda",
            EntityModelKind::PolarBear => "polar_bear",
            EntityModelKind::Camel => "camel",
            EntityModelKind::Llama => "llama",
            EntityModelKind::TraderLlama => "trader_llama",
            EntityModelKind::Parrot => "parrot",
            EntityModelKind::Horse => "horse",
            EntityModelKind::Donkey => "donkey",
            EntityModelKind::Mule => "mule",
            EntityModelKind::SkeletonHorse => "skeleton_horse",
            EntityModelKind::ZombieHorse => "zombie_horse",
            EntityModelKind::SnowGolem => "snow_golem",
            EntityModelKind::IronGolem => "iron_golem",
            EntityModelKind::Allay => "allay",
            EntityModelKind::Warden => "warden",
            EntityModelKind::Sniffer => "sniffer",
            EntityModelKind::Breeze => "breeze",
            EntityModelKind::Creaking => "creaking",
            EntityModelKind::Ravager => "ravager",
            EntityModelKind::Wither => "wither",
            EntityModelKind::EnderDragon => "ender_dragon",
            EntityModelKind::HappyGhast => "happy_ghast",
            EntityModelKind::CopperGolem => "copper_golem",
            EntityModelKind::Nautilus => "nautilus",
            EntityModelKind::ZombieNautilus => "zombie_nautilus",
            EntityModelKind::Capsule => "capsule",
        }
    }
}

/// Map a wire entity-type name (`minecraft:<x>`) to a model kind. Players
/// are matched by type id upstream, not by this table. The `name()` of
/// almost every kind IS its wire name, so the table is generated from
/// `ALL`; the handful of aliases follow.
pub fn kind_for_entity_name(name: &str) -> EntityModelKind {
    let Some(short) = name.strip_prefix("minecraft:") else {
        return EntityModelKind::Capsule;
    };
    for k in EntityModelKind::ALL {
        if k != EntityModelKind::Player && k != EntityModelKind::Capsule && k.name() == short {
            return k;
        }
    }
    EntityModelKind::Capsule
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
    MobDef { kind: EntityModelKind::ZombieVillager, textures: &["zombie_villager"], build: zombie_villager },
    MobDef { kind: EntityModelKind::Husk, textures: &["husk"], build: husk },
    MobDef { kind: EntityModelKind::Drowned, textures: &["drowned", "drowned_outer"], build: drowned },
    MobDef { kind: EntityModelKind::Skeleton, textures: &["skeleton"], build: skeleton },
    MobDef { kind: EntityModelKind::Stray, textures: &["stray", "stray_overlay"], build: stray },
    MobDef { kind: EntityModelKind::Bogged, textures: &["bogged", "bogged_overlay"], build: bogged },
    MobDef { kind: EntityModelKind::Parched, textures: &["parched"], build: skeleton },
    MobDef { kind: EntityModelKind::WitherSkeleton, textures: &["wither_skeleton"], build: wither_skeleton },
    MobDef { kind: EntityModelKind::Creeper, textures: &["creeper"], build: creeper },
    MobDef { kind: EntityModelKind::Spider, textures: &["spider"], build: spider },
    MobDef { kind: EntityModelKind::CaveSpider, textures: &["cave_spider"], build: cave_spider },
    MobDef { kind: EntityModelKind::Enderman, textures: &["enderman"], build: enderman },
    MobDef { kind: EntityModelKind::Slime, textures: &["slime"], build: slime },
    MobDef { kind: EntityModelKind::MagmaCube, textures: &["magma_cube"], build: magma_cube },
    MobDef { kind: EntityModelKind::Cow, textures: &["cow"], build: cow },
    MobDef { kind: EntityModelKind::Mooshroom, textures: &["mooshroom"], build: cow },
    MobDef { kind: EntityModelKind::Pig, textures: &["pig"], build: pig },
    MobDef { kind: EntityModelKind::Sheep, textures: &["sheep", "sheep_wool"], build: sheep },
    MobDef { kind: EntityModelKind::Chicken, textures: &["chicken"], build: chicken },
    MobDef { kind: EntityModelKind::Wolf, textures: &["wolf"], build: wolf },
    MobDef { kind: EntityModelKind::Squid, textures: &["squid"], build: squid },
    MobDef { kind: EntityModelKind::GlowSquid, textures: &["glow_squid"], build: squid },
    MobDef { kind: EntityModelKind::Rabbit, textures: &["rabbit"], build: rabbit },
    MobDef { kind: EntityModelKind::Villager, textures: &["villager"], build: villager },
    MobDef { kind: EntityModelKind::WanderingTrader, textures: &["wandering_trader"], build: villager },
    MobDef { kind: EntityModelKind::Witch, textures: &["witch"], build: witch },
    MobDef { kind: EntityModelKind::Pillager, textures: &["pillager"], build: illager_arms },
    MobDef { kind: EntityModelKind::Vindicator, textures: &["vindicator"], build: illager_crossed },
    MobDef { kind: EntityModelKind::Evoker, textures: &["evoker"], build: illager_crossed },
    MobDef { kind: EntityModelKind::Illusioner, textures: &["illusioner"], build: illager_crossed },
    MobDef { kind: EntityModelKind::Vex, textures: &["vex"], build: vex },
    MobDef { kind: EntityModelKind::Phantom, textures: &["phantom"], build: phantom },
    MobDef { kind: EntityModelKind::Guardian, textures: &["guardian"], build: guardian },
    MobDef { kind: EntityModelKind::ElderGuardian, textures: &["elder_guardian"], build: elder_guardian },
    MobDef { kind: EntityModelKind::Shulker, textures: &["shulker"], build: shulker },
    MobDef { kind: EntityModelKind::Silverfish, textures: &["silverfish"], build: silverfish },
    MobDef { kind: EntityModelKind::Endermite, textures: &["endermite"], build: endermite },
    MobDef { kind: EntityModelKind::Blaze, textures: &["blaze"], build: blaze },
    MobDef { kind: EntityModelKind::Ghast, textures: &["ghast"], build: ghast },
    MobDef { kind: EntityModelKind::Piglin, textures: &["piglin"], build: piglin_normal },
    MobDef { kind: EntityModelKind::PiglinBrute, textures: &["piglin_brute"], build: piglin_normal },
    MobDef { kind: EntityModelKind::ZombifiedPiglin, textures: &["zombified_piglin"], build: piglin_zombified },
    MobDef { kind: EntityModelKind::Hoglin, textures: &["hoglin"], build: hoglin },
    MobDef { kind: EntityModelKind::Zoglin, textures: &["zoglin"], build: hoglin },
    MobDef { kind: EntityModelKind::Strider, textures: &["strider"], build: strider },
    MobDef { kind: EntityModelKind::Bat, textures: &["bat"], build: bat },
    MobDef { kind: EntityModelKind::Cat, textures: &["cat"], build: feline },
    MobDef { kind: EntityModelKind::Ocelot, textures: &["ocelot"], build: feline },
    MobDef { kind: EntityModelKind::Fox, textures: &["fox"], build: fox },
    MobDef { kind: EntityModelKind::Goat, textures: &["goat"], build: goat },
    MobDef { kind: EntityModelKind::Bee, textures: &["bee"], build: bee },
    MobDef { kind: EntityModelKind::Frog, textures: &["frog"], build: frog },
    MobDef { kind: EntityModelKind::Tadpole, textures: &["tadpole"], build: tadpole },
    MobDef { kind: EntityModelKind::Armadillo, textures: &["armadillo"], build: armadillo },
    MobDef { kind: EntityModelKind::Axolotl, textures: &["axolotl"], build: axolotl },
    MobDef { kind: EntityModelKind::Dolphin, textures: &["dolphin"], build: dolphin },
    MobDef { kind: EntityModelKind::Turtle, textures: &["turtle"], build: turtle },
    MobDef { kind: EntityModelKind::Cod, textures: &["cod"], build: cod },
    MobDef { kind: EntityModelKind::Salmon, textures: &["salmon"], build: salmon },
    MobDef { kind: EntityModelKind::Pufferfish, textures: &["pufferfish"], build: pufferfish },
    MobDef { kind: EntityModelKind::TropicalFish, textures: &["tropical_fish"], build: tropical_fish },
    MobDef { kind: EntityModelKind::Panda, textures: &["panda"], build: panda },
    MobDef { kind: EntityModelKind::PolarBear, textures: &["polar_bear"], build: polar_bear },
    MobDef { kind: EntityModelKind::Camel, textures: &["camel"], build: camel },
    MobDef { kind: EntityModelKind::Llama, textures: &["llama"], build: llama },
    MobDef { kind: EntityModelKind::TraderLlama, textures: &["llama"], build: llama },
    MobDef { kind: EntityModelKind::Parrot, textures: &["parrot"], build: parrot },
    MobDef { kind: EntityModelKind::Horse, textures: &["horse"], build: equine_plain },
    MobDef { kind: EntityModelKind::Donkey, textures: &["donkey"], build: equine_eared },
    MobDef { kind: EntityModelKind::Mule, textures: &["mule"], build: equine_eared },
    MobDef { kind: EntityModelKind::SkeletonHorse, textures: &["skeleton_horse"], build: equine_plain },
    MobDef { kind: EntityModelKind::ZombieHorse, textures: &["zombie_horse"], build: equine_plain },
    MobDef { kind: EntityModelKind::SnowGolem, textures: &["snow_golem"], build: snow_golem },
    MobDef { kind: EntityModelKind::IronGolem, textures: &["iron_golem"], build: iron_golem },
    MobDef { kind: EntityModelKind::Allay, textures: &["allay"], build: allay },
    MobDef { kind: EntityModelKind::Warden, textures: &["warden"], build: warden },
    MobDef { kind: EntityModelKind::Sniffer, textures: &["sniffer"], build: sniffer },
    MobDef { kind: EntityModelKind::Breeze, textures: &["breeze", "breeze_wind"], build: breeze },
    MobDef { kind: EntityModelKind::Creaking, textures: &["creaking"], build: creaking },
    MobDef { kind: EntityModelKind::Ravager, textures: &["ravager"], build: ravager },
    MobDef { kind: EntityModelKind::Wither, textures: &["wither"], build: wither },
    MobDef { kind: EntityModelKind::EnderDragon, textures: &["ender_dragon"], build: ender_dragon },
    MobDef { kind: EntityModelKind::HappyGhast, textures: &["happy_ghast"], build: happy_ghast },
    MobDef { kind: EntityModelKind::CopperGolem, textures: &["copper_golem"], build: copper_golem },
    MobDef { kind: EntityModelKind::Nautilus, textures: &["nautilus"], build: nautilus },
    MobDef { kind: EntityModelKind::ZombieNautilus, textures: &["zombie_nautilus"], build: nautilus },
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
/// Returns the head part so variants (bogged) can grow things on the skull.
fn skeleton_build(b: &mut ModelBuilder, overlay: bool) -> usize {
    let b = &mut *b;
    let head = humanoid_head_body(b, 0);
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
    head
}

fn skeleton_like(scale: f32, overlay: bool) -> Model {
    let mut b = ModelBuilder::new();
    skeleton_build(&mut b, overlay);
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
/// body, crossed arms, half-swing legs. Returns the head part so the witch
/// can stack her hat on it.
fn villager_build(b: &mut ModelBuilder) -> usize {
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
    head
}

fn villager() -> Model {
    let mut b = ModelBuilder::new();
    villager_build(&mut b);
    b.finish(1.0)
}

// ---------------------------------------------------------------------------
// Humanoid variants II — piglins, illagers, witch, zombie villager
// ---------------------------------------------------------------------------

/// Humanoid body + arms + legs shared by the piglin family
/// (`AbstractPiglinModel` = `HumanoidModel.createMesh` with a replaced
/// 10-wide head + snout + ears; the humanoid hat is hidden in vanilla).
fn piglin_like(arms_forward: bool) -> Model {
    let mut b = ModelBuilder::new();
    let head = b.part([0.0, 0.0, 0.0], Anim::Head, 1.0);
    b.cube(head, 0, (0.0, 0.0), [-5.0, -8.0, -4.0], [10.0, 8.0, 8.0], NONE);
    b.cube(head, 0, (31.0, 1.0), [-2.0, -4.0, -5.0], [4.0, 4.0, 1.0], NONE);
    b.cube(head, 0, (2.0, 4.0), [2.0, -2.0, -5.0], [1.0, 2.0, 1.0], NONE);
    b.cube(head, 0, (2.0, 0.0), [-3.0, -2.0, -5.0], [1.0, 2.0, 1.0], NONE);
    b.cube_f(head, 0, (39.0, 6.0), [-1.0, 0.0, -2.0], [1.0, 5.0, 4.0], 0.0, false, &[Fold::rot([0.0, 0.0, PI / 6.0], [-4.5, -6.0, 0.0])]);
    b.cube_f(head, 0, (51.0, 6.0), [0.0, 0.0, -2.0], [1.0, 5.0, 4.0], 0.0, false, &[Fold::rot([0.0, 0.0, -PI / 6.0], [4.5, -6.0, 0.0])]);
    b.cube(STATIC_PART, 0, (16.0, 16.0), [-4.0, 0.0, -2.0], [8.0, 12.0, 4.0], NONE);
    if arms_forward {
        let arms = Fold::rot([-FRAC_PI_2, 0.0, 0.0], [-5.0, 2.0, 0.0]);
        b.cube_f(STATIC_PART, 0, (40.0, 16.0), [-3.0, -2.0, -2.0], [4.0, 12.0, 4.0], 0.0, false, &[arms]);
        let arms_l = Fold::rot([-FRAC_PI_2, 0.0, 0.0], [5.0, 2.0, 0.0]);
        b.cube_f(STATIC_PART, 0, (40.0, 16.0), [-1.0, -2.0, -2.0], [4.0, 12.0, 4.0], 0.0, true, &[arms_l]);
    } else {
        let arm_r = b.part([-5.0, 2.0, 0.0], Anim::ArmRight, 1.0);
        b.cube(arm_r, 0, (40.0, 16.0), [-3.0, -2.0, -2.0], [4.0, 12.0, 4.0], NONE);
        let arm_l = b.part([5.0, 2.0, 0.0], Anim::ArmLeft, 1.0);
        b.cube_m(arm_l, 0, (40.0, 16.0), [-1.0, -2.0, -2.0], [4.0, 12.0, 4.0], NONE);
    }
    let leg_r = b.part([-1.9, 12.0, 0.0], Anim::LegRight, 1.0);
    b.cube(leg_r, 0, (0.0, 16.0), [-2.0, 0.0, -2.0], [4.0, 12.0, 4.0], NONE);
    let leg_l = b.part([1.9, 12.0, 0.0], Anim::LegLeft, 1.0);
    b.cube_m(leg_l, 0, (0.0, 16.0), [-2.0, 0.0, -2.0], [4.0, 12.0, 4.0], NONE);
    b.finish(1.0)
}

fn piglin_normal() -> Model {
    piglin_like(false)
}

fn piglin_zombified() -> Model {
    piglin_like(true)
}

/// `ZombieVillagerModel`: villager-style head (with integral nose) + hat +
/// rim + robed body, zombie forward arms (44,22 rects), villager legs.
fn zombie_villager() -> Model {
    let mut b = ModelBuilder::new();
    let head = b.part([0.0, 0.0, 0.0], Anim::Head, 1.0);
    b.cube(head, 0, (0.0, 0.0), [-4.0, -10.0, -4.0], [8.0, 10.0, 8.0], NONE);
    b.cube(head, 0, (24.0, 0.0), [-1.0, -3.0, -6.0], [2.0, 4.0, 2.0], NONE);
    b.cube_g(head, 0, (32.0, 0.0), [-4.0, -10.0, -4.0], [8.0, 10.0, 8.0], 0.5, NONE);
    b.cube_f(head, 0, (30.0, 47.0), [-8.0, -8.0, -6.0], [16.0, 16.0, 1.0], 0.0, false, &[Fold::rot([-FRAC_PI_2, 0.0, 0.0], [0.0, 0.0, 0.0])]);
    b.cube(STATIC_PART, 0, (16.0, 20.0), [-4.0, 0.0, -3.0], [8.0, 12.0, 6.0], NONE);
    b.cube_g(STATIC_PART, 0, (0.0, 38.0), [-4.0, 0.0, -3.0], [8.0, 20.0, 6.0], 0.05, NONE);
    let arms = Fold::rot([-FRAC_PI_2, 0.0, 0.0], [-5.0, 2.0, 0.0]);
    b.cube_f(STATIC_PART, 0, (44.0, 22.0), [-3.0, -2.0, -2.0], [4.0, 12.0, 4.0], 0.0, false, &[arms]);
    let arms_l = Fold::rot([-FRAC_PI_2, 0.0, 0.0], [5.0, 2.0, 0.0]);
    b.cube_f(STATIC_PART, 0, (44.0, 22.0), [-1.0, -2.0, -2.0], [4.0, 12.0, 4.0], 0.0, true, &[arms_l]);
    let leg_r = b.part([-2.0, 12.0, 0.0], Anim::LegRight, 1.0);
    b.cube(leg_r, 0, (0.0, 22.0), [-2.0, 0.0, -2.0], [4.0, 12.0, 4.0], NONE);
    let leg_l = b.part([2.0, 12.0, 0.0], Anim::LegLeft, 1.0);
    b.cube_m(leg_l, 0, (0.0, 22.0), [-2.0, 0.0, -2.0], [4.0, 12.0, 4.0], NONE);
    b.finish(1.0)
}

/// `BoggedModel`: the skeleton (+ stray-style overlay on texture 1) with
/// mushroom plates sprouting from the head.
fn bogged() -> Model {
    let mut b = ModelBuilder::new();
    let head = skeleton_build(&mut b, true);
    let shrooms: [((f32, f32), [f32; 3], [f32; 3], f32); 6] = [
        ((50.0, 16.0), [-3.0, -3.0, 0.0], [3.0, -8.0, 3.0], PI / 4.0),
        ((50.0, 16.0), [-3.0, -3.0, 0.0], [3.0, -8.0, 3.0], PI * 3.0 / 4.0),
        ((50.0, 22.0), [-3.0, -3.0, 0.0], [-3.0, -8.0, -3.0], PI / 4.0),
        ((50.0, 22.0), [-3.0, -3.0, 0.0], [-3.0, -8.0, -3.0], PI * 3.0 / 4.0),
        ((50.0, 28.0), [-3.0, -4.0, 0.0], [-2.0, -1.0, 4.0], 0.0),
        ((50.0, 28.0), [-3.0, -4.0, 0.0], [-2.0, -1.0, 4.0], 0.0),
    ];
    for (i, ((u, v), min, off, yrot)) in shrooms.into_iter().enumerate() {
        // The last two lie flat on the skull (x −π/2 + z ±π/4).
        let rot = if i >= 4 {
            [-FRAC_PI_2, 0.0, if i == 4 { PI / 4.0 } else { PI * 3.0 / 4.0 }]
        } else {
            [0.0, yrot, 0.0]
        };
        b.cube_f(head, 0, (u, v), min, [6.0, 4.0, 0.0], 0.0, false, &[Fold::rot(rot, off)]);
    }
    b.finish(1.0)
}

/// `IllagerModel` (pillager/vindicator/evoker/illusioner share the mesh):
/// big head + hat + nose, robed body, and either crossed arms (idle
/// vindicator/evoker) or hanging arms (crossbow-carrying pillager).
fn illager(crossed: bool) -> Model {
    let mut b = ModelBuilder::new();
    let head = b.part([0.0, 0.0, 0.0], Anim::Head, 1.0);
    b.cube(head, 0, (0.0, 0.0), [-4.0, -10.0, -4.0], [8.0, 10.0, 8.0], NONE);
    b.cube_g(head, 0, (32.0, 0.0), [-4.0, -10.0, -4.0], [8.0, 12.0, 8.0], 0.45, NONE);
    b.cube_f(head, 0, (24.0, 0.0), [-1.0, -1.0, -6.0], [2.0, 4.0, 2.0], 0.0, false, &[Fold::at([0.0, -2.0, 0.0])]);
    b.cube(STATIC_PART, 0, (16.0, 20.0), [-4.0, 0.0, -3.0], [8.0, 12.0, 6.0], NONE);
    b.cube_g(STATIC_PART, 0, (0.0, 38.0), [-4.0, 0.0, -3.0], [8.0, 20.0, 6.0], 0.5, NONE);
    if crossed {
        let arms = Fold::rot([-0.75, 0.0, 0.0], [0.0, 3.0, -1.0]);
        b.cube_f(STATIC_PART, 0, (44.0, 22.0), [-8.0, -2.0, -2.0], [4.0, 8.0, 4.0], 0.0, false, &[arms]);
        b.cube_f(STATIC_PART, 0, (44.0, 22.0), [4.0, -2.0, -2.0], [4.0, 8.0, 4.0], 0.0, true, &[arms]);
        b.cube_f(STATIC_PART, 0, (40.0, 38.0), [-4.0, 2.0, -2.0], [8.0, 4.0, 4.0], 0.0, false, &[arms]);
    } else {
        let arm_r = b.part([-5.0, 2.0, 0.0], Anim::ArmRight, 1.0);
        b.cube(arm_r, 0, (40.0, 46.0), [-3.0, -2.0, -2.0], [4.0, 12.0, 4.0], NONE);
        let arm_l = b.part([5.0, 2.0, 0.0], Anim::ArmLeft, 1.0);
        b.cube_m(arm_l, 0, (40.0, 46.0), [-1.0, -2.0, -2.0], [4.0, 12.0, 4.0], NONE);
    }
    let leg_r = b.part([-2.0, 12.0, 0.0], Anim::LegRight, 0.5);
    b.cube(leg_r, 0, (0.0, 22.0), [-2.0, 0.0, -2.0], [4.0, 12.0, 4.0], NONE);
    let leg_l = b.part([2.0, 12.0, 0.0], Anim::LegLeft, 0.5);
    b.cube_m(leg_l, 0, (0.0, 22.0), [-2.0, 0.0, -2.0], [4.0, 12.0, 4.0], NONE);
    b.finish(1.0)
}

fn illager_crossed() -> Model {
    illager(true)
}

fn illager_arms() -> Model {
    illager(false)
}

/// `WitchModel` = the villager mesh (children preserved — vanilla's
/// `addOrReplaceChild` keeps the nose/hat) + the 4-segment crooked hat and
/// the nose mole. Texture 64×128.
fn witch() -> Model {
    let mut b = ModelBuilder::new();
    let head = villager_build(&mut b);
    let hat = Fold::at([-5.0, -10.03125, -5.0]);
    b.cube_f(head, 0, (0.0, 64.0), [0.0, 0.0, 0.0], [10.0, 2.0, 10.0], 0.0, false, &[hat]);
    let hat2 = Fold::rot([-0.05235988, 0.0, 0.02617994], [1.75, -4.0, 2.0]);
    b.cube_f(head, 0, (0.0, 76.0), [0.0, 0.0, 0.0], [7.0, 4.0, 7.0], 0.0, false, &[hat2, hat]);
    let hat3 = Fold::rot([-0.10471976, 0.0, 0.05235988], [1.75, -4.0, 2.0]);
    b.cube_f(head, 0, (0.0, 87.0), [0.0, 0.0, 0.0], [4.0, 4.0, 4.0], 0.0, false, &[hat3, hat2, hat]);
    let hat4 = Fold::rot([-PI / 15.0, 0.0, 0.10471976], [1.75, -2.0, 2.0]);
    b.cube_f(head, 0, (0.0, 95.0), [0.0, 0.0, 0.0], [1.0, 2.0, 1.0], 0.25, false, &[hat4, hat3, hat2, hat]);
    // Mole under the nose (nose pose (0,−2,0), mole pose (0,−2,0)).
    b.cube_f(head, 0, (0.0, 0.0), [0.0, 3.0, -6.75], [1.0, 1.0, 1.0], -0.25, false, &[Fold::at([0.0, -2.0, 0.0]), Fold::at([0.0, -2.0, 0.0])]);
    b.finish(1.0)
}

// ---------------------------------------------------------------------------
// Monsters II
// ---------------------------------------------------------------------------

/// `MagmaCubeModel`: 8 stacked 8×1×8 slabs + the inner core cube.
fn magma_cube() -> Model {
    let mut b = ModelBuilder::new();
    for i in 0..8 {
        let (mut u, mut v) = (0.0, 0.0);
        if (1..4).contains(&i) {
            v = 9.0 * i as f32;
        } else if i > 3 {
            u = 32.0;
            v = 9.0 * i as f32 - 36.0;
        }
        b.cube(STATIC_PART, 0, (u, v), [-4.0, 16.0 + i as f32, -4.0], [8.0, 1.0, 8.0], NONE);
    }
    b.cube(STATIC_PART, 0, (24.0, 40.0), [-2.0, 18.0, -2.0], [4.0, 4.0, 4.0], NONE);
    // Same fixed medium size as the slime.
    b.finish(2.0)
}

/// `VexModel`: floating imp — head + tapered body + tiny arms + 0-thick
/// wings, all lifted by the root's −2.5 offset.
fn vex() -> Model {
    let mut b = ModelBuilder::new();
    let root = Fold::at([0.0, -2.5, 0.0]);
    let head = b.part([0.0, 17.5, 0.0], Anim::Head, 1.0);
    b.cube(head, 0, (0.0, 0.0), [-2.5, -5.0, -2.5], [5.0, 5.0, 5.0], NONE);
    let body = Fold::at([0.0, 20.0, 0.0]);
    b.cube_f(STATIC_PART, 0, (0.0, 10.0), [-1.5, 0.0, -1.0], [3.0, 4.0, 2.0], 0.0, false, &[body, root]);
    b.cube_f(STATIC_PART, 0, (0.0, 16.0), [-1.5, 1.0, -1.0], [3.0, 5.0, 2.0], -0.2, false, &[body, root]);
    b.cube_f(STATIC_PART, 0, (23.0, 0.0), [-1.25, -0.5, -1.0], [2.0, 4.0, 2.0], -0.1, false, &[Fold::at([-1.75, 0.25, 0.0]), body, root]);
    b.cube_f(STATIC_PART, 0, (23.0, 6.0), [-0.75, -0.5, -1.0], [2.0, 4.0, 2.0], -0.1, false, &[Fold::at([1.75, 0.25, 0.0]), body, root]);
    b.cube_f(STATIC_PART, 0, (16.0, 14.0), [0.0, 0.0, 0.0], [0.0, 5.0, 8.0], 0.0, true, &[Fold::at([0.5, 1.0, 1.0]), body, root]);
    b.cube_f(STATIC_PART, 0, (16.0, 14.0), [0.0, 0.0, 0.0], [0.0, 5.0, 8.0], 0.0, false, &[Fold::at([-0.5, 1.0, 1.0]), body, root]);
    b.finish(1.0)
}

/// `PhantomModel`: swept body + two-segment wings + tail, tilted head.
fn phantom() -> Model {
    let mut b = ModelBuilder::new();
    let body = Fold::rot([-0.1, 0.0, 0.0], [0.0, 0.0, 0.0]);
    b.cube_f(STATIC_PART, 0, (0.0, 8.0), [-3.0, -2.0, -8.0], [5.0, 3.0, 9.0], 0.0, false, &[body]);
    let tail_base = Fold::at([0.0, -2.0, 1.0]);
    b.cube_f(STATIC_PART, 0, (3.0, 20.0), [-2.0, 0.0, 0.0], [3.0, 2.0, 6.0], 0.0, false, &[tail_base, body]);
    b.cube_f(STATIC_PART, 0, (4.0, 29.0), [-1.0, 0.0, 0.0], [1.0, 1.0, 6.0], 0.0, false, &[Fold::at([0.0, 0.5, 6.0]), tail_base, body]);
    let wl = Fold::rot([0.0, 0.0, 0.1], [2.0, -2.0, -8.0]);
    b.cube_f(STATIC_PART, 0, (23.0, 12.0), [0.0, 0.0, 0.0], [6.0, 2.0, 9.0], 0.0, false, &[wl, body]);
    b.cube_f(STATIC_PART, 0, (16.0, 24.0), [0.0, 0.0, 0.0], [13.0, 1.0, 9.0], 0.0, false, &[Fold::rot([0.0, 0.0, 0.1], [6.0, 0.0, 0.0]), wl, body]);
    let wr = Fold::rot([0.0, 0.0, -0.1], [-3.0, -2.0, -8.0]);
    b.cube_f(STATIC_PART, 0, (23.0, 12.0), [-6.0, 0.0, 0.0], [6.0, 2.0, 9.0], 0.0, true, &[wr, body]);
    b.cube_f(STATIC_PART, 0, (16.0, 24.0), [-13.0, 0.0, 0.0], [13.0, 1.0, 9.0], 0.0, true, &[Fold::rot([0.0, 0.0, -0.1], [-6.0, 0.0, 0.0]), wr, body]);
    b.cube_f(STATIC_PART, 0, (0.0, 0.0), [-4.0, -2.0, -5.0], [7.0, 3.0, 5.0], 0.0, false, &[Fold::rot([0.2, 0.0, 0.0], [0.0, 1.0, -7.0]), body]);
    b.finish(1.0)
}

/// `GuardianModel`: the boxy body with side/top/bottom fins, 12 radial
/// spikes at their rest extension, the eye, and the 3-segment tail.
fn guardian_like(scale: f32) -> Model {
    const SPIKE_X_ROT: [f32; 12] = [1.75, 0.25, 0.0, 0.0, 0.5, 0.5, 0.5, 0.5, 1.25, 0.75, 0.0, 0.0];
    const SPIKE_Y_ROT: [f32; 12] = [0.0, 0.0, 0.0, 0.0, 0.25, 1.75, 1.25, 0.75, 0.0, 0.0, 0.0, 0.0];
    const SPIKE_Z_ROT: [f32; 12] = [0.0, 0.0, 0.25, 1.75, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.75, 1.25];
    const SPIKE_X: [f32; 12] = [0.0, 0.0, 8.0, -8.0, -8.0, 8.0, 8.0, -8.0, 0.0, 0.0, 8.0, -8.0];
    const SPIKE_Y: [f32; 12] = [-8.0, -8.0, -8.0, -8.0, 0.0, 0.0, 0.0, 0.0, 8.0, 8.0, 8.0, 8.0];
    const SPIKE_Z: [f32; 12] = [8.0, -8.0, 0.0, 0.0, -8.0, -8.0, 8.0, 8.0, 8.0, -8.0, 0.0, 0.0];
    let mut b = ModelBuilder::new();
    b.cube(STATIC_PART, 0, (0.0, 0.0), [-6.0, 10.0, -8.0], [12.0, 12.0, 16.0], NONE);
    b.cube(STATIC_PART, 0, (0.0, 28.0), [-8.0, 10.0, -6.0], [2.0, 12.0, 12.0], NONE);
    b.cube_m(STATIC_PART, 0, (0.0, 28.0), [6.0, 10.0, -6.0], [2.0, 12.0, 12.0], NONE);
    b.cube(STATIC_PART, 0, (16.0, 40.0), [-6.0, 8.0, -6.0], [12.0, 2.0, 12.0], NONE);
    b.cube(STATIC_PART, 0, (16.0, 40.0), [-6.0, 22.0, -6.0], [12.0, 2.0, 12.0], NONE);
    for i in 0..12 {
        // Rest pose: getSpike*(i, 0, 0) with offset 1 + 0.01·cos(i).
        let k = 1.0 + 0.01 * (i as f32).cos();
        let off = [SPIKE_X[i] * k, 16.0 + SPIKE_Y[i] * k, SPIKE_Z[i] * k];
        let rot = [PI * SPIKE_X_ROT[i], PI * SPIKE_Y_ROT[i], PI * SPIKE_Z_ROT[i]];
        b.cube_f(STATIC_PART, 0, (0.0, 0.0), [-1.0, -4.5, -1.0], [2.0, 9.0, 2.0], 0.0, false, &[Fold::rot(rot, off)]);
    }
    b.cube_f(STATIC_PART, 0, (8.0, 0.0), [-1.0, 15.0, 0.0], [2.0, 2.0, 1.0], 0.0, false, &[Fold::at([0.0, 0.0, -8.25])]);
    b.cube(STATIC_PART, 0, (40.0, 0.0), [-2.0, 14.0, 7.0], [4.0, 4.0, 8.0], NONE);
    let t1 = Fold::at([-1.5, 0.5, 14.0]);
    b.cube_f(STATIC_PART, 0, (0.0, 54.0), [0.0, 14.0, 0.0], [3.0, 3.0, 7.0], 0.0, false, &[t1]);
    let t2 = Fold::at([0.5, 0.5, 6.0]);
    b.cube_f(STATIC_PART, 0, (41.0, 32.0), [0.0, 14.0, 0.0], [2.0, 2.0, 6.0], 0.0, false, &[t2, t1]);
    b.cube_f(STATIC_PART, 0, (25.0, 19.0), [1.0, 10.5, 3.0], [1.0, 9.0, 9.0], 0.0, false, &[t2, t1]);
    b.finish(scale)
}

fn guardian() -> Model {
    guardian_like(1.0)
}

fn elder_guardian() -> Model {
    // Vanilla `ELDER_GUARDIAN_SCALE` mesh transform.
    guardian_like(2.35)
}

/// `ShulkerModel`: closed box (lid resting on base) + the head hidden
/// inside.
fn shulker() -> Model {
    let mut b = ModelBuilder::new();
    b.cube_f(STATIC_PART, 0, (0.0, 0.0), [-8.0, -16.0, -8.0], [16.0, 12.0, 16.0], 0.0, false, &[Fold::at([0.0, 24.0, 0.0])]);
    b.cube_f(STATIC_PART, 0, (0.0, 28.0), [-8.0, -8.0, -8.0], [16.0, 8.0, 16.0], 0.0, false, &[Fold::at([0.0, 24.0, 0.0])]);
    b.cube_f(STATIC_PART, 0, (0.0, 52.0), [-3.0, 0.0, -3.0], [6.0, 6.0, 6.0], 0.0, false, &[Fold::at([0.0, 12.0, 0.0])]);
    b.finish(1.0)
}

/// Segment-chain crawlers (`SilverfishModel` / `EndermiteModel`): boxes
/// centered on a marching z placement.
fn segmented(sizes: &[[f32; 3]], texs: &[(f32, f32)]) -> (ModelBuilder, Vec<f32>) {
    let mut b = ModelBuilder::new();
    let mut z = Vec::with_capacity(sizes.len());
    let mut placement = -3.5f32;
    for (i, s) in sizes.iter().enumerate() {
        b.cube_f(STATIC_PART, 0, texs[i], [s[0] * -0.5, 0.0, s[2] * -0.5], *s, 0.0, false, &[Fold::at([0.0, 24.0 - s[1], placement])]);
        z.push(placement);
        if i + 1 < sizes.len() {
            placement += (s[2] + sizes[i + 1][2]) * 0.5;
        }
    }
    (b, z)
}

fn silverfish() -> Model {
    let sizes: [[f32; 3]; 7] = [
        [3.0, 2.0, 2.0], [4.0, 3.0, 2.0], [6.0, 4.0, 3.0], [3.0, 3.0, 3.0],
        [2.0, 2.0, 3.0], [2.0, 1.0, 2.0], [1.0, 1.0, 2.0],
    ];
    let texs = [(0.0, 0.0), (0.0, 4.0), (0.0, 9.0), (0.0, 16.0), (0.0, 22.0), (11.0, 0.0), (13.0, 4.0)];
    let (mut b, z) = segmented(&sizes, &texs);
    b.cube_f(STATIC_PART, 0, (20.0, 0.0), [-5.0, 0.0, -1.5], [10.0, 8.0, 3.0], 0.0, false, &[Fold::at([0.0, 16.0, z[2]])]);
    b.cube_f(STATIC_PART, 0, (20.0, 11.0), [-3.0, 0.0, -1.5], [6.0, 4.0, 3.0], 0.0, false, &[Fold::at([0.0, 20.0, z[4]])]);
    b.cube_f(STATIC_PART, 0, (20.0, 18.0), [-3.0, 0.0, -1.5], [6.0, 5.0, 2.0], 0.0, false, &[Fold::at([0.0, 19.0, z[1]])]);
    b.finish(1.0)
}

fn endermite() -> Model {
    let sizes: [[f32; 3]; 4] = [[4.0, 3.0, 2.0], [6.0, 4.0, 5.0], [3.0, 3.0, 1.0], [1.0, 2.0, 1.0]];
    let texs = [(0.0, 0.0), (0.0, 5.0), (0.0, 14.0), (0.0, 18.0)];
    let (b, _) = segmented(&sizes, &texs);
    b.finish(1.0)
}

/// `BlazeModel`: head + 12 orbiting rods in three rings at their rest
/// positions.
fn blaze() -> Model {
    let mut b = ModelBuilder::new();
    let head = b.part([0.0, 0.0, 0.0], Anim::Head, 1.0);
    b.cube(head, 0, (0.0, 0.0), [-4.0, -4.0, -4.0], [8.0, 8.0, 8.0], NONE);
    let mut place = |angle0: f32, radius: f32, range: std::ops::Range<usize>, base_y: f32, y_freq: f32| {
        let mut angle = angle0;
        for i in range {
            let off = [angle.cos() * radius, base_y + (i as f32 * y_freq).cos(), angle.sin() * radius];
            b.cube_f(STATIC_PART, 0, (0.0, 16.0), [0.0, 0.0, 0.0], [2.0, 8.0, 2.0], 0.0, false, &[Fold::at(off)]);
            angle += FRAC_PI_2;
        }
    };
    place(0.0, 9.0, 0..4, -2.0, 0.5);
    place(PI / 4.0, 7.0, 4..8, 2.0, 0.5);
    place(0.47123894, 5.0, 8..12, 11.0, 0.75);
    b.finish(1.0)
}

/// Java's `Random` LCG — the ghast's tentacle lengths come from a fixed
/// seed (1660), so reproducing them exactly needs the exact generator.
struct JavaRandom {
    seed: u64,
}

impl JavaRandom {
    fn new(seed: u64) -> Self {
        JavaRandom { seed: (seed ^ 0x5DEECE66D) & ((1 << 48) - 1) }
    }
    fn next(&mut self, bits: u32) -> i32 {
        self.seed = self.seed.wrapping_mul(0x5DEECE66D).wrapping_add(0xB) & ((1 << 48) - 1);
        (self.seed >> (48 - bits)) as i32
    }
    fn next_int(&mut self, bound: i32) -> i32 {
        loop {
            let bits = self.next(31);
            let val = bits % bound;
            if bits - val + (bound - 1) >= 0 {
                return val;
            }
        }
    }
}

/// `GhastModel`: the 16³ body + 9 tentacles with seeded random lengths;
/// vanilla applies `MeshTransformer.scaling(4.5)` (ground-anchored — equal
/// to our `scale`).
fn ghast() -> Model {
    let mut b = ModelBuilder::new();
    b.cube(STATIC_PART, 0, (0.0, 0.0), [-8.0, -8.0, -8.0], [16.0, 16.0, 16.0], &[Fold::at([0.0, 17.6, 0.0])]);
    let mut rng = JavaRandom::new(1660);
    for i in 0..9i32 {
        let xo = ((i % 3) as f32 - (i / 3 % 2) as f32 * 0.5 + 0.25 - 1.0) * 5.0;
        let yo = ((i / 3) as f32 - 1.0) * 5.0;
        let len = (rng.next_int(7) + 8) as f32;
        b.cube(STATIC_PART, 0, (0.0, 0.0), [-1.0, 0.0, -1.0], [2.0, len, 2.0], &[Fold::at([xo, 24.6, yo])]);
    }
    b.finish(4.5)
}

/// `HoglinModel`: huge body + mane bristle, tilted head with flapped ears
/// + tusks, chunky asymmetric legs. Zoglin shares it.
fn hoglin() -> Model {
    let mut b = ModelBuilder::new();
    let body = Fold::at([0.0, 7.0, 0.0]);
    b.cube_f(STATIC_PART, 0, (1.0, 1.0), [-8.0, -7.0, -13.0], [16.0, 14.0, 26.0], 0.0, false, &[body]);
    b.cube_f(STATIC_PART, 0, (90.0, 33.0), [0.0, 0.0, -9.0], [0.0, 10.0, 19.0], 0.001, false, &[Fold::at([0.0, -14.0, -7.0]), body]);
    let head = b.part([0.0, 2.0, -12.0], Anim::Head, 1.0);
    let tilt = Fold::rot([0.87266463, 0.0, 0.0], [0.0, 0.0, 0.0]);
    b.cube_f(head, 0, (61.0, 1.0), [-7.0, -3.0, -19.0], [14.0, 6.0, 19.0], 0.0, false, &[tilt]);
    b.cube_f(head, 0, (1.0, 1.0), [-6.0, -1.0, -2.0], [6.0, 1.0, 4.0], 0.0, false, &[Fold::rot([0.0, 0.0, -PI * 2.0 / 9.0], [-6.0, -2.0, -3.0]), tilt]);
    b.cube_f(head, 0, (1.0, 6.0), [0.0, -1.0, -2.0], [6.0, 1.0, 4.0], 0.0, false, &[Fold::rot([0.0, 0.0, PI * 2.0 / 9.0], [6.0, -2.0, -3.0]), tilt]);
    b.cube_f(head, 0, (10.0, 13.0), [-1.0, -11.0, -1.0], [2.0, 11.0, 2.0], 0.0, false, &[Fold::at([-7.0, 2.0, -12.0]), tilt]);
    b.cube_f(head, 0, (1.0, 13.0), [-1.0, -11.0, -1.0], [2.0, 11.0, 2.0], 0.0, false, &[Fold::at([7.0, 2.0, -12.0]), tilt]);
    let legs = [
        ((66.0, 42.0), [-3.0, 0.0, -3.0], [6.0, 14.0, 6.0], [-4.0, 10.0, -8.5], Anim::QuadFrontRight),
        ((41.0, 42.0), [-3.0, 0.0, -3.0], [6.0, 14.0, 6.0], [4.0, 10.0, -8.5], Anim::QuadFrontLeft),
        ((21.0, 45.0), [-2.5, 0.0, -2.5], [5.0, 11.0, 5.0], [-5.0, 13.0, 10.0], Anim::QuadHindRight),
        ((0.0, 45.0), [-2.5, 0.0, -2.5], [5.0, 11.0, 5.0], [5.0, 13.0, 10.0], Anim::QuadHindLeft),
    ];
    for (uv, min, dims, pivot, anim) in legs {
        let p = b.part(pivot, anim, 1.0);
        b.cube(p, 0, uv, min, dims, NONE);
    }
    b.finish(1.0)
}

/// `AdultStriderModel`: tall legs, boxy body, six 0-thick hair bristles
/// fanning out the sides.
fn strider() -> Model {
    let mut b = ModelBuilder::new();
    let leg_r = b.part([-4.0, 8.0, 0.0], Anim::LegRight, 1.0);
    b.cube(leg_r, 0, (0.0, 32.0), [-2.0, 0.0, -2.0], [4.0, 16.0, 4.0], NONE);
    let leg_l = b.part([4.0, 8.0, 0.0], Anim::LegLeft, 1.0);
    b.cube(leg_l, 0, (0.0, 55.0), [-2.0, 0.0, -2.0], [4.0, 16.0, 4.0], NONE);
    let body = Fold::at([0.0, 1.0, 0.0]);
    b.cube_f(STATIC_PART, 0, (0.0, 0.0), [-8.0, -6.0, -8.0], [16.0, 14.0, 16.0], 0.0, false, &[body]);
    let bristles: [((f32, f32), [f32; 3], f32, [f32; 3], bool); 6] = [
        ((16.0, 65.0), [-12.0, 0.0, 0.0], -1.2217305, [-8.0, 4.0, -8.0], true),
        ((16.0, 49.0), [-12.0, 0.0, 0.0], -1.134464, [-8.0, -1.0, -8.0], true),
        ((16.0, 33.0), [-12.0, 0.0, 0.0], -0.87266463, [-8.0, -5.0, -8.0], true),
        ((16.0, 33.0), [0.0, 0.0, 0.0], 0.87266463, [8.0, -6.0, -8.0], false),
        ((16.0, 49.0), [0.0, 0.0, 0.0], 1.134464, [8.0, -2.0, -8.0], false),
        ((16.0, 65.0), [0.0, 0.0, 0.0], 1.2217305, [8.0, 3.0, -8.0], false),
    ];
    for (uv, min, zrot, off, mirror) in bristles {
        b.cube_f(STATIC_PART, 0, uv, min, [12.0, 0.0, 16.0], 0.0, mirror, &[Fold::rot([0.0, 0.0, zrot], off), body]);
    }
    b.finish(1.0)
}

// ---------------------------------------------------------------------------
// Passives II — the overworld menagerie
// ---------------------------------------------------------------------------

/// `BatModel`: roosting pose as authored — small body, big ears, folded
/// wings (0-thick plates).
fn bat() -> Model {
    let mut b = ModelBuilder::new();
    b.cube_f(STATIC_PART, 0, (0.0, 0.0), [-1.5, 0.0, -1.0], [3.0, 5.0, 2.0], 0.0, false, &[Fold::at([0.0, 17.0, 0.0])]);
    let head = b.part([0.0, 17.0, 0.0], Anim::Head, 1.0);
    b.cube(head, 0, (0.0, 7.0), [-2.0, -3.0, -1.0], [4.0, 3.0, 2.0], NONE);
    b.cube_f(head, 0, (1.0, 15.0), [-2.5, -4.0, 0.0], [3.0, 5.0, 0.0], 0.0, false, &[Fold::at([-1.5, -2.0, 0.0])]);
    b.cube_f(head, 0, (8.0, 15.0), [-0.1, -3.0, 0.0], [3.0, 5.0, 0.0], 0.0, false, &[Fold::at([1.1, -3.0, 0.0])]);
    let body = Fold::at([0.0, 17.0, 0.0]);
    let wr = Fold::at([-1.5, 0.0, 0.0]);
    b.cube_f(STATIC_PART, 0, (12.0, 0.0), [-2.0, -2.0, 0.0], [2.0, 7.0, 0.0], 0.0, false, &[wr, body]);
    b.cube_f(STATIC_PART, 0, (16.0, 0.0), [-6.0, -2.0, 0.0], [6.0, 8.0, 0.0], 0.0, false, &[Fold::at([-2.0, 0.0, 0.0]), wr, body]);
    let wl = Fold::at([1.5, 0.0, 0.0]);
    b.cube_f(STATIC_PART, 0, (12.0, 7.0), [0.0, -2.0, 0.0], [2.0, 7.0, 0.0], 0.0, false, &[wl, body]);
    b.cube_f(STATIC_PART, 0, (16.0, 8.0), [0.0, -2.0, 0.0], [6.0, 8.0, 0.0], 0.0, false, &[Fold::at([2.0, 0.0, 0.0]), wl, body]);
    b.cube_f(STATIC_PART, 0, (16.0, 16.0), [-1.5, 0.0, 0.0], [3.0, 2.0, 0.0], 0.0, false, &[Fold::at([0.0, 5.0, 0.0]), body]);
    b.finish(1.0)
}

/// `AdultFelineModel` (cat + ocelot): slinky body, two-segment tail,
/// tall front legs.
fn feline() -> Model {
    let mut b = ModelBuilder::new();
    let head = b.part([0.0, 15.0, -9.0], Anim::Head, 1.0);
    b.cube(head, 0, (0.0, 0.0), [-2.5, -2.0, -3.0], [5.0, 4.0, 5.0], NONE);
    b.cube(head, 0, (0.0, 24.0), [-1.5, -0.001, -4.0], [3.0, 2.0, 2.0], NONE);
    b.cube(head, 0, (0.0, 10.0), [-2.0, -3.0, 0.0], [1.0, 1.0, 2.0], NONE);
    b.cube(head, 0, (6.0, 10.0), [1.0, -3.0, 0.0], [1.0, 1.0, 2.0], NONE);
    let body = Fold::rot([FRAC_PI_2, 0.0, 0.0], [0.0, 12.0, -10.0]);
    b.cube_f(STATIC_PART, 0, (20.0, 0.0), [-2.0, 3.0, -8.0], [4.0, 16.0, 6.0], 0.0, false, &[body]);
    b.cube_f(STATIC_PART, 0, (0.0, 15.0), [-0.5, 0.0, 0.0], [1.0, 8.0, 1.0], 0.0, false, &[Fold::rot([0.9, 0.0, 0.0], [0.0, 15.0, 8.0])]);
    b.cube_f(STATIC_PART, 0, (4.0, 15.0), [-0.5, 0.0, 0.0], [1.0, 8.0, 1.0], -0.02, false, &[Fold::at([0.0, 20.0, 14.0])]);
    let legs = [
        ((8.0, 13.0), [-1.0, 0.0, 1.0], [2.0, 6.0, 2.0], [1.1, 18.0, 5.0], Anim::QuadHindLeft),
        ((8.0, 13.0), [-1.0, 0.0, 1.0], [2.0, 6.0, 2.0], [-1.1, 18.0, 5.0], Anim::QuadHindRight),
        ((40.0, 0.0), [-1.0, 0.0, 0.0], [2.0, 10.0, 2.0], [1.2, 14.1, -5.0], Anim::QuadFrontLeft),
        ((40.0, 0.0), [-1.0, 0.0, 0.0], [2.0, 10.0, 2.0], [-1.2, 14.1, -5.0], Anim::QuadFrontRight),
    ];
    for (uv, min, dims, pivot, anim) in legs {
        let p = b.part(pivot, anim, 1.0);
        b.cube(p, 0, uv, min, dims, NONE);
    }
    b.finish(1.0)
}

/// `AdultFoxModel`: big-eared head, rotated body with the tail riding it.
fn fox() -> Model {
    let mut b = ModelBuilder::new();
    let head = b.part([-1.0, 16.5, -3.0], Anim::Head, 1.0);
    b.cube(head, 0, (1.0, 5.0), [-3.0, -2.0, -5.0], [8.0, 6.0, 6.0], NONE);
    b.cube(head, 0, (8.0, 1.0), [-3.0, -4.0, -4.0], [2.0, 2.0, 1.0], NONE);
    b.cube(head, 0, (15.0, 1.0), [3.0, -4.0, -4.0], [2.0, 2.0, 1.0], NONE);
    b.cube(head, 0, (6.0, 18.0), [-1.0, 2.01, -8.0], [4.0, 2.0, 3.0], NONE);
    let body = Fold::rot([FRAC_PI_2, 0.0, 0.0], [0.0, 16.0, -6.0]);
    b.cube_f(STATIC_PART, 0, (24.0, 15.0), [-3.0, 3.999, -3.5], [6.0, 11.0, 6.0], 0.0, false, &[body]);
    let legs = [
        ((13.0, 24.0), [-5.0, 17.5, 7.0], Anim::QuadHindRight),
        ((4.0, 24.0), [-1.0, 17.5, 7.0], Anim::QuadHindLeft),
        ((13.0, 24.0), [-5.0, 17.5, 0.0], Anim::QuadFrontRight),
        ((4.0, 24.0), [-1.0, 17.5, 0.0], Anim::QuadFrontLeft),
    ];
    for (uv, pivot, anim) in legs {
        let p = b.part(pivot, anim, 1.0);
        b.cube_g(p, 0, uv, [2.0, 0.5, -1.0], [2.0, 6.0, 2.0], 0.001, NONE);
    }
    b.cube_f(STATIC_PART, 0, (30.0, 0.0), [2.0, 0.0, -1.0], [4.0, 9.0, 5.0], 0.0, false, &[Fold::rot([-0.05235988, 0.0, 0.0], [-4.0, 15.0, -1.0]), body]);
    b.finish(1.0)
}

/// `GoatModel`: horned head with rotated skull box + goatee, double-box
/// body, asymmetric legs.
fn goat() -> Model {
    let mut b = ModelBuilder::new();
    let head = b.part([1.0, 14.0, 0.0], Anim::Head, 1.0);
    b.cube(head, 0, (2.0, 61.0), [-6.0, -11.0, -10.0], [3.0, 2.0, 1.0], NONE);
    b.cube_m(head, 0, (2.0, 61.0), [2.0, -11.0, -10.0], [3.0, 2.0, 1.0], NONE);
    b.cube(head, 0, (23.0, 52.0), [-0.5, -3.0, -14.0], [0.0, 7.0, 5.0], NONE);
    b.cube(head, 0, (12.0, 55.0), [-0.01, -16.0, -10.0], [2.0, 7.0, 2.0], NONE);
    b.cube(head, 0, (12.0, 55.0), [-2.99, -16.0, -10.0], [2.0, 7.0, 2.0], NONE);
    b.cube_f(head, 0, (34.0, 46.0), [-3.0, -4.0, -8.0], [5.0, 7.0, 10.0], 0.0, false, &[Fold::rot([0.9599, 0.0, 0.0], [0.0, -8.0, -8.0])]);
    let body = Fold::at([0.0, 24.0, 0.0]);
    b.cube_f(STATIC_PART, 0, (1.0, 1.0), [-4.0, -17.0, -7.0], [9.0, 11.0, 16.0], 0.0, false, &[body]);
    b.cube_f(STATIC_PART, 0, (0.0, 28.0), [-5.0, -18.0, -8.0], [11.0, 14.0, 11.0], 0.0, false, &[body]);
    let legs = [
        ((36.0, 29.0), [0.0, 4.0, 0.0], [3.0, 6.0, 3.0], [1.0, 14.0, 4.0], Anim::QuadHindLeft),
        ((49.0, 29.0), [0.0, 4.0, 0.0], [3.0, 6.0, 3.0], [-3.0, 14.0, 4.0], Anim::QuadHindRight),
        ((49.0, 2.0), [0.0, 0.0, 0.0], [3.0, 10.0, 3.0], [1.0, 14.0, -6.0], Anim::QuadFrontLeft),
        ((35.0, 2.0), [0.0, 0.0, 0.0], [3.0, 10.0, 3.0], [-3.0, 14.0, -6.0], Anim::QuadFrontRight),
    ];
    for (uv, min, dims, pivot, anim) in legs {
        let p = b.part(pivot, anim, 1.0);
        b.cube(p, 0, uv, min, dims, NONE);
    }
    b.finish(1.0)
}

/// `AdultBeeModel`: fat body with antennae + stinger, spread 0-thick
/// wings, dangling leg plates. Everything hangs off the hovering bone.
fn bee() -> Model {
    let mut b = ModelBuilder::new();
    let bone = Fold::at([0.0, 19.0, 0.0]);
    b.cube_f(STATIC_PART, 0, (0.0, 0.0), [-3.5, -4.0, -5.0], [7.0, 7.0, 10.0], 0.0, false, &[bone]);
    b.cube_f(STATIC_PART, 0, (26.0, 7.0), [0.0, -1.0, 5.0], [0.0, 1.0, 2.0], 0.0, false, &[bone]);
    b.cube_f(STATIC_PART, 0, (2.0, 0.0), [1.5, -2.0, -3.0], [1.0, 2.0, 3.0], 0.0, false, &[Fold::at([0.0, -2.0, -5.0]), bone]);
    b.cube_f(STATIC_PART, 0, (2.0, 3.0), [-2.5, -2.0, -3.0], [1.0, 2.0, 3.0], 0.0, false, &[Fold::at([0.0, -2.0, -5.0]), bone]);
    b.cube_f(STATIC_PART, 0, (0.0, 18.0), [-9.0, 0.0, 0.0], [9.0, 0.0, 6.0], 0.001, false, &[Fold::rot([0.0, -0.2618, 0.0], [-1.5, -4.0, -3.0]), bone]);
    b.cube_f(STATIC_PART, 0, (0.0, 18.0), [0.0, 0.0, 0.0], [9.0, 0.0, 6.0], 0.001, true, &[Fold::rot([0.0, 0.2618, 0.0], [1.5, -4.0, -3.0]), bone]);
    b.cube_f(STATIC_PART, 0, (26.0, 1.0), [-5.0, 0.0, 0.0], [7.0, 2.0, 0.0], 0.0, false, &[Fold::at([1.5, 3.0, -2.0]), bone]);
    b.cube_f(STATIC_PART, 0, (26.0, 3.0), [-5.0, 0.0, 0.0], [7.0, 2.0, 0.0], 0.0, false, &[Fold::at([1.5, 3.0, 0.0]), bone]);
    b.cube_f(STATIC_PART, 0, (26.0, 5.0), [-5.0, 0.0, 0.0], [7.0, 2.0, 0.0], 0.0, false, &[Fold::at([1.5, 3.0, 2.0]), bone]);
    b.finish(1.0)
}

/// `FrogModel`: flat body/head plates, pop eyes, folded arms with hand
/// plates, squat legs with feet.
fn frog() -> Model {
    let mut b = ModelBuilder::new();
    let root = Fold::at([0.0, 24.0, 0.0]);
    let body = Fold::at([0.0, -2.0, 4.0]);
    b.cube_f(STATIC_PART, 0, (3.0, 1.0), [-3.5, -2.0, -8.0], [7.0, 3.0, 9.0], 0.0, false, &[body, root]);
    b.cube_f(STATIC_PART, 0, (23.0, 22.0), [-3.5, -1.0, -8.0], [7.0, 0.0, 9.0], 0.0, false, &[body, root]);
    let head = Fold::at([0.0, -2.0, -1.0]);
    b.cube_f(STATIC_PART, 0, (23.0, 13.0), [-3.5, -1.0, -7.0], [7.0, 0.0, 9.0], 0.0, false, &[head, body, root]);
    b.cube_f(STATIC_PART, 0, (0.0, 13.0), [-3.5, -2.0, -7.0], [7.0, 3.0, 9.0], 0.0, false, &[head, body, root]);
    let eyes = Fold::at([-0.5, 0.0, 2.0]);
    b.cube_f(STATIC_PART, 0, (0.0, 0.0), [-1.5, -1.0, -1.5], [3.0, 2.0, 3.0], 0.0, false, &[Fold::at([-1.5, -3.0, -6.5]), eyes, head, body, root]);
    b.cube_f(STATIC_PART, 0, (0.0, 5.0), [-1.5, -1.0, -1.5], [3.0, 2.0, 3.0], 0.0, false, &[Fold::at([2.5, -3.0, -6.5]), eyes, head, body, root]);
    b.cube_f(STATIC_PART, 0, (26.0, 5.0), [-3.5, -0.1, -2.9], [7.0, 2.0, 3.0], -0.1, false, &[Fold::at([0.0, -1.0, -5.0]), body, root]);
    b.cube_f(STATIC_PART, 0, (17.0, 13.0), [-2.0, 0.0, -7.1], [4.0, 0.0, 7.0], 0.0, false, &[Fold::at([0.0, -1.01, 1.0]), body, root]);
    let arm_l = Fold::at([4.0, -1.0, -6.5]);
    b.cube_f(STATIC_PART, 0, (0.0, 32.0), [-1.0, 0.0, -1.0], [2.0, 3.0, 3.0], 0.0, false, &[arm_l, body, root]);
    b.cube_f(STATIC_PART, 0, (18.0, 40.0), [-4.0, 0.01, -4.0], [8.0, 0.0, 8.0], 0.0, false, &[Fold::at([0.0, 3.0, -1.0]), arm_l, body, root]);
    let arm_r = Fold::at([-4.0, -1.0, -6.5]);
    b.cube_f(STATIC_PART, 0, (0.0, 38.0), [-1.0, 0.0, -1.0], [2.0, 3.0, 3.0], 0.0, false, &[arm_r, body, root]);
    b.cube_f(STATIC_PART, 0, (2.0, 40.0), [-4.0, 0.01, -5.0], [8.0, 0.0, 8.0], 0.0, false, &[Fold::at([0.0, 3.0, 0.0]), arm_r, body, root]);
    let leg_l = Fold::at([3.5, -3.0, 4.0]);
    b.cube_f(STATIC_PART, 0, (14.0, 25.0), [-1.0, 0.0, -2.0], [3.0, 3.0, 4.0], 0.0, false, &[leg_l, root]);
    b.cube_f(STATIC_PART, 0, (2.0, 32.0), [-4.0, 0.01, -4.0], [8.0, 0.0, 8.0], 0.0, false, &[Fold::at([2.0, 3.0, 0.0]), leg_l, root]);
    let leg_r = Fold::at([-3.5, -3.0, 4.0]);
    b.cube_f(STATIC_PART, 0, (0.0, 25.0), [-2.0, 0.0, -2.0], [3.0, 3.0, 4.0], 0.0, false, &[leg_r, root]);
    b.cube_f(STATIC_PART, 0, (18.0, 32.0), [-4.0, 0.01, -4.0], [8.0, 0.0, 8.0], 0.0, false, &[Fold::at([-2.0, 3.0, 0.0]), leg_r, root]);
    b.finish(1.0)
}

/// `TadpoleModel`: a nubbin + 0-thick tail.
fn tadpole() -> Model {
    let mut b = ModelBuilder::new();
    b.cube_f(STATIC_PART, 0, (0.0, 0.0), [-1.5, -1.0, 0.0], [3.0, 2.0, 3.0], 0.0, false, &[Fold::at([0.0, 22.0, -3.0])]);
    b.cube_f(STATIC_PART, 0, (0.0, 0.0), [0.0, -1.0, 0.0], [0.0, 2.0, 7.0], 0.0, false, &[Fold::at([0.0, 22.0, 0.0])]);
    b.finish(1.0)
}

/// `AdultArmadilloModel`: double-shelled body + angled tail + tiny head
/// with wispy ears (the rolled-up ball cube is visibility-gated in vanilla
/// and skipped here).
fn armadillo() -> Model {
    let mut b = ModelBuilder::new();
    let body = Fold::at([0.0, 21.0, 4.0]);
    b.cube_f(STATIC_PART, 0, (0.0, 20.0), [-4.0, -7.0, -10.0], [8.0, 8.0, 12.0], 0.3, false, &[body]);
    b.cube_f(STATIC_PART, 0, (0.0, 40.0), [-4.0, -7.0, -10.0], [8.0, 8.0, 12.0], 0.0, false, &[body]);
    b.cube_f(STATIC_PART, 0, (44.0, 53.0), [-0.5, -0.0865, 0.0933], [1.0, 6.0, 1.0], 0.0, false, &[Fold::rot([0.5061, 0.0, 0.0], [0.0, -3.0, 1.0]), body]);
    let head = b.part([0.0, 19.0, -7.0], Anim::Head, 1.0);
    b.cube_f(head, 0, (43.0, 15.0), [-1.5, -1.0, -1.0], [3.0, 5.0, 2.0], 0.0, false, &[Fold::rot([-0.3927, 0.0, 0.0], [0.0, 0.0, 0.0])]);
    b.cube_f(head, 0, (43.0, 10.0), [-2.0, -3.0, 0.0], [2.0, 5.0, 0.0], 0.0, false, &[Fold::rot([0.1886, -0.3864, -0.0718], [-0.5, 0.0, -0.6]), Fold::at([-1.0, -1.0, 0.0])]);
    b.cube_f(head, 0, (47.0, 10.0), [0.0, -3.0, 0.0], [2.0, 5.0, 0.0], 0.0, false, &[Fold::rot([0.1886, 0.3864, 0.0718], [0.5, 1.0, -0.6]), Fold::at([1.0, -2.0, 0.0])]);
    let legs = [
        ((51.0, 31.0), [-2.0, 21.0, 4.0], Anim::QuadHindRight),
        ((42.0, 31.0), [2.0, 21.0, 4.0], Anim::QuadHindLeft),
        ((51.0, 43.0), [-2.0, 21.0, -4.0], Anim::QuadFrontRight),
        ((42.0, 43.0), [2.0, 21.0, -4.0], Anim::QuadFrontLeft),
    ];
    for (uv, pivot, anim) in legs {
        let p = b.part(pivot, anim, 1.0);
        b.cube(p, 0, uv, [-1.0, 0.0, -1.0], [2.0, 3.0, 2.0], NONE);
    }
    b.finish(1.0)
}

/// `AdultAxolotlModel`: flat body with a top fin, gilled head, stubby leg
/// plates, long tail fin.
fn axolotl() -> Model {
    let mut b = ModelBuilder::new();
    let body = Fold::at([0.0, 19.5, 5.0]);
    b.cube_f(STATIC_PART, 0, (0.0, 11.0), [-4.0, -2.0, -9.0], [8.0, 4.0, 10.0], 0.0, false, &[body]);
    b.cube_f(STATIC_PART, 0, (2.0, 17.0), [0.0, -3.0, -8.0], [0.0, 5.0, 9.0], 0.0, false, &[body]);
    let head = b.part([0.0, 19.5, -4.0], Anim::Head, 1.0);
    b.cube_g(head, 0, (0.0, 1.0), [-4.0, -3.0, -5.0], [8.0, 5.0, 5.0], 0.001, NONE);
    b.cube_f(head, 0, (3.0, 37.0), [-4.0, -3.0, 0.0], [8.0, 3.0, 0.0], 0.001, false, &[Fold::at([0.0, -3.0, -1.0])]);
    b.cube_f(head, 0, (0.0, 40.0), [-3.0, -5.0, 0.0], [3.0, 7.0, 0.0], 0.001, false, &[Fold::at([-4.0, 0.0, -1.0])]);
    b.cube_f(head, 0, (11.0, 40.0), [0.0, -5.0, 0.0], [3.0, 7.0, 0.0], 0.001, false, &[Fold::at([4.0, 0.0, -1.0])]);
    let legs = [
        ([-2.0, 0.0, 0.0], [-3.5, 20.5, 4.0], Anim::QuadHindRight),
        ([-1.0, 0.0, 0.0], [3.5, 20.5, 4.0], Anim::QuadHindLeft),
        ([-2.0, 0.0, 0.0], [-3.5, 20.5, -3.0], Anim::QuadFrontRight),
        ([-1.0, 0.0, 0.0], [3.5, 20.5, -3.0], Anim::QuadFrontLeft),
    ];
    for (min, pivot, anim) in legs {
        let p = b.part(pivot, anim, 1.0);
        b.cube_g(p, 0, (2.0, 13.0), min, [3.0, 5.0, 0.0], 0.001, NONE);
    }
    b.cube_f(STATIC_PART, 0, (2.0, 19.0), [0.0, -3.0, 0.0], [0.0, 5.0, 12.0], 0.0, false, &[Fold::at([0.0, 0.0, 1.0]), body]);
    b.finish(1.0)
}

/// `DolphinModel`: streamlined body, angled fins, two-segment tail, nosed
/// head.
fn dolphin() -> Model {
    let mut b = ModelBuilder::new();
    let body = Fold::at([0.0, 22.0, -5.0]);
    b.cube_f(STATIC_PART, 0, (22.0, 0.0), [-4.0, -7.0, 0.0], [8.0, 7.0, 13.0], 0.0, false, &[body]);
    b.cube_f(STATIC_PART, 0, (51.0, 0.0), [-0.5, 0.0, 8.0], [1.0, 4.0, 5.0], 0.0, false, &[Fold::rot([PI / 3.0, 0.0, 0.0], [0.0, 0.0, 0.0]), body]);
    b.cube_f(STATIC_PART, 0, (48.0, 20.0), [-0.5, -4.0, 0.0], [1.0, 4.0, 7.0], 0.0, true, &[Fold::rot([PI / 3.0, 0.0, PI * 2.0 / 3.0], [2.0, -2.0, 4.0]), body]);
    b.cube_f(STATIC_PART, 0, (48.0, 20.0), [-0.5, -4.0, 0.0], [1.0, 4.0, 7.0], 0.0, false, &[Fold::rot([PI / 3.0, 0.0, -PI * 2.0 / 3.0], [-2.0, -2.0, 4.0]), body]);
    let tail = Fold::rot([-0.10471976, 0.0, 0.0], [0.0, -2.5, 11.0]);
    b.cube_f(STATIC_PART, 0, (0.0, 19.0), [-2.0, -2.5, 0.0], [4.0, 5.0, 11.0], 0.0, false, &[tail, body]);
    b.cube_f(STATIC_PART, 0, (19.0, 20.0), [-5.0, -0.5, 0.0], [10.0, 1.0, 6.0], 0.0, false, &[Fold::at([0.0, 0.0, 9.0]), tail, body]);
    let head = Fold::at([0.0, -4.0, -3.0]);
    b.cube_f(STATIC_PART, 0, (0.0, 0.0), [-4.0, -3.0, -3.0], [8.0, 7.0, 6.0], 0.0, false, &[head, body]);
    b.cube_f(STATIC_PART, 0, (0.0, 13.0), [-1.0, 2.0, -7.0], [2.0, 2.0, 4.0], 0.0, false, &[head, body]);
    b.finish(1.0)
}

/// `AdultTurtleModel`: shell + belly plates, flipper legs, headed front.
fn turtle() -> Model {
    let mut b = ModelBuilder::new();
    let head = b.part([0.0, 19.0, -10.0], Anim::Head, 1.0);
    b.cube(head, 0, (3.0, 0.0), [-3.0, -1.0, -3.0], [6.0, 5.0, 6.0], NONE);
    let body = Fold::rot([FRAC_PI_2, 0.0, 0.0], [0.0, 11.0, -10.0]);
    b.cube_f(STATIC_PART, 0, (7.0, 37.0), [-9.5, 3.0, -10.0], [19.0, 20.0, 6.0], 0.0, false, &[body]);
    b.cube_f(STATIC_PART, 0, (31.0, 1.0), [-5.5, 3.0, -13.0], [11.0, 18.0, 3.0], 0.0, false, &[body]);
    b.cube_f(STATIC_PART, 0, (70.0, 33.0), [-4.5, 3.0, -14.0], [9.0, 18.0, 1.0], 0.0, false, &[body]);
    b.cube_f(STATIC_PART, 0, (1.0, 23.0), [-2.0, 0.0, 0.0], [4.0, 1.0, 10.0], 0.0, false, &[Fold::at([-3.5, 22.0, 11.0])]);
    b.cube_f(STATIC_PART, 0, (1.0, 12.0), [-2.0, 0.0, 0.0], [4.0, 1.0, 10.0], 0.0, false, &[Fold::at([3.5, 22.0, 11.0])]);
    b.cube_f(STATIC_PART, 0, (27.0, 30.0), [-13.0, 0.0, -2.0], [13.0, 1.0, 5.0], 0.0, false, &[Fold::at([-5.0, 21.0, -4.0])]);
    b.cube_f(STATIC_PART, 0, (27.0, 24.0), [0.0, 0.0, -2.0], [13.0, 1.0, 5.0], 0.0, false, &[Fold::at([5.0, 21.0, -4.0])]);
    b.finish(1.0)
}

// ---------------------------------------------------------------------------
// Fish
// ---------------------------------------------------------------------------

fn cod() -> Model {
    let mut b = ModelBuilder::new();
    b.cube_f(STATIC_PART, 0, (0.0, 0.0), [-1.0, -2.0, 0.0], [2.0, 4.0, 7.0], 0.0, false, &[Fold::at([0.0, 22.0, 0.0])]);
    b.cube_f(STATIC_PART, 0, (11.0, 0.0), [-1.0, -2.0, -3.0], [2.0, 4.0, 3.0], 0.0, false, &[Fold::at([0.0, 22.0, 0.0])]);
    b.cube_f(STATIC_PART, 0, (0.0, 0.0), [-1.0, -2.0, -1.0], [2.0, 3.0, 1.0], 0.0, false, &[Fold::at([0.0, 22.0, -3.0])]);
    b.cube_f(STATIC_PART, 0, (22.0, 1.0), [-2.0, 0.0, -1.0], [2.0, 0.0, 2.0], 0.0, false, &[Fold::rot([0.0, 0.0, -PI / 4.0], [-1.0, 23.0, 0.0])]);
    b.cube_f(STATIC_PART, 0, (22.0, 4.0), [0.0, 0.0, -1.0], [2.0, 0.0, 2.0], 0.0, false, &[Fold::rot([0.0, 0.0, PI / 4.0], [1.0, 23.0, 0.0])]);
    b.cube_f(STATIC_PART, 0, (22.0, 3.0), [0.0, -2.0, 0.0], [0.0, 4.0, 4.0], 0.0, false, &[Fold::at([0.0, 22.0, 7.0])]);
    b.cube_f(STATIC_PART, 0, (20.0, -6.0), [0.0, -1.0, -1.0], [0.0, 1.0, 6.0], 0.0, false, &[Fold::at([0.0, 20.0, 0.0])]);
    b.finish(1.0)
}

fn salmon() -> Model {
    let mut b = ModelBuilder::new();
    let front = Fold::at([0.0, 20.0, -7.2]);
    let back = Fold::at([0.0, 20.0, 0.8]);
    b.cube_f(STATIC_PART, 0, (0.0, 0.0), [-1.5, -2.5, 0.0], [3.0, 5.0, 8.0], 0.0, false, &[front]);
    b.cube_f(STATIC_PART, 0, (0.0, 13.0), [-1.5, -2.5, 0.0], [3.0, 5.0, 8.0], 0.0, false, &[back]);
    b.cube_f(STATIC_PART, 0, (22.0, 0.0), [-1.0, -2.0, -3.0], [2.0, 4.0, 3.0], 0.0, false, &[front]);
    b.cube_f(STATIC_PART, 0, (20.0, 10.0), [0.0, -2.5, 0.0], [0.0, 5.0, 6.0], 0.0, false, &[Fold::at([0.0, 0.0, 8.0]), back]);
    b.cube_f(STATIC_PART, 0, (2.0, 1.0), [0.0, 0.0, 0.0], [0.0, 2.0, 3.0], 0.0, false, &[Fold::at([0.0, -4.5, 5.0]), front]);
    b.cube_f(STATIC_PART, 0, (0.0, 2.0), [0.0, 0.0, 0.0], [0.0, 2.0, 4.0], 0.0, false, &[Fold::at([0.0, -4.5, -1.0]), back]);
    b.cube_f(STATIC_PART, 0, (-4.0, 0.0), [-2.0, 0.0, 0.0], [2.0, 0.0, 2.0], 0.0, false, &[Fold::rot([0.0, 0.0, -PI / 4.0], [-1.5, 21.5, -7.2])]);
    b.cube_f(STATIC_PART, 0, (0.0, 0.0), [0.0, 0.0, 0.0], [2.0, 0.0, 2.0], 0.0, false, &[Fold::rot([0.0, 0.0, PI / 4.0], [1.5, 21.5, -7.2])]);
    b.finish(1.0)
}

/// `PufferfishBigModel` — the fully inflated form, spikes and all.
fn pufferfish() -> Model {
    let mut b = ModelBuilder::new();
    b.cube_f(STATIC_PART, 0, (0.0, 0.0), [-4.0, -8.0, -4.0], [8.0, 8.0, 8.0], 0.0, false, &[Fold::at([0.0, 22.0, 0.0])]);
    b.cube_f(STATIC_PART, 0, (24.0, 0.0), [-2.0, 0.0, -1.0], [2.0, 1.0, 2.0], 0.0, false, &[Fold::at([-4.0, 15.0, -2.0])]);
    b.cube_f(STATIC_PART, 0, (24.0, 3.0), [0.0, 0.0, -1.0], [2.0, 1.0, 2.0], 0.0, false, &[Fold::at([4.0, 15.0, -2.0])]);
    let spikes: [((f32, f32), [f32; 3], [f32; 3], [f32; 3], [f32; 3]); 10] = [
        ((15.0, 17.0), [-4.0, -1.0, 0.0], [8.0, 1.0, 0.0], [PI / 4.0, 0.0, 0.0], [0.0, 14.0, -4.0]),
        ((14.0, 16.0), [-4.0, -1.0, 0.0], [8.0, 1.0, 1.0], [0.0, 0.0, 0.0], [0.0, 14.0, 0.0]),
        ((23.0, 18.0), [-4.0, -1.0, 0.0], [8.0, 1.0, 0.0], [-PI / 4.0, 0.0, 0.0], [0.0, 14.0, 4.0]),
        ((5.0, 17.0), [-1.0, -8.0, 0.0], [1.0, 8.0, 0.0], [0.0, -PI / 4.0, 0.0], [-4.0, 22.0, -4.0]),
        ((1.0, 17.0), [0.0, -8.0, 0.0], [1.0, 8.0, 0.0], [0.0, PI / 4.0, 0.0], [4.0, 22.0, -4.0]),
        ((15.0, 20.0), [-4.0, 0.0, 0.0], [8.0, 1.0, 0.0], [-PI / 4.0, 0.0, 0.0], [0.0, 22.0, -4.0]),
        ((15.0, 20.0), [-4.0, 0.0, 0.0], [8.0, 1.0, 0.0], [0.0, 0.0, 0.0], [0.0, 22.0, 0.0]),
        ((15.0, 20.0), [-4.0, 0.0, 0.0], [8.0, 1.0, 0.0], [PI / 4.0, 0.0, 0.0], [0.0, 22.0, 4.0]),
        ((9.0, 17.0), [-1.0, -8.0, 0.0], [1.0, 8.0, 0.0], [0.0, PI / 4.0, 0.0], [-4.0, 22.0, 4.0]),
        ((9.0, 17.0), [0.0, -8.0, 0.0], [1.0, 8.0, 0.0], [0.0, -PI / 4.0, 0.0], [4.0, 22.0, 4.0]),
    ];
    for (uv, min, dims, rot, off) in spikes {
        b.cube_f(STATIC_PART, 0, uv, min, dims, 0.0, false, &[Fold::rot(rot, off)]);
    }
    b.finish(1.0)
}

/// `TropicalFishSmallModel` (shape A).
fn tropical_fish() -> Model {
    let mut b = ModelBuilder::new();
    b.cube_f(STATIC_PART, 0, (0.0, 0.0), [-1.0, -1.5, -3.0], [2.0, 3.0, 6.0], 0.0, false, &[Fold::at([0.0, 22.0, 0.0])]);
    b.cube_f(STATIC_PART, 0, (22.0, -6.0), [0.0, -1.5, 0.0], [0.0, 3.0, 6.0], 0.0, false, &[Fold::at([0.0, 22.0, 3.0])]);
    b.cube_f(STATIC_PART, 0, (2.0, 16.0), [-2.0, -1.0, 0.0], [2.0, 2.0, 0.0], 0.0, false, &[Fold::rot([0.0, PI / 4.0, 0.0], [-1.0, 22.5, 0.0])]);
    b.cube_f(STATIC_PART, 0, (2.0, 12.0), [0.0, -1.0, 0.0], [2.0, 2.0, 0.0], 0.0, false, &[Fold::rot([0.0, -PI / 4.0, 0.0], [1.0, 22.5, 0.0])]);
    b.cube_f(STATIC_PART, 0, (10.0, -5.0), [0.0, -3.0, 0.0], [0.0, 3.0, 6.0], 0.0, false, &[Fold::at([0.0, 20.5, -3.0])]);
    b.finish(1.0)
}

// ---------------------------------------------------------------------------
// Big passives
// ---------------------------------------------------------------------------

/// `PandaModel`: wide head with ears + nose, huge rotated body, thick legs.
fn panda() -> Model {
    let mut b = ModelBuilder::new();
    let head = b.part([0.0, 11.5, -17.0], Anim::Head, 1.0);
    b.cube(head, 0, (0.0, 6.0), [-6.5, -5.0, -4.0], [13.0, 10.0, 9.0], NONE);
    b.cube(head, 0, (45.0, 16.0), [-3.5, 0.0, -6.0], [7.0, 5.0, 2.0], NONE);
    b.cube(head, 0, (52.0, 25.0), [3.5, -8.0, -1.0], [5.0, 4.0, 1.0], NONE);
    b.cube(head, 0, (52.0, 25.0), [-8.5, -8.0, -1.0], [5.0, 4.0, 1.0], NONE);
    let body = Fold::rot([FRAC_PI_2, 0.0, 0.0], [0.0, 10.0, 0.0]);
    b.cube_f(STATIC_PART, 0, (0.0, 25.0), [-9.5, -13.0, -6.5], [19.0, 26.0, 13.0], 0.0, false, &[body]);
    let legs = [
        ([-5.5, 15.0, 9.0], Anim::QuadHindRight),
        ([5.5, 15.0, 9.0], Anim::QuadHindLeft),
        ([-5.5, 15.0, -9.0], Anim::QuadFrontRight),
        ([5.5, 15.0, -9.0], Anim::QuadFrontLeft),
    ];
    for (pivot, anim) in legs {
        let p = b.part(pivot, anim, 1.0);
        b.cube(p, 0, (40.0, 0.0), [-3.0, 0.0, -3.0], [6.0, 9.0, 6.0], NONE);
    }
    b.finish(1.0)
}

/// `PolarBearModel` (mesh-scaled 1.2 in vanilla — ground-anchored, equal
/// to our scale).
fn polar_bear() -> Model {
    let mut b = ModelBuilder::new();
    let head = b.part([0.0, 10.0, -16.0], Anim::Head, 1.0);
    b.cube(head, 0, (0.0, 0.0), [-3.5, -3.0, -3.0], [7.0, 7.0, 7.0], NONE);
    b.cube(head, 0, (0.0, 44.0), [-2.5, 1.0, -6.0], [5.0, 3.0, 3.0], NONE);
    b.cube(head, 0, (26.0, 0.0), [-4.5, -4.0, -1.0], [2.0, 2.0, 1.0], NONE);
    b.cube_m(head, 0, (26.0, 0.0), [2.5, -4.0, -1.0], [2.0, 2.0, 1.0], NONE);
    let body = Fold::rot([FRAC_PI_2, 0.0, 0.0], [-2.0, 9.0, 12.0]);
    b.cube_f(STATIC_PART, 0, (0.0, 19.0), [-5.0, -13.0, -7.0], [14.0, 14.0, 11.0], 0.0, false, &[body]);
    b.cube_f(STATIC_PART, 0, (39.0, 0.0), [-4.0, -25.0, -7.0], [12.0, 12.0, 10.0], 0.0, false, &[body]);
    let legs = [
        ((50.0, 22.0), [4.0, 10.0, 8.0], [-4.5, 14.0, 6.0], Anim::QuadHindRight),
        ((50.0, 22.0), [4.0, 10.0, 8.0], [4.5, 14.0, 6.0], Anim::QuadHindLeft),
        ((50.0, 40.0), [4.0, 10.0, 6.0], [-3.5, 14.0, -8.0], Anim::QuadFrontRight),
        ((50.0, 40.0), [4.0, 10.0, 6.0], [3.5, 14.0, -8.0], Anim::QuadFrontLeft),
    ];
    for (uv, dims, pivot, anim) in legs {
        let p = b.part(pivot, anim, 1.0);
        b.cube(p, 0, uv, [-2.0, 0.0, -2.0], dims, NONE);
    }
    b.finish(1.2)
}

/// `AdultCamelModel`: long body with hump + tail, towering neck/head,
/// stilt legs.
fn camel() -> Model {
    let mut b = ModelBuilder::new();
    let body = Fold::at([0.0, 4.0, 9.5]);
    b.cube_f(STATIC_PART, 0, (0.0, 25.0), [-7.5, -12.0, -23.5], [15.0, 12.0, 27.0], 0.0, false, &[body]);
    b.cube_f(STATIC_PART, 0, (74.0, 0.0), [-4.5, -5.0, -5.5], [9.0, 5.0, 11.0], 0.0, false, &[Fold::at([0.0, -12.0, -10.0]), body]);
    b.cube_f(STATIC_PART, 0, (122.0, 0.0), [-1.5, 0.0, 0.0], [3.0, 14.0, 0.0], 0.0, false, &[Fold::at([0.0, -9.0, 3.5]), body]);
    let head = b.part([0.0, 1.0, -10.0], Anim::Head, 1.0);
    b.cube(head, 0, (60.0, 24.0), [-3.5, -7.0, -15.0], [7.0, 8.0, 19.0], NONE);
    b.cube(head, 0, (21.0, 0.0), [-3.5, -21.0, -15.0], [7.0, 14.0, 7.0], NONE);
    b.cube(head, 0, (50.0, 0.0), [-2.5, -21.0, -21.0], [5.0, 5.0, 6.0], NONE);
    b.cube_f(head, 0, (45.0, 0.0), [-0.5, 0.5, -1.0], [3.0, 1.0, 2.0], 0.0, false, &[Fold::at([2.5, -21.0, -9.5])]);
    b.cube_f(head, 0, (67.0, 0.0), [-2.5, 0.5, -1.0], [3.0, 1.0, 2.0], 0.0, false, &[Fold::at([-2.5, -21.0, -9.5])]);
    let legs = [
        ((58.0, 16.0), [4.9, 1.0, 9.5], Anim::QuadHindLeft),
        ((94.0, 16.0), [-4.9, 1.0, 9.5], Anim::QuadHindRight),
        ((0.0, 0.0), [4.9, 1.0, -10.5], Anim::QuadFrontLeft),
        ((0.0, 26.0), [-4.9, 1.0, -10.5], Anim::QuadFrontRight),
    ];
    for (uv, pivot, anim) in legs {
        let p = b.part(pivot, anim, 1.0);
        b.cube(p, 0, uv, [-2.5, 2.0, -2.5], [5.0, 21.0, 5.0], NONE);
    }
    b.finish(1.0)
}

/// `LlamaModel`: tall neck + eared head, rotated body, side chest boxes
/// (transparent on unchested skins), straight legs.
fn llama() -> Model {
    let mut b = ModelBuilder::new();
    let head = b.part([0.0, 7.0, -6.0], Anim::Head, 1.0);
    b.cube(head, 0, (0.0, 0.0), [-2.0, -14.0, -10.0], [4.0, 4.0, 9.0], NONE);
    b.cube(head, 0, (0.0, 14.0), [-4.0, -16.0, -6.0], [8.0, 18.0, 6.0], NONE);
    b.cube(head, 0, (17.0, 0.0), [-4.0, -19.0, -4.0], [3.0, 3.0, 2.0], NONE);
    b.cube(head, 0, (17.0, 0.0), [1.0, -19.0, -4.0], [3.0, 3.0, 2.0], NONE);
    let body = Fold::rot([FRAC_PI_2, 0.0, 0.0], [0.0, 5.0, 2.0]);
    b.cube_f(STATIC_PART, 0, (29.0, 0.0), [-6.0, -10.0, -7.0], [12.0, 18.0, 10.0], 0.0, false, &[body]);
    b.cube_f(STATIC_PART, 0, (45.0, 28.0), [-3.0, 0.0, 0.0], [8.0, 8.0, 3.0], 0.0, false, &[Fold::rot([0.0, FRAC_PI_2, 0.0], [-8.5, 3.0, 3.0])]);
    b.cube_f(STATIC_PART, 0, (45.0, 41.0), [-3.0, 0.0, 0.0], [8.0, 8.0, 3.0], 0.0, false, &[Fold::rot([0.0, FRAC_PI_2, 0.0], [5.5, 3.0, 3.0])]);
    let legs = [
        ([-3.5, 10.0, 6.0], Anim::QuadHindRight),
        ([3.5, 10.0, 6.0], Anim::QuadHindLeft),
        ([-3.5, 10.0, -5.0], Anim::QuadFrontRight),
        ([3.5, 10.0, -5.0], Anim::QuadFrontLeft),
    ];
    for (pivot, anim) in legs {
        let p = b.part(pivot, anim, 1.0);
        b.cube(p, 0, (29.0, 29.0), [-2.0, 0.0, -2.0], [4.0, 14.0, 4.0], NONE);
    }
    b.finish(1.0)
}

/// `ParrotModel`: standing pose — leaned body, folded wings, crest
/// feather, twin beak boxes.
fn parrot() -> Model {
    let mut b = ModelBuilder::new();
    b.cube_f(STATIC_PART, 0, (2.0, 8.0), [-1.5, 0.0, -1.5], [3.0, 6.0, 3.0], 0.0, false, &[Fold::rot([0.4937, 0.0, 0.0], [0.0, 16.5, -3.0])]);
    b.cube_f(STATIC_PART, 0, (22.0, 1.0), [-1.5, -1.0, -1.0], [3.0, 4.0, 1.0], 0.0, false, &[Fold::rot([1.015, 0.0, 0.0], [0.0, 21.07, 1.16])]);
    b.cube_f(STATIC_PART, 0, (19.0, 8.0), [-0.5, 0.0, -1.5], [1.0, 5.0, 3.0], 0.0, false, &[Fold::rot([-0.6981, -PI, 0.0], [1.5, 16.94, -2.76])]);
    b.cube_f(STATIC_PART, 0, (19.0, 8.0), [-0.5, 0.0, -1.5], [1.0, 5.0, 3.0], 0.0, false, &[Fold::rot([-0.6981, -PI, 0.0], [-1.5, 16.94, -2.76])]);
    let head = b.part([0.0, 15.69, -2.76], Anim::Head, 1.0);
    b.cube(head, 0, (2.0, 2.0), [-1.0, -1.5, -1.0], [2.0, 3.0, 2.0], NONE);
    b.cube_f(head, 0, (10.0, 0.0), [-1.0, -0.5, -2.0], [2.0, 1.0, 4.0], 0.0, false, &[Fold::at([0.0, -2.0, -1.0])]);
    b.cube_f(head, 0, (11.0, 7.0), [-0.5, -1.0, -0.5], [1.0, 2.0, 1.0], 0.0, false, &[Fold::at([0.0, -0.5, -1.5])]);
    b.cube_f(head, 0, (16.0, 7.0), [-0.5, 0.0, -0.5], [1.0, 2.0, 1.0], 0.0, false, &[Fold::at([0.0, -1.75, -2.45])]);
    b.cube_f(head, 0, (2.0, 18.0), [0.0, -4.0, -2.0], [0.0, 5.0, 4.0], 0.0, false, &[Fold::rot([-0.2214, 0.0, 0.0], [0.0, -2.15, 0.15])]);
    b.cube_f(STATIC_PART, 0, (14.0, 18.0), [-0.5, 0.0, -0.5], [1.0, 2.0, 1.0], 0.0, false, &[Fold::rot([-0.0299, 0.0, 0.0], [1.0, 22.0, -1.05])]);
    b.cube_f(STATIC_PART, 0, (14.0, 18.0), [-0.5, 0.0, -0.5], [1.0, 2.0, 1.0], 0.0, false, &[Fold::rot([-0.0299, 0.0, 0.0], [-1.0, 22.0, -1.05])]);
    b.finish(1.0)
}

/// `AbstractEquineModel` (horse family). `donkey_ears` swaps the horse
/// ears for the long donkey pair (vanilla `DonkeyModel`).
fn equine(donkey_ears: bool) -> Model {
    let mut b = ModelBuilder::new();
    let body = Fold::at([0.0, 11.0, 5.0]);
    b.cube_f(STATIC_PART, 0, (0.0, 32.0), [-5.0, -8.0, -17.0], [10.0, 10.0, 22.0], 0.05, false, &[body]);
    let neck = Fold::rot([PI / 6.0, 0.0, 0.0], [0.0, 4.0, -12.0]);
    b.cube_f(STATIC_PART, 0, (0.0, 35.0), [-2.05, -6.0, -2.0], [4.0, 12.0, 7.0], 0.0, false, &[neck]);
    b.cube_f(STATIC_PART, 0, (0.0, 13.0), [-3.0, -11.0, -2.0], [6.0, 5.0, 7.0], 0.0, false, &[neck]);
    b.cube_f(STATIC_PART, 0, (56.0, 36.0), [-1.0, -11.0, 5.01], [2.0, 16.0, 2.0], 0.0, false, &[neck]);
    b.cube_f(STATIC_PART, 0, (0.0, 25.0), [-2.0, -11.0, -7.0], [4.0, 5.0, 5.0], 0.0, false, &[neck]);
    if donkey_ears {
        b.cube_f(STATIC_PART, 0, (0.0, 12.0), [-1.0, -7.0, 0.0], [2.0, 7.0, 1.0], 0.0, false, &[Fold::rot([PI / 12.0, 0.0, PI / 12.0], [1.25, -10.0, 4.0]), neck]);
        b.cube_f(STATIC_PART, 0, (0.0, 12.0), [-1.0, -7.0, 0.0], [2.0, 7.0, 1.0], 0.0, false, &[Fold::rot([PI / 12.0, 0.0, -PI / 12.0], [-1.25, -10.0, 4.0]), neck]);
    } else {
        b.cube_f(STATIC_PART, 0, (19.0, 16.0), [0.55, -13.0, 4.0], [2.0, 3.0, 1.0], -0.001, false, &[neck]);
        b.cube_f(STATIC_PART, 0, (19.0, 16.0), [-2.55, -13.0, 4.0], [2.0, 3.0, 1.0], -0.001, false, &[neck]);
    }
    let legs: [([f32; 3], [f32; 3], bool, Anim); 4] = [
        ([-3.0, -1.01, -1.0], [4.0, 14.0, 7.0], true, Anim::QuadHindLeft),
        ([-1.0, -1.01, -1.0], [-4.0, 14.0, 7.0], false, Anim::QuadHindRight),
        ([-3.0, -1.01, -1.9], [4.0, 14.0, -10.0], true, Anim::QuadFrontLeft),
        ([-1.0, -1.01, -1.9], [-4.0, 14.0, -10.0], false, Anim::QuadFrontRight),
    ];
    for (min, pivot, mirror, anim) in legs {
        let p = b.part(pivot, anim, 1.0);
        b.cube_f(p, 0, (48.0, 21.0), min, [4.0, 11.0, 4.0], 0.0, mirror, NONE);
    }
    b.cube_f(STATIC_PART, 0, (42.0, 36.0), [-1.5, 0.0, 0.0], [3.0, 14.0, 4.0], 0.0, false, &[Fold::rot([PI / 6.0, 0.0, 0.0], [0.0, -5.0, 2.0]), body]);
    b.finish(1.0)
}

fn equine_plain() -> Model {
    equine(false)
}

fn equine_eared() -> Model {
    equine(true)
}

// ---------------------------------------------------------------------------
// Golems + allay
// ---------------------------------------------------------------------------

/// `SnowGolemModel`: three deflated snowballs + pumpkin head + stick arms.
fn snow_golem() -> Model {
    let mut b = ModelBuilder::new();
    let g = -0.5;
    let head = b.part([0.0, 4.0, 0.0], Anim::Head, 1.0);
    b.cube_g(head, 0, (0.0, 0.0), [-4.0, -8.0, -4.0], [8.0, 8.0, 8.0], g, NONE);
    b.cube_f(STATIC_PART, 0, (32.0, 0.0), [-1.0, 0.0, -1.0], [12.0, 2.0, 2.0], g, false, &[Fold::rot([0.0, 0.0, 1.0], [5.0, 6.0, 1.0])]);
    b.cube_f(STATIC_PART, 0, (32.0, 0.0), [-1.0, 0.0, -1.0], [12.0, 2.0, 2.0], g, false, &[Fold::rot([0.0, PI, -1.0], [-5.0, 6.0, -1.0])]);
    b.cube_f(STATIC_PART, 0, (0.0, 16.0), [-5.0, -10.0, -5.0], [10.0, 10.0, 10.0], g, false, &[Fold::at([0.0, 13.0, 0.0])]);
    b.cube_f(STATIC_PART, 0, (0.0, 36.0), [-6.0, -12.0, -6.0], [12.0, 12.0, 12.0], g, false, &[Fold::at([0.0, 24.0, 0.0])]);
    b.finish(1.0)
}

/// `IronGolemModel`: nosed head, massive body + skirt, 30-px arms, mirrored
/// legs.
fn iron_golem() -> Model {
    let mut b = ModelBuilder::new();
    let head = b.part([0.0, -7.0, -2.0], Anim::Head, 1.0);
    b.cube(head, 0, (0.0, 0.0), [-4.0, -12.0, -5.5], [8.0, 10.0, 8.0], NONE);
    b.cube(head, 0, (24.0, 0.0), [-1.0, -5.0, -7.5], [2.0, 4.0, 2.0], NONE);
    let body = Fold::at([0.0, -7.0, 0.0]);
    b.cube_f(STATIC_PART, 0, (0.0, 40.0), [-9.0, -2.0, -6.0], [18.0, 12.0, 11.0], 0.0, false, &[body]);
    b.cube_f(STATIC_PART, 0, (0.0, 70.0), [-4.5, 10.0, -3.0], [9.0, 5.0, 6.0], 0.5, false, &[body]);
    let arm_r = b.part([0.0, -7.0, 0.0], Anim::ArmRight, 0.5);
    b.cube(arm_r, 0, (60.0, 21.0), [-13.0, -2.5, -3.0], [4.0, 30.0, 6.0], NONE);
    let arm_l = b.part([0.0, -7.0, 0.0], Anim::ArmLeft, 0.5);
    b.cube(arm_l, 0, (60.0, 58.0), [9.0, -2.5, -3.0], [4.0, 30.0, 6.0], NONE);
    let leg_r = b.part([-4.0, 11.0, 0.0], Anim::LegRight, 1.0);
    b.cube(leg_r, 0, (37.0, 0.0), [-3.5, -3.0, -3.0], [6.0, 16.0, 5.0], NONE);
    let leg_l = b.part([5.0, 11.0, 0.0], Anim::LegLeft, 1.0);
    b.cube_m(leg_l, 0, (60.0, 0.0), [-3.5, -3.0, -3.0], [6.0, 16.0, 5.0], NONE);
    b.finish(1.0)
}

/// `AllayModel`: the vex's friendly cousin — same body plan, upright wings.
fn allay() -> Model {
    let mut b = ModelBuilder::new();
    let root = Fold::at([0.0, 23.5, 0.0]);
    let head = b.part([0.0, 19.51, 0.0], Anim::Head, 1.0);
    b.cube(head, 0, (0.0, 0.0), [-2.5, -5.0, -2.5], [5.0, 5.0, 5.0], NONE);
    let body = Fold::at([0.0, -4.0, 0.0]);
    b.cube_f(STATIC_PART, 0, (0.0, 10.0), [-1.5, 0.0, -1.0], [3.0, 4.0, 2.0], 0.0, false, &[body, root]);
    b.cube_f(STATIC_PART, 0, (0.0, 16.0), [-1.5, 0.0, -1.0], [3.0, 5.0, 2.0], -0.2, false, &[body, root]);
    b.cube_f(STATIC_PART, 0, (23.0, 0.0), [-0.75, -0.5, -1.0], [1.0, 4.0, 2.0], -0.01, false, &[Fold::at([-1.75, 0.5, 0.0]), body, root]);
    b.cube_f(STATIC_PART, 0, (23.0, 6.0), [-0.25, -0.5, -1.0], [1.0, 4.0, 2.0], -0.01, false, &[Fold::at([1.75, 0.5, 0.0]), body, root]);
    b.cube_f(STATIC_PART, 0, (16.0, 14.0), [0.0, 1.0, 0.0], [0.0, 5.0, 8.0], 0.0, false, &[Fold::at([0.5, 0.0, 0.6]), body, root]);
    b.cube_f(STATIC_PART, 0, (16.0, 14.0), [0.0, 1.0, 0.0], [0.0, 5.0, 8.0], 0.0, false, &[Fold::at([-0.5, 0.0, 0.6]), body, root]);
    b.finish(1.0)
}

// ---------------------------------------------------------------------------
// The last holdouts — bosses, deep-dark, 26.x newcomers
// ---------------------------------------------------------------------------

/// `WardenModel` (128²): huge ribbed body, tendril plates on the skull,
/// 28-px arms, stubby legs.
fn warden() -> Model {
    let mut b = ModelBuilder::new();
    let bone = Fold::at([0.0, 24.0, 0.0]);
    let body = Fold::at([0.0, -21.0, 0.0]);
    b.cube_f(STATIC_PART, 0, (0.0, 0.0), [-9.0, -13.0, -4.0], [18.0, 21.0, 11.0], 0.0, false, &[body, bone]);
    b.cube_f(STATIC_PART, 0, (90.0, 11.0), [-2.0, -11.0, -0.1], [9.0, 21.0, 0.0], 0.0, false, &[Fold::at([-7.0, -2.0, -4.0]), body, bone]);
    b.cube_f(STATIC_PART, 0, (90.0, 11.0), [-7.0, -11.0, -0.1], [9.0, 21.0, 0.0], 0.0, true, &[Fold::at([7.0, -2.0, -4.0]), body, bone]);
    // Head chain: bone → body → head ⇒ absolute pivot (0, −10, 0).
    let head = b.part([0.0, -10.0, 0.0], Anim::Head, 1.0);
    b.cube(head, 0, (0.0, 32.0), [-8.0, -16.0, -5.0], [16.0, 16.0, 10.0], NONE);
    b.cube_f(head, 0, (52.0, 32.0), [-16.0, -13.0, 0.0], [16.0, 16.0, 0.0], 0.0, false, &[Fold::at([-8.0, -12.0, 0.0])]);
    b.cube_f(head, 0, (58.0, 0.0), [0.0, -13.0, 0.0], [16.0, 16.0, 0.0], 0.0, false, &[Fold::at([8.0, -12.0, 0.0])]);
    let arm_r = b.part([-13.0, -10.0, 1.0], Anim::ArmRight, 1.0);
    b.cube(arm_r, 0, (44.0, 50.0), [-4.0, 0.0, -4.0], [8.0, 28.0, 8.0], NONE);
    let arm_l = b.part([13.0, -10.0, 1.0], Anim::ArmLeft, 1.0);
    b.cube(arm_l, 0, (0.0, 58.0), [-4.0, 0.0, -4.0], [8.0, 28.0, 8.0], NONE);
    let leg_r = b.part([-5.9, 11.0, 0.0], Anim::LegRight, 1.0);
    b.cube(leg_r, 0, (76.0, 48.0), [-3.1, 0.0, -3.0], [6.0, 13.0, 6.0], NONE);
    let leg_l = b.part([5.9, 11.0, 0.0], Anim::LegLeft, 1.0);
    b.cube(leg_l, 0, (76.0, 76.0), [-2.9, 0.0, -3.0], [6.0, 13.0, 6.0], NONE);
    b.finish(1.0)
}

/// `SnifferModel` (192²): moss-backed barrel body on six legs, droopy head.
fn sniffer() -> Model {
    let mut b = ModelBuilder::new();
    let bone = Fold::at([0.0, 5.0, 0.0]);
    b.cube_f(STATIC_PART, 0, (62.0, 68.0), [-12.5, -14.0, -20.0], [25.0, 29.0, 40.0], 0.0, false, &[bone]);
    b.cube_f(STATIC_PART, 0, (62.0, 0.0), [-12.5, -14.0, -20.0], [25.0, 24.0, 40.0], 0.5, false, &[bone]);
    b.cube_f(STATIC_PART, 0, (87.0, 68.0), [-12.5, 12.0, -20.0], [25.0, 0.0, 40.0], 0.0, false, &[bone]);
    let legs = [
        ((32.0, 87.0), [-7.5, 15.0, -15.0], Anim::QuadFrontRight),
        ((32.0, 105.0), [-7.5, 15.0, 0.0], Anim::None),
        ((32.0, 123.0), [-7.5, 15.0, 15.0], Anim::QuadHindRight),
        ((0.0, 87.0), [7.5, 15.0, -15.0], Anim::QuadFrontLeft),
        ((0.0, 105.0), [7.5, 15.0, 0.0], Anim::None),
        ((0.0, 123.0), [7.5, 15.0, 15.0], Anim::QuadHindLeft),
    ];
    for (uv, pivot, anim) in legs {
        if anim == Anim::None {
            b.cube_f(STATIC_PART, 0, uv, [-3.5, -1.0, -4.0], [7.0, 10.0, 8.0], 0.0, false, &[Fold::at(pivot)]);
        } else {
            let p = b.part(pivot, anim, 1.0);
            b.cube(p, 0, uv, [-3.5, -1.0, -4.0], [7.0, 10.0, 8.0], NONE);
        }
    }
    // Head: bone → body → head ⇒ absolute pivot (0, 11.5, −19.48).
    let head = b.part([0.0, 11.5, -19.48], Anim::Head, 1.0);
    b.cube(head, 0, (8.0, 15.0), [-6.5, -7.5, -11.5], [13.0, 18.0, 11.0], NONE);
    b.cube(head, 0, (8.0, 4.0), [-6.5, 7.5, -11.5], [13.0, 0.0, 11.0], NONE);
    b.cube_f(head, 0, (2.0, 0.0), [0.0, 0.0, -3.0], [1.0, 19.0, 7.0], 0.0, false, &[Fold::at([6.51, -7.5, -4.51])]);
    b.cube_f(head, 0, (48.0, 0.0), [-1.0, 0.0, -3.0], [1.0, 19.0, 7.0], 0.0, false, &[Fold::at([-6.51, -7.5, -4.51])]);
    b.cube_f(head, 0, (10.0, 45.0), [-6.5, -2.0, -9.0], [13.0, 2.0, 9.0], 0.0, false, &[Fold::at([0.0, -4.5, -11.5])]);
    b.finish(1.0)
}

/// `BreezeModel`: head + three orbiting rods on breeze.png, the swirling
/// wind funnel on breeze_wind.png (texture 1).
fn breeze() -> Model {
    let mut b = ModelBuilder::new();
    let rods = Fold::at([0.0, 8.0, 0.0]);
    let rod_poses: [([f32; 3], [f32; 3]); 3] = [
        ([2.5981, -3.0, 1.5], [-2.7489, -1.0472, 3.1416]),
        ([-2.5981, -3.0, 1.5], [-2.7489, 1.0472, 3.1416]),
        ([0.0, -3.0, -3.0], [0.3927, 0.0, 0.0]),
    ];
    for (off, rot) in rod_poses {
        b.cube_f(STATIC_PART, 0, (0.0, 17.0), [-1.0, 0.0, -3.0], [2.0, 8.0, 2.0], 0.0, false, &[Fold::rot(rot, off), rods]);
    }
    let head = b.part([0.0, 4.0, 0.0], Anim::Head, 1.0);
    b.cube(head, 0, (4.0, 24.0), [-5.0, -5.0, -4.2], [10.0, 3.0, 4.0], NONE);
    b.cube(head, 0, (0.0, 0.0), [-4.0, -8.0, -4.0], [8.0, 8.0, 8.0], NONE);
    // Wind funnel (three stacked swirl tiers, widest at the top).
    let wb = Fold::at([0.0, 24.0, 0.0]);
    b.cube_f(STATIC_PART, 1, (1.0, 83.0), [-2.5, -7.0, -2.5], [5.0, 7.0, 5.0], 0.0, false, &[wb]);
    let wm = Fold::at([0.0, -7.0, 0.0]);
    b.cube_f(STATIC_PART, 1, (74.0, 28.0), [-6.0, -6.0, -6.0], [12.0, 6.0, 12.0], 0.0, false, &[wm, wb]);
    b.cube_f(STATIC_PART, 1, (78.0, 32.0), [-4.0, -6.0, -4.0], [8.0, 6.0, 8.0], 0.0, false, &[wm, wb]);
    b.cube_f(STATIC_PART, 1, (49.0, 71.0), [-2.5, -6.0, -2.5], [5.0, 6.0, 5.0], 0.0, false, &[wm, wb]);
    let wt = Fold::at([0.0, -6.0, 0.0]);
    b.cube_f(STATIC_PART, 1, (0.0, 0.0), [-9.0, -8.0, -9.0], [18.0, 8.0, 18.0], 0.0, false, &[wt, wm, wb]);
    b.cube_f(STATIC_PART, 1, (6.0, 6.0), [-6.0, -8.0, -6.0], [12.0, 8.0, 12.0], 0.0, false, &[wt, wm, wb]);
    b.cube_f(STATIC_PART, 1, (105.0, 57.0), [-2.5, -8.0, -2.5], [5.0, 8.0, 5.0], 0.0, false, &[wt, wm, wb]);
    b.finish(1.0)
}

/// `CreakingModel`: gnarled tree humanoid — twig-crowned head, split-log
/// body, branch arms, root legs with 0-thick foot fans.
fn creaking() -> Model {
    let mut b = ModelBuilder::new();
    let root = Fold::at([0.0, 24.0, 0.0]);
    let upper = Fold::at([-1.0, -19.0, 0.0]);
    // Head chain: root → upper_body → head ⇒ pivot (−4, −6, 0).
    let head = b.part([-4.0, -6.0, 0.0], Anim::Head, 1.0);
    b.cube(head, 0, (0.0, 0.0), [-3.0, -10.0, -3.0], [6.0, 10.0, 6.0], NONE);
    b.cube(head, 0, (28.0, 31.0), [-3.0, -13.0, -3.0], [6.0, 3.0, 6.0], NONE);
    b.cube(head, 0, (12.0, 40.0), [3.0, -13.0, 0.0], [9.0, 14.0, 0.0], NONE);
    b.cube(head, 0, (34.0, 12.0), [-12.0, -14.0, 0.0], [9.0, 14.0, 0.0], NONE);
    let body = Fold::at([0.0, -7.0, 1.0]);
    b.cube_f(STATIC_PART, 0, (0.0, 16.0), [0.0, -3.0, -3.0], [6.0, 13.0, 5.0], 0.0, false, &[body, upper, root]);
    b.cube_f(STATIC_PART, 0, (24.0, 0.0), [-6.0, -4.0, -3.0], [6.0, 7.0, 5.0], 0.0, false, &[body, upper, root]);
    let arm_r = Fold::at([-7.0, -9.5, 1.5]);
    b.cube_f(STATIC_PART, 0, (22.0, 13.0), [-2.0, -1.5, -1.5], [3.0, 21.0, 3.0], 0.0, false, &[arm_r, upper, root]);
    b.cube_f(STATIC_PART, 0, (46.0, 0.0), [-2.0, 19.5, -1.5], [3.0, 4.0, 3.0], 0.0, false, &[arm_r, upper, root]);
    let arm_l = Fold::at([6.0, -9.0, 0.5]);
    b.cube_f(STATIC_PART, 0, (30.0, 40.0), [0.0, -1.0, -1.5], [3.0, 16.0, 3.0], 0.0, false, &[arm_l, upper, root]);
    b.cube_f(STATIC_PART, 0, (52.0, 12.0), [0.0, -5.0, -1.5], [3.0, 4.0, 3.0], 0.0, false, &[arm_l, upper, root]);
    b.cube_f(STATIC_PART, 0, (52.0, 19.0), [0.0, 15.0, -1.5], [3.0, 4.0, 3.0], 0.0, false, &[arm_l, upper, root]);
    let leg_l = b.part([1.5, 8.0, 0.5], Anim::LegLeft, 1.0);
    b.cube(leg_l, 0, (42.0, 40.0), [-1.5, 0.0, -1.5], [3.0, 16.0, 3.0], NONE);
    b.cube(leg_l, 0, (45.0, 55.0), [-1.5, 15.7, -4.5], [5.0, 0.0, 9.0], NONE);
    let leg_r = b.part([-1.0, 6.5, 0.5], Anim::LegRight, 1.0);
    b.cube(leg_r, 0, (0.0, 34.0), [-3.0, -1.5, -1.5], [3.0, 19.0, 3.0], NONE);
    b.cube(leg_r, 0, (45.0, 46.0), [-5.0, 17.2, -4.5], [5.0, 0.0, 9.0], NONE);
    b.cube(leg_r, 0, (12.0, 34.0), [-3.0, -4.5, -1.5], [3.0, 3.0, 3.0], NONE);
    b.finish(1.0)
}

/// `RavagerModel` (128²): armored bull — horned head on a thick neck,
/// rotated slab body, 37-px legs.
fn ravager() -> Model {
    let mut b = ModelBuilder::new();
    let neck = Fold::at([0.0, -7.0, 5.5]);
    b.cube_f(STATIC_PART, 0, (68.0, 73.0), [-5.0, -1.0, -18.0], [10.0, 10.0, 18.0], 0.0, false, &[neck]);
    // Head: neck child at (0, 16, −17) ⇒ absolute pivot (0, 9, −11.5).
    let head = b.part([0.0, 9.0, -11.5], Anim::Head, 1.0);
    b.cube(head, 0, (0.0, 0.0), [-8.0, -20.0, -14.0], [16.0, 20.0, 16.0], NONE);
    b.cube(head, 0, (0.0, 0.0), [-2.0, -6.0, -18.0], [4.0, 8.0, 4.0], NONE);
    b.cube_f(head, 0, (74.0, 55.0), [0.0, -14.0, -2.0], [2.0, 14.0, 4.0], 0.0, false, &[Fold::rot([1.0995574, 0.0, 0.0], [-10.0, -14.0, -8.0])]);
    b.cube_f(head, 0, (74.0, 55.0), [0.0, -14.0, -2.0], [2.0, 14.0, 4.0], 0.0, true, &[Fold::rot([1.0995574, 0.0, 0.0], [8.0, -14.0, -8.0])]);
    b.cube_f(head, 0, (0.0, 36.0), [-8.0, 0.0, -16.0], [16.0, 3.0, 16.0], 0.0, false, &[Fold::at([0.0, -2.0, 2.0])]);
    let body = Fold::rot([FRAC_PI_2, 0.0, 0.0], [0.0, 1.0, 2.0]);
    b.cube_f(STATIC_PART, 0, (0.0, 55.0), [-7.0, -10.0, -7.0], [14.0, 16.0, 20.0], 0.0, false, &[body]);
    b.cube_f(STATIC_PART, 0, (0.0, 91.0), [-6.0, 6.0, -7.0], [12.0, 13.0, 18.0], 0.0, false, &[body]);
    let legs = [
        ((96.0, 0.0), [-8.0, -13.0, 18.0], false, Anim::QuadHindRight),
        ((96.0, 0.0), [8.0, -13.0, 18.0], true, Anim::QuadHindLeft),
        ((64.0, 0.0), [-8.0, -13.0, -5.0], false, Anim::QuadFrontRight),
        ((64.0, 0.0), [8.0, -13.0, -5.0], true, Anim::QuadFrontLeft),
    ];
    for (uv, pivot, mirror, anim) in legs {
        let p = b.part(pivot, anim, 1.0);
        b.cube_f(p, 0, uv, [-4.0, 0.0, -4.0], [8.0, 37.0, 8.0], 0.0, mirror, NONE);
    }
    b.finish(1.0)
}

/// `WitherBossModel`: three skulls over a floating spine + ribs + tail.
fn wither() -> Model {
    let mut b = ModelBuilder::new();
    b.cube(STATIC_PART, 0, (0.0, 16.0), [-10.0, 3.9, -0.5], [20.0, 3.0, 3.0], NONE);
    let spine = Fold::rot([0.20420352, 0.0, 0.0], [-2.0, 6.9, -0.5]);
    b.cube_f(STATIC_PART, 0, (0.0, 22.0), [0.0, 0.0, 0.0], [3.0, 10.0, 3.0], 0.0, false, &[spine]);
    b.cube_f(STATIC_PART, 0, (24.0, 22.0), [-4.0, 1.5, 0.5], [11.0, 2.0, 2.0], 0.0, false, &[spine]);
    b.cube_f(STATIC_PART, 0, (24.0, 22.0), [-4.0, 4.0, 0.5], [11.0, 2.0, 2.0], 0.0, false, &[spine]);
    b.cube_f(STATIC_PART, 0, (24.0, 22.0), [-4.0, 6.5, 0.5], [11.0, 2.0, 2.0], 0.0, false, &[spine]);
    // Tail pose: vanilla computes it from the spine angle.
    let a = 0.20420352f32;
    let tail = Fold::rot([0.83252203, 0.0, 0.0], [-2.0, 6.9 + a.cos() * 10.0, -0.5 + a.sin() * 10.0]);
    b.cube_f(STATIC_PART, 0, (12.0, 22.0), [0.0, 0.0, 0.0], [3.0, 6.0, 3.0], 0.0, false, &[tail]);
    let head = b.part([0.0, 0.0, 0.0], Anim::Head, 1.0);
    b.cube(head, 0, (0.0, 0.0), [-4.0, -4.0, -4.0], [8.0, 8.0, 8.0], NONE);
    b.cube_f(STATIC_PART, 0, (32.0, 0.0), [-4.0, -4.0, -4.0], [6.0, 6.0, 6.0], 0.0, false, &[Fold::at([-8.0, 4.0, 0.0])]);
    b.cube_f(STATIC_PART, 0, (32.0, 0.0), [-4.0, -4.0, -4.0], [6.0, 6.0, 6.0], 0.0, false, &[Fold::at([10.0, 4.0, 0.0])]);
    b.finish(1.0)
}

/// `EnderDragonModel` (256²): the full dragon — head + jaw, 5 neck and 12
/// tail spine segments, 24×24×64 body, two-segment wings with 0-thick
/// membrane skins, three-segment legs. Static pose (vanilla's animation is
/// procedural flight).
fn ender_dragon() -> Model {
    let mut b = ModelBuilder::new();
    // Head (+ jaw folded in).
    let head = b.part([0.0, 20.0, -62.0], Anim::Head, 1.0);
    b.cube(head, 0, (176.0, 44.0), [-6.0, -1.0, -24.0], [12.0, 5.0, 16.0], NONE);
    b.cube(head, 0, (112.0, 30.0), [-8.0, -8.0, -10.0], [16.0, 16.0, 16.0], NONE);
    b.cube_m(head, 0, (0.0, 0.0), [-5.0, -12.0, -4.0], [2.0, 4.0, 6.0], NONE);
    b.cube_m(head, 0, (112.0, 0.0), [-5.0, -3.0, -22.0], [2.0, 2.0, 4.0], NONE);
    b.cube_m(head, 0, (0.0, 0.0), [3.0, -12.0, -4.0], [2.0, 4.0, 6.0], NONE);
    b.cube_m(head, 0, (112.0, 0.0), [3.0, -3.0, -22.0], [2.0, 2.0, 4.0], NONE);
    b.cube_f(head, 0, (176.0, 65.0), [-6.0, 0.0, -16.0], [12.0, 4.0, 16.0], 0.0, false, &[Fold::at([0.0, 4.0, -8.0])]);
    // Neck + tail spine segments (10³ box + top scale).
    let mut spine = |off: [f32; 3]| {
        b.cube_f(STATIC_PART, 0, (192.0, 104.0), [-5.0, -5.0, -5.0], [10.0, 10.0, 10.0], 0.0, false, &[Fold::at(off)]);
        b.cube_f(STATIC_PART, 0, (48.0, 0.0), [-1.0, -9.0, -3.0], [2.0, 4.0, 6.0], 0.0, false, &[Fold::at(off)]);
    };
    for i in 0..5 {
        spine([0.0, 20.0, -12.0 - i as f32 * 10.0]);
    }
    for i in 0..12 {
        spine([0.0, 10.0, 60.0 + i as f32 * 10.0]);
    }
    let body = Fold::at([0.0, 3.0, 8.0]);
    b.cube_f(STATIC_PART, 0, (0.0, 0.0), [-12.0, 1.0, -16.0], [24.0, 24.0, 64.0], 0.0, false, &[body]);
    for z in [-10.0, 10.0, 30.0] {
        b.cube_f(STATIC_PART, 0, (220.0, 53.0), [-1.0, -5.0, z], [2.0, 6.0, 12.0], 0.0, false, &[body]);
    }
    // Wings: bone + membrane, tip child; the right side mirrors geometry.
    let wl = Fold::at([12.0, 2.0, -6.0]);
    b.cube_f(STATIC_PART, 0, (112.0, 88.0), [0.0, -4.0, -4.0], [56.0, 8.0, 8.0], 0.0, true, &[wl, body]);
    b.cube_f(STATIC_PART, 0, (-56.0, 88.0), [0.0, 0.0, 2.0], [56.0, 0.0, 56.0], 0.0, true, &[wl, body]);
    let wlt = Fold::at([56.0, 0.0, 0.0]);
    b.cube_f(STATIC_PART, 0, (112.0, 136.0), [0.0, -2.0, -2.0], [56.0, 4.0, 4.0], 0.0, true, &[wlt, wl, body]);
    b.cube_f(STATIC_PART, 0, (-56.0, 144.0), [0.0, 0.0, 2.0], [56.0, 0.0, 56.0], 0.0, true, &[wlt, wl, body]);
    let wr = Fold::at([-12.0, 2.0, -6.0]);
    b.cube_f(STATIC_PART, 0, (112.0, 88.0), [-56.0, -4.0, -4.0], [56.0, 8.0, 8.0], 0.0, false, &[wr, body]);
    b.cube_f(STATIC_PART, 0, (-56.0, 88.0), [-56.0, 0.0, 2.0], [56.0, 0.0, 56.0], 0.0, false, &[wr, body]);
    let wrt = Fold::at([-56.0, 0.0, 0.0]);
    b.cube_f(STATIC_PART, 0, (112.0, 136.0), [-56.0, -2.0, -2.0], [56.0, 4.0, 4.0], 0.0, false, &[wrt, wr, body]);
    b.cube_f(STATIC_PART, 0, (-56.0, 144.0), [-56.0, 0.0, 2.0], [56.0, 0.0, 56.0], 0.0, false, &[wrt, wr, body]);
    // Legs: (upper, tip, foot) chains per side.
    let mut leg = |sx: f32| {
        let front = Fold::rot([1.3, 0.0, 0.0], [12.0 * sx, 17.0, -6.0]);
        b.cube_f(STATIC_PART, 0, (112.0, 104.0), [-4.0, -4.0, -4.0], [8.0, 24.0, 8.0], 0.0, false, &[front, body]);
        let ftip = Fold::rot([-0.5, 0.0, 0.0], [0.0, 20.0, -1.0]);
        b.cube_f(STATIC_PART, 0, (226.0, 138.0), [-3.0, -1.0, -3.0], [6.0, 24.0, 6.0], 0.0, false, &[ftip, front, body]);
        b.cube_f(STATIC_PART, 0, (144.0, 104.0), [-4.0, 0.0, -12.0], [8.0, 4.0, 16.0], 0.0, false, &[Fold::rot([0.75, 0.0, 0.0], [0.0, 23.0, 0.0]), ftip, front, body]);
        let rear = Fold::rot([1.0, 0.0, 0.0], [16.0 * sx, 13.0, 34.0]);
        b.cube_f(STATIC_PART, 0, (0.0, 0.0), [-8.0, -4.0, -8.0], [16.0, 32.0, 16.0], 0.0, false, &[rear, body]);
        let rtip = Fold::rot([0.5, 0.0, 0.0], [0.0, 32.0, -4.0]);
        b.cube_f(STATIC_PART, 0, (196.0, 0.0), [-6.0, -2.0, 0.0], [12.0, 32.0, 12.0], 0.0, false, &[rtip, rear, body]);
        b.cube_f(STATIC_PART, 0, (112.0, 0.0), [-9.0, 0.0, -20.0], [18.0, 6.0, 24.0], 0.0, false, &[Fold::rot([0.75, 0.0, 0.0], [0.0, 31.0, 4.0]), rtip, rear, body]);
    };
    leg(1.0);
    leg(-1.0);
    b.finish(1.0)
}

/// `HappyGhastModel`: the tame ghast — body + big inner cube, nine short
/// dangling legs. Mesh-scaled 4.0 (ground-anchored ≡ our scale).
fn happy_ghast() -> Model {
    let mut b = ModelBuilder::new();
    let body = Fold::at([0.0, 16.0, 0.0]);
    b.cube_f(STATIC_PART, 0, (0.0, 0.0), [-8.0, -8.0, -8.0], [16.0, 16.0, 16.0], 0.0, false, &[body]);
    b.cube_f(STATIC_PART, 0, (0.0, 32.0), [-8.0, -16.0, -8.0], [16.0, 16.0, 16.0], -0.5, false, &[Fold::at([0.0, 8.0, 0.0]), body]);
    let legs: [([f32; 3], f32); 9] = [
        ([-3.75, 7.0, -5.0], 5.0),
        ([1.25, 7.0, -5.0], 7.0),
        ([6.25, 7.0, -5.0], 4.0),
        ([-6.25, 7.0, 0.0], 5.0),
        ([-1.25, 7.0, 0.0], 5.0),
        ([3.75, 7.0, 0.0], 7.0),
        ([-3.75, 7.0, 5.0], 8.0),
        ([1.25, 7.0, 5.0], 8.0),
        ([6.25, 7.0, 5.0], 5.0),
    ];
    for (off, len) in legs {
        b.cube_f(STATIC_PART, 0, (0.0, 0.0), [-1.0, 0.0, -1.0], [2.0, len, 2.0], 0.0, false, &[Fold::at(off), body]);
    }
    b.finish(4.0)
}

/// `CopperGolemModel`: the little statue — antennaed head, slab body,
/// mitten arms, stub legs. The whole mesh rides a +24 root translate.
fn copper_golem() -> Model {
    let mut b = ModelBuilder::new();
    let root = Fold::at([0.0, 24.0, 0.0]);
    let body = Fold::at([0.0, -5.0, 0.0]);
    b.cube_f(STATIC_PART, 0, (0.0, 15.0), [-4.0, -6.0, -3.0], [8.0, 6.0, 6.0], 0.0, false, &[body, root]);
    // Head chain: root → body → head ⇒ pivot (0, 13, 0).
    let head = b.part([0.0, 13.0, 0.0], Anim::Head, 1.0);
    b.cube_g(head, 0, (0.0, 0.0), [-4.0, -5.0, -5.0], [8.0, 5.0, 10.0], 0.015, NONE);
    b.cube(head, 0, (56.0, 0.0), [-1.0, -2.0, -6.0], [2.0, 3.0, 2.0], NONE);
    b.cube_g(head, 0, (37.0, 8.0), [-1.0, -9.0, -1.0], [2.0, 4.0, 2.0], -0.015, NONE);
    b.cube_g(head, 0, (37.0, 0.0), [-2.0, -13.0, -2.0], [4.0, 4.0, 4.0], -0.015, NONE);
    let arm_r = b.part([-4.0, 13.0, 0.0], Anim::ArmRight, 1.0);
    b.cube(arm_r, 0, (36.0, 16.0), [-3.0, -1.0, -2.0], [3.0, 10.0, 4.0], NONE);
    let arm_l = b.part([4.0, 13.0, 0.0], Anim::ArmLeft, 1.0);
    b.cube(arm_l, 0, (50.0, 16.0), [0.0, -1.0, -2.0], [3.0, 10.0, 4.0], NONE);
    let leg_r = b.part([0.0, 19.0, 0.0], Anim::LegRight, 1.0);
    b.cube(leg_r, 0, (0.0, 27.0), [-4.0, 0.0, -2.0], [4.0, 5.0, 4.0], NONE);
    let leg_l = b.part([0.0, 19.0, 0.0], Anim::LegLeft, 1.0);
    b.cube(leg_l, 0, (16.0, 27.0), [0.0, 0.0, -2.0], [4.0, 5.0, 4.0], NONE);
    b.finish(1.0)
}

/// `NautilusModel` (128²): spiral shell + soft body with layered tentacle
/// slabs. The zombie variant shares the mesh.
fn nautilus() -> Model {
    let mut b = ModelBuilder::new();
    let root = Fold::at([0.0, 29.0, -6.0]);
    let shell = Fold::at([0.0, -13.0, 5.0]);
    b.cube_f(STATIC_PART, 0, (0.0, 0.0), [-7.0, -10.0, -7.0], [14.0, 10.0, 16.0], 0.0, false, &[shell, root]);
    b.cube_f(STATIC_PART, 0, (0.0, 26.0), [-7.0, 0.0, -7.0], [14.0, 8.0, 20.0], 0.0, false, &[shell, root]);
    b.cube_f(STATIC_PART, 0, (48.0, 26.0), [-7.0, 0.0, 6.0], [14.0, 8.0, 0.0], 0.0, false, &[shell, root]);
    let body = Fold::at([0.0, -8.5, 12.3]);
    b.cube_f(STATIC_PART, 0, (0.0, 54.0), [-5.0, -4.51, -3.0], [10.0, 8.0, 14.0], 0.0, false, &[body, root]);
    b.cube_f(STATIC_PART, 0, (0.0, 76.0), [-5.0, -4.51, 7.0], [10.0, 8.0, 0.0], 0.0, false, &[body, root]);
    b.cube_f(STATIC_PART, 0, (54.0, 54.0), [-5.0, -2.0, 0.0], [10.0, 4.0, 4.0], -0.001, false, &[Fold::at([0.0, -2.51, 7.0]), body, root]);
    b.cube_f(STATIC_PART, 0, (54.0, 70.0), [-3.0, -2.0, -0.5], [6.0, 4.0, 4.0], 0.0, false, &[Fold::at([0.0, -0.51, 7.5]), body, root]);
    b.cube_f(STATIC_PART, 0, (54.0, 62.0), [-5.0, -1.98, 0.0], [10.0, 4.0, 4.0], -0.001, false, &[Fold::at([0.0, 1.49, 7.0]), body, root]);
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
        assert_eq!(kind_for_entity_name("minecraft:magma_cube"), EntityModelKind::MagmaCube);
        assert_eq!(kind_for_entity_name("minecraft:wither_skeleton"), EntityModelKind::WitherSkeleton);
        assert_eq!(kind_for_entity_name("minecraft:warden"), EntityModelKind::Warden);
        assert_eq!(kind_for_entity_name("minecraft:armor_stand"), EntityModelKind::Capsule);
        // Every registry def's kind is reachable from its own wire name.
        for def in MOBS {
            if def.kind != EntityModelKind::Player {
                assert_eq!(
                    kind_for_entity_name(&format!("minecraft:{}", def.kind.name())),
                    def.kind,
                    "wire mapping for {:?}",
                    def.kind
                );
            }
        }
    }
}
