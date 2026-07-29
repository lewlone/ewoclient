//! The crosshair entity pick — what entity are you looking at? (M73)
//!
//! M70 shipped the entity-label visibility ladder with one clause it could not
//! evaluate: `EntityRenderer.shouldShowName` is
//! `entity.shouldShowName() || (hasCustomName() && entity ==
//! crosshairPickEntity)`, and Rewo's raycast was voxel-only, so the second
//! disjunct was transcribed, driven both ways by the gate and fed a hard
//! `false` live. This module is the answer to it.
//!
//! Transcribed from the 26.2 decompile:
//! `net/minecraft/client/Minecraft.java` (`pick`),
//! `net/minecraft/client/player/LocalPlayer.java` (`raycastHitResult`, the
//! private static `pick`, `filterHitResult`),
//! `net/minecraft/world/entity/projectile/ProjectileUtil.java`
//! (`getEntityHitResult`, the six-argument pick overload),
//! `net/minecraft/world/phys/AABB.java` (`clip`, `contains`, `inflate`,
//! `expandTowards`, `intersects`),
//! `net/minecraft/world/phys/Vec3.java` (`closerThan`),
//! `net/minecraft/world/entity/EntityDimensions.java` (`makeBoundingBox`) and
//! `net/minecraft/world/entity/ai/attributes/Attributes.java` (the two
//! interaction ranges).
//!
//! # The shape of it
//!
//! `Minecraft.pick` assigns `crosshairPickEntity` from
//! `player.raycastHitResult(partial, cameraEntity)`, which is *the*
//! `hitResult` — the same one that decides which block you are mining. There
//! is no second, label-only pick.
//!
//! ```text
//! maxDistance   = max(blockInteractionRange, entityInteractionRange)
//! blockHit      = cameraEntity.pick(maxDistance)     // the voxel ray
//! blockDistSq   = |blockHit.location - eye|²         // = maxDistance² on a MISS
//! if blockHit hit: maxDistance = sqrt(blockDistSq)   // truncate the entity ray
//! entityHit     = getEntityHitResult(eye, eye + dir*maxDistance, …, maxDistance²)
//! return entityHit != null && entityHit.distSq < blockDistSq
//!          ? filter(entityHit,  eye, entityInteractionRange)
//!          : filter(blockHit,   eye, blockInteractionRange)
//! ```
//!
//! **A block in front of a mob wins twice over, and the redundancy is real.**
//! The entity ray is *truncated* at the block hit, so a mob behind a wall is
//! never swept; and the surviving hit is then compared against the block
//! anyway. Measured rather than assumed (`labelshot` g5): **neither half alone
//! is observable** — remove either and every answer is unchanged, because the
//! truncation feeds `getEntityHitResult`'s `maxValue`, whose `dd < nearest` is
//! itself strict. So a dead heat is already excluded by the sweep bound, and
//! `entityHit.distSq < blockDistSq` is unreachable for an ordinary candidate.
//! The one path that can reach it is the same-root-vehicle arm below, which
//! assigns `hovered` after an inside-pick *without* consulting `maxValue` at
//! all. Both are transcribed because both are in the source; only their
//! conjunction is testable.
//!
//! **The range that bounds the sweep is the larger of the two**, not the
//! entity range. `getEntityHitResult`'s `maxValue` is `maxDistanceSq` =
//! `max(block, entity)²`, and the entity range is applied afterwards by
//! `filterHitResult`, which turns an over-range entity hit into a
//! `BlockHitResult.miss` — and a miss carries no entity, so
//! `crosshairPickEntity` becomes null. Collapsing the two steps into "sweep to
//! the entity range" gives the same answer here but is a different program:
//! with `blockInteractionRange` (4.5) above `entityInteractionRange` (3.0), a
//! mob at 4 blocks is found, measured, and *then* discarded.
//!
//! # The inflation, and the tie-break
//!
//! The candidate box is `entity.getBoundingBox().inflate(entity.getPickRadius())`.
//! **`getPickRadius()` is `0.0F` for every entity except a `Projectile`**,
//! which returns `isPickable() ? 1.0F : 0.0F`. It is emphatically *not* the
//! `DEFAULT_ENTITY_HIT_RESULT_MARGIN = 0.3F` next to it in the same file —
//! that constant feeds `computeMargin`, which is used by the *projectile*
//! overload of `getEntityHitResult` (a moving arrow's forgiveness ramp), not by
//! the crosshair pick. So a mob is swept at its exact hitbox with no
//! forgiveness at all.
//!
//! The tie-break is nearest-first on `from.distanceToSqr(clipPoint)` with a
//! **strict** `<`, seeded at `maxValue` — so the range bound and the tie-break
//! are the same comparison, and an entity whose clip point sits exactly on the
//! bound loses to it.
//!
//! Two arms of that loop read strangely and are transcribed as written:
//!
//! * **An entity containing the eye** short-circuits to `nearest = 0.0` if it
//!   `canBePickedFromInside()`, taking `clipPoint.orElse(from)` as the
//!   location. `AABB.clip` only tests the *near* face of each slab, so a
//!   segment starting inside a box clips nothing and the `orElse` is the live
//!   branch.
//! * **A candidate sharing the source's root vehicle** is skipped — but only
//!   because `nearest != 0.0` in the ordinary case. The guard is
//!   `dd < nearest || nearest == 0.0`, and once an inside-pick has set
//!   `nearest` to zero the same-vehicle arm assigns `hovered` *without*
//!   updating `nearest`. That is what the source does; it is not what the
//!   surrounding code reads like it should do.
//!
//! # What this module does not model
//!
//! * **The `AttackRange` branch.** `raycastHitResult` runs an entirely
//!   different algorithm first — `ProjectileUtil.getHitEntitiesAlong` plus
//!   `getManyEntityHitResult`, with a minimum reach, a motion-dependent
//!   maximum and a two-stage re-clip — when the *active item* carries a
//!   `minecraft:attack_range` component. In vanilla 26.2 that is the spear
//!   builder and nothing else, so the branch is inert unless a spear is held
//!   or in use. While one is, this falls back to the ordinary pick, whose
//!   reach and margin differ (a spear reaches 4.5, or 6.5 in creative, from a
//!   2.0 minimum with a 0.125 hitbox margin).
//! * **Per-type pose and baby dimensions.** [`bounding_box`] implements the
//!   chain in `Entity`/`LivingEntity`/`Avatar` — the type's `sized(w, h)`, the
//!   `SLEEPING` substitution, `Avatar`'s pose map, `getAgeScale()` and the
//!   `SCALE` attribute — but **not** the 47 per-class `getDefaultDimensions`
//!   overrides. Thirty of those substitute an explicit `BABY_DIMENSIONS`
//!   constant that is only sometimes the adult box halved (a baby cow's
//!   0.45×0.7 is exactly half; a baby chicken's 0.3×0.4 is not half of
//!   0.4×0.7), and the rest are situational (a sitting fox, a puffed
//!   pufferfish, a peeking shulker, an emerging warden). The error is a box of
//!   the wrong size on those entities, never a wrong *algorithm*.

