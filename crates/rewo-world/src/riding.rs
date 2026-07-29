//! `Entity.positionRider` — where a passenger sits on its vehicle (M72).
//!
//! # The shape of it
//!
//! ```text
//! protected void positionRider(Entity passenger, MoveFunction moveFunction) {
//!    Vec3 position = this.getPassengerRidingPosition(passenger);
//!    Vec3 offset = passenger.getVehicleAttachmentPoint(this);
//!    moveFunction.accept(passenger, position.x - offset.x, …);
//! }
//! ```
//!
//! Two attachment points, from two different types. The **vehicle's**
//! PASSENGER point (rotated by the vehicle's yaw, indexed by the rider's place
//! in the roster) says where the seat is; the **rider's own** VEHICLE point
//! (rotated by the *rider's* yaw) says where on the rider that seat should
//! meet. A player's VEHICLE point is `(0, 0.6, 0)`, so a mounted player is
//! placed 0.6 blocks *below* the seat — which is what puts them in a saddle
//! rather than standing on the horse's head.
//!
//! The points themselves are entity-type data; [`rewo_data::entity_attachments`]
//! owns them. This module owns the arithmetic, because the rotation goes
//! through `Mth`'s 65,536-entry sine table (`Vec3.yRot` calls `Mth.cos`/
//! `Mth.sin`, not platform trig) which lives next door in [`crate::lightmap`].
//!
//! # A passenger does not interpolate
//!
//! This is the part that is easy to get wrong, and vanilla is unambiguous.
//! `ClientLevel.tickEntities` **skips passengers entirely**:
//!
//! ```text
//! if (!entity.isRemoved() && !entity.isPassenger() && …) tickNonPassenger(entity);
//! ```
//!
//! A rider is reached only through its vehicle, by `tickNonPassenger` →
//! `tickPassenger` → `rideTick()`, which ticks it and then calls
//! `getVehicle().positionRider(this)` — an unconditional `setPos`. So the
//! rider's own server-sent position and its own 3-step lerp are computed and
//! then **overwritten every tick**; they never reach the screen.
//!
//! And the renderer does not re-derive per frame either.
//! `EntityRenderer.extractRenderState` is
//! `Mth.lerp(partialTicks, entity.xOld, entity.getX())` for every entity alike
//! — for a rider, a lerp between two *derived* positions, because
//! `tickPassenger` calls `setOldPosAndRot()` before `rideTick()`.
//!
//! Rewo therefore derives into [`EntityState`](crate::entities::EntityState)'s
//! `cur` at the end of the tick, after every entity's ordinary lerp step has
//! already moved `prev = cur`. `render_pos(alpha)` then blends two derived
//! positions, which is vanilla's model exactly. Deriving per frame from the
//! vehicle's *interpolated* position would be a different (and smoother)
//! answer than vanilla's, and interpolating the rider's own synced position
//! independently is the pre-M72 bug: the rider lags its vehicle by up to
//! three ticks and slides around beside it.
//!
//! # Scale
//!
//! `LivingEntity.getPassengerRidingPosition` passes `getScale() * getAgeScale()`
//! and reads `getDimensions(pose)`, whose attachments are already scaled by the
//! same product — so one uniform factor scales both the table point and the
//! override formulas. Rewo supplies `getAgeScale()`, which is exactly
//! `isBaby() ? 0.5F : 1.0F`, and **not** the `minecraft:scale` attribute:
//! Rewo's renderer does not scale any model by that attribute either, so
//! applying it to the seat alone would move the rider off the mount it is
//! drawn on.

use rewo_data::entity_attachments::{Attachments, VehicleClass};

use crate::lightmap::{mth_cos, mth_sin};

/// `Vec3.yRot(radians)` — vanilla's own component order, through `Mth`.
fn y_rot(p: [f64; 3], radians: f32) -> [f64; 3] {
    let (c, s) = (mth_cos(radians) as f64, mth_sin(radians) as f64);
    [p[0] * c + p[2] * s, p[1], p[2] * c - p[0] * s]
}

/// `EntityAttachments.transformPoint(point, rotY)` — note the **negated**
/// degrees, which is what makes a `+z` seat sit behind a vehicle facing `+z`.
fn transform(p: [f64; 3], y_rot_degrees: f32) -> [f64; 3] {
    y_rot(p, -y_rot_degrees * std::f32::consts::PI / 180.0)
}

fn scaled(p: [f64; 3], s: f32) -> [f64; 3] {
    let s = s as f64;
    [p[0] * s, p[1] * s, p[2] * s]
}