use crate::attributes::{resolve, EntityAttributes};

/// `net.minecraft.world.phys.AABB`, as much of it as the pick reads.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aabb {
    pub min: [f64; 3],
    pub max: [f64; 3],
}

/// `AABB.clip`'s perpendicular slack, and `getDirection`'s zero-delta cut-off.
const EPSILON: f64 = 1.0E-7;

impl Aabb {
    pub fn new(min: [f64; 3], max: [f64; 3]) -> Aabb {
        Aabb { min, max }
    }

    /// `AABB.inflate(d)` — all six faces move outwards. A negative `d`
    /// deflates, exactly as vanilla's `deflate` is `inflate(-d)`.
    pub fn inflate(self, d: f64) -> Aabb {
        Aabb {
            min: [self.min[0] - d, self.min[1] - d, self.min[2] - d],
            max: [self.max[0] + d, self.max[1] + d, self.max[2] + d],
        }
    }

    /// `AABB.expandTowards(x, y, z)` — grows the box along the delta only,
    /// moving `min` for a negative component and `max` for a positive one.
    pub fn expand_towards(self, d: [f64; 3]) -> Aabb {
        let mut out = self;
        for a in 0..3 {
            if d[a] < 0.0 {
                out.min[a] += d[a];
            } else if d[a] > 0.0 {
                out.max[a] += d[a];
            }
        }
        out
    }

    /// `AABB.contains(x, y, z)` — **half-open**: `>= min` and `< max` on every
    /// axis. A point exactly on the far face is outside.
    pub fn contains(&self, p: [f64; 3]) -> bool {
        (0..3).all(|a| p[a] >= self.min[a] && p[a] < self.max[a])
    }

    /// `AABB.intersects(other)` — strict on every axis, so two boxes that
    /// merely touch do not intersect.
    pub fn intersects(&self, other: &Aabb) -> bool {
        (0..3).all(|a| self.min[a] < other.max[a] && self.max[a] > other.min[a])
    }

    /// `AABB.clip(from, to)` — the nearest point at which the segment enters
    /// this box, or `None`.
    ///
    /// Verbatim `getDirection` + `clipPoint`: each axis contributes only its
    /// **near** face (the `min` face when the ray runs positive along that
    /// axis, the `max` face when it runs negative), the candidate parameter
    /// must satisfy `0 < s < scale` with `scale` the best found so far
    /// (initially 1.0, which is why the far endpoint is excluded), and the
    /// crossing point is tested against the other two slabs with `EPSILON`
    /// slack on each side.
    ///
    /// Because only near faces are tested, a segment whose origin is inside
    /// the box yields `None` — every near-face parameter is negative. The
    /// caller's `contains(from)` branch exists for exactly that reason.
    pub fn clip(&self, from: [f64; 3], to: [f64; 3]) -> Option<[f64; 3]> {
        let d = [to[0] - from[0], to[1] - from[1], to[2] - from[2]];
        let mut scale = 1.0f64;
        let mut hit = false;
        // Axis order is X, Y, Z — it cannot change the answer (the loop keeps
        // the smallest qualifying `s` regardless of visit order) but it is the
        // order the source walks.
        for a in 0..3 {
            let (b, c) = ((a + 1) % 3, (a + 2) % 3);
            let point = if d[a] > EPSILON {
                self.min[a]
            } else if d[a] < -EPSILON {
                self.max[a]
            } else {
                continue;
            };
            let s = (point - from[a]) / d[a];
            let pb = from[b] + s * d[b];
            let pc = from[c] + s * d[c];
            if 0.0 < s
                && s < scale
                && self.min[b] - EPSILON < pb
                && pb < self.max[b] + EPSILON
                && self.min[c] - EPSILON < pc
                && pc < self.max[c] + EPSILON
            {
                scale = s;
                hit = true;
            }
        }
        hit.then(|| {
            [
                from[0] + scale * d[0],
                from[1] + scale * d[1],
                from[2] + scale * d[2],
            ]
        })
    }
}

fn distance_sq(a: [f64; 3], b: [f64; 3]) -> f64 {
    let (dx, dy, dz) = (b[0] - a[0], b[1] - a[1], b[2] - a[2]);
    dx * dx + dy * dy + dz * dz
}

/// `Pose.SLEEPING`'s id. `Pose.BY_ID` is `ByIdMap.continuous(…, ZERO)`, so any
/// id outside the enum reads as `STANDING` — which is also what an
/// un-decoded pose gives, so nothing here needs a fallback branch.
pub const POSE_SLEEPING: u8 = 2;

/// `LivingEntity.SLEEPING_DIMENSIONS` — `EntityDimensions.fixed(0.2F, 0.2F)`.
/// **Fixed**, so `scale()` returns it unchanged and neither the `SCALE`
/// attribute nor `getAgeScale()` moves it.
const SLEEPING_SIZE: (f32, f32) = (0.2, 0.2);

/// `Avatar.POSES`, and `Avatar.STANDING_DIMENSIONS` as the `getOrDefault`
/// fallback. `DYING` is `EntityDimensions.fixed`; everything else is
/// `scalable`. `SLEEPING` is in the map too but never reached through it —
/// `LivingEntity.getDimensions` short-circuits on that pose first.
fn avatar_pose_size(pose: u8) -> ((f32, f32), bool) {
    match pose {
        1 | 3 | 4 => ((0.6, 0.6), false), // FALL_FLYING, SWIMMING, SPIN_ATTACK
        5 => ((0.6, 1.5), false),         // CROUCHING
        7 => ((0.2, 0.2), true),          // DYING — fixed
        POSE_SLEEPING => (SLEEPING_SIZE, true),
        _ => ((0.6, 1.8), false), // STANDING, and getOrDefault for the rest
    }
}

/// Everything [`bounding_box`] reads about one entity's size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DimensionInputs {
    /// The registered type's `EntityType.Builder.sized(w, h)`.
    pub width: f32,
    pub height: f32,
    /// Whether the implementation class descends from `LivingEntity`. Decides
    /// which `getDimensions(Pose)` runs: `Entity`'s ignores the pose entirely.
    pub living: bool,
    /// Whether it descends from `Avatar` — the only branch with a pose map.
    pub avatar: bool,
    /// `Entity.getPose()`, metadata index 6.
    pub pose: u8,
    /// `LivingEntity.isBaby()` — `getAgeScale()` is `0.5F` when set.
    pub baby: bool,
    /// `LivingEntity.getScale()`, the resolved `minecraft:scale` attribute.
    /// `1.0` for anything non-living, which has no attribute map at all.
    pub scale: f32,
}