/// Everything about the vehicle the derivation reads.
#[derive(Clone, Copy, Debug)]
pub struct VehicleInputs {
    pub type_id: i32,
    /// `Entity.position()` — this tick's, already lerped.
    pub pos: [f64; 3],
    /// `getYRot()`, degrees.
    pub yaw: f32,
    /// `getScale() * getAgeScale()`; see the module docs.
    pub scale: f32,
    /// `getPassengers().size()` — the boat and camel branch on it.
    pub passenger_count: usize,
    /// `AbstractCubeMob.getSize()`. Read only for that class.
    pub cube_size: i32,
    /// `walkAnimation.position()` / `.speed()`. Read only by `Strider`.
    pub limb: (f32, f32),
    /// `isBaby()` — the camel's sit anchor uses a different age offset from
    /// its uniform scale, so the flag is needed as well as the factor.
    pub baby: bool,
}

/// Everything about the rider the derivation reads.
#[derive(Clone, Copy, Debug)]
pub struct RiderInputs {
    pub type_id: i32,
    /// `getYRot()`, degrees — the rider's own, which rotates its VEHICLE point.
    pub yaw: f32,
    pub scale: f32,
    /// `vehicle.getPassengers().indexOf(passenger)`.
    pub index: usize,
}

/// `vehicle.getPassengerRidingPosition(passenger) - vehicle.position()` — the
/// seat, in the vehicle's local frame.
///
/// Every override that replaces the table lookup lives here, selected by the
/// vehicle's [`VehicleClass`] exactly as the JVM selects it by class.
pub fn passenger_attachment_point(
    att: &Attachments,
    v: &VehicleInputs,
    r: &RiderInputs,
) -> Option<[f64; 3]> {
    let vp = att.points(v.type_id)?;
    // The scaled bounding-box height every override reads as
    // `dimensions.height()`.
    let height = (vp.height * v.scale) as f64;
    let default = || transform(scaled(vp.passenger_point(r.index), v.scale), v.yaw);
    Some(match att.class(v.type_id) {
        VehicleClass::Default | VehicleClass::Horse => {
            // `AbstractHorse` adds `(0, 0.15·standAnimO·scale,
            // -0.7·standAnimO·scale)` — the rearing lift. `standAnimO` is a
            // client-simulated stand animation that no packet carries, and it
            // is **0 whenever the horse is not rearing**, which is the term
            // this reduces to. `Llama` overrides the horse back to the plain
            // default, so the two coincide here.
            default()
        }
        VehicleClass::Boat { raft, chest } => {
            // `AbstractBoat` replaces the lookup outright — a boat's declared
            // PASSENGER points (it declares none) are never read.
            let mut offset = if chest { 0.15f32 } else { 0.0 };
            if v.passenger_count > 1 {
                offset = if r.index == 0 { 0.2 } else { -0.6 };
                if att.is_animal(r.type_id) {
                    offset += 0.2;
                }
            }
            // `rideHeight(dimensions)`. The split is by leaf class, not by
            // chest: `Raft` and `ChestRaft` share it across the chest boundary.
            let ride_height = if raft {
                height * 0.8888889f32 as f64
            } else {
                height / 3.0f32 as f64
            };
            // A boat is not a `LivingEntity`, so `Entity`'s
            // `getPassengerRidingPosition` passes the unscaled dimensions and
            // the offset is **not** multiplied by scale — unlike the camel's.
            transform([0.0, ride_height, offset as f64], v.yaw)
        }
        VehicleClass::Minecart => {
            // `LOWERED_PASSENGER_ATTACHMENT` is `Vec3.ZERO` — a villager rides
            // at the cart's own feet, not 0.1875 above them.
            if att.lowers_in_minecart(r.type_id) {
                [0.0, 0.0, 0.0]
            } else {
                default()
            }
        }
        VehicleClass::CubeMob => {
            // `new Vec3(0, dimensions.height() - 0.015625 * getSize() * scale, 0)`
            // — x and z are zero, so the rotation is a no-op and vanilla omits it.
            let size = v.cube_size.clamp(1, 127) as f64;
            [
                0.0,
                (vp.height as f64 * size * v.scale as f64) - 0.015625 * size * v.scale as f64,
                0.0,
            ]
        }
        VehicleClass::Strider => {
            // Client-only, and `isClientSide` is true here by construction.
            let (pos, speed) = v.limb;
            let anim_speed = speed.min(0.25);
            let bob = 0.12 * mth_cos(pos * 1.5) * 2.0 * anim_speed;
            let d = default();
            [d[0], d[1] + (bob * v.scale) as f64, d[2]]
        }
        VehicleClass::Camel => {
            let driver = r.index == 0;
            let mut offset = 0.5f32;
            if v.passenger_count > 1 {
                if !driver {
                    offset = -0.7;
                }
                if att.is_animal(r.type_id) {
                    offset += 0.2;
                }
            }
            // `getBodyAnchorAnimationYOffset`, at rest: `dimensions.height() -
            // (isBaby ? 0.09375 : 0.375)`. Its sitting and pose-transition
            // arms need `LAST_POSE_CHANGE_TICK`, a synced LONG this client does
            // not decode; a standing camel is the term they reduce to, and the
            // alternative — the AT_HEIGHT fallback, since the camel declares no
            // PASSENGER points at all — would put a rider 0.375 blocks above
            // its own back.
            let age_sit = if v.baby { 0.09375 } else { 0.375 };
            transform([0.0, height - age_sit, (offset * v.scale) as f64], v.yaw)
        }
    })
}