/// `Entity.getBoundingBox()` at `pos` — `getDimensions(getPose())` fed through
/// `EntityDimensions.makeBoundingBox`.
///
/// ```text
/// w = width / 2
/// AABB(x - w, y, z - w,  x + w, y + height, z + w)
/// ```
///
/// The box sits **on** `y`, not centred on it: an entity's position is its
/// feet. Getting that wrong puts every hitbox half a body too low and is
/// invisible in a render, because Rewo's models are placed from the same
/// origin and would agree with the mistake.
pub fn bounding_box(pos: [f64; 3], d: &DimensionInputs) -> Aabb {
    let (w, h) = if !d.living {
        // `Entity.getDimensions(pose)` is `this.type.getDimensions()` — the
        // pose is accepted and ignored, and **no scale is applied at all**:
        // `getScale()` and `getAgeScale()` are both `LivingEntity` members.
        (d.width, d.height)
    } else {
        let (size, fixed) = if d.pose == POSE_SLEEPING {
            // `LivingEntity.getDimensions`'s own short-circuit, ahead of both
            // the Avatar map and every per-type override.
            (SLEEPING_SIZE, true)
        } else if d.avatar {
            // `Avatar.getDefaultDimensions` replaces the type's dimensions
            // outright, and applies no age scale.
            avatar_pose_size(d.pose)
        } else {
            // `LivingEntity.getDefaultDimensions` =
            // `type.getDimensions().scale(getAgeScale())`.
            let age = if d.baby { 0.5 } else { 1.0 };
            ((d.width * age, d.height * age), false)
        };
        // `.scale(getScale())`, which `EntityDimensions.scale` refuses to
        // apply to a `fixed` record — so a sleeping or dying entity keeps
        // 0.2×0.2 however the server scales it.
        if fixed || d.scale == 1.0 {
            size
        } else {
            (size.0 * d.scale, size.1 * d.scale)
        }
    };
    // Vanilla halves the width **as a float** and widens it only inside the
    // `AABB` constructor, so a 0.6-wide mob's half-width is 0.30000001192…,
    // not 0.3. Computing it in f64 here would disagree in the last bits with
    // every distance the sweep measures.
    let hw = (w / 2.0) as f64;
    Aabb {
        min: [pos[0] - hw, pos[1], pos[2] - hw],
        max: [pos[0] + hw, pos[1] + h as f64, pos[2] + hw],
    }
}

/// One entity the sweep may hit, with every per-entity predicate already
/// evaluated by the caller.
///
/// A struct of resolved values rather than callbacks, for the same reason
/// `label::LabelInputs` is: the gate can drive every combination directly, and
/// exactly one place has to know where each value comes from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Candidate {
    /// The network entity id, which is what the caller wants back.
    pub id: i32,
    /// `entity.getBoundingBox()`.
    pub bb: Aabb,
    /// `EntitySelector.CAN_BE_PICKED`, i.e. `Entity.isPickable()`.
    ///
    /// Note the selector is **not** `isAlive() && isPickable()` — that pairing
    /// is `canBeHitByProjectile`. A dying-but-not-removed mob is pickable.
    pub pickable: bool,
    /// `entity.getPickRadius()` — `0.0` for everything but a `Projectile`.
    pub pick_radius: f64,
    /// `entity.canBePickedFromInside()` — `true` except a `SulfurCube`
    /// carrying a body item.
    pub can_be_picked_from_inside: bool,
    /// `entity.getRootVehicle() == except.getRootVehicle()` — you, your
    /// vehicle, and anything else riding it.
    pub shares_root_vehicle: bool,
}

/// What the pick found.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EntityHit {
    pub id: i32,
    /// `EntityHitResult.getLocation()` — where the segment met the box.
    pub location: [f64; 3],
    /// `from.distanceToSqr(location)`, kept because both the block tie-break
    /// and `filterHitResult` compare against it.
    pub distance_sq: f64,
}

/// `ProjectileUtil.getEntityHitResult(except, from, to, box, matching, maxValue)`
/// — the six-argument overload the crosshair pick uses.
///
/// `search` is the broad-phase box vanilla passes to `level.getEntities`; it
/// can only remove candidates, never add one, and is applied here so the
/// transcription is complete rather than merely equivalent for the cases that
/// happen to arise. Candidates whose `pickable` is false are the `matching`
/// predicate and are dropped the same way.
pub fn entity_hit_result(
    from: [f64; 3],
    to: [f64; 3],
    search: &Aabb,
    candidates: &[Candidate],
    max_value: f64,
) -> Option<EntityHit> {
    let mut nearest = max_value;
    let mut hovered: Option<EntityHit> = None;

    for c in candidates {
        // `level.getEntities(except, box, matching)` — the caller has already
        // excluded `except` itself by not listing it.
        if !c.pickable || !c.bb.intersects(search) {
            continue;
        }
        let bb = c.bb.inflate(c.pick_radius);
        let clip_point = bb.clip(from, to);
        if bb.contains(from) {
            if nearest >= 0.0 && c.can_be_picked_from_inside {
                let location = clip_point.unwrap_or(from);
                hovered = Some(EntityHit {
                    id: c.id,
                    location,
                    distance_sq: distance_sq(from, location),
                });
                nearest = 0.0;
            }
        } else if let Some(location) = clip_point {
            let dd = distance_sq(from, location);
            if dd < nearest || nearest == 0.0 {
                if c.shares_root_vehicle {
                    if nearest == 0.0 {
                        hovered = Some(EntityHit {
                            id: c.id,
                            location,
                            distance_sq: dd,
                        });
                    }
                } else {
                    hovered = Some(EntityHit {
                        id: c.id,
                        location,
                        distance_sq: dd,
                    });
                    nearest = dd;
                }
            }
        }
    }
    hovered
}

/// The two `RangedAttribute`s `LocalPlayer.raycastHitResult` reads.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct InteractionRanges {
    /// `Attributes.BLOCK_INTERACTION_RANGE` — `RangedAttribute(4.5, [0, 64])`.
    pub block: f64,
    /// `Attributes.ENTITY_INTERACTION_RANGE` — `RangedAttribute(3.0, [0, 64])`.
    pub entity: f64,
}

impl InteractionRanges {
    /// `Player.blockInteractionRange()` / `entityInteractionRange()`, which are
    /// `getAttributeValue(…)` and nothing else.
    ///
    /// Resolved through M55's attribute machinery, so a server's modifiers and
    /// the attribute's own `[0, 64]` clamp both apply — creative mode is itself
    /// a `+2.0 ADD_VALUE` modifier on the entity range
    /// (`Player.CREATIVE_ENTITY_INTERACTION_RANGE_MODIFIER_VALUE`), not a
    /// special case in the pick. Hard-coding 3.0/4.5 would make a creative
    /// player's crosshair stop reaching two blocks short of where it does, and
    /// would look exactly like a bug in the sweep.
    ///
    /// Falls back to the attribute's registered default only when the registry
    /// itself cannot answer — which is the same fail-closed answer
    /// `attributes::resolve` gives, not a guess.
    pub fn resolve(
        attrs: Option<&EntityAttributes>,
        entity_name: Option<&str>,
        reg: &rewo_data::attributes::AttributeRegistry,
    ) -> InteractionRanges {
        let one = |name: &str, fallback: f64| {
            resolve(attrs, entity_name, name, reg).map_or(fallback, |(v, _)| v)
        };
        InteractionRanges {
            block: one("block_interaction_range", DEFAULT_BLOCK_INTERACTION_RANGE),
            entity: one("entity_interaction_range", DEFAULT_ENTITY_INTERACTION_RANGE),
        }
    }

    /// `Math.max(blockInteractionRange, entityInteractionRange)` — how far the
    /// **block** ray is cast before either result is considered.
    pub fn max(self) -> f64 {
        self.block.max(self.entity)
    }
}

/// `Player.DEFAULT_BLOCK_INTERACTION_RANGE`, which is also the attribute's
/// registered default.
pub const DEFAULT_BLOCK_INTERACTION_RANGE: f64 = 4.5;
/// `Player.DEFAULT_ENTITY_INTERACTION_RANGE`.
pub const DEFAULT_ENTITY_INTERACTION_RANGE: f64 = 3.0;

/// Everything `LocalPlayer.pick` reads.
#[derive(Clone, Copy, Debug)]
pub struct PickInputs<'a> {
    /// `cameraEntity.getEyePosition(partialTicks)`.
    pub eye: [f64; 3],
    /// `cameraEntity.getViewVector(partialTicks)` — unit length.
    pub dir: [f64; 3],
    /// `cameraEntity.getBoundingBox()`, the seed of the broad-phase box.
    pub camera_bb: Aabb,
    pub ranges: InteractionRanges,
    /// Distance from the eye to the block hit, when the block ray — cast to
    /// [`InteractionRanges::max`], **not** to the block range — hit something.
    /// `None` is `HitResult.Type.MISS`, for which vanilla's
    /// `BlockHitResult.miss(to, …)` puts the location at the far endpoint, so
    /// `blockDistanceSq` is `maxDistance²`.
    pub block_hit_distance: Option<f64>,
    pub candidates: &'a [Candidate],
}