/// `passenger.getVehicleAttachmentPoint(vehicle)` — the rider's own offset,
/// rotated by the **rider's** yaw.
pub fn vehicle_attachment_point(
    att: &Attachments,
    v: &VehicleInputs,
    r: &RiderInputs,
) -> Option<[f64; 3]> {
    let rp = att.points(r.type_id)?;
    if att.is_spider(r.type_id) {
        // `Spider.getVehicleAttachmentPoint` — the only override of this half.
        // The comparison is of the *current* widths, so both are scaled.
        let vehicle_width = att.points(v.type_id)?.width * v.scale;
        if vehicle_width <= rp.width * r.scale {
            // Untransformed in vanilla: x and z are zero.
            return Some([0.0, 0.3125 * r.scale as f64, 0.0]);
        }
    }
    Some(transform(scaled(rp.vehicle_point(), r.scale), r.yaw))
}

/// `Entity.positionRider` — the world position the rider is snapped to.
///
/// `None` when either type is not in the registry, rather than a guessed
/// fallback: a rider placed at an invented offset is worse than one left at
/// its own synced position, because the error is silent and constant.
pub fn rider_position(
    att: &Attachments,
    v: &VehicleInputs,
    r: &RiderInputs,
) -> Option<[f64; 3]> {
    let seat = passenger_attachment_point(att, v, r)?;
    let off = vehicle_attachment_point(att, v, r)?;
    Some([
        v.pos[0] + seat[0] - off[0],
        v.pos[1] + seat[1] - off[1],
        v.pos[2] + seat[2] - off[2],
    ])
}