/// `LocalPlayer.pick` — the entity under the crosshair, or `None`.
///
/// `None` covers all three of vanilla's ways of not having one: no entity was
/// swept, the block hit was nearer, and the entity hit was outside
/// `entityInteractionRange` (which `filterHitResult` rewrites into a
/// `BlockHitResult.miss`, and `Minecraft.pick`'s `instanceof EntityHitResult`
/// then reads as null).
pub fn crosshair_pick(i: &PickInputs) -> Option<EntityHit> {
    let mut max_distance = i.ranges.max();
    let mut max_distance_sq = max_distance * max_distance;
    // `blockHitResult.getLocation().distanceToSqr(from)` — on a MISS the
    // location is the far endpoint, so this is the untruncated range squared.
    let block_distance_sq = match i.block_hit_distance {
        Some(d) => {
            max_distance_sq = d * d;
            max_distance = d;
            max_distance_sq
        }
        None => max_distance_sq,
    };
    let delta = [
        i.dir[0] * max_distance,
        i.dir[1] * max_distance,
        i.dir[2] * max_distance,
    ];
    let to = [i.eye[0] + delta[0], i.eye[1] + delta[1], i.eye[2] + delta[2]];
    // `cameraEntity.getBoundingBox().expandTowards(direction.scale(maxDistance)).inflate(1.0)`.
    let search = i.camera_bb.expand_towards(delta).inflate(1.0);
    let hit = entity_hit_result(i.eye, to, &search, i.candidates, max_distance_sq)?;
    if hit.distance_sq >= block_distance_sq {
        // The block won. `filterHitResult(blockHitResult, …)` returns a
        // `BlockHitResult` either way, so there is no crosshair entity.
        return None;
    }
    // `filterHitResult(entityHitResult, from, entityInteractionRange)` —
    // `Vec3.closerThan` is `distanceToSqr < distance * distance`, strict.
    (hit.distance_sq < i.ranges.entity * i.ranges.entity).then_some(hit)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn box_at(x: f64, y: f64, z: f64, w: f64, h: f64) -> Aabb {
        Aabb::new([x - w / 2.0, y, z - w / 2.0], [x + w / 2.0, y + h, z + w / 2.0])
    }

    fn candidate(id: i32, bb: Aabb) -> Candidate {
        Candidate {
            id,
            bb,
            pickable: true,
            pick_radius: 0.0,
            can_be_picked_from_inside: true,
            shares_root_vehicle: false,
        }
    }

    /// An eye at the origin looking east, with the default ranges.
    fn inputs<'a>(candidates: &'a [Candidate], block: Option<f64>) -> PickInputs<'a> {
        PickInputs {
            eye: [0.0, 0.0, 0.0],
            dir: [1.0, 0.0, 0.0],
            camera_bb: box_at(0.0, -1.62, 0.0, 0.6, 1.8),
            ranges: InteractionRanges {
                block: DEFAULT_BLOCK_INTERACTION_RANGE,
                entity: DEFAULT_ENTITY_INTERACTION_RANGE,
            },
            block_hit_distance: block,
            candidates,
        }
    }

    #[test]
    fn a_mob_ahead_is_picked_and_one_behind_is_not() {
        let ahead = [candidate(7, box_at(2.0, -1.0, 0.0, 0.6, 1.8))];
        assert_eq!(crosshair_pick(&inputs(&ahead, None)).unwrap().id, 7);
        let behind = [candidate(7, box_at(-2.0, -1.0, 0.0, 0.6, 1.8))];
        assert!(crosshair_pick(&inputs(&behind, None)).is_none());
    }

    #[test]
    fn the_entity_range_bound_is_strict_and_sampled_on_the_bound() {
        // The near face of a 0.6-wide box centred at x is `x - 0.3`, so a box
        // centred at 3.3 has its clip point at exactly 3.0 — the bound.
        // `Vec3.closerThan` is `<`, so that is out.
        let on = [candidate(1, box_at(3.3, -1.0, 0.0, 0.6, 1.8))];
        assert!(
            crosshair_pick(&inputs(&on, None)).is_none(),
            "a clip point exactly on entityInteractionRange must be excluded"
        );
        let inside = [candidate(1, box_at(3.3 - 1e-6, -1.0, 0.0, 0.6, 1.8))];
        assert!(crosshair_pick(&inputs(&inside, None)).is_some());
    }

    #[test]
    fn the_range_comes_from_the_attribute_not_a_constant() {
        // The creative modifier is `+2.0` on the entity range. A mob at 4
        // blocks is out of reach at the default and in reach with it.
        let far = [candidate(1, box_at(4.0, -1.0, 0.0, 0.6, 1.8))];
        assert!(crosshair_pick(&inputs(&far, None)).is_none());
        let mut creative = inputs(&far, None);
        creative.ranges.entity = 5.0;
        assert_eq!(crosshair_pick(&creative).unwrap().id, 1);
    }

    #[test]
    fn a_block_in_front_of_the_mob_wins_and_a_dead_heat_goes_to_the_block() {
        let mob = [candidate(1, box_at(2.0, -1.0, 0.0, 0.6, 1.8))];
        // Clip point is x = 1.7. A block at 1.5 is nearer.
        assert!(crosshair_pick(&inputs(&mob, Some(1.5))).is_none());
        // A block at 2.5 is further, so the mob survives.
        assert_eq!(crosshair_pick(&inputs(&mob, Some(2.5))).unwrap().id, 1);
        // Exactly level: the comparison is `entityDistSq < blockDistSq`.
        assert!(
            crosshair_pick(&inputs(&mob, Some(1.7))).is_none(),
            "a tie must go to the block"
        );
    }

    #[test]
    fn the_block_hit_truncates_the_sweep_as_well_as_winning_the_tie() {
        // Two mobs, one behind the block. Without truncation the far one would
        // still be swept and merely lose; with it, it is never a candidate.
        let mobs = [
            candidate(1, box_at(4.0, -1.0, 0.0, 0.6, 1.8)),
            candidate(2, box_at(2.0, -1.0, 0.0, 0.6, 1.8)),
        ];
        let hit = crosshair_pick(&inputs(&mobs, Some(3.0))).unwrap();
        assert_eq!(hit.id, 2);
    }

    #[test]
    fn the_nearest_of_two_candidates_wins_in_either_list_order() {
        let near = candidate(1, box_at(1.5, -1.0, 0.0, 0.6, 1.8));
        let far = candidate(2, box_at(2.5, -1.0, 0.0, 0.6, 1.8));
        assert_eq!(crosshair_pick(&inputs(&[near, far], None)).unwrap().id, 1);
        assert_eq!(crosshair_pick(&inputs(&[far, near], None)).unwrap().id, 1);
    }

    #[test]
    fn an_unpickable_entity_is_never_returned() {
        let mut c = candidate(1, box_at(2.0, -1.0, 0.0, 0.6, 1.8));
        assert!(crosshair_pick(&inputs(&[c], None)).is_some());
        c.pickable = false;
        assert!(crosshair_pick(&inputs(&[c], None)).is_none());
    }

    #[test]
    fn the_pick_radius_is_the_inflation_and_it_is_zero_for_a_mob() {
        // A ray grazing 0.5 to the side of a 0.6-wide box misses it outright;
        // a projectile's 1.0 pick radius is what would catch it.
        let mut c = candidate(1, box_at(2.0, -0.5, 0.5, 0.6, 1.0));
        assert!(
            crosshair_pick(&inputs(&[c], None)).is_none(),
            "no forgiveness at pick radius 0"
        );
        c.pick_radius = 1.0;
        assert_eq!(crosshair_pick(&inputs(&[c], None)).unwrap().id, 1);
    }

    #[test]
    fn an_entity_containing_the_eye_is_picked_from_inside_only_if_it_allows_it() {
        let mut c = candidate(1, box_at(0.0, -1.0, 0.0, 4.0, 4.0));
        assert!(c.bb.contains([0.0, 0.0, 0.0]));
        let hit = crosshair_pick(&inputs(&[c], None)).unwrap();
        assert_eq!(hit.id, 1);
        assert_eq!(hit.location, [0.0, 0.0, 0.0], "clip is empty from inside");
        c.can_be_picked_from_inside = false;
        assert!(crosshair_pick(&inputs(&[c], None)).is_none());
    }

    #[test]
    fn a_candidate_sharing_the_root_vehicle_is_skipped() {
        let mut c = candidate(1, box_at(2.0, -1.0, 0.0, 0.6, 1.8));
        assert!(crosshair_pick(&inputs(&[c], None)).is_some());
        c.shares_root_vehicle = true;
        assert!(crosshair_pick(&inputs(&[c], None)).is_none());
    }

    #[test]
    fn the_sweep_bound_is_the_larger_range_and_the_entity_range_filters_after() {
        // A mob at 4 blocks is inside `max(4.5, 3.0)` and outside 3.0. It must
        // be *found* (so it can shadow a further one) and then discarded.
        let far = candidate(1, box_at(4.0, -1.0, 0.0, 0.6, 1.8));
        let search = Aabb::new([-0.3, -1.62, -0.3], [0.3, 0.18, 0.3])
            .expand_towards([4.5, 0.0, 0.0])
            .inflate(1.0);
        let found = entity_hit_result(
            [0.0, 0.0, 0.0],
            [4.5, 0.0, 0.0],
            &search,
            &[far],
            4.5 * 4.5,
        );
        assert!(found.is_some(), "swept to the larger of the two ranges");
        assert!(crosshair_pick(&inputs(&[far], None)).is_none(), "then filtered");
    }

    #[test]
    fn clip_is_half_open_at_the_far_endpoint() {
        // `s < scale` with scale seeded at 1.0 excludes the segment's own end.
        let bb = Aabb::new([1.0, -1.0, -1.0], [2.0, 1.0, 1.0]);
        assert!(bb.clip([0.0, 0.0, 0.0], [1.0, 0.0, 0.0]).is_none());
        assert!(bb.clip([0.0, 0.0, 0.0], [1.5, 0.0, 0.0]).is_some());
    }

    #[test]
    fn a_bounding_box_stands_on_its_position() {
        let d = DimensionInputs {
            width: 0.6,
            height: 1.95,
            living: true,
            avatar: false,
            pose: 0,
            baby: false,
            scale: 1.0,
        };
        let bb = bounding_box([10.0, 64.0, -3.0], &d);
        // The half-width is `0.6F / 2.0F` widened, not `0.6 / 2.0` — the same
        // 0.30000001192… vanilla's `AABB` constructor receives.
        let hw = (0.6f32 / 2.0) as f64;
        assert_eq!(bb.min, [10.0 - hw, 64.0, -3.0 - hw]);
        assert_eq!(bb.max, [10.0 + hw, 64.0 + 1.95f32 as f64, -3.0 + hw]);
        assert_ne!(hw, 0.3, "an f64 halving would disagree in the last bits");
    }

    #[test]
    fn sleeping_replaces_the_box_and_ignores_the_scale_attribute() {
        let mut d = DimensionInputs {
            width: 0.6,
            height: 1.95,
            living: true,
            avatar: false,
            pose: POSE_SLEEPING,
            baby: false,
            scale: 4.0,
        };
        let bb = bounding_box([0.0, 0.0, 0.0], &d);
        assert!(
            (bb.max[1] - 0.2).abs() < 1e-6,
            "SLEEPING_DIMENSIONS is fixed(0.2, 0.2), got {}",
            bb.max[1]
        );
        // The same scale does move a standing box.
        d.pose = 0;
        assert!((bounding_box([0.0, 0.0, 0.0], &d).max[1] - 7.8).abs() < 1e-6);
    }

    #[test]
    fn an_avatar_takes_its_pose_map_and_a_mob_does_not() {
        let mut d = DimensionInputs {
            width: 0.6,
            height: 1.8,
            living: true,
            avatar: true,
            pose: 5, // CROUCHING
            baby: false,
            scale: 1.0,
        };
        assert_eq!(bounding_box([0.0, 0.0, 0.0], &d).max[1], 1.5);
        d.avatar = false;
        assert!((bounding_box([0.0, 0.0, 0.0], &d).max[1] - 1.8).abs() < 1e-6);
    }

    #[test]
    fn a_non_living_entity_ignores_the_pose_entirely() {
        let d = DimensionInputs {
            width: 0.25,
            height: 0.25,
            living: false,
            avatar: false,
            pose: POSE_SLEEPING,
            baby: true,
            scale: 4.0,
        };
        // Every one of `pose`, `baby` and `scale` is set to something that
        // would move a living entity's box; a non-living one reads none of
        // them, because all three live on `LivingEntity`.
        let bb = bounding_box([0.0, 0.0, 0.0], &d);
        assert!(
            (bb.max[1] - 0.25).abs() < 1e-6,
            "Entity.getDimensions takes the pose and drops it, got {}",
            bb.max[1]
        );
    }

    #[test]
    fn a_baby_is_half_size_through_the_age_scale() {
        let mut d = DimensionInputs {
            width: 0.9,
            height: 1.4,
            living: true,
            avatar: false,
            pose: 0,
            baby: true,
            scale: 1.0,
        };
        let bb = bounding_box([0.0, 0.0, 0.0], &d);
        assert!((bb.max[1] - 0.7).abs() < 1e-6);
        // An Avatar has no age scale — `Avatar.getDefaultDimensions` replaces
        // the whole record rather than scaling the type's.
        d.avatar = true;
        assert!((bounding_box([0.0, 0.0, 0.0], &d).max[1] - 1.8).abs() < 1e-6);
    }
}