/// The body yaw a vehicle forces onto a living rider, if any.
///
/// `AbstractHorse.positionRider` and `Chicken.positionRider` both end with
/// `if (passenger instanceof LivingEntity l) l.yBodyRot = this.yBodyRot;`.
/// It is why a player on a horse keeps their torso facing the horse's
/// direction while their head turns freely — the head is a separate field and
/// is deliberately not touched.
pub fn forced_body_yaw(att: &Attachments, v: &VehicleInputs, rider_type: i32) -> Option<f32> {
    (att.forces_rider_body_yaw(v.type_id) && att.is_living(rider_type)).then_some(v.yaw)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rewo_data::entity_attachments::TypePoints;

    const PIG: i32 = 1;
    const PLAYER: i32 = 2;
    const BOAT: i32 = 3;
    const GHAST: i32 = 4;
    const SPIDER: i32 = 5;
    const VILLAGER: i32 = 6;
    const CART: i32 = 7;

    static GHAST_SEATS: &[[f64; 3]] = &[
        [0.0, 4.0, 1.7],
        [-1.7, 4.0, 0.0],
        [0.0, 4.0, -1.7],
        [1.7, 4.0, 0.0],
    ];
    static PIG_SEAT: &[[f64; 3]] = &[[0.0, 0.86875, 0.0]];
    static CART_SEAT: &[[f64; 3]] = &[[0.0, 0.1875, 0.0]];

    fn att() -> Attachments {
        let p = |width: f32, height: f32, passenger: &'static [[f64; 3]], vehicle| TypePoints {
            width,
            height,
            passenger,
            vehicle,
        };
        Attachments::from_raw(
            &[
                (PIG, p(0.9, 0.9, PIG_SEAT, None)),
                (PLAYER, p(0.6, 1.8, &[], Some([0.0, 0.6, 0.0]))),
                (BOAT, p(1.375, 0.5625, &[], None)),
                (GHAST, p(4.0, 4.0, GHAST_SEATS, Some([0.0, -0.5, 0.0]))),
                (SPIDER, p(1.4, 0.9, &[[0.0, 0.765, 0.0]], None)),
                (VILLAGER, p(0.6, 1.95, &[], None)),
                (CART, p(0.98, 0.7, CART_SEAT, None)),
            ],
            &[
                (BOAT, VehicleClass::Boat { raft: false, chest: false }),
                (CART, VehicleClass::Minecart),
            ],
            &[PIG],
            &[VILLAGER],
            &[SPIDER],
            &[PIG],
            &[PLAYER, PIG, SPIDER, VILLAGER, GHAST],
        )
    }

    fn vehicle(type_id: i32, pos: [f64; 3], yaw: f32, count: usize) -> VehicleInputs {
        VehicleInputs {
            type_id,
            pos,
            yaw,
            scale: 1.0,
            passenger_count: count,
            cube_size: 1,
            limb: (0.0, 0.0),
            baby: false,
        }
    }

    fn rider(type_id: i32, yaw: f32, index: usize) -> RiderInputs {
        RiderInputs { type_id, yaw, scale: 1.0, index }
    }

    #[test]
    fn a_player_on_a_pig_sits_at_the_seat_minus_its_own_vehicle_point() {
        let a = att();
        let p = rider_position(&a, &vehicle(PIG, [10.0, 64.0, -3.0], 0.0, 1), &rider(PLAYER, 0.0, 0))
            .expect("both types registered");
        // 0.86875 seat − 0.6 the player's own VEHICLE point.
        assert!((p[0] - 10.0).abs() < 1e-9);
        assert!((p[1] - (64.0 + 0.86875 - 0.6)).abs() < 1e-9, "{p:?}");
        assert!((p[2] - -3.0).abs() < 1e-9);
    }

    #[test]
    fn the_seat_rotates_with_the_vehicle_yaw() {
        let a = att();
        // The chicken-style `-z` seat, expressed on the ghast's front seat.
        let at0 = rider_position(&a, &vehicle(GHAST, [0.0, 0.0, 0.0], 0.0, 1), &rider(PIG, 0.0, 0))
            .unwrap();
        let at90 =
            rider_position(&a, &vehicle(GHAST, [0.0, 0.0, 0.0], 90.0, 1), &rider(PIG, 0.0, 0))
                .unwrap();
        // Seat 0 is `+1.7 z` at yaw 0 …
        assert!((at0[2] - 1.7).abs() < 1e-4, "{at0:?}");
        assert!(at0[0].abs() < 1e-4);
        // … and a quarter turn moves it onto `-x`, not `+x`. The sign is what
        // `transformPoint`'s negated degrees decides.
        assert!((at90[0] - -1.7).abs() < 1e-4, "{at90:?}");
        assert!(at90[2].abs() < 1e-4, "{at90:?}");
    }

    #[test]
    fn the_second_passenger_takes_the_second_seat() {
        let a = att();
        let v = vehicle(GHAST, [0.0, 0.0, 0.0], 0.0, 2);
        let first = rider_position(&a, &v, &rider(PLAYER, 0.0, 0)).unwrap();
        let second = rider_position(&a, &v, &rider(PLAYER, 0.0, 1)).unwrap();
        assert_ne!(first, second, "two riders must not stack on seat 0");
        assert!((second[0] - -1.7).abs() < 1e-4, "{second:?}");
        // And a fifth clamps onto the fourth rather than wrapping.
        let fifth = rider_position(&a, &v, &rider(PLAYER, 0.0, 4)).unwrap();
        let fourth = rider_position(&a, &v, &rider(PLAYER, 0.0, 3)).unwrap();
        assert_eq!(fifth, fourth);
    }

    #[test]
    fn the_riders_own_yaw_rotates_its_vehicle_point_not_the_vehicles() {
        let a = att();
        // The ghast's VEHICLE point is `(0, -0.5, 0)` — a pure `y`, so its own
        // rotation is invisible. Use the player's and a seat that is off-axis
        // to prove the two rotations are independent: turning the RIDER must
        // not move a rider whose own point is on the `y` axis.
        let v = vehicle(GHAST, [0.0, 0.0, 0.0], 0.0, 1);
        let a0 = rider_position(&a, &v, &rider(PLAYER, 0.0, 0)).unwrap();
        let a90 = rider_position(&a, &v, &rider(PLAYER, 90.0, 0)).unwrap();
        assert_eq!(a0, a90, "a y-axis VEHICLE point is rotation-invariant");
    }

    #[test]
    fn a_boat_replaces_the_lookup_rather_than_adding_to_it() {
        let a = att();
        // `rideHeight` = height/3 = 0.1875, NOT the AT_HEIGHT fallback 0.5625
        // the boat's empty declaration would otherwise produce.
        let one = rider_position(&a, &vehicle(BOAT, [0.0, 0.0, 0.0], 0.0, 1), &rider(PLAYER, 0.0, 0))
            .unwrap();
        assert!((one[1] - (0.5625 / 3.0 - 0.6)).abs() < 1e-6, "{one:?}");
        assert!(one[2].abs() < 1e-6, "a lone passenger sits amidships");
        // Two passengers split fore/aft …
        let v2 = vehicle(BOAT, [0.0, 0.0, 0.0], 0.0, 2);
        let fore = rider_position(&a, &v2, &rider(PLAYER, 0.0, 0)).unwrap();
        let aft = rider_position(&a, &v2, &rider(PLAYER, 0.0, 1)).unwrap();
        assert!((fore[2] - 0.2).abs() < 1e-6, "{fore:?}");
        assert!((aft[2] - -0.6).abs() < 1e-6, "{aft:?}");
        // … and an animal in the aft seat is nudged forward by 0.2.
        let pig_aft = rider_position(&a, &v2, &rider(PIG, 0.0, 1)).unwrap();
        assert!((pig_aft[2] - -0.4).abs() < 1e-6, "{pig_aft:?}");
    }

    #[test]
    fn a_villager_in_a_minecart_rides_at_the_carts_feet() {
        let a = att();
        let v = vehicle(CART, [0.0, 70.0, 0.0], 0.0, 1);
        let villager = rider_position(&a, &v, &rider(VILLAGER, 0.0, 0)).unwrap();
        let player = rider_position(&a, &v, &rider(PLAYER, 0.0, 0)).unwrap();
        assert!((villager[1] - 70.0).abs() < 1e-9, "{villager:?}");
        // The mutation partner: anyone else takes the declared 0.1875.
        assert!((player[1] - (70.0 + 0.1875 - 0.6)).abs() < 1e-9, "{player:?}");
    }

    #[test]
    fn a_spider_overrides_its_own_vehicle_point_only_under_a_narrower_mount() {
        let a = att();
        // Pig width 0.9 <= spider width 1.4 → the override, 0.3125.
        let on_pig = rider_position(&a, &vehicle(PIG, [0.0, 0.0, 0.0], 0.0, 1), &rider(SPIDER, 0.0, 0))
            .unwrap();
        assert!((on_pig[1] - (0.86875 - 0.3125)).abs() < 1e-9, "{on_pig:?}");
        // Ghast width 4.0 > 1.4 → `super`, and the spider declares no VEHICLE
        // point, so AT_FEET: zero.
        let on_ghast =
            rider_position(&a, &vehicle(GHAST, [0.0, 0.0, 0.0], 0.0, 1), &rider(SPIDER, 0.0, 0))
                .unwrap();
        assert!((on_ghast[1] - 4.0).abs() < 1e-9, "{on_ghast:?}");
    }

    #[test]
    fn an_unregistered_type_yields_no_position_rather_than_a_guess() {
        let a = att();
        assert!(rider_position(&a, &vehicle(999, [0.0; 3], 0.0, 1), &rider(PLAYER, 0.0, 0)).is_none());
        assert!(rider_position(&a, &vehicle(PIG, [0.0; 3], 0.0, 1), &rider(999, 0.0, 0)).is_none());
    }

    #[test]
    fn only_a_forcing_vehicle_with_a_living_rider_forces_a_body_yaw() {
        let a = att();
        // PIG stands in for the horse/chicken set in this fixture.
        let pig = vehicle(PIG, [0.0; 3], 37.0, 1);
        assert_eq!(forced_body_yaw(&a, &pig, PLAYER), Some(37.0));
        // Mutation partner: a vehicle that does not force it.
        assert_eq!(forced_body_yaw(&a, &vehicle(GHAST, [0.0; 3], 37.0, 1), PLAYER), None);
        // And `instanceof LivingEntity` — a boat riding a pig would not be
        // turned, because `positionRider`'s cast fails.
        assert_eq!(forced_body_yaw(&a, &pig, BOAT), None);
    }
}
